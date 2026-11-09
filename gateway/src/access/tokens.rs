//! The panel's session tokens: stateless, HMAC-signed with the secret only the gateway's sealed store holds. The
//! payload is the person, their token version and the expiry: bumping the version (a new password, «выйти везде», a
//! new role) ends every session at once. `v2.<user>.<version>.<expiry>.<signature>`, the signature base64url.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenClaims {
    pub user_id: String,
    pub version: u64,
    /// Milliseconds since the epoch.
    pub expires_at: u64,
}

fn sign(payload: &str, secret: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes a key of any size");
    mac.update(payload.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

fn digits(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

pub fn issue_token(claims: &TokenClaims, secret: &str) -> String {
    let payload = format!("v2.{}.{}.{}", claims.user_id, claims.version, claims.expires_at);
    let signature = sign(&payload, secret);
    format!("{payload}.{signature}")
}

/// The claims of a token this secret signed and that has not expired; None otherwise.
pub fn read_token(token: &str, secret: &str, now: u64) -> Option<TokenClaims> {
    let parts: Vec<&str> = token.split('.').collect();
    let [version_tag, user_id, version, expiry, signature] = parts[..] else { return None };
    if version_tag != "v2" || user_id.is_empty() {
        return None;
    }
    let (version_number, expires_at) = (digits(version)?, digits(expiry)?);
    if !same(signature, &sign(&format!("v2.{user_id}.{version}.{expiry}"), secret)) {
        return None;
    }
    if expires_at < now {
        return None;
    }
    Some(TokenClaims { user_id: user_id.to_string(), version: version_number, expires_at })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn claims() -> TokenClaims {
        TokenClaims { user_id: "7c1f".into(), version: 3, expires_at: 2_000_000 }
    }

    #[test]
    fn reads_back_what_it_issued_until_it_expires() {
        let token = issue_token(&claims(), SECRET);
        assert!(token.starts_with("v2.7c1f.3.2000000."));
        assert!(!token.contains('='), "base64url without padding");
        assert_eq!(read_token(&token, SECRET, 1_000_000), Some(claims()));
        assert_eq!(read_token(&token, SECRET, 2_000_000), Some(claims()), "valid up to its expiry");
        assert_eq!(read_token(&token, SECRET, 2_000_001), None);
    }

    #[test]
    fn refuses_another_secret_a_changed_payload_and_other_formats() {
        let token = issue_token(&claims(), SECRET);
        assert_eq!(read_token(&token, &SECRET.replace('0', "1"), 0), None);
        assert_eq!(read_token(&token.replace(".3.", ".4."), SECRET, 0), None);
        assert_eq!(read_token(&token.replace("v2.", "v3."), SECRET, 0), None);
        // The first panel's `<expiry>.<id>.<signature>` is not read: the Rust gateway starts its own database.
        let legacy = format!("2000000.7c1f.{}", sign("2000000.7c1f", SECRET));
        assert_eq!(read_token(&legacy, SECRET, 0), None);
        let unsigned = format!("v2.7c1f.x.2000000.{}", sign("v2.7c1f.x.2000000", SECRET));
        assert_eq!(read_token(&unsigned, SECRET, 0), None, "the version is a number");
        assert_eq!(read_token("", SECRET, 0), None);
    }
}
