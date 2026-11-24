//! Internal model of the station protocol. Codecs translate the wire format to these types and back; the session
//! logic never sees protobuf or JSON frames directly.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireFormat {
    Aapi,
    Json,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventHeader {
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_manufacturer: Option<String>,
    /// The body's colour as the station names it: «blue».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quasmodrom_group: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AliceEventType {
    VoiceInput,
    TextInput,
    SuggestedInput,
    ImageInput,
    MusicInput,
    ServerAction,
    HybridAsrPartialResult,
    FileInput,
    SpeechInput,
    Unknown,
}

impl AliceEventType {
    pub fn parse(value: &str) -> AliceEventType {
        match value {
            "voice_input" => AliceEventType::VoiceInput,
            "text_input" => AliceEventType::TextInput,
            "suggested_input" => AliceEventType::SuggestedInput,
            "image_input" => AliceEventType::ImageInput,
            "music_input" => AliceEventType::MusicInput,
            "server_action" => AliceEventType::ServerAction,
            "hybrid_asr_partial_result" => AliceEventType::HybridAsrPartialResult,
            "file_input" => AliceEventType::FileInput,
            "speech_input" => AliceEventType::SpeechInput,
            _ => AliceEventType::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AliceEventType::VoiceInput => "voice_input",
            AliceEventType::TextInput => "text_input",
            AliceEventType::SuggestedInput => "suggested_input",
            AliceEventType::ImageInput => "image_input",
            AliceEventType::MusicInput => "music_input",
            AliceEventType::ServerAction => "server_action",
            AliceEventType::HybridAsrPartialResult => "hybrid_asr_partial_result",
            AliceEventType::FileInput => "file_input",
            AliceEventType::SpeechInput => "speech_input",
            AliceEventType::Unknown => "unknown",
        }
    }
}

// The device's own report — volume, player, timers, alarms — is the ear contract's (src/ear/state.rs).
pub use crate::ear::state::{AlarmDate, DeviceState, PlayerState, PlayerView, StationAlarm, StationTimer};

#[derive(Debug, Clone, PartialEq)]
pub struct AliceEvent {
    pub r#type: AliceEventType,
    pub text: Option<String>,
    pub name: Option<String>,
    pub payload: Option<Map<String, Value>>,
    /// A typed payload (a semantic frame's callback), as it came.
    pub payload_raw: Option<Bytes>,
    pub asr_whisper: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AliceRequest {
    pub request_id: Option<String>,
    pub sequence_number: Option<i64>,
    pub dialog_id: Option<String>,
    pub application: Option<ClientInfo>,
    pub event: AliceEvent,
    pub enable_spotter_validation: Option<bool>,
    /// The wake word the station heard ("алиса").
    pub spotter_phrase: Option<String>,
    /// "spotter" when started by the wake word; then the audio begins with the wake word buffer.
    pub activation_type: Option<String>,
    pub lang: Option<String>,
    pub topic: Option<String>,
    /// Decoded on its own from the frame; not part of the journaled event.
    pub device_state: Option<DeviceState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CancelReason {
    UserStop,
    Interruption,
    LocalExecution,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CloseReason {
    Ok,
    InternalError,
    StopRecording,
    AudioSourceStopped,
    Cancel,
    NoSpeech,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TtsStateReason {
    Interruption,
    Finish,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ClientEvent {
    SynchronizeState {
        header: EventHeader,
        application: Option<ClientInfo>,
        supported_features: Vec<String>,
    },
    VoiceInput {
        header: EventHeader,
        request: AliceRequest,
    },
    TextInput {
        header: EventHeader,
        request: AliceRequest,
    },
    MatchedUser {
        header: EventHeader,
        request: AliceRequest,
    },
    LogSpotter {
        header: EventHeader,
        format: Option<String>,
        source: Option<String>,
        transcript: Option<String>,
        /// The station's firmware: the only place it names it (its synchronizeState has no firmwareVersion; Lite 2,
        /// 08.11).
        firmware: Option<String>,
    },
    RequestStat {
        header: EventHeader,
        request_message_id: Option<String>,
        error: Option<String>,
    },
    Ack {
        header: EventHeader,
        received_message_id: Option<String>,
    },
    Ping {
        header: EventHeader,
    },
    CancelRequest {
        header: EventHeader,
        request_message_id: Option<String>,
        reason: CancelReason,
    },
    TtsGenerate {
        header: EventHeader,
        text: String,
        voice: Option<String>,
        lang: Option<String>,
        format: Option<String>,
    },
    TtsUpdateState {
        header: EventHeader,
        request_message_id: Option<String>,
        played_ms: Option<i64>,
        is_playing: Option<bool>,
        reason: TtsStateReason,
    },
    Unknown {
        header: EventHeader,
        name: String,
        raw: Option<Value>,
    },
}

impl ClientEvent {
    pub fn header(&self) -> &EventHeader {
        match self {
            ClientEvent::SynchronizeState { header, .. }
            | ClientEvent::VoiceInput { header, .. }
            | ClientEvent::TextInput { header, .. }
            | ClientEvent::MatchedUser { header, .. }
            | ClientEvent::LogSpotter { header, .. }
            | ClientEvent::RequestStat { header, .. }
            | ClientEvent::Ack { header, .. }
            | ClientEvent::Ping { header }
            | ClientEvent::CancelRequest { header, .. }
            | ClientEvent::TtsGenerate { header, .. }
            | ClientEvent::TtsUpdateState { header, .. }
            | ClientEvent::Unknown { header, .. } => header,
        }
    }

    /// The kind's name as the journal and the logs write it: synchronizeState, voiceInput, …
    pub fn kind(&self) -> &'static str {
        match self {
            ClientEvent::SynchronizeState { .. } => "synchronizeState",
            ClientEvent::VoiceInput { .. } => "voiceInput",
            ClientEvent::TextInput { .. } => "textInput",
            ClientEvent::MatchedUser { .. } => "matchedUser",
            ClientEvent::LogSpotter { .. } => "logSpotter",
            ClientEvent::RequestStat { .. } => "requestStat",
            ClientEvent::Ack { .. } => "ack",
            ClientEvent::Ping { .. } => "ping",
            ClientEvent::CancelRequest { .. } => "cancelRequest",
            ClientEvent::TtsGenerate { .. } => "ttsGenerate",
            ClientEvent::TtsUpdateState { .. } => "ttsUpdateState",
            ClientEvent::Unknown { .. } => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamAction {
    Close,
    SpotterEnd,
    LazyTts,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StreamControlEvent {
    pub stream_id: u32,
    pub message_id: Option<String>,
    pub action: StreamAction,
    pub reason: Option<CloseReason>,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ClientMessage {
    Event(ClientEvent),
    StreamControl(StreamControlEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectiveType {
    ClientAction,
    ServerAction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Directive {
    pub r#type: DirectiveType,
    pub name: String,
    /// Empty string: none, as Yandex sends some typed commands. None: the name.
    pub analytics_type: Option<String>,
    /// Parameters of an ordinary command, a google.protobuf.Struct on the wire.
    pub payload: Option<Map<String, Value>>,
    /// A typed command (`*_directive`) takes a serialized proto message in field 15 instead of the Struct, which the
    /// station's protobuf-lite build cannot convert.
    pub typed_payload: Option<Bytes>,
    pub is_led_silent: Option<bool>,
    pub ignore_answer: Option<bool>,
}

impl Directive {
    /// An ordinary client command with a payload.
    pub fn client(name: &str, payload: Map<String, Value>) -> Directive {
        Directive {
            r#type: DirectiveType::ClientAction,
            name: name.to_string(),
            analytics_type: None,
            payload: Some(payload),
            typed_payload: None,
            is_led_silent: None,
            ignore_answer: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AliceResponseModel {
    pub request_id: String,
    pub response_id: String,
    pub sequence_number: Option<i64>,
    pub dialog_id: Option<String>,
    pub text: Option<String>,
    pub speech: Option<String>,
    /// Speech follows in TtsSpeak although the response has no text yet (an early response). None: speech is set.
    pub has_voice: Option<bool>,
    pub should_listen: Option<bool>,
    pub directives: Vec<Directive>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    SynchronizeStateResponse {
        r#ref: String,
        session_id: String,
    },
    InputStartAck {
        r#ref: String,
        request_start_time_us: i64,
    },
    AsrResult {
        r#ref: String,
        text: String,
        end_of_utt: bool,
    },
    SpotterValidation {
        r#ref: String,
        valid: bool,
    },
    LogAck {
        r#ref: String,
    },
    RequestStatAck {
        r#ref: String,
    },
    AliceResponse {
        r#ref: String,
        response: AliceResponseModel,
    },
    TtsSpeak {
        r#ref: String,
        stream_id: u32,
        format: String,
    },
    StreamClose {
        stream_id: u32,
        reason: CloseReason,
    },
    /// A server_action for the station to call back with: how speech nobody asked for gets a request to answer.
    Push {
        id: String,
        directive: Directive,
    },
}
