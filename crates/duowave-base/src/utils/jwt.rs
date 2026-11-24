//! A JWT signed with a shared secret (HS256), made here — no library needed: LiveKit's tokens (src/calls/livekit: the
//! family room's; src/connectors/checks.rs: the panel's «Проверить» of the connection).

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine as _;
use hmac::{Hmac, KeyInit, Mac};
use serde::Serialize;
use sha2::Sha256;

/// The token of `claims`, signed with `secret`: the header, the claims and the signature, each base64url.
pub fn hs256(claims: &impl Serialize, secret: &str) -> String {
    let header = B64URL.encode(br#"{"alg":"HS256"}"#);
    let payload = B64URL.encode(serde_json::to_vec(claims).unwrap_or_default());
    let signed = format!("{header}.{payload}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes a key of any size");
    mac.update(signed.as_bytes());
    let signature = B64URL.encode(mac.finalize().into_bytes());
    format!("{signed}.{signature}")
}
