//! Schema-less protobuf reader for the debug log: shows field numbers, wire types and values of messages the
//! gateway has no schema for, so unknown station events can be studied from the log.

use serde::{Serialize, Serializer};
use serde_json::Value;

use super::proto_edit::{read_varint, ProtoError};
use super::proto_object::js_number;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RawField {
    pub field: u64,
    pub wire: RawWire,
    pub value: RawValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RawWire {
    Varint,
    Fixed64,
    Bytes,
    Fixed32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RawValue {
    Varint(u64),
    /// A fixed64 read as a double, as the TS view read it.
    Double(f64),
    Fixed32(u32),
    Text(String),
    Fields(Vec<RawField>),
}

impl Serialize for RawValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            RawValue::Varint(value) => serializer.serialize_u64(*value),
            RawValue::Double(value) => js_number(*value).serialize(serializer),
            RawValue::Fixed32(value) => serializer.serialize_u32(*value),
            RawValue::Text(text) => serializer.serialize_str(text),
            RawValue::Fields(fields) => fields.serialize(serializer),
        }
    }
}

const MAX_DEPTH: usize = 6;

fn looks_like_text(buf: &[u8]) -> bool {
    match std::str::from_utf8(buf) {
        Ok(text) => !text.chars().any(|c| c == '\u{FFFD}' || matches!(c, '\u{0}'..='\u{8}' | '\u{E}'..='\u{1F}')),
        Err(_) => false,
    }
}

pub fn decode_raw(buf: &[u8], depth: usize) -> Result<Vec<RawField>, ProtoError> {
    let mut fields = Vec::new();
    let mut pos = 0;
    while pos < buf.len() {
        let (key, after_key) = read_varint(buf, pos)?;
        pos = after_key;
        let field = key / 8;
        let wire_type = key & 7;
        if field == 0 {
            return Err(ProtoError::new("field number 0"));
        }
        match wire_type {
            0 => {
                let (value, after) = read_varint(buf, pos)?;
                pos = after;
                fields.push(RawField { field, wire: RawWire::Varint, value: RawValue::Varint(value) });
            }
            1 => {
                let bytes = buf.get(pos..pos + 8).ok_or_else(|| ProtoError::new("truncated fixed64"))?;
                let value = f64::from_le_bytes(bytes.try_into().unwrap_or_default());
                fields.push(RawField { field, wire: RawWire::Fixed64, value: RawValue::Double(value) });
                pos += 8;
            }
            5 => {
                let bytes = buf.get(pos..pos + 4).ok_or_else(|| ProtoError::new("truncated fixed32"))?;
                let value = u32::from_le_bytes(bytes.try_into().unwrap_or_default());
                fields.push(RawField { field, wire: RawWire::Fixed32, value: RawValue::Fixed32(value) });
                pos += 4;
            }
            2 => {
                let (length, after) = read_varint(buf, pos)?;
                let end = usize::try_from(length).ok().and_then(|length| after.checked_add(length));
                let chunk =
                    end.and_then(|end| buf.get(after..end)).ok_or_else(|| ProtoError::new("truncated bytes"))?;
                pos = after + chunk.len();
                fields.push(RawField { field, wire: RawWire::Bytes, value: describe_bytes(chunk, depth) });
            }
            _ => return Err(ProtoError::new(format!("unsupported wire type {wire_type}"))),
        }
    }
    Ok(fields)
}

fn describe_bytes(chunk: &[u8], depth: usize) -> RawValue {
    if depth < MAX_DEPTH && !chunk.is_empty() {
        // Not a nested message when it does not parse as one; then text or hex.
        if let Ok(fields) = decode_raw(chunk, depth + 1) {
            return RawValue::Fields(fields);
        }
    }
    if looks_like_text(chunk) {
        return RawValue::Text(String::from_utf8_lossy(chunk).into_owned());
    }
    let more = if chunk.len() > 64 { format!("…({} bytes)", chunk.len()) } else { String::new() };
    RawValue::Text(format!("0x{}{more}", hex::encode(&chunk[..chunk.len().min(64)])))
}

/// Best-effort raw view as the journal writes it (the fields, or a line saying why not); never fails, for
/// logging only.
pub fn describe_raw(buf: &[u8]) -> Value {
    match decode_raw(buf, 0) {
        Ok(fields) => serde_json::to_value(fields).unwrap_or(Value::Null),
        Err(error) => Value::String(format!("undecodable ({error}): 0x{}", hex::encode(&buf[..buf.len().min(64)]))),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::gateway::protocol::proto_edit::put_varint;

    /// A length-delimited field, to build messages our schema does not describe.
    fn field(number: u64, value: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(&mut out, (number << 3) | 2);
        put_varint(&mut out, value.len() as u64);
        out.extend_from_slice(value);
        out
    }

    #[test]
    fn shows_a_message_without_a_schema_for_the_journal() {
        // A server event unknown to our schema, as a new Yandex feature would look: nested messages and text.
        let feature = [field(1, b"is_whisper"), vec![0x10, 0x01]].concat();
        let event = field(1, &[field(1, &field(1, b"m-77")), field(77, &feature)].concat());
        assert_eq!(
            describe_raw(&event),
            json!([{ "field": 1, "wire": "bytes", "value": [
                { "field": 1, "wire": "bytes", "value": [{ "field": 1, "wire": "bytes", "value": "m-77" }] },
                { "field": 77, "wire": "bytes", "value": [
                    { "field": 1, "wire": "bytes", "value": "is_whisper" },
                    { "field": 2, "wire": "varint", "value": 1 },
                ] },
            ] }])
        );
        // Fixed values (a fixed64 read as a double), long bytes cut in hex, what does not parse.
        assert_eq!(
            describe_raw(&hex::decode("09000000000000f83f0d01000000").unwrap()),
            json!([{ "field": 1, "wire": "fixed64", "value": 1.5 }, { "field": 1, "wire": "fixed32", "value": 1 }])
        );
        let long = field(1, &[0xff; 80]);
        assert_eq!(
            describe_raw(&long),
            json!([{ "field": 1, "wire": "bytes", "value": format!("0x{}…(80 bytes)", "ff".repeat(64)) }])
        );
        assert_eq!(describe_raw(&[0x0a, 0x05, 0x01]), json!("undecodable (truncated bytes): 0x0a0501"));
        assert_eq!(describe_raw(&[0x02, 0x01]), json!("undecodable (field number 0): 0x0201"));
    }
}
