//! protobufjs's plain objects over the station's schema: what the TS codec got from `Type.decode` + `Type.toObject`
//! and gave to `Type.fromObject` + `Type.encode`, as serde_json values. The schema is proto/station-protocol.proto
//! with the well-known types it imports, embedded at build time (build.rs).
//!
//! The objects are protobufjs's, not the proto3 JSON mapping: field names in camelCase, 64-bit integers as numbers,
//! enums by name or by number (as the caller asks), Struct and Duration as the messages they are
//! (`{fields: {…}}`, `{seconds, nanos}`), and bytes as `{"$bytes": "<base64>"}` where the TS had a Buffer
//! (`bytes_value`, `value_bytes`). A decoded object has every field that came on the wire, default values included;
//! a singular field that comes twice is the last one (protobufjs replaces a message that comes again, it does not
//! merge). Encoding writes every field the object has, default values included, in field number order: the bytes
//! protobufjs wrote, which the station has always got.
//!
//! Two differences from protobufjs: a map's entries (a Struct's keys) are written in key order, where protobufjs
//! kept the object's insertion order; and a known field that comes with another wire type is skipped as unknown,
//! as protobuf's C++ (the station's) reads it, where protobufjs misread it.

use std::sync::LazyLock;

use base64::alphabet;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, STANDARD};
use base64::engine::DecodePaddingMode;
use base64::Engine as _;
use prost_reflect::{DescriptorPool, EnumDescriptor, FieldDescriptor, Kind, MessageDescriptor};
use serde_json::{Map, Number, Value};

use super::proto_edit::{put_varint, read_varint, ProtoError};

const SCHEMA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/station-protocol.fds"));

/// The proto package of the station's messages.
pub const PACKAGE: &str = "duowave.station";

/// protoc's and protobufjs's nesting limit.
const RECURSION_LIMIT: u32 = 100;

/// The key of a bytes value in a plain object.
const BYTES_KEY: &str = "$bytes";

static POOL: LazyLock<DescriptorPool> =
    LazyLock::new(|| DescriptorPool::decode(SCHEMA).expect("the schema build.rs encoded decodes"));

/// protobufjs's base64 reader: the standard alphabet, padding optional.
const LENIENT_BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

/// A message type by its name in the package (`AlarmCapability`) or its full name (`google.protobuf.Struct`).
pub fn message_type(name: &str) -> Result<MessageDescriptor, ProtoError> {
    let full = if name.contains('.') { name.to_string() } else { format!("{PACKAGE}.{name}") };
    POOL.get_message_by_name(&full).ok_or_else(|| ProtoError::new(format!("no such type: {full}")))
}

/// How a decoded object gives enums: protobufjs's `enums: String`, or their numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enums {
    Names,
    Numbers,
}

/// A bytes value of a plain object (a Buffer in the TS).
pub fn bytes_value(bytes: &[u8]) -> Value {
    let mut object = Map::new();
    object.insert(BYTES_KEY.to_string(), Value::String(STANDARD.encode(bytes)));
    Value::Object(object)
}

/// The bytes of a bytes value (`bytes_value`); None for anything else.
pub fn value_bytes(value: &Value) -> Option<Vec<u8>> {
    let Value::Object(object) = value else { return None };
    if object.len() != 1 {
        return None;
    }
    LENIENT_BASE64.decode(object.get(BYTES_KEY)?.as_str()?).ok()
}

/// A field's name as protobufjs gives it: the proto name in camelCase (`_x` → `X` for a lowercase x).
pub fn field_name(field: &FieldDescriptor) -> String {
    let name = field.name();
    let mut out = String::with_capacity(name.len());
    let mut chars = name.chars();
    out.extend(chars.next());
    let mut chars = chars.peekable();
    while let Some(c) = chars.next() {
        if c == '_' {
            if let Some(next) = chars.next_if(char::is_ascii_lowercase) {
                out.push(next.to_ascii_uppercase());
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// A double as JavaScript has it in a JSON value: integral values as integers (`1`, not `1.0`), NaN and the
/// infinities as null (JSON.stringify's null).
pub fn js_number(value: f64) -> Value {
    if !value.is_finite() {
        return Value::Null;
    }
    if value.fract() == 0.0 && value.abs() < 9.2e18 {
        return Value::Number(Number::from(value as i64));
    }
    Number::from_f64(value).map_or(Value::Null, Value::Number)
}

// ---------------------------------------------------------------- decode + toObject

/// `Type.decode` and `Type.toObject` with `{longs: Number, defaults: false}` and the enums as asked.
pub fn decode(message: &MessageDescriptor, bytes: &[u8], enums: Enums) -> Result<Value, ProtoError> {
    decode_message(message, bytes, enums, 0).map(Value::Object)
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, pos: 0 }
    }

    fn done(&self) -> bool {
        self.pos >= self.buf.len()
    }

    fn varint(&mut self) -> Result<u64, ProtoError> {
        let (value, pos) = read_varint(self.buf, self.pos)?;
        self.pos = pos;
        Ok(value)
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], ProtoError> {
        let end = self.pos.checked_add(length).filter(|&end| end <= self.buf.len()).ok_or_else(|| {
            ProtoError::new(format!("index out of range: {} + {length} > {}", self.pos, self.buf.len()))
        })?;
        let bytes = &self.buf[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn length_delimited(&mut self) -> Result<&'a [u8], ProtoError> {
        let length = self.varint()?;
        self.take(usize::try_from(length).unwrap_or(usize::MAX))
    }

    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], ProtoError> {
        let mut bytes = [0u8; N];
        bytes.copy_from_slice(self.take(N)?);
        Ok(bytes)
    }

    /// A field we do not read (protobufjs's skipType).
    fn skip(&mut self, wire: u8, depth: u32) -> Result<(), ProtoError> {
        if depth > RECURSION_LIMIT {
            return Err(ProtoError::new("maximum nesting depth exceeded"));
        }
        match wire {
            0 => {
                self.varint()?;
            }
            1 => {
                self.take(8)?;
            }
            2 => {
                self.length_delimited()?;
            }
            3 => loop {
                let inner = (self.varint()? & 7) as u8;
                if inner == 4 {
                    break;
                }
                self.skip(inner, depth + 1)?;
            },
            5 => {
                self.take(4)?;
            }
            _ => return Err(ProtoError::new(format!("invalid wire type {wire} at offset {}", self.pos))),
        }
        Ok(())
    }
}

/// The wire type a value of this kind comes with (not packed).
fn wire_of(kind: &Kind) -> u8 {
    match kind {
        Kind::Double | Kind::Fixed64 | Kind::Sfixed64 => 1,
        Kind::Float | Kind::Fixed32 | Kind::Sfixed32 => 5,
        Kind::String | Kind::Bytes | Kind::Message(_) => 2,
        _ => 0,
    }
}

fn packable(kind: &Kind) -> bool {
    !matches!(kind, Kind::String | Kind::Bytes | Kind::Message(_))
}

fn decode_message(
    message: &MessageDescriptor,
    bytes: &[u8],
    enums: Enums,
    depth: u32,
) -> Result<Map<String, Value>, ProtoError> {
    if depth > RECURSION_LIMIT {
        return Err(ProtoError::new("maximum nesting depth exceeded"));
    }
    let mut object = Map::new();
    let mut reader = Reader::new(bytes);
    while !reader.done() {
        let key = reader.varint()?;
        let wire = (key & 7) as u8;
        let Some(field) = u32::try_from(key >> 3).ok().and_then(|number| message.get_field(number)) else {
            reader.skip(wire, depth)?;
            continue;
        };
        let kind = field.kind();
        let packed = field.is_list() && wire == 2 && packable(&kind);
        if field.is_group() || (wire != wire_of(&kind) && !packed) {
            reader.skip(wire, depth)?;
            continue;
        }
        let name = field_name(&field);
        if field.is_map() {
            let (key, value) = decode_map_entry(&kind, reader.length_delimited()?, enums, depth)?;
            if let Value::Object(entries) = object.entry(name).or_insert_with(|| Value::Object(Map::new())) {
                entries.insert(key, value);
            }
        } else if field.is_list() {
            let mut values = Vec::new();
            if packed {
                // Packed or not, as protobufjs reads either.
                let mut run = Reader::new(reader.length_delimited()?);
                while !run.done() {
                    values.push(read_value(&mut run, &kind, enums, depth)?);
                }
            } else {
                values.push(read_value(&mut reader, &kind, enums, depth)?);
            }
            if !values.is_empty() {
                if let Value::Array(list) = object.entry(name).or_insert_with(|| Value::Array(Vec::new())) {
                    list.extend(values);
                }
            }
        } else {
            let value = read_value(&mut reader, &kind, enums, depth)?;
            object.insert(name, value);
        }
    }
    Ok(object)
}

/// One value of a field's kind, at the reader.
fn read_value(reader: &mut Reader, kind: &Kind, enums: Enums, depth: u32) -> Result<Value, ProtoError> {
    Ok(match kind {
        Kind::Message(message) => Value::Object(decode_message(message, reader.length_delimited()?, enums, depth + 1)?),
        Kind::String => Value::String(String::from_utf8_lossy(reader.length_delimited()?).into_owned()),
        Kind::Bytes => bytes_value(reader.length_delimited()?),
        Kind::Enum(descriptor) => enum_value(descriptor, reader.varint()? as i32, enums),
        Kind::Bool => Value::Bool(reader.varint()? != 0),
        Kind::Int32 => Value::from(reader.varint()? as i32),
        Kind::Uint32 => Value::from(reader.varint()? as u32),
        Kind::Sint32 => {
            let raw = reader.varint()? as u32;
            Value::from((raw >> 1) as i32 ^ -((raw & 1) as i32))
        }
        Kind::Int64 => Value::from(reader.varint()? as i64),
        Kind::Uint64 => Value::from(reader.varint()?),
        Kind::Sint64 => {
            let raw = reader.varint()?;
            Value::from((raw >> 1) as i64 ^ -((raw & 1) as i64))
        }
        Kind::Fixed32 => Value::from(u32::from_le_bytes(reader.fixed()?)),
        Kind::Sfixed32 => Value::from(i32::from_le_bytes(reader.fixed()?)),
        Kind::Float => js_number(f64::from(f32::from_le_bytes(reader.fixed()?))),
        Kind::Fixed64 => Value::from(u64::from_le_bytes(reader.fixed()?)),
        Kind::Sfixed64 => Value::from(i64::from_le_bytes(reader.fixed()?)),
        Kind::Double => js_number(f64::from_le_bytes(reader.fixed()?)),
    })
}

fn enum_value(descriptor: &EnumDescriptor, number: i32, enums: Enums) -> Value {
    match (enums, descriptor.get_value(number)) {
        (Enums::Names, Some(value)) => Value::String(value.name().to_string()),
        _ => Value::from(number),
    }
}

/// A missing map value: what protobufjs starts an entry with.
fn default_value(kind: &Kind, enums: Enums) -> Value {
    match kind {
        Kind::Message(_) => Value::Object(Map::new()),
        Kind::String => Value::String(String::new()),
        Kind::Bytes => bytes_value(&[]),
        Kind::Bool => Value::Bool(false),
        Kind::Enum(descriptor) => enum_value(descriptor, 0, enums),
        _ => Value::from(0),
    }
}

/// A map entry as a JavaScript object's key and value.
fn decode_map_entry(kind: &Kind, bytes: &[u8], enums: Enums, depth: u32) -> Result<(String, Value), ProtoError> {
    let Kind::Message(entry) = kind else { return Err(ProtoError::new("a map field without its entry type")) };
    let key_kind = entry.map_entry_key_field().kind();
    let value_kind = entry.map_entry_value_field().kind();
    let mut key = default_value(&key_kind, enums);
    let mut value = None;
    let mut reader = Reader::new(bytes);
    while !reader.done() {
        let tag = reader.varint()?;
        let wire = (tag & 7) as u8;
        match tag >> 3 {
            1 if wire == wire_of(&key_kind) => key = read_value(&mut reader, &key_kind, enums, depth)?,
            2 if wire == wire_of(&value_kind) => value = Some(read_value(&mut reader, &value_kind, enums, depth)?),
            _ => reader.skip(wire, depth)?,
        }
    }
    let key = match key {
        Value::String(text) => text,
        other => other.to_string(),
    };
    Ok((key, value.unwrap_or_else(|| default_value(&value_kind, enums))))
}

// ---------------------------------------------------------------- fromObject + encode

/// `Type.encode(Type.fromObject(object))`: protobufjs's conversions of the object's values (a string where a
/// number goes, a base64 string for bytes, an enum by name or number) and its errors (an object or an array
/// expected).
pub fn encode(message: &MessageDescriptor, object: &Value) -> Result<Vec<u8>, ProtoError> {
    let mut out = Vec::new();
    encode_message(message, object, &mut out, 0)?;
    Ok(out)
}

fn put_key(out: &mut Vec<u8>, number: u32, wire: u8) {
    put_varint(out, (u64::from(number) << 3) | u64::from(wire));
}

fn put_length_delimited(out: &mut Vec<u8>, number: u32, bytes: &[u8]) {
    put_key(out, number, 2);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// `util.isObject`: arrays count, null does not.
fn is_js_object(value: &Value) -> bool {
    matches!(value, Value::Object(_) | Value::Array(_))
}

fn encode_message(
    message: &MessageDescriptor,
    object: &Value,
    out: &mut Vec<u8>,
    depth: u32,
) -> Result<(), ProtoError> {
    if depth > RECURSION_LIMIT {
        return Err(ProtoError::new("maximum nesting depth exceeded"));
    }
    let empty = Map::new();
    let properties = match object {
        Value::Object(properties) => properties,
        Value::Array(_) => &empty,
        _ => return Err(ProtoError::new(format!("{}: object expected", message.full_name()))),
    };
    // "When a message is serialized its known fields should be written sequentially by field number."
    let mut fields: Vec<FieldDescriptor> = message.fields().collect();
    fields.sort_by_key(FieldDescriptor::number);
    for field in fields {
        let Some(value) = properties.get(&field_name(&field)) else { continue };
        if field.is_map() {
            encode_map(&field, value, out, depth)?;
        } else if field.is_list() {
            encode_list(&field, value, out, depth)?;
        } else {
            encode_singular(&field, value, out, depth)?;
        }
    }
    Ok(())
}

fn encode_nested(
    field: &FieldDescriptor,
    message: &MessageDescriptor,
    value: &Value,
    depth: u32,
) -> Result<Vec<u8>, ProtoError> {
    if !is_js_object(value) {
        return Err(ProtoError::new(format!("{}: object expected", field.full_name())));
    }
    let mut inner = Vec::new();
    encode_message(message, value, &mut inner, depth + 1)?;
    Ok(inner)
}

fn encode_singular(field: &FieldDescriptor, value: &Value, out: &mut Vec<u8>, depth: u32) -> Result<(), ProtoError> {
    let kind = field.kind();
    // fromObject skips null and undefined (an enum's switch does the same).
    if value.is_null() {
        return Ok(());
    }
    match &kind {
        Kind::Message(message) => {
            put_length_delimited(out, field.number(), &encode_nested(field, message, value, depth)?)
        }
        _ => {
            if let Some(bytes) = scalar(field, &kind, value, false)? {
                put_key(out, field.number(), wire_of(&kind));
                out.extend(bytes);
            }
        }
    }
    Ok(())
}

fn encode_list(field: &FieldDescriptor, value: &Value, out: &mut Vec<u8>, depth: u32) -> Result<(), ProtoError> {
    if !js_truthy(value) {
        return Ok(());
    }
    let Value::Array(items) = value else {
        return Err(ProtoError::new(format!("{}: array expected", field.full_name())));
    };
    let kind = field.kind();
    if let Kind::Message(message) = &kind {
        for item in items {
            put_length_delimited(out, field.number(), &encode_nested(field, message, item, depth)?);
        }
        return Ok(());
    }
    let mut values = Vec::with_capacity(items.len());
    for item in items {
        let bytes = scalar(field, &kind, item, true)?
            .ok_or_else(|| ProtoError::new(format!("{}: invalid value in the list", field.full_name())))?;
        values.push(bytes);
    }
    if values.is_empty() {
        return Ok(());
    }
    if field.is_packed() && packable(&kind) {
        put_length_delimited(out, field.number(), &values.concat());
    } else {
        for bytes in values {
            put_key(out, field.number(), wire_of(&kind));
            out.extend(bytes);
        }
    }
    Ok(())
}

fn encode_map(field: &FieldDescriptor, value: &Value, out: &mut Vec<u8>, depth: u32) -> Result<(), ProtoError> {
    if !js_truthy(value) {
        return Ok(());
    }
    let Value::Object(entries) = value else {
        return Err(ProtoError::new(format!("{}: object expected", field.full_name())));
    };
    let Kind::Message(entry_type) = field.kind() else { return Ok(()) };
    let key_field = entry_type.map_entry_key_field();
    let value_field = entry_type.map_entry_value_field();
    for (key, entry_value) in entries {
        let mut entry = Vec::new();
        let key_kind = key_field.kind();
        let key_bytes = scalar(&key_field, &key_kind, &Value::String(key.clone()), false)?.unwrap_or_default();
        put_key(&mut entry, 1, wire_of(&key_kind));
        entry.extend(key_bytes);
        let value_kind = value_field.kind();
        match &value_kind {
            // A map's message value is not checked for null: protobufjs throws on it.
            Kind::Message(message) => {
                put_length_delimited(&mut entry, 2, &encode_nested(field, message, entry_value, depth)?)
            }
            _ => {
                // An enum value fromObject does not know is written as 0, as protobufjs writes an unset one.
                let bytes = match scalar(&value_field, &value_kind, entry_value, false)? {
                    Some(bytes) => bytes,
                    None => encode_varint_value(0),
                };
                put_key(&mut entry, 2, wire_of(&value_kind));
                entry.extend(bytes);
            }
        }
        put_length_delimited(out, field.number(), &entry);
    }
    Ok(())
}

fn encode_varint_value(value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, value);
    out
}

/// A scalar or enum value as fromObject converts it, encoded without its key; None when fromObject leaves the
/// field unset (bytes given a number, an enum given a name it does not have).
fn scalar(field: &FieldDescriptor, kind: &Kind, value: &Value, repeated: bool) -> Result<Option<Vec<u8>>, ProtoError> {
    let mut out = Vec::new();
    match kind {
        Kind::Enum(descriptor) => {
            let number = match value {
                Value::String(name) => descriptor.get_value_by_name(name).map(|value| f64::from(value.number())),
                Value::Number(number) => number.as_f64(),
                _ => None,
            };
            // In a list, a value fromObject does not know becomes the enum's first value.
            let number = match (number, repeated) {
                (Some(number), _) => number,
                (None, true) => f64::from(descriptor.default_value().number()),
                (None, false) => return Ok(None),
            };
            put_int32(&mut out, number);
        }
        Kind::Double => out.extend(js_to_number(value).to_le_bytes()),
        Kind::Float => out.extend((js_to_number(value) as f32).to_le_bytes()),
        Kind::Int32 => put_int32(&mut out, f64::from(to_int32(js_to_number(value)))),
        Kind::Uint32 => put_varint(&mut out, u64::from(to_uint32(js_to_number(value)))),
        Kind::Sint32 => {
            let n = to_int32(js_to_number(value));
            put_varint(&mut out, u64::from(((n << 1) ^ (n >> 31)) as u32));
        }
        Kind::Fixed32 => out.extend(to_uint32(js_to_number(value)).to_le_bytes()),
        Kind::Sfixed32 => out.extend(to_int32(js_to_number(value)).to_le_bytes()),
        Kind::Int64 => put_varint(&mut out, js_to_long(field, value, false)?),
        Kind::Uint64 => put_varint(&mut out, js_to_long(field, value, true)?),
        Kind::Sint64 => {
            let n = js_to_long(field, value, false)? as i64;
            put_varint(&mut out, ((n << 1) ^ (n >> 63)) as u64);
        }
        Kind::Fixed64 => out.extend(js_to_long(field, value, true)?.to_le_bytes()),
        Kind::Sfixed64 => out.extend(js_to_long(field, value, false)?.to_le_bytes()),
        Kind::Bool => out.push(u8::from(js_truthy(value))),
        Kind::String => {
            let text = js_to_string(value);
            put_varint(&mut out, text.len() as u64);
            out.extend_from_slice(text.as_bytes());
        }
        Kind::Bytes => {
            let Some(bytes) = js_bytes(field, value)? else { return Ok(None) };
            put_varint(&mut out, bytes.len() as u64);
            out.extend(bytes);
        }
        Kind::Message(_) => return Err(ProtoError::new(format!("{}: a message is not a scalar", field.full_name()))),
    }
    Ok(Some(out))
}

/// protobufjs's int32 writer: a negative value as ten bytes, sign-extended.
fn put_int32(out: &mut Vec<u8>, value: f64) {
    if value < 0.0 {
        put_varint(out, value.trunc() as i64 as u64);
    } else {
        put_varint(out, u64::from(to_uint32(value)));
    }
}

/// A bytes field's value: a base64 string, an array of byte values, or a bytes value; None for anything else.
fn js_bytes(field: &FieldDescriptor, value: &Value) -> Result<Option<Vec<u8>>, ProtoError> {
    match value {
        Value::String(text) => LENIENT_BASE64
            .decode(text)
            .map(Some)
            .map_err(|_| ProtoError::new(format!("{}: invalid encoding", field.full_name()))),
        Value::Array(items) => Ok(Some(items.iter().map(|item| to_uint32(js_to_number(item)) as u8).collect())),
        Value::Object(_) => Ok(value_bytes(value)),
        _ => Ok(None),
    }
}

// ---------------------------------------------------------------- JavaScript's conversions

/// `Boolean(value)`.
fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `Number(value)`.
fn js_to_number(value: &Value) -> f64 {
    match value {
        Value::Null => 0.0,
        Value::Bool(flag) => f64::from(u8::from(*flag)),
        Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
        Value::String(text) => js_string_to_number(text),
        Value::Array(items) => match items.as_slice() {
            [] => 0.0,
            [single] => js_string_to_number(&js_to_string(single)),
            _ => f64::NAN,
        },
        Value::Object(_) => f64::NAN,
    }
}

/// JavaScript's `\s`.
pub(crate) fn is_js_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{B}' | '\u{C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}')
        || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}')
}

/// `Number(text)`: decimal, 0x/0o/0b integers, Infinity; NaN for anything else.
fn js_string_to_number(text: &str) -> f64 {
    let text = text.trim_matches(is_js_space);
    if text.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
        if let Some(digits) = text.strip_prefix(prefix) {
            return u128::from_str_radix(digits, radix).map_or(f64::NAN, |n| n as f64);
        }
    }
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if unsigned == "Infinity" {
        return if text.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    // Rust also reads «inf» and «nan», JavaScript does not.
    if !unsigned.starts_with(|c: char| c.is_ascii_digit() || c == '.')
        || !unsigned.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
    {
        return f64::NAN;
    }
    text.parse().unwrap_or(f64::NAN)
}

/// ECMAScript's ToUint32.
fn to_uint32(value: f64) -> u32 {
    if !value.is_finite() {
        return 0;
    }
    value.trunc().rem_euclid(4_294_967_296.0) as u32
}

/// ECMAScript's ToInt32.
fn to_int32(value: f64) -> i32 {
    to_uint32(value) as i32
}

/// `String(value)`.
pub(crate) fn js_to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => js_number_string(number),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| if item.is_null() { String::new() } else { js_to_string(item) })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

/// A number as JavaScript writes it: integral values without a fraction.
pub(crate) fn js_number_string(number: &Number) -> String {
    match number.as_f64() {
        Some(value) if !number.is_f64() || value.fract() != 0.0 || value.abs() >= 1e21 => number.to_string(),
        Some(value) => format!("{}", value as i128),
        None => number.to_string(),
    }
}

/// long.js's `Long.fromValue`, as two's complement bits: numbers truncated and clamped, decimal strings,
/// `{low, high}`; anything else is zero.
fn js_to_long(field: &FieldDescriptor, value: &Value, unsigned: bool) -> Result<u64, ProtoError> {
    Ok(match value {
        Value::Number(number) => {
            if let Some(n) = number.as_u64() {
                if unsigned {
                    n
                } else {
                    n.min(i64::MAX as u64)
                }
            } else if let Some(n) = number.as_i64() {
                if unsigned {
                    0
                } else {
                    n as u64
                }
            } else {
                long_from_number(number.as_f64().unwrap_or(f64::NAN), unsigned)
            }
        }
        Value::String(text) => {
            let parsed: i128 = match text.as_str() {
                "NaN" | "Infinity" | "+Infinity" | "-Infinity" => 0,
                _ => text
                    .parse()
                    .map_err(|_| ProtoError::new(format!("{}: invalid long «{text}»", field.full_name())))?,
            };
            parsed as u64
        }
        Value::Object(bits) => {
            let part = |key: &str| bits.get(key).map_or(0, |part| to_uint32(js_to_number(part)));
            (u64::from(part("high")) << 32) | u64::from(part("low"))
        }
        _ => 0,
    })
}

fn long_from_number(value: f64, unsigned: bool) -> u64 {
    const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;
    if value.is_nan() {
        return 0;
    }
    if unsigned {
        if value < 0.0 {
            0
        } else if value >= 2.0 * TWO_POW_63 {
            u64::MAX
        } else {
            value.trunc() as u64
        }
    } else if value <= -TWO_POW_63 {
        i64::MIN as u64
    } else if value + 1.0 >= TWO_POW_63 {
        i64::MAX as u64
    } else {
        value.trunc() as i64 as u64
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn encode_hex(type_name: &str, object: Value) -> String {
        hex::encode(encode(&message_type(type_name).unwrap(), &object).unwrap())
    }

    fn decode_hex(type_name: &str, bytes: &str, enums: Enums) -> Value {
        decode(&message_type(type_name).unwrap(), &hex::decode(bytes).unwrap(), enums).unwrap()
    }

    #[test]
    fn converts_values_as_protobufjs_from_object_did() {
        // The bytes protobufjs wrote for the same objects. Numbers and bools where strings go, a decimal string
        // and a fraction where 64-bit integers go, base64 for bytes.
        let header = json!({ "event": {
            "header": { "messageId": 5, "refMessageId": true, "streamId": "7", "ack": 1.9 },
            "syncPushResponse": "CgA=",
        } });
        assert_eq!(encode_hex("ClientMessage", header), "0a130a0d0a01351204747275652007280142020a00");
        // A negative int64 in ten bytes; an enum by a number it knows or not; by a name it does not know: unset.
        let control = |stream_id: i64, reason: Value| json!({ "streamControl": { "streamId": stream_id, "close": { "reason": reason } } });
        assert_eq!(encode_hex("ClientMessage", control(-1, json!(3))), "120f08ffffffffffffffffff011a020803");
        assert_eq!(encode_hex("ClientMessage", control(2, json!(42))), "120608021a02082a");
        assert_eq!(encode_hex("ClientMessage", control(2, json!("NOT_A_REASON"))), "120408021a00");
        // Strings and bools where doubles go, anything where a bool goes, a number where a string goes.
        let tts = json!({ "event": { "ttsGenerate": { "text": "a", "speed": "1.5", "volume": true, "isStreaming": "yes", "tag": 12.5 } } });
        assert_eq!(
            encode_hex("ClientMessage", tts),
            "0a1f6a1d29000000000000f83f320431322e353a016149000000000000f03f6001"
        );
        // A negative uint32 wraps, and byte values beyond 255.
        let text = json!({ "event": { "textInput": {
            "header": { "sequenceNumber": -2 },
            "request": { "event": { "type": "text_input", "payloadRaw": [1, 2, 300] } },
        } } });
        assert_eq!(encode_hex("ClientMessage", text), "0a1612140a0618feffffff0f1a0a0a080802c2010301022c");
        // A negative int32 in ten bytes; enums in a list by name and by number.
        let alarm = json!({ "name": "alarm_add_directive", "alarm": {
            "alarmId": "x",
            "localTime": { "hours": 7, "minutes": 0 },
            "activationDays": [{ "weekday": "MONDAY" }, { "weekday": 3 }, { "date": { "year": 2026, "month": 11, "day": 2 } }],
            "timezone": { "timezoneName": "Asia/Bangkok", "offsetSeconds": -3600 },
            "editTimestamp": 1793578022,
        } });
        assert_eq!(
            encode_hex("AlarmAddPayload", alarm),
            "0a13616c61726d5f6164645f646972656374697665123d0a01781204080710002a0208012a0208032a09120708ea0f100b18023a19\
             0a0c417369612f42616e676b6f6b10f0e3ffffffffffffff0158a6a89fd706"
        );
    }

    #[test]
    fn refuses_what_protobufjs_refused() {
        let client = message_type("ClientMessage").unwrap();
        assert_eq!(
            encode(&client, &json!("text")),
            Err(ProtoError::new("duowave.station.ClientMessage: object expected"))
        );
        assert!(encode(&client, &json!({ "event": { "header": "h" } })).is_err());
        assert!(
            encode(&client, &json!({ "event": { "synchronizeStateRequest": { "supportedFeatures": "a" } } })).is_err()
        );
        assert!(encode(&client, &json!({ "event": { "syncPushResponse": "not base64!" } })).is_err());
        assert!(message_type("NoSuchMessage").is_err());
    }

    #[test]
    fn decodes_as_protobufjs_to_object_did() {
        // Floats as JavaScript reads them, 64-bit integers as numbers (what protobufjs decoded).
        assert_eq!(
            decode_hex("ServerMessage", "0a180a030a01706a11220f0d6666663f12080a016115cdcccc3d", Enums::Names),
            json!({ "event": { "header": { "messageId": "p" }, "asrResult": { "recognition": [
                { "confidence": 0.8999999761581421, "words": [{ "value": "a", "confidence": 0.10000000149011612 }] },
            ] } } })
        );
        assert_eq!(
            decode_hex("ServerMessage", "0a160a090a01712080e497d012520908c0ef8ae7fee79703", Enums::Names),
            json!({ "event": {
                "header": { "messageId": "q", "streamId": 5_000_000_000u64 },
                "inputStartAck": { "requestStartTime": 1_793_578_022_123_456u64 },
            } })
        );
        // Default values that came are kept; an enum by name or by number, one it does not know by number; an
        // unknown field (99) is skipped.
        let close = "120908009806011a020800";
        assert_eq!(
            decode_hex("ClientMessage", close, Enums::Names),
            json!({ "streamControl": { "streamId": 0, "close": { "reason": "CLOSE_OK" } } })
        );
        assert_eq!(
            decode_hex("ClientMessage", close, Enums::Numbers),
            json!({ "streamControl": { "streamId": 0, "close": { "reason": 0 } } })
        );
        assert_eq!(
            decode_hex("ClientMessage", "120608021a02082a", Enums::Names)["streamControl"]["close"]["reason"],
            42
        );
        // A known field with another wire type is skipped as unknown: here stream_id (a varint) as bytes.
        assert_eq!(
            decode_hex("ClientMessage", "12050a01611a00", Enums::Names),
            json!({ "streamControl": { "close": {} } })
        );
        // A singular message that comes twice is the last one: protobufjs replaced it, it did not merge.
        assert_eq!(
            decode_hex("ClientMessage", "0a050a030a01610a024a00", Enums::Names),
            json!({ "event": { "ack": {} } })
        );
        // A Struct as the message it is, an empty one without fields.
        assert_eq!(
            decode_hex("google.protobuf.Struct", "0a090a0161120420012a00", Enums::Names),
            json!({ "fields": { "a": { "boolValue": true, "structValue": {} } } })
        );
    }

    #[test]
    fn keeps_bytes_as_bytes_values() {
        let value = bytes_value(&[0x0a, 0x00]);
        assert_eq!(value, json!({ "$bytes": "CgA=" }));
        assert_eq!(value_bytes(&value), Some(vec![0x0a, 0x00]));
        assert_eq!(value_bytes(&json!("CgA=")), None);
        assert_eq!(value_bytes(&json!({ "$bytes": "CgA=", "other": 1 })), None);
    }
}
