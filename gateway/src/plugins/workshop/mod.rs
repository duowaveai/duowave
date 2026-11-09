//! The workshop (docs/plans/plugins.md, «Мастерская», stage 6): a plugin or a theme is written in a draft, checked in a
//! sandbox and installed as a new version beside the old one — by the agent in the panel, by Claude Code through the
//! gateway's MCP server, or by hand with `duowave plugin`.
//!
//! - **Drafts** live in `DATA_DIR/packages-dev/<id>/` (PLUGINS_CONFIG `dev_dir`): the same files as a package, only
//!   plain paths inside it, nothing a browser would run in `assets/`. A built-in or a catalog's package is not edited:
//!   its copy of one's own is («Сделать свою копию»).
//! - **The sandbox** runs the draft's code in QuickJS as the host would, with a stand-in `host`: `kv` in memory (kept
//!   per draft between runs), speech written down instead of said, the network only to the manifest's hosts (the same
//!   checks as a plugin that is on). «Проверить» reads the manifest and loads the code; «Запустить» calls a function;
//!   «Прогнать фразу» matches a phrase the way the router does and shows what the station would get.
//! - **Installing** checks the draft first, puts it into PACKAGES_DIR as a new version (one taken already — the next
//!   patch) and reloads the plugin: the new version starts beside the old one, and if it does not, the old one stays.
//!   The first install and new rights or hosts wait for the admin's «ок» in the panel (`approve`): the agent and voice
//!   cannot give it.

pub mod agent;
pub mod cli;
pub mod openapi;
pub mod routes;
pub mod sandbox;
pub mod templates;
pub mod voice;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{Map, Value};

use super::controller::{PackageView, PluginHost};
use super::manifest::{self, Budget, PackageKind, Permission};
use super::registry::{self, Origin};
use crate::core::events::Actor;
use crate::platform::http::ApiError;

pub use sandbox::{CheckReport, PhraseReport, RunReport};

/// A file of a draft at most this big; the whole draft at most this.
const MAX_FILE: u64 = 1024 * 1024;
const MAX_DRAFT: u64 = 8 * 1024 * 1024;
/// How many versions of a package of one's own stay after an install: the new one and the two before.
const KEPT_VERSIONS: usize = 3;
/// The longest trial on the gateway, minutes.
const MAX_TRIAL_MINUTES: u32 = 60;
/// Files of a draft at its root; anything else lives under `assets/`.
const ROOT_FILES: &[&str] = &["manifest.json", "backend.js", "panel.js", "panel.css", "README.md", "SKILL.md"];
/// What `assets/` may not hold: anything a browser runs on the panel's origin.
const REFUSED: &[&str] = &["svg", "svgz", "html", "htm", "xhtml", "xml", "js", "mjs"];

/// A path inside a draft, checked: plain names, a root file or one under `assets/`.
pub fn draft_path(path: &str) -> Result<PathBuf, String> {
    let parts: Vec<&str> = path.trim().trim_start_matches("./").split('/').collect();
    let plain = |part: &&str| {
        !part.is_empty()
            && !part.starts_with('.')
            && part.len() <= 80
            && part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    if parts.is_empty() || parts.len() > 4 || !parts.iter().all(plain) {
        return Err(format!("«{path}»: путь из латинских букв, цифр, «.», «_», «-», до четырёх частей"));
    }
    match parts.as_slice() {
        [file] if ROOT_FILES.contains(file) => Ok(PathBuf::from(file)),
        [file] => Err(format!("«{file}»: в корне черновика только {}; остальное — в assets/", ROOT_FILES.join(", "))),
        ["assets", rest @ ..] => {
            let extension = rest.last().and_then(|name| name.rsplit_once('.')).map(|(_, e)| e.to_ascii_lowercase());
            if extension.as_deref().is_some_and(|extension| REFUSED.contains(&extension)) {
                return Err(format!("«{path}»: в assets/ нельзя то, что браузер запустит (SVG, HTML, скрипты)"));
            }
            Ok(parts.iter().collect())
        }
        _ => Err(format!("«{path}»: вне assets/ папок нет")),
    }
}

/// A file of a draft, for the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileView {
    pub path: String,
    pub size: u64,
}

/// A draft as the panel and the agent see it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftView {
    pub id: String,
    pub kind: Option<PackageKind>,
    pub title: Option<String>,
    pub version: Option<String>,
    pub files: Vec<FileView>,
    /// The manifest's problems, if it does not read.
    pub problems: Vec<String>,
    /// The version installed now, if any; and where the installed package came from.
    pub installed: Option<String>,
    pub origin: Option<Origin>,
    /// A trial of the draft running on the gateway now («Проба на колонке»), if any.
    pub trial: Option<Trial>,
}

/// A trial: the draft installed for a while in place of the installed version (or of nothing), then taken back unless
/// the admin keeps it. Kept in `<dev_dir>/.trials/<id>.json`, so a restart still takes it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trial {
    /// The version the trial put on the gateway.
    pub version: String,
    /// The version it stands in for; None — the package was not installed and goes after the trial.
    pub previous: Option<String>,
    /// Whether the plugin was on before: a trial switches it on and back off.
    pub was_enabled: bool,
    /// When it ends, RFC 3339.
    pub until: String,
}

/// What «Поставить» would do, shown before it does: versions, rights and hosts that change, what the draft gives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPlan {
    pub kind: PackageKind,
    pub title: String,
    /// The version installed now; None — the first install.
    pub from: Option<String>,
    /// The version the draft will get: its own, or the next patch when that one is taken.
    pub to: String,
    pub first: bool,
    /// Rights it asks for that the installed version did not; and those it no longer asks for. Hosts apart.
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub hosts_added: Vec<String>,
    pub hosts_removed: Vec<String>,
    /// Rights and hosts it keeps from the installed version.
    pub kept: Vec<String>,
    pub budget: Budget,
    /// Phrase templates and tool names the draft gives.
    pub phrases: Vec<String>,
    pub tools: Vec<String>,
    /// Whether the installed plugin is on: a new version of one that is off stays off.
    pub enabled: bool,
    /// Older versions that stay on the gateway for a rollback; how many are kept in all.
    pub kept_versions: usize,
}

/// What the admin is asked before an install: the first one, or one with new rights or hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub first: bool,
    /// The rights it asks for that the installed version did not (all of them, the first time).
    pub permissions: Vec<String>,
    pub hosts: Vec<String>,
    pub budget: Budget,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    pub installed: bool,
    pub version: Option<String>,
    /// Set when it waits for the admin's «ок» in the panel.
    pub needs_approval: Option<Approval>,
    pub message: String,
    pub package: Option<PackageView>,
}

/// A draft's store in the sandbox.
type DraftStore = Arc<Mutex<Map<String, Value>>>;

pub struct Workshop {
    dir: PathBuf,
    host: Arc<PluginHost>,
    handle: Option<tokio::runtime::Handle>,
    /// The sandbox's `kv` per draft, kept between runs (gone at a restart).
    kv: Mutex<HashMap<String, DraftStore>>,
    /// One install at a time.
    installing: tokio::sync::Mutex<()>,
}

fn walk(root: &Path, at: &Path, files: &mut Vec<FileView>) {
    let Ok(entries) = std::fs::read_dir(at) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => walk(root, &path, files),
            Ok(kind) if kind.is_file() => {
                let relative = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                files.push(FileView { path: relative, size: entry.metadata().map(|meta| meta.len()).unwrap_or(0) });
            }
            _ => {}
        }
    }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if kind.is_dir() {
            copy_dir(&entry.path(), &to.join(entry.file_name()))?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

impl Workshop {
    pub fn new(dir: PathBuf, host: Arc<PluginHost>) -> Arc<Workshop> {
        Arc::new(Workshop {
            dir,
            host,
            handle: tokio::runtime::Handle::try_current().ok(),
            kv: Mutex::new(HashMap::new()),
            installing: tokio::sync::Mutex::new(()),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn host(&self) -> &Arc<PluginHost> {
        &self.host
    }

    fn folder(&self, id: &str) -> Result<PathBuf, ApiError> {
        if !manifest::valid_id(id) {
            return Err(ApiError::field("id", "строчные латинские буквы, цифры и «-», с буквы, до 32 знаков"));
        }
        Ok(self.dir.join(id))
    }

    fn existing(&self, id: &str) -> Result<PathBuf, ApiError> {
        let folder = self.folder(id)?;
        if !folder.is_dir() {
            return Err(ApiError::not_found(format!("Черновика {id} нет.")));
        }
        Ok(folder)
    }

    fn view_of(&self, id: &str, folder: &Path) -> DraftView {
        let mut files = Vec::new();
        walk(folder, folder, &mut files);
        let text = std::fs::read_to_string(folder.join("manifest.json")).ok();
        let (kind, title, version, problems) = match text.as_deref().map(manifest::parse) {
            Some(Ok(manifest)) => {
                (Some(manifest.kind), Some(manifest.title), Some(manifest.version.to_string()), Vec::new())
            }
            Some(Err(error)) => {
                let value: Option<Value> = text.as_deref().and_then(|text| serde_json::from_str(text).ok());
                let field = |key: &str| {
                    value.as_ref().and_then(|value| value.get(key)).and_then(Value::as_str).map(str::to_string)
                };
                (
                    field("kind").and_then(|kind| PackageKind::parse(&kind)),
                    field("title"),
                    field("version"),
                    error.problems,
                )
            }
            None => (None, None, None, vec!["нет manifest.json".into()]),
        };
        let installed = self.host.installed(id);
        DraftView {
            id: id.to_string(),
            kind,
            title,
            version,
            files,
            problems,
            installed: installed.as_ref().map(|package| package.version()),
            origin: installed.map(|_| self.host.origin_of(id)),
            trial: self.trial(id),
        }
    }

    /// Every draft, by id.
    pub fn drafts(&self) -> Vec<DraftView> {
        let mut ids: Vec<String> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|id| manifest::valid_id(id))
            .collect();
        ids.sort();
        ids.iter().map(|id| self.view_of(id, &self.dir.join(id))).collect()
    }

    pub fn draft(&self, id: &str) -> Result<DraftView, ApiError> {
        let folder = self.existing(id)?;
        Ok(self.view_of(id, &folder))
    }

    /// A new draft: from a template (`plugin` or `theme`), from an OpenAPI description, or from an installed package
    /// of one's own (its newest version) — a built-in's or a catalog's is copied under an id of one's own instead.
    pub fn create(
        &self,
        id: &str,
        kind: Option<&str>,
        title: Option<&str>,
        openapi: Option<&str>,
    ) -> Result<DraftView, ApiError> {
        let folder = self.folder(id)?;
        if folder.exists() {
            return Err(ApiError::conflict(format!("Черновик {id} уже есть.")));
        }
        // A new plugin or theme under an id a package already has would become that package's next version.
        if let Some(taken) = self.host.installed(id).filter(|_| kind.is_some() || openapi.is_some()) {
            let origin = self.host.origin_of(id);
            let way = if origin.editable() {
                " Или откройте его самого: «Открыть в мастерской» в «Дом» → «Пакеты»."
            } else {
                ""
            };
            return Err(ApiError::conflict(format!(
                "id «{id}» уже занят: так зовётся {} пакет «{}». Выберите другой id.{way}",
                origin.title(),
                taken.manifest.title
            )));
        }
        let files: Vec<(String, String)> = if let Some(spec) = openapi {
            openapi::draft_from(id, title, spec).map_err(|why| ApiError::bad_request(format!("OpenAPI: {why}")))?
        } else if let Some(installed) = self.host.installed(id).filter(|_| kind.is_none()) {
            let origin = self.host.origin_of(id);
            if !origin.editable() {
                return Err(ApiError::conflict(format!(
                    "«{id}» — {} пакет: его не правят, а делают свою копию («Сделать свою копию»).",
                    origin.title()
                )));
            }
            copy_dir(&installed.dir, &folder).map_err(ApiError::internal)?;
            return Ok(self.view_of(id, &folder));
        } else {
            match kind.unwrap_or("plugin") {
                "plugin" => templates::plugin(id, title.unwrap_or(id)),
                "theme" => templates::theme(id, title.unwrap_or(id)),
                other => return Err(ApiError::field("kind", format!("«{other}»: черновик — plugin или theme"))),
            }
        };
        std::fs::create_dir_all(&folder).map_err(ApiError::internal)?;
        for (path, text) in files {
            let path = draft_path(&path).map_err(ApiError::bad_request)?;
            if let Some(parent) = folder.join(&path).parent() {
                std::fs::create_dir_all(parent).map_err(ApiError::internal)?;
            }
            std::fs::write(folder.join(path), text).map_err(ApiError::internal)?;
        }
        tracing::info!(target: "plugins", "workshop: draft {id} made");
        Ok(self.view_of(id, &folder))
    }

    /// A draft of an installed package of one's own that has none yet: «Открыть в мастерской».
    pub fn open(&self, id: &str) -> Result<DraftView, ApiError> {
        match self.existing(id) {
            Ok(folder) => Ok(self.view_of(id, &folder)),
            Err(_) => self.create(id, None, None, None),
        }
    }

    pub fn read(&self, id: &str, path: &str) -> Result<String, ApiError> {
        let folder = self.existing(id)?;
        let path = draft_path(path).map_err(ApiError::bad_request)?;
        let file = folder.join(&path);
        let size =
            std::fs::metadata(&file).map_err(|_| ApiError::not_found(format!("Нет файла {}.", path.display())))?.len();
        if size > MAX_FILE {
            return Err(ApiError::bad_request("Файл больше 1 МБ."));
        }
        let bytes = std::fs::read(&file).map_err(ApiError::internal)?;
        String::from_utf8(bytes).map_err(|_| ApiError::bad_request("Файл не текстовый."))
    }

    pub fn write(&self, id: &str, path: &str, content: &str) -> Result<DraftView, ApiError> {
        let folder = self.existing(id)?;
        let path = draft_path(path).map_err(ApiError::bad_request)?;
        if content.len() as u64 > MAX_FILE {
            return Err(ApiError::bad_request("Файл больше 1 МБ."));
        }
        let mut files = Vec::new();
        walk(&folder, &folder, &mut files);
        let others: u64 = files.iter().filter(|file| Path::new(&file.path) != path).map(|file| file.size).sum();
        if others + content.len() as u64 > MAX_DRAFT {
            return Err(ApiError::bad_request("Черновик больше 8 МБ."));
        }
        let file = folder.join(&path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).map_err(ApiError::internal)?;
        }
        std::fs::write(&file, content).map_err(ApiError::internal)?;
        Ok(self.view_of(id, &folder))
    }

    pub fn delete_file(&self, id: &str, path: &str) -> Result<DraftView, ApiError> {
        let folder = self.existing(id)?;
        let path = draft_path(path).map_err(ApiError::bad_request)?;
        std::fs::remove_file(folder.join(&path))
            .map_err(|_| ApiError::not_found(format!("Нет файла {}.", path.display())))?;
        Ok(self.view_of(id, &folder))
    }

    /// The draft goes; the installed package stays as it is.
    pub fn delete(&self, id: &str) -> Result<(), ApiError> {
        let folder = self.existing(id)?;
        std::fs::remove_dir_all(folder).map_err(ApiError::internal)?;
        self.kv.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(id);
        Ok(())
    }

    fn sandbox_kv(&self, id: &str) -> Arc<Mutex<Map<String, Value>>> {
        self.kv.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).entry(id.to_string()).or_default().clone()
    }

    /// «Проверить»: the manifest, the code loaded in the sandbox, every handler exported, what the station is known
    /// not to play.
    pub async fn check(&self, id: &str) -> Result<CheckReport, ApiError> {
        let folder = self.existing(id)?;
        Ok(sandbox::check(&folder, id, self.handle.clone()).await)
    }

    /// «Запустить»: one function of the draft with its input, in the sandbox.
    pub async fn run(&self, id: &str, function: &str, input: Value) -> Result<RunReport, ApiError> {
        let folder = self.existing(id)?;
        sandbox::run(&folder, id, function, input, self.sandbox_kv(id), self.handle.clone())
            .await
            .map_err(ApiError::bad_request)
    }

    /// «Прогнать фразу»: the phrase as the router would match it against the draft's phrases, and what the station
    /// would get.
    pub async fn try_phrase(&self, id: &str, text: &str) -> Result<PhraseReport, ApiError> {
        let folder = self.existing(id)?;
        sandbox::try_phrase(&folder, id, text, self.sandbox_kv(id), self.handle.clone())
            .await
            .map_err(ApiError::bad_request)
    }

    /// The installed plugin's own journal: its console, its calls, what the station said of its items.
    pub fn journal(&self, id: &str) -> Vec<super::controller::JournalLine> {
        self.host.journal(id)
    }

    /// «Поставить»: the draft checked, put in as a new version beside the old one, the plugin reloaded (or switched on
    /// the first time). The first install and new rights or hosts wait for the admin's `approve`.
    /// «Поставить»: a real install keeps a running trial's version (it is no longer taken back).
    pub async fn install(&self, id: &str, approve: bool, actor: Option<Actor>) -> Result<InstallReport, ApiError> {
        let report = self.install_draft(id, approve, actor).await?;
        if report.installed {
            self.forget_trial(id);
        }
        Ok(report)
    }

    /// What «Поставить» would do now: the draft checked, its version, its rights against the installed version's.
    pub async fn plan(&self, id: &str) -> Result<InstallPlan, ApiError> {
        let folder = self.existing(id)?;
        let report = sandbox::check(&folder, id, self.handle.clone()).await;
        if !report.ok {
            return Err(ApiError::bad_request(format!("Черновик не ставится: {}.", problems_of(&report).join("; "))));
        }
        let text = std::fs::read_to_string(folder.join("manifest.json")).map_err(ApiError::internal)?;
        let draft = manifest::parse(&text).map_err(|error| ApiError::bad_request(error.to_string()))?;
        let installed = self.host.installed(id);
        let before: BTreeSet<String> = installed
            .as_ref()
            .map(|package| package.manifest.permissions.iter().map(Permission::name).collect())
            .unwrap_or_default();
        let after: BTreeSet<String> = draft.permissions.iter().map(Permission::name).collect();
        let host = |name: &String| name.strip_prefix("net:").map(str::to_string);
        let right = |name: &&String| !name.starts_with("net:");
        let enabled = match installed {
            Some(_) => self.host.view_one(id).await.map(|view| view.enabled).unwrap_or(false),
            None => false,
        };
        Ok(InstallPlan {
            kind: draft.kind,
            title: draft.title.clone(),
            from: installed.as_ref().map(|package| package.version()),
            to: next_version(self.host.packages_dir(), id, &draft.version).to_string(),
            first: installed.is_none(),
            added: after.difference(&before).filter(right).cloned().collect(),
            removed: before.difference(&after).filter(right).cloned().collect(),
            hosts_added: after.difference(&before).filter_map(host).collect(),
            hosts_removed: before.difference(&after).filter_map(host).collect(),
            kept: after.intersection(&before).cloned().collect(),
            budget: draft.budget,
            phrases: draft
                .contributes
                .intents
                .iter()
                .flat_map(|intent| intent.phrases.iter().map(|phrase| phrase.template.clone()))
                .collect(),
            tools: draft.contributes.tools.iter().map(|tool| tool.name.clone()).collect(),
            enabled,
            kept_versions: KEPT_VERSIONS,
        })
    }

    fn trials_dir(&self) -> PathBuf {
        self.dir.join(".trials")
    }

    /// The trial of a draft running now, if any.
    pub fn trial(&self, id: &str) -> Option<Trial> {
        let text = std::fs::read_to_string(self.trials_dir().join(format!("{id}.json"))).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn forget_trial(&self, id: &str) {
        let _ = std::fs::remove_file(self.trials_dir().join(format!("{id}.json")));
    }

    /// «Проба на колонке»: the draft installed and switched on for `minutes`, then taken back by itself — the version
    /// before comes back (and goes off again if it was off), or the package goes if there was none. The same
    /// approval as an install; a restart in between still takes it back.
    pub async fn start_trial(
        self: &Arc<Self>,
        id: &str,
        minutes: u32,
        approve: bool,
        actor: Option<Actor>,
    ) -> Result<InstallReport, ApiError> {
        if self.trial(id).is_some() {
            return Err(ApiError::conflict("Проба уже идёт: закончите её или оставьте версию."));
        }
        let folder = self.existing(id)?;
        let text = std::fs::read_to_string(folder.join("manifest.json")).map_err(ApiError::internal)?;
        if manifest::parse(&text).map(|draft| draft.kind).ok() != Some(PackageKind::Plugin) {
            return Err(ApiError::bad_request("На колонке пробуют плагин; тему примеряют в пульте."));
        }
        let previous = self.host.installed(id).map(|package| package.version());
        let was_enabled = match previous {
            Some(_) => self.host.view_one(id).await.map(|view| view.enabled).unwrap_or(false),
            None => false,
        };
        let mut report = self.install_draft(id, approve, actor.clone()).await?;
        let Some(version) = report.version.clone().filter(|_| report.installed) else { return Ok(report) };
        if previous.is_some() && !was_enabled {
            if let Err(error) = self.host.enable(id, actor.clone()).await {
                report.message = format!("{} Но не включилась: {}", report.message, error.message);
            }
        }
        let minutes = minutes.clamp(1, MAX_TRIAL_MINUTES);
        let until = chrono::Utc::now() + chrono::Duration::minutes(minutes as i64);
        let trial =
            Trial { version: version.clone(), previous: previous.clone(), was_enabled, until: until.to_rfc3339() };
        std::fs::create_dir_all(self.trials_dir()).map_err(ApiError::internal)?;
        let json = serde_json::to_string_pretty(&trial).map_err(ApiError::internal)?;
        std::fs::write(self.trials_dir().join(format!("{id}.json")), json).map_err(ApiError::internal)?;
        self.host.note_line(id, "load", &format!("проба: версия {version} на {minutes} мин"));
        self.watch_trial(id, trial);
        report.message = match previous {
            Some(previous) => format!(
                "Проба: версия {version} работает {minutes} мин, потом вернётся {previous}. Говорите колонке как обычно."
            ),
            None => format!("Проба: плагин работает {minutes} мин, потом уйдёт. Говорите колонке как обычно."),
        };
        Ok(report)
    }

    /// The trial's end on its time: unless it was kept or ended before.
    fn watch_trial(self: &Arc<Self>, id: &str, trial: Trial) {
        let Some(handle) = self.handle.clone() else { return };
        let (workshop, id) = (self.clone(), id.to_string());
        handle.spawn(async move {
            let left = chrono::DateTime::parse_from_rfc3339(&trial.until)
                .map(|until| (until.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_milliseconds().max(0))
                .unwrap_or(0);
            tokio::time::sleep(std::time::Duration::from_millis(left as u64)).await;
            if workshop.trial(&id).as_ref() != Some(&trial) {
                return;
            }
            if let Err(error) = workshop.end_trial(&id, false, None).await {
                tracing::warn!(target: "plugins", "workshop: trial of {id} not taken back: {}", error.message);
            }
        });
    }

    /// Trials left from before a restart: each ends on its time (at once, if that has passed).
    pub fn resume_trials(self: &Arc<Self>) {
        let ids: Vec<String> = std::fs::read_dir(self.trials_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter_map(|name| name.strip_suffix(".json").map(str::to_string))
            .filter(|id| manifest::valid_id(id))
            .collect();
        for id in ids {
            if let Some(trial) = self.trial(&id) {
                self.watch_trial(&id, trial);
            }
        }
    }

    /// The trial's end: `keep` — its version stays as an install; otherwise the version before comes back (switched
    /// off again if it was off), or the package goes if there was none.
    pub async fn end_trial(&self, id: &str, keep: bool, actor: Option<Actor>) -> Result<String, ApiError> {
        let _turn = self.installing.lock().await;
        let Some(trial) = self.trial(id) else {
            return Err(ApiError::conflict("Пробы сейчас нет."));
        };
        self.forget_trial(id);
        if keep {
            self.host.note_line(id, "load", &format!("проба: версия {} оставлена", trial.version));
            return Ok(format!("Версия {} остаётся.", trial.version));
        }
        let now = self.host.installed(id).map(|package| package.version());
        if now.as_deref() != Some(trial.version.as_str()) {
            // Something else was installed or removed since: nothing of the trial's to take back.
            return Ok("Проба закончилась; с тех пор пакет меняли — оставляю как есть.".into());
        }
        let message = match &trial.previous {
            Some(previous) => {
                self.host.rollback(id, actor.clone()).await?;
                if !trial.was_enabled {
                    self.host.disable(id, actor.clone()).await?;
                }
                format!("Проба закончилась: вернулась версия {previous}.")
            }
            None => {
                let root = self.host.packages_dir().to_path_buf();
                self.host.forget(id).await?;
                super::install::remove_package(&root, id).map_err(ApiError::internal)?;
                self.host.rescan();
                self.host.announce(id, "removed", None, actor.clone());
                "Проба закончилась: плагин убран.".to_string()
            }
        };
        self.host.note_line(id, "load", &format!("проба версии {} закончилась", trial.version));
        tracing::info!(target: "plugins", "workshop: trial of {id} {} taken back", trial.version);
        Ok(message)
    }

    async fn install_draft(&self, id: &str, approve: bool, actor: Option<Actor>) -> Result<InstallReport, ApiError> {
        let _turn = self.installing.lock().await;
        let folder = self.existing(id)?;
        let report = sandbox::check(&folder, id, self.handle.clone()).await;
        if !report.ok {
            return Err(ApiError::bad_request(format!("Черновик не ставится: {}.", problems_of(&report).join("; "))));
        }
        let text = std::fs::read_to_string(folder.join("manifest.json")).map_err(ApiError::internal)?;
        let draft = manifest::parse(&text).map_err(|error| ApiError::bad_request(error.to_string()))?;
        let installed = self.host.installed(id);
        if installed.is_some() {
            let origin = self.host.origin_of(id);
            if !origin.editable() {
                return Err(ApiError::conflict(format!(
                    "«{id}» — {} пакет: его не меняют, а делают свою копию.",
                    origin.title()
                )));
            }
        }
        // What the admin is asked about: the first install, new rights, new hosts.
        let before: BTreeSet<String> = installed
            .as_ref()
            .map(|package| package.manifest.permissions.iter().map(Permission::name).collect())
            .unwrap_or_default();
        let added: Vec<String> =
            draft.permissions.iter().map(Permission::name).filter(|name| !before.contains(name)).collect();
        let needs = installed.is_none() || !added.is_empty();
        if needs && !approve {
            let approval = Approval {
                first: installed.is_none(),
                permissions: added.iter().filter(|name| !name.starts_with("net:")).cloned().collect(),
                hosts: added.iter().filter_map(|name| name.strip_prefix("net:")).map(str::to_string).collect(),
                budget: draft.budget,
            };
            let message = if approval.first {
                "Первую установку одобряет администратор в пульте: права, хосты и бюджет на экране.".to_string()
            } else {
                format!("Новые права ({}) одобряет администратор в пульте.", added.join(", "))
            };
            return Ok(InstallReport {
                installed: false,
                version: None,
                needs_approval: Some(approval),
                message,
                package: None,
            });
        }
        if draft.kind == PackageKind::Plugin {
            self.host
                .room_for(crate::operations::system_memory::folder_mb(&folder))
                .map_err(|why| ApiError::conflict(format!("Не поставить: {why}.")))?;
        }
        // The version: the draft's, or the next patch when it is taken.
        let root = self.host.packages_dir().to_path_buf();
        let version = next_version(&root, id, &draft.version);
        if version != draft.version {
            let mut value: Value = serde_json::from_str(&text).map_err(ApiError::internal)?;
            value["version"] = Value::String(version.to_string());
            let pretty = serde_json::to_string_pretty(&value).map_err(ApiError::internal)?;
            std::fs::write(folder.join("manifest.json"), format!("{pretty}\n")).map_err(ApiError::internal)?;
        }
        let partial = root.join(format!(".partial-{id}-{version}"));
        let _ = std::fs::remove_dir_all(&partial);
        copy_dir(&folder, &partial).map_err(ApiError::internal)?;
        std::fs::create_dir_all(root.join(id)).map_err(ApiError::internal)?;
        if let Err(error) = std::fs::rename(&partial, root.join(id).join(version.to_string())) {
            let _ = std::fs::remove_dir_all(&partial);
            return Err(ApiError::internal(error));
        }
        self.host.note_installed(id, registry::SOURCE_LOCAL, actor.clone())?;
        prune(&root, id, KEPT_VERSIONS);
        let version = version.to_string();
        self.host.note_line(id, "load", &format!("мастерская поставила версию {version}"));
        self.host.announce(
            id,
            if installed.is_some() { "updated" } else { "installed" },
            Some(version.clone()),
            actor.clone(),
        );
        let mut message = format!("Версия {version} поставлена.");
        if draft.kind == PackageKind::Plugin {
            let enabled = self.host.view_one(id).await.map(|view| view.enabled).unwrap_or(false);
            let started = if enabled {
                self.host.reload(id, actor.clone()).await.map(|_| "работает она".to_string())
            } else if installed.is_none() {
                self.host.enable(id, actor.clone()).await.map(|_| "плагин включён".to_string())
            } else {
                Ok("плагин выключен — включите его, когда нужно".to_string())
            };
            match started {
                Ok(how) => message = format!("{message} {}.", capitalized(&how)),
                Err(error) => message = format!("{message} Но не поднялась: {}", error.message),
            }
        }
        Ok(InstallReport {
            installed: true,
            version: Some(version),
            needs_approval: None,
            message,
            package: self.host.view_one(id).await.ok(),
        })
    }
}

/// Why a checked draft does not install, each in words.
fn problems_of(report: &CheckReport) -> Vec<String> {
    let mut problems = report.manifest.clone();
    problems.extend(report.code.iter().cloned());
    problems.extend(report.missing.iter().map(|handler| format!("нет функции {handler} в backend.js")));
    problems
}

/// The version an install gets: the draft's own, or the next patch while that one is taken.
fn next_version(root: &Path, id: &str, version: &semver::Version) -> semver::Version {
    let mut version = version.clone();
    while root.join(id).join(version.to_string()).exists() {
        version.patch += 1;
    }
    version
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Versions of a package of one's own beyond the newest `kept` go.
fn prune(root: &Path, id: &str, kept: usize) {
    let mut versions: Vec<semver::Version> = std::fs::read_dir(root.join(id))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| semver::Version::parse(&name).ok())
        .collect();
    versions.sort_by(|a, b| b.cmp(a));
    for old in versions.into_iter().skip(kept) {
        let _ = std::fs::remove_dir_all(root.join(id).join(old.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_drafts_files_to_plain_paths_and_nothing_a_browser_runs_in_its_assets() {
        assert_eq!(draft_path("backend.js").unwrap(), PathBuf::from("backend.js"));
        assert_eq!(draft_path("./manifest.json").unwrap(), PathBuf::from("manifest.json"));
        assert_eq!(draft_path("assets/stations.json").unwrap(), PathBuf::from("assets/stations.json"));
        for bad in [
            "../x",
            "assets/../../etc",
            "assets/.hidden",
            "other.js",
            "lib/a.js",
            "assets/page.html",
            "assets/a.svg",
            "",
        ] {
            assert!(draft_path(bad).is_err(), "{bad}");
        }
    }
}
