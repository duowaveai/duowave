//! The station's history on the computer (docs/plans/package-history.md, «Колонка и компьютер»): the history belongs
//! to the app, and the gateway that keeps it whole is the computer's.
//!
//! - The station sends what is new a minute after it was written: first which objects the computer lacks
//!   (`/home/v1/history/have`), then those (`…/objects`), then its lines and marks (`…/lines`). The computer keeps the
//!   station's marks apart (`remotes/<station>/…`) and never lets them over its own; the station's records join the
//!   app's line on the computer by their time.
//! - A step back on the station to a version it has already cleaned takes the objects from the computer first
//!   (`…/fetch`).
//! - Like the rest of `/home/v1`: only from the home network and only with the home key — the way the station's speech
//!   and its talks with the model already go.
//! - What the computer has is noted in `synced`: the station's cleaning with «На компьютере» takes only that.
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde_json::{json, Map, Value};

use super::gc::Place;
use super::store::valid_name;
use super::History;
use crate::plugins::controller::HomeLink;

/// Ids asked about in one request; objects sent in one, at most this many bytes deflated.
const IDS_A_REQUEST: usize = 2000;
const BYTES_A_REQUEST: usize = 2 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
/// How often the background work looks: the sending, the room.
const TICK: Duration = Duration::from_secs(60);
/// The data kept and the cleaning: once a day, the first a quarter of an hour after the start.
const DAILY: Duration = Duration::from_secs(24 * 3600);
const FIRST_DAILY: Duration = Duration::from_secs(15 * 60);

#[derive(Default)]
pub struct SyncState {
    /// Something new waits to be sent.
    dirty: AtomicBool,
    /// Something was written: the room is looked at.
    grown: AtomicBool,
    home: OnceLock<Arc<dyn HomeLink>>,
    /// The objects the computer has (read from `synced` once).
    sent: Mutex<Option<HashSet<String>>>,
}

impl SyncState {
    pub fn dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
        self.grown.store(true, Ordering::Relaxed);
    }

    pub fn home_up(&self) -> bool {
        self.home.get().is_some_and(|home| home.up().is_some())
    }

    fn home(&self) -> Option<(String, String)> {
        self.home.get().and_then(|home| home.up())
    }
}

fn client() -> reqwest::Client {
    crate::utils::http::client()
}

impl History {
    pub fn set_home(&self, home: Arc<dyn HomeLink>) {
        let _ = self.sync.home.set(home);
    }

    fn synced_path(&self) -> std::path::PathBuf {
        self.store().root().join("synced")
    }

    fn with_sent<T>(&self, work: impl FnOnce(&mut HashSet<String>) -> T) -> T {
        let mut sent = self.sync.sent.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let set = sent.get_or_insert_with(|| {
            std::fs::read_to_string(self.synced_path())
                .unwrap_or_default()
                .lines()
                .map(str::trim)
                .filter(|id| super::store::valid_id(id))
                .map(str::to_string)
                .collect()
        });
        work(set)
    }

    /// Whether the computer has the object.
    pub(crate) fn was_sent(&self, id: &str) -> bool {
        self.with_sent(|sent| sent.contains(id))
    }

    fn note_sent(&self, ids: &[String]) {
        self.with_sent(|sent| {
            sent.extend(ids.iter().cloned());
            // Only what is still here is worth remembering.
            let here: HashSet<String> = self.store().objects().into_iter().map(|(id, _)| id).collect();
            sent.retain(|id| here.contains(id));
            let mut text = sent.iter().cloned().collect::<Vec<_>>().join("\n");
            text.push('\n');
            if let Err(error) = super::store::write_whole(&self.synced_path(), text.as_bytes()) {
                tracing::warn!(target: "plugins", "history: what the computer has is not noted: {error}");
            }
        });
    }

    /// The background work: what is on disk recorded at the start; the sending to the computer; the room; once a day
    /// the plugins' data kept and the cleaning.
    pub fn start(self: &Arc<Self>) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
        let me = Arc::downgrade(self);
        handle.spawn(async move {
            if let Some(history) = me.upgrade() {
                let _ = tokio::task::spawn_blocking(move || history.adopt()).await;
            }
            let mut next_daily = Instant::now() + FIRST_DAILY;
            let mut tick = tokio::time::interval(TICK);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let Some(history) = me.upgrade() else { return };
                if history.sync.dirty.swap(false, Ordering::Relaxed) {
                    if let Err(why) = history.push().await {
                        history.sync.dirty.store(true, Ordering::Relaxed);
                        tracing::info!(target: "plugins", "history: not sent to the computer: {why}");
                    }
                }
                let daily = Instant::now() >= next_daily;
                let grown = history.sync.grown.swap(false, Ordering::Relaxed);
                if daily || grown {
                    let work = history.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        if daily {
                            let kept = work.keep_data();
                            if kept > 0 {
                                tracing::info!(target: "plugins", "history: the data of {kept} plugin(s) kept");
                            }
                        }
                        let policy = work.policy();
                        if daily || work.store().bytes() > policy.budget_mb * 1024 * 1024 {
                            work.clean();
                        }
                    })
                    .await;
                    if daily {
                        next_daily = Instant::now() + DAILY;
                    }
                }
            }
        });
    }

    /// The station's new objects, lines and marks to the computer. How many objects went.
    pub async fn push(&self) -> Result<usize, String> {
        if !self.on_station() || self.policy().place == Place::Here {
            return Ok(0);
        }
        let Some((url, key)) = self.sync.home() else { return Ok(0) };
        let url = url.trim_end_matches('/').to_string();
        let fresh: Vec<String> =
            self.store().objects().into_iter().map(|(id, _)| id).filter(|id| !self.was_sent(id)).collect();
        let mut missing = Vec::new();
        for chunk in fresh.chunks(IDS_A_REQUEST) {
            let answer = post(&url, &key, "have", json!({ "ids": chunk })).await?;
            missing.extend(
                answer
                    .get("missing")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string),
            );
        }
        let mut batch = Vec::new();
        let mut bytes = 0;
        let mut sent = 0;
        for id in &missing {
            let Some(packed) = self.store().packed(id) else { continue };
            bytes += packed.len();
            batch.push(json!({ "id": id, "packed": B64.encode(&packed) }));
            if bytes >= BYTES_A_REQUEST {
                post(&url, &key, "objects", json!({ "objects": batch })).await?;
                sent += batch.len();
                (batch, bytes) = (Vec::new(), 0);
            }
        }
        if !batch.is_empty() {
            post(&url, &key, "objects", json!({ "objects": batch })).await?;
            sent += batch.len();
        }
        let mut apps = Map::new();
        for app in self.store().apps() {
            apps.insert(app.clone(), json!({ "marks": self.store().refs(&app), "log": self.store().log(&app) }));
        }
        post(&url, &key, "lines", json!({ "apps": apps })).await?;
        self.note_sent(&fresh);
        if sent > 0 {
            tracing::info!(target: "plugins", "history: {sent} object(s) sent to the computer");
        }
        Ok(sent)
    }

    /// Objects this side lacks, from the computer: how many came.
    pub async fn fetch(&self, ids: &[String]) -> usize {
        let wanted: Vec<&String> = ids.iter().filter(|id| !self.store().has(id)).collect();
        if wanted.is_empty() || !self.on_station() {
            return 0;
        }
        let Some((url, key)) = self.sync.home() else { return 0 };
        let answer = match post(url.trim_end_matches('/'), &key, "fetch", json!({ "ids": wanted })).await {
            Ok(answer) => answer,
            Err(why) => {
                tracing::info!(target: "plugins", "history: nothing from the computer: {why}");
                return 0;
            }
        };
        let mut came = 0;
        for object in answer.get("objects").and_then(Value::as_array).into_iter().flatten() {
            let (Some(id), Some(packed)) = (
                object.get("id").and_then(Value::as_str),
                object.get("packed").and_then(Value::as_str).and_then(|text| B64.decode(text).ok()),
            ) else {
                continue;
            };
            if self.store().put_packed(id, &packed).is_ok() {
                came += 1;
            }
        }
        came
    }

    /// A record and all it points to here, taken from the computer when this side cleaned them.
    pub async fn ensure_local(&self, record: &str) {
        if !self.on_station() || self.sync.home().is_none() {
            return;
        }
        self.fetch(&[record.to_string()]).await;
        let Some(found) = self.record(record) else { return };
        let mut wanted = vec![found.tree.clone()];
        wanted.extend(found.data.iter().cloned());
        self.fetch(&wanted).await;
        let mut files: Vec<String> =
            self.tree(&found.tree).unwrap_or_default().into_iter().map(|entry| entry.file).collect();
        if let Some(snapshot) = found.data.as_deref().and_then(|data| super::data::DataParts::read(self.store(), data))
        {
            files.extend(snapshot.pictures.into_iter().map(|picture| picture.file));
        }
        self.fetch(&files).await;
    }

    // The computer's side.

    /// Of these objects, the ones not here.
    pub fn missing(&self, ids: &[String]) -> Vec<String> {
        ids.iter().filter(|id| super::store::valid_id(id) && !self.store().has(id)).cloned().collect()
    }

    /// Objects from the station: each kept only when its name is its hash.
    pub fn receive(&self, objects: &[Value]) -> usize {
        let _writing = self.writing();
        objects
            .iter()
            .filter_map(|object| {
                let id = object.get("id").and_then(Value::as_str)?;
                let packed = B64.decode(object.get("packed").and_then(Value::as_str)?).ok()?;
                self.store().put_packed(id, &packed).ok()
            })
            .count()
    }

    /// The station's lines and marks: its marks apart, its records into each app's line by their time.
    pub fn receive_lines(&self, peer: &str, apps: &Map<String, Value>) -> Result<usize, String> {
        if !valid_name(peer) {
            return Err(crate::tr!("имя колонки не то"));
        }
        let _writing = self.writing();
        let mut joined = 0;
        for (app, line) in apps {
            if !valid_name(app) {
                continue;
            }
            for (name, id) in line.get("marks").and_then(Value::as_object).into_iter().flatten() {
                if let Some(id) = id.as_str().filter(|id| self.store().has(id)) {
                    let _ = self.store().remote_ref_set(Some(peer), app, name, Some(id));
                }
            }
            let lock = self.lock(app);
            let _held = super::held(&lock);
            let mut ids = self.store().log(app);
            let known: HashSet<String> = ids.iter().cloned().collect();
            let theirs: Vec<String> = line
                .get("log")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|id| !known.contains(*id) && self.record(id).is_some_and(|record| &record.app == app))
                .map(str::to_string)
                .collect();
            if theirs.is_empty() {
                continue;
            }
            joined += theirs.len();
            ids.extend(theirs);
            let mut dated: Vec<(String, String)> =
                ids.into_iter().map(|id| (self.record(&id).map(|record| record.at).unwrap_or_default(), id)).collect();
            dated.sort_by(|a, b| a.0.cmp(&b.0));
            let ids: Vec<String> = dated.into_iter().map(|(_, id)| id).collect();
            self.store().log_write(app, &ids).map_err(|error| error.to_string())?;
            drop(_held);
            self.tell(app, "history");
        }
        Ok(joined)
    }

    /// Objects the station asks for, deflated as they lie.
    pub fn give(&self, ids: &[String]) -> Vec<Value> {
        ids.iter()
            .take(IDS_A_REQUEST)
            .filter_map(|id| Some(json!({ "id": id, "packed": B64.encode(self.store().packed(id)?) })))
            .collect()
    }
}

async fn post(url: &str, key: &str, what: &str, body: Value) -> Result<Value, String> {
    let answer = client()
        .post(format!("{url}/home/v1/history/{what}"))
        .bearer_auth(key)
        .timeout(TIMEOUT)
        .json(&body)
        .send()
        .await
        .map_err(|error| crate::tr!("компьютер не ответил: {error}", error = error))?;
    let status = answer.status();
    let body: Value =
        answer.json().await.map_err(|_| crate::tr!("компьютер ответил не JSON (HTTP {status})", status = status))?;
    if !status.is_success() {
        return Err(body
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| crate::tr!("отказ")));
    }
    Ok(body.get("message").filter(|message| message.is_object()).cloned().unwrap_or(body))
}

#[cfg(test)]
mod tests {
    use super::super::tests::{history, write_draft};
    use super::super::{Author, RecordKind, StepMeta, Who};
    use super::*;

    #[test]
    fn the_computer_keeps_the_stations_marks_apart_and_its_records_in_the_apps_line_by_time() {
        let station_dir = tempfile::tempdir().unwrap();
        let station = history(station_dir.path());
        write_draft(station_dir.path(), "notes", &[("manifest.json", "{}")]);
        let step = station.step("notes", StepMeta::new(RecordKind::Found, Author::new(Who::System))).unwrap();

        let computer_dir = tempfile::tempdir().unwrap();
        let computer = history(computer_dir.path());
        let ids: Vec<String> = station.store().objects().into_iter().map(|(id, _)| id).collect();
        assert_eq!(computer.missing(&ids).len(), ids.len());
        let objects: Vec<Value> = ids
            .iter()
            .map(|id| json!({ "id": id, "packed": B64.encode(station.store().packed(id).unwrap()) }))
            .collect();
        assert_eq!(computer.receive(&objects), ids.len());
        assert!(computer.missing(&ids).is_empty());
        // A forged object is not kept.
        assert_eq!(computer.receive(&[json!({ "id": "0".repeat(64), "packed": B64.encode(b"x") })]), 0);

        let mut apps = Map::new();
        apps.insert(
            "notes".into(),
            json!({ "marks": station.store().refs("notes"), "log": station.store().log("notes") }),
        );
        assert_eq!(computer.receive_lines("kitchen", &apps).unwrap(), 1);
        assert_eq!(computer.store().log("notes"), vec![step.clone()]);
        assert_eq!(computer.store().remote_ref_get(Some("kitchen"), "notes", "draft"), Some(step.clone()));
        assert_eq!(computer.store().ref_get("notes", "draft"), None, "the station's marks are not the computer's");
        assert!(computer.receive_lines("../x", &apps).is_err());
        assert_eq!(computer.give(std::slice::from_ref(&step)).len(), 1);
    }
}
