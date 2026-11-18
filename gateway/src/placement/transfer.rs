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

use crate::dialog::brain::presets::{no_address, HELPER_PRESET};
use crate::helpers::{parse_helpers, HelperSetting, HOME_COMPUTER};
use crate::secrets::SecretsService;
use crate::settings::definitions::{GATEWAY_SELF, HELPERS};
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
///
/// 3 (0.6.0): the plugins' private settings travel only sealed among the secrets (`plugin:<package>:<key>`), no longer
/// among their settings. An import of schema 2 (0.5.x) takes the plugins' settings without them and passes by the
/// secrets it does not know: a calendar's key would be gone from a station still on 0.5 while its computer is on 0.6,
/// and would not come back with its update. Such a station gets the routine package with the keys put back among the
/// settings as it reads them (`keys_in_settings`), schema 2; its first start on 0.6 moves them into its safe.
///
/// 4 (block 1 of «Дом» и помощники, docs/plans/home-and-helpers.md): the home and its helpers travel — the settings and
/// the secrets' prefixes of `SINCE`; each gateway's own id and name (`gateway_self`) stay, an earlier import too
/// (`LOCAL_SINCE`). A gateway of an earlier schema gets the package without them, at its own schema (`for_schema`):
/// its import knows nothing of them, and a change of them alone sends it nothing (src/placement/watch.rs). A package
/// from such a gateway leaves this one's as they are (`Bundle::carries`). The apps' own settings (`app_settings`, 0.13.0)
/// came without a schema of their own — an earlier import passes the table by —, so 4 is the next after both.
pub const SCHEMA: u32 = 4;

/// The schema before the home (0.6.0 to 0.13): the computer's routine sending to such a station is cut to it
/// (`for_schema`).
pub const BEFORE_HOME_SCHEMA: u32 = 3;

/// The schema whose import knows a «Мозг» row of a helper's model that stores no address (the hidden preset `helper`,
/// dialog/brain/brain_service.rs): a linked helper's, asked only through this start's bridge to its link, or one kept
/// while a choice answers with it after its helper went. An earlier import reads such a row as an OpenAI provider with
/// no address of its own — OpenAI's, where a chosen question would go without a key —, so a package for it leaves the
/// row out (`for_schema`).
const ADDRESSLESS_ROWS_SCHEMA: u32 = 4;

/// What each schema added to the package: its settings, and its secrets by their prefixes. A package for a gateway
/// whose import knows an earlier schema leaves them out (`for_schema`), and an import keeps this gateway's own when the
/// package does not carry that schema (`Bundle::carries`): its writer did not know them, and its package would remove
/// them as settings it lacks. 4 brings the home (`home_identity`, the key `home:key`, src/core/identity/home.rs) and the
/// helpers (`helpers`, `helper_pins`, `away`, their keys `helper:<id>:key`), the whole block in one schema.
pub const SINCE: &[(u32, &[&str], &[&str])] =
    &[(4, &["home_identity", "helpers", "helper_pins", "away"], &["home:", "helper:"])];

/// The routine package for a station whose import knows schema 2 (0.5.x): the same, the plugins' keys among their
/// settings too (`keys_in_settings`).
pub const KEYS_IN_SETTINGS_SCHEMA: u32 = 2;

/// What a gateway's route answers a package not for it — another format, or a schema newer than its import knows
/// (routes.rs, `import`): the same words in every build since the route came, so the computer knows an earlier
/// station's refusal by them (src/placement/watch.rs).
pub const NOT_FOR_THIS_GATEWAY: &str = "Набор настроек не для этого шлюза: обновите его.";

/// The schema of a whole package (`export`): every setting that travels, the mode among them when one is stored. An
/// import of schema 1 applies it as it applied one of its own build, so a whole package still goes to an earlier
/// build: a move to a station whose build is older than this computer's (a file, the release of this version number),
/// the station's settings back to an earlier computer, a kept copy to an earlier station. Raised when a whole package
/// needs more of its import than schema 1.
pub const WHOLE: u32 = 1;

/// The tables that travel whole, in the order they are written (people before their own settings). `app_settings` —
/// but for its keys each side keeps for itself (`LOCAL_APP_SETTINGS`). A build before it passes the table by.
pub const TABLES: &[&str] =
    &["users", "user_settings", "api_keys", "characters", "llm_providers", "mcp_servers", "stations", "app_settings"];

/// Of the apps' own settings (`app_settings`), the keys each side keeps for itself: what «Выход в интернет» learned
/// of this home's network — a site that did not open directly from here (src/egress/rules.rs).
pub const LOCAL_APP_SETTINGS: &[&str] = &["egress.learned"];

/// A row each side keeps for itself.
fn local_row(table: &str, row: &Map<String, Value>) -> bool {
    table == "app_settings"
        && row.get("key").and_then(Value::as_str).is_some_and(|key| LOCAL_APP_SETTINGS.contains(&key))
}

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
    // The station's cloud hearing and voice (src/speech/cloud_speech.rs): the station's own.
    "cloud_speech",
    // The station's own name and device as the computer knows them: a move brings them (`MOVE_SETS`).
    "station_self",
    // The apps' history: where each gateway keeps its own and how much (plugins/history/gc.rs).
    "package_history",
    // «Как говорить с DuoWave»: the computer's choice of its own microphone (src/talk/input.rs).
    "voice_input",
    // This gateway itself, its id and name (src/core/identity/home.rs): the home travels, each gateway stays itself.
    "gateway_self",
    // Who this computer helps as a helper, and its address from outside (docs/plans/home-and-helpers.md, stage 7):
    // the machine's own, like its link's secrets under `helping:`.
    "helper_clients",
    "helper_outside",
    // The work folders and the agents (src/works): their paths and the way Claude Code pays are this computer's.
    "work_folders",
    "harness_claude_path",
    "harness_claude_auth",
    "harness_paths",
    // The home's copy (src/home/copy.rs): who holds one, at the centre; this holder's own marks.
    "home_copies",
    "home_copy",
    // An ear's centre (src/home/ear.rs): each device's own.
    "home_ear",
    // The model a Турбо's centre named for a character's voice (src/home_server/voice_plan.rs): the Турбо's own.
    "turbo_voice_engines",
];

/// Of each side's own settings (`LOCAL_SETTINGS`), what a schema brought: an import of an earlier schema does not know
/// it as its own and removes it as a setting the package lacks. So what the computer sends a gateway of such a schema
/// carries its own as its own package says it (`their_own`, src/placement/watch.rs): a station put back on 0.11 by the
/// safety net keeps its id and the name a person gave it. 4 brings the gateway itself (`gateway_self`) and what it
/// keeps as a helper (`helper_clients`, `helper_outside`).
pub const LOCAL_SINCE: &[(u32, &[&str])] = &[(4, &["gateway_self", "helper_clients", "helper_outside"])];

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

/// The secrets that travel: the model providers', the connectors', the MCP servers', the voices' (ElevenLabs,
/// `voice:<provider>:key`), the plugins' private settings
/// (`plugin:<package>:<key>`, their settings travel too), the home's key and the helpers' (schema 4, `SINCE`). Like
/// every secret of the package they go sealed with the pair's key, and so do the copies kept on the computer.
/// Everything else (the pair's key, the home server's, the panel's sessions, a sign-in's tokens under `oauth:`) is each
/// side's own, and so is what `STAYING_SECRETS` keeps out of these.
pub const TRAVELLING_SECRETS: &[&str] =
    &["llm:", "connector:", "mcp:", "voice:", crate::secrets::PLUGIN_SECRETS, "home:", "helper:"];

/// Of the travelling prefixes, what stays all the same: the key of the pair's own helper entry, `home-computer` (each
/// gateway's own door to its computer, `OWN_PARTS`). A helper's own secrets — its connection key, its certificate, its
/// clients' keys (docs/plans/home-and-helpers.md, stage 7) — live under `helping:`, which is no travelling prefix at
/// all: one slip in a rule here must never ship them.
pub const STAYING_SECRETS: &[&str] = &["helper:home-computer:"];

/// Shared settings with one part that is each gateway's own: (setting, part id). The pair's door — the helper entry
/// `home-computer`, the old «Домашний сервер» (src/helpers/legacy.rs) — and its certificate's pin are the station's
/// own, as `connector_home_server` was: `export` leaves the part out, and an import keeps the stored one at its place
/// whatever the package says (`with_own`). The computer's own list never carries the computer itself.
pub const OWN_PARTS: &[(&str, &str)] = &[("helpers", "home-computer"), ("helper_pins", "home-computer")];

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
    /// The newest schema whose settings this package carries in full (`SINCE`): its writer's, or less when it was cut
    /// for a gateway of an earlier schema (`for_schema`). 0 — a writer from before schema 4, whose JSON has none: an
    /// import then keeps its own of what came since.
    #[serde(default)]
    pub carries: u32,
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
    /// The plugins' pictures (plugins/files.rs): only a move carries them, with the plugins' state — to the station as
    /// many as fit its 5 MB, the newest first (the owner's decision 11); the rest stay, and a plugin downloads again
    /// what it misses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pictures: Option<Vec<Picture>>,
    /// The computer's own sources and installed characters (carry.rs, item 43): only a move to the station carries
    /// them, and the station's gateway installs them at its first start (`take_carried`); a build before it passes
    /// them by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carried: Option<super::carry::Carried>,
}

/// One plugin's picture on its way: its bytes in base64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Picture {
    pub plugin: String,
    pub name: String,
    pub data: String,
    pub written: i64,
}

/// The plugins' pictures here, the newest first, as many as fit `budget` bytes (none: all).
pub fn pictures(root: &Path, budget: Option<u64>) -> Vec<Picture> {
    let mut found: Vec<(String, String, PathBuf, u64, i64)> = Vec::new();
    for plugin in std::fs::read_dir(root).into_iter().flatten().flatten().filter(|entry| entry.path().is_dir()) {
        let id = plugin.file_name().to_string_lossy().into_owned();
        for file in std::fs::read_dir(plugin.path()).into_iter().flatten().flatten() {
            let Ok(meta) = file.metadata() else { continue };
            let name = file.file_name().to_string_lossy().into_owned();
            if !meta.is_file() || name.starts_with('.') {
                continue;
            }
            let written = meta
                .modified()
                .ok()
                .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |at| at.as_millis() as i64);
            found.push((id.clone(), name, file.path(), meta.len(), written));
        }
    }
    found.sort_by_key(|picture| std::cmp::Reverse(picture.4));
    let mut taken = 0u64;
    found
        .into_iter()
        .filter(|(_, _, _, size, _)| match budget {
            Some(budget) if taken + size > budget => false,
            _ => {
                taken += size;
                true
            }
        })
        .filter_map(|(plugin, name, path, _, written)| {
            Some(Picture { plugin, name, data: B64.encode(std::fs::read(path).ok()?), written })
        })
        .collect()
}

/// The pictures that came with a move, for the plugins this side has a state for too: each written whole, a name or
/// an id that is not one of ours passed by.
pub fn keep_pictures(root: &Path, pictures: &[Picture], plugins: &BTreeMap<String, PluginState>) -> usize {
    let mut kept = 0;
    for picture in pictures.iter().filter(|picture| plugins.contains_key(&picture.plugin)) {
        let safe = crate::plugins::manifest::valid_id(&picture.plugin)
            && !picture.name.starts_with('.')
            && picture
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'));
        let Ok(bytes) = B64.decode(&picture.data) else { continue };
        if !safe || crate::plugins::files::picture_kind(&bytes).is_none() {
            continue;
        }
        let folder = root.join(&picture.plugin);
        if std::fs::create_dir_all(&folder).is_ok() && std::fs::write(folder.join(&picture.name), bytes).is_ok() {
            kept += 1;
        }
    }
    kept
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
    /// Its schedule here (plugins/schedule.rs): the rhythms' pauses and runs, the tasks its code set. A build before it
    /// passes it by.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<Value>,
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

/// `value` without its part `id`: the array's item with that `id`, or the object's key. None — nothing left.
fn without_own(value: &Value, id: &str) -> Option<Value> {
    match value {
        Value::Array(items) => {
            let kept: Vec<Value> =
                items.iter().filter(|item| item.get("id").and_then(Value::as_str) != Some(id)).cloned().collect();
            (!kept.is_empty()).then_some(Value::Array(kept))
        }
        Value::Object(fields) => {
            let mut kept = fields.clone();
            kept.remove(id);
            (!kept.is_empty()).then_some(Value::Object(kept))
        }
        other => Some(other.clone()),
    }
}

/// What an import stores of a setting with an own part (`OWN_PARTS`): the package's value without any part `id`, and
/// the stored part put back where it stood — at its index (clamped), or under its key. None — nothing left: the
/// stored setting goes.
fn with_own(incoming: Option<&Value>, stored: Option<&Value>, id: &str) -> Option<Value> {
    let incoming = incoming.and_then(|value| without_own(value, id));
    match stored {
        Some(Value::Array(items)) => {
            let Some(at) = items.iter().position(|item| item.get("id").and_then(Value::as_str) == Some(id)) else {
                return incoming;
            };
            let mut list = match incoming {
                Some(Value::Array(list)) => list,
                _ => Vec::new(),
            };
            list.insert(at.min(list.len()), items[at].clone());
            Some(Value::Array(list))
        }
        Some(Value::Object(fields)) => {
            let Some(own) = fields.get(id) else { return incoming };
            let mut object = match incoming {
                Some(Value::Object(object)) => object,
                _ => Map::new(),
            };
            object.insert(id.to_string(), own.clone());
            Some(Value::Object(object))
        }
        _ => incoming,
    }
}

fn travels(secret: &str) -> bool {
    TRAVELLING_SECRETS.iter().any(|prefix| secret.starts_with(prefix))
        && !STAYING_SECRETS.iter().any(|prefix| secret.starts_with(prefix))
}

/// The schema that brought this setting into the package (`SINCE`); 0 — it travelled before.
fn setting_since(key: &str) -> u32 {
    SINCE.iter().find(|(_, settings, _)| settings.contains(&key)).map_or(0, |(schema, _, _)| *schema)
}

/// The schema that brought this secret into the package (`SINCE`, by its prefix); 0 — it travelled before.
fn secret_since(id: &str) -> u32 {
    SINCE
        .iter()
        .find(|(_, _, prefixes)| prefixes.iter().any(|prefix| id.starts_with(prefix)))
        .map_or(0, |(schema, _, _)| *schema)
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
                .collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .filter(|row| !local_row(table, row))
                .collect();
            tables.insert(table.to_string(), rows);
        }
        let mut statement = connection.prepare("SELECT key, value FROM settings ORDER BY key")?;
        let mut settings: BTreeMap<String, Value> = statement
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|(key, _)| !LOCAL_SETTINGS.contains(&key.as_str()))
            .filter_map(|(key, text)| serde_json::from_str(&text).ok().map(|value| (key, value)))
            .collect();
        // Each gateway's own part of a shared setting stays here; a setting it alone made is left out.
        for (key, id) in OWN_PARTS {
            if let Some(value) = settings.remove(*key) {
                if let Some(kept) = without_own(&value, id) {
                    settings.insert(key.to_string(), kept);
                }
            }
        }
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
        carries: SCHEMA,
        tables,
        settings,
        secrets,
        plugins: None,
        pictures: None,
        carried: None,
    })
}

/// The plugins' schedules in the `state` table (plugins/schedule.rs): they travel with a move as the plugins' data.
const SCHEDULES: &str = crate::plugins::schedule::STORE;

/// The plugins' state here (`PluginState`), for a move: each package with a row of its own — every built-in has one,
/// and so has every package installed or switched on once — and its data.
pub fn plugins(db: &Db) -> rusqlite::Result<BTreeMap<String, PluginState>> {
    db.with(|connection| {
        let mut found: BTreeMap<String, PluginState> = connection
            .prepare("SELECT id, enabled, updated_at FROM packages ORDER BY id")?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    PluginState {
                        enabled: row.get::<_, i64>(1)? != 0,
                        at: row.get(2)?,
                        kv: Vec::new(),
                        jobs: Vec::new(),
                    },
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
        let mut statement = connection.prepare("SELECT device, items FROM state WHERE kind = ?1")?;
        let jobs =
            statement.query_map(params![SCHEDULES], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        for row in jobs {
            let (id, items) = row?;
            if let (Some(state), Ok(items)) = (found.get_mut(&id), serde_json::from_str::<Vec<Value>>(&items)) {
                state.jobs = items;
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
        connection.execute("DELETE FROM state WHERE kind = ?1 AND device = ?2", params![SCHEDULES, id])?;
        if !state.jobs.is_empty() {
            connection.execute(
                "INSERT INTO state (kind, device, items, updated_at) VALUES (?1, ?2, ?3, ?4)",
                params![SCHEDULES, id, Value::Array(state.jobs.clone()).to_string(), now_iso()],
            )?;
        }
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

/// A helper's model as a gateway's «Мозг» has it: a row of the hidden preset `helper`, which «Мозг» makes itself for
/// each helper of its own list (dialog/brain/brain_service.rs), or a provider at a home server's model route
/// (`…/home/v1/llm`, home_server/serve.rs) — a station's old «Модель на компьютере». Known by its preset or its
/// address, not its title: never a computer's own model.
pub fn at_home_server(row: &Map<String, Value>) -> bool {
    row.get("preset").and_then(Value::as_str) == Some(HELPER_PRESET)
        || row.get("base_url").and_then(Value::as_str).is_some_and(at_model_route)
}

/// What a computer leaves out of a station's package: the station's model of this computer, which here would point at
/// the computer itself (live, 10.11: «Модель на компьютере» came back with the move) — the station's row for its door
/// `home-computer` or for an entry that names this computer (`helperId`, `me`), or its old «Модель на компьютере» (a
/// provider at a model route that is no helper's row; on a station before the helpers it was always the pair's
/// computer). The other helpers' rows stay: their helpers come with the package, and this computer asks them as they
/// are.
fn of_this_computer(row: &Map<String, Value>, helpers: &[HelperSetting], me: Option<&str>) -> bool {
    if row.get("preset").and_then(Value::as_str) != Some(HELPER_PRESET) {
        return row.get("base_url").and_then(Value::as_str).is_some_and(at_model_route);
    }
    let options = match row.get("options") {
        Some(Value::String(text)) => serde_json::from_str::<Value>(text).ok(),
        other => other.cloned(),
    };
    // A helper's row that names no helper is asked nowhere: nothing to take.
    let Some(helper) = options.as_ref().and_then(|options| options.get("helper")).and_then(Value::as_str) else {
        return true;
    };
    helper == HOME_COMPUTER
        || helpers.iter().any(|entry| entry.id == helper && me.is_some() && entry.helper_id.as_deref() == me)
}

/// A helper's model row that stores no address (`ADDRESSLESS_ROWS_SCHEMA`) or only the stand-in for none
/// (`HELPER_NO_ADDRESS`: an earlier import would ask it nowhere, and a pick of it would answer nothing): it reaches its
/// helper only through this gateway's own bridge to a link, made at each start, or nowhere at all.
fn addressless(row: &Map<String, Value>) -> bool {
    row.get("preset").and_then(Value::as_str) == Some(HELPER_PRESET)
        && row.get("base_url").and_then(Value::as_str).is_none_or(|url| url.trim().is_empty() || no_address(url))
}

/// An address at a helper's model route.
fn at_model_route(url: &str) -> bool {
    url::Url::parse(url.trim()).is_ok_and(|url| url.path().trim_end_matches('/').ends_with(HOME_MODEL_ROUTE))
}

/// The home server's model route on the computer (home_server/serve.rs): a station's provider for it ends so.
const HOME_MODEL_ROUTE: &str = "/home/v1/llm";

/// The computer's own model among the providers a package brings: the one its home server hands the station's «Мозг»
/// (home_server/serve.rs, `own_model`) — the first OpenAI-compatible provider at home, by when it was added; never a
/// helper's model.
fn own_model(rows: &[&Map<String, Value>]) -> Option<String> {
    let mut models: Vec<(&str, usize, &str)> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| !at_home_server(row))
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

/// A choice of model that answered with a provider left out (`of_this_computer`; `leave_out_rows` for an earlier
/// import): with the computer's own model, which that provider reached through the home server, else none — «Мозг»
/// falls back to .env's, and has no backup. `brain` holds `fast`, `smart` and `backup`, `workshop.agent` one pick.
/// None: nothing of it stays.
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
                for profile in ["fast", "smart", "backup"] {
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
/// what only a move carries — the station keeps its own mode — and so of schema 2 at least (`SCHEMA`): a station of an
/// earlier build refuses it rather than lose its mode. Such a station gets `for_earlier` instead.
pub fn routine(mut bundle: Bundle) -> Bundle {
    bundle.settings.retain(|key, _| !MOVE_ONLY.contains(&key.as_str()));
    bundle.schema = SCHEMA;
    bundle
}

/// The plugins' private settings put back among their settings as well, as a build from before the safe kept them
/// reads them (schema 2 and earlier, `SCHEMA`): the package for such a station — the routine one at schema 2, the
/// whole one of a move or for an earlier build. The secrets stay in it too: a build of this one takes them from
/// there, and its start moves what it finds among the settings into its safe (plugins/controller.rs,
/// `keep_secrets_in_the_safe`), a value already in the safe winning. The package goes sealed, as the secrets do.
pub fn keys_in_settings(mut bundle: Bundle) -> Bundle {
    let key = crate::settings::definitions::PACKAGE_SETTINGS.key;
    let keys: Vec<(String, String, String)> = bundle
        .secrets
        .iter()
        .filter_map(|(id, value)| {
            let (package, setting) = id.strip_prefix(crate::secrets::PLUGIN_SECRETS)?.split_once(':')?;
            Some((package.to_string(), setting.to_string(), value.clone()))
        })
        .collect();
    if keys.is_empty() {
        return bundle;
    }
    let settings = bundle.settings.entry(key.to_string()).or_insert_with(|| Value::Object(Map::new()));
    if !settings.is_object() {
        *settings = Value::Object(Map::new());
    }
    if let Some(plugins) = settings.as_object_mut() {
        for (package, setting, value) in keys {
            let own = plugins.entry(package).or_insert_with(|| Value::Object(Map::new()));
            if let Some(own) = own.as_object_mut() {
                own.insert(setting, Value::String(value));
            }
        }
    }
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

/// The package for a gateway whose import knows `schema` (its /health's `build.settingsSchema`, the computer's watch):
/// what came after that schema left out (`SINCE`), and it asks no newer import than that. A station of 0.6 to 0.13
/// (schema 3) takes the computer's routine package as before, without the home and the helpers, and without a helper's
/// model row that stores no address (`ADDRESSLESS_ROWS_SCHEMA`, `leave_out_rows`) — the apps' own settings stay in it:
/// a 0.13 import takes them, an earlier one passes the table by; a station of this build gets it as it is.
pub fn for_schema(mut bundle: Bundle, schema: u32) -> Bundle {
    bundle.settings.retain(|key, _| setting_since(key) <= schema);
    bundle.secrets.retain(|id, _| secret_since(id) <= schema);
    if schema < ADDRESSLESS_ROWS_SCHEMA {
        leave_out_rows(&mut bundle, addressless);
    }
    bundle.schema = bundle.schema.min(schema);
    bundle.carries = bundle.carries.min(schema);
    bundle
}

/// «Мозг»'s rows that `out` names left out of a package, with their secrets (`llm:<id>:`), and every pick of them taken
/// again with no model in their place (`pick_again`): the receiver answers with the rest of its choice, else with its
/// .env's model, and has no backup from them. A pick `pick_again` does not know stays — the memory's own model, an
/// app's —: a 0.11 gateway's night waits for a model it does not find («Ждёт модель…»), and an app's question goes to
/// the profile's model; neither asks a row it has not.
fn leave_out_rows(bundle: &mut Bundle, out: impl Fn(&Map<String, Value>) -> bool) {
    let Some(rows) = bundle.tables.get_mut("llm_providers") else { return };
    let left_out: Vec<String> = rows
        .iter()
        .filter(|row| out(row))
        .filter_map(|row| row.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    if left_out.is_empty() {
        return;
    }
    rows.retain(|row| !out(row));
    bundle.secrets.retain(|id, _| !left_out.iter().any(|row| id.starts_with(&format!("llm:{row}:"))));
    bundle.settings = std::mem::take(&mut bundle.settings)
        .into_iter()
        .filter_map(|(key, value)| pick_again(&key, &value, &left_out, None).map(|value| (key, value)))
        .collect();
}

/// Whether an import of `schema` knows as its own all that this build keeps for each side (`LOCAL_SINCE`): a package
/// for it needs nothing of its own put in (`their_own`).
pub fn knows_own(schema: u32) -> bool {
    LOCAL_SINCE.iter().all(|(since, _)| *since <= schema)
}

/// The package for a gateway whose import knows `schema`, with what this build keeps for each side and that import
/// does not know as its own (`LOCAL_SINCE`) as the gateway's own package says it (`theirs`), left out when that has
/// none: its import takes it as an ordinary setting and so keeps it, and never gets the sender's own. A kept copy put
/// back gets it too: the gateway's own is never a copy's to change. The whole package for an earlier build carries it
/// already (`for_earlier`).
pub fn their_own(mut bundle: Bundle, theirs: &Bundle, schema: u32) -> Bundle {
    for key in LOCAL_SINCE.iter().filter(|(since, _)| *since > schema).flat_map(|(_, keys)| keys.iter()) {
        match theirs.settings.get(*key) {
            Some(value) => bundle.settings.insert(key.to_string(), value.clone()),
            None => bundle.settings.remove(*key),
        };
    }
    bundle
}

/// Makes this gateway's settings the package's: the travelling tables replaced whole, the travelling settings and
/// secrets set and the ones the package has not removed — but a `MOVE_ONLY` one it does not carry stays as it is, and
/// of each side's own only what a move sets (`MOVE_SETS`) is taken. What came with a schema the package does not carry
/// (`SINCE` above `Bundle::carries` — its writer is of an earlier build) stays as this gateway has it, the home and the
/// helpers with their keys: a new station whose settings a 0.11 computer keeps would lose them at every sending.
/// `station` leaves out what the station's build cannot run (MCP servers as programs); the computer leaves out the
/// station's model of this computer (`of_this_computer`), its key, and a choice that answered with it goes to the
/// computer's own model. A key revoked here stays revoked whatever the package says. The plugins' state a move
/// carries goes to the packages both sides have (`apply_plugins`). Of a shared setting, this gateway's own part — the
/// pair's door `home-computer` and its pin (`OWN_PARTS`) — stays at its place, and so does its key
/// (`STAYING_SECRETS`). One transaction for the database; the secrets after it.
pub fn import(db: &Db, secrets: &SecretsService, bundle: &Bundle, station: bool) -> Result<Imported, String> {
    import_keeping(db, secrets, bundle, station, Keep::default())
}

/// What an import leaves as it is here (the home's copy, docs/plans/home-and-devices.md, stage 3).
#[derive(Debug, Clone, Copy, Default)]
pub struct Keep<'a> {
    /// Tables of `TABLES` left whole: a copy holder keeps its own access keys — the ones its centre reaches it as Турбо
    /// with.
    pub tables: &'a [&'a str],
    /// This device's own things among the home's: its own models — providers at its own address that are no helper's,
    /// the model it gives the home as Турбо (helpers/builtin.rs, `route`) — and its own programs (MCP servers over
    /// stdio), each with its secrets. A choice of model that pointed at the centre's model of this device (a helper's
    /// row) is this device's own model here, so it answers with it while the centre is away. A centre's provider at a
    /// local address is the centre's own machine, not this one: left out here.
    pub own: bool,
    /// The Telegram bot this device runs stays as it is (`connector_telegram` and its token): the home has none yet and
    /// did not take this one.
    pub telegram: bool,
}

/// A table's rows as objects by column, in their order.
fn rows_of(connection: &rusqlite::Connection, table: &str) -> rusqlite::Result<Vec<Map<String, Value>>> {
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
    Ok(rows)
}

/// A provider row at a local address that is no helper's: a model of the machine it was made on.
fn local_model(row: &Map<String, Value>) -> bool {
    !at_home_server(row)
        && row
            .get("base_url")
            .and_then(Value::as_str)
            .is_some_and(|url| crate::operations::networks::is_local_url(Some(url)))
}

/// An MCP server that is a program on the machine it was made on.
fn own_program(row: &Map<String, Value>) -> bool {
    row.get("transport").and_then(Value::as_str) == Some("stdio")
}

/// This device's own rows (`Keep::own`): its local models and its programs, by table.
fn own_rows(db: &Db) -> rusqlite::Result<Vec<(&'static str, Map<String, Value>)>> {
    db.with(|connection| {
        let mut own = Vec::new();
        for row in rows_of(connection, "llm_providers")? {
            if local_model(&row) {
                own.push(("llm_providers", row));
            }
        }
        for row in rows_of(connection, "mcp_servers")? {
            if own_program(&row) {
                own.push(("mcp_servers", row));
            }
        }
        Ok(own)
    })
}

/// The settings and the secret of the Telegram bot a device runs.
const TELEGRAM_SETTING: &str = "connector_telegram";
const TELEGRAM_SECRET: &str = "connector:telegram:token";

pub fn import_keeping(
    db: &Db,
    secrets: &SecretsService,
    bundle: &Bundle,
    station: bool,
    keep: Keep<'_>,
) -> Result<Imported, String> {
    if bundle.format != FORMAT {
        return Err(crate::tr!("это не набор настроек DuoWave"));
    }
    if bundle.schema > SCHEMA {
        return Err(crate::tr!(
            "Набор настроек новее этой версии DuoWave (схема {schema}): сначала обновите DuoWave.",
            schema = bundle.schema
        ));
    }
    let mut imported = Imported::default();
    // The station's model of this computer, never the computer's: by id, with its key.
    let providers: Vec<&Map<String, Value>> =
        bundle.tables.get("llm_providers").map(|rows| rows.iter().collect()).unwrap_or_default();
    let left_out: Vec<String> = if station {
        Vec::new()
    } else {
        let helpers = bundle.settings.get(HELPERS.key).and_then(parse_helpers).unwrap_or_default();
        let me = this_gateway(db);
        providers
            .iter()
            .filter(|row| of_this_computer(row, &helpers, me.as_deref()))
            .filter_map(|row| row.get("id").and_then(Value::as_str).map(str::to_string))
            .collect()
    };
    // This device's own rows, read before anything is cleared (`Keep::own`).
    let mine: Vec<(&'static str, Map<String, Value>)> = if keep.own {
        own_rows(db).map_err(|error| crate::tr!("База DuoWave: {error}", error = error))?
    } else {
        Vec::new()
    };
    let mine_ids: Vec<(&'static str, String)> =
        mine.iter().filter_map(|(table, row)| Some((*table, row.get("id")?.as_str()?.to_string()))).collect();
    let own = if keep.own {
        own_model(&mine.iter().filter(|(table, _)| *table == "llm_providers").map(|(_, row)| row).collect::<Vec<_>>())
    } else {
        own_model(&providers.iter().copied().filter(|row| !at_home_server(row)).collect::<Vec<_>>())
    };
    let kept_secret = |id: &str| !left_out.iter().any(|provider| id.starts_with(&format!("llm:{provider}:")));
    // This device's own secrets: its models' and its programs' keys, and its bot's token when it stays.
    let own_secret = |id: &str| {
        mine_ids.iter().any(|(table, row)| {
            let prefix = if *table == "llm_providers" { "llm" } else { "mcp" };
            id.starts_with(&format!("{prefix}:{row}:"))
        }) || (keep.telegram && id == TELEGRAM_SECRET)
    };
    // What the package's writer did not know (`SINCE`): this gateway's own stays, absent or not in the package.
    let unknown_setting = |key: &str| setting_since(key) > bundle.carries;
    let unknown_secret = |id: &str| secret_since(id) > bundle.carries;
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
            if keep.tables.contains(table) {
                continue;
            }
            if *table == "app_settings" {
                // What this side learned of its own network stays (`LOCAL_APP_SETTINGS`).
                let local = LOCAL_APP_SETTINGS.iter().map(|key| format!("'{key}'")).collect::<Vec<_>>().join(", ");
                transaction.execute(&format!("DELETE FROM app_settings WHERE key NOT IN ({local})"), [])?;
                continue;
            }
            transaction.execute(&format!("DELETE FROM {table}"), [])?;
        }
        for table in TABLES {
            if keep.tables.contains(table) {
                continue;
            }
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
                if local_row(table, row) {
                    imported.skipped += 1;
                    continue;
                }
                // The centre's model at a local address is the centre's own machine, never this one.
                if keep.own && *table == "llm_providers" && local_model(row) {
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
            // This device's own rows, back as they were.
            for (_, row) in mine.iter().filter(|(own_table, _)| own_table == table) {
                let names: Vec<&String> = row.keys().filter(|name| known.contains(name)).collect();
                let placeholders =
                    (1..=names.len()).map(|index| format!("?{index}")).collect::<Vec<_>>().join(", ");
                let list = names.iter().map(|name| name.as_str()).collect::<Vec<_>>().join(", ");
                let values: Vec<SqlValue> = names.iter().map(|name| json_to_sql(&row[*name])).collect();
                transaction.execute(
                    &format!("INSERT OR IGNORE INTO {table} ({list}) VALUES ({placeholders})"),
                    rusqlite::params_from_iter(values),
                )?;
            }
            imported.rows.insert(table.to_string(), written);
        }
        for (id, at) in &revoked {
            transaction.execute(
                "UPDATE api_keys SET revoked_at = ?2 WHERE id = ?1 AND (revoked_at IS NULL OR revoked_at = '')",
                params![id, at],
            )?;
        }
        // This gateway's own part of a shared setting (`OWN_PARTS`) is kept at its place, whatever the package says.
        let mut incoming = bundle.settings.clone();
        for (key, id) in OWN_PARTS.iter().filter(|(key, _)| !unknown_setting(key)) {
            let stored: Option<Value> = transaction
                .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |row| row.get::<_, String>(0))
                .optional()?
                .and_then(|text| serde_json::from_str(&text).ok());
            match with_own(incoming.get(*key), stored.as_ref(), id) {
                Some(value) => incoming.insert(key.to_string(), value),
                None => incoming.remove(*key),
            };
        }
        let stored: Vec<String> = transaction
            .prepare("SELECT key FROM settings")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for key in stored {
            let kept = LOCAL_SETTINGS.contains(&key.as_str())
                || MOVE_ONLY.contains(&key.as_str())
                || unknown_setting(&key)
                || (keep.telegram && key == TELEGRAM_SETTING);
            if !kept && !incoming.contains_key(&key) {
                transaction.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
            }
        }
        for (key, value) in &incoming {
            if (LOCAL_SETTINGS.contains(&key.as_str()) && !MOVE_SETS.contains(&key.as_str())) || unknown_setting(key) {
                continue;
            }
            if keep.telegram && key == TELEGRAM_SETTING {
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
    .map_err(|error| crate::tr!("База DuoWave: {error}", error = error))?;
    for id in secrets.ids("") {
        if travels(&id)
            && !unknown_secret(&id)
            && !own_secret(&id)
            && !(bundle.secrets.contains_key(&id) && kept_secret(&id))
        {
            secrets.delete(&id).map_err(|error| error.message)?;
        }
    }
    for (id, value) in &bundle.secrets {
        if travels(id) && !unknown_secret(id) && kept_secret(id) && !own_secret(id) {
            secrets.set(id, Some(value)).map_err(|error| error.message)?;
            imported.secrets += 1;
        }
    }
    Ok(imported)
}

/// This gateway's own id (`gateway_self`, each gateway's own: it never travels), as the helpers' lists name it.
fn this_gateway(db: &Db) -> Option<String> {
    let stored: Option<String> = db
        .with(|c| {
            c.query_row("SELECT value FROM settings WHERE key = ?1", params![GATEWAY_SELF.key], |row| row.get(0))
                .optional()
        })
        .ok()
        .flatten();
    let me = stored.and_then(|text| serde_json::from_str::<Value>(&text).ok())?;
    me.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()).map(str::to_string)
}

/// The plugins' state alone, from the station a move back took it from while the settings lived on the computer
/// (moves.rs): the computer keeps its own settings and takes the plugins' (`apply_plugins`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginsPackage {
    pub from: String,
    pub at: String,
    pub plugins: BTreeMap<String, PluginState>,
    /// The plugins' pictures that came with it (plugins/files.rs).
    #[serde(default)]
    pub pictures: Vec<Picture>,
}

/// The plugins' state of a package alone, applied in a transaction of its own.
pub fn import_plugins(db: &Db, plugins: &BTreeMap<String, PluginState>) -> Result<usize, String> {
    db.transaction(|transaction| apply_plugins(transaction, plugins))
        .map_err(|error| crate::tr!("База DuoWave: {error}", error = error))
}

/// What the other gateway of the pair left for this one's start (a package of settings, the plugins' state of a move
/// back), applied before the services start; the whole database copied first (`copy_database`). When the copy fails
/// nothing is applied: what was left stays beside it as `.failed` and the gateway starts on what it had — as it does
/// with a package that does not apply.
pub fn apply_incoming(
    state_dir: &Path,
    db: &Db,
    secrets: &SecretsService,
    station: bool,
    pictures_root: Option<&Path>,
) -> Option<Imported> {
    let settings = incoming_file(state_dir);
    let plugins = incoming_plugins_file(state_dir);
    if !settings.exists() && !plugins.exists() {
        return None;
    }
    // The plugins' pictures that came along, kept beside the plugins' state (read before the files are applied).
    if let Some(root) = pictures_root {
        let bundle = std::fs::read(&settings).ok().and_then(|text| serde_json::from_slice::<Bundle>(&text).ok());
        if let Some((Some(pictures), Some(states))) = bundle.map(|bundle| (bundle.pictures, bundle.plugins)) {
            tracing::info!("settings: {} of the plugins' pictures kept", keep_pictures(root, &pictures, &states));
        }
        let package =
            std::fs::read(&plugins).ok().and_then(|text| serde_json::from_slice::<PluginsPackage>(&text).ok());
        if let Some(package) = package.filter(|package| !package.pictures.is_empty()) {
            let kept = keep_pictures(root, &package.pictures, &package.plugins);
            tracing::info!("settings: {kept} of the plugins' pictures kept");
        }
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
        .map_err(|error| format!("набор настроек не читается: {error}"))
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

/// What a move to the station carried besides the settings (carry.rs), installed at the gateway's start before the
/// package is applied: the own sources and the characters, each archive's hash checked. How many packages it installed.
pub fn take_carried(state_dir: &Path, packages_root: &Path, catalog_dir: &Path, db: &Db) -> usize {
    let carried = std::fs::read(incoming_file(state_dir))
        .ok()
        .and_then(|text| serde_json::from_slice::<Bundle>(&text).ok())
        .and_then(|bundle| bundle.carried);
    carried.map_or(0, |carried| super::carry::take(&carried, packages_root, catalog_dir, db))
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
        return Err(crate::tr!("это не запечатанный набор настроек"));
    };
    let nonce = Nonce::try_from(&nonce[..]).map_err(|_| crate::tr!("набор настроек повреждён"))?;
    let plain = pair_cipher(pair_key)
        .decrypt(&nonce, data.as_slice())
        .map_err(|_| crate::tr!("набор настроек не открывается: другой ключ пары или он повреждён"))?;
    serde_json::from_slice(&plain).map_err(|error| crate::tr!("Набор настроек не читается: {error}", error = error))
}

/// The import of every build before schema 2 (the apply loop in 708eec6 and earlier, the release 0.2.0's), its settings
/// part, for the tests here and the computer's (watch.rs): refuses a package newer than schema 1; removes each stored
/// setting the package lacks but its own — the voice waiting for its model was not its own then — and sets the
/// package's.
#[cfg(test)]
pub(crate) fn earlier_import(stored: &mut BTreeMap<String, Value>, bundle: &Bundle) -> Result<(), String> {
    let own = |key: &str| key != "voice_pending" && LOCAL_SETTINGS.contains(&key);
    if bundle.schema > 1 {
        return Err(crate::tr!(
            "Набор настроек новее этой версии DuoWave (схема {schema}): сначала обновите DuoWave.",
            schema = bundle.schema
        ));
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
    use crate::core::identity::home::HOME_KEY;
    use crate::settings::definitions::{
        GatewaySelf, HomeIdentity, ModeSetting, PanelMode, GATEWAY_SELF, GATEWAY_STATE, HOME_IDENTITY, KEEP_REQUESTS,
        MODE,
    };
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
        pc_secrets.set(&crate::secrets::plugin_secret("todo", "token"), Some("todo-not-a-real-token")).unwrap();
        pc_secrets.set("placement:peer", Some("pair")).unwrap();
        // A sign-in's tokens and the host's id: a refresh token renewed on two computers would end the sign-in.
        pc_secrets.set("oauth:llm:chatgpt", Some(r#"{"client_id":"oaiapp_1"}"#)).unwrap();
        pc_secrets.set("oauth:host", Some("urn:uuid:00000000-0000-4000-8000-000000000000")).unwrap();
        let bundle = export(&pc, &pc_secrets, "pc").unwrap();
        assert!(bundle.settings.contains_key("gateway_mode") && !bundle.settings.contains_key("gateway_state"));
        assert_eq!(
            bundle.secrets.keys().collect::<Vec<_>>(),
            vec!["llm:claude:key", "plugin:todo:token"],
            "a plugin's key travels as a model's; the pair's key, the sign-in's tokens and the host's id stay"
        );

        let (station, station_secrets, station_settings) = gateway();
        person(&station, "u9", "Старый");
        station_secrets.set("connector:telegram:token", Some("old")).unwrap();
        station_secrets.set(&crate::secrets::plugin_secret("radio", "token"), Some("old-radio")).unwrap();
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
        assert_eq!(station_secrets.get("plugin:todo:token").as_deref(), Some("todo-not-a-real-token"));
        assert_eq!(station_secrets.get("plugin:radio:token"), None, "a plugin's key goes as its settings do");
        assert_eq!(station_secrets.get("home-server:key").as_deref(), Some("mine"), "the station's own stays");

        // Back the other way: the same settings, the revision does not move for the time written.
        let back = export(&station, &station_secrets, "station").unwrap();
        assert_eq!(fingerprint(&back), fingerprint(&export(&station, &station_secrets, "station").unwrap()));
        assert_eq!(back.tables["users"].len(), 1);
    }

    /// The apps' own settings travel as the rest (an app's route through a server), but what one side learned of its
    /// own network stays there: a site that did not open directly from the computer's network says nothing of the
    /// station's.
    #[test]
    fn the_apps_settings_travel_but_what_a_side_learned_stays() {
        let row = |db: &Db, app: &str, key: &str, value: &str| {
            db.with(|c| {
                c.execute(
                    "INSERT INTO app_settings (app_id, key, value, updated_at) VALUES (?1, ?2, ?3, 'now')",
                    params![app, key, value],
                )
            })
            .unwrap();
        };
        let rows = |db: &Db| -> Vec<(String, String, String)> {
            db.with(|c| {
                c.prepare("SELECT app_id, key, value FROM app_settings ORDER BY app_id, key")?
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect()
            })
            .unwrap()
        };
        let (pc, pc_secrets, _) = gateway();
        row(&pc, "maps", "egress.route", r#""c1""#);
        row(&pc, "maps", "egress.learned", r#"{"blocked":true}"#);
        let bundle = export(&pc, &pc_secrets, "pc").unwrap();
        assert_eq!(bundle.tables["app_settings"].len(), 1, "the learned row stays on the computer");

        let (station, station_secrets, _) = gateway();
        row(&station, "radio", "egress.route", r#""direct""#);
        row(&station, "radio", "egress.learned", r#"{"blocked":false}"#);
        import(&station, &station_secrets, &bundle, true).unwrap();
        assert_eq!(
            rows(&station),
            vec![
                ("maps".to_string(), "egress.route".to_string(), r#""c1""#.to_string()),
                ("radio".to_string(), "egress.learned".to_string(), r#"{"blocked":false}"#.to_string()),
            ],
            "the computer's rules replace the station's; the station's own learning stays"
        );
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

    /// 0.6.0 keeps the plugins' keys in the safe; a station still on 0.5 reads them among the plugins' settings. Its
    /// package carries them there too, beside the plugin's other settings, and keeps the sealed secret for a newer
    /// build; a package without plugin keys is left as it was.
    #[test]
    fn a_station_on_0_5_gets_the_plugins_keys_among_their_settings() {
        let (pc, pc_secrets, pc_settings) = gateway();
        let mut stored = BTreeMap::new();
        stored.insert("calendar".to_string(), json!({ "count": 3 }).as_object().unwrap().clone());
        pc_settings.set(&crate::settings::definitions::PACKAGE_SETTINGS, &stored, None).unwrap();
        pc_secrets.set(&crate::secrets::plugin_secret("calendar", "token"), Some("cal-not-a-real-key")).unwrap();
        pc_secrets.set(&crate::secrets::plugin_secret("todo", "token"), Some("todo-not-a-real-key")).unwrap();

        let plain = routine(export(&pc, &pc_secrets, "pc").unwrap());
        assert!(plain.settings["packages.settings"]["calendar"].get("token").is_none(), "at 0.6 only in the safe");
        let sent = keys_in_settings(plain.clone());
        assert_eq!(
            sent.settings["packages.settings"]["calendar"],
            json!({ "count": 3, "token": "cal-not-a-real-key" })
        );
        assert_eq!(sent.settings["packages.settings"]["todo"], json!({ "token": "todo-not-a-real-key" }));
        assert_eq!(sent.secrets.get("plugin:calendar:token").map(String::as_str), Some("cal-not-a-real-key"));

        let (none, none_secrets, _) = gateway();
        let untouched = routine(export(&none, &none_secrets, "pc").unwrap());
        assert_eq!(keys_in_settings(untouched.clone()), untouched);
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
        assert_eq!((SCHEMA, sent.schema, whole.schema), (4, 4, WHOLE), "the routine package needs schema 2 at least");

        let station = BTreeMap::from([
            ("gateway_mode".to_string(), json!({ "mode": "relay" })),
            ("voice_pending".to_string(), json!({ "modelId": "station-voice" })),
        ]);
        let mut refused = station.clone();
        let error = earlier_import(&mut refused, &sent).unwrap_err();
        assert!(error.contains("сначала обновите DuoWave"), "{error}");
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

    /// A gateway's home and itself as its first start makes them (src/core/identity/home.rs).
    fn at_home(settings: &SettingsService, secrets: &SecretsService, home: &str, me: &str) {
        settings.set(&HOME_IDENTITY, &HomeIdentity { id: home.into(), name: None }, None).unwrap();
        secrets.set(HOME_KEY, Some(&format!("{home}-not-a-real-key"))).unwrap();
        settings.set(&GATEWAY_SELF, &GatewaySelf { id: me.into(), name: None }, None).unwrap();
    }

    /// The helpers' settings as the block keeps them: the list (the pair's own door first), the certificates' pins,
    /// the regulator — stored raw — and the helpers' keys.
    fn helpers_kept(db: &Db, secrets: &SecretsService) {
        let helpers = json!([
            { "id": "home-computer", "name": "Компьютер дома", "url": "http://192.0.2.10:7780", "kind": "home" },
            { "id": "own-1", "name": "Мой сервер", "url": "https://helper.example.com", "kind": "own" }
        ]);
        let pins = json!({ "home-computer": { "sha256": "aa" }, "own-1": { "public": "helper.example.com" } });
        for (key, value) in [("helpers", helpers), ("helper_pins", pins), ("away", json!("home"))] {
            db.with(|c| {
                c.execute(
                    "INSERT INTO settings (key, value, is_default, updated_at) VALUES (?1, ?2, 0, 'now')
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    params![key, value.to_string()],
                )
            })
            .unwrap();
        }
        secrets.set("helper:home-computer:key", Some("helper-not-a-real-key")).unwrap();
        secrets.set("helper:own-1:key", Some("own-not-a-real-key")).unwrap();
    }

    /// Default 8 of «Дом» и помощники: the settings owner's home comes to the other gateway with its package — the same
    /// home, the same key — and each gateway stays itself: its own id never travels.
    #[test]
    fn the_settings_owners_home_comes_with_its_package_each_side_keeps_its_own_id() {
        let (pc, pc_secrets, pc_settings) = gateway();
        at_home(&pc_settings, &pc_secrets, "home-pc", "gateway-pc");
        let (station, station_secrets, station_settings) = gateway();
        at_home(&station_settings, &station_secrets, "home-station", "gateway-station");

        let sent = routine(export(&pc, &pc_secrets, "pc").unwrap());
        assert_eq!((sent.schema, sent.carries), (SCHEMA, SCHEMA));
        assert_eq!(sent.settings["home_identity"], json!({ "id": "home-pc" }));
        assert!(!sent.settings.contains_key("gateway_self"), "each gateway's own");
        import(&station, &station_secrets, &sent, true).unwrap();
        assert_eq!(station_settings.get(&HOME_IDENTITY, None).unwrap().id, "home-pc");
        assert_eq!(station_secrets.get(HOME_KEY), pc_secrets.get(HOME_KEY), "one home, one key");
        assert_eq!(station_settings.get(&GATEWAY_SELF, None).unwrap().id, "gateway-station", "still itself");

        // The same both ways: a move back with the station's settings brings the computer its home, not its id.
        at_home(&station_settings, &station_secrets, "home-station", "gateway-station");
        import(&pc, &pc_secrets, &export(&station, &station_secrets, "station").unwrap(), false).unwrap();
        assert_eq!(pc_settings.get(&HOME_IDENTITY, None).unwrap().id, "home-station");
        assert_eq!(pc_settings.get(&GATEWAY_SELF, None).unwrap().id, "gateway-pc");
    }

    /// «Старому шлюзу уходит схема 3 без новых полей»: a package for a gateway of an earlier schema leaves out what
    /// schema 4 brought, its settings and its secrets, and asks no newer import; the 0.5 cut and an earlier build's whole
    /// package alike. A gateway of this build gets the package as it is.
    #[test]
    fn a_package_cut_for_schema_3_carries_nothing_schema_4_brought() {
        let (pc, pc_secrets, pc_settings) = gateway();
        at_home(&pc_settings, &pc_secrets, "home-pc", "gateway-pc");
        helpers_kept(&pc, &pc_secrets);
        pc_settings.set(&KEEP_REQUESTS, &7, None).unwrap();
        pc_secrets.set("llm:claude:key", Some("sk-not-a-real-key")).unwrap();
        pc.with(|c| {
            c.execute(
                "INSERT INTO app_settings (app_id, key, value, updated_at) VALUES (?1, ?2, ?3, 'now')",
                params!["maps", "egress.route", r#""c1""#],
            )
        })
        .unwrap();
        let whole = export(&pc, &pc_secrets, "pc").unwrap();
        let brought = ["home_identity", "helpers", "helper_pins", "away"];
        assert!(brought.iter().all(|key| whole.settings.contains_key(*key)), "{:?}", whole.settings.keys());
        let none_new = |bundle: &Bundle| {
            brought.iter().all(|key| !bundle.settings.contains_key(*key))
                && bundle.secrets.keys().all(|id| !id.starts_with("home:") && !id.starts_with("helper:"))
        };

        let cut = for_schema(routine(whole.clone()), BEFORE_HOME_SCHEMA);
        assert_eq!((cut.schema, cut.carries), (3, 3), "a 0.11 import takes it");
        assert!(none_new(&cut), "{:?} {:?}", cut.settings.keys(), cut.secrets.keys());
        assert_eq!(cut.settings["requests_keep_days"], json!(7), "the rest as before");
        assert_eq!(cut.secrets.keys().collect::<Vec<_>>(), ["llm:claude:key"]);
        // The apps' own settings came with 0.13.0 at schema 3: a 0.13 import takes them, an earlier passes them by.
        assert_eq!(cut.tables["app_settings"].len(), 1, "{:?}", cut.tables["app_settings"]);
        assert_eq!(for_schema(routine(whole.clone()), SCHEMA), routine(whole.clone()), "this build: as it is");

        let five =
            for_schema(Bundle { schema: KEYS_IN_SETTINGS_SCHEMA, ..keys_in_settings(routine(whole.clone())) }, 2);
        assert!((five.schema, five.carries) == (2, 2) && none_new(&five));
        let bare = Bundle { settings: BTreeMap::new(), ..whole.clone() };
        let earliest = for_schema(for_earlier(whole, &bare), 1);
        assert!((earliest.schema, earliest.carries) == (WHOLE, 1) && none_new(&earliest));
        assert_eq!(earliest.settings["requests_keep_days"], json!(7));
    }

    /// Stage 4's rollback risk (docs/plans/home-and-helpers.md): a linked helper's «Мозг» row stores no address — its
    /// bridge and token are each start's —, and a 0.11 import reads such a row as an OpenAI provider at OpenAI's own
    /// address, where a chosen question would go without a key. A package for an earlier import leaves it out, with
    /// whatever is kept under its id and every pick of it that `pick_again` knows; a helper's row with its address,
    /// which 0.11 asks as today, stays. What a 0.11 import then does with a pick of a row it did not get (bd1671f4:
    /// dialog/brain/brain_service.rs `picked`, `slot`, `slot_of`; dialog/dialog_service.rs; memory/reflect.rs): «Мозг»
    /// answers with its .env's model, a backup or an agent's model it does not find is none and the profile's, and the
    /// memory's own model waits («Ждёт модель…») — no client is made for a row it has not, so nothing goes to a cloud.
    #[test]
    fn a_package_for_an_earlier_import_leaves_out_a_helpers_row_without_an_address_and_its_picks() {
        let (pc, pc_secrets, _) = gateway();
        provider(&pc, "claude", "anthropic", "Claude", "https://api.anthropic.com", "2026-11-01T00:00:00Z");
        pc_secrets.set("llm:claude:key", Some("sk-not-a-real-key")).unwrap();
        let helper_row = |id: &str, helper: &str, url: Option<&str>| {
            let options = json!({ "helper": helper, "reach": "mine" }).to_string();
            pc.with(|c| {
                c.execute(
                    "INSERT INTO llm_providers (id, preset, kind, title, base_url, options, created_at, updated_at)
                     VALUES (?1, 'helper', 'openai', ?2, ?3, ?4, '2026-11-15T00:00:00Z', '2026-11-15T00:00:00Z')",
                    params![id, format!("Модель: {helper}"), url, options],
                )
            })
            .unwrap();
        };
        // A linked helper's row (no address, no key) and a plain one's (its address and key, as 0.11 asks it).
        helper_row("helper-own-l", "own-l", None);
        helper_row("helper-own-p", "own-p", Some("https://gpu.example.com/home/v1/llm"));
        // One that keeps the stand-in for an address, as this build's table has it: left out alike.
        helper_row("helper-own-s", "own-s", Some(crate::dialog::brain::presets::HELPER_NO_ADDRESS));
        pc_secrets.set("llm:helper-own-p:key", Some("vgw_not-a-real-key")).unwrap();
        pc_secrets.set("llm:helper-own-l:key", Some("whatever-stays-under-its-id")).unwrap();
        let settings = [
            (
                "brain",
                json!({
                    "fast": { "provider": "claude", "model": "claude-haiku" },
                    "smart": { "provider": "helper-own-l", "model": "qwen" },
                    "backup": { "provider": "helper-own-l", "model": "qwen" }
                }),
            ),
            ("workshop.agent", json!({ "provider": "helper-own-l", "model": "qwen" })),
            ("memory_model", json!({ "mode": "pick", "provider": "helper-own-l", "model": "qwen" })),
        ];
        for (key, value) in &settings {
            pc.with(|c| {
                c.execute(
                    "INSERT INTO settings (key, value, is_default, updated_at) VALUES (?1, ?2, 0, 'now')",
                    params![key, value.to_string()],
                )
            })
            .unwrap();
        }
        let whole = export(&pc, &pc_secrets, "pc").unwrap();
        let ids = |bundle: &Bundle| -> Vec<String> {
            bundle.tables["llm_providers"].iter().filter_map(|row| row["id"].as_str().map(str::to_string)).collect()
        };
        assert_eq!(ids(&whole), ["claude", "helper-own-l", "helper-own-p", "helper-own-s"]);

        let cut = for_schema(routine(whole.clone()), BEFORE_HOME_SCHEMA);
        assert_eq!(ids(&cut), ["claude", "helper-own-p"], "the row 0.11 would ask at OpenAI's address is left out");
        assert!(
            cut.tables["llm_providers"].iter().all(|row| row["base_url"].as_str().is_some_and(|url| !url.is_empty())),
            "every row it carries has its own address"
        );
        assert_eq!(cut.secrets.keys().collect::<Vec<_>>(), ["llm:claude:key", "llm:helper-own-p:key"]);
        // The smart and the backup picks go: the smart answers with the fast model, and there is no backup.
        assert_eq!(cut.settings["brain"], json!({ "fast": { "provider": "claude", "model": "claude-haiku" } }));
        assert!(!cut.settings.contains_key("workshop.agent"), "the agent answers with the smart profile");
        assert_eq!(
            cut.settings["memory_model"], settings[2].1,
            "kept: a 0.11 night waits for a model it does not find"
        );

        // The 0.5 cut and an earlier build's whole package alike; this build knows the row: the package as it is.
        let five =
            for_schema(Bundle { schema: KEYS_IN_SETTINGS_SCHEMA, ..keys_in_settings(routine(whole.clone())) }, 2);
        assert_eq!(ids(&five), ["claude", "helper-own-p"]);
        let earliest =
            for_schema(for_earlier(whole.clone(), &Bundle { settings: BTreeMap::new(), ..whole.clone() }), 1);
        assert_eq!(ids(&earliest), ["claude", "helper-own-p"]);
        assert_eq!(for_schema(routine(whole.clone()), SCHEMA), routine(whole));
    }

    /// A new station whose settings a 0.11 computer keeps: the computer's packages know nothing of the home and the
    /// helpers (no `carries` in them), and the station keeps its own — or it would make a new home at every start. A
    /// package of this build without them removes them, as any setting its writer has not — but the station's own door
    /// to its computer (`OWN_PARTS`) and its key.
    #[test]
    fn a_package_from_a_writer_before_the_home_keeps_the_home_and_helpers_here() {
        let (station, station_secrets, station_settings) = gateway();
        at_home(&station_settings, &station_secrets, "home-station", "gateway-station");
        helpers_kept(&station, &station_secrets);
        let (pc, pc_secrets, pc_settings) = gateway();
        pc_settings.set(&KEEP_REQUESTS, &7, None).unwrap();
        let mut earlier = serde_json::to_value(routine(export(&pc, &pc_secrets, "pc").unwrap())).unwrap();
        earlier.as_object_mut().unwrap().remove("carries");
        earlier["schema"] = json!(BEFORE_HOME_SCHEMA);
        let earlier: Bundle = serde_json::from_value(earlier).unwrap();
        assert_eq!(earlier.carries, 0, "a writer from before schema 4");

        let kept = |settings: &SettingsService, secrets: &SecretsService| {
            ["home_identity", "helpers", "helper_pins", "away"].map(|key| settings.raw(key).is_some()) == [true; 4]
                && secrets.get(HOME_KEY).as_deref() == Some("home-station-not-a-real-key")
                && secrets.has("helper:home-computer:key")
        };
        import(&station, &station_secrets, &earlier, true).unwrap();
        assert!(kept(&station_settings, &station_secrets), "the home and the helpers stay");
        assert_eq!(station_settings.get(&KEEP_REQUESTS, None), Some(7), "the rest of the computer's settings came");
        assert_eq!(station_settings.get(&HOME_IDENTITY, None).unwrap().id, "home-station");

        // Such a package holding them all the same (a 0.11 build stores what it does not know): not taken.
        let mut stale = earlier.clone();
        stale.settings.insert("home_identity".into(), json!({ "id": "home-stale" }));
        stale.secrets.insert(HOME_KEY.into(), "stale-not-a-real-key".into());
        import(&station, &station_secrets, &stale, true).unwrap();
        assert!(kept(&station_settings, &station_secrets));
        assert_eq!(station_settings.get(&HOME_IDENTITY, None).unwrap().id, "home-station");

        let now = routine(export(&pc, &pc_secrets, "pc").unwrap());
        import(&station, &station_secrets, &now, true).unwrap();
        assert!(["home_identity", "away"].iter().all(|key| station_settings.raw(key).is_none()));
        let helpers = station_settings.raw("helpers").unwrap();
        assert_eq!(helpers.as_array().map(|list| list.len()), Some(1));
        assert_eq!(helpers[0]["id"], "home-computer", "the station's own door stays");
        assert_eq!(station_settings.raw("helper_pins"), Some(json!({ "home-computer": { "sha256": "aa" } })));
        assert!(!station_secrets.has(HOME_KEY) && !station_secrets.has("helper:own-1:key"));
        assert!(station_secrets.has("helper:home-computer:key"), "and its key");
        assert_eq!(station_settings.get(&GATEWAY_SELF, None).unwrap().id, "gateway-station", "its own never goes");
    }

    /// The pair's door is each gateway's own (the spec's C4): the computer's package leaves out its `home-computer`
    /// entry, its pin and its key, and an import keeps the station's own at its place — an incoming one is passed by.
    #[test]
    fn the_home_computer_entry_its_pin_and_its_key_stay_on_each_gateway() {
        let (pc, pc_secrets, _) = gateway();
        helpers_kept(&pc, &pc_secrets);
        let sent = routine(export(&pc, &pc_secrets, "pc").unwrap());
        let ids = |list: &Value| list.as_array().unwrap().iter().map(|item| item["id"].clone()).collect::<Vec<_>>();
        assert_eq!(ids(&sent.settings["helpers"]), [json!("own-1")]);
        assert_eq!(sent.settings["helper_pins"], json!({ "own-1": { "public": "helper.example.com" } }));
        assert_eq!(
            sent.secrets.keys().filter(|id| id.starts_with("helper:")).collect::<Vec<_>>(),
            ["helper:own-1:key"]
        );

        // The station has its own door — second in its list — with its own key and pin.
        let (station, station_secrets, station_settings) = gateway();
        let own =
            json!({ "id": "home-computer", "name": "Компьютер дома", "url": "http://192.0.2.20:7780", "kind": "home" });
        let theirs = json!({ "id": "cloud-1", "name": "Облако", "url": "https://cloud.example.com", "kind": "cloud" });
        let stored =
            [("helpers", json!([theirs, own])), ("helper_pins", json!({ "home-computer": { "sha256": "bb" } }))];
        for (key, value) in stored {
            station
                .with(|c| {
                    c.execute(
                        "INSERT INTO settings (key, value, is_default, updated_at) VALUES (?1, ?2, 0, 'now')",
                        params![key, value.to_string()],
                    )
                })
                .unwrap();
        }
        station_secrets.set("helper:home-computer:key", Some("station-door-not-a-real-key")).unwrap();
        // A package that carries a `home-computer` all the same (a hand-made one): passed by.
        let mut forged = sent.clone();
        let mut list = forged.settings["helpers"].as_array().unwrap().clone();
        list.insert(
            0,
            json!({ "id": "home-computer", "name": "Чужой", "url": "http://192.0.2.99:7780", "kind": "home" }),
        );
        forged.settings.insert("helpers".into(), Value::Array(list));
        forged.secrets.insert("helper:home-computer:key".into(), "forged-not-a-real-key".into());
        import(&station, &station_secrets, &forged, true).unwrap();
        let helpers = station_settings.raw("helpers").unwrap();
        assert_eq!(
            ids(&helpers),
            [json!("own-1"), json!("home-computer")],
            "its own at its own place, the rest the package's"
        );
        assert_eq!(helpers[1], own, "its own as it was");
        assert_eq!(
            station_settings.raw("helper_pins"),
            Some(json!({ "own-1": { "public": "helper.example.com" }, "home-computer": { "sha256": "bb" } }))
        );
        assert_eq!(station_secrets.get("helper:home-computer:key").as_deref(), Some("station-door-not-a-real-key"));
        assert_eq!(station_secrets.get("helper:own-1:key").as_deref(), Some("own-not-a-real-key"), "the rest travel");

        // A station whose list is only its own door gets nothing else and keeps it; the computer's own door never
        // reaches it.
        let (alone, alone_secrets, alone_settings) = gateway();
        import(&alone, &alone_secrets, &sent, true).unwrap();
        assert_eq!(ids(&alone_settings.raw("helpers").unwrap()), [json!("own-1")], "none of its own: none at all");
        assert!(!alone_secrets.has("helper:home-computer:key"));
    }

    /// A helper's own secrets — its connection key, its certificate, its clients' keys (stage 7) — live under
    /// `helping:`, no travelling prefix: never in its package, and an import neither takes another's nor removes its
    /// own. So do what it keeps as a helper among the settings (`helper_clients`, `helper_outside`).
    #[test]
    fn the_helpers_own_secrets_never_leave_its_machine() {
        let (pc, pc_secrets, pc_settings) = gateway();
        for id in ["helping:link", "helping:tls", "helping:client:x"] {
            pc_secrets.set(id, Some("pc-own-not-a-real-key")).unwrap();
        }
        pc_secrets.set("helper:cloud:key", Some("cloud-not-a-real-key")).unwrap();
        for (key, value) in [
            ("helper_clients", json!({ "link:1": {} })),
            ("helper_outside", json!({ "url": "https://helper.example.com" })),
        ] {
            pc.with(|c| {
                c.execute(
                    "INSERT INTO settings (key, value, is_default, updated_at) VALUES (?1, ?2, 0, 'now')",
                    params![key, value.to_string()],
                )
            })
            .unwrap();
        }
        let sent = export(&pc, &pc_secrets, "pc").unwrap();
        assert_eq!(sent.secrets.keys().collect::<Vec<_>>(), ["helper:cloud:key"]);
        assert!(!sent.settings.contains_key("helper_clients") && !sent.settings.contains_key("helper_outside"));

        let (station, station_secrets, _) = gateway();
        station_secrets.set("helping:link", Some("station-link-not-a-real-key")).unwrap();
        let mut forged = sent.clone();
        forged.secrets.insert("helping:link".into(), "another-link-not-a-real-key".into());
        forged.secrets.insert("helping:tls".into(), "another-tls-not-a-real-key".into());
        forged.settings.insert("helper_outside".into(), json!({ "url": "https://elsewhere.example.com" }));
        import(&station, &station_secrets, &forged, true).unwrap();
        assert_eq!(station_secrets.get("helping:link").as_deref(), Some("station-link-not-a-real-key"));
        assert!(!station_secrets.has("helping:tls"), "another's never taken");
        assert_eq!(station_secrets.get("helper:cloud:key").as_deref(), Some("cloud-not-a-real-key"), "the rest travel");
        import(&station, &station_secrets, &routine(export(&pc, &pc_secrets, "pc").unwrap()), true).unwrap();
        assert!(station_secrets.has("helping:link"), "not removed by a package without it");
        import(&pc, &pc_secrets, &routine(export(&station, &station_secrets, "station").unwrap()), false).unwrap();
        assert_eq!(
            pc_settings.raw("helper_outside"),
            Some(json!({ "url": "https://helper.example.com" })),
            "its own stays"
        );
        assert!(pc_settings.raw("helper_clients").is_some() && pc_secrets.has("helping:client:x"));
    }

    /// Review, round one: a station put back on 0.11 by the safety net, its database kept, does not know its own id and
    /// name as its own, and its import removes each setting a package lacks. A package for it carries them as its own
    /// package says them — never the sender's — and leaves them out when it has none; a gateway of this build keeps its
    /// own itself, and the whole package for the earliest builds carries them already.
    #[test]
    fn a_package_for_an_earlier_import_carries_the_receivers_own_id_as_its_package_says_it() {
        let (pc, pc_secrets, pc_settings) = gateway();
        at_home(&pc_settings, &pc_secrets, "home-pc", "gateway-pc");
        pc_settings.set(&KEEP_REQUESTS, &7, None).unwrap();
        let whole = export(&pc, &pc_secrets, "pc").unwrap();
        let sent = for_schema(routine(whole.clone()), BEFORE_HOME_SCHEMA);
        assert!(!sent.settings.contains_key("gateway_self"), "the computer's own never goes");

        // A 0.11 station's own package carries its id as an ordinary setting; so does its database.
        let own = json!({ "id": "gateway-station", "name": "Кухня" });
        let theirs = Bundle { settings: BTreeMap::from([("gateway_self".to_string(), own.clone())]), ..whole.clone() };
        let stored = BTreeMap::from([("gateway_self".to_string(), own.clone())]);
        // What a 0.11 import does with the settings: what it knows as its own stays, the rest is the package's.
        let before_home = |bundle: &Bundle| {
            let known = |key: &str| {
                (LOCAL_SETTINGS.contains(&key) && !LOCAL_SINCE.iter().any(|(_, keys)| keys.contains(&key)))
                    || MOVE_ONLY.contains(&key)
            };
            let mut after: BTreeMap<String, Value> = stored
                .iter()
                .filter(|(key, _)| known(key) || bundle.settings.contains_key(*key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            after.extend(bundle.settings.iter().filter(|(key, _)| !known(key)).map(|(k, v)| (k.clone(), v.clone())));
            after
        };
        assert!(!before_home(&sent).contains_key("gateway_self"), "what a station put back on 0.11 lost");

        let kept = their_own(sent.clone(), &theirs, BEFORE_HOME_SCHEMA);
        let after = before_home(&kept);
        assert_eq!((&after["gateway_self"], &after["requests_keep_days"]), (&own, &json!(7)));
        let mut rest = kept.clone();
        rest.settings.remove("gateway_self");
        assert_eq!(rest, sent, "nothing else of the package changes");
        let bare = Bundle { settings: BTreeMap::new(), ..whole.clone() };
        let none = their_own(kept, &bare, KEYS_IN_SETTINGS_SCHEMA);
        assert!(!none.settings.contains_key("gateway_self"), "none of its own: none at all");
        assert!(!knows_own(BEFORE_HOME_SCHEMA) && knows_own(SCHEMA));
        assert_eq!(their_own(routine(whole.clone()), &theirs, SCHEMA), routine(whole.clone()), "this build: as it is");
        assert_eq!(
            for_earlier(whole, &theirs).settings["gateway_self"],
            own,
            "the earliest builds' package carries it"
        );
        assert!(LOCAL_SINCE.iter().flat_map(|(_, keys)| keys.iter()).all(|key| LOCAL_SETTINGS.contains(key)));
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
        let imported = apply_incoming(dir.path(), &station, &station_secrets, true, None).unwrap();
        assert_eq!(imported.rows["users"], 1);
        assert!(!incoming_file(dir.path()).exists());
        assert!(apply_incoming(dir.path(), &station, &station_secrets, true, None).is_none());
        std::fs::write(incoming_file(dir.path()), b"not a package").unwrap();
        assert!(apply_incoming(dir.path(), &station, &station_secrets, true, None).is_none());
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

    /// The plugins' pictures go with a move: to the station as many as fit its 5 MB, the newest first; a picture of a
    /// plugin the other side has no state for, or one that is not a picture, is passed by.
    #[test]
    fn a_move_carries_the_newest_pictures_that_fit_and_keeps_them_for_the_plugins_it_knows() {
        let png = |size: usize| {
            let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
            bytes.resize(size, 0);
            bytes
        };
        let mb = 1024 * 1024;
        let here = tempfile::tempdir().unwrap();
        let day = here.path().join("day");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join("old.png"), png(3 * mb)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(day.join("new.png"), png(3 * mb)).unwrap();
        let fit = pictures(here.path(), Some(crate::plugins::files::STATION_ALL_MAX));
        assert_eq!(fit.iter().map(|picture| picture.name.as_str()).collect::<Vec<_>>(), ["new.png"]);
        assert_eq!(pictures(here.path(), None).len(), 2);
        let there = tempfile::tempdir().unwrap();
        let mut states = BTreeMap::new();
        states.insert("day".to_string(), PluginState { enabled: true, at: None, kv: Vec::new(), jobs: Vec::new() });
        let mut carried = fit.clone();
        carried.push(Picture { plugin: "other".into(), name: "x.png".into(), data: fit[0].data.clone(), written: 0 });
        carried.push(Picture {
            plugin: "day".into(),
            name: "page.png".into(),
            data: B64.encode(b"<html>"),
            written: 0,
        });
        assert_eq!(keep_pictures(there.path(), &carried, &states), 1);
        assert!(there.path().join("day/new.png").is_file() && !there.path().join("other").exists());
    }

    /// The plugins' schedules live where the gateway runs, as their data does: a move carries the tasks the code set
    /// and the rhythms' pauses, for the packages both sides have.
    #[test]
    fn a_move_carries_the_plugins_schedules_with_their_data() {
        let jobs = |db: &Db, id: &str| -> Option<String> {
            db.with(|c| {
                c.query_row("SELECT items FROM state WHERE kind = ?1 AND device = ?2", params![SCHEDULES, id], |row| {
                    row.get(0)
                })
                .optional()
            })
            .unwrap()
        };
        let (pc, pc_secrets, _) = gateway();
        plugin(&pc, "repeat", true, &[]);
        let pills = r#"[{"id":"pills","title":"Таблетки","manifest":false,"when":{"type":"at","at":"09:00","days":"daily"},"then":{"say":"Таблетки"}}]"#;
        pc.with(|c| {
            c.execute(
                "INSERT INTO state (kind, device, items, updated_at) VALUES (?1, 'repeat', ?2, '2026-11-12T10:00:00Z')",
                params![SCHEDULES, pills],
            )
        })
        .unwrap();
        let (station, station_secrets, _) = gateway();
        plugin(&station, "repeat", false, &[]);
        let mut moved = export(&pc, &pc_secrets, "pc").unwrap();
        moved.plugins = Some(plugins(&pc).unwrap());
        assert_eq!(moved.plugins.as_ref().unwrap()["repeat"].jobs.len(), 1);
        import(&station, &station_secrets, &moved, true).unwrap();
        let carried: Value = serde_json::from_str(&jobs(&station, "repeat").unwrap()).unwrap();
        assert_eq!(carried[0]["id"], "pills");
        // Back without them: the computer's go too, as its data would.
        let mut back = export(&station, &station_secrets, "station").unwrap();
        let mut theirs = plugins(&station).unwrap();
        theirs.get_mut("repeat").unwrap().jobs.clear();
        back.plugins = Some(theirs);
        import(&pc, &pc_secrets, &back, false).unwrap();
        assert_eq!(jobs(&pc, "repeat"), None);
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

    /// docs/plans/home-and-helpers.md, stage 4: a station's «Мозг» has a row for each helper's model. The computer
    /// leaves out only the station's model of the computer itself — its door's, or an entry that names the computer —
    /// and every pick of it, the backup's too, goes to the computer's own model; the other helpers' rows come with
    /// their helpers, picks and keys and all.
    #[test]
    fn the_computer_leaves_out_only_its_own_model_and_keeps_the_other_helpers_rows() {
        let (station, station_secrets, _) = gateway();
        provider(&station, "ollama", "openai", "Ollama", "http://127.0.0.1:11434/v1", "2026-11-01T00:00:00Z");
        let helper_row = |id: &str, helper: &str, reach: &str, url: &str| {
            let options = json!({ "helper": helper, "reach": reach }).to_string();
            station
                .with(|c| {
                    c.execute(
                        "INSERT INTO llm_providers (id, preset, kind, title, base_url, options, created_at, updated_at)
                         VALUES (?1, 'helper', 'openai', ?2, ?3, ?4, '2026-11-15T00:00:00Z', '2026-11-15T00:00:00Z')",
                        params![id, format!("Модель: {helper}"), url, options],
                    )
                })
                .unwrap();
            station_secrets.set(&format!("llm:{id}:key"), Some(&format!("{helper}-not-a-real-key"))).unwrap();
        };
        helper_row("helper-home-computer", "home-computer", "home", "http://192.0.2.20:7780/home/v1/llm");
        helper_row("helper-own-gpu", "own-gpu", "mine", "https://gpu.example.com/home/v1/llm");
        helper_row("helper-home-b", "home-b", "home", "http://192.0.2.21:7780/home/v1/llm");
        let helpers = json!([
            { "id": "own-gpu", "name": "GPU", "url": "https://gpu.example.com", "kind": "own", "helperId": "gpu-1" },
            { "id": "home-b", "name": "Кабинет", "url": "http://192.0.2.21:7780", "kind": "home", "helperId": "pc-1" }
        ]);
        let brain = json!({
            "fast": { "provider": "helper-own-gpu", "model": "qwen" },
            "smart": { "provider": "helper-home-computer", "model": "qwen" },
            "backup": { "provider": "helper-home-b", "model": "small" }
        });
        station
            .with(|c| {
                c.execute(
                    "INSERT INTO settings (key, value, is_default, updated_at) VALUES ('helpers', ?1, 0, 'now'),
                     ('brain', ?2, 0, 'now')",
                    params![helpers.to_string(), brain.to_string()],
                )
            })
            .unwrap();
        let theirs = export(&station, &station_secrets, "station").unwrap();

        let (pc, pc_secrets, pc_settings) = gateway();
        at_home(&pc_settings, &pc_secrets, "home-1", "pc-1");
        let imported = import(&pc, &pc_secrets, &theirs, false).unwrap();
        assert_eq!((imported.rows["llm_providers"], imported.skipped), (2, 2), "the door's row and the one naming it");
        let ids: Vec<String> = pc
            .with(|c| c.prepare("SELECT id FROM llm_providers ORDER BY id")?.query_map([], |row| row.get(0))?.collect())
            .unwrap();
        assert_eq!(ids, ["helper-own-gpu", "ollama"]);
        assert_eq!(pc_secrets.get("llm:helper-own-gpu:key").as_deref(), Some("own-gpu-not-a-real-key"));
        assert_eq!(
            (pc_secrets.get("llm:helper-home-computer:key"), pc_secrets.get("llm:helper-home-b:key")),
            (None, None)
        );
        assert_eq!(
            pc_settings.raw("brain"),
            Some(json!({
                "fast": { "provider": "helper-own-gpu", "model": "qwen" },
                "smart": { "provider": "ollama", "model": "qwen" },
                "backup": { "provider": "ollama", "model": "small" }
            }))
        );

        // A computer that does not know its own id leaves out the door's row only.
        let (other, other_secrets, _) = gateway();
        let imported = import(&other, &other_secrets, &theirs, false).unwrap();
        assert_eq!((imported.rows["llm_providers"], imported.skipped), (3, 1));
    }

    /// The home's copy on a computer (docs/plans/home-and-devices.md, stage 3): the home's settings come, and what is
    /// this computer's own stays — its model at its own address (the one it gives the home as Турбо) with its key, its
    /// MCP program, its access keys; the home's choice «думает Турбо» is this computer's own model here; a model the
    /// centre keeps at the centre's own local address is left out. Its bot stays only while the home has none.
    #[test]
    fn the_home_copy_keeps_this_computers_own_model_program_and_bot() {
        let (station, station_secrets, _) = gateway();
        provider(
            &station,
            "openrouter",
            "openai",
            "OpenRouter",
            "https://openrouter.ai/api/v1",
            "2026-11-01T00:00:00Z",
        );
        station_secrets.set("llm:openrouter:key", Some("router-not-a-real-key")).unwrap();
        provider(&station, "station-local", "openai", "На колонке", "http://127.0.0.1:8080/v1", "2026-11-01T00:00:00Z");
        let options = json!({ "helper": "home-computer", "reach": "home" }).to_string();
        station
            .with(|c| {
                c.execute(
                    "INSERT INTO llm_providers (id, preset, kind, title, base_url, options, created_at, updated_at)
                     VALUES ('helper-home-computer', 'helper', 'openai', 'Турбо', 'http://192.0.2.20:7780/home/v1/llm', ?1,
                     '2026-11-15T00:00:00Z', '2026-11-15T00:00:00Z')",
                    params![options],
                )
            })
            .unwrap();
        let mcp = |db: &Db, id: &str, transport: &str| {
            db.with(|c| {
                c.execute(
                    "INSERT INTO mcp_servers (id, title, transport, config, created_at, updated_at) VALUES (?1, ?1, ?2, '{}', 'now', 'now')",
                    params![id, transport],
                )
            })
            .unwrap();
        };
        mcp(&station, "notion", "http");
        station_secrets.set("mcp:notion:token", Some("notion-not-a-real-key")).unwrap();
        let brain = json!({
            "fast": { "provider": "helper-home-computer", "model": "qwen" },
            "smart": { "provider": "openrouter", "model": "claude" }
        });
        station
            .with(|c| {
                c.execute(
                    "INSERT INTO settings (key, value, is_default, updated_at) VALUES ('brain', ?1, 0, 'now')",
                    params![brain.to_string()],
                )
            })
            .unwrap();
        let theirs = export(&station, &station_secrets, "station").unwrap();

        let (pc, pc_secrets, pc_settings) = gateway();
        provider(&pc, "lmstudio", "openai", "LM Studio", "http://127.0.0.1:1234/v1", "2026-11-02T00:00:00Z");
        pc_secrets.set("llm:lmstudio:key", Some("lm-not-a-real-key")).unwrap();
        mcp(&pc, "files", "stdio");
        pc_secrets.set("mcp:files:token", Some("files-not-a-real-key")).unwrap();
        pc.with(|c| {
            c.execute(
                "INSERT INTO settings (key, value, is_default, updated_at) VALUES ('connector_telegram', ?1, 0, 'now')",
                params![json!({ "enabled": true, "contacts": [{ "name": "мама", "to": "111" }] }).to_string()],
            )
        })
        .unwrap();
        pc_secrets.set("connector:telegram:token", Some("123:not-a-real-token")).unwrap();

        let keep = Keep { tables: &["api_keys"], own: true, telegram: true };
        import_keeping(&pc, &pc_secrets, &theirs, false, keep).unwrap();
        let ids = |db: &Db, table: &str| -> Vec<String> {
            db.with(|c| {
                c.prepare(&format!("SELECT id FROM {table} ORDER BY id"))?.query_map([], |row| row.get(0))?.collect()
            })
            .unwrap()
        };
        assert_eq!(
            ids(&pc, "llm_providers"),
            ["lmstudio", "openrouter"],
            "its own model stays, the centre's local one does not come"
        );
        assert_eq!(pc_secrets.get("llm:lmstudio:key").as_deref(), Some("lm-not-a-real-key"));
        assert_eq!(pc_secrets.get("llm:openrouter:key").as_deref(), Some("router-not-a-real-key"));
        assert_eq!(ids(&pc, "mcp_servers"), ["files", "notion"], "its program stays beside the home's connection");
        assert_eq!(pc_secrets.get("mcp:files:token").as_deref(), Some("files-not-a-real-key"));
        assert_eq!(
            pc_settings.raw("brain"),
            Some(json!({
                "fast": { "provider": "lmstudio", "model": "qwen" },
                "smart": { "provider": "openrouter", "model": "claude" }
            })),
            "«думает Турбо» is this computer's own model here"
        );
        assert!(pc_settings.raw("connector_telegram").is_some());
        assert_eq!(
            pc_secrets.get("connector:telegram:token").as_deref(),
            Some("123:not-a-real-token"),
            "the bot stays"
        );

        // The home has a bot of its own (or took this one): the home's settings win, this one goes.
        let keep = Keep { tables: &["api_keys"], own: true, telegram: false };
        import_keeping(&pc, &pc_secrets, &theirs, false, keep).unwrap();
        assert_eq!(pc_secrets.get("connector:telegram:token"), None);
        assert_eq!(ids(&pc, "llm_providers"), ["lmstudio", "openrouter"], "the next copy keeps it the same");
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
        assert!(apply_incoming(dir.path(), &db, &secrets, true, None).is_some());
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
        assert!(apply_incoming(waiting.path(), &held, &secrets, true, None).is_none());
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
        let package = PluginsPackage {
            from: "station".into(),
            at: now_iso(),
            plugins: plugins(&station).unwrap(),
            pictures: Vec::new(),
        };
        std::fs::write(incoming_plugins_file(dir.path()), serde_json::to_vec(&package).unwrap()).unwrap();
        assert!(apply_incoming(dir.path(), &pc, &pc_secrets, false, None).is_none(), "no settings came");
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
