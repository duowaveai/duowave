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
}
