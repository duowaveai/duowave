//! Keys for what is not a person: a script with curl, an agent (OpenClaw, Hermes) that talks to the station, a future
//! cloud link. Each carries exactly the grants the admin gave it; the secret is shown once and kept only as its
//! SHA-256.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::js_len;
use super::users_repository::clean_restrictions;
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::core::identity::principal::{api_key_principal, Principal};
use crate::platform::http::ApiError;
use crate::storage::Db;
use crate::utils::ids::{random_bytes, uuid};
use crate::utils::time::{iso, now_iso, now_ms};

/// How a key starts: `vgw_<id>_<secret>`; the id finds the row, the secret is checked against its hash.
const PREFIX: &str = "vgw_";
/// lastUsedAt is written at most once a minute per key: a busy agent must not write on every call.
const TOUCH_EVERY_MS: u64 = 60_000;
const DAY_MS: f64 = 86_400_000.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyView {
    pub id: String,
    pub name: String,
    pub grants: Vec<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub expires_at: Option<String>,
    pub revoked: bool,
}

/// A new key: its secret is in this answer only.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CreatedKey {
    pub key: ApiKeyView,
    pub secret: String,
}

fn sha256(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

/// The grants of a stored list: only what the catalog knows.
fn grants_of(value: Option<&Value>) -> Vec<String> {
    let wrapped = json!({ "allow": value.cloned().unwrap_or(Value::Null) });
    clean_restrictions(Some(&wrapped)).allow.unwrap_or_default()
}

/// The id of a token shaped like a key: `vgw_` + 12 hex digits + `_` + at least 32 of [A-Za-z0-9_-].
fn key_id(token: &str) -> Option<&str> {
    let rest = token.strip_prefix(PREFIX)?;
    let (id, secret) = (rest.get(..12)?, rest.get(12..)?);
    let secret = secret.strip_prefix('_')?;
    let hex = id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let alphabet = secret.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    (hex && alphabet && secret.len() >= 32).then_some(id)
}

pub struct ApiKeysService {
    db: Arc<Db>,
    events: EventBus,
    touched: Mutex<HashMap<String, u64>>,
}

impl ApiKeysService {
    pub fn new(db: Arc<Db>, events: EventBus) -> ApiKeysService {
        ApiKeysService { db, events, touched: Mutex::new(HashMap::new()) }
    }

    pub fn looks_like_key(token: &str) -> bool {
        token.starts_with(PREFIX)
    }

    pub fn list(&self) -> Result<Vec<ApiKeyView>, ApiError> {
        let rows = self.db.with(|c| {
            let mut statement = c.prepare(
                "SELECT id, name, grants, created_at, last_used_at, expires_at, revoked_at FROM api_keys
                 ORDER BY created_at, rowid",
            )?;
            let rows = statement.query_map([], |row| {
                let grants: String = row.get(2)?;
                let revoked: Option<String> = row.get(6)?;
                Ok(ApiKeyView {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    grants: grants_of(serde_json::from_str::<Value>(&grants).ok().as_ref()),
                    created_at: row.get(3)?,
                    last_used_at: row.get(4)?,
                    expires_at: row.get(5)?,
                    revoked: revoked.is_some(),
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })?;
        Ok(rows)
    }

    pub fn create(&self, input: &Map<String, Value>, actor: Option<Actor>) -> Result<CreatedKey, ApiError> {
        let name = input.get("name").and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
        if !(2..=60).contains(&js_len(&name)) {
            return Err(ApiError::bad_request("Имя ключа — от 2 до 60 символов."));
        }
        let grants = grants_of(input.get("grants"));
        if grants.is_empty() {
            return Err(ApiError::bad_request("Ключу нужны права: хотя бы одно из каталога."));
        }
        let days = input.get("expiresInDays").and_then(Value::as_f64).filter(|days| *days > 0.0).map(|d| d.min(3650.0));
        let id: String = uuid().replace('-', "").chars().take(12).collect();
        let secret = format!("{PREFIX}{id}_{}", URL_SAFE_NO_PAD.encode(random_bytes(32)));
        let now = now_ms();
        let created_at = now_iso();
        let expires_at = days.and_then(|days| {
            chrono::DateTime::from_timestamp_millis(now as i64 + (days * DAY_MS).round() as i64).map(iso)
        });
        self.db.with(|c| {
            c.execute(
                "INSERT INTO api_keys (id, name, secret_hash, grants, created_by, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    name,
                    sha256(&secret),
                    serde_json::to_string(&grants).unwrap_or_else(|_| "[]".into()),
                    actor.as_ref().map(|actor| actor.id.clone()),
                    created_at,
                    expires_at,
                ],
            )
        })?;
        self.events.emit(GatewayEvent::ApiKeyCreated { key_id: id.clone(), name: name.clone(), actor });
        Ok(CreatedKey {
            key: ApiKeyView { id, name, grants, created_at, last_used_at: None, expires_at, revoked: false },
            secret,
        })
    }

    pub fn revoke(&self, id: &str, actor: Option<Actor>) -> Result<(), ApiError> {
        let name: Option<String> = self.db.with(|c| {
            c.query_row("SELECT name FROM api_keys WHERE id = ?1", params![id], |row| row.get(0)).optional()
        })?;
        let Some(name) = name else { return Err(ApiError::not_found("Нет такого ключа.")) };
        self.db.with(|c| c.execute("UPDATE api_keys SET revoked_at = ?1 WHERE id = ?2", params![now_iso(), id]))?;
        self.events.emit(GatewayEvent::ApiKeyRevoked { key_id: id.to_string(), name, actor });
        Ok(())
    }

    /// The key's principal, for a live, unexpired key whose secret matches; None otherwise.
    pub fn verify(&self, token: &str) -> Option<Principal> {
        self.try_verify(token).unwrap_or(None)
    }

    /// The same, with the database's failure as an error rather than «no such key».
    pub fn try_verify(&self, token: &str) -> Result<Option<Principal>, ApiError> {
        let Some(id) = key_id(token) else { return Ok(None) };
        type Row = (String, String, String, Option<String>, Option<String>);
        let row: Option<Row> = self.db.with(|c| {
            c.query_row(
                "SELECT name, secret_hash, grants, expires_at, revoked_at FROM api_keys WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional()
        })?;
        let Some((name, secret_hash, grants, expires_at, revoked_at)) = row else { return Ok(None) };
        if revoked_at.is_some() {
            return Ok(None);
        }
        if let Some(expires_at) = expires_at {
            // An expiry that does not read is no expiry the key can pass.
            let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(&expires_at) else { return Ok(None) };
            if expiry.timestamp_millis() < now_ms() as i64 {
                return Ok(None);
            }
        }
        let Ok(expected) = hex::decode(&secret_hash) else { return Ok(None) };
        let actual = Sha256::digest(token.as_bytes());
        if expected.len() != actual.len() || !bool::from(expected.as_slice().ct_eq(actual.as_slice())) {
            return Ok(None);
        }
        self.touch(id);
        let grants = grants_of(serde_json::from_str::<Value>(&grants).ok().as_ref());
        Ok(Some(api_key_principal(id, &name, &grants)))
    }

    fn touch(&self, id: &str) {
        let now = now_ms();
        {
            let mut touched = self.touched.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if now.saturating_sub(touched.get(id).copied().unwrap_or(0)) < TOUCH_EVERY_MS {
                return;
            }
            touched.insert(id.to_string(), now);
        }
        let at = chrono::DateTime::from_timestamp_millis(now as i64).map(iso).unwrap_or_else(now_iso);
        if let Err(error) =
            self.db.with(|c| c.execute("UPDATE api_keys SET last_used_at = ?1 WHERE id = ?2", params![at, id]))
        {
            tracing::debug!("api key {id}: last use not written: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::identity::can;

    fn service() -> ApiKeysService {
        ApiKeysService::new(Arc::new(Db::memory()), EventBus::new())
    }

    fn input(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn gives_a_key_exactly_its_grants_and_shows_the_secret_once() {
        let keys = service();
        let created =
            keys.create(&input(json!({"name": " OpenClaw ", "grants": ["station.view", "fly"]})), None).unwrap();
        assert_eq!(created.key.name, "OpenClaw");
        assert_eq!(created.key.grants, vec!["station.view"]);
        assert!(key_id(&created.secret).is_some(), "{}", created.secret);
        assert!(created.secret.starts_with(&format!("vgw_{}_", created.key.id)));
        let principal = keys.verify(&created.secret).unwrap();
        assert!(can(Some(&principal), "station.view"));
        assert!(!can(Some(&principal), "station.mode"));
        let listed = serde_json::to_string(&keys.list().unwrap()).unwrap();
        assert!(!listed.contains(&created.secret));
        assert!(keys.list().unwrap()[0].last_used_at.is_some(), "the first use is written");
    }

    #[test]
    fn refuses_a_revoked_an_expired_or_a_forged_key() {
        let keys = service();
        let created = keys.create(&input(json!({"name": "curl", "grants": ["*"]})), None).unwrap();
        let mut forged = created.secret.clone();
        forged.pop();
        forged.push(if created.secret.ends_with('A') { 'B' } else { 'A' });
        assert!(keys.verify(&forged).is_none());
        assert!(keys.verify("vgw_short").is_none());
        keys.revoke(&created.key.id, None).unwrap();
        assert!(keys.verify(&created.secret).is_none());
        assert!(keys.list().unwrap()[0].revoked);
        assert_eq!(keys.revoke("000000000000", None).unwrap_err().message, "Нет такого ключа.");

        let expiring = keys.create(&input(json!({"name": "day", "grants": ["*"], "expiresInDays": 1})), None).unwrap();
        assert!(expiring.key.expires_at.is_some());
        assert!(keys.verify(&expiring.secret).is_some());
        keys.db
            .with(|c| {
                c.execute(
                    "UPDATE api_keys SET expires_at = '2020-01-01T00:00:00.000Z' WHERE id = ?1",
                    params![expiring.key.id],
                )
            })
            .unwrap();
        assert!(keys.verify(&expiring.secret).is_none());
    }

    #[test]
    fn asks_for_a_name_and_at_least_one_grant() {
        let keys = service();
        assert_eq!(
            keys.create(&input(json!({"name": "x", "grants": ["*"]})), None).unwrap_err().message,
            "Имя ключа — от 2 до 60 символов."
        );
        assert_eq!(
            keys.create(&input(json!({"name": "script", "grants": ["fly"]})), None).unwrap_err().message,
            "Ключу нужны права: хотя бы одно из каталога."
        );
    }

    #[test]
    fn knows_a_key_by_its_shape() {
        let secret = "A".repeat(43);
        assert_eq!(key_id(&format!("vgw_0123456789ab_{secret}")), Some("0123456789ab"));
        assert_eq!(key_id(&format!("vgw_0123456789AB_{secret}")), None, "the id is lowercase hex");
        assert_eq!(key_id(&format!("vgw_0123456789ab_{}", "A".repeat(31))), None);
        assert_eq!(key_id(&format!("vgw_0123456789ab-{secret}")), None);
        assert_eq!(key_id(&format!("vgw_0123456789ab_{secret}!")), None);
        assert!(ApiKeysService::looks_like_key("vgw_anything"));
        assert!(!ApiKeysService::looks_like_key("v2.user.0.1.sig"));
    }
}
