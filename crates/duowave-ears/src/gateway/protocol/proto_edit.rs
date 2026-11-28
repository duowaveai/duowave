//! Byte-level edits of protobuf messages: a field is replaced, added or removed and everything else stays as it
//! came, fields our schema does not know included. Frames we pass between the station and Yandex are edited this
//! way (a reference, a stream id), never decoded and encoded again, which would drop what we do not declare.

pub const MAGIC: &[u8; 4] = b"AAPI";

/// A message or frame that is not what it should be: a truncated varint, a field running past its message, …
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ProtoError(pub String);

impl ProtoError {
    pub fn new(message: impl Into<String>) -> ProtoError {
        ProtoError(message.into())
    }
}

/// A varint at `pos`: its value and the position after it.
pub(crate) fn read_varint(buf: &[u8], mut pos: usize) -> Result<(u64, usize), ProtoError> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        let Some(&byte) = buf.get(pos) else { return Err(ProtoError::new("truncated varint")) };
        pos += 1;
        result |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Ok((result, pos));
        }
        shift += 7;
        if shift > 63 {
            return Err(ProtoError::new("varint too long"));
        }
    }
}

pub(crate) fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

pub fn encode_varint(value: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(10);
    put_varint(&mut out, value);
    out
}

struct Field {
    field: u64,
    wire: u8,
    /// The whole field: key, length and value.
    start: usize,
    end: usize,
    /// The value alone (for length-delimited fields, without the length).
    value_start: usize,
}

fn fields_of(message: &[u8]) -> Result<Vec<Field>, ProtoError> {
    let mut fields = Vec::new();
    let mut pos = 0;
    while pos < message.len() {
        let start = pos;
        let (key, after_key) = read_varint(message, pos)?;
        pos = after_key;
        let field = key / 8;
        let wire = (key & 7) as u8;
        let mut value_start = pos;
        pos = match wire {
            0 => read_varint(message, pos)?.1,
            1 => pos + 8,
            5 => pos + 4,
            2 => {
                let (length, after_length) = read_varint(message, pos)?;
                value_start = after_length;
                usize::try_from(length).ok().and_then(|length| after_length.checked_add(length)).unwrap_or(usize::MAX)
            }
            _ => return Err(ProtoError::new(format!("unsupported wire type {wire}"))),
        };
        if pos > message.len() {
            return Err(ProtoError::new(format!("field {field} runs past the message")));
        }
        fields.push(Field { field, wire, start, end: pos, value_start });
    }
    Ok(fields)
}

fn length_delimited(field: u32, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len() + 6);
    put_varint(&mut out, u64::from(field) * 8 + 2);
    put_varint(&mut out, value.len() as u64);
    out.extend_from_slice(value);
    out
}

fn varint_field(field: u32, value: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    put_varint(&mut out, u64::from(field) * 8);
    put_varint(&mut out, value);
    out
}

/// The value of a length-delimited field (a nested message, a string, bytes); the first occurrence.
pub fn get_bytes(message: &[u8], field: u32) -> Result<Option<&[u8]>, ProtoError> {
    let found = fields_of(message)?.into_iter().find(|f| f.field == u64::from(field) && f.wire == 2);
    Ok(found.map(|f| &message[f.value_start..f.end]))
}

pub fn get_varint(message: &[u8], field: u32) -> Result<Option<u64>, ProtoError> {
    let found = fields_of(message)?.into_iter().find(|f| f.field == u64::from(field) && f.wire == 0);
    found.map(|f| read_varint(message, f.value_start).map(|(value, _)| value)).transpose()
}

pub fn get_string(message: &[u8], field: u32) -> Result<Option<String>, ProtoError> {
    Ok(get_bytes(message, field)?.map(|bytes| String::from_utf8_lossy(bytes).into_owned()))
}

/// Replaces every occurrence of a field with one new value, in place of the first; appends it when absent.
fn replace_field(message: &[u8], field: u32, encoded: Option<&[u8]>) -> Result<Vec<u8>, ProtoError> {
    let mut out = Vec::with_capacity(message.len() + encoded.map_or(0, <[u8]>::len));
    let mut pos = 0;
    let mut placed = false;
    for f in fields_of(message)? {
        if f.field != u64::from(field) {
            continue;
        }
        out.extend_from_slice(&message[pos..f.start]);
        if !placed {
            out.extend_from_slice(encoded.unwrap_or_default());
        }
        placed = true;
        pos = f.end;
    }
    out.extend_from_slice(&message[pos..]);
    if !placed {
        out.extend_from_slice(encoded.unwrap_or_default());
    }
    Ok(out)
}

/// A length-delimited field set to bytes or a string.
pub fn set_bytes(message: &[u8], field: u32, value: impl AsRef<[u8]>) -> Result<Vec<u8>, ProtoError> {
    replace_field(message, field, Some(&length_delimited(field, value.as_ref())))
}

pub fn set_varint(message: &[u8], field: u32, value: u64) -> Result<Vec<u8>, ProtoError> {
    replace_field(message, field, Some(&varint_field(field, value)))
}

pub fn remove_field(message: &[u8], field: u32) -> Result<Vec<u8>, ProtoError> {
    replace_field(message, field, None)
}

/// Keeps only the listed fields, in their order.
pub fn keep_fields(message: &[u8], fields: &[u32]) -> Result<Vec<u8>, ProtoError> {
    let mut out = Vec::with_capacity(message.len());
    for f in fields_of(message)? {
        if fields.iter().any(|&kept| u64::from(kept) == f.field) {
            out.extend_from_slice(&message[f.start..f.end]);
        }
    }
    Ok(out)
}

/// Edits a nested message field in place (every occurrence); nothing happens when the field is absent.
pub fn edit_message(
    message: &[u8],
    field: u32,
    mut edit: impl FnMut(&[u8]) -> Result<Vec<u8>, ProtoError>,
) -> Result<Vec<u8>, ProtoError> {
    let mut out = Vec::with_capacity(message.len());
    let mut pos = 0;
    for f in fields_of(message)? {
        if f.field != u64::from(field) || f.wire != 2 {
            continue;
        }
        out.extend_from_slice(&message[pos..f.start]);
        out.extend(length_delimited(field, &edit(&message[f.value_start..f.end])?));
        pos = f.end;
    }
    if pos == 0 {
        return Ok(message.to_vec());
    }
    out.extend_from_slice(&message[pos..]);
    Ok(out)
}

/// Moves a field to another number (voice_input 3 → text_input 2), value unchanged.
pub fn renumber_field(message: &[u8], from: u32, to: u32) -> Result<Vec<u8>, ProtoError> {
    let mut out = Vec::with_capacity(message.len() + 4);
    let mut pos = 0;
    for f in fields_of(message)? {
        if f.field != u64::from(from) {
            continue;
        }
        // A new key; the length (if any) and the value as they were.
        out.extend_from_slice(&message[pos..f.start]);
        put_varint(&mut out, u64::from(to) * 8 + u64::from(f.wire));
        let after_key = read_varint(message, f.start)?.1;
        out.extend_from_slice(&message[after_key..f.end]);
        pos = f.end;
    }
    out.extend_from_slice(&message[pos..]);
    Ok(out)
}

// ---------------------------------------------------------------- frames

pub fn is_aapi(frame: &[u8]) -> bool {
    frame.starts_with(MAGIC)
}

fn body(frame: &[u8]) -> Result<&[u8], ProtoError> {
    if !is_aapi(frame) {
        return Err(ProtoError::new("not an AAPI frame"));
    }
    Ok(&frame[MAGIC.len()..])
}

fn frame(message: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(MAGIC.len() + message.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(message);
    out
}

#[derive(Default, Clone, Copy)]
pub struct FrameEdit<'a> {
    /// A new ref_message_id in the event header (where there is one).
    pub r#ref: Option<&'a str>,
    /// A new message_id in the event header.
    pub message_id: Option<&'a str>,
    /// Maps a stream id (the event header's, the stream control's) to another; None keeps it.
    pub stream_id: Option<&'a dyn Fn(u32) -> Option<u32>>,
}

/// The stream id mapped, when there is a mapping for it (an id beyond u32 has none).
fn mapped_stream(edit: &FrameEdit, stream_id: u64) -> Option<u32> {
    let map = edit.stream_id?;
    map(u32::try_from(stream_id).ok()?)
}

/// Edits the header of an event frame or the stream id of a stream control frame, both directions (field 1 is
/// the event, field 2 the stream control, in ClientMessage and ServerMessage alike).
pub fn edit_frame(source: &[u8], edit: &FrameEdit) -> Result<Vec<u8>, ProtoError> {
    let mut message = edit_message(body(source)?, 1, |event| {
        edit_message(event, 1, |header| {
            let mut h = header.to_vec();
            if let Some(message_id) = edit.message_id {
                h = set_bytes(&h, 1, message_id)?;
            }
            if let Some(r#ref) = edit.r#ref {
                if get_bytes(&h, 2)?.is_some() {
                    h = set_bytes(&h, 2, r#ref)?;
                }
            }
            if edit.stream_id.is_some() {
                if let Some(stream_id) = get_varint(&h, 4)?.filter(|&id| id != 0) {
                    if let Some(mapped) = mapped_stream(edit, stream_id) {
                        h = set_varint(&h, 4, u64::from(mapped))?;
                    }
                }
            }
            Ok(h)
        })
    })?;
    if edit.stream_id.is_some() {
        message = edit_message(&message, 2, |control| match get_varint(control, 1)? {
            Some(stream_id) => match mapped_stream(edit, stream_id) {
                Some(mapped) => set_varint(control, 1, u64::from(mapped)),
                None => Ok(control.to_vec()),
            },
            None => Ok(control.to_vec()),
        })?;
    }
    Ok(frame(&message))
}

/// The audio frame `[u32 BE stream id][payload]` on another stream.
pub fn restream(audio: &[u8], stream_id: u32) -> Result<Vec<u8>, ProtoError> {
    if audio.len() < 4 {
        return Err(ProtoError::new(format!("an audio frame of {} bytes has no stream id", audio.len())));
    }
    let mut out = audio.to_vec();
    out[..4].copy_from_slice(&stream_id.to_be_bytes());
    Ok(out)
}

/// The station's VoiceInput as a TextInput with our recognised text: the same request (its header, application,
/// experiments, device state), a new message id, no stream, the event replaced with text, and the voice-only
/// fields left out. This is how the station itself shapes its TextInput.
pub fn voice_to_text_input(
    voice_input: &[u8],
    message_id: &str,
    text: &str,
    request_id: Option<&str>,
) -> Result<Vec<u8>, ProtoError> {
    let message = edit_message(body(voice_input)?, 1, |event| {
        let e = edit_message(event, 1, |header| {
            let h = remove_field(&set_bytes(header, 1, message_id)?, 4)?;
            with_header_request_id(h, request_id)
        })?;
        let e = renumber_field(&e, 3, 2)?;
        edit_message(&e, 2, |request| {
            // header, application, body, and the station's 11–13 and 18 (lang), as in its own TextInput
            let kept = keep_fields(request, &[1, 2, 3, 11, 12, 13, 18])?;
            let retexted = edit_message(&kept, 3, |request_body| set_bytes(request_body, 1, text_event(text)))?;
            with_request_id(retexted, request_id)
        })
    })?;
    Ok(frame(&message))
}

/// The event header's field 8 (where there is one) set to our own request id.
fn with_header_request_id(header: Vec<u8>, request_id: Option<&str>) -> Result<Vec<u8>, ProtoError> {
    match request_id {
        Some(request_id) if get_bytes(&header, 8)?.is_some() => set_bytes(&header, 8, request_id),
        _ => Ok(header),
    }
}

fn text_event(text: &str) -> Vec<u8> {
    let mut event = varint_field(1, TEXT_INPUT);
    event.extend(length_delimited(5, text.as_bytes()));
    event
}

/// A request id of its own (a lookup that is not the station's request): AliceRequest.header.request_id.
fn with_request_id(request: Vec<u8>, request_id: Option<&str>) -> Result<Vec<u8>, ProtoError> {
    match request_id {
        Some(request_id) => edit_message(&request, 1, |header| set_bytes(header, 1, request_id)),
        None => Ok(request),
    }
}

/// The station's TextInput with another text and message id (a typed question asked again quietly).
pub fn retext_text_input(
    text_input: &[u8],
    message_id: &str,
    text: &str,
    request_id: Option<&str>,
) -> Result<Vec<u8>, ProtoError> {
    let message = edit_message(body(text_input)?, 1, |event| {
        let e = edit_message(event, 1, |header| with_header_request_id(set_bytes(header, 1, message_id)?, request_id))?;
        edit_message(&e, 2, |request| {
            let retexted = edit_message(request, 3, |request_body| set_bytes(request_body, 1, text_event(text)))?;
            with_request_id(retexted, request_id)
        })
    })?;
    Ok(frame(&message))
}

/// EventType.text_input
const TEXT_INPUT: u64 = 2;

// ---------------------------------------------------------------- Yandex's ear, our brain (docs/plans/yandex-ear.md)

/// AliceRequest.request.uniproxy_options and its wait_client_approval.
const UNIPROXY_OPTIONS: u32 = 17;
const WAIT_CLIENT_APPROVAL: u32 = 4;

/// The station's VoiceInput asking Yandex to wait (`UniproxyOptions.WaitClientApproval`): Yandex recognizes the speech,
/// sends its final AsrResult and holds its answer until a ContinueRequest or a CancelRequest. Everything else stays as
/// the station sent it, the options it set itself included.
pub fn wait_for_approval(voice_input: &[u8]) -> Result<Vec<u8>, ProtoError> {
    let message = edit_message(body(voice_input)?, 1, |event| {
        edit_message(event, 3, |request| {
            edit_message(request, 3, |request_body| {
                let options = get_bytes(request_body, UNIPROXY_OPTIONS)?.unwrap_or_default();
                set_bytes(request_body, UNIPROXY_OPTIONS, set_varint(options, WAIT_CLIENT_APPROVAL, 1)?)
            })
        })
    })?;
    Ok(frame(&message))
}

/// Whether a VoiceInput asks Yandex to wait: a request without its body cannot be marked, and goes unmarked.
pub fn waits_for_approval(voice_input: &[u8]) -> bool {
    let read = || -> Result<Option<u64>, ProtoError> {
        let event = get_bytes(body(voice_input)?, 1)?.unwrap_or_default();
        let request = get_bytes(event, 3)?.unwrap_or_default();
        let request_body = get_bytes(request, 3)?.unwrap_or_default();
        let options = get_bytes(request_body, UNIPROXY_OPTIONS)?.unwrap_or_default();
        get_varint(options, WAIT_CLIENT_APPROVAL)
    };
    read().ok().flatten() == Some(1)
}

/// A request we give to Yandex to answer, without asking it to wait: the wait the station asked for goes.
///
/// The station asks Yandex to wait on its own requests (`WaitClientApproval`, live, 28.11: on every voiceInput, with
/// a `continueRequest` naming it about two seconds in). A request sent on under our message id is one that
/// continueRequest never names: Yandex recognizes it, holds its answer, and about three minutes later gives up with
/// «error processing response chunk» (live, 26.11–28.11: «Включи музыку», «Гитербой», every voiceInput and the one
/// textInput we sent). The station's other options stay. A request that asks nothing, and a frame that is not a voice
/// or text input, stay as they are.
pub fn answer_without_approval(input: &[u8]) -> Result<Vec<u8>, ProtoError> {
    let message = edit_message(body(input)?, 1, |event| {
        let event = edit_message(event, 3, without_approval_wait)?;
        edit_message(&event, 2, without_approval_wait)
    })?;
    Ok(frame(&message))
}

fn without_approval_wait(request: &[u8]) -> Result<Vec<u8>, ProtoError> {
    edit_message(request, 3, |request_body| {
        edit_message(request_body, UNIPROXY_OPTIONS, |options| remove_field(options, WAIT_CLIENT_APPROVAL))
    })
}

/// AliceRequest.request: voice_session, reset_session, activation_type.
const VOICE_SESSION: u32 = 6;
const RESET_SESSION: u32 = 7;
const ACTIVATION_TYPE: u32 = 14;

/// A follow-up of a dialog Yandex does not own, sent to Yandex as a new request.
///
/// The station marks that follow-up `voiceSession` and `auto_listening`: a session Yandex did not open. The timeouts
/// once put down to it (live, 26.11: «Любую музыку» and «какая погода?» after `ASR_OK`) were the station's wait for
/// its approval (`answer_without_approval`); its `continueRequest` names the follow-up itself, not our answer.
/// `voice_session` goes off, `reset_session` goes on, and `auto_listening` is removed. A request with no body, and a
/// frame that is not a voice or text input, stay as they are.
pub fn detach_voice_session(input: &[u8]) -> Result<Vec<u8>, ProtoError> {
    let message = edit_message(body(input)?, 1, |event| {
        let event = edit_message(event, 3, detach_request_body)?;
        edit_message(&event, 2, detach_request_body)
    })?;
    Ok(frame(&message))
}

fn detach_request_body(request: &[u8]) -> Result<Vec<u8>, ProtoError> {
    if get_bytes(request, 3)?.is_none() {
        return Ok(request.to_vec());
    }
    edit_message(request, 3, |request_body| {
        let mut body = set_varint(request_body, VOICE_SESSION, 0)?;
        body = set_varint(&body, RESET_SESSION, 1)?;
        if get_string(&body, ACTIVATION_TYPE)?.as_deref() == Some("auto_listening") {
            body = remove_field(&body, ACTIVATION_TYPE)?;
        }
        Ok(body)
    })
}

/// «Answer it yourself»: the ContinueRequest for a VoiceInput Yandex holds. Its header is the request's own (as the
/// station would send it) with a new message id, no reference and no stream.
pub fn continue_request(voice_input: &[u8], message_id: &str) -> Result<Vec<u8>, ProtoError> {
    let event = get_bytes(body(voice_input)?, 1)?.ok_or_else(|| ProtoError::new("a frame without its event"))?;
    let request_header = get_bytes(event, 1)?.unwrap_or_default();
    let request_message_id = get_string(request_header, 1)?.unwrap_or_default();
    let header = remove_field(&remove_field(&set_bytes(request_header, 1, message_id)?, 2)?, 4)?;
    let mut continued = length_delimited(1, &header);
    continued.extend(length_delimited(10, &length_delimited(1, request_message_id.as_bytes())));
    Ok(frame(&length_delimited(1, &continued)))
}

/// «We answer it here»: the CancelRequest of a request Yandex holds, its reason local execution.
pub fn cancel_request(message_id: &str, request_message_id: &str) -> Vec<u8> {
    let mut cancel = length_delimited(1, request_message_id.as_bytes());
    cancel.extend(length_delimited(4, &[]));
    let mut event = length_delimited(1, &length_delimited(1, message_id.as_bytes()));
    event.extend(length_delimited(11, &cancel));
    frame(&length_delimited(1, &event))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::gateway::protocol::aapi_codec::{AapiCodec, DecodedClientFrame};
    use crate::gateway::protocol::protocol_types::{ClientEvent, ClientMessage, CloseReason, ServerMessage};

    #[test]
    fn renames_the_ref_and_the_stream_of_a_yandex_event_keeping_fields_our_schema_does_not_know() {
        let codec = AapiCodec::new();
        let speak = ServerMessage::TtsSpeak { r#ref: "yandex-msg".into(), stream_id: 7, format: "audio/opus".into() };
        let original = codec.encode_server(&speak).unwrap().frame;
        // An unknown field 99 in the event, as Yandex may send.
        let unknown = [0x9a, 0x06, 0x02, 0x68, 0x69];
        let event_length = (original.len() - 4 - 2 + unknown.len()) as u8;
        let with_unknown = [&original[..4], &[0x0a, event_length], &original[6..], &unknown].concat();
        let map = |id: u32| (id == 7).then_some(4);
        let edit = FrameEdit { r#ref: Some("station-msg"), stream_id: Some(&map), ..Default::default() };
        let edited = edit_frame(&with_unknown, &edit).unwrap();
        let decoded = codec.decode_server_frame(&edited).unwrap();
        assert_eq!(decoded["event"]["header"]["refMessageId"], "station-msg");
        assert_eq!(decoded["event"]["header"]["streamId"], 4);
        assert_eq!(decoded["event"]["ttsSpeak"]["format"], "audio/opus");
        assert!(edited.windows(unknown.len()).any(|window| window == unknown));
    }

    #[test]
    fn renumbers_a_stream_close_and_an_audio_frame() {
        let codec = AapiCodec::new();
        let close =
            codec.encode_server(&ServerMessage::StreamClose { stream_id: 7, reason: CloseReason::Ok }).unwrap().frame;
        let ten = |_: u32| Some(10);
        let edited = edit_frame(&close, &FrameEdit { stream_id: Some(&ten), ..Default::default() }).unwrap();
        assert_eq!(codec.decode_server_frame(&edited).unwrap()["streamControl"]["streamId"], 10);
        let audio = [&[0, 0, 0, 7][..], b"OggS"].concat();
        assert_eq!(restream(&audio, 10).unwrap()[..4], [0, 0, 0, 10]);
        assert_eq!(&restream(&audio, 10).unwrap()[4..], b"OggS");
    }

    #[test]
    fn makes_the_station_voice_input_a_text_input_with_our_text_the_request_otherwise_unchanged() {
        let codec = AapiCodec::new();
        let voice = codec
            .encode_client_object(&json!({ "event": {
                "header": { "messageId": "voice-1", "streamId": 5 },
                "voiceInput": {
                    "header": { "requestId": "req-1", "sequenceNumber": 24 },
                    "application": { "deviceModel": "plum", "timezone": "Asia/Dubai" },
                    "request": { "event": { "type": "voice_input" }, "voiceSession": true, "activationType": "spotter" },
                    "enableSpotterValidation": true,
                    "spotterPhrase": "алиса",
                    "lang": "ru-RU",
                    "topic": "quasar-general",
                    "format": "audio/opus",
                },
            } }))
            .unwrap();
        let text = voice_to_text_input(&voice, "yandex-1", "включи радио", None).unwrap();
        let DecodedClientFrame { message, decoded } = codec.decode_client(&text).unwrap();
        assert!(matches!(message, ClientMessage::Event(ClientEvent::TextInput { .. })), "{message:?}");
        assert_eq!(decoded["event"]["header"], json!({ "messageId": "yandex-1" }));
        let input = &decoded["event"]["textInput"];
        assert_eq!(input["header"], json!({ "requestId": "req-1", "sequenceNumber": 24 }));
        assert_eq!(input["application"], json!({ "deviceModel": "plum", "timezone": "Asia/Dubai" }));
        assert_eq!(
            input["request"],
            json!({ "event": { "type": "text_input", "text": "включи радио" }, "voiceSession": true, "activationType": "spotter" })
        );
        assert_eq!(input["lang"], "ru-RU");
        // Voice-only fields are gone.
        assert_eq!(input.get("spotterPhrase"), None);
        assert_eq!(input.get("format"), None);
        assert!(get_string(&text[4..], 1).unwrap().is_some());
    }

    #[test]
    fn asks_a_text_input_again_with_another_text_and_its_own_request_id() {
        let codec = AapiCodec::new();
        let callback = codec
            .encode_client_object(&json!({ "event": {
                "header": { "messageId": "callback-1" },
                "textInput": {
                    "header": { "requestId": "req-callback", "sequenceNumber": 3 },
                    "request": { "event": { "type": "server_action", "name": "vgw_reminder" }, "voiceSession": false },
                },
            } }))
            .unwrap();
        let again = retext_text_input(&callback, "yandex-3", "ещё раз", Some("own-req")).unwrap();
        let decoded = codec.decode_client(&again).unwrap().decoded;
        assert_eq!(decoded["event"]["header"], json!({ "messageId": "yandex-3" }));
        assert_eq!(decoded["event"]["textInput"]["header"], json!({ "requestId": "own-req", "sequenceNumber": 3 }));
        assert_eq!(
            decoded["event"]["textInput"]["request"],
            json!({ "event": { "type": "text_input", "text": "ещё раз" }, "voiceSession": false })
        );
        assert_eq!(edit_frame(b"JSON", &FrameEdit::default()), Err(ProtoError::new("not an AAPI frame")));
    }

    fn station_voice(codec: &AapiCodec) -> bytes::Bytes {
        codec
            .encode_client_object(&json!({ "event": {
                "header": { "messageId": "voice-7", "streamId": 3 },
                "voiceInput": {
                    "header": { "requestId": "req-7", "sequenceNumber": 2 },
                    "request": { "event": { "type": "voice_input" }, "voiceSession": true, "activationType": "spotter" },
                    "lang": "ru-RU",
                },
            } }))
            .unwrap()
    }

    #[test]
    fn asks_yandex_to_wait_keeping_the_request_and_the_options_the_station_set() {
        let codec = AapiCodec::new();
        let voice = station_voice(&codec);
        assert!(!waits_for_approval(&voice));
        let gated = wait_for_approval(&voice).unwrap();
        assert!(waits_for_approval(&gated));
        let decoded = codec.decode_client(&gated).unwrap().decoded;
        assert_eq!(decoded["event"]["header"], json!({ "messageId": "voice-7", "streamId": 3 }));
        assert_eq!(decoded["event"]["voiceInput"]["request"]["activationType"], "spotter");
        // An option the station set itself (megamind_url, field 1) stays beside ours.
        let with_option = edit_message(&voice[4..], 1, |event| {
            edit_message(event, 3, |request| {
                edit_message(request, 3, |request_body| set_bytes(request_body, 17, length_delimited(1, b"mm")))
            })
        })
        .unwrap();
        let gated = wait_for_approval(&frame(&with_option)).unwrap();
        let event = get_bytes(&gated[4..], 1).unwrap().unwrap();
        let options =
            get_bytes(get_bytes(get_bytes(event, 3).unwrap().unwrap(), 3).unwrap().unwrap(), 17).unwrap().unwrap();
        assert_eq!(get_string(options, 1).unwrap().as_deref(), Some("mm"));
        assert_eq!(get_varint(options, 4).unwrap(), Some(1));
        // Twice is once.
        assert_eq!(wait_for_approval(&gated).unwrap(), gated);
    }

    #[test]
    fn detaches_a_follow_up_from_a_voice_session_yandex_does_not_own() {
        let codec = AapiCodec::new();
        let voice = codec
            .encode_client_object(&json!({ "event": {
                "header": { "messageId": "voice-9", "streamId": 3 },
                "voiceInput": {
                    "header": { "requestId": "req-9", "sequenceNumber": 4 },
                    "request": {
                        "event": { "type": "voice_input" },
                        "voiceSession": true,
                        "activationType": "auto_listening",
                    },
                    "lang": "ru-RU",
                },
            } }))
            .unwrap();
        let detached = detach_voice_session(&voice).unwrap();
        let decoded = codec.decode_client(&detached).unwrap().decoded;
        assert_eq!(decoded["event"]["header"], json!({ "messageId": "voice-9", "streamId": 3 }));
        assert_eq!(decoded["event"]["voiceInput"]["header"]["requestId"], "req-9");
        assert_eq!(decoded["event"]["voiceInput"]["lang"], "ru-RU");
        let request = &decoded["event"]["voiceInput"]["request"];
        assert_eq!(request["voiceSession"], false);
        assert_eq!(request["resetSession"], true);
        assert_eq!(request.get("activationType"), None);

        let text = voice_to_text_input(&voice, "text-9", "какая погода", None).unwrap();
        let detached = detach_voice_session(&text).unwrap();
        let decoded = codec.decode_client(&detached).unwrap().decoded;
        let request = &decoded["event"]["textInput"]["request"];
        assert_eq!(request["event"]["text"], "какая погода");
        assert_eq!(request["voiceSession"], false);
        assert_eq!(request["resetSession"], true);

        let close = codec
            .encode_client_object(&json!({ "streamControl": { "streamId": 3, "messageId": "close-1", "close": {} } }))
            .unwrap();
        assert_eq!(detach_voice_session(&close).unwrap(), close.to_vec());
    }

    #[test]
    fn lets_yandex_answer_a_request_the_station_asked_it_to_hold() {
        let codec = AapiCodec::new();
        // The station asks Yandex to wait itself (live, 28.11: options 2, 4 = 1 and an empty 10 on every voiceInput).
        let held = edit_message(&station_voice(&codec)[4..], 1, |event| {
            edit_message(event, 3, |request| {
                edit_message(request, 3, |request_body| {
                    let mut options = length_delimited(2, b"station-option");
                    options.extend(varint_field(WAIT_CLIENT_APPROVAL, 1));
                    options.extend(length_delimited(10, b""));
                    set_bytes(request_body, UNIPROXY_OPTIONS, options)
                })
            })
        })
        .unwrap();
        let held = frame(&held);
        assert!(waits_for_approval(&held));

        let answered = answer_without_approval(&held).unwrap();
        assert!(!waits_for_approval(&answered));
        let decoded = codec.decode_client(&answered).unwrap().decoded;
        assert_eq!(decoded["event"]["header"], json!({ "messageId": "voice-7", "streamId": 3 }));
        assert_eq!(decoded["event"]["voiceInput"]["request"]["activationType"], "spotter");
        // The station's other options stay.
        let event = get_bytes(&answered[4..], 1).unwrap().unwrap();
        let options =
            get_bytes(get_bytes(get_bytes(event, 3).unwrap().unwrap(), 3).unwrap().unwrap(), 17).unwrap().unwrap();
        assert_eq!(get_string(options, 2).unwrap().as_deref(), Some("station-option"));
        assert_eq!(get_bytes(options, 10).unwrap(), Some(&b""[..]));

        // Our text in the station's request keeps its options, the wait among them: it goes without the wait too.
        let text_wait = |frame: &[u8]| {
            let event = get_bytes(&frame[4..], 1).unwrap().unwrap();
            let request_body = get_bytes(get_bytes(event, 2).unwrap().unwrap(), 3).unwrap().unwrap();
            get_bytes(request_body, UNIPROXY_OPTIONS)
                .unwrap()
                .and_then(|options| get_varint(options, WAIT_CLIENT_APPROVAL).unwrap())
        };
        let text = voice_to_text_input(&held, "text-7", "включи музыку", None).unwrap();
        assert_eq!(text_wait(&text), Some(1));
        assert_eq!(text_wait(&answer_without_approval(&text).unwrap()), None);

        // A request that asks nothing, and a frame that is not an input, stay as they are.
        let plain = station_voice(&codec);
        assert_eq!(answer_without_approval(&plain).unwrap(), plain.to_vec());
        let close = codec
            .encode_client_object(&json!({ "streamControl": { "streamId": 3, "messageId": "close-1", "close": {} } }))
            .unwrap();
        assert_eq!(answer_without_approval(&close).unwrap(), close.to_vec());
    }

    #[test]
    fn continues_and_cancels_the_request_yandex_holds() {
        let codec = AapiCodec::new();
        let voice = station_voice(&codec);
        let continued = continue_request(&voice, "cont-1").unwrap();
        let decoded = codec.decode_client(&continued).unwrap().decoded;
        assert_eq!(decoded["event"]["header"], json!({ "messageId": "cont-1" }));
        let request = get_bytes(get_bytes(&continued[4..], 1).unwrap().unwrap(), 10).unwrap().unwrap();
        assert_eq!(get_string(request, 1).unwrap().as_deref(), Some("voice-7"));

        let cancel = cancel_request("cancel-1", "voice-7");
        let DecodedClientFrame { message, .. } = codec.decode_client(&cancel).unwrap();
        assert_eq!(
            message,
            ClientMessage::Event(ClientEvent::CancelRequest {
                header: crate::gateway::protocol::protocol_types::EventHeader {
                    message_id: "cancel-1".into(),
                    ..Default::default()
                },
                request_message_id: Some("voice-7".into()),
                reason: crate::gateway::protocol::protocol_types::CancelReason::LocalExecution,
            })
        );
    }
}
