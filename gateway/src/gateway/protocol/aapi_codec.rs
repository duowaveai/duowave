//! Codec for binary "AAPI" frames: the protobuf flavour of the protocol used by current firmware. A frame is
//! `AAPI` and a ClientMessage (from the station) or a ServerMessage (to it).
//!
//! Frames are read and written through protobufjs's plain objects (proto_object.rs), as the TS codec did: the
//! `decoded` object of a client frame and the `encoded` object of a server frame are what the journal keeps, and
//! the bytes on the wire are the ones protobufjs wrote.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use bytes::Bytes;
use prost_reflect::{FieldDescriptor, MessageDescriptor};
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::ids::new_message_id;
use super::proto_edit::{ProtoError, MAGIC};
use super::proto_object::{self, bytes_value, field_name, is_js_space, value_bytes, Enums};
use super::protocol_types::{
    AlarmDate, AliceEvent, AliceEventType, AliceRequest, AliceResponseModel, CancelReason, ClientEvent, ClientInfo,
    ClientMessage, CloseReason, DeviceState, Directive, DirectiveType, EventHeader, PlayerState, PlayerView,
    ServerMessage, StationAlarm, StreamAction, StreamControlEvent, TtsStateReason,
};
use super::structs::{from_struct, to_struct};
use crate::utils::time::now_ms;

const CLOSE_REASONS: [(&str, CloseReason); 6] = [
    ("CLOSE_OK", CloseReason::Ok),
    ("CLOSE_INTERNAL_ERROR", CloseReason::InternalError),
    ("CLOSE_STOP_RECORDING", CloseReason::StopRecording),
    ("CLOSE_AUDIO_SOURCE_STOPPED", CloseReason::AudioSourceStopped),
    ("CLOSE_CANCEL", CloseReason::Cancel),
    ("CLOSE_NO_SPEECH", CloseReason::NoSpeech),
];

const ALARM_CAPABILITY: u64 = 21;

pub struct DecodedClientFrame {
    pub message: ClientMessage,
    /// The decoded protobuf as a plain object (bytes as bytes values), for the session log.
    pub decoded: Value,
}

pub struct EncodedServerFrame {
    pub frame: Bytes,
    /// The plain object that was encoded, for the session log.
    pub encoded: Value,
}

/// A capability state a request carries, whole and base64 (the relay journal keeps them for study).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilitySnapshot {
    pub r#type: u32,
    pub base64: String,
}

/// Codec for binary "AAPI" frames: the protobuf flavour of the protocol used by current firmware.
#[derive(Debug, Clone)]
pub struct AapiCodec {
    client_type: MessageDescriptor,
    server_type: MessageDescriptor,
    state_view_type: MessageDescriptor,
    alarm_capability_type: MessageDescriptor,
    push_view_type: MessageDescriptor,
    /// ClientEvent's and ServerEvent's fields after the header, in field number order: an event's first one is
    /// its kind.
    client_event_fields: Vec<String>,
    server_event_fields: Vec<String>,
}

impl Default for AapiCodec {
    fn default() -> AapiCodec {
        AapiCodec::new()
    }
}

/// The frame without its `AAPI` (a frame too short to have one: nothing).
fn body_of(frame: &[u8]) -> &[u8] {
    frame.get(MAGIC.len()..).unwrap_or_default()
}

fn aapi(body: Vec<u8>) -> Bytes {
    let mut frame = Vec::with_capacity(MAGIC.len() + body.len());
    frame.extend_from_slice(MAGIC);
    frame.extend(body);
    Bytes::from(frame)
}

fn event_fields(event: &MessageDescriptor) -> Vec<String> {
    let mut fields: Vec<FieldDescriptor> = event.fields().filter(|field| field.name() != "header").collect();
    fields.sort_by_key(FieldDescriptor::number);
    fields.iter().map(field_name).collect()
}

impl AapiCodec {
    pub fn new() -> AapiCodec {
        let lookup = |name: &str| proto_object::message_type(name).expect("the station's schema declares it");
        AapiCodec {
            client_type: lookup("ClientMessage"),
            server_type: lookup("ServerMessage"),
            state_view_type: lookup("RequestStateView"),
            alarm_capability_type: lookup("AlarmCapability"),
            push_view_type: lookup("PushView"),
            client_event_fields: event_fields(&lookup("ClientEvent")),
            server_event_fields: event_fields(&lookup("ServerEvent")),
        }
    }

    pub fn is_aapi_frame(frame: &[u8]) -> bool {
        frame.starts_with(MAGIC)
    }

    pub fn decode_client(&self, frame: &[u8]) -> Result<DecodedClientFrame, ProtoError> {
        let decoded = proto_object::decode(&self.client_type, body_of(frame), Enums::Names)?;
        if let Some(control) = present(&decoded, "streamControl") {
            return Ok(DecodedClientFrame {
                message: ClientMessage::StreamControl(map_stream_control(control)),
                decoded,
            });
        }
        if let Some(event) = present(&decoded, "event") {
            let mut event = self.map_event(event);
            if let ClientEvent::VoiceInput { request, .. } | ClientEvent::TextInput { request, .. } = &mut event {
                request.device_state = self.decode_device_state(frame);
            }
            return Ok(DecodedClientFrame { message: ClientMessage::Event(event), decoded });
        }
        let empty = ClientEvent::Unknown { header: EventHeader::default(), name: "empty".to_string(), raw: None };
        Ok(DecodedClientFrame { message: ClientMessage::Event(empty), decoded })
    }

    /// The device state of a VoiceInput or TextInput frame; None when there is none or it cannot be read.
    pub fn decode_device_state(&self, frame: &[u8]) -> Option<DeviceState> {
        let view = proto_object::decode(&self.state_view_type, body_of(frame), Enums::Numbers).ok()?;
        let body = request_body(&view)?;
        let device_state = present(body, "deviceState");
        let environment = present(body, "environmentState");
        if device_state.is_none() && environment.is_none() {
            return None;
        }
        let empty = Value::Object(Map::new());
        let device = device_state.unwrap_or(&empty);
        let player = present(device, "audioPlayer");
        let track = player.and_then(|player| present(player, "currentTrack"));
        let (timer_ids, alarms) = self.alarm_state(environment);
        Some(DeviceState {
            sound_level: device.get("soundLevel").and_then(Value::as_f64),
            sound_muted: bool_of(device, "soundMuted"),
            sound_max_level: device.get("soundMaxLevel").and_then(Value::as_f64),
            player: player.map(|player| PlayerView {
                state: match player.get("playerState").and_then(Value::as_u64) {
                    Some(1) => PlayerState::Idle,
                    Some(2) => PlayerState::Playing,
                    Some(5) => PlayerState::Stopped,
                    _ => PlayerState::Unknown,
                },
                id: track.and_then(|track| non_empty(track, "id")),
                title: track.and_then(|track| non_empty(track, "title")),
                r#type: track.and_then(|track| non_empty(track, "type")),
                offset_ms: i64_of(player, "offsetMs"),
            }),
            timer_ids,
            alarms,
        })
    }

    /// The capability states a request carries, whole and base64 (the relay journal keeps them for study).
    pub fn capability_states(&self, frame: &[u8]) -> Vec<CapabilitySnapshot> {
        let Ok(view) = proto_object::decode(&self.state_view_type, body_of(frame), Enums::Numbers) else {
            return Vec::new();
        };
        capabilities(request_body(&view))
            .iter()
            .map(|capability| CapabilitySnapshot {
                r#type: u32_of(capability, "type").unwrap_or(0),
                base64: B64.encode(capability.get("capability").and_then(value_bytes).unwrap_or_default()),
            })
            .collect()
    }

    /// The timers and alarms the station's alarm capability lists.
    fn alarm_state(&self, environment: Option<&Value>) -> (Vec<String>, Option<Vec<StationAlarm>>) {
        let capability = capabilities_of(environment)
            .iter()
            .find(|entry| entry.get("type").and_then(Value::as_u64) == Some(ALARM_CAPABILITY));
        let Some(bytes) = capability.and_then(|capability| capability.get("capability")).and_then(value_bytes) else {
            return (Vec::new(), None);
        };
        let Ok(decoded) = proto_object::decode(&self.alarm_capability_type, &bytes, Enums::Numbers) else {
            return (Vec::new(), None);
        };
        let state = present(&decoded, "state");
        let list = |key: &str| state.and_then(|state| state.get(key)).map(list_of).unwrap_or_default();
        let timer_ids = list("timers").iter().filter_map(|timer| non_empty(timer, "timerId")).collect();
        let alarms =
            list("alarms").iter().filter(|alarm| non_empty(alarm, "alarmId").is_some()).map(station_alarm).collect();
        (timer_ids, Some(alarms))
    }

    pub fn encode_server(&self, message: &ServerMessage) -> Result<EncodedServerFrame, ProtoError> {
        self.encode_server_with(message, &mut new_message_id, now_ms())
    }

    /// encode_server with the message ids and the time given (tests compare its bytes with the TS codec's).
    fn encode_server_with(
        &self,
        message: &ServerMessage,
        ids: &mut dyn FnMut() -> String,
        now_ms: u64,
    ) -> Result<EncodedServerFrame, ProtoError> {
        let encoded = match message {
            ServerMessage::Push { id, directive } => self.build_push(id, directive, ids, now_ms)?,
            _ => build_server_object(message, ids),
        };
        let body = proto_object::encode(&self.server_type, &encoded)?;
        Ok(EncodedServerFrame { frame: aapi(body), encoded })
    }

    /// A push as Yandex's: the message as raw bytes in field 5, a header with an ack time and no reference.
    fn build_push(
        &self,
        id: &str,
        directive: &Directive,
        ids: &mut dyn FnMut() -> String,
        now_ms: u64,
    ) -> Result<Value, ProtoError> {
        let view = json!({
            "id": id,
            "source": format!("notificator-internal-{}", ids()),
            "directive": build_directive(directive),
            "request": { "id": id },
        });
        let push = proto_object::encode(&self.push_view_type, &view)?;
        Ok(json!({ "event": { "header": { "messageId": ids(), "ack": now_ms }, "push": bytes_value(&push) } }))
    }

    /// For tests and the station simulator: decodes a push of ours (field 5 of a server event).
    pub fn decode_push(&self, push: &[u8]) -> Result<Value, ProtoError> {
        proto_object::decode(&self.push_view_type, push, Enums::Numbers)
    }

    /// For tests and the station simulator: encodes a client message object the way the station does.
    pub fn encode_client_object(&self, object: &Value) -> Result<Bytes, ProtoError> {
        proto_object::encode(&self.client_type, object).map(aapi)
    }

    /// For tests and the station simulator: decodes a server frame to a plain object.
    pub fn decode_server_frame(&self, frame: &[u8]) -> Result<Value, ProtoError> {
        proto_object::decode(&self.server_type, body_of(frame), Enums::Names)
    }

    /// For tests: a server frame from a plain object, for what encode_server does not build (Yandex's pushes).
    pub fn encode_server_object(&self, object: &Value) -> Result<Bytes, ProtoError> {
        proto_object::encode(&self.server_type, object).map(aapi)
    }

    /// Any message of the schema from a plain object, by its name (`AlarmCapability`): typed commands, and what
    /// tests build by name.
    pub fn encode_message(&self, type_name: &str, object: &Value) -> Result<Vec<u8>, ProtoError> {
        proto_object::encode(&proto_object::message_type(type_name)?, object)
    }

    /// Any message of the schema as a plain object, by its name.
    pub fn decode_message(&self, type_name: &str, bytes: &[u8], enums: Enums) -> Result<Value, ProtoError> {
        proto_object::decode(&proto_object::message_type(type_name)?, bytes, enums)
    }

    /// The kind of a decoded server frame's event, as the sessions name it: its first field after the header, in
    /// field number order (`aliceResponse`, `ttsSpeak`, `push`, …). None for an event with nothing else and for a
    /// frame without an event (a stream control).
    pub fn server_event_kind<'a>(&'a self, decoded: &Value) -> Option<&'a str> {
        let event = decoded.get("event")?;
        self.server_event_fields.iter().find(|name| event.get(name.as_str()).is_some()).map(String::as_str)
    }

    fn map_event(&self, event: &Value) -> ClientEvent {
        let header = map_header(event.get("header"));
        if let Some(sync) = present(event, "synchronizeStateRequest") {
            let features = sync.get("supportedFeatures").and_then(Value::as_array);
            return ClientEvent::SynchronizeState {
                header,
                application: present(sync, "application").map(map_client_info),
                supported_features: features
                    .map(|features| features.iter().filter_map(|f| f.as_str().map(str::to_string)).collect())
                    .unwrap_or_default(),
            };
        }
        if let Some(request) = present(event, "voiceInput") {
            return ClientEvent::VoiceInput { header, request: map_request(request) };
        }
        if let Some(request) = present(event, "textInput") {
            return ClientEvent::TextInput { header, request: map_request(request) };
        }
        if let Some(request) = present(event, "matchedUser") {
            return ClientEvent::MatchedUser { header, request: map_request(request) };
        }
        if let Some(log) = present(event, "logSpotter") {
            return ClientEvent::LogSpotter {
                header,
                format: str_of(log, "format"),
                source: str_of(log, "source"),
                transcript: str_of(log, "transcript"),
                firmware: str_of(log, "firmware"),
            };
        }
        if let Some(stat) = present(event, "requestStat") {
            return ClientEvent::RequestStat {
                header,
                request_message_id: str_of(stat, "requestMessageId"),
                error: str_of(stat, "error"),
            };
        }
        if let Some(ack) = present(event, "ack") {
            let received = present(ack, "receivedHeader").and_then(|received| str_of(received, "messageId"));
            return ClientEvent::Ack { header, received_message_id: received };
        }
        if present(event, "ping").is_some() {
            return ClientEvent::Ping { header };
        }
        if let Some(cancel) = present(event, "cancelRequest") {
            return ClientEvent::CancelRequest {
                header,
                request_message_id: str_of(cancel, "requestMessageId"),
                reason: map_cancel_reason(cancel),
            };
        }
        if let Some(tts) = present(event, "ttsGenerate") {
            return ClientEvent::TtsGenerate {
                header,
                text: str_of(tts, "text").unwrap_or_default(),
                voice: str_of(tts, "voice"),
                lang: str_of(tts, "lang"),
                format: str_of(tts, "format"),
            };
        }
        if let Some(update) = present(event, "ttsUpdateState") {
            let state = present(update, "state");
            let played = state.and_then(|state| present(state, "playedTtsDuration"));
            return ClientEvent::TtsUpdateState {
                header,
                request_message_id: str_of(update, "requestMessageId"),
                played_ms: played.map(|played| {
                    let seconds = played.get("seconds").and_then(Value::as_f64).unwrap_or(0.0);
                    let nanos = played.get("nanos").and_then(Value::as_f64).unwrap_or(0.0);
                    (seconds * 1000.0 + js_round(nanos / 1e6)) as i64
                }),
                is_playing: state.and_then(|state| bool_of(state, "isPlaying")),
                reason: match update.get("reportReason").and_then(Value::as_str) {
                    Some("TTS_REPORT_INTERRUPTION") => TtsStateReason::Interruption,
                    Some("TTS_REPORT_FINISH") => TtsStateReason::Finish,
                    _ => TtsStateReason::Unknown,
                },
            };
        }
        let name = self.client_event_fields.iter().find(|name| event.get(name.as_str()).is_some());
        ClientEvent::Unknown {
            header,
            name: name.cloned().unwrap_or_else(|| "empty".to_string()),
            raw: name.and_then(|name| event.get(name.as_str())).cloned(),
        }
    }
}

// ---------------------------------------------------------------- reading plain objects

/// A field that came (a message, bytes: what JavaScript found truthy there).
fn present<'a>(object: &'a Value, key: &str) -> Option<&'a Value> {
    object.get(key).filter(|value| !value.is_null())
}

fn str_of(object: &Value, key: &str) -> Option<String> {
    object.get(key).and_then(Value::as_str).map(str::to_string)
}

/// A string field that came and is not empty (the TS's `value || undefined`).
fn non_empty(object: &Value, key: &str) -> Option<String> {
    str_of(object, key).filter(|text| !text.is_empty())
}

fn bool_of(object: &Value, key: &str) -> Option<bool> {
    object.get(key).and_then(Value::as_bool)
}

fn i64_of(object: &Value, key: &str) -> Option<i64> {
    let value = object.get(key)?;
    value.as_i64().or_else(|| value.as_f64().map(|n| n as i64))
}

fn u32_of(object: &Value, key: &str) -> Option<u32> {
    object.get(key).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok())
}

/// JavaScript's Math.round: halves up.
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// The request body of a decoded RequestStateView: the VoiceInput's, else the TextInput's.
fn request_body(view: &Value) -> Option<&Value> {
    let event = present(view, "event")?;
    let request = present(event, "voiceInput").or_else(|| present(event, "textInput"))?;
    present(request, "request")
}

fn capabilities(body: Option<&Value>) -> &[Value] {
    capabilities_of(body.and_then(|body| present(body, "environmentState")))
}

fn capabilities_of(environment: Option<&Value>) -> &[Value] {
    environment
        .and_then(|environment| present(environment, "endpoint"))
        .and_then(|endpoint| endpoint.get("capabilities"))
        .map(list_of)
        .unwrap_or_default()
}

/// A repeated field's values (none when it is not a list).
fn list_of(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

// ---------------------------------------------------------------- client -> internal model

/// An alarm of the station's alarm capability (AlarmSpec decoded with longs as numbers).
fn station_alarm(alarm: &Value) -> StationAlarm {
    let days = alarm.get("activationDays").map(list_of).unwrap_or_default();
    let date = days.iter().find_map(|day| present(day, "date"));
    let local_time = present(alarm, "localTime");
    let time = |key: &str| local_time.and_then(|time| u32_of(time, key)).unwrap_or(0);
    let timezone = present(alarm, "timezone");
    StationAlarm {
        id: str_of(alarm, "alarmId").unwrap_or_default(),
        hours: time("hours"),
        minutes: time("minutes"),
        seconds: time("seconds"),
        date: date.map(|date| AlarmDate {
            year: i64_of(date, "year").map_or(0, |year| year as i32),
            month: u32_of(date, "month").unwrap_or(0),
            day: u32_of(date, "day").unwrap_or(0),
        }),
        timezone: timezone.and_then(|timezone| non_empty(timezone, "timezoneName")),
        offset_seconds: timezone.and_then(|timezone| i64_of(timezone, "offsetSeconds")),
        edit_timestamp: i64_of(alarm, "editTimestamp").filter(|&at| at != 0),
        ringing: bool_of(alarm, "ringing") == Some(true),
        rung: bool_of(alarm, "rung") == Some(true),
    }
}

fn map_header(header: Option<&Value>) -> EventHeader {
    let Some(header) = header else { return EventHeader::default() };
    EventHeader {
        message_id: str_of(header, "messageId").unwrap_or_default(),
        ref_message_id: non_empty(header, "refMessageId"),
        stream_id: i64_of(header, "streamId").and_then(|id| u32::try_from(id).ok()),
    }
}

fn map_client_info(info: &Value) -> ClientInfo {
    ClientInfo {
        app_id: str_of(info, "appId"),
        app_version: str_of(info, "appVersion"),
        platform: str_of(info, "platform"),
        uuid: str_of(info, "uuid"),
        device_id: str_of(info, "deviceId"),
        lang: str_of(info, "lang"),
        timezone: str_of(info, "timezone"),
        device_model: str_of(info, "deviceModel"),
        device_manufacturer: str_of(info, "deviceManufacturer"),
        device_color: str_of(info, "deviceColor"),
        firmware_version: str_of(info, "firmwareVersion"),
        quasmodrom_group: str_of(info, "quasmodromGroup"),
    }
}

fn map_request(request: &Value) -> AliceRequest {
    let empty = Value::Object(Map::new());
    let header = present(request, "header").unwrap_or(&empty);
    let body = present(request, "request");
    let event = body.and_then(|body| present(body, "event")).unwrap_or(&empty);
    AliceRequest {
        request_id: str_of(header, "requestId"),
        sequence_number: i64_of(header, "sequenceNumber"),
        dialog_id: str_of(header, "dialogId"),
        application: present(request, "application").map(map_client_info),
        event: AliceEvent {
            r#type: event.get("type").and_then(Value::as_str).map_or(AliceEventType::Unknown, AliceEventType::parse),
            text: str_of(event, "text"),
            name: str_of(event, "name"),
            payload: from_struct(present(event, "payload")),
            payload_raw: event.get("payloadRaw").and_then(value_bytes).filter(|raw| !raw.is_empty()).map(Bytes::from),
            asr_whisper: bool_of(event, "asrWhisper"),
        },
        enable_spotter_validation: bool_of(request, "enableSpotterValidation"),
        spotter_phrase: non_empty(request, "spotterPhrase"),
        activation_type: body.and_then(|body| non_empty(body, "activationType")),
        lang: str_of(request, "lang"),
        topic: str_of(request, "topic"),
        device_state: None,
    }
}

fn map_cancel_reason(cancel: &Value) -> CancelReason {
    if present(cancel, "userStop").is_some() {
        CancelReason::UserStop
    } else if present(cancel, "interruption").is_some() {
        CancelReason::Interruption
    } else if present(cancel, "localExecution").is_some() {
        CancelReason::LocalExecution
    } else {
        CancelReason::Unknown
    }
}

fn map_stream_control(control: &Value) -> StreamControlEvent {
    let stream_id = i64_of(control, "streamId").and_then(|id| u32::try_from(id).ok()).unwrap_or(0);
    let message_id = str_of(control, "messageId");
    let (action, reason) = if let Some(close) = present(control, "close") {
        let name = close.get("reason").and_then(Value::as_str).unwrap_or("CLOSE_OK");
        let reason =
            CLOSE_REASONS.iter().find(|(wire, _)| *wire == name).map_or(CloseReason::Ok, |(_, reason)| *reason);
        (StreamAction::Close, Some(reason))
    } else if present(control, "spotterEnd").is_some() {
        (StreamAction::SpotterEnd, None)
    } else if present(control, "lazyTts").is_some() {
        (StreamAction::LazyTts, None)
    } else {
        (StreamAction::Unknown, None)
    };
    StreamControlEvent { stream_id, message_id, action, reason }
}

// ---------------------------------------------------------------- internal model -> server

fn header(r#ref: &str, stream_id: Option<u32>, ids: &mut dyn FnMut() -> String) -> Value {
    let mut header = json!({ "messageId": ids(), "refMessageId": r#ref });
    if let Some(stream_id) = stream_id {
        header["streamId"] = json!(stream_id);
    }
    header
}

/// A directive as the wire's Directive object.
pub fn build_directive(directive: &Directive) -> Value {
    let typed = directive.typed_payload.is_some();
    let mut object = json!({
        "type": match directive.r#type {
            DirectiveType::ClientAction => "client_action",
            DirectiveType::ServerAction => "server_action",
        },
        "name": directive.name,
        "analyticsType": directive.analytics_type.as_deref().unwrap_or(&directive.name),
        "ignoreAnswer": directive.ignore_answer.unwrap_or(false),
        "isLedSilent": directive.is_led_silent.unwrap_or(false),
    });
    // A typed command carries its proto in field 15 and no Struct, as Yandex sends it.
    match &directive.payload {
        Some(payload) => object["payload"] = to_struct(payload),
        None if !typed => object["payload"] = to_struct(&Map::new()),
        None => {}
    }
    if let Some(typed_payload) = &directive.typed_payload {
        object["typedPayload"] = bytes_value(typed_payload);
    }
    object
}

/// The words of a recognised text as `text.split(/\s+/u)` gave them: empty ones at the ends where it starts or
/// ends with spaces.
fn split_words(text: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start = 0;
    let mut in_space = false;
    for (index, c) in text.char_indices() {
        if is_js_space(c) {
            if !in_space {
                words.push(&text[start..index]);
                in_space = true;
            }
        } else if in_space {
            start = index;
            in_space = false;
        }
    }
    words.push(if in_space { "" } else { &text[start..] });
    words
}

fn alice_response(r#ref: &str, response: &AliceResponseModel, ids: &mut dyn FnMut() -> String) -> Value {
    // The header's id first, then the card's, as the TS drew them.
    let header = header(r#ref, None, ids);
    let speech = response.speech.clone().unwrap_or_default();
    let mut voice_response = json!({
        "hasVoiceResponse": response.has_voice.unwrap_or(!speech.is_empty()),
        "shouldListen": response.should_listen.unwrap_or(false),
    });
    if !speech.is_empty() {
        voice_response["outputSpeech"] = json!({ "text": speech });
    }
    let cards = match response.text.as_deref() {
        Some(text) if !text.is_empty() => json!([{ "type": "simple_text", "text": text, "cardId": ids() }]),
        _ => json!([]),
    };
    json!({
        "event": {
            "header": header,
            "aliceResponse": {
                "header": {
                    "requestId": response.request_id,
                    "responseId": response.response_id,
                    "sequenceNumber": response.sequence_number.unwrap_or(0),
                    "dialogId": response.dialog_id.as_deref().unwrap_or(""),
                    "parentRequestId": response.request_id,
                },
                "voiceResponse": voice_response,
                "response": {
                    "cards": cards,
                    "directives": response.directives.iter().map(build_directive).collect::<Vec<_>>(),
                },
            },
        },
    })
}

fn build_server_object(message: &ServerMessage, ids: &mut dyn FnMut() -> String) -> Value {
    match message {
        ServerMessage::SynchronizeStateResponse { r#ref, session_id } => {
            json!({ "event": { "header": header(r#ref, None, ids), "synchronizeStateResponse": { "sessionId": session_id } } })
        }
        ServerMessage::InputStartAck { r#ref, request_start_time_us } => json!({
            "event": { "header": header(r#ref, None, ids), "inputStartAck": { "requestStartTime": request_start_time_us } }
        }),
        ServerMessage::AsrResult { r#ref, text, end_of_utt } => {
            let recognition = if text.is_empty() {
                json!([])
            } else {
                let words: Vec<Value> =
                    split_words(text).into_iter().map(|value| json!({ "value": value, "confidence": 1 })).collect();
                json!([{ "normalized": text, "confidence": 1, "words": words }])
            };
            json!({
                "event": {
                    "header": header(r#ref, None, ids),
                    "asrResult": {
                        "responseCode": "ASR_OK",
                        "messagesCount": 1,
                        "endOfUtt": end_of_utt,
                        "recognition": recognition,
                    },
                },
            })
        }
        ServerMessage::SpotterValidation { r#ref, valid } => json!({
            "event": {
                "header": header(r#ref, None, ids),
                "spotterValidation": { "result": if *valid { "SPOTTING_OK" } else { "SPOTTING_REJECTED" } },
            }
        }),
        ServerMessage::LogAck { r#ref } => json!({ "event": { "header": header(r#ref, None, ids), "logAck": {} } }),
        ServerMessage::RequestStatAck { r#ref } => {
            json!({ "event": { "header": header(r#ref, None, ids), "requestStatAck": {} } })
        }
        ServerMessage::AliceResponse { r#ref, response } => alice_response(r#ref, response, ids),
        ServerMessage::TtsSpeak { r#ref, stream_id, format } => json!({
            "event": {
                "header": header(r#ref, Some(*stream_id), ids),
                "ttsSpeak": { "format": format, "lazyTtsStreaming": false },
            }
        }),
        ServerMessage::StreamClose { stream_id, reason } => {
            let name = CLOSE_REASONS.iter().find(|(_, known)| known == reason).map_or("CLOSE_OK", |(name, _)| name);
            json!({ "streamControl": { "streamId": stream_id, "messageId": ids(), "close": { "reason": name } } })
        }
        // A push is built with its schema (AapiCodec::build_push).
        ServerMessage::Push { .. } => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The alarm capability (type 21) as our station sent it with no timer: meta, parameters, alarm sound settings.
    const IDLE_ALARMS: &str =
        "0a110a0fd3027822747620840170717273940212020a001a161a140a120801100718a09c0125000020412d00008c42";

    fn encode(type_name: &str, object: Value) -> Vec<u8> {
        AapiCodec::new().encode_message(type_name, &object).unwrap()
    }

    /// A VoiceInput frame whose request carries a device state, as the station sends it.
    fn voice_input_with_state(capability: &[u8]) -> Vec<u8> {
        let view = encode(
            "RequestStateView",
            json!({ "event": { "voiceInput": { "request": {
                "deviceState": {
                    "soundLevel": 4,
                    "soundMaxLevel": 10,
                    "audioPlayer": {
                        "playerState": 2,
                        "offsetMs": 8358,
                        "currentTrack": { "id": "studio21", "title": "Studio 21", "type": "FmRadio" },
                    },
                },
                "environmentState": { "endpoint": { "capabilities": [{ "type": 21, "capability": bytes_value(capability) }] } },
            } } } }),
        );
        // The view's field numbers are the request's, so its bytes are a valid ClientMessage; add the header too.
        let header =
            encode("ClientMessage", json!({ "event": { "header": { "messageId": "voice-1", "streamId": 1 } } }));
        [MAGIC.as_slice(), &header, &view].concat()
    }

    fn device_state(frame: &[u8]) -> DeviceState {
        match AapiCodec::new().decode_client(frame).unwrap().message {
            ClientMessage::Event(ClientEvent::VoiceInput { request, .. }) => request.device_state.unwrap(),
            other => panic!("not a voice input: {other:?}"),
        }
    }

    #[test]
    fn reads_volume_and_the_player_from_a_request_apart_from_the_journaled_event() {
        let frame = voice_input_with_state(&hex::decode(IDLE_ALARMS).unwrap());
        let DecodedClientFrame { message, decoded } = AapiCodec::new().decode_client(&frame).unwrap();
        let ClientMessage::Event(ClientEvent::VoiceInput { request, .. }) = message else { panic!("{message:?}") };
        let player = PlayerView {
            state: PlayerState::Playing,
            id: Some("studio21".into()),
            title: Some("Studio 21".into()),
            r#type: Some("FmRadio".into()),
            offset_ms: Some(8358),
        };
        assert_eq!(
            request.device_state,
            Some(DeviceState {
                sound_level: Some(4.0),
                sound_muted: None,
                sound_max_level: Some(10.0),
                player: Some(player),
                timer_ids: vec![],
                alarms: Some(vec![]),
            })
        );
        // The journal's decoded event has no device state: the rest of it (Wi-Fi networks) stays out.
        assert!(!decoded.to_string().contains("Studio 21"));
    }

    #[test]
    fn lists_the_timers_the_alarm_capability_reports() {
        let ringing = encode("AlarmCapability", json!({ "state": { "timers": [{ "timerId": "a01376d3-6069df2f" }] } }));
        assert_eq!(device_state(&voice_input_with_state(&ringing)).timer_ids, vec!["a01376d3-6069df2f"]);
    }

    #[test]
    fn lists_the_alarms_with_the_one_that_rings_and_the_ones_that_rang_relay_journal_02_11() {
        let alarm = |alarm_id: &str, hours: u32, minutes: u32, flag: &str| {
            let mut alarm = json!({
                "alarmId": alarm_id,
                "localTime": { "hours": hours, "minutes": minutes },
                "activationDays": [{ "date": { "year": 2026, "month": 11, "day": 2 } }],
                "timezone": { "timezoneName": "Asia/Bangkok", "offsetSeconds": 25200 },
                "editTimestamp": 1793578022,
            });
            alarm[flag] = json!(true);
            alarm
        };
        let state = encode(
            "AlarmCapability",
            json!({ "state": {
                "alarms": [alarm("a5d25963", 7, 8, "rung"), alarm("3111feb9", 7, 11, "ringing")],
                "timers": [{ "timerId": "8db6c86a", "startTimestamp": 1793578035, "duration": 600, "remaining": 592, "paused": true }],
            } }),
        );
        let device = device_state(&voice_input_with_state(&state));
        assert_eq!(device.timer_ids, vec!["8db6c86a"]);
        let alarms = device.alarms.unwrap();
        assert_eq!(alarms.len(), 2);
        let rang = &alarms[0];
        assert_eq!(
            (rang.id.as_str(), rang.hours, rang.minutes, rang.ringing, rang.rung),
            ("a5d25963", 7, 8, false, true)
        );
        assert_eq!(
            alarms[1],
            StationAlarm {
                id: "3111feb9".into(),
                hours: 7,
                minutes: 11,
                seconds: 0,
                date: Some(AlarmDate { year: 2026, month: 11, day: 2 }),
                timezone: Some("Asia/Bangkok".into()),
                offset_seconds: Some(25200),
                edit_timestamp: Some(1793578022),
                ringing: true,
                rung: false,
            }
        );
    }

    #[test]
    fn encodes_a_typed_command_with_its_proto_in_field_15_and_no_struct() {
        let typed_payload = Bytes::from_static(&[0x12, 0x02, 0x69, 0x64]);
        let directive = Directive {
            r#type: DirectiveType::ClientAction,
            name: "timer_stop_directive".into(),
            analytics_type: Some(String::new()),
            payload: None,
            typed_payload: Some(typed_payload.clone()),
            is_led_silent: None,
            ignore_answer: None,
        };
        let response = AliceResponseModel {
            request_id: "req-1".into(),
            response_id: "resp-1".into(),
            directives: vec![directive],
            ..Default::default()
        };
        let message = ServerMessage::AliceResponse { r#ref: "voice-1".into(), response };
        let EncodedServerFrame { encoded, .. } = AapiCodec::new().encode_server(&message).unwrap();
        let directive = &encoded["event"]["aliceResponse"]["response"]["directives"][0];
        assert_eq!(directive.get("payload"), None);
        assert_eq!(value_bytes(&directive["typedPayload"]).as_deref(), Some(&typed_payload[..]));
    }

    /// Message ids as the TS codec drew them for the frames below: id-1, id-2, …
    fn counter() -> impl FnMut() -> String {
        let mut next = 0;
        move || {
            next += 1;
            format!("id-{next}")
        }
    }

    #[test]
    fn writes_server_frames_byte_for_byte_as_the_ts_codec_did() {
        // Frames of the TS codec (protobufjs) with the same ids: default values are written (a rejected
        // validation is result 0 on the wire, false flags are there), a Struct's values in their oneof, a typed
        // payload in field 15, a push as raw bytes in field 5.
        let directive = |name: &str| Directive {
            r#type: DirectiveType::ClientAction,
            name: name.into(),
            analytics_type: None,
            payload: None,
            typed_payload: None,
            is_led_silent: None,
            ignore_answer: None,
        };
        let response = AliceResponseModel {
            request_id: "req-1".into(),
            response_id: "resp-1".into(),
            text: Some("Да.".into()),
            speech: Some("Да.".into()),
            directives: vec![
                Directive {
                    payload: json!({ "stream": { "offset": 1.5, "url": "u" } }).as_object().cloned(),
                    ..directive("audio_play")
                },
                Directive {
                    analytics_type: Some(String::new()),
                    typed_payload: Some(Bytes::from_static(&[0x12, 0x02, 0x69, 0x64])),
                    ..directive("timer_stop_directive")
                },
            ],
            ..Default::default()
        };
        let reminder = Directive {
            r#type: DirectiveType::ServerAction,
            payload: json!({ "text": "пора" }).as_object().cloned(),
            ..directive("vgw_reminder")
        };
        let cases = [
            (
                ServerMessage::SpotterValidation { r#ref: "voice-1".into(), valid: false },
                "414150490a160a0f0a0469642d311207766f6963652d318201020800",
            ),
            (
                ServerMessage::AsrResult { r#ref: "voice-1".into(), text: " включи  радио ".into(), end_of_utt: true },
                "414150490a760a0f0a0469642d311207766f6963652d316a63080010011801225b0d0000803f12070a00150000803f12130a0c\
                 d0b2d0bad0bbd18ed187d0b8150000803f12110a0ad180d0b0d0b4d0b8d0be150000803f12070a00150000803f1a1a20d0b2\
                 d0bad0bbd18ed187d0b82020d180d0b0d0b4d0b8d0be20",
            ),
            (
                ServerMessage::AliceResponse { r#ref: "voice-1".into(), response },
                "414150490aef010a0f0a0469642d311207766f6963652d3112db010a1a0a057265712d311206726573702d31180022003a05\
                 7265712d31120d0a071205d094d0b02e100048011aad010a1a0a0b73696d706c655f746578741205d094d0b02e320469642d\
                 32125c0a0d636c69656e745f616374696f6e120a617564696f5f706c61791a0a617564696f5f706c617920002a2f0a2d0a06\
                 73747265616d12232a210a130a066f6666736574120911000000000000f83f0a0a0a0375726c12031a0175400012310a0d63\
                 6c69656e745f616374696f6e121474696d65725f73746f705f6469726563746976651a00200040007a0412026964",
            ),
            (
                ServerMessage::TtsSpeak { r#ref: "tts-1".into(), stream_id: 7, format: "audio/opus".into() },
                "414150490a210a0f0a0469642d3112057474732d312007720e0a0a617564696f2f6f7075731000",
            ),
            (ServerMessage::StreamClose { stream_id: 9, reason: CloseReason::Cancel }, "41415049120c0809120469642d311a020804"),
            (
                ServerMessage::Push { id: "push-1".into(), directive: reminder },
                "414150490a85010a0d0a0469642d3228f0e8d2cc99342a740a06707573682d3112196e6f746966696361746f722d696e7465\
                 726e616c2d69642d311a450a0d7365727665725f616374696f6e120c7667775f72656d696e6465721a0c7667775f72656d69\
                 6e64657220002a140a120a0474657874120a1a08d0bfd0bed180d0b0400022080a06707573682d31",
            ),
        ];
        let codec = AapiCodec::new();
        for (message, frame) in cases {
            let encoded = codec.encode_server_with(&message, &mut counter(), 1793578022000).unwrap();
            assert_eq!(hex::encode(&encoded.frame), frame, "{message:?}");
        }
    }

    #[test]
    fn reads_the_station_events_into_the_model() {
        let codec = AapiCodec::new();
        let read = |object: Value| codec.decode_client(&codec.encode_client_object(&object).unwrap()).unwrap().message;
        let header = |id: &str| EventHeader { message_id: id.into(), ..Default::default() };

        // A callback of our push: a server_action with its name, its Struct payload and a typed payload.
        let callback = read(json!({ "event": {
            "header": { "messageId": "callback-1" },
            "textInput": {
                "header": { "requestId": "req-callback" },
                "request": { "event": {
                    "type": "server_action",
                    "name": "vgw_reminder",
                    "payload": { "fields": { "text": { "stringValue": "пора" } } },
                    "payloadRaw": bytes_value(&[0x0a, 0x01, 0x01]),
                } },
            },
        } }));
        let ClientMessage::Event(ClientEvent::TextInput { header: got, request }) = callback else {
            panic!("{callback:?}")
        };
        assert_eq!(got, header("callback-1"));
        assert_eq!(request.request_id.as_deref(), Some("req-callback"));
        assert_eq!(request.event.r#type, AliceEventType::ServerAction);
        assert_eq!(request.event.name.as_deref(), Some("vgw_reminder"));
        assert_eq!(request.event.payload, json!({ "text": "пора" }).as_object().cloned());
        assert_eq!(request.event.payload_raw.as_deref(), Some(&[0x0a, 0x01, 0x01][..]));
        assert_eq!(request.device_state, None);

        // Played 3 s and 499.5 ms: rounded as JavaScript rounds.
        let played = read(json!({ "event": { "header": { "messageId": "u-1" }, "ttsUpdateState": {
            "requestMessageId": "voice-1",
            "state": { "playedTtsDuration": { "seconds": 3, "nanos": 499500000 }, "isPlaying": true },
            "reportReason": "TTS_REPORT_FINISH",
        } } }));
        assert_eq!(
            played,
            ClientMessage::Event(ClientEvent::TtsUpdateState {
                header: header("u-1"),
                request_message_id: Some("voice-1".into()),
                played_ms: Some(3500),
                is_playing: Some(true),
                reason: TtsStateReason::Finish,
            })
        );

        // An empty local_execution still says why.
        let cancel = read(json!({ "event": { "header": { "messageId": "c-2" }, "cancelRequest": {
            "requestMessageId": "voice-1",
            "localExecution": bytes_value(&[]),
        } } }));
        assert_eq!(
            cancel,
            ClientMessage::Event(ClientEvent::CancelRequest {
                header: header("c-2"),
                request_message_id: Some("voice-1".into()),
                reason: CancelReason::LocalExecution,
            })
        );

        // What the model does not describe keeps its field's name and value.
        let sync_push = read(json!({ "event": {
            "header": { "messageId": "spr-1", "refMessageId": "sync-push-1" },
            "syncPushResponse": bytes_value(&[0x0a, 0x00]),
        } }));
        assert_eq!(
            sync_push,
            ClientMessage::Event(ClientEvent::Unknown {
                header: EventHeader { ref_message_id: Some("sync-push-1".into()), ..header("spr-1") },
                name: "syncPushResponse".into(),
                raw: Some(bytes_value(&[0x0a, 0x00])),
            })
        );
        assert_eq!(
            read(json!({})),
            ClientMessage::Event(ClientEvent::Unknown {
                header: EventHeader::default(),
                name: "empty".into(),
                raw: None
            })
        );

        // A close without a reason is a close with CLOSE_OK.
        let control = |stream_id, message_id: Option<&str>, action, reason| {
            ClientMessage::StreamControl(StreamControlEvent {
                stream_id,
                message_id: message_id.map(str::to_string),
                action,
                reason,
            })
        };
        let close = read(json!({ "streamControl": { "streamId": 1, "messageId": "sc-1", "close": {} } }));
        assert_eq!(close, control(1, Some("sc-1"), StreamAction::Close, Some(CloseReason::Ok)));
        let no_speech = read(json!({ "streamControl": { "streamId": 2, "close": { "reason": "CLOSE_NO_SPEECH" } } }));
        assert_eq!(no_speech, control(2, None, StreamAction::Close, Some(CloseReason::NoSpeech)));
        let spotter_end = read(json!({ "streamControl": { "streamId": 3, "spotterEnd": {} } }));
        assert_eq!(spotter_end, control(3, None, StreamAction::SpotterEnd, None));
    }

    #[test]
    fn names_a_server_event_by_its_first_field_after_the_header() {
        let codec = AapiCodec::new();
        let kind =
            |frame: &[u8]| codec.server_event_kind(&codec.decode_server_frame(frame).unwrap()).map(str::to_string);
        let speak = ServerMessage::TtsSpeak { r#ref: "tts-1".into(), stream_id: 7, format: "audio/opus".into() };
        assert_eq!(kind(&codec.encode_server(&speak).unwrap().frame).as_deref(), Some("ttsSpeak"));
        // Yandex's push: raw bytes in field 5.
        let push = json!({ "event": { "header": { "messageId": "push-1" }, "push": bytes_value(&[0x1a, 0x00]) } });
        assert_eq!(kind(&codec.encode_server_object(&push).unwrap()).as_deref(), Some("push"));
        let close = ServerMessage::StreamClose { stream_id: 7, reason: CloseReason::Ok };
        assert_eq!(kind(&codec.encode_server(&close).unwrap().frame), None);
    }
}
