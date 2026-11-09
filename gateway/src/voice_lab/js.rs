//! What the Node gateway's numbers, strings and paths did, where the panel, a message or a test sees the difference:
//! Math.round rounds halves up, toFixed(1) with a decimal comma, numbers in JSON without a needless `.0`, lengths in
//! UTF-16 units, trim that takes a BOM too, path.resolve.

use std::path::{Component, Path, PathBuf};

use serde::Serializer;

/// Math.round: halves go up (towards +∞), -2.5 → -2.
pub fn round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// Math.round to an integer.
pub fn round_i64(value: f64) -> i64 {
    round(value) as i64
}

/// Megabytes as the panel reads gigabytes: one decimal, a comma — `(mb / 1024).toFixed(1).replace('.', ',')`.
pub fn gb(mb: f64) -> String {
    // The values are whole megabytes or bytes over 2^20: times ten they stay exact, so a half rounds up as toFixed
    // rounds it.
    let tenths = round(mb / 1024.0 * 10.0);
    format!("{:.1}", tenths / 10.0).replace('.', ",")
}

/// A number as JSON.stringify writes it: a whole one without `.0`.
pub fn number<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 9.0e15 {
        serializer.serialize_i64(*value as i64)
    } else {
        serializer.serialize_f64(*value)
    }
}

/// A number for a JSON value, whole ones without `.0` as [`number`] writes them.
pub fn value(number: f64) -> serde_json::Value {
    if number.is_finite() && number.fract() == 0.0 && number.abs() < 9.0e15 {
        serde_json::Value::from(number as i64)
    } else {
        serde_json::Value::from(number)
    }
}

/// [`number`] for an optional field (skipped when absent).
pub fn opt_number<S: Serializer>(value: &Option<f64>, serializer: S) -> Result<S::Ok, S::Error> {
    match value {
        Some(value) => number(value, serializer),
        None => serializer.serialize_none(),
    }
}

/// String.prototype.trim: white space, line ends and the BOM.
pub fn trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// The length JavaScript counts: UTF-16 units.
pub fn length(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `text.slice(0, units)` without cutting a character in two.
pub fn slice(text: &str, units: usize) -> &str {
    let mut count = 0;
    for (index, c) in text.char_indices() {
        count += c.len_utf16();
        if count > units {
            return &text[..index];
        }
    }
    text
}

/// Number(text) for a path or query parameter, when it is a whole number: `"01"` is 1, `""` is 0.
pub fn integer(text: &str) -> Option<i64> {
    let text = trim(text);
    if text.is_empty() {
        return Some(0);
    }
    if !text.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'+' | b'e' | b'E')) {
        return None;
    }
    let value: f64 = text.parse().ok()?;
    (value.is_finite() && value.fract() == 0.0 && value.abs() < 9.0e15).then_some(value as i64)
}

/// path.resolve: absolute, with `.` and `..` worked out (symbolic links are not followed).
pub fn resolve(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_as_javascript_did() {
        assert_eq!(round(2.5), 3.0);
        assert_eq!(round(-2.5), -2.0);
        assert_eq!(round(-0.5), 0.0);
        assert_eq!(round(3622.5), 3623.0);
        assert_eq!(gb(3_500.0), "3,4");
        assert_eq!(gb(256.0), "0,3", "toFixed rounds an exact half up");
        assert_eq!(gb(5_289.0), "5,2");
        assert_eq!(gb(4_000.0), "3,9");
    }

    #[test]
    fn counts_and_cuts_strings_in_utf16_units() {
        assert_eq!(length("голос"), 5);
        assert_eq!(length("🙂"), 2);
        assert_eq!(slice("голос", 3), "гол");
        assert_eq!(slice("a🙂b", 2), "a", "a pair is never split");
        assert_eq!(trim("\u{feff} x \n"), "x");
    }

    #[test]
    fn reads_numbers_from_paths_as_number_did() {
        assert_eq!(integer("01"), Some(1));
        assert_eq!(integer("7"), Some(7));
        assert_eq!(integer("1.5"), None);
        assert_eq!(integer("abc"), None);
        assert_eq!(integer(""), Some(0));
        assert_eq!(integer("NaN"), None);
    }

    #[test]
    fn resolves_paths_lexically() {
        let resolved = resolve(Path::new("/a/b/../c/./d"));
        assert_eq!(resolved, PathBuf::from("/a/c/d"));
        assert!(resolve(Path::new("x")).is_absolute());
    }
}
