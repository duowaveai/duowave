//! The pair: the computer's gateway and the station's. The computer reaches the station's gateway with the pair's key
//! (PEER_KEY in the station's .env, written at install; on the computer a secret): its settings package, its updates,
//! a restart. Only from the home network. The station reaches the computer's — its home server — with a key of the
//! computer's own (an API key with the right `home.models`, src/home_server).

use std::time::Duration;

use axum::http::header::AUTHORIZATION;
use serde_json::Value;
use subtle::ConstantTimeEq;

use super::transfer::{self, Bundle};
use crate::platform::http::{ApiError, Caller, Zone};

/// The secret the computer keeps the pair's key under.
pub const PEER_SECRET: &str = "placement:peer";

/// Whether the request carries the pair's key, from the home network.
pub fn from_peer(caller: &Caller, peer_key: Option<&str>) -> bool {
    let Some(expected) = peer_key.filter(|key| key.len() >= 16) else { return false };
    if !matches!(caller.zone, Zone::Local | Zone::Home) {
        return false;
    }
    let given = caller
        .headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    given.len() == expected.len() && bool::from(given.as_bytes().ct_eq(expected.as_bytes()))
}

/// The pair's key, or a person with the right: who may act on the station's gateway.
pub fn peer_or_permitted(caller: &Caller, peer_key: Option<&str>, permission: &str) -> Result<bool, ApiError> {
    if from_peer(caller, peer_key) {
        return Ok(true);
    }
    caller.permitted(permission)?;
    Ok(false)
}

/// A fresh pair key: 32 random bytes, hex.
pub fn new_key() -> String {
    hex::encode(crate::utils::ids::random_bytes(32))
}

/// The computer's side: its requests to the station's gateway.
#[derive(Debug, Clone)]
pub struct PeerClient {
    base: String,
    key: String,
}

impl PeerClient {
    pub fn new(address: &str, port: u16, key: &str) -> PeerClient {
        PeerClient { base: format!("http://{address}:{port}"), key: key.to_string() }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        wait: Duration,
    ) -> Result<Value, String> {
        let mut request = crate::utils::http::client()
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.key)
            .timeout(wait);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|error| describe(&error))?;
        let status = response.status();
        let text = response.text().await.map_err(|error| describe(&error))?;
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !status.is_success() {
            let message = value.get("message").and_then(Value::as_str).unwrap_or(text.as_str());
            return Err(format!("шлюз колонки: {} {}", status.as_u16(), message.chars().take(200).collect::<String>()));
        }
        // The API's envelope: {message, code}.
        Ok(value.get("message").cloned().unwrap_or(value))
    }

    /// The station gateway's /health.
    pub async fn health(&self) -> Result<Value, String> {
        self.call(reqwest::Method::GET, "/health", None, Duration::from_secs(5)).await
    }

    /// The station's settings, opened with the pair's key.
    pub async fn settings(&self) -> Result<Bundle, String> {
        let sealed = self.call(reqwest::Method::GET, "/api/v1/transfer", None, Duration::from_secs(20)).await?;
        transfer::open(&sealed, &self.key)
    }

    /// Gives the station these settings: it applies them at a restart.
    pub async fn send_settings(&self, bundle: &Bundle) -> Result<(), String> {
        let sealed = transfer::seal(bundle, &self.key);
        self.call(reqwest::Method::POST, "/api/v1/transfer", Some(sealed), Duration::from_secs(30)).await.map(|_| ())
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        self.call(reqwest::Method::POST, path, Some(body), Duration::from_secs(30)).await
    }

    /// A signed update for the station's gateway: the archive and its signature (src/updates).
    pub async fn upload_update(&self, version: &str, archive: &[u8], signature: &str) -> Result<Value, String> {
        use base64::Engine as _;
        let body = serde_json::json!({
            "version": version,
            "archive": base64::engine::general_purpose::STANDARD.encode(archive),
            "signature": signature,
        });
        self.call(reqwest::Method::POST, "/api/v1/updates/upload", Some(body), Duration::from_secs(300)).await
    }
}

fn describe(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "шлюз колонки не ответил вовремя".into()
    } else if error.is_connect() {
        "шлюз колонки не отвечает".into()
    } else {
        format!("шлюз колонки: {error}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_key_is_long_and_random() {
        let (one, two) = (new_key(), new_key());
        assert_eq!(one.len(), 64);
        assert_ne!(one, two);
    }
}
