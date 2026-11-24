//! The pair: the computer's gateway and the station's. The computer reaches the station's gateway with the pair's key
//! (PEER_KEY in the station's .env, written at install, or the one the station made and gave its admin; on the computer
//! a secret, each station's own): its settings package, its updates, a restart. Only from the home network. The
//! station reaches the computer's — its home server — with a key of the computer's own (an API key with the right
//! `home.models`, src/home_server).
use std::time::Duration;

use serde_json::Value;

use super::transfer::{self, Bundle};
/// How a request proves it comes from the computer of the pair, or from a person through its pass: the caller's
/// (platform/http/peer.rs), read by every floor's routes.
pub use crate::platform::http::peer::{
    from_peer, peer_or_permitted, through_pass, HELPER_PERSON, HELPER_PERSON_ID, HELPER_ROLE,
};

/// The computer's one pair key from before each station got its own (`station_secret`): the key of the stations moved
/// or linked before then, which they open with until their next move gives them their own. New moves neither set it
/// nor install it; the computer only reads it, for those stations and their copies (watch.rs `pair_key`, `open_copy`).
pub const PEER_SECRET: &str = "placement:peer";
/// The secret a station's own pair key is kept under on the computer: each station gives its own
/// (`POST /api/v1/placement/pair`), and linking a second one leaves the first one's as it is.
pub fn station_secret(station: &str) -> String {
    format!("{PEER_SECRET}:{station}")
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

    /// The pair's key: the helper's pass sends the panel's requests with it (src/placement/proxy.rs).
    pub fn key(&self) -> &str {
        &self.key
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
        answer(response).await
    }

    /// The station gateway's /health.
    pub async fn health(&self) -> Result<Value, String> {
        self.call(reqwest::Method::GET, "/health", None, Duration::from_secs(5)).await
    }

    /// The station gateway's /health for its watch: on a miss, why too (for the journal), and whether the station was
    /// reached at all.
    pub async fn look(&self) -> Result<Value, Missed> {
        let response = crate::utils::http::client()
            .get(format!("{}/health", self.base))
            .bearer_auth(&self.key)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|error| Missed { said: describe(&error), why: why(&error), gone: true })?;
        answer(response).await.map_err(|said| Missed { why: said.clone(), said, gone: false })
    }

    /// The station's settings, opened with the pair's key.
    pub async fn settings(&self) -> Result<Bundle, String> {
        let sealed = self.call(reqwest::Method::GET, "/api/v1/transfer", None, Duration::from_secs(20)).await?;
        transfer::open(&sealed, &self.key)
    }

    /// The station's settings with its plugins' state (`?plugins=1`): a move back takes both. A station of a build
    /// before it gives the settings alone (`Bundle::plugins` None).
    pub async fn settings_and_plugins(&self) -> Result<Bundle, String> {
        let sealed =
            self.call(reqwest::Method::GET, "/api/v1/transfer?plugins=1", None, Duration::from_secs(30)).await?;
        transfer::open(&sealed, &self.key)
    }

    /// Gives the station these settings: it applies them at a restart.
    pub async fn send_settings(&self, bundle: &Bundle) -> Result<(), String> {
        let sealed = transfer::seal(bundle, &self.key);
        self.call(reqwest::Method::POST, "/api/v1/transfer", Some(sealed), Duration::from_secs(30)).await.map(|_| ())
    }

    /// A reading of the station's API with the pair's key: the home's devices for a computer that is its Turbo
    /// (src/home/devices.rs).
    pub async fn get(&self, path: &str) -> Result<Value, String> {
        self.call(reqwest::Method::GET, path, None, Duration::from_secs(5)).await
    }

    /// A longer reading: the home's memory for its copy (src/home/copy.rs).
    pub async fn get_long(&self, path: &str) -> Result<Value, String> {
        self.call(reqwest::Method::GET, path, None, Duration::from_secs(60)).await
    }

    pub async fn put(&self, path: &str, body: Value) -> Result<Value, String> {
        self.call(reqwest::Method::PUT, path, Some(body), Duration::from_secs(60)).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        self.call(reqwest::Method::POST, path, Some(body), Duration::from_secs(30)).await
    }

    /// A signed update for the station's gateway (src/updates): the archive itself as the body, its version and
    /// signature in headers; the station streams the program from it to its disk (PEER_API 2). A station's gateway
    /// before that has no such route: it is told to be updated by hand once.
    pub async fn upload_update(&self, version: &str, archive: &[u8], signature: &str) -> Result<Value, String> {
        use base64::Engine as _;
        // A header holds one line: Tauri's form of a signature is one line of base64, minisign's own text is not.
        let signature = signature.trim();
        let signature = if signature.contains('\n') {
            base64::engine::general_purpose::STANDARD.encode(signature)
        } else {
            signature.to_string()
        };
        let response = crate::utils::http::client()
            .post(format!("{}/api/v1/updates/archive", self.base))
            .bearer_auth(&self.key)
            .timeout(Duration::from_secs(300))
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header(crate::updates::VERSION_HEADER, version)
            .header(crate::updates::SIGNATURE_HEADER, signature)
            .body(archive.to_vec())
            .send()
            .await
            .map_err(|error| describe(&error))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(old_station());
        }
        answer(response).await
    }
}

/// The station's gateway takes its update as the archive itself from this PEER_API on.
pub const ARCHIVE_SINCE_PEER_API: u64 = 2;

/// Said of a station's gateway too old to take an update from the computer that way.
pub fn old_station() -> String {
    crate::tr!("DuoWave на колонке старше 0.5.1: такое обновление с компьютера он не примет. Поставьте новую версию на колонку вручную — дальше она будет обновляться отсюда.")
}

/// A station's answer, unwrapped from the API's envelope ({message, code}); a failure as the station said it.
async fn answer(response: reqwest::Response) -> Result<Value, String> {
    let status = response.status();
    let text = response.text().await.map_err(|error| describe(&error))?;
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        let message = value.get("message").and_then(Value::as_str).unwrap_or(text.as_str());
        return Err(crate::tr!(
            "Колонка: {status} {message}",
            status = status.as_u16(),
            message = message.chars().take(200).collect::<String>()
        ));
    }
    Ok(value.get("message").cloned().unwrap_or(value))
}

/// A look at the station that got no health.
#[derive(Debug, Clone)]
pub struct Missed {
    /// For people: «колонка не отвечает».
    pub said: String,
    /// For the journal: the bottom of the error (see `why`).
    pub why: String,
    /// Nothing answered at all — refused, no route, no answer in time —, not a station that answered with an error.
    pub gone: bool,
}

/// Why the station was not reached, for the journal: the bottom of the error's chain — «Connection refused (os error
/// 111)», «Network unreachable (os error 101)» —, not reqwest's «error sending request for url (…)» alone.
pub fn why(error: &reqwest::Error) -> String {
    crate::utils::http::deepest_cause(error)
}

pub fn describe(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        crate::tr!("колонка не ответила вовремя")
    } else if error.is_connect() {
        crate::tr!("колонка не отвечает")
    } else if error.is_request() || error.is_body() {
        // Cut off midway: the station's gateway went away under the request (restarted, or killed for memory).
        crate::tr!("связь с колонкой оборвалась на полпути")
    } else {
        crate::tr!("Колонка: {error}", error = error)
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
