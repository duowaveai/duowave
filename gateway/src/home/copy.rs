//! The home copy (docs/plans/home-and-devices.md, stage 3). The centre is the only one that writes the home; a device
//! that can answer by itself — the computer that is the home's Турбо now, a second station later (stage 4) — keeps a
//! copy of it and answers from it while the centre is away.
//!
//! - **Settings, people, keys, characters, connections, rooms** — the settings package (src/placement/transfer.rs) as
//!   one section: the centre says its revision in /health; a new one is taken with the pair's key and applied here at
//!   once, without a restart (`take`). The computer's own things (`transfer::LOCAL_SETTINGS`) stay its own.
//! - **Memory** — merged both ways by record: a record keeps its id, when it was made and changed and who said it, so
//!   the newer change of a record wins and the rest add up (`memory_since`, `merge_memory`; `GET|POST
//!   /api/v1/home/memory`). What the computer remembered away from home reaches the centre when it is back.
//! - **Changes made here while the centre is away** — queued and sent in order when it is back (src/home/queue.rs):
//!   in the settings the last change wins, the one before stays in the centre's history.
//!
//! The centre keeps who holds a copy and of which revision (`home_copies`, its own setting): the panel marks those
//! devices «есть копия дома» (src/home/devices.rs).
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::Router;
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::app::App;
use crate::core::events::{EventBus, GatewayEvent};
use crate::core::settings::{SettingDefinition, SettingScope};
use crate::placement::peer::{from_peer, PeerClient};
use crate::placement::transfer;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller};
use crate::secrets::SecretsService;
use crate::settings::SettingsService;
use crate::storage::Db;
use crate::utils::time::now_iso;

/// The memory's tables in the order they are merged (an entity before its facts), each with its time column — the
/// newer row of a record wins; none — a link row, added once.
pub const MEMORY_TABLES: &[(&str, Option<&str>)] = &[
    ("memory_entities", Some("updated_at")),
    ("memory_aliases", None),
    ("memory_facts", Some("updated_at")),
    ("memory_visible_to", None),
    ("memory_episodes", Some("at")),
    ("memory_notes", Some("updated_at")),
];

/// Who holds a copy of the home, at the centre: a gateway's id → the revision it took and when. The centre's own
/// (`transfer::LOCAL_SETTINGS`): it never travels and changes no revision.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyHolder {
    pub revision: u64,
    pub at: String,
    /// A station's ear (stage 4): its station's device and the port its gateway listens on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Where it came from on the home network: «Сделать центром» reaches it there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

pub const HOME_COPIES: SettingDefinition<BTreeMap<String, CopyHolder>> = SettingDefinition {
    key: "home_copies",
    scope: SettingScope::Global,
    description: "Копии дома",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// The copy holder's own marks: the revision it took, and how far the memory went each way.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyState {
    #[serde(default)]
    pub revision: Option<u64>,
    #[serde(default)]
    pub taken_at: Option<String>,
    /// The centre's time of the newest memory taken from it.
    #[serde(default)]
    pub memory_from: Option<String>,
    /// This side's time of the newest memory sent to the centre.
    #[serde(default)]
    pub memory_to: Option<String>,
}

pub const HOME_COPY: SettingDefinition<CopyState> = SettingDefinition {
    key: "home_copy",
    scope: SettingScope::Global,
    description: "Копия дома",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// The Telegram bot a device runs: its setting and its token (src/connectors/settings.rs).
const TELEGRAM_SETTING: &str = "connector_telegram";
const TELEGRAM_SECRET: &str = "connector:telegram:token";

/// How long a holder's copy counts as kept: it takes the centre's new revisions within a poll, so a day without a word
/// from it means it is gone or away for long.
pub const COPY_FRESH: Duration = Duration::from_secs(24 * 3600);

fn columns(db: &rusqlite::Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    db.prepare(&format!("PRAGMA table_info({table})"))?.query_map([], |row| row.get::<_, String>(1))?.collect()
}

fn to_json(value: SqlValue) -> Value {
    match value {
        SqlValue::Null => Value::Null,
        SqlValue::Integer(number) => json!(number),
        SqlValue::Real(number) => json!(number),
        SqlValue::Text(text) => Value::String(text),
        SqlValue::Blob(bytes) => Value::String(String::from_utf8_lossy(&bytes).into_owned()),
    }
}

fn to_sql(value: &Value) -> SqlValue {
    match value {
        Value::Null => SqlValue::Null,
        Value::Bool(flag) => SqlValue::Integer(i64::from(*flag)),
        Value::Number(number) => match number.as_i64() {
            Some(integer) => SqlValue::Integer(integer),
            None => SqlValue::Real(number.as_f64().unwrap_or_default()),
        },
        Value::String(text) => SqlValue::Text(text.clone()),
        other => SqlValue::Text(other.to_string()),
    }
}

/// The memory's records changed after `since` (the time columns' RFC 3339 text), all when none; link rows whole.
pub fn memory_since(db: &Db, since: Option<&str>) -> rusqlite::Result<Map<String, Value>> {
    db.with(|connection| {
        let mut out = Map::new();
        for (table, time) in MEMORY_TABLES {
            let names = columns(connection, table)?;
            let list = names.join(", ");
            let (sql, filter) = match (time, since) {
                (Some(time), Some(since)) => (format!("SELECT {list} FROM {table} WHERE {time} > ?1"), Some(since)),
                _ => (format!("SELECT {list} FROM {table}"), None),
            };
            let mut statement = connection.prepare(&sql)?;
            let read = |row: &rusqlite::Row<'_>| {
                let mut object = Map::new();
                for (index, name) in names.iter().enumerate() {
                    object.insert(name.clone(), to_json(row.get::<_, SqlValue>(index)?));
                }
                Ok(Value::Object(object))
            };
            let rows: Vec<Value> = match filter {
                Some(since) => statement.query_map(params![since], read)?.collect::<rusqlite::Result<_>>()?,
                None => statement.query_map([], read)?.collect::<rusqlite::Result<_>>()?,
            };
            out.insert(table.to_string(), Value::Array(rows));
        }
        Ok(out)
    })
}

/// The newest time among the records given: how far a merge went.
pub fn newest(rows: &Map<String, Value>) -> Option<String> {
    MEMORY_TABLES
        .iter()
        .filter_map(|(table, time)| Some((rows.get(*table)?.as_array()?, (*time)?)))
        .flat_map(|(rows, time)| rows.iter().filter_map(move |row| row.get(time)?.as_str().map(str::to_string)))
        .max()
}

/// Merges records from another device: a record not here is added; one here is replaced only by a newer change; a link
/// row is added once. What the other side does not know of a table (a newer column here) stays as its default.
pub fn merge_memory(db: &Db, rows: &Map<String, Value>) -> rusqlite::Result<usize> {
    db.transaction(|transaction| {
        let mut merged = 0;
        for (table, time) in MEMORY_TABLES {
            let Some(list) = rows.get(*table).and_then(Value::as_array) else { continue };
            let known = columns(transaction, table)?;
            for row in list {
                let Some(row) = row.as_object() else { continue };
                let names: Vec<&String> = row.keys().filter(|name| known.contains(name)).collect();
                if names.is_empty() {
                    continue;
                }
                let placeholders = (1..=names.len()).map(|at| format!("?{at}")).collect::<Vec<_>>().join(", ");
                let list = names.iter().map(|name| name.as_str()).collect::<Vec<_>>().join(", ");
                let values: Vec<SqlValue> = names.iter().map(|name| to_sql(&row[*name])).collect();
                let sql = match time {
                    Some(time) => {
                        let updates = names
                            .iter()
                            .filter(|name| name.as_str() != "id")
                            .map(|name| format!("{name} = excluded.{name}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!(
                            "INSERT INTO {table} ({list}) VALUES ({placeholders}) ON CONFLICT (id) DO UPDATE SET \
                             {updates} WHERE excluded.{time} > {table}.{time}"
                        )
                    }
                    None => format!("INSERT OR IGNORE INTO {table} ({list}) VALUES ({placeholders})"),
                };
                merged += transaction.execute(&sql, rusqlite::params_from_iter(values))?;
            }
        }
        Ok(merged)
    })
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/home/memory", get(memory_out).post(memory_in)).route("/api/v1/home/held", post(held))
}

/// Only the pair's key, from the home network: the copy holder of this home.
fn holder(app: &App, caller: &Caller) -> Result<(), ApiError> {
    if from_peer(caller, app.placement.peer_key()) {
        return Ok(());
    }
    Err(ApiError::forbidden(crate::tr!("Копию дома берёт только устройство этого дома с ключом пары.")))
}

#[derive(Deserialize)]
struct Since {
    since: Option<String>,
}

/// The centre's memory changed after `since`, and its time now: the holder asks from there next.
async fn memory_out(State(app): State<Arc<App>>, caller: Caller, Query(query): Query<Since>) -> ApiResult<Value> {
    holder(&app, &caller)?;
    let rows = memory_since(&app.db, query.since.as_deref())?;
    Ok(ok(json!({ "rows": rows, "newest": newest(&rows) })))
}

/// What the holder remembered — away from home too — merged into the home's memory.
async fn memory_in(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<Value> {
    holder(&app, &caller)?;
    let rows = body.get("rows").and_then(Value::as_object).cloned().unwrap_or_default();
    let merged = merge_memory(&app.db, &rows)?;
    if merged > 0 {
        tracing::info!(target: "home", "memory: {merged} record(s) from the home's copy");
        app.events.emit(GatewayEvent::MemoryChanged {
            action: "merge".into(),
            target: "copy".into(),
            target_id: String::new(),
            actor: None,
        });
    }
    Ok(ok(json!({ "merged": merged })))
}

#[derive(Deserialize)]
struct Held {
    gateway: String,
    revision: u64,
    /// An ear's (stage 4): its station, its port and its pair key — «Сделать центром» asks it with them.
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    key: Option<String>,
}

/// A holder took this revision of the home.
async fn held(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<Value> {
    holder(&app, &caller)?;
    let held: Held =
        serde_json::from_value(body).map_err(|_| ApiError::bad_request("{gateway, revision}".to_string()))?;
    if held.gateway.is_empty() || held.gateway.len() > 64 {
        return Err(ApiError::bad_request("gateway".to_string()));
    }
    let mut copies = app.settings.get(&HOME_COPIES, None).unwrap_or_default();
    let device = held.device.clone().filter(|device| !device.is_empty() && device.len() <= 128);
    if let (Some(device), Some(key)) = (&device, held.key.as_deref().filter(|key| key.len() >= 16)) {
        app.secrets.set(&super::ear::ear_key_secret(device), Some(key))?;
    }
    let address = Some(caller.source.clone()).filter(|address| !address.is_empty());
    copies.insert(
        held.gateway.clone(),
        CopyHolder { revision: held.revision, at: now_iso(), device, port: held.port, address },
    );
    app.settings.set(&HOME_COPIES, &copies, None)?;
    app.events.emit(GatewayEvent::HomeDevicesChanged { reason: "copy".into() });
    Ok(ok(json!({ "ok": true })))
}

/// Whether this gateway holds a fresh copy for the centre's list: it took a revision within `COPY_FRESH`.
pub fn holds(settings: &SettingsService, gateway: &str) -> bool {
    settings
        .get(&HOME_COPIES, None)
        .and_then(|copies| copies.get(gateway).cloned())
        .is_some_and(|holder| fresh(&holder))
}

/// The holder that is this station's ear (stage 4), whatever its copy's age.
pub fn holder_of_device(settings: &SettingsService, device: &str) -> Option<CopyHolder> {
    settings.get(&HOME_COPIES, None)?.into_values().find(|holder| holder.device.as_deref() == Some(device))
}

/// Whether this station's ear holds a fresh copy.
pub fn device_holds(settings: &SettingsService, device: &str) -> bool {
    holder_of_device(settings, device).is_some_and(|holder| fresh(&holder))
}

fn fresh(holder: &CopyHolder) -> bool {
    chrono::DateTime::parse_from_rfc3339(&holder.at).is_ok_and(|at| {
        chrono::Utc::now().signed_duration_since(at.with_timezone(&chrono::Utc)).to_std().unwrap_or_default()
            < COPY_FRESH
    })
}

/// The copy holder's side, each look at the centre (src/placement/watch.rs): the settings of a new revision applied
/// here at once, the memory merged both ways, the changes queued while the centre was away sent. Each part on its own:
/// one failing leaves the others.
pub struct Keeper<'a> {
    pub db: &'a Db,
    pub secrets: &'a SecretsService,
    pub settings: &'a SettingsService,
    pub events: &'a EventBus,
    pub client: &'a PeerClient,
    /// The centre's revision by its /health.
    pub revision: Option<u64>,
    /// This gateway's own id: the centre counts it among the holders.
    pub me: Option<String>,
}

impl Keeper<'_> {
    pub async fn keep(&self) -> Result<(), String> {
        let mut problems = Vec::new();
        if let Err(error) = self.take().await {
            problems.push(error);
        }
        if let Err(error) = self.memory().await {
            problems.push(error);
        }
        if let Err(error) = super::queue::send(self.db, self.client).await {
            problems.push(error);
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    fn state(&self) -> CopyState {
        self.settings.get(&HOME_COPY, None).unwrap_or_default()
    }

    fn store(&self, state: &CopyState) {
        if let Err(error) = self.settings.set(&HOME_COPY, state, None) {
            tracing::warn!(target: "home", "the copy's marks not kept: {}", error.message);
        }
    }

    /// The centre's settings of a new revision, applied here now: no restart — every reader takes them from the
    /// database, and those that keep their own view hear `home.copied`.
    async fn take(&self) -> Result<(), String> {
        let Some(revision) = self.revision else { return Ok(()) };
        let mut state = self.state();
        if state.revision == Some(revision) {
            return Ok(());
        }
        let mut bundle = self.client.settings().await?;
        // A bot this device runs and the home lacks goes to the home first (stage 2: Telegram is the home's): asked
        // once, then the copy carries it back. A home with a bot of its own keeps it — the copy brings that one; a
        // centre that does not take this one (older, or away a moment) leaves it here as it is, asked again at its
        // next revision.
        let token = self.secrets.get(TELEGRAM_SECRET).filter(|token| !token.trim().is_empty());
        let home_has = bundle.secrets.get(TELEGRAM_SECRET).is_some_and(|token| !token.trim().is_empty());
        let mut keep_bot = false;
        if let (Some(token), false) = (token, home_has) {
            let mut body = self.settings.raw(TELEGRAM_SETTING).filter(Value::is_object).unwrap_or_else(|| json!({}));
            body["token"] = json!(token);
            match self.client.post("/api/v1/home/telegram-adopt", body).await {
                Ok(_) => {
                    tracing::info!(target: "home", "this device's Telegram bot is the home's now");
                    bundle = self.client.settings().await?;
                }
                Err(error) => {
                    tracing::info!(target: "home", "the home did not take this device's Telegram bot: {error}");
                    keep_bot = true;
                }
            }
        }
        // Its own access keys stay — the centre reaches this computer as Турбо with one of them —, and so do its own
        // models, programs and, while the home has none, its bot.
        let keep = transfer::Keep { tables: &["api_keys"], own: true, telegram: keep_bot };
        let imported = transfer::import_keeping(self.db, self.secrets, &bundle, false, keep)?;
        transfer::note_sync(self.db, "copied");
        state.revision = Some(bundle.revision.max(revision));
        state.taken_at = Some(now_iso());
        self.store(&state);
        tracing::info!(
            target: "home",
            "the home's copy: revision {revision}, {} setting(s), {} secret(s)",
            imported.settings,
            imported.secrets
        );
        self.events.emit(GatewayEvent::HomeCopied { revision });
        if let Some(me) = &self.me {
            self.client.post("/api/v1/home/held", json!({ "gateway": me, "revision": revision })).await?;
        }
        Ok(())
    }

    /// The memory both ways: the centre's news merged here, then this side's news merged there.
    async fn memory(&self) -> Result<(), String> {
        let mut state = self.state();
        let path = match &state.memory_from {
            Some(since) => format!("/api/v1/home/memory?since={}", urlencoding(since)),
            None => "/api/v1/home/memory".to_string(),
        };
        let answer = self.client.get_long(&path).await?;
        let rows = answer.get("rows").and_then(Value::as_object).cloned().unwrap_or_default();
        let merged = merge_memory(self.db, &rows).map_err(|error| error.to_string())?;
        if let Some(newest) = answer.get("newest").and_then(Value::as_str) {
            state.memory_from = Some(newest.to_string());
        }
        if merged > 0 {
            self.events.emit(GatewayEvent::MemoryChanged {
                action: "merge".into(),
                target: "copy".into(),
                target_id: String::new(),
                actor: None,
            });
        }
        let ours = memory_since(self.db, state.memory_to.as_deref()).map_err(|error| error.to_string())?;
        let sent_up_to = newest(&ours);
        let any = ours.values().filter_map(Value::as_array).any(|rows| !rows.is_empty());
        if any && state.memory_to.is_none() || sent_up_to.as_deref() > state.memory_to.as_deref() {
            self.client.post("/api/v1/home/memory", json!({ "rows": ours })).await?;
            state.memory_to = sent_up_to.or(state.memory_to);
        }
        self.store(&state);
        Ok(())
    }
}

/// A query's value: the few characters an RFC 3339 time has that a query must escape.
fn urlencoding(text: &str) -> String {
    text.replace('%', "%25").replace('+', "%2B").replace(':', "%3A")
}

/// The home copy for the services that keep their own view of the settings: they read them again.
pub fn watch(app: &Arc<App>) {
    let weak = Arc::downgrade(app);
    app.events.on("home.copy", move |event| {
        if !matches!(&*event, GatewayEvent::HomeCopied { .. }) {
            return;
        }
        let Some(app) = weak.upgrade() else { return };
        app.connector_settings.read_again();
        app.characters.sync();
    });
}

/// The revision this gateway's copy was taken at, if it holds one.
pub fn copied_revision(settings: &SettingsService) -> Option<u64> {
    settings.get(&HOME_COPY, None).and_then(|state| state.revision)
}

/// When the copy was last taken.
pub fn copied_at(db: &Db) -> Option<String> {
    db.with(|connection| {
        connection
            .query_row("SELECT updated_at FROM state WHERE kind = 'settings-sync' AND device = 'copied'", [], |row| {
                row.get(0)
            })
            .optional()
    })
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;

    fn entity(id: &str, name: &str, at: &str) -> Value {
        json!({ "id": id, "kind": "person", "name": name, "created_at": at, "updated_at": at })
    }

    fn fact(id: &str, subject: &str, text: &str, at: &str) -> Value {
        json!({
            "id": id, "subject_id": subject, "text": text, "lang": "ru", "source": "voice", "said_by_kind": "user",
            "channel": "station", "audience": "speaker", "created_at": at, "updated_at": at,
        })
    }

    fn rows(entities: Vec<Value>, facts: Vec<Value>) -> Map<String, Value> {
        let mut rows = Map::new();
        rows.insert("memory_entities".into(), Value::Array(entities));
        rows.insert("memory_facts".into(), Value::Array(facts));
        rows
    }

    /// The project's rule for a migration (CLAUDE.md, «Миграции БД в worktree»): run it on a copy of a live database —
    /// taken with `sqlite3 <db> ".backup '<copy>'"` into a folder of its own, never the live one. Opened by the same code
    /// as the gateway's start, without starting anything that goes to the network. `DUOWAVE_DB_COPY=<the copy's state
    /// folder> cargo test -p duowave --lib home::copy -- --ignored`.
    #[test]
    #[ignore = "on a copy of a live database: DUOWAVE_DB_COPY=<its state folder>"]
    fn the_home_migration_on_a_copy_of_a_live_database() {
        let Some(dir) = std::env::var_os("DUOWAVE_DB_COPY") else { return };
        let dir = std::path::PathBuf::from(dir);
        let before = |db: &Db, sql: &str| -> i64 { db.with(|c| c.query_row(sql, [], |row| row.get(0))).unwrap() };
        let db = Db::open(Some(&dir)).expect("the copy opens and migrates");
        assert_eq!(before(&db, "SELECT MAX(version) FROM schema_migrations"), 16, "0016 applied");
        let columns: Vec<String> = db.with(|c| columns(c, "home_queue")).unwrap();
        assert!(columns.contains(&"person_id".to_string()) && columns.contains(&"refused".to_string()), "{columns:?}");
        // The data before is there and reads.
        let settings = before(&db, "SELECT COUNT(*) FROM settings");
        let users = before(&db, "SELECT COUNT(*) FROM users");
        assert!(settings > 0 && users > 0, "settings {settings}, people {users}");
        // The queue works on it, and leaves nothing behind.
        let body = Bytes::from_static(b"{\"days\":9}");
        let id = super::super::queue::keep(
            &db,
            &super::super::queue::Change {
                method: "PUT",
                path: "/api/v1/requests/keep",
                content_type: Some("application/json"),
                body: &body,
                person: "Проверка",
                person_id: None,
                role: "admin",
            },
        )
        .unwrap();
        assert_eq!(super::super::queue::list(&db).len(), 1);
        db.with(|c| c.execute("DELETE FROM home_queue WHERE id = ?1", params![id])).unwrap();
        // The memory section reads its memory, and a merge of the same changes nothing.
        let rows = memory_since(&db, None).unwrap();
        assert_eq!(merge_memory(&db, &rows).unwrap(), 0, "the same records: nothing written");
        assert_eq!(before(&db, "SELECT COUNT(*) FROM settings"), settings);
        eprintln!("copy: 0016 applied; settings {settings}, people {users}; queue and memory work");
    }

    #[test]
    fn memory_adds_up_and_the_newer_change_of_a_record_wins() {
        let home = Db::memory();
        let away = Db::memory();
        // At home: Маша doesn't eat nuts.
        merge_memory(
            &home,
            &rows(
                vec![entity("e1", "Маша", "2026-11-17T10:00:00Z")],
                vec![fact("f1", "e1", "Маша не ест орехи", "2026-11-17T10:00:00Z")],
            ),
        )
        .unwrap();
        // The computer took the home's memory, then away it learned to buy milk and Маша's fact changed later.
        let taken = memory_since(&home, None).unwrap();
        merge_memory(&away, &taken).unwrap();
        merge_memory(
            &away,
            &rows(
                vec![entity("e2", "Дмитрий", "2026-11-17T12:00:00Z")],
                vec![
                    fact("f2", "e2", "Купить молоко", "2026-11-17T12:00:00Z"),
                    fact("f1", "e1", "Маша не ест орехи и арахис", "2026-11-17T12:05:00Z"),
                ],
            ),
        )
        .unwrap();
        // Back home: what changed away after the last exchange goes to the centre.
        let news = memory_since(&away, Some("2026-11-17T11:00:00Z")).unwrap();
        assert_eq!(newest(&news).as_deref(), Some("2026-11-17T12:05:00Z"));
        merge_memory(&home, &news).unwrap();
        let text = |db: &Db, id: &str| -> String {
            db.with(|c| c.query_row("SELECT text FROM memory_facts WHERE id = ?1", params![id], |row| row.get(0)))
                .unwrap()
        };
        assert_eq!(text(&home, "f2"), "Купить молоко", "the milk is home");
        assert_eq!(text(&home, "f1"), "Маша не ест орехи и арахис", "the newer change wins");
        // An older change never overwrites a newer one.
        merge_memory(&home, &rows(vec![], vec![fact("f1", "e1", "старое", "2026-11-17T09:00:00Z")])).unwrap();
        assert_eq!(text(&home, "f1"), "Маша не ест орехи и арахис");
    }
}
