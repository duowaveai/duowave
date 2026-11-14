//! The apps' history (docs/plans/package-history.md): every finished piece of work on an app kept by hash, as git
//! keeps it, and any version of it brought back — the code, its data or both.
//!
//! - **The store** (`store.rs`): files, the lists of an app's files (trees), records and the plugins' data snapshots
//!   as objects named by their SHA-256; the marks (`draft`, `installed`, `catalog`) and each app's line of records.
//! - **A step** is a finished piece of work on a draft — the agent's work on one request, a save by hand (saves by the
//!   same hand within two minutes are one step), a step back, a merge — written only when the files changed. An
//!   install, an update from the catalog, a copy, a step back of the installed app are records too, with the data the
//!   plugin had just before.
//! - **Forward only.** Bringing an old version back is a new record with the old content: nothing is rewritten, and
//!   the step back can be undone the same way. Only the cleaning (`gc.rs`) takes old records away.
//! - **Out of the way.** A record that does not get written says so in the log; the work goes on without it.
//! - **A record's `parents`** is a list from the start: one parent now, two for a merge — branches and merges come
//!   without a new format.

pub mod data;
pub mod diff;
pub mod gc;
pub mod merge;
pub mod restore;
pub mod routes;
pub mod store;
pub mod sync;
pub mod voice;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock, Weak};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::controller::PluginHost;
use super::registry::Origin;
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::settings::SettingsService;
use store::{Kind, Store};

/// A record's format.
pub const RECORD_V: u32 = 1;
/// The marks of an app's line.
pub const DRAFT: &str = "draft";
pub const INSTALLED: &str = "installed";
pub const CATALOG: &str = "catalog";
/// Saves by the same hand within this long are one step.
const COALESCE_MS: i64 = 2 * 60_000;
/// Files of an app at most, and all of them together: the installer's limits (install.rs).
const MAX_FILES: usize = super::install::MAX_FILES;
const MAX_BYTES: u64 = super::install::MAX_UNPACKED;
/// What a record keeps of a request and of the agent's last words.
const ASKED_MAX: usize = 600;
const NOTE_MAX: usize = 300;

/// A file of an app as a tree keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    pub file: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordKind {
    /// A finished piece of work on the draft.
    Step,
    /// The draft put in as a version (`trial` — for a trial).
    Install,
    /// An old version brought back: the draft's, or the installed app's (code, data or both — `what`).
    Restore,
    /// Two lines merged: two parents.
    Merge,
    /// A version from the catalog.
    Catalog,
    /// A copy of one's own of another app: its parent is the other app's record.
    Copy,
    /// Found on disk: a version or a draft the history had not seen (put in by hand, from before the history).
    Found,
    /// The plugin's data kept (once a day when it changed).
    Data,
    /// The app removed; its history lives on for a while.
    Removed,
}

impl RecordKind {
    /// Whether the record is a version of the installed app (what `installed` points to).
    pub fn installs(self) -> bool {
        matches!(self, RecordKind::Install | RecordKind::Catalog | RecordKind::Copy | RecordKind::Found)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Who {
    Agent,
    Hand,
    Voice,
    Mcp,
    Catalog,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    pub who: Who,
    /// The person (the panel's), the station (voice) — as this gateway knows them; it never leaves the gateways.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// How the request came: `panel`, `voice`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

impl Author {
    pub fn new(who: Who) -> Author {
        Author { who, name: None, via: None }
    }

    /// The panel's person, or the agent when nobody pressed (the agent's own `install`).
    pub fn of(actor: Option<&Actor>, otherwise: Who) -> Author {
        match actor {
            Some(actor) => Author { who: Who::Hand, name: Some(actor.name.clone()), via: Some("panel".into()) },
            None => Author::new(otherwise),
        }
    }
}

/// What a restore brought back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum What {
    #[serde(rename = "code")]
    Code,
    #[serde(rename = "data")]
    Data,
    #[serde(rename = "code+data")]
    CodeData,
}

impl What {
    pub fn parse(text: &str) -> Option<What> {
        match text {
            "code" => Some(What::Code),
            "data" => Some(What::Data),
            "code+data" | "both" => Some(What::CodeData),
            _ => None,
        }
    }

    pub fn code(self) -> bool {
        self != What::Data
    }

    pub fn data(self) -> bool {
        self != What::Code
    }
}

/// A record of an app's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub v: u32,
    pub app: String,
    pub tree: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parents: Vec<String>,
    /// RFC 3339, UTC, to the second.
    pub at: String,
    pub kind: RecordKind,
    pub by: Author,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// `ok` or `failed`: «Проверить» before an install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    /// The plugin's data just before the change the record is of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// The record brought back (a restore).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub what: Option<What>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub trial: bool,
    /// Where it was made: `station` or `computer`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
}

/// A record as the panel and the agent see it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordView {
    pub id: String,
    pub short: String,
    #[serde(flatten)]
    pub record: Record,
    /// The marks that point to it: `draft`, `installed`, `catalog`.
    pub marks: Vec<String>,
    /// It is in the draft's past: merging it brings nothing by itself (the feed's «Взять из неё…»).
    pub in_draft: bool,
}

/// An app's feed: its records, the newest first.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Feed {
    pub app: String,
    pub records: Vec<RecordView>,
    /// Older ones are there, past `limit`.
    pub more: bool,
    /// Older records were taken by the cleaning: «раньше — удалено по сроку».
    pub earlier_gone: bool,
    pub marks: BTreeMap<String, String>,
    /// For a copy of one's own: a newer version of the app it was copied from, to merge (merge.rs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<merge::Upstream>,
}

/// How a step is written.
#[derive(Debug, Clone)]
pub struct StepMeta {
    pub kind: RecordKind,
    pub by: Author,
    pub asked: Option<String>,
    pub note: Option<String>,
    pub from: Option<String>,
    /// Another parent besides the draft's last step (a merge's other side).
    pub other: Option<String>,
    /// A save by hand: one within two minutes of the same hand's last is the same step.
    pub coalesce: bool,
}

impl StepMeta {
    pub fn new(kind: RecordKind, by: Author) -> StepMeta {
        StepMeta { kind, by, asked: None, note: None, from: None, other: None, coalesce: false }
    }
}

/// A file of a record's tree, as the panel lists it.
#[derive(Debug, Clone, Serialize)]
pub struct FileView {
    pub path: String,
    pub size: u64,
}

/// What changed in a file between two versions.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    /// `added`, `removed`, `changed`.
    pub change: &'static str,
    /// Not text: no lines.
    pub binary: bool,
    pub lines: Vec<diff::Line>,
}

/// Two versions compared: files, and the manifest's changes in words.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub from: Option<String>,
    pub to: Option<String>,
    pub files: Vec<FileChange>,
    /// The manifest's changes as a person reads them: rights, hosts, phrases, the version.
    pub manifest: Vec<String>,
}

pub fn now_at() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn ms_of(at: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(at).ok().map(|at| at.timestamp_millis())
}

fn shortened(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max).collect();
    cut.push('…');
    cut
}

/// A path of a tree, checked: relative, plain parts, no way out.
pub fn plain_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path.split('/').all(|part| !part.is_empty() && part != "." && part != ".." && !part.starts_with('.'))
}

/// The files of a folder: relative paths, hidden ones left out.
fn walk(root: &Path, at: &Path, files: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(at)?.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(root, &entry.path(), files)?;
        } else if kind.is_file() {
            let relative =
                entry.path().strip_prefix(root).unwrap_or(&entry.path()).to_string_lossy().replace('\\', "/");
            files.push((relative, entry.path()));
        }
    }
    Ok(())
}

/// The service: the store, and what it reads the apps and their data from.
pub struct History {
    store: Store,
    /// The workshop's drafts: `<dev_dir>/<id>/`.
    dev_dir: PathBuf,
    host: OnceLock<Weak<PluginHost>>,
    data: OnceLock<data::DataParts>,
    events: OnceLock<EventBus>,
    settings: Option<Arc<SettingsService>>,
    /// One change of an app's line at a time.
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Writers share it; the cleaning takes it alone.
    gate: RwLock<()>,
    /// Where it runs: the station's limits (gc.rs).
    on_station: Arc<dyn Fn() -> bool + Send + Sync>,
    /// The other gateway: the station sends its history to the computer (sync.rs).
    pub(crate) sync: sync::SyncState,
}

impl History {
    /// The history in `dir`; None when its folder cannot be made (the gateway goes on without one).
    pub fn open(
        dir: PathBuf,
        dev_dir: PathBuf,
        settings: Option<Arc<SettingsService>>,
        on_station: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Option<Arc<History>> {
        match Store::open(dir.clone()) {
            Ok(store) => Some(Arc::new(History {
                store,
                dev_dir,
                host: OnceLock::new(),
                data: OnceLock::new(),
                events: OnceLock::new(),
                settings,
                locks: Mutex::new(HashMap::new()),
                gate: RwLock::new(()),
                on_station,
                sync: sync::SyncState::default(),
            })),
            Err(error) => {
                tracing::warn!(target: "plugins", "history: {} is not usable ({error}); the gateway goes on without it", dir.display());
                None
            }
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn set_host(&self, host: &Arc<PluginHost>) {
        let _ = self.host.set(Arc::downgrade(host));
    }

    pub fn set_data(&self, parts: data::DataParts) {
        let _ = self.data.set(parts);
    }

    pub fn set_events(&self, events: EventBus) {
        let _ = self.events.set(events);
    }

    pub(crate) fn host(&self) -> Option<Arc<PluginHost>> {
        self.host.get().and_then(Weak::upgrade)
    }

    pub(crate) fn data_parts(&self) -> Option<&data::DataParts> {
        self.data.get()
    }

    pub(crate) fn settings(&self) -> Option<&Arc<SettingsService>> {
        self.settings.as_ref()
    }

    pub fn on_station(&self) -> bool {
        (self.on_station)()
    }

    pub fn dev_dir(&self) -> &Path {
        &self.dev_dir
    }

    fn place(&self) -> String {
        if self.on_station() { "station" } else { "computer" }.to_string()
    }

    pub(crate) fn lock(&self, app: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        locks.entry(app.to_string()).or_default().clone()
    }

    pub(crate) fn writing(&self) -> std::sync::RwLockReadGuard<'_, ()> {
        self.gate.read().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn cleaning(&self) -> std::sync::RwLockWriteGuard<'_, ()> {
        self.gate.write().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn tell(&self, app: &str, change: &str) {
        if let Some(events) = self.events.get() {
            events.emit(GatewayEvent::WorkshopChanged { draft: app.to_string(), change: change.to_string() });
        }
        self.sync.dirty();
    }

    pub(crate) fn tell_packages(&self, app: &str, change: &str, version: Option<String>, actor: Option<Actor>) {
        if let Some(events) = self.events.get() {
            events.emit(GatewayEvent::PackagesChanged {
                change: change.into(),
                package_id: app.into(),
                version,
                error: None,
                actor,
            });
        }
        self.sync.dirty();
    }

    // Trees.

    /// A folder's files put in; its tree's id.
    pub fn snapshot_dir(&self, dir: &Path) -> Result<String, String> {
        let mut files = Vec::new();
        walk(dir, dir, &mut files).map_err(|error| crate::tr!("папка не читается: {error}", error = error))?;
        if files.len() > MAX_FILES {
            return Err(crate::tr!("больше {MAX_FILES} файлов", MAX_FILES = MAX_FILES));
        }
        let mut entries = Vec::new();
        let mut total = 0u64;
        for (path, full) in files {
            let bytes = std::fs::read(&full).map_err(|error| format!("{path}: {error}"))?;
            total += bytes.len() as u64;
            if total > MAX_BYTES {
                return Err(crate::tr!("больше {max_bytes} МБ", max_bytes = MAX_BYTES / 1024 / 1024));
            }
            let file = self.store.put(Kind::File, &bytes).map_err(|error| format!("{path}: {error}"))?;
            entries.push(TreeEntry { path, file, size: bytes.len() as u64 });
        }
        self.put_tree(entries)
    }

    pub(crate) fn put_tree(&self, mut entries: Vec<TreeEntry>) -> Result<String, String> {
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let bytes = serde_json::to_vec(&entries).map_err(|error| error.to_string())?;
        self.store.put(Kind::Tree, &bytes).map_err(|error| error.to_string())
    }

    pub fn tree(&self, id: &str) -> Option<Vec<TreeEntry>> {
        serde_json::from_slice(&self.store.get_kind(id, Kind::Tree)?).ok()
    }

    /// A file of a tree by its path.
    pub fn file_at(&self, tree: &str, path: &str) -> Option<Vec<u8>> {
        let entry = self.tree(tree)?.into_iter().find(|entry| entry.path == path)?;
        self.store.get_kind(&entry.file, Kind::File)
    }

    /// The tree's files written into `into`; `clean` — files of `into` not in the tree go (hidden ones stay).
    pub fn materialize(&self, tree: &str, into: &Path, clean: bool) -> Result<(), String> {
        let entries = self.tree(tree).ok_or_else(|| crate::tr!("этой версии в истории уже нет"))?;
        let mut contents = Vec::new();
        for entry in &entries {
            if !plain_path(&entry.path) {
                return Err(crate::tr!("путь «{path}» не годится", path = entry.path));
            }
            let bytes = self
                .store
                .get_kind(&entry.file, Kind::File)
                .ok_or_else(|| crate::tr!("файла {path} в истории уже нет", path = entry.path))?;
            contents.push((entry.path.clone(), bytes));
        }
        std::fs::create_dir_all(into).map_err(|error| error.to_string())?;
        if clean {
            let mut present = Vec::new();
            let _ = walk(into, into, &mut present);
            let keep: HashSet<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
            for (path, full) in present {
                if !keep.contains(path.as_str()) {
                    let _ = std::fs::remove_file(full);
                }
            }
            remove_empty_dirs(into);
        }
        for (path, bytes) in contents {
            let file = into.join(&path);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            if std::fs::read(&file).ok().as_deref() != Some(bytes.as_slice()) {
                std::fs::write(&file, &bytes).map_err(|error| format!("{path}: {error}"))?;
            }
        }
        Ok(())
    }

    /// The manifest of a tree, as JSON.
    pub fn manifest_of(&self, tree: &str) -> Option<Value> {
        serde_json::from_slice(&self.file_at(tree, "manifest.json")?).ok()
    }

    // Records.

    pub fn record(&self, id: &str) -> Option<Record> {
        serde_json::from_slice(&self.store.get_kind(id, Kind::Step)?).ok()
    }

    /// A record put in at the end of its app's line.
    pub(crate) fn write(&self, record: &Record) -> Result<String, String> {
        let bytes = serde_json::to_vec(record).map_err(|error| error.to_string())?;
        let id = self.store.put(Kind::Step, &bytes).map_err(|error| error.to_string())?;
        self.store.log_push(&record.app, &id).map_err(|error| error.to_string())?;
        Ok(id)
    }

    /// A new record of this app: its time, its place, the format.
    pub(crate) fn new_record(&self, app: &str, kind: RecordKind, tree: String, by: Author) -> Record {
        Record {
            v: RECORD_V,
            app: app.to_string(),
            tree,
            parents: Vec::new(),
            at: now_at(),
            kind,
            by,
            asked: None,
            note: None,
            version: None,
            check: None,
            data: None,
            from: None,
            what: None,
            trial: false,
            on: Some(self.place()),
        }
    }

    /// The record a mark points to.
    pub fn head(&self, app: &str, mark: &str) -> Option<(String, Record)> {
        let id = self.store.ref_get(app, mark)?;
        let record = self.record(&id)?;
        Some((id, record))
    }

    pub(crate) fn mark(&self, app: &str, mark: &str, id: Option<&str>) {
        if let Err(error) = self.store.ref_set(app, mark, id) {
            tracing::warn!(target: "plugins", "history: {app}: the mark {mark} is not set: {error}");
        }
    }

    fn view(&self, id: &str, record: Record, marks: &BTreeMap<String, String>) -> RecordView {
        RecordView {
            id: id.to_string(),
            short: id[..7].to_string(),
            record,
            marks: marks.iter().filter(|(_, target)| target.as_str() == id).map(|(name, _)| name.clone()).collect(),
            in_draft: false,
        }
    }

    /// The draft's last step and every record before it.
    fn draft_past(&self, app: &str) -> HashSet<String> {
        let mut seen = HashSet::new();
        let mut queue: Vec<String> = self.store.ref_get(app, DRAFT).into_iter().collect();
        while let Some(id) = queue.pop() {
            if seen.len() > 10_000 || !seen.insert(id.clone()) {
                continue;
            }
            if let Some(record) = self.record(&id) {
                queue.extend(record.parents);
            }
        }
        seen
    }

    pub fn record_view(&self, app: &str, id: &str) -> Option<RecordView> {
        let record = self.record(id).filter(|record| record.app == app)?;
        Some(self.view(id, record, &self.store.refs(app)))
    }

    /// The app's records, the newest first, from before `before` (a record's id), at most `limit`.
    pub fn feed(&self, app: &str, before: Option<&str>, limit: usize) -> Feed {
        let marks = self.store.refs(app);
        let ids = self.store.log(app);
        let end = before.and_then(|before| ids.iter().position(|id| id == before)).unwrap_or(ids.len());
        let start = end.saturating_sub(limit.max(1));
        let past = self.draft_past(app);
        let records = ids[start..end]
            .iter()
            .rev()
            .filter_map(|id| {
                let mut view = self.view(id, self.record(id)?, &marks);
                view.in_draft = past.contains(id);
                Some(view)
            })
            .collect();
        let earlier_gone = ids
            .first()
            .and_then(|first| self.record(first))
            .is_some_and(|first| first.parents.iter().any(|parent| !self.store.has(parent)));
        Feed {
            app: app.to_string(),
            records,
            more: start > 0,
            earlier_gone,
            marks,
            upstream: merge::upstream(self, app),
        }
    }

    /// The app's records, the oldest first, with their ids.
    pub(crate) fn line(&self, app: &str) -> Vec<(String, Record)> {
        self.store.log(app).into_iter().filter_map(|id| Some((id.clone(), self.record(&id)?))).collect()
    }

    /// A record's id from a short or whole one, within the app's line.
    pub fn resolve(&self, app: &str, short: &str) -> Option<String> {
        let short = short.trim().to_ascii_lowercase();
        if short.len() < 4 {
            return None;
        }
        let found: Vec<String> = self.store.log(app).into_iter().filter(|id| id.starts_with(&short)).collect();
        match found.as_slice() {
            [one] => Some(one.clone()),
            _ => store::valid_id(&short).then_some(short).filter(|id| self.record(id).is_some_and(|r| r.app == app)),
        }
    }

    pub fn files_of(&self, record: &str) -> Vec<FileView> {
        self.record(record)
            .and_then(|record| self.tree(&record.tree))
            .unwrap_or_default()
            .into_iter()
            .map(|entry| FileView { path: entry.path, size: entry.size })
            .collect()
    }

    // Steps of a draft.

    fn draft_dir(&self, app: &str) -> PathBuf {
        self.dev_dir.join(app)
    }

    /// The draft's files as a step, when they changed since its last one; the step's id.
    pub fn step(&self, app: &str, meta: StepMeta) -> Option<String> {
        let dir = self.draft_dir(app);
        if !dir.is_dir() {
            return None;
        }
        let _writing = self.writing();
        let lock = self.lock(app);
        let _held = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match self.step_locked(app, &dir, meta) {
            Ok(id) => id,
            Err(error) => {
                tracing::warn!(target: "plugins", "history: {app}: the step is not kept: {error}");
                None
            }
        }
    }

    fn step_locked(&self, app: &str, dir: &Path, meta: StepMeta) -> Result<Option<String>, String> {
        let tree = self.snapshot_dir(dir)?;
        let head = self.head(app, DRAFT);
        if head.as_ref().is_some_and(|(_, record)| record.tree == tree) && meta.other.is_none() {
            return Ok(None);
        }
        let mut record = self.new_record(app, meta.kind, tree, meta.by.clone());
        record.asked = meta.asked.as_deref().map(|asked| shortened(asked, ASKED_MAX)).filter(|asked| !asked.is_empty());
        record.note = meta.note.as_deref().map(|note| shortened(note, NOTE_MAX)).filter(|note| !note.is_empty());
        record.from = meta.from.clone();
        record.version = self
            .manifest_of(&record.tree)
            .and_then(|manifest| manifest.get("version").and_then(Value::as_str).map(str::to_string));
        // Two saves by the same hand within minutes: the newer replaces the older, which nothing else points to.
        let mut replaces = None;
        if let Some((head_id, head_record)) = &head {
            let fresh =
                ms_of(&head_record.at).is_some_and(|at| chrono::Utc::now().timestamp_millis() - at < COALESCE_MS);
            let pointed = self.store.refs(app).iter().any(|(name, id)| name != DRAFT && id == head_id);
            let last = self.store.log(app).last() == Some(head_id);
            if meta.coalesce
                && head_record.kind == RecordKind::Step
                && head_record.by == meta.by
                && fresh
                && !pointed
                && last
            {
                record.parents = head_record.parents.clone();
                replaces = Some(head_id.clone());
            } else {
                record.parents.push(head_id.clone());
            }
        }
        if let Some(other) = meta.other.filter(|other| !record.parents.contains(other)) {
            record.parents.push(other);
        }
        let bytes = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
        let id = self.store.put(Kind::Step, &bytes).map_err(|error| error.to_string())?;
        match &replaces {
            Some(old) => self.store.log_replace_last(app, old, &id),
            None => self.store.log_push(app, &id),
        }
        .map_err(|error| error.to_string())?;
        self.mark(app, DRAFT, Some(&id));
        drop(head);
        self.tell(app, "history");
        Ok(Some(id))
    }

    /// Before work on a draft (the agent's, a step back, a merge): what was changed on disk by hand since its last
    /// step becomes a step of its own, so nothing of it is lost.
    pub fn before_work(&self, app: &str) {
        let mut meta = StepMeta::new(RecordKind::Step, Author::new(Who::Hand));
        meta.note = Some(crate::tr!("правки руками"));
        self.step(app, meta);
    }

    /// The draft back to a record's files: a new step with them (forward only). Its id.
    pub fn restore_draft(&self, app: &str, record: &str, by: Author) -> Result<String, String> {
        let target = self
            .record(record)
            .filter(|target| target.app == app)
            .ok_or_else(|| crate::tr!("такой записи у приложения нет"))?;
        let dir = self.draft_dir(app);
        if !dir.is_dir() {
            std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        }
        self.before_work(app);
        if self.head(app, DRAFT).is_some_and(|(_, head)| head.tree == target.tree) {
            return Err(crate::tr!("черновик и так такой"));
        }
        self.materialize(&target.tree, &dir, true)?;
        let mut meta = StepMeta::new(RecordKind::Restore, by);
        meta.from = Some(record.to_string());
        meta.note = Some(crate::tr!("черновик как в записи от {at}", at = target.at));
        self.step(app, meta).ok_or_else(|| crate::tr!("шаг не записался"))
    }

    // Versions of the installed app.

    /// The record of a version folder: one with the same files already in the line, else a new one found on disk.
    pub fn ensure_recorded(&self, app: &str, dir: &Path, version: &str) -> Option<String> {
        let _writing = self.writing();
        let lock = self.lock(app);
        let _held = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.ensure_recorded_locked(app, dir, version)
    }

    pub(crate) fn ensure_recorded_locked(&self, app: &str, dir: &Path, version: &str) -> Option<String> {
        let tree = match self.snapshot_dir(dir) {
            Ok(tree) => tree,
            Err(error) => {
                tracing::warn!(target: "plugins", "history: {app} {version}: not kept: {error}");
                return None;
            }
        };
        if let Some((id, _)) = self.line(app).into_iter().rev().find(|(_, record)| record.tree == tree) {
            return Some(id);
        }
        let mut record = self.new_record(app, RecordKind::Found, tree, Author::new(Who::System));
        record.version = Some(version.to_string());
        record.note = Some(crate::tr!("найдено на диске"));
        if let Some(at) = std::fs::metadata(dir.join("manifest.json"))
            .and_then(|meta| meta.modified())
            .ok()
            .map(chrono::DateTime::<chrono::Utc>::from)
        {
            record.at = at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        }
        match self.write(&record) {
            Ok(id) => Some(id),
            Err(error) => {
                tracing::warn!(target: "plugins", "history: {app} {version}: not kept: {error}");
                None
            }
        }
    }

    /// A version put in (the workshop's «Поставить», a trial, the catalog, a copy): its record, the plugin's data just
    /// before it, the marks moved. `draft` — the workshop's: the draft's mark moves too.
    #[allow(clippy::too_many_arguments)]
    pub fn note_install(
        &self,
        app: &str,
        dir: &Path,
        version: &str,
        kind: RecordKind,
        by: Author,
        draft: bool,
        trial: bool,
        parent: Option<String>,
    ) -> Option<String> {
        let data = self.capture(app);
        let _writing = self.writing();
        let lock = self.lock(app);
        let _held = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let tree = match self.snapshot_dir(dir) {
            Ok(tree) => tree,
            Err(error) => {
                tracing::warn!(target: "plugins", "history: {app} {version}: the install is not kept: {error}");
                return None;
            }
        };
        let mut record = self.new_record(app, kind, tree, by);
        record.version = Some(version.to_string());
        record.trial = trial;
        record.data = data;
        if kind == RecordKind::Install {
            record.check = Some("ok".into());
        }
        let line_head = if draft { self.store.ref_get(app, DRAFT) } else { None };
        let installed_head = self.store.ref_get(app, INSTALLED);
        for parent in [line_head, parent, installed_head].into_iter().flatten() {
            if !record.parents.contains(&parent) {
                record.parents.push(parent);
            }
        }
        // An install's line is the draft's: the installed version's record joins it as a second parent only when it
        // came from elsewhere (a step back, the catalog).
        if draft && record.parents.len() > 1 {
            let first = record.parents[0].clone();
            let second = record.parents[1].clone();
            if self.is_ancestor(&second, &first) {
                record.parents.truncate(1);
            }
        }
        match self.write(&record) {
            Ok(id) => {
                self.mark(app, INSTALLED, Some(&id));
                if draft {
                    self.mark(app, DRAFT, Some(&id));
                }
                if kind == RecordKind::Catalog {
                    self.mark(app, CATALOG, Some(&id));
                }
                let _ = self.store.set_removed(app, None);
                drop(_held);
                self.tell(app, "history");
                Some(id)
            }
            Err(error) => {
                tracing::warn!(target: "plugins", "history: {app} {version}: the install is not kept: {error}");
                None
            }
        }
    }

    /// The app removed: a record with its data as it was, the installed mark taken away; the history lives on.
    pub fn note_removed(&self, app: &str, actor: Option<&Actor>) {
        let data = self.capture(app);
        let _writing = self.writing();
        let lock = self.lock(app);
        let _held = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some((head_id, head)) = self.head(app, INSTALLED) else {
            let _ = self.store.set_removed(app, Some(&now_at()));
            return;
        };
        let mut record = self.new_record(app, RecordKind::Removed, head.tree.clone(), Author::of(actor, Who::System));
        record.parents = vec![head_id];
        record.version = head.version.clone();
        record.data = data;
        if let Err(error) = self.write(&record) {
            tracing::warn!(target: "plugins", "history: {app}: the removal is not kept: {error}");
        }
        self.mark(app, INSTALLED, None);
        let _ = self.store.set_removed(app, Some(&now_at()));
        drop(_held);
        self.tell(app, "history");
    }

    /// Whether `ancestor` is `of` or one of its parents' parents… (as far as the store still has them).
    pub fn is_ancestor(&self, ancestor: &str, of: &str) -> bool {
        let mut seen = HashSet::new();
        let mut queue = vec![of.to_string()];
        while let Some(id) = queue.pop() {
            if id == ancestor {
                return true;
            }
            if !seen.insert(id.clone()) || seen.len() > 10_000 {
                continue;
            }
            if let Some(record) = self.record(&id) {
                queue.extend(record.parents);
            }
        }
        false
    }

    // Data.

    /// The plugin's data now, kept: its id. None — not a plugin (a theme, a character), or no data parts here.
    pub fn capture(&self, app: &str) -> Option<String> {
        let parts = self.data.get()?;
        if let Some(package) = self.host().and_then(|host| host.installed(app)) {
            if package.manifest.kind != super::manifest::PackageKind::Plugin {
                return None;
            }
        }
        match parts.capture(&self.store, app) {
            Ok(id) => id,
            Err(error) => {
                tracing::warn!(target: "plugins", "history: {app}: the data is not kept: {error}");
                None
            }
        }
    }

    // What is on disk, at the start.

    /// The versions and drafts on disk the history has not seen get their records; the installed mark follows the
    /// version on disk. At the start, and before a step back of the installed app.
    pub fn adopt(&self) {
        let Some(host) = self.host() else { return };
        for package in host.installed_all() {
            let app = package.manifest.id.clone();
            if !matches!(host.origin_of(&app), Origin::User | Origin::Catalog) {
                continue;
            }
            self.adopt_installed(&host, &app);
        }
        for draft in std::fs::read_dir(&self.dev_dir).into_iter().flatten().flatten() {
            let Some(app) = draft.file_name().to_str().map(str::to_string) else { continue };
            if !super::manifest::valid_id(&app) || !draft.path().is_dir() {
                continue;
            }
            let mut meta = StepMeta::new(RecordKind::Found, Author::new(Who::System));
            meta.note = Some(if self.store.ref_get(&app, DRAFT).is_some() {
                crate::tr!("правки руками")
            } else {
                crate::tr!("черновик найден")
            });
            if self.store.ref_get(&app, DRAFT).is_some() {
                meta.kind = RecordKind::Step;
                meta.by = Author::new(Who::Hand);
            }
            self.step(&app, meta);
        }
    }

    /// One app's version folders recorded; its installed mark on the version that is on disk now.
    pub fn adopt_installed(&self, host: &PluginHost, app: &str) {
        let Some(package) = host.installed(app) else { return };
        let root = host.packages_dir().join(app);
        let mut versions: Vec<(semver::Version, PathBuf)> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| Some((semver::Version::parse(entry.file_name().to_str()?).ok()?, entry.path())))
            .collect();
        versions.sort_by(|a, b| a.0.cmp(&b.0));
        let _writing = self.writing();
        let lock = self.lock(app);
        let _held = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut current = None;
        for (version, dir) in &versions {
            let id = self.ensure_recorded_locked(app, dir, &version.to_string());
            if *dir == package.dir {
                current = id;
            }
        }
        if let Some(current) = current {
            let marked = self.store.ref_get(app, INSTALLED);
            let same = marked
                .as_deref()
                .and_then(|id| self.record(id))
                .zip(self.record(&current))
                .is_some_and(|(marked, current)| marked.tree == current.tree);
            if !same {
                self.mark(app, INSTALLED, Some(&current));
            }
        }
    }

    // Comparing.

    /// Two versions compared: `from` and `to` are records; `to` None — the draft as it is on disk.
    pub fn compare(&self, app: &str, from: Option<&str>, to: Option<&str>) -> Result<Comparison, String> {
        let tree_of = |record: &str| -> Result<String, String> {
            self.record(record).filter(|r| r.app == app).map(|r| r.tree).ok_or_else(|| crate::tr!("такой записи нет"))
        };
        let from_tree = match from {
            Some(record) => Some(tree_of(record)?),
            None => None,
        };
        let to_tree = match to {
            Some(record) => Some(tree_of(record)?),
            None => {
                let dir = self.draft_dir(app);
                if dir.is_dir() {
                    Some(self.snapshot_dir(&dir)?)
                } else {
                    None
                }
            }
        };
        let files = |tree: &Option<String>| -> BTreeMap<String, TreeEntry> {
            tree.as_deref()
                .and_then(|tree| self.tree(tree))
                .unwrap_or_default()
                .into_iter()
                .map(|entry| (entry.path.clone(), entry))
                .collect()
        };
        let (before, after) = (files(&from_tree), files(&to_tree));
        let paths: std::collections::BTreeSet<&String> = before.keys().chain(after.keys()).collect();
        let text = |entry: Option<&TreeEntry>| -> Option<Option<String>> {
            match entry {
                None => Some(Some(String::new())),
                Some(entry) => {
                    let bytes = self.store.get_kind(&entry.file, Kind::File)?;
                    Some(String::from_utf8(bytes).ok())
                }
            }
        };
        let mut changes = Vec::new();
        for path in paths {
            let (old, new) = (before.get(path), after.get(path));
            if old.map(|entry| &entry.file) == new.map(|entry| &entry.file) {
                continue;
            }
            let change = match (old, new) {
                (None, _) => "added",
                (_, None) => "removed",
                _ => "changed",
            };
            let (a, b) = (text(old).flatten(), text(new).flatten());
            let (binary, lines) = match (a, b) {
                (Some(a), Some(b)) => (false, diff::lines(&a, &b)),
                _ => (true, Vec::new()),
            };
            changes.push(FileChange { path: path.clone(), change, binary, lines });
        }
        let manifest = manifest_changes(
            from_tree.as_deref().and_then(|tree| self.manifest_of(tree)),
            to_tree.as_deref().and_then(|tree| self.manifest_of(tree)),
        );
        Ok(Comparison { from: from.map(str::to_string), to: to.map(str::to_string), files: changes, manifest })
    }
}

fn remove_empty_dirs(at: &Path) {
    for entry in std::fs::read_dir(at).into_iter().flatten().flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) && !entry.file_name().to_string_lossy().starts_with('.') {
            remove_empty_dirs(&entry.path());
            let _ = std::fs::remove_dir(entry.path());
        }
    }
}

/// The manifest's changes as a person reads them.
pub fn manifest_changes(before: Option<Value>, after: Option<Value>) -> Vec<String> {
    let strings = |manifest: &Option<Value>, key: &str| -> Vec<String> {
        manifest
            .as_ref()
            .and_then(|manifest| manifest.get(key))
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    let phrases = |manifest: &Option<Value>| -> Vec<String> {
        let mut found = Vec::new();
        if let Some(intents) =
            manifest.as_ref().and_then(|m| m.pointer("/contributes/intents")).and_then(Value::as_array)
        {
            for intent in intents {
                for phrase in intent.get("phrases").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(text) = phrase.as_str().or_else(|| phrase.get("template").and_then(Value::as_str)) {
                        found.push(text.to_string());
                    }
                }
            }
        }
        found
    };
    let mut lines = Vec::new();
    let field = |manifest: &Option<Value>, key: &str| {
        manifest
            .as_ref()
            .and_then(|m| m.get(key))
            .map(|value| value.as_str().map(str::to_string).unwrap_or(value.to_string()))
    };
    let (old_version, new_version) = (field(&before, "version"), field(&after, "version"));
    if old_version != new_version {
        lines.push(crate::tr!(
            "Версия: {old_version} → {new_version}",
            old_version = old_version.unwrap_or_else(|| "—".into()),
            new_version = new_version.unwrap_or_else(|| "—".into())
        ));
    }
    if field(&before, "title") != field(&after, "title") {
        lines.push(crate::tr!("Название: «{after}»", after = field(&after, "title").unwrap_or_default()));
    }
    let (old_rights, new_rights) = (strings(&before, "permissions"), strings(&after, "permissions"));
    let added: Vec<&String> = new_rights.iter().filter(|right| !old_rights.contains(right)).collect();
    let removed: Vec<&String> = old_rights.iter().filter(|right| !new_rights.contains(right)).collect();
    let (hosts_added, rights_added): (Vec<&&String>, Vec<&&String>) = added.iter().partition(|r| r.starts_with("net:"));
    let (hosts_removed, rights_removed): (Vec<&&String>, Vec<&&String>) =
        removed.iter().partition(|r| r.starts_with("net:"));
    let join = |items: &[&&String], strip: bool| {
        items
            .iter()
            .map(|item| if strip { item.trim_start_matches("net:").to_string() } else { item.to_string() })
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !rights_added.is_empty() {
        lines.push(crate::tr!("Новые права: {list}", list = join(&rights_added, false)));
    }
    if !rights_removed.is_empty() {
        lines.push(crate::tr!("Больше не нужны права: {list}", list = join(&rights_removed, false)));
    }
    if !hosts_added.is_empty() {
        lines.push(crate::tr!("Новые адреса: {list}", list = join(&hosts_added, true)));
    }
    if !hosts_removed.is_empty() {
        lines.push(crate::tr!("Больше не ходит на: {list}", list = join(&hosts_removed, true)));
    }
    let (old_phrases, new_phrases) = (phrases(&before), phrases(&after));
    let new: Vec<&String> = new_phrases.iter().filter(|phrase| !old_phrases.contains(phrase)).collect();
    let gone: Vec<&String> = old_phrases.iter().filter(|phrase| !new_phrases.contains(phrase)).collect();
    if !new.is_empty() {
        lines.push(crate::tr!(
            "Новые фразы: {list}",
            list = new.iter().map(|p| format!("«{p}»")).collect::<Vec<_>>().join(", ")
        ));
    }
    if !gone.is_empty() {
        lines.push(crate::tr!(
            "Ушли фразы: {list}",
            list = gone.iter().map(|p| format!("«{p}»")).collect::<Vec<_>>().join(", ")
        ));
    }
    let data_version = |manifest: &Option<Value>| manifest.as_ref().and_then(|m| m.get("dataVersion")).cloned();
    if data_version(&before) != data_version(&after) {
        lines.push(crate::tr!("Изменился вид данных (dataVersion)"));
    }
    lines
}

/// The lock of an app's line held, poisoned or not.
pub(crate) fn held(lock: &Mutex<()>) -> MutexGuard<'_, ()> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn history(dir: &Path) -> Arc<History> {
        History::open(dir.join("history"), dir.join("dev"), None, Arc::new(|| false)).unwrap()
    }

    pub fn write_draft(dir: &Path, app: &str, files: &[(&str, &str)]) {
        let folder = dir.join("dev").join(app);
        for (path, text) in files {
            let file = folder.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
    }

    fn hand() -> Author {
        Author { who: Who::Hand, name: Some("Админ".into()), via: Some("panel".into()) }
    }

    #[test]
    fn keeps_a_step_only_when_the_files_changed_and_gives_the_folder_back_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let history = history(dir.path());
        write_draft(dir.path(), "weather", &[("manifest.json", "{\"version\":\"1.0.0\"}"), ("assets/a.json", "[1]")]);
        let first = history.step("weather", StepMeta::new(RecordKind::Found, Author::new(Who::System))).unwrap();
        assert_eq!(history.step("weather", StepMeta::new(RecordKind::Step, hand())), None, "nothing changed");
        write_draft(dir.path(), "weather", &[("backend.js", "export function a() {}\n")]);
        let mut meta = StepMeta::new(RecordKind::Step, Author::new(Who::Agent));
        meta.asked = Some("сделай, чтобы погода говорила коротко".into());
        let second = history.step("weather", meta).unwrap();
        let record = history.record(&second).unwrap();
        assert_eq!((record.parents.clone(), record.version.as_deref()), (vec![first.clone()], Some("1.0.0")));
        assert_eq!(record.asked.as_deref(), Some("сделай, чтобы погода говорила коротко"));

        // Back to the first step: a new step with its files, the file added since gone.
        let back = history.restore_draft("weather", &first, hand()).unwrap();
        assert!(!dir.path().join("dev/weather/backend.js").exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("dev/weather/assets/a.json")).unwrap(), "[1]");
        assert_eq!(history.record(&back).unwrap().from.as_deref(), Some(first.as_str()));
        // And forward again: nothing was lost.
        history.restore_draft("weather", &second, hand()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("dev/weather/backend.js")).unwrap(),
            "export function a() {}\n"
        );
        let feed = history.feed("weather", None, 10);
        assert_eq!(feed.records.len(), 4);
        assert_eq!(feed.records[0].marks, vec!["draft".to_string()]);
        assert!(!feed.more && !feed.earlier_gone);
        assert!(history.feed("weather", None, 3).more);
    }

    #[test]
    fn makes_saves_by_the_same_hand_within_minutes_one_step() {
        let dir = tempfile::tempdir().unwrap();
        let history = history(dir.path());
        write_draft(dir.path(), "notes", &[("manifest.json", "{}")]);
        history.step("notes", StepMeta::new(RecordKind::Found, Author::new(Who::System))).unwrap();
        let save = || StepMeta { coalesce: true, ..StepMeta::new(RecordKind::Step, hand()) };
        write_draft(dir.path(), "notes", &[("backend.js", "1")]);
        let one = history.step("notes", save()).unwrap();
        write_draft(dir.path(), "notes", &[("backend.js", "2")]);
        let two = history.step("notes", save()).unwrap();
        assert_ne!(one, two);
        assert_eq!(history.store().log("notes").len(), 2, "the second save replaced the first");
        assert_eq!(history.record(&two).unwrap().parents, history.record(&one).unwrap().parents);
        // Another hand's save is a step of its own.
        write_draft(dir.path(), "notes", &[("backend.js", "3")]);
        history.step("notes", StepMeta { coalesce: true, ..StepMeta::new(RecordKind::Step, Author::new(Who::Mcp)) });
        assert_eq!(history.store().log("notes").len(), 3);
    }

    #[test]
    fn compares_two_versions_by_files_and_says_the_manifests_changes_in_words() {
        let dir = tempfile::tempdir().unwrap();
        let history = history(dir.path());
        write_draft(
            dir.path(),
            "radio",
            &[("manifest.json", "{\"version\":\"1.0.0\",\"permissions\":[]}\n"), ("backend.js", "a\nb\n")],
        );
        let first = history.step("radio", StepMeta::new(RecordKind::Found, Author::new(Who::System))).unwrap();
        write_draft(
            dir.path(),
            "radio",
            &[
                ("manifest.json", "{\"version\":\"1.1.0\",\"permissions\":[\"net:api.example.org\"]}\n"),
                ("backend.js", "a\nc\n"),
            ],
        );
        let comparison = history.compare("radio", Some(&first), None).unwrap();
        assert_eq!(comparison.files.iter().map(|file| file.change).collect::<Vec<_>>(), vec!["changed", "changed"]);
        assert_eq!(comparison.manifest, vec!["Версия: 1.0.0 → 1.1.0", "Новые адреса: api.example.org"]);
        assert!(history.compare("radio", Some("nope"), None).is_err());
    }
}
