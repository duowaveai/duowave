//! JavaScript's conventions where the skills' answers depend on them: numbers without an integer type (an intent's
//! `{level: 7}` is 7, not 7.0), Math.round, String(x), Number(x), truthiness, trim, encodeURIComponent, Node's
//! lenient base64 and the range of a Date. The skills were written against them; the station, the model and the
//! panel read the results.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

/// How far from the epoch a Date reaches, in ms (TimeClip): past it a Date is invalid, and Intl throws «Invalid time
/// value» for it.
const DATE_RANGE_MS: f64 = 8.64e15;

/// `new Date(ms)`: None where that Date is invalid. chrono's range ends a little earlier (with the year 262 142, a
/// Date's in 275 760): a moment between the two is None too.
pub fn date(ms: f64) -> Option<DateTime<Utc>> {
    if ms.is_nan() || ms.abs() > DATE_RANGE_MS {
        return None;
    }
    DateTime::from_timestamp_millis(ms.trunc() as i64)
}

/// Math.round: halves go up, also below zero (-2.5 → -2).
pub fn round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let floor = x.floor();
    if x - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// A number as String(x) writes it: 7, 55.75, -0 as 0, NaN, Infinity, 1e+21.
pub fn number_text(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if x == 0.0 {
        return "0".into();
    }
    if x.abs() >= 1e21 || x.abs() < 1e-6 {
        let text = format!("{x:e}");
        return match text.split_once('e') {
            Some((mantissa, exponent)) if !exponent.starts_with('-') => format!("{mantissa}e+{exponent}"),
            _ => text,
        };
    }
    format!("{x}")
}

/// A number as JSON: an integer when it is whole (as JSON.stringify writes it), null when it is not finite.
pub fn number_value(x: f64) -> Value {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 9_007_199_254_740_992.0 {
        return Value::from(x as i64);
    }
    serde_json::Number::from_f64(x).map(Value::Number).unwrap_or(Value::Null)
}

/// String.prototype.trim: Unicode white space and the byte order mark.
pub fn trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// Number(x); `None` is undefined (NaN).
pub fn to_number(value: Option<&Value>) -> f64 {
    match value {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(flag)) => f64::from(u8::from(*flag)),
        Some(Value::Number(number)) => number.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(text)) => string_to_number(text),
        Some(array @ Value::Array(_)) => string_to_number(&string_of(array)),
        Some(Value::Object(_)) => f64::NAN,
    }
}

/// Number(string): decimal (with an exponent), Infinity, 0x/0o/0b; empty is 0, anything else NaN.
pub fn string_to_number(text: &str) -> f64 {
    let text = trim(text);
    if text.is_empty() {
        return 0.0;
    }
    let (sign, unsigned) = match text.as_bytes()[0] {
        b'-' => (-1.0, &text[1..]),
        b'+' => (1.0, &text[1..]),
        _ => (1.0, text),
    };
    if unsigned == "Infinity" {
        return sign * f64::INFINITY;
    }
    for (prefix, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
        if let Some(digits) = text.strip_prefix(prefix) {
            return u64::from_str_radix(digits, radix).map(|n| n as f64).unwrap_or(f64::NAN);
        }
    }
    if decimal_literal(unsigned) {
        return text.parse::<f64>().unwrap_or(f64::NAN);
    }
    f64::NAN
}

/// `12`, `1.5`, `.5`, `5.`, `1e3`: what Number reads as a decimal (Rust's parser also takes «inf» and «nan»).
fn decimal_literal(text: &str) -> bool {
    let (mantissa, exponent) = match text.find(['e', 'E']) {
        Some(at) => (&text[..at], Some(&text[at + 1..])),
        None => (text, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty() && fraction.is_empty() || !digits(whole) || !digits(fraction) {
        return false;
    }
    match exponent {
        None => true,
        Some(exponent) => {
            let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
            !exponent.is_empty() && digits(exponent)
        }
    }
}

/// String(x) of a JSON value.
pub fn string_of(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number_text(number.as_f64().unwrap_or(f64::NAN)),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| if item.is_null() { String::new() } else { string_of(item) })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// `String(x ?? '')`: absent and null are empty.
pub fn string_or_empty(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(value) => string_of(value),
    }
}

/// Whether `if (x)` takes it.
pub fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// `typeof params[key] === 'number'`.
pub fn number_param(params: &Map<String, Value>, key: &str) -> Option<f64> {
    params.get(key).and_then(Value::as_f64)
}

/// `typeof params[key] === 'string'`.
pub fn string_param<'a>(params: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

/// `params[key] === true`.
pub fn is_true(params: &Map<String, Value>, key: &str) -> bool {
    params.get(key) == Some(&Value::Bool(true))
}

/// encodeURIComponent: everything but `A–Z a–z 0–9 - _ . ! ~ * ' ( )` as UTF-8 percent escapes.
pub fn encode_uri_component(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Buffer.from(text, 'base64'): both alphabets, anything else skipped, the end at the first `=`, a partial group
/// decoded as far as it goes.
pub fn base64_decode(text: &str) -> Vec<u8> {
    let sextets: Vec<u8> = text
        .chars()
        .take_while(|c| *c != '=')
        .filter_map(|c| match c {
            'A'..='Z' => Some(c as u8 - b'A'),
            'a'..='z' => Some(c as u8 - b'a' + 26),
            '0'..='9' => Some(c as u8 - b'0' + 52),
            '+' | '-' => Some(62),
            '/' | '_' => Some(63),
            _ => None,
        })
        .collect();
    let mut bytes = Vec::with_capacity(sextets.len() * 3 / 4);
    for group in sextets.chunks(4) {
        let value = group.iter().enumerate().fold(0u32, |acc, (i, s)| acc | (u32::from(*s) << (18 - 6 * i)));
        let count = match group.len() {
            4 => 3,
            3 => 2,
            2 => 1,
            _ => 0,
        };
        for i in 0..count {
            bytes.push((value >> (16 - 8 * i)) as u8);
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rounds_and_writes_numbers_as_javascript_does() {
        assert_eq!(round(2.5), 3.0);
        assert_eq!(round(-2.5), -2.0);
        assert_eq!(round(0.49999999999999994), 0.0);
        assert_eq!(number_text(-0.0), "0");
        assert_eq!(number_text(55.0), "55");
        assert_eq!(number_text(55.75), "55.75");
        assert_eq!(number_text(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(number_text(1e21), "1e+21");
        assert_eq!(number_text(f64::NAN), "NaN");
        assert_eq!(number_value(7.0), json!(7));
        assert_eq!(number_value(3.5), json!(3.5));
    }

    #[test]
    fn reads_values_as_number_and_string_do() {
        assert!(to_number(None).is_nan());
        assert_eq!(to_number(Some(&json!(null))), 0.0);
        assert_eq!(to_number(Some(&json!(" 5 "))), 5.0);
        assert_eq!(to_number(Some(&json!(""))), 0.0);
        assert!(to_number(Some(&json!("5x"))).is_nan());
        assert!(to_number(Some(&json!("inf"))).is_nan());
        assert_eq!(to_number(Some(&json!("0x1f"))), 31.0);
        assert_eq!(to_number(Some(&json!(".5"))), 0.5);
        assert_eq!(to_number(Some(&json!("5."))), 5.0);
        assert_eq!(to_number(Some(&json!([5]))), 5.0);
        assert_eq!(string_of(&json!([1, null, "a"])), "1,,a");
        assert!(!truthy(Some(&json!(0))));
        assert!(truthy(Some(&json!("0"))));
    }

    #[test]
    fn holds_a_moment_where_a_date_does() {
        assert_eq!(date(1_793_527_200_000.0), "2026-11-01T10:00:00Z".parse().ok());
        // TimeClip drops the fraction towards zero.
        assert_eq!(date(-1.9).map(|at| at.timestamp_millis()), Some(-1));
        assert_eq!(date(-8.2e15).map(|at| at.timestamp_millis()), Some(-8_200_000_000_000_000));
        assert!(date(f64::NAN).is_none());
        assert!(date(8.64e15 + 1.0).is_none());
        // A Date still, chrono no longer.
        assert!(date(8.5e15).is_none());
    }

    #[test]
    fn encodes_and_decodes_as_the_browser_and_node_do() {
        assert_eq!(encode_uri_component("Альбом"), "%D0%90%D0%BB%D1%8C%D0%B1%D0%BE%D0%BC");
        assert_eq!(encode_uri_component("Кино - Группа крови.mp3").matches('%').count(), 33);
        assert_eq!(encode_uri_component("a b(c)!"), "a%20b(c)!");
        assert_eq!(base64_decode("H4s I\n"), vec![0x1f, 0x8b, 0x08]);
        assert_eq!(base64_decode("a$bcd"), vec![0x69, 0xb7, 0x1d]);
        assert_eq!(base64_decode("ab=cd"), vec![0x69]);
        assert_eq!(base64_decode("abc"), vec![0x69, 0xb7]);
        assert_eq!(base64_decode("-_-_"), vec![0xfb, 0xff, 0xbf]);
    }
}
