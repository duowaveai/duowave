//! What the journal may keep of a protocol object: secrets never, identifiers hashed when RECORDER_REDACT is on,
//! bytes as a raw protobuf view.

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::gateway::protocol::proto_object::{js_to_string, value_bytes};
use crate::gateway::protocol::raw_proto::describe_raw;

/// Keys are compared normalized, so `x-uprx-oauth-token` (a handshake header), `oauthToken` and `OAuthToken` are
/// one key. Secrets are always replaced with a short hash, whatever RECORDER_REDACT says.
fn is_secret(normalized: &str) -> bool {
    normalized.ends_with("token")
        || normalized.ends_with("cookie")
        || normalized.ends_with("cookies")
        || normalized == "apikey"
        || normalized == "authorization"
        || normalized.contains("secret")
        || normalized.contains("password")
}

/// Values that identify the device or the account: hashed when `redact` is on, so events of one device can still
/// be correlated in the journal without storing the identifier.
const IDENTIFIER_KEYS: [&str; 4] = ["uuid", "deviceid", "metricadeviceid", "uid"];

fn normalize_key(key: &str) -> String {
    let lower = key.to_lowercase();
    let unprefixed = lower.strip_prefix("x-uprx-").unwrap_or(&lower);
    unprefixed.chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).collect()
}

fn is_sensitive(key: &str, redact: bool) -> bool {
    let normalized = normalize_key(key);
    is_secret(&normalized) || (redact && IDENTIFIER_KEYS.contains(&normalized.as_str()))
}

pub fn hash_id(value: &str) -> String {
    let digest = hex::encode(Sha256::digest(value.as_bytes()));
    format!("h:{}", &digest[..10])
}

/// Turns a decoded protocol object into something safe and readable for the journal: bytes become a raw
/// protobuf view, secrets always and identifiers (when `redact` is on) become hashes.
pub fn to_log_value(value: &Value, redact: bool) -> Value {
    log_value(value, redact, "")
}

fn log_value(value: &Value, redact: bool, key: &str) -> Value {
    if !key.is_empty() && matches!(value, Value::String(_) | Value::Number(_)) && is_sensitive(key, redact) {
        return Value::String(hash_id(&js_to_string(value)));
    }
    if let Some(bytes) = value_bytes(value) {
        return json!({ "raw": describe_raw(&bytes) });
    }
    match value {
        Value::Array(items) => Value::Array(items.iter().map(|item| log_value(item, redact, "")).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(child_key, child)| (child_key.clone(), log_value(child, redact, child_key)))
                .collect::<Map<_, _>>(),
        ),
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers() -> Value {
        json!({
            "user-agent": "WebSocket++/0.8.2",
            "x-uprx-oauth-token": "y0_secret",
            "x-uprx-auth-token": "auth-secret",
            "x-uprx-uid": "123456",
            "x-uprx-device-id": "device-1",
            "x-uprx-device-model": "plum",
        })
    }

    #[test]
    fn hashes_handshake_tokens_and_account_ids() {
        assert_eq!(
            to_log_value(&json!({ "headers": headers() }), true),
            json!({
                "headers": {
                    "user-agent": "WebSocket++/0.8.2",
                    "x-uprx-oauth-token": hash_id("y0_secret"),
                    "x-uprx-auth-token": hash_id("auth-secret"),
                    "x-uprx-uid": hash_id("123456"),
                    "x-uprx-device-id": hash_id("device-1"),
                    "x-uprx-device-model": "plum",
                },
            })
        );
    }

    #[test]
    fn never_writes_secrets_even_with_redaction_off() {
        let logged =
            to_log_value(&json!({ "headers": headers(), "oauthToken": "y0_secret", "megamindCookies": "c" }), false);
        assert_eq!(logged["headers"]["x-uprx-oauth-token"], hash_id("y0_secret"));
        assert_eq!(logged["oauthToken"], hash_id("y0_secret"));
        assert_eq!(logged["megamindCookies"], hash_id("c"));
        assert_eq!(logged["headers"]["x-uprx-device-id"], "device-1");
    }

    #[test]
    fn keeps_counters_whose_names_only_mention_tokens() {
        let counters = json!({ "maxTokens": 400, "outputTokens": 12 });
        assert_eq!(to_log_value(&counters, true), counters);
    }

    #[test]
    fn shows_bytes_as_a_raw_view() {
        use crate::gateway::protocol::proto_object::bytes_value;
        // Field 1: the string «id».
        let logged = to_log_value(&json!({ "push": bytes_value(&[0x0a, 0x02, 0x69, 0x64]), "uid": 123456 }), true);
        assert_eq!(
            logged,
            json!({ "push": { "raw": [{ "field": 1, "wire": "bytes", "value": "id" }] }, "uid": hash_id("123456") })
        );
    }
}
