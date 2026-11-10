//! The settings package (docs/plans/station-placement.md, sections 4 and 10): what travels between the computer's
//! gateway and the station's — people and their keys, routes, «Мозг» with its keys, connectors with theirs, the
//! characters, MCP servers, the stations and their looks — with a schema and a version, never the database file: the
//! builds' tables differ (the voice and model catalogs are the computer's), versions may too. The mode travels with a
//! move only (`MOVE_ONLY`): the computer's routine sending leaves it to the station. Never travels: the journals and
//! the history (they stay where they were written), voices, models, the voice lab, and each side's own state (where it
//! runs, its pair, its sign-in secret).
//!
//! A move carries more than the settings, and only a move (docs/plans/desk-after-station.md, В): the plugins' state —
//! each package's switch and its own data (`plugin_kv`) — for the packages both sides have, because the plugins' data
//! lives where the gateway runs (`Bundle::plugins`); and on the way to the station the computer's choice of voice and
//! the station's name as the computer knows it (`MOVE_SETS`).
//!
//! A package is applied at the gateway's start, before any service reads the database (`apply_incoming`), the whole
//! database copied first (`copy_database`): a running gateway that receives one keeps it and starts again. Between
//! the two gateways it travels sealed with the pair's key (`seal`, `open`): plain HTTP at home does not show it.

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
/// The schema this gateway's import knows; its /health says it (`build.settingsSchema`). A package names the oldest one
/// its import must know (`Bundle::schema`), and a gateway refuses a package that needs a newer one: the route before it
/// keeps anything, in the words every build's route has said since it came (`NOT_FOR_THIS_GATEWAY`), the start before
/// it applies anything (`import`, in words of its own). 2: a package may leave out what only a move carries
/// (`MOVE_ONLY`), and the import keeps the one stored — the computer's routine sending leaves out the mode (`routine`).
/// An import of schema 1 (every build before this one, the release 0.2.0 among them) removes each setting a package
/// lacks, the mode too: a station's gateway of such a build would lose its own mode at every sending and start in
/// .env's — on a station «Яндекс» (station_fs.rs, `write_env`). So the computer sends a station whose /health says no
/// schema the whole package with its own mode in it (`for_earlier`, src/placement/watch.rs); a routine package that
/// reaches such a station all the same is refused by its route («Пакет настроек не для этого шлюза: обновите его.»),
/// and its mode stays. The pair's routes stay the same: no new `PEER_API`.
pub const SCHEMA: u32 = 2;

/// What a gateway's route answers a package not for it — another format, or a schema newer than its import knows
/// (routes.rs, `import`): the same words in every build since the route came, so the computer knows an earlier
/// station's refusal by them (src/placement/watch.rs).
pub const NOT_FOR_THIS_GATEWAY: &str = "Пакет настроек не для этого шлюза: обновите его.";

/// The schema of a whole package (`export`): every setting that travels, the mode among them when one is stored. An
/// import of schema 1 applies it as it applied one of its own build, so a whole package still goes to an earlier
/// build: a move to a station whose build is older than this computer's (a file, the release of this version number),
/// the station's settings back to an earlier computer, a kept copy to an earlier station. Raised when a whole package
/// needs more of its import than schema 1.
pub const WHOLE: u32 = 1;

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
    // The voice and the recognition are chosen from each side's own catalog; so is the voice that waits for its model
    // («Голос»).
    "tts_provider",
    "tts_own",
    "asr_model",
    "voice_pending",
    // The station's own name and device as the computer knows them: a move brings them (`MOVE_SETS`).
    "station_self",
];

/// Of each side's own settings (`LOCAL_SETTINGS`), the ones a move to the station sets there all the same when its
/// package carries them (moves.rs): the computer's choice of voice when the computer gives the station its voice
/// (`stationServer`) — the station then speaks the computer's voice in «Шлюз» too, not only in «Свой» (live, 10.11) —
/// and the station's own name and device as the computer knows them («Детская», not «Колонка»). `export` leaves them
/// out as each side's own, so nothing but a move carries them; an import without them keeps the stored ones.
pub const MOVE_SETS: &[&str] = &["tts_provider", "station_self"];

/// Settings that travel with a move only. The station's mode is switched there — by voice, from either panel, by its
/// gateway itself when its home server goes — so the computer's routine sending (`routine`) leaves it out, and an
/// import keeps the one stored when the package does not carry it. A move carries the mode in force (moves.rs), the
/// station's own settings coming back carry its own, and so do a kept copy restored and the computer's sending to a
/// station of an earlier build: the station's own mode goes in it (`for_earlier`).
pub const MOVE_ONLY: &[&str] = &["gateway_mode"];

/// The secrets that travel: the model providers', the connectors', the MCP servers'. Everything else (the pair's
/// key, the home server's, the panel's sessions) is each side's own.
pub const TRAVELLING_SECRETS: &[&str] = &["llm:", "connector:", "mcp:"];

/// The settings package.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bundle {
    pub format: String,
    /// The oldest schema whose import applies it right (`SCHEMA`, `WHOLE`); who wrote it says `version`.
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
    /// The plugins' state by package id (`plugins`): only a move carries it, both ways — the plugins' data lives where
    /// the gateway runs. The routine sending, the copies and their restores leave it out; a build before it passes it
    /// by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<BTreeMap<String, PluginState>>,
}

/// A plugin's state that lives where the gateway runs: whether it is on, and its own data (`plugin_kv`). Never its
/// code: the package itself stays on each side as it was installed there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginState {
    pub enabled: bool,
    /// When it was switched on or off there: the order in which the plugins go off under pressure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default)]
    pub kv: Vec<KvEntry>,
}

/// One row of a plugin's own key-value store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KvEntry {
    pub key: String,
    /// Its JSON, as the store keeps it.
    pub value: String,
    pub updated_at: String,
}

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    pub rows: BTreeMap<String, usize>,
    pub settings: usize,
    pub secrets: usize,
    /// Rows this gateway cannot use and left out (MCP programs on the station, a station's provider for its home
    /// server on the computer).
    pub skipped: usize,
    /// The plugins whose state came with the package (both sides have them).
    pub plugins: usize,
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
        schema: WHOLE,
        version: crate::VERSION.into(),
        from: from.into(),
        at: now_iso(),
        revision: 0,
        tables,
        settings,
        secrets,
        plugins: None,
    })
}

/// The plugins' state here (`PluginState`), for a move: each package with a row of its own — every built-in has one,
/// and so has every package installed or switched on once — and its data.
pub fn plugins(db: &Db) -> rusqlite::Result<BTreeMap<String, PluginState>> {
    db.with(|connection| {
        let mut found: BTreeMap<String, PluginState> = connection
            .prepare("SELECT id, enabled, updated_at FROM packages ORDER BY id")?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    PluginState { enabled: row.get::<_, i64>(1)? != 0, at: row.get(2)?, kv: Vec::new() },
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut statement =
            connection.prepare("SELECT plugin_id, key, value, updated_at FROM plugin_kv ORDER BY plugin_id, key")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, KvEntry { key: row.get(1)?, value: row.get(2)?, updated_at: row.get(3)? }))
        })?;
        for row in rows {
            let (id, entry) = row?;
            if let Some(state) = found.get_mut(&id) {
                state.kv.push(entry);
            }
        }
        Ok(found)
    })
}

/// The other side's plugins' state, for the packages this side has too: on or off as there, their data as there. A
/// package this side does not have is passed by — its code never travels — and one only this side has stays as it is.
/// «Has»: a row of its own here, or built into this gateway (a station's first start has no rows yet: the built-ins
/// are laid out after the package is applied, and keep the choice they find, bundled.rs). A system package stays on.
fn apply_plugins(
    connection: &rusqlite::Connection,
    plugins: &BTreeMap<String, PluginState>,
) -> rusqlite::Result<usize> {
    use crate::plugins::registry::SOURCE_SYSTEM;
    let rows: BTreeMap<String, String> = connection
        .prepare("SELECT id, source FROM packages")?
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let built_in: BTreeMap<String, &'static str> =
        crate::plugins::bundled::packages().into_iter().map(|package| (package.id, package.source)).collect();
    let mut carried = 0;
    for (id, state) in plugins {
        let source = match (rows.get(id), built_in.get(id)) {
            (Some(source), _) => source.clone(),
            (None, Some(source)) => source.to_string(),
            (None, None) => continue,
        };
        let enabled = state.enabled || source == SOURCE_SYSTEM;
        connection.execute(
            "INSERT INTO packages (id, enabled, version, updated_at, updated_by, source, verified)
             VALUES (?1, ?2, NULL, ?3, NULL, ?4, ?5)
             ON CONFLICT (id) DO UPDATE SET enabled = excluded.enabled, updated_at = excluded.updated_at",
            params![
                id,
                enabled as i64,
                state.at.clone().unwrap_or_else(now_iso),
                source,
                built_in.contains_key(id) as i64
            ],
        )?;
        connection.execute("DELETE FROM plugin_kv WHERE plugin_id = ?1", params![id])?;
        for entry in &state.kv {
            connection.execute(
                "INSERT INTO plugin_kv (plugin_id, key, value, updated_at) VALUES (?1, ?2, ?3, ?4)",
                params![id, entry.key, entry.value, entry.updated_at],
            )?;
        }
        carried += 1;
    }
    Ok(carried)
}

/// The computer's model as a station's «Мозг» has it: a provider at a home server's model route (`…/home/v1/llm`,
/// home_server/serve.rs), which the station adds itself (home_server/mod.rs). On the computer it would point at the
/// computer itself (live, 10.11: «Модель на компьютере» came back with the move). Known by its address, not its
/// title.
pub fn at_home_server(row: &Map<String, Value>) -> bool {
    row.get("base_url")
        .and_then(Value::as_str)
        .and_then(|url| url::Url::parse(url.trim()).ok())
        .is_some_and(|url| url.path().trim_end_matches('/').ends_with(HOME_MODEL_ROUTE))
}

/// The home server's model route on the computer (home_server/serve.rs): a station's provider for it ends so.
const HOME_MODEL_ROUTE: &str = "/home/v1/llm";

/// The computer's own model among the providers a package brings: the one its home server hands the station's «Мозг»
/// (home_server/serve.rs, `own_model`) — the first OpenAI-compatible provider at home, by when it was added.
fn own_model(rows: &[&Map<String, Value>]) -> Option<String> {
    let mut models: Vec<(&str, usize, &str)> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.get("kind").and_then(Value::as_str) == Some("openai"))
        .filter(|(_, row)| {
            row.get("base_url")
                .and_then(Value::as_str)
                .is_some_and(|url| crate::operations::networks::is_local_url(Some(url)))
        })
        .filter_map(|(index, row)| {
            Some((row.get("created_at").and_then(Value::as_str).unwrap_or(""), index, row.get("id")?.as_str()?))
        })
        .collect();
    models.sort();
    models.first().map(|(_, _, id)| id.to_string())
}

/// A choice of model that answered with a provider left out (`at_home_server`): with the computer's own model, which
/// that provider reached through the home server, else none — «Мозг» falls back to .env's. `brain` holds `fast` and
/// `smart`, `workshop.agent` one pick. None: nothing of it stays.
fn pick_again(key: &str, value: &Value, left_out: &[String], own: Option<&str>) -> Option<Value> {
    let gone = |pick: &Value| {
        pick.get("provider").and_then(Value::as_str).is_some_and(|id| left_out.iter().any(|left| left == id))
    };
    let again = |pick: &Value| -> Option<Value> {
        let mut pick = pick.clone();
        pick["provider"] = Value::String(own?.to_string());
        Some(pick)
    };
    match key {
        "brain" => {
            let mut value = value.clone();
            if let Some(object) = value.as_object_mut() {
                for profile in ["fast", "smart"] {
                    match object.get(profile).filter(|pick| gone(pick)).map(again) {
                        Some(Some(pick)) => {
                            object.insert(profile.into(), pick);
                        }
                        Some(None) => {
                            object.remove(profile);
                        }
                        None => {}
                    }
                }
            }
            Some(value)
        }
        "workshop.agent" if gone(value) => again(value),
        _ => Some(value.clone()),
    }
}

/// The package of the computer's routine sending, every time its settings change (src/placement/watch.rs): without
/// what only a move carries — the station keeps its own mode — and so of schema 2 (`SCHEMA`): a station of an earlier
/// build refuses it rather than lose its mode. Such a station gets `for_earlier` instead.
pub fn routine(mut bundle: Bundle) -> Bundle {
    bundle.settings.retain(|key, _| !MOVE_ONLY.contains(&key.as_str()));
    bundle.schema = SCHEMA;
    bundle
}

/// The whole package for a station's gateway of an earlier build, whose /health says no schema (watch.rs): its import
/// knows schema 1 only and removes each setting a package lacks. So what this build keeps for each side
/// (`LOCAL_SETTINGS`) or carries with a move only (`MOVE_ONLY`) goes in as the station's own, as its own package says
/// it (`theirs`), and stays out when its package has none: its import then leaves the station its mode and its waiting
/// voice (`voice_pending` travelled before this build). What such a build kept for itself too is not in its package,
/// and its import passes it by anyway. Schema `WHOLE`: every build takes it.
pub fn for_earlier(mut bundle: Bundle, theirs: &Bundle) -> Bundle {
    for key in LOCAL_SETTINGS.iter().chain(MOVE_ONLY) {
        match theirs.settings.get(*key) {
            Some(value) => bundle.settings.insert(key.to_string(), value.clone()),
            None => bundle.settings.remove(*key),
        };
    }
    bundle.schema = WHOLE;
    bundle
}

/// Makes this gateway's settings the package's: the travelling tables replaced whole, the travelling settings and
/// secrets set and the ones the package has not removed — but a `MOVE_ONLY` one it does not carry stays as it is, and
/// of each side's own only what a move sets (`MOVE_SETS`) is taken. `station` leaves out what the station's build
/// cannot run (MCP servers as programs); the computer leaves out the provider a station made for its home server
/// (`at_home_server`), its key, and a choice that answered with it goes to the computer's own model. A key revoked
/// here stays revoked whatever the package says. The plugins' state a move carries goes to the packages both sides
/// have (`apply_plugins`). One transaction for the database; the secrets after it.
pub fn import(db: &Db, secrets: &SecretsService, bundle: &Bundle, station: bool) -> Result<Imported, String> {
    if bundle.format != FORMAT {
        return Err("это не пакет настроек шлюза".into());
    }
    if bundle.schema > SCHEMA {
        return Err(format!("пакет настроек новее этого шлюза (схема {}): сначала обновите шлюз", bundle.schema));
    }
    let mut imported = Imported::default();
    // The station's provider for its home server, never the computer's: by id, with its key.
    let providers: Vec<&Map<String, Value>> =
        bundle.tables.get("llm_providers").map(|rows| rows.iter().collect()).unwrap_or_default();
    let left_out: Vec<String> = if station {
        Vec::new()
    } else {
        providers
            .iter()
            .filter(|row| at_home_server(row))
            .filter_map(|row| row.get("id").and_then(Value::as_str).map(str::to_string))
            .collect()
    };
    let own = own_model(&providers.iter().copied().filter(|row| !at_home_server(row)).collect::<Vec<_>>());
    let kept_secret = |id: &str| !left_out.iter().any(|provider| id.starts_with(&format!("llm:{provider}:")));
    db.transaction(|transaction| {
        // A key revoked here stays revoked: the package is the other side's copy, made before this side revoked it —
        // a move back revokes the station's key to its home server, then applies the station's settings, and the key
        // came back to life (live, 08.11 and 10.11).
        let revoked: Vec<(String, String)> = transaction
            .prepare("SELECT id, revoked_at FROM api_keys WHERE revoked_at IS NOT NULL AND revoked_at != ''")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
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
                if *table == "llm_providers"
                    && row.get("id").and_then(Value::as_str).is_some_and(|id| left_out.iter().any(|left| left == id))
                {
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
        for (id, at) in &revoked {
            transaction.execute(
                "UPDATE api_keys SET revoked_at = ?2 WHERE id = ?1 AND (revoked_at IS NULL OR revoked_at = '')",
                params![id, at],
            )?;
        }
        let stored: Vec<String> = transaction
            .prepare("SELECT key FROM settings")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for key in stored {
            let kept = LOCAL_SETTINGS.contains(&key.as_str()) || MOVE_ONLY.contains(&key.as_str());
            if !kept && !bundle.settings.contains_key(&key) {
                transaction.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
            }
        }
        for (key, value) in &bundle.settings {
            if LOCAL_SETTINGS.contains(&key.as_str()) && !MOVE_SETS.contains(&key.as_str()) {
                continue;
            }
            let Some(value) = pick_again(key, value, &left_out, own.as_deref()) else {
                transaction.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
                continue;
            };
            transaction.execute(
                "INSERT INTO settings (key, value, is_default, updated_at) VALUES (?1, ?2, 0, ?3)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value, is_default = 0, updated_at = excluded.updated_at",
                params![key, value.to_string(), now_iso()],
            )?;
            imported.settings += 1;
        }
        if let Some(plugins) = &bundle.plugins {
            imported.plugins = apply_plugins(transaction, plugins)?;
        }
        Ok(())
    })
    .map_err(|error| format!("база шлюза: {error}"))?;
    for id in secrets.ids("") {
        if travels(&id) && !(bundle.secrets.contains_key(&id) && kept_secret(&id)) {
            secrets.delete(&id).map_err(|error| error.message)?;
        }
    }
    for (id, value) in &bundle.secrets {
        if travels(id) && kept_secret(id) {
            secrets.set(id, Some(value)).map_err(|error| error.message)?;
            imported.secrets += 1;
        }
    }
    Ok(imported)
}

/// The plugins' state alone, from the station a move back took it from while the settings lived on the computer
/// (moves.rs): the computer keeps its own settings and takes the plugins' (`apply_plugins`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginsPackage {
    pub from: String,
    pub at: String,
    pub plugins: BTreeMap<String, PluginState>,
}

/// The plugins' state of a package alone, applied in a transaction of its own.
pub fn import_plugins(db: &Db, plugins: &BTreeMap<String, PluginState>) -> Result<usize, String> {
    db.transaction(|transaction| apply_plugins(transaction, plugins)).map_err(|error| format!("база шлюза: {error}"))
}

/// What the other gateway of the pair left for this one's start (a package of settings, the plugins' state of a move
/// back), applied before the services start; the whole database copied first (`copy_database`). When the copy fails
/// nothing is applied: what was left stays beside it as `.failed` and the gateway starts on what it had — as it does
/// with a package that does not apply.
pub fn apply_incoming(state_dir: &Path, db: &Db, secrets: &SecretsService, station: bool) -> Option<Imported> {
    let settings = incoming_file(state_dir);
    let plugins = incoming_plugins_file(state_dir);
    if !settings.exists() && !plugins.exists() {
        return None;
    }
    if let Err(error) = copy_database(db) {
        for file in [&settings, &plugins] {
            if file.exists() {
                let _ = std::fs::rename(file, failed(file));
            }
        }
        tracing::error!("settings: {error}: the package from the other gateway is not applied (kept as .failed)");
        return None;
    }
    let imported = apply_settings(&settings, db, secrets, station);
    apply_plugins_file(&plugins, db);
    imported
}

fn failed(file: &Path) -> PathBuf {
    file.with_extension("json.failed")
}

fn apply_settings(file: &Path, db: &Db, secrets: &SecretsService, station: bool) -> Option<Imported> {
    let text = std::fs::read(file).ok()?;
    let outcome = serde_json::from_slice::<Bundle>(&text)
        .map_err(|error| format!("пакет настроек не читается: {error}"))
        .and_then(|bundle| import(db, secrets, &bundle, station).map(|imported| (imported, bundle)));
    match outcome {
        Ok((imported, bundle)) => {
            let _ = std::fs::remove_file(file);
            tracing::info!(
                "settings: the package from the {} ({}) applied: {} settings, {} secrets, {} plugins, rows {:?}",
                bundle.from,
                bundle.at,
                imported.settings,
                imported.secrets,
                imported.plugins,
                imported.rows
            );
            Some(imported)
        }
        Err(error) => {
            let _ = std::fs::rename(file, failed(file));
            tracing::error!("settings: the package from the other gateway is not applied: {error}");
            None
        }
    }
}

fn apply_plugins_file(file: &Path, db: &Db) {
    let Ok(text) = std::fs::read(file) else { return };
    let outcome = serde_json::from_slice::<PluginsPackage>(&text)
        .map_err(|error| format!("не читается: {error}"))
        .and_then(|package| import_plugins(db, &package.plugins).map(|count| (count, package)));
    match outcome {
        Ok((count, package)) => {
            let _ = std::fs::remove_file(file);
            tracing::info!(
                "settings: the plugins' state from the {} ({}) applied: {count} plugins",
                package.from,
                package.at
            );
        }
        Err(error) => {
            let _ = std::fs::rename(file, failed(file));
            tracing::error!("settings: the plugins' state from the other gateway is not applied: {error}");
        }
    }
}

/// The whole database copied beside it before a package changes it: `VACUUM INTO`, so what the write-ahead log holds
/// is in the copy too — a copy of the main file alone was 4 KB and useless (live, 10.11). None: in memory, nothing to
/// copy.
pub fn copy_database(db: &Db) -> Result<Option<PathBuf>, String> {
    let Some(file) = db.file() else { return Ok(None) };
    let stamp = stamp();
    let copy = (1..)
        .map(|n| {
            let suffix = if n == 1 { String::new() } else { format!("-{n}") };
            file.with_extension(format!("sqlite.before-settings-{stamp}{suffix}"))
        })
        .find(|copy| !copy.exists())
        .expect("a free name");
    db.with(|connection| connection.execute("VACUUM INTO ?1", params![copy.to_string_lossy()]))
        .map_err(|error| format!("the database is not copied before the package: {error}"))?;
    super::station_fs::restrict(&copy);
    tracing::info!("settings: the database copied before the package: {}", copy.display());
    Ok(Some(copy))
}

pub fn incoming_file(state_dir: &Path) -> PathBuf {
    state_dir.join("incoming-settings.json")
}

/// The plugins' state a move back left for the computer's start (`PluginsPackage`).
pub fn incoming_plugins_file(state_dir: &Path) -> PathBuf {
    state_dir.join("incoming-plugins.json")
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

/// The import of every build before schema 2 (the apply loop in 708eec6 and earlier, the release 0.2.0's), its settings
/// part, for the tests here and the computer's (watch.rs): refuses a package newer than schema 1; removes each stored
/// setting the package lacks but its own — the voice waiting for its model was not its own then — and sets the
/// package's.
#[cfg(test)]
pub(crate) fn earlier_import(stored: &mut BTreeMap<String, Value>, bundle: &Bundle) -> Result<(), String> {
    let own = |key: &str| key != "voice_pending" && LOCAL_SETTINGS.contains(&key);
    if bundle.schema > 1 {
        return Err(format!("пакет настроек новее этого шлюза (схема {}): сначала обновите шлюз", bundle.schema));
    }
    stored.retain(|key, _| own(key) || bundle.settings.contains_key(key));
    for (key, value) in bundle.settings.iter().filter(|(key, _)| !own(key)) {
        stored.insert(key.clone(), value.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::core::events::EventBus;
    use crate::settings::definitions::{ModeSetting, PanelMode, GATEWAY_STATE, KEEP_REQUESTS, MODE};
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

    /// The voice waiting for its model («Голос»), stored as the models' service keeps it.
    fn waiting(db: &Db, model: &str) {
        db.with(|c| {
            c.execute(
                "INSERT INTO settings (key, value, is_default, updated_at) VALUES ('voice_pending', ?1, 0, 'now')",
                params![json!({ "modelId": model }).to_string()],
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

    /// Review G1: the station's mode is switched there — by voice, from either panel, by its gateway itself — and the
    /// computer's sending on every change of its own overwrote it. The routine sending leaves it as it is; a move
    /// still sets the mode in force. The voice waiting for its model is each side's own and never travels.
    #[test]
    fn a_routine_sending_keeps_the_stations_mode_and_a_move_sets_the_mode_in_force() {
        let (pc, pc_secrets, pc_settings) = gateway();
        pc_settings.set(&MODE, &ModeSetting { mode: PanelMode::Own }, None).unwrap();
        pc_settings.set(&KEEP_REQUESTS, &7, None).unwrap();
        waiting(&pc, "pc-voice");
        let (station, station_secrets, station_settings) = gateway();
        station_settings.set(&MODE, &ModeSetting { mode: PanelMode::Relay }, None).unwrap();
        waiting(&station, "station-voice");

        let full = export(&pc, &pc_secrets, "pc").unwrap();
        assert!(full.settings.contains_key("gateway_mode") && !full.settings.contains_key("voice_pending"));
        let sent = routine(full.clone());
        assert!(!sent.settings.contains_key("gateway_mode") && sent.settings.contains_key("requests_keep_days"));
        import(&station, &station_secrets, &sent, true).unwrap();
        let relay = Some(ModeSetting { mode: PanelMode::Relay });
        assert_eq!(station_settings.get(&MODE, None), relay, "the station's own");
        assert_eq!(station_settings.get(&KEEP_REQUESTS, None), Some(7), "the rest of the computer's settings came");
        assert_eq!(station_settings.raw("voice_pending"), Some(serde_json::json!({ "modelId": "station-voice" })));

        // A move carries the mode in force: the station starts in it.
        import(&station, &station_secrets, &full, true).unwrap();
        assert_eq!(station_settings.get(&MODE, None), Some(ModeSetting { mode: PanelMode::Own }));
        assert_eq!(station_settings.raw("voice_pending"), Some(serde_json::json!({ "modelId": "station-voice" })));
    }

    /// Review G1, round four: a station's gateway of an earlier build removes each setting a package lacks, and the
    /// routine package leaves out the mode — so it says schema 2, and such a station refuses it and keeps its mode and
    /// its waiting voice. As schema 1 it would have taken both: the station started in .env's mode, «Яндекс». A whole
    /// package still says schema 1 and goes to an earlier build, the mode in it.
    #[test]
    fn an_earlier_station_refuses_the_routine_package_and_keeps_its_mode() {
        let (pc, pc_secrets, pc_settings) = gateway();
        pc_settings.set(&MODE, &ModeSetting { mode: PanelMode::Own }, None).unwrap();
        pc_settings.set(&KEEP_REQUESTS, &7, None).unwrap();
        let whole = export(&pc, &pc_secrets, "pc").unwrap();
        let sent = routine(whole.clone());
        assert_eq!((SCHEMA, sent.schema, whole.schema), (2, 2, WHOLE), "the routine package needs schema 2");

        let station = BTreeMap::from([
            ("gateway_mode".to_string(), json!({ "mode": "relay" })),
            ("voice_pending".to_string(), json!({ "modelId": "station-voice" })),
        ]);
        let mut refused = station.clone();
        let error = earlier_import(&mut refused, &sent).unwrap_err();
        assert!(error.contains("сначала обновите шлюз"), "{error}");
        assert_eq!(refused, station, "nothing applied: its mode and its waiting voice stay");

        let mut erased = station.clone();
        earlier_import(&mut erased, &Bundle { schema: 1, ..sent }).unwrap();
        assert!(!erased.contains_key("gateway_mode") && !erased.contains_key("voice_pending"), "what schema 2 spares");

        // A move, the settings coming back, a copy restored: whole, and an earlier build still takes them.
        let mut moved = station.clone();
        earlier_import(&mut moved, &whole).unwrap();
        assert_eq!((&moved["gateway_mode"], &moved["requests_keep_days"]), (&json!({ "mode": "own" }), &json!(7)));
    }

    /// Review G1, round five: the computer's settings «на ПК» reach a station's gateway of an earlier build as the
    /// whole package with the station's own mode and waiting voice in it, as its own package says them — its import
    /// (schema 1) leaves it both and takes the rest. A station without them gets none of the computer's in their place.
    #[test]
    fn the_whole_package_for_an_earlier_station_carries_its_own_mode_and_waiting_voice() {
        let (pc, pc_secrets, pc_settings) = gateway();
        pc_settings.set(&MODE, &ModeSetting { mode: PanelMode::Own }, None).unwrap();
        pc_settings.set(&KEEP_REQUESTS, &7, None).unwrap();
        waiting(&pc, "pc-voice");
        let whole = export(&pc, &pc_secrets, "pc").unwrap();
        let (station, station_secrets, station_settings) = gateway();
        station_settings.set(&MODE, &ModeSetting { mode: PanelMode::Relay }, None).unwrap();
        station_settings.set(&KEEP_REQUESTS, &3, None).unwrap();
        // Its package as an earlier build writes it: the waiting voice travelled then.
        let mut theirs = export(&station, &station_secrets, "station").unwrap();
        theirs.settings.insert("voice_pending".into(), json!({ "modelId": "station-voice" }));

        let sent = for_earlier(whole.clone(), &theirs);
        assert_eq!(sent.schema, WHOLE);
        assert_eq!(sent.settings["gateway_mode"], json!({ "mode": "relay" }), "its own mode, not the computer's");
        assert_eq!(sent.settings["voice_pending"], json!({ "modelId": "station-voice" }));
        assert_eq!(sent.settings["requests_keep_days"], json!(7), "the computer's settings");
        let mut stored = theirs.settings.clone();
        earlier_import(&mut stored, &sent).unwrap();
        assert_eq!(stored["gateway_mode"], json!({ "mode": "relay" }));
        assert_eq!(stored["voice_pending"], json!({ "modelId": "station-voice" }));
        assert_eq!(stored["requests_keep_days"], json!(7));

        // Nothing stored there: nothing of the computer's in their place, and its import has nothing to remove.
        let bare = Bundle { settings: BTreeMap::new(), ..theirs };
        let sent = for_earlier(whole, &bare);
        assert!(!sent.settings.contains_key("gateway_mode") && !sent.settings.contains_key("voice_pending"));
        assert_eq!(sent.settings["requests_keep_days"], json!(7));
    }

    /// An earlier computer's sending to a station of this build: its whole package as schema 1, its mode and its
    /// waiting voice in it (neither was set apart then). This build takes it: the mode is the computer's, as it always
    /// was from such a computer; the waiting voice stays the station's own.
    #[test]
    fn this_build_still_takes_a_schema_1_package_from_an_earlier_computer() {
        let (pc, pc_secrets, pc_settings) = gateway();
        pc_settings.set(&MODE, &ModeSetting { mode: PanelMode::Gateway }, None).unwrap();
        pc_settings.set(&KEEP_REQUESTS, &5, None).unwrap();
        let mut earlier = export(&pc, &pc_secrets, "pc").unwrap();
        earlier.schema = 1;
        earlier.settings.insert("voice_pending".into(), json!({ "modelId": "pc-voice" }));
        let (station, station_secrets, station_settings) = gateway();
        station_settings.set(&MODE, &ModeSetting { mode: PanelMode::Relay }, None).unwrap();
        waiting(&station, "station-voice");

        import(&station, &station_secrets, &earlier, true).unwrap();
        assert_eq!(station_settings.get(&MODE, None), Some(ModeSetting { mode: PanelMode::Gateway }));
        assert_eq!(station_settings.get(&KEEP_REQUESTS, None), Some(5));
        assert_eq!(station_settings.raw("voice_pending"), Some(json!({ "modelId": "station-voice" })));
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

    /// A package's own switch and data for one plugin, as `plugins` reads them.
    fn plugin(db: &Db, id: &str, enabled: bool, kv: &[(&str, &str)]) {
        crate::plugins::registry::set_enabled(db, id, enabled, None, None).unwrap();
        for (key, value) in kv {
            db.with(|c| {
                c.execute(
                    "INSERT INTO plugin_kv (plugin_id, key, value, updated_at) VALUES (?1, ?2, ?3, 'then')
                     ON CONFLICT (plugin_id, key) DO UPDATE SET value = excluded.value",
                    params![id, key, value],
                )
            })
            .unwrap();
        }
    }

    fn state_of(db: &Db, id: &str) -> (Option<bool>, Vec<(String, String)>) {
        let enabled = db
            .with(|c| {
                c.query_row("SELECT enabled FROM packages WHERE id = ?1", params![id], |row| row.get::<_, i64>(0))
                    .optional()
            })
            .unwrap()
            .map(|enabled| enabled != 0);
        let kv = db
            .with(|c| {
                c.prepare("SELECT key, value FROM plugin_kv WHERE plugin_id = ?1 ORDER BY key")?
                    .query_map(params![id], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect()
            })
            .unwrap();
        (enabled, kv)
    }

    /// Live, 10.11: after the move «Список покупок» was off on the station and its list stayed on the computer. A move
    /// carries each plugin's switch and data both ways for the packages both sides have — never a package only one side
    /// has, never code; the routine sending, a copy, its restore carry none of it.
    #[test]
    fn a_move_carries_the_plugins_switches_and_data_both_ways_for_the_packages_both_sides_have() {
        let (pc, pc_secrets, _) = gateway();
        plugin(&pc, "shopping", true, &[("list", r#"["молоко"]"#)]);
        plugin(&pc, "weather", true, &[]);
        plugin(&pc, "pc-only", true, &[("x", "1")]);
        let (station, station_secrets, _) = gateway();
        plugin(&station, "shopping", false, &[("list", r#"["старое"]"#)]);
        plugin(&station, "weather", false, &[("city", r#""Казань""#)]);
        plugin(&station, "station-only", true, &[("y", "2")]);

        // The routine sending carries nothing of the plugins.
        let routine_package = routine(export(&pc, &pc_secrets, "pc").unwrap());
        assert!(routine_package.plugins.is_none());
        import(&station, &station_secrets, &routine_package, true).unwrap();
        assert_eq!(state_of(&station, "shopping").0, Some(false));

        // To the station: the computer's switches and data for what both have.
        let mut moved = export(&pc, &pc_secrets, "pc").unwrap();
        moved.plugins = Some(plugins(&pc).unwrap());
        let imported = import(&station, &station_secrets, &moved, true).unwrap();
        assert_eq!(imported.plugins, 2, "shopping and weather");
        assert_eq!(state_of(&station, "shopping"), (Some(true), vec![("list".into(), r#"["молоко"]"#.into())]));
        assert_eq!(state_of(&station, "weather"), (Some(true), vec![]), "the computer's data, none");
        assert_eq!(state_of(&station, "pc-only"), (None, vec![]), "not on the station: passed by");
        assert_eq!(state_of(&station, "station-only"), (Some(true), vec![("y".into(), "2".into())]), "untouched");

        // The list grows on the station; back to the computer, the station's.
        plugin(&station, "shopping", true, &[("list", r#"["молоко","хлеб"]"#)]);
        let mut back = export(&station, &station_secrets, "station").unwrap();
        back.plugins = Some(plugins(&station).unwrap());
        import(&pc, &pc_secrets, &back, false).unwrap();
        assert_eq!(state_of(&pc, "shopping"), (Some(true), vec![("list".into(), r#"["молоко","хлеб"]"#.into())]));
        assert_eq!(state_of(&pc, "station-only"), (None, vec![]));
        assert_eq!(state_of(&pc, "pc-only"), (Some(true), vec![("x".into(), "1".into())]), "untouched");

        // A station's first start: no rows yet — a package built into it counts as there, and keeps its kind.
        let (fresh, fresh_secrets, _) = gateway();
        import(&fresh, &fresh_secrets, &moved, true).unwrap();
        assert_eq!(state_of(&fresh, "shopping"), (Some(true), vec![("list".into(), r#"["молоко"]"#.into())]));
        let source: String = fresh
            .with(|c| c.query_row("SELECT source FROM packages WHERE id = 'shopping'", [], |row| row.get(0)))
            .unwrap();
        assert_eq!(source, crate::plugins::registry::SOURCE_BUNDLED);
        assert_eq!(state_of(&fresh, "pc-only").0, None);
    }

    /// The choice of voice and the station's own name are each side's own; only a move to the station sets them there.
    #[test]
    fn a_move_sets_the_computers_voice_and_the_stations_name_and_nothing_else_carries_them() {
        let (pc, pc_secrets, pc_settings) = gateway();
        pc_settings.set(&crate::settings::definitions::VOICE_PROVIDER, &"mixed".to_string(), None).unwrap();
        let whole = export(&pc, &pc_secrets, "pc").unwrap();
        assert!(!whole.settings.contains_key("tts_provider") && !whole.settings.contains_key("station_self"));
        let (station, station_secrets, station_settings) = gateway();
        station_settings.set(&crate::settings::definitions::VOICE_PROVIDER, &"yandex".to_string(), None).unwrap();

        let mut moved = whole.clone();
        moved.settings.insert("tts_provider".into(), json!("mixed"));
        moved.settings.insert("station_self".into(), json!({ "device": "h:nursery", "name": "Детская" }));
        import(&station, &station_secrets, &moved, true).unwrap();
        assert_eq!(station_settings.raw("tts_provider"), Some(json!("mixed")));
        let own = station_settings.get(&crate::settings::definitions::STATION_SELF, None).unwrap();
        assert_eq!((own.device.as_deref(), own.name.as_deref()), (Some("h:nursery"), Some("Детская")));

        // The routine sending keeps them; the station's own package never carries them back.
        import(&station, &station_secrets, &routine(whole), true).unwrap();
        assert_eq!(station_settings.raw("tts_provider"), Some(json!("mixed")));
        assert!(station_settings.raw("station_self").is_some());
        let back = export(&station, &station_secrets, "station").unwrap();
        assert!(!back.settings.contains_key("tts_provider") && !back.settings.contains_key("station_self"));
    }

    fn key(db: &Db, id: &str, revoked_at: Option<&str>) {
        db.with(|c| {
            c.execute(
                "INSERT INTO api_keys (id, name, secret_hash, grants, created_at, revoked_at) VALUES (?1, 'Колонка: домашний сервер', 'h', '[\"home.models\"]', 'then', ?2)",
                params![id, revoked_at],
            )
        })
        .unwrap();
    }

    fn revoked_at(db: &Db, id: &str) -> Option<String> {
        db.with(|c| c.query_row("SELECT revoked_at FROM api_keys WHERE id = ?1", params![id], |row| row.get(0)))
            .unwrap()
    }

    /// Live, 08.11 and 10.11: the move back revoked the station's key to its home server, then the station's settings
    /// were applied — their `api_keys` brought it back to life. A key revoked here stays revoked.
    #[test]
    fn a_key_revoked_here_stays_revoked_whatever_the_package_says() {
        let (station, station_secrets, _) = gateway();
        key(&station, "1d8ae8a8a2c8", None);
        key(&station, "other", None);
        let theirs = export(&station, &station_secrets, "station").unwrap();
        let (pc, pc_secrets, _) = gateway();
        key(&pc, "1d8ae8a8a2c8", Some("2026-11-10T18:00:00.000Z"));
        import(&pc, &pc_secrets, &theirs, false).unwrap();
        assert_eq!(revoked_at(&pc, "1d8ae8a8a2c8").as_deref(), Some("2026-11-10T18:00:00.000Z"));
        assert_eq!(revoked_at(&pc, "other"), None, "the others as the package says");
    }

    fn provider(db: &Db, id: &str, kind: &str, title: &str, url: &str, created_at: &str) {
        db.with(|c| {
            c.execute(
                "INSERT INTO llm_providers (id, preset, kind, title, base_url, created_at, updated_at) VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?5)",
                params![id, kind, title, url, created_at],
            )
        })
        .unwrap();
    }

    /// Live, 10.11: the move back brought the provider the station made for its home server — «Модель на компьютере»
    /// at the computer's own /home/v1/llm — to the computer, where it points at itself. Known by its address: not
    /// taken, nor its key; a choice that answered with it answers with the computer's own model.
    #[test]
    fn the_computer_does_not_take_the_provider_the_station_made_for_its_home_server() {
        let (station, station_secrets, station_settings) = gateway();
        provider(&station, "lmstudio", "openai", "LM Studio", "http://127.0.0.1:1234/v1", "2026-11-01T00:00:00Z");
        provider(&station, "claude", "anthropic", "Claude", "https://api.anthropic.com", "2026-11-02T00:00:00Z");
        provider(
            &station,
            "custom-openai",
            "openai",
            "Свой сервер",
            "http://192.168.1.20:7780/home/v1/llm/",
            "2026-11-10T00:00:00Z",
        );
        station_secrets.set("llm:custom-openai:key", Some("the-pair-home-key")).unwrap();
        station_secrets.set("llm:claude:key", Some("sk-not-a-real-key")).unwrap();
        let brain = json!({ "fast": { "provider": "custom-openai", "model": "qwen" }, "smart": { "provider": "claude", "model": "sonnet" } });
        station.with(|c| {
            c.execute(
                "INSERT INTO settings (key, value, is_default, updated_at) VALUES ('brain', ?1, 0, 'now'), ('workshop.agent', ?2, 0, 'now')",
                params![brain.to_string(), json!({ "provider": "custom-openai", "model": "qwen" }).to_string()],
            )
        })
        .unwrap();
        let theirs = export(&station, &station_secrets, "station").unwrap();

        let (pc, pc_secrets, pc_settings) = gateway();
        pc_secrets.set("llm:custom-openai:key", Some("stale")).unwrap();
        let imported = import(&pc, &pc_secrets, &theirs, false).unwrap();
        assert_eq!((imported.rows["llm_providers"], imported.skipped), (2, 1));
        let ids: Vec<String> = pc
            .with(|c| c.prepare("SELECT id FROM llm_providers ORDER BY id")?.query_map([], |row| row.get(0))?.collect())
            .unwrap();
        assert_eq!(ids, vec!["claude", "lmstudio"]);
        assert_eq!(pc_secrets.get("llm:custom-openai:key"), None, "its key neither");
        assert_eq!(pc_secrets.get("llm:claude:key").as_deref(), Some("sk-not-a-real-key"));
        assert_eq!(
            pc_settings.raw("brain"),
            Some(
                json!({ "fast": { "provider": "lmstudio", "model": "qwen" }, "smart": { "provider": "claude", "model": "sonnet" } })
            )
        );
        assert_eq!(pc_settings.raw("workshop.agent"), Some(json!({ "provider": "lmstudio", "model": "qwen" })));

        // On the station it is its own: the station's package to a station keeps it.
        let (other, other_secrets, _) = gateway();
        import(&other, &other_secrets, &theirs, true).unwrap();
        assert_eq!(other_secrets.get("llm:custom-openai:key").as_deref(), Some("the-pair-home-key"));
        let _ = station_settings;
    }

    /// Live, 10.11: the copy before a package was 4 KB — under WAL the main file alone holds little of the database.
    /// The copy is the whole database now; when it cannot be made, the package is not applied and stays as `.failed`.
    #[test]
    fn the_whole_database_is_copied_before_a_package_or_the_package_waits() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(Some(dir.path())).unwrap();
        let secrets = SecretsService::new(Arc::new(Db::memory()), None).unwrap();
        person(&db, "u1", "Мама");
        let wal = dir.path().join(format!("{}-wal", crate::storage::db::DB_FILE));
        assert!(std::fs::metadata(&wal).map(|wal| wal.len() > 0).unwrap_or(false), "the rows are in the log");
        let (pc, pc_secrets, _) = gateway();
        person(&pc, "u2", "Папа");
        std::fs::write(
            incoming_file(dir.path()),
            serde_json::to_vec(&export(&pc, &pc_secrets, "pc").unwrap()).unwrap(),
        )
        .unwrap();
        assert!(apply_incoming(dir.path(), &db, &secrets, true).is_some());
        let copy = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.to_string_lossy().contains(".before-settings-"))
            .expect("a copy beside the database");
        let names: Vec<String> = rusqlite::Connection::open(&copy)
            .unwrap()
            .prepare("SELECT name FROM users")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(names, vec!["Мама"], "the database as it was, the log's rows in it");
        // What the copy of the main file alone held: not the log's rows.
        let main_only = dir.path().join("main-only.sqlite");
        std::fs::copy(dir.path().join(crate::storage::db::DB_FILE), &main_only).unwrap();
        let held: rusqlite::Result<i64> =
            rusqlite::Connection::open(&main_only)
                .unwrap()
                .query_row("SELECT count(*) FROM users", [], |row| row.get(0));
        assert!(held.map_or(true, |count| count == 0), "the main file alone lacks the log's rows");

        // The copy cannot be made (its folder gone): nothing applied, the package kept as .failed.
        let gone = tempfile::tempdir().unwrap();
        let held = Db::open(Some(gone.path())).unwrap();
        person(&held, "u1", "Мама");
        std::fs::remove_dir_all(gone.path()).unwrap();
        let waiting = tempfile::tempdir().unwrap();
        std::fs::write(
            incoming_file(waiting.path()),
            serde_json::to_vec(&export(&pc, &pc_secrets, "pc").unwrap()).unwrap(),
        )
        .unwrap();
        assert!(apply_incoming(waiting.path(), &held, &secrets, true).is_none());
        assert!(waiting.path().join("incoming-settings.json.failed").exists());
        assert!(!incoming_file(waiting.path()).exists());
        let names: Vec<String> =
            held.with(|c| c.prepare("SELECT name FROM users")?.query_map([], |row| row.get(0))?.collect()).unwrap();
        assert_eq!(names, vec!["Мама"], "not applied");
    }

    /// A move back while the settings live on the computer leaves the station's plugins' state alone for the start.
    #[test]
    fn the_plugins_state_a_move_back_left_is_applied_at_the_start_once() {
        let dir = tempfile::tempdir().unwrap();
        let (pc, pc_secrets, _) = gateway();
        plugin(&pc, "shopping", false, &[]);
        let (station, _, _) = gateway();
        plugin(&station, "shopping", true, &[("list", r#"["хлеб"]"#)]);
        let package = PluginsPackage { from: "station".into(), at: now_iso(), plugins: plugins(&station).unwrap() };
        std::fs::write(incoming_plugins_file(dir.path()), serde_json::to_vec(&package).unwrap()).unwrap();
        assert!(apply_incoming(dir.path(), &pc, &pc_secrets, false).is_none(), "no settings came");
        assert_eq!(state_of(&pc, "shopping"), (Some(true), vec![("list".into(), r#"["хлеб"]"#.into())]));
        assert!(!incoming_plugins_file(dir.path()).exists());
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
