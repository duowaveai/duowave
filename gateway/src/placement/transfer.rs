//! The settings package (docs/plans/station-placement.md, sections 4 and 10): what travels between the computer's
//! gateway and the station's — people and their keys, routes, «Мозг» with its keys, connectors with theirs, the
//! characters, MCP servers, the stations and their looks, the mode — with a schema and a version, never the database
//! file: the builds' tables differ (the voice and model catalogs are the computer's), versions may too. Never travels:
//! the journals and the history (they stay where they were written), voices, models, the voice lab, and each side's
//! own state (where it runs, its pair, its sign-in secret).
//!
//! A package is applied at the gateway's start, before any service reads the database (`apply_incoming`): a running
//! gateway that receives one keeps it and starts again. Between the two gateways it travels sealed with the pair's key
//! (`seal`, `open`): plain HTTP at home does not show it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::secrets::SecretsService;
use crate::storage::Db;
use crate::utils::time::now_iso;

pub const FORMAT: &str = "duowave-settings";
/// The package's schema: a gateway refuses one newer than it knows.
pub const SCHEMA: u32 = 1;

/// The tables that travel whole, in the order they are written (people before their own settings).
pub const TABLES: &[&str] =
    &["users", "user_settings", "api_keys", "characters", "llm_providers", "mcp_servers", "stations"];

/// Settings each gateway keeps for itself.
pub const LOCAL_SETTINGS: &[&str] = &[
    "panel_secret",
    "gateway_state",
    "station_placement",
    "station_placements",
    "station_links",
    "connector_home_server",
    // The voice and the recognition are chosen from each side's own catalog.
    "tts_provider",
    "tts_own",
    "asr_model",
];

/// The secrets that travel: the model providers', the connectors', the MCP servers'. Everything else (the pair's
/// key, the home server's, the panel's sessions) is each side's own.
pub const TRAVELLING_SECRETS: &[&str] = &["llm:", "connector:", "mcp:"];

/// The settings package.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bundle {
    pub format: String,
    pub schema: u32,
    /// The version of the gateway that wrote it.
    pub version: String,
    /// pc or station.
    pub from: String,
    pub at: String,
    /// The writer's settings revision.
    #[serde(default)]
    pub revision: u64,
    /// Table → rows, each row column → value.
    pub tables: BTreeMap<String, Vec<Map<String, Value>>>,
    /// Setting key → its stored JSON.
    pub settings: BTreeMap<String, Value>,
    pub secrets: BTreeMap<String, String>,
}

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    pub rows: BTreeMap<String, usize>,
    pub settings: usize,
    pub secrets: usize,
    /// Rows this gateway cannot use and left out (MCP programs on the station).
    pub skipped: usize,
}

fn sql_to_json(value: SqlValue) -> Value {
    match value {
        SqlValue::Null => Value::Null,
        SqlValue::Integer(number) => Value::from(number),
        SqlValue::Real(number) => Value::from(number),
        SqlValue::Text(text) => Value::String(text),
        SqlValue::Blob(bytes) => Value::String(B64.encode(bytes)),
    }
}

fn json_to_sql(value: &Value) -> SqlValue {
    match value {
        Value::Null => SqlValue::Null,
        Value::Bool(flag) => SqlValue::Integer(*flag as i64),
        Value::Number(number) => match number.as_i64() {
            Some(integer) => SqlValue::Integer(integer),
            None => SqlValue::Real(number.as_f64().unwrap_or(0.0)),
        },
        Value::String(text) => SqlValue::Text(text.clone()),
        other => SqlValue::Text(other.to_string()),
    }
}

fn columns(connection: &rusqlite::Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement.query_map([], |row| row.get::<_, String>(1))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(names)
}

fn travels(secret: &str) -> bool {
    TRAVELLING_SECRETS.iter().any(|prefix| secret.starts_with(prefix))
}

/// This gateway's settings as a package.
pub fn export(db: &Db, secrets: &SecretsService, from: &str) -> rusqlite::Result<Bundle> {
    let (tables, settings) = db.with(|connection| {
        let mut tables = BTreeMap::new();
        for table in TABLES {
            let names = columns(connection, table)?;
            let mut statement = connection.prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))?;
            let rows = statement
                .query_map([], |row| {
                    let mut object = Map::new();
                    for (index, name) in names.iter().enumerate() {
                        object.insert(name.clone(), sql_to_json(row.get::<_, SqlValue>(index)?));
                    }
                    Ok(object)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            tables.insert(table.to_string(), rows);
        }
        let mut statement = connection.prepare("SELECT key, value FROM settings ORDER BY key")?;
        let settings = statement
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|(key, _)| !LOCAL_SETTINGS.contains(&key.as_str()))
            .filter_map(|(key, text)| serde_json::from_str(&text).ok().map(|value| (key, value)))
            .collect();
        Ok((tables, settings))
    })?;
    let secrets = secrets
        .ids("")
        .into_iter()
        .filter(|id| travels(id))
        .filter_map(|id| secrets.get(&id).map(|value| (id, value)))
        .collect();
    Ok(Bundle {
        format: FORMAT.into(),
        schema: SCHEMA,
        version: crate::VERSION.into(),
        from: from.into(),
        at: now_iso(),
        revision: 0,
        tables,
        settings,
        secrets,
    })
}

/// Makes this gateway's settings the package's: the travelling tables replaced whole, the travelling settings and
/// secrets set and the ones the package has not removed. `station` leaves out what the station's build cannot run
/// (MCP servers as programs). One transaction for the database; the secrets after it.
pub fn import(db: &Db, secrets: &SecretsService, bundle: &Bundle, station: bool) -> Result<Imported, String> {
    if bundle.format != FORMAT {
        return Err("это не пакет настроек шлюза".into());
    }
    if bundle.schema > SCHEMA {
        return Err(format!("пакет настроек новее этого шлюза (схема {}): сначала обновите шлюз", bundle.schema));
    }
    let mut imported = Imported::default();
    db.transaction(|transaction| {
        // People last cleared first: their settings go with them.
        for table in TABLES.iter().rev() {
            transaction.execute(&format!("DELETE FROM {table}"), [])?;
        }
        for table in TABLES {
            let known = columns(transaction, table)?;
            let mut written = 0;
            for row in bundle.tables.get(*table).map(Vec::as_slice).unwrap_or_default() {
                if station && *table == "mcp_servers" && row.get("transport").and_then(Value::as_str) == Some("stdio") {
                    imported.skipped += 1;
                    continue;
                }
                let names: Vec<&String> = row.keys().filter(|name| known.contains(name)).collect();
                if names.is_empty() {
                    continue;
                }
                let placeholders = (1..=names.len()).map(|index| format!("?{index}")).collect::<Vec<_>>().join(", ");
                let list = names.iter().map(|name| name.as_str()).collect::<Vec<_>>().join(", ");
                let values: Vec<SqlValue> = names.iter().map(|name| json_to_sql(&row[*name])).collect();
                transaction.execute(
                    &format!("INSERT INTO {table} ({list}) VALUES ({placeholders})"),
                    rusqlite::params_from_iter(values),
                )?;
                written += 1;
            }
            imported.rows.insert(table.to_string(), written);
        }
        let stored: Vec<String> = transaction
            .prepare("SELECT key FROM settings")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for key in stored {
            if !LOCAL_SETTINGS.contains(&key.as_str()) && !bundle.settings.contains_key(&key) {
                transaction.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
            }
        }
        for (key, value) in &bundle.settings {
            if LOCAL_SETTINGS.contains(&key.as_str()) {
                continue;
            }
            transaction.execute(
                "INSERT INTO settings (key, value, is_default, updated_at) VALUES (?1, ?2, 0, ?3)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value, is_default = 0, updated_at = excluded.updated_at",
                params![key, value.to_string(), now_iso()],
            )?;
            imported.settings += 1;
        }
        Ok(())
    })
    .map_err(|error| format!("база шлюза: {error}"))?;
    for id in secrets.ids("") {
        if travels(&id) && !bundle.secrets.contains_key(&id) {
            secrets.delete(&id).map_err(|error| error.message)?;
        }
    }
    for (id, value) in &bundle.secrets {
        if travels(id) {
            secrets.set(id, Some(value)).map_err(|error| error.message)?;
            imported.secrets += 1;
        }
    }
    Ok(imported)
}

/// The package the computer sent, applied before the services start; the database copied first. A package that does
/// not apply stays beside it as `.failed`, the gateway starts on what it had.
pub fn apply_incoming(state_dir: &Path, db: &Db, secrets: &SecretsService, station: bool) -> Option<Imported> {
    let file = incoming_file(state_dir);
    let text = std::fs::read(&file).ok()?;
    let outcome = serde_json::from_slice::<Bundle>(&text)
        .map_err(|error| format!("пакет настроек не читается: {error}"))
        .and_then(|bundle| {
            if let Some(database) = db.file() {
                let copy = database.with_extension(format!("sqlite.before-settings-{}", stamp()));
                if let Err(error) = std::fs::copy(database, &copy) {
                    tracing::warn!("settings: the database is not copied before the package: {error}");
                }
            }
            import(db, secrets, &bundle, station).map(|imported| (imported, bundle))
        });
    match outcome {
        Ok((imported, bundle)) => {
            let _ = std::fs::remove_file(&file);
            tracing::info!(
                "settings: the package from the {} ({}) applied: {} settings, {} secrets, rows {:?}",
                bundle.from,
                bundle.at,
                imported.settings,
                imported.secrets,
                imported.rows
            );
            Some(imported)
        }
        Err(error) => {
            let _ = std::fs::rename(&file, file.with_extension("json.failed"));
            tracing::error!("settings: the package from the computer is not applied: {error}");
            None
        }
    }
}

pub fn incoming_file(state_dir: &Path) -> PathBuf {
    state_dir.join("incoming-settings.json")
}

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// Columns the gateway writes by itself as it runs — a station seen, a sign-in, a key used, a provider checked. They
/// travel with their rows but are no change of settings: counted, the revision grew every minute, and the computer
/// sent its settings each time and the station's gateway started again on them (live, 07.11).
pub const VOLATILE: &[(&str, &[&str])] = &[
    ("users", &["last_login_at"]),
    ("api_keys", &["last_used_at"]),
    ("llm_providers", &["last_check"]),
    ("stations", &["last_address", "first_seen", "last_seen"]),
];

/// The package's fingerprint: what changes with any setting that travels, nothing else (not the time it was
/// written, nor the columns the gateway keeps up by itself).
pub fn fingerprint(bundle: &Bundle) -> String {
    let mut tables = bundle.tables.clone();
    for (table, columns) in VOLATILE {
        for row in tables.get_mut(*table).into_iter().flatten() {
            for column in *columns {
                row.remove(*column);
            }
        }
    }
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&tables).unwrap_or_default());
    digest.update(serde_json::to_vec(&bundle.settings).unwrap_or_default());
    digest.update(serde_json::to_vec(&bundle.secrets).unwrap_or_default());
    hex::encode(digest.finalize())
}

/// The settings revision: grows by one whenever the settings that travel change. Kept in the database's state
/// table, beside nothing that travels.
pub fn revision(db: &Db, secrets: &SecretsService) -> rusqlite::Result<u64> {
    let bundle = export(db, secrets, "")?;
    let fingerprint = fingerprint(&bundle);
    db.with(|connection| {
        let stored: Option<String> = connection
            .query_row("SELECT items FROM state WHERE kind = 'settings-revision' AND device = ''", [], |row| row.get(0))
            .optional()?;
        let stored: Option<(u64, String)> = stored.and_then(|text| serde_json::from_str(&text).ok());
        match stored {
            Some((revision, hash)) if hash == fingerprint => Ok(revision),
            other => {
                let revision = other.map(|(revision, _)| revision + 1).unwrap_or(1);
                connection.execute(
                    "INSERT INTO state (kind, device, items, updated_at) VALUES ('settings-revision', '', ?1, ?2)
                     ON CONFLICT (kind, device) DO UPDATE SET items = excluded.items, updated_at = excluded.updated_at",
                    params![serde_json::to_string(&(revision, fingerprint)).unwrap_or_default(), now_iso()],
                )?;
                Ok(revision)
            }
        }
    })
}

/// When the computer of the pair last took this gateway's settings (`copied`) or sent its own (`received`): kept in
/// the state table, beside nothing that travels and without an event — a copy every revision is no news for the
/// history.
pub fn note_sync(db: &Db, what: &str) {
    let _ = db.with(|connection| {
        connection.execute(
            "INSERT INTO state (kind, device, items, updated_at) VALUES ('settings-sync', ?1, ?2, ?2)
             ON CONFLICT (kind, device) DO UPDATE SET items = excluded.items, updated_at = excluded.updated_at",
            params![what, now_iso()],
        )
    });
}

pub fn synced_at(db: &Db, what: &str) -> Option<String> {
    db.with(|connection| {
        connection
            .query_row("SELECT items FROM state WHERE kind = 'settings-sync' AND device = ?1", params![what], |row| {
                row.get(0)
            })
            .optional()
    })
    .ok()
    .flatten()
}

/// The pair's key as AES-256: the key itself never travels.
fn pair_cipher(pair_key: &str) -> Aes256Gcm {
    let mut digest = Sha256::new();
    digest.update(b"duowave-pair:");
    digest.update(pair_key.as_bytes());
    Aes256Gcm::new_from_slice(&digest.finalize()).expect("a 32-byte key")
}

/// A package sealed with the pair's key: `{sealed: 1, nonce, data}`.
pub fn seal(bundle: &Bundle, pair_key: &str) -> Value {
    let nonce = crate::utils::ids::random_bytes(12);
    let plain = serde_json::to_vec(bundle).unwrap_or_default();
    let data = pair_cipher(pair_key)
        .encrypt(&Nonce::try_from(&nonce[..]).expect("a 12-byte nonce"), plain.as_slice())
        .expect("AES-GCM seals any package");
    serde_json::json!({"sealed": 1, "nonce": B64.encode(nonce), "data": B64.encode(data)})
}

/// The package inside, or why not: another pair's key, a damaged one.
pub fn open(sealed: &Value, pair_key: &str) -> Result<Bundle, String> {
    let field = |name: &str| sealed.get(name).and_then(Value::as_str).and_then(|text| B64.decode(text).ok());
    let (Some(nonce), Some(data)) = (field("nonce"), field("data")) else {
        return Err("это не запечатанный пакет настроек".into());
    };
    let nonce = Nonce::try_from(&nonce[..]).map_err(|_| "пакет настроек повреждён".to_string())?;
    let plain = pair_cipher(pair_key)
        .decrypt(&nonce, data.as_slice())
        .map_err(|_| "пакет настроек не открывается: другой ключ пары или он повреждён".to_string())?;
    serde_json::from_slice(&plain).map_err(|error| format!("пакет настроек не читается: {error}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::core::events::EventBus;
    use crate::settings::definitions::{ModeSetting, PanelMode, GATEWAY_STATE, MODE};
    use crate::settings::SettingsService;

    fn gateway() -> (Arc<Db>, SecretsService, SettingsService) {
        let db = Arc::new(Db::memory());
        let secrets = SecretsService::new(db.clone(), None).unwrap();
        let settings = SettingsService::new(db.clone(), EventBus::new());
        (db, secrets, settings)
    }

    fn person(db: &Db, id: &str, name: &str) {
        db.with(|c| {
            c.execute(
                "INSERT INTO users (id, name, password_hash, role, created_at, updated_at) VALUES (?1, ?2, 'scrypt$x', 'admin', 'now', 'now')",
                params![id, name],
            )
        })
        .unwrap();
    }

    #[test]
    fn carries_the_settings_that_travel_and_keeps_each_sides_own() {
        let (pc, pc_secrets, pc_settings) = gateway();
        person(&pc, "u1", "Мама");
        pc.with(|c| {
            c.execute(
                "INSERT INTO mcp_servers (id, title, transport, config, created_at, updated_at) VALUES
                 ('home', 'Дом', 'http', '{}', 'now', 'now'), ('files', 'Файлы', 'stdio', '{}', 'now', 'now')",
                [],
            )
        })
        .unwrap();
        pc_settings.set(&MODE, &ModeSetting { mode: PanelMode::Gateway }, None).unwrap();
        pc_settings.set(&GATEWAY_STATE, &Default::default(), None).unwrap();
        pc_secrets.set("llm:claude:key", Some("sk-not-a-real-key")).unwrap();
        pc_secrets.set("placement:peer", Some("pair")).unwrap();
        let bundle = export(&pc, &pc_secrets, "pc").unwrap();
        assert!(bundle.settings.contains_key("gateway_mode") && !bundle.settings.contains_key("gateway_state"));
        assert_eq!(bundle.secrets.keys().collect::<Vec<_>>(), vec!["llm:claude:key"], "the pair's key stays");

        let (station, station_secrets, station_settings) = gateway();
        person(&station, "u9", "Старый");
        station_secrets.set("connector:telegram:token", Some("old")).unwrap();
        station_secrets.set("home-server:key", Some("mine")).unwrap();
        station_settings.set(&MODE, &ModeSetting { mode: PanelMode::Relay }, None).unwrap();
        let imported = import(&station, &station_secrets, &bundle, true).unwrap();
        assert_eq!((imported.rows["users"], imported.rows["mcp_servers"], imported.skipped), (1, 1, 1));
        assert_eq!(station_settings.get(&MODE, None), Some(ModeSetting { mode: PanelMode::Gateway }));
        let names: Vec<String> =
            station.with(|c| c.prepare("SELECT name FROM users")?.query_map([], |row| row.get(0))?.collect()).unwrap();
        assert_eq!(names, vec!["Мама"], "the package's people, the old ones gone");
        assert_eq!(station_secrets.get("llm:claude:key").as_deref(), Some("sk-not-a-real-key"));
        assert_eq!(station_secrets.get("connector:telegram:token"), None, "removed on the computer, removed here");
        assert_eq!(station_secrets.get("home-server:key").as_deref(), Some("mine"), "the station's own stays");

        // Back the other way: the same settings, the revision does not move for the time written.
        let back = export(&station, &station_secrets, "station").unwrap();
        assert_eq!(fingerprint(&back), fingerprint(&export(&station, &station_secrets, "station").unwrap()));
        assert_eq!(back.tables["users"].len(), 1);
    }

    #[test]
    fn the_revision_grows_with_each_change_and_only_then() {
        let (db, secrets, settings) = gateway();
        let first = revision(&db, &secrets).unwrap();
        assert_eq!(revision(&db, &secrets).unwrap(), first);
        settings.set(&MODE, &ModeSetting { mode: PanelMode::Own }, None).unwrap();
        assert_eq!(revision(&db, &secrets).unwrap(), first + 1);
        settings.set(&GATEWAY_STATE, &Default::default(), None).unwrap();
        assert_eq!(revision(&db, &secrets).unwrap(), first + 1, "the gateway's own state does not travel");
        secrets.set("llm:x:key", Some("k")).unwrap();
        assert_eq!(revision(&db, &secrets).unwrap(), first + 2);
    }

    #[test]
    fn what_the_gateway_writes_by_itself_is_no_new_revision() {
        let (db, secrets, _) = gateway();
        person(&db, "u1", "Мама");
        db.with(|c| {
            c.execute(
                "INSERT INTO stations (device, name, status, last_address, first_seen, last_seen) VALUES ('h:1', 'Кухня', 'approved', '192.0.2.23', 'then', 'then')",
                [],
            )
        })
        .unwrap();
        let first = revision(&db, &secrets).unwrap();
        db.with(|c| c.execute("UPDATE stations SET last_seen = 'now', last_address = '192.0.2.24'", [])).unwrap();
        db.with(|c| c.execute("UPDATE users SET last_login_at = 'now'", [])).unwrap();
        assert_eq!(revision(&db, &secrets).unwrap(), first, "a station seen, a sign-in: the same settings");
        db.with(|c| c.execute("UPDATE stations SET name = 'Спальня'", [])).unwrap();
        assert_eq!(revision(&db, &secrets).unwrap(), first + 1, "a name is a setting");
        let bundle = export(&db, &secrets, "pc").unwrap();
        assert_eq!(bundle.tables["stations"][0]["last_seen"], "now", "the columns still travel");
    }

    #[test]
    fn sealed_with_the_pairs_key_it_opens_only_with_it_and_a_newer_schema_is_refused() {
        let (db, secrets, _) = gateway();
        person(&db, "u1", "Мама");
        let bundle = export(&db, &secrets, "station").unwrap();
        let sealed = seal(&bundle, "pair-key");
        assert!(!sealed.to_string().contains("Мама"), "nothing readable on the way");
        assert_eq!(open(&sealed, "pair-key").unwrap(), bundle);
        assert!(open(&sealed, "another").is_err());
        let newer = Bundle { schema: SCHEMA + 1, ..bundle };
        assert!(import(&db, &secrets, &newer, false).unwrap_err().contains("новее"));
    }

    #[test]
    fn applies_a_package_left_for_the_next_start_once() {
        let (pc, pc_secrets, _) = gateway();
        person(&pc, "u1", "Мама");
        let bundle = export(&pc, &pc_secrets, "pc").unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(incoming_file(dir.path()), serde_json::to_vec(&bundle).unwrap()).unwrap();
        let (station, station_secrets, _) = gateway();
        let imported = apply_incoming(dir.path(), &station, &station_secrets, true).unwrap();
        assert_eq!(imported.rows["users"], 1);
        assert!(!incoming_file(dir.path()).exists());
        assert!(apply_incoming(dir.path(), &station, &station_secrets, true).is_none());
        std::fs::write(incoming_file(dir.path()), b"not a package").unwrap();
        assert!(apply_incoming(dir.path(), &station, &station_secrets, true).is_none());
        assert!(dir.path().join("incoming-settings.json.failed").exists());
    }

    #[test]
    fn notes_when_the_computer_took_or_sent_the_settings() {
        let (db, _, _) = gateway();
        assert_eq!(synced_at(&db, "copied"), None);
        note_sync(&db, "copied");
        assert!(synced_at(&db, "copied").is_some());
        assert_eq!(synced_at(&db, "received"), None);
    }
}
