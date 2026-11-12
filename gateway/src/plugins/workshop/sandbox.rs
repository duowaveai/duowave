//! The workshop's sandbox: a draft's code in QuickJS with a stand-in `host` — its store in memory, its speech written
//! down, its network only to the manifest's hosts (checked as for a plugin that is on), its log collected. What the
//! station is known not to play is said as a warning (docs/plans/plugins.md, «Проверка вживую»): streams over http,
//! AAC.
//!
//! **The example** (docs/plans/voice-creation.md, «Пример вместо своих данных»): before the person's «да» a draft
//! never reaches the network. Its agent writes `examples.json` — stand-in answers of the network — and a run in the
//! example's mode takes each `fetch` from there by the start of its address; one the example lacks fails with «нет
//! примера для …», and nothing goes out. The desk's tile and «Примерка» run so; the plain settings a person gave
//! («Показывать: 3») go with it, the private ones never.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures::future::BoxFuture;
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::plugins::controller::{theme_form, JournalLine};
use crate::plugins::host_api::fetch_checked;
use crate::plugins::manifest::{self, Manifest, PackageKind, Permission, Runtime};
use crate::plugins::media::{kind_for, media_items};
use crate::plugins::phrases::REQUEST;
use crate::plugins::quickjs::QuickJs;
use crate::plugins::registry::MAX_CODE;
use crate::plugins::runtime::{CallKind, HostApi, PluginCode, PluginInstance, PluginRuntime};
use crate::utils::time::now_iso;

/// What the sandbox shows of a link: where the media goes in the sandbox (no real station).
const SANDBOX_MEDIA: &str = "http://<шлюз>/media";

/// The draft's example: stand-in answers of the network, at its root.
pub const EXAMPLES: &str = "examples.json";
/// An example's file at most this big: it is read for every run of the tile.
const MAX_EXAMPLES: u64 = 512 * 1024;

/// How a run reads the world: the real network to the manifest's hosts (the workshop's «Запустить»), or the draft's
/// example and never the network; with the plain settings given over the defaults.
#[derive(Debug, Clone, Default)]
pub struct Trying {
    pub example: bool,
    /// Plain settings only, checked against the manifest (`Workshop::plain_settings`).
    pub settings: Map<String, Value>,
}

impl Trying {
    pub fn example() -> Trying {
        Trying { example: true, settings: Map::new() }
    }
}

/// `examples.json`: `{ "fetch": [ { "url": "https://…", "status": 200, "json": … } ] }`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
struct Examples {
    #[serde(default)]
    fetch: Vec<FetchExample>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct FetchExample {
    /// The start of the addresses it answers; the longest that fits wins.
    url: String,
    /// GET, POST…; none — any.
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    headers: Option<Map<String, Value>>,
    /// The answer as JSON, or as text.
    #[serde(default)]
    json: Option<Value>,
    #[serde(default)]
    text: Option<String>,
}

impl Examples {
    /// The example of a draft; none when there is no file, an error when it does not read.
    fn read(folder: &Path) -> Result<Option<Examples>, String> {
        let file = folder.join(EXAMPLES);
        match std::fs::metadata(&file) {
            Err(_) => return Ok(None),
            Ok(meta) if meta.len() > MAX_EXAMPLES => {
                return Err(format!("{EXAMPLES} больше {} КБ", MAX_EXAMPLES / 1024));
            }
            Ok(_) => {}
        }
        let text = std::fs::read_to_string(&file).map_err(|error| format!("{EXAMPLES} не читается: {error}"))?;
        serde_json::from_str(&text).map(Some).map_err(|error| format!("{EXAMPLES} не читается: {error}"))
    }

    /// The network's answer from the example, as `fetch_checked` gives it: the address checked against the rights
    /// first, then the longest start that fits. Nothing goes out.
    fn answer(&self, hosts: &[String], request: &Value) -> Result<Value, String> {
        let text = request.get("url").and_then(Value::as_str).unwrap_or_default();
        let url = crate::plugins::host_api::checked_url(hosts, text)?;
        let method = request.get("method").and_then(Value::as_str).unwrap_or("GET").to_ascii_uppercase();
        let found = self
            .fetch
            .iter()
            .filter(|example| url.as_str().starts_with(example.url.as_str()) || text.starts_with(example.url.as_str()))
            .filter(|example| example.method.as_deref().is_none_or(|wanted| wanted.eq_ignore_ascii_case(&method)))
            .max_by_key(|example| example.url.len())
            .ok_or_else(|| {
                format!("нет примера для {}: допишите его в {EXAMPLES}", text.chars().take(200).collect::<String>())
            })?;
        let body = match (&found.json, &found.text) {
            (Some(json), _) => json.to_string(),
            (None, Some(text)) => text.clone(),
            (None, None) => String::new(),
        };
        let mut headers = found.headers.clone().unwrap_or_default();
        if found.json.is_some() && !headers.keys().any(|name| name.eq_ignore_ascii_case("content-type")) {
            headers.insert("content-type".into(), json!("application/json"));
        }
        Ok(json!({ "status": found.status.unwrap_or(200), "url": url.as_str(), "headers": headers, "body": body }))
    }
}

/// A stand-in `host` for a draft.
struct SandboxHost {
    manifest: Arc<Manifest>,
    kv: Arc<Mutex<Map<String, Value>>>,
    log: Arc<Mutex<Vec<JournalLine>>>,
    said: Arc<Mutex<Vec<String>>>,
    handle: Option<tokio::runtime::Handle>,
    /// Its settings: the defaults, the plain ones given over them.
    settings: Map<String, Value>,
    /// The example's mode: the network's answers from here, never from the network.
    examples: Option<Arc<Examples>>,
}

impl SandboxHost {
    fn kv_allowed(&self) -> Result<(), String> {
        if self.manifest.has(&Permission::Kv) {
            Ok(())
        } else {
            Err("нет права kv: в манифесте его нет".into())
        }
    }

    fn note(&self, level: &str, message: String) {
        let mut log = self.log.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if log.len() < 200 {
            log.push(JournalLine {
                at: now_iso(),
                level: level.to_string(),
                message: message.chars().take(1000).collect(),
            });
        }
    }
}

impl HostApi for SandboxHost {
    fn fetch(&self, request: Value) -> BoxFuture<'static, Result<Value, String>> {
        let url = request.get("url").and_then(Value::as_str).unwrap_or_default().to_string();
        if let Some(examples) = &self.examples {
            self.note("net", format!("fetch {} — из примера", url.chars().take(200).collect::<String>()));
            let answer = examples.answer(&self.manifest.hosts(), &request);
            return Box::pin(async move { answer });
        }
        self.note("net", format!("fetch {}", url.chars().take(200).collect::<String>()));
        fetch_checked(self.manifest.hosts(), &request, self.handle.clone())
    }

    fn kv_get(&self, key: &str) -> Result<Option<Value>, String> {
        self.kv_allowed()?;
        Ok(self.kv.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(key).cloned())
    }

    fn kv_set(&self, key: &str, value: Value) -> Result<(), String> {
        self.kv_allowed()?;
        if key.is_empty() || key.len() > 200 {
            return Err("ключ: от 1 до 200 байт".into());
        }
        if value.to_string().len() > 64 * 1024 {
            return Err("значение больше 64 КБ".into());
        }
        self.kv.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(key.to_string(), value);
        Ok(())
    }

    fn kv_delete(&self, key: &str) -> Result<(), String> {
        self.kv_allowed()?;
        self.kv.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(key);
        Ok(())
    }

    fn kv_keys(&self, prefix: &str) -> Result<Vec<String>, String> {
        self.kv_allowed()?;
        let kv = self.kv.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(kv.keys().filter(|key| key.starts_with(prefix)).cloned().collect())
    }

    fn say(&self, text: String, _device: Option<String>) -> BoxFuture<'static, Result<bool, String>> {
        let allowed = self.manifest.has(&Permission::VoiceSay);
        let said = self.said.clone();
        Box::pin(async move {
            if !allowed {
                return Err("нет права voice.say: в манифесте его нет".into());
            }
            said.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(text);
            Ok(true)
        })
    }

    fn settings(&self) -> Map<String, Value> {
        self.settings.clone()
    }

    fn log(&self, level: &str, message: &str) {
        self.note(level, message.to_string());
    }

    /// `host.schedule.*` (Host API 1.2): checked as the gateway checks it, set nowhere — the journal says what would
    /// be.
    fn call(&self, name: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        use crate::plugins::schedule;
        let scheduled = name.starts_with("schedule.");
        let answer = match name {
            _ if scheduled && !self.manifest.has(&Permission::Schedule) => {
                Err("нет права schedule: в манифесте его нет".to_string())
            }
            "schedule.set" => schedule::try_task(&input, &self.manifest.permissions).inspect(|set| {
                self.note("schedule", format!("поставлено бы: {set} — {input}"));
            }),
            "schedule.list" => Ok(Value::Array(Vec::new())),
            "schedule.cancel" => Ok(Value::Bool(false)),
            "schedule.parse" => Ok(schedule::parse_now(input.get("text").and_then(Value::as_str).unwrap_or_default())),
            "files.download" | "files.list" | "files.delete" if !self.manifest.has(&Permission::Files) => {
                Err("нет права files: в манифесте его нет".to_string())
            }
            "files.download" => {
                let url = input.get("url").and_then(Value::as_str).unwrap_or_default();
                let host =
                    url::Url::parse(url).ok().and_then(|url| url.host_str().map(str::to_string)).unwrap_or_default();
                if !crate::plugins::manifest::host_allowed(&self.manifest.hosts(), &host) {
                    Err(format!("хоста {host} нет в правах net:"))
                } else {
                    self.note("files", format!("скачала бы картинку {url}"));
                    Ok(json!({ "name": input.get("name").cloned().unwrap_or(Value::Null), "size": 0, "type": "?" }))
                }
            }
            "files.list" => Ok(Value::Array(Vec::new())),
            "files.delete" => Ok(Value::Bool(false)),
            "notify.send" if !self.manifest.has(&Permission::Notify) => {
                Err("нет права notify: в манифесте его нет".to_string())
            }
            "notify.send" => {
                let text = input.get("text").and_then(Value::as_str).unwrap_or_default();
                self.note("notify", format!("прислала бы владельцу: «{text}»"));
                Ok(Value::Bool(true))
            }
            other => Err(format!("нет функции {other}")),
        };
        Box::pin(async move { answer })
    }
}

/// «Проверить».
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckReport {
    /// Installable: the manifest reads, the code loads, every handler is there.
    pub ok: bool,
    pub kind: Option<PackageKind>,
    pub version: Option<String>,
    /// The manifest's problems.
    pub manifest: Vec<String>,
    /// Why the code did not load.
    pub code: Option<String>,
    pub exports: Vec<String>,
    /// Handlers the manifest names and the code does not export.
    pub missing: Vec<String>,
    /// What the station is known to do with it, and other advice.
    pub warnings: Vec<String>,
    pub ms: u64,
}

/// «Запустить».
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunReport {
    /// It ran on the draft's example: nothing went to the network.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub example: bool,
    pub ok: bool,
    pub result: Value,
    pub error: Option<String>,
    pub log: Vec<JournalLine>,
    /// What it would have said aloud (`host.voice.say`).
    pub said: Vec<String>,
    pub kv: Map<String, Value>,
    pub ms: u64,
}

/// What the station would get: speech, what plays, whether it listens on.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayView {
    pub title: String,
    pub url: String,
    pub format: String,
    pub r#type: String,
}

/// «Прогнать фразу».
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhraseReport {
    /// It ran on the draft's example: nothing went to the network.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub example: bool,
    pub text: String,
    /// The words as the router matches them.
    pub normalized: String,
    /// The phrase of the manifest that took them; none: the plugin does not hear them.
    pub matched: Option<String>,
    pub handler: Option<String>,
    pub capability: Option<String>,
    pub slots: Map<String, Value>,
    pub answer: Value,
    pub speech: Option<String>,
    pub play: Vec<PlayView>,
    pub listen: bool,
    /// It let the words go (null): the built-in skill of the same capability answers, if there is one.
    pub let_go: bool,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub log: Vec<JournalLine>,
    pub said: Vec<String>,
    pub ms: u64,
}

/// What the station is known not to play, and what plugins cannot do, as warnings on the code.
pub fn code_warnings(source: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    if source.contains("\"http://") || source.contains("'http://") || source.contains("`http://") {
        warnings.push(
            "Ссылка по http: поток из интернета по http колонка не играет и молчит — нужен https (проверено 07.11.2026)."
                .into(),
        );
    }
    let lower = source.to_ascii_lowercase();
    if lower.contains(".aac") || lower.contains("aacp") {
        warnings.push(
            "AAC колонка не играет: шлюз перекодирует его в MP3 на компьютере, на колонке — только с флагом MEDIA_TRANSCODE_ON_STATION. Лучше MP3 или HLS."
                .into(),
        );
    }
    if source.contains("setTimeout") || source.contains("setInterval") {
        warnings.push("setTimeout и setInterval нет: код работает, только когда его позвали.".into());
    }
    if source.lines().any(|line| line.trim_start().starts_with("import ")) {
        warnings.push("import не поддерживается: backend.js — один модуль (собирайте esbuild).".into());
    }
    warnings
}

/// What the workshop says of a draft with pictures while the gateway runs on the station (the owner, 12.11.2026:
/// «предупреждать об этом в мастерской, когда мы понимаем, что мы на колонке»).
pub fn station_warnings(folder: &Path) -> Vec<String> {
    let Ok((manifest, source)) = read_draft(folder) else { return Vec::new() };
    let pictures = manifest.has(&Permission::Files)
        || source.as_ref().is_some_and(|source| source.contains("host.files.") || source.contains("files.download"));
    if !pictures || manifest.prefer_home {
        return Vec::new();
    }
    vec!["Шлюз на колонке, а места там мало: картинка — до 1 МБ, все картинки плагинов — до 5 МБ. Лучше \"prefer\": \"home\" — тогда картинки живут на компьютере.".into()]
}

/// What a link of an item says about the station.
fn link_warnings(play: &[PlayView]) -> Vec<String> {
    let mut warnings = Vec::new();
    for item in play {
        if item.url.starts_with("http://") && !item.url.starts_with(SANDBOX_MEDIA) {
            warnings.push(format!("«{}»: ссылка по http — колонка её не сыграет, нужен https.", item.title));
        }
        let path = item.url.split(['?', '#']).next().unwrap_or_default().to_ascii_lowercase();
        if path.ends_with(".aac") || path.ends_with(".aacp") {
            warnings
                .push(format!("«{}»: AAC — колонка не играет, шлюз перекодирует только на компьютере.", item.title));
        }
    }
    warnings
}

/// The draft's manifest and code, read.
fn read_draft(folder: &Path) -> Result<(Manifest, Option<String>), Vec<String>> {
    let text =
        std::fs::read_to_string(folder.join("manifest.json")).map_err(|_| vec!["нет manifest.json".to_string()])?;
    let manifest = manifest::parse(&text).map_err(|error| error.problems)?;
    let code = folder.join("backend.js");
    let source = match std::fs::metadata(&code) {
        Ok(meta) if meta.len() > MAX_CODE => return Err(vec![format!("backend.js больше {} КБ", MAX_CODE / 1024)]),
        Ok(_) => {
            Some(std::fs::read_to_string(&code).map_err(|error| vec![format!("backend.js не читается: {error}")])?)
        }
        Err(_) => None,
    };
    Ok((manifest, source))
}

fn handlers(manifest: &Manifest) -> Vec<String> {
    let contributes = &manifest.contributes;
    let mut names: Vec<String> = contributes
        .intents
        .iter()
        .map(|intent| intent.handler.clone())
        .chain(contributes.tools.iter().map(|tool| tool.handler.clone()))
        .chain(contributes.media.iter().map(|media| media.handler.clone()))
        .chain(contributes.events.iter().map(|event| event.handler.clone()))
        .chain(contributes.schedules.iter().filter_map(|rhythm| rhythm.then.handler.clone()))
        .collect();
    if let Some(calls) = contributes.panel.as_ref().and_then(|panel| panel.get("calls")).and_then(Value::as_array) {
        names.extend(calls.iter().filter_map(Value::as_str).map(str::to_string));
    }
    names.sort();
    names.dedup();
    names
}

struct Started {
    instance: Arc<dyn PluginInstance>,
    log: Arc<Mutex<Vec<JournalLine>>>,
    said: Arc<Mutex<Vec<String>>>,
}

/// The draft's engine and its code: QuickJS for backend.js, wasmi for backend.wasm, an MCP server by its address.
fn engine(
    folder: &Path,
    id: &str,
    manifest: &Manifest,
    source: Option<String>,
) -> Result<(Arc<dyn PluginRuntime>, PluginCode), String> {
    let mut code = PluginCode {
        id: id.to_string(),
        version: manifest.version.to_string(),
        memory_mb: manifest.budget.memory_mb,
        hosts: manifest.hosts(),
        endpoint: manifest.mcp_url.clone(),
        ..PluginCode::default()
    };
    match manifest.runtime {
        Runtime::Js => {
            code.source = source.ok_or("нет backend.js")?;
            Ok((Arc::new(QuickJs), code))
        }
        #[cfg(feature = "wasm")]
        Runtime::Wasm => {
            code.bytes = std::fs::read(folder.join("backend.wasm")).map_err(|_| "нет backend.wasm".to_string())?;
            Ok((Arc::new(crate::plugins::wasm::Wasm), code))
        }
        #[cfg(not(feature = "wasm"))]
        Runtime::Wasm => {
            let _ = folder;
            Err("в этой сборке шлюза нет WebAssembly".into())
        }
        Runtime::Mcp => Ok((Arc::new(crate::plugins::mcp_runtime::McpRuntime), code)),
        Runtime::Process => Err("приложение проверяется своим запуском, не в песочнице плагинов".into()),
    }
}

async fn start(
    folder: &Path,
    id: &str,
    manifest: &Manifest,
    source: Option<String>,
    kv: Arc<Mutex<Map<String, Value>>>,
    handle: Option<tokio::runtime::Handle>,
    trying: &Trying,
) -> Result<Started, String> {
    let (log, said) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
    // Without a file every `fetch` fails with «нет примера»: still nothing goes out.
    let examples = match trying.example {
        true => Some(Arc::new(Examples::read(folder)?.unwrap_or_default())),
        false => None,
    };
    let host = Arc::new(SandboxHost {
        manifest: Arc::new(manifest.clone()),
        kv,
        log: log.clone(),
        said: said.clone(),
        handle,
        settings: manifest.settings_values(Some(&trying.settings)),
        examples,
    });
    let (runtime, code) = engine(folder, id, manifest, source)?;
    let instance = runtime.start(code, host).await.map_err(|error| error.to_string())?;
    Ok(Started { instance, log, said })
}

fn taken<T: Clone>(list: &Mutex<Vec<T>>) -> Vec<T> {
    list.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

pub async fn check(folder: &Path, id: &str, handle: Option<tokio::runtime::Handle>) -> CheckReport {
    let began = Instant::now();
    let mut report = CheckReport::default();
    let (manifest, source) = match read_draft(folder) {
        Ok(found) => found,
        Err(problems) => {
            report.manifest = problems;
            report.ms = began.elapsed().as_millis() as u64;
            return report;
        }
    };
    report.kind = Some(manifest.kind);
    report.version = Some(manifest.version.to_string());
    if manifest.id != id {
        report.manifest.push(format!("id «{}» в манифесте, а черновик — «{id}»", manifest.id));
    }
    if let Some(themes) = manifest.contributes.themes.as_ref().and_then(Value::as_array) {
        for theme in themes {
            if let Err(why) = theme_form(theme) {
                report.manifest.push(format!("тема: {why}"));
            }
        }
    }
    if manifest.kind == PackageKind::Theme
        && manifest.contributes.themes.as_ref().and_then(Value::as_array).is_none_or(Vec::is_empty)
    {
        report.manifest.push("тема без contributes.themes".into());
    }
    // What Host API 1.1 brought, under a range a 1.0 gateway also takes: there it would refuse the rights or lack the
    // functions.
    let admits_1_0 =
        semver::VersionReq::parse(&manifest.api).is_ok_and(|range| range.matches(&semver::Version::new(1, 0, 0)));
    let uses_1_1 = manifest.permissions.iter().any(Permission::since_1_1)
        || source.as_ref().is_some_and(|source| {
            ["host.call(", "host.content.", "host.tools.", "host.smartHome.", "host.home."]
                .iter()
                .any(|word| source.contains(word))
        });
    // The turn timer (Host API 1.2) under a range a 1.1 gateway takes: there the station would never say «время
    // вышло».
    let admits_1_1 =
        semver::VersionReq::parse(&manifest.api).is_ok_and(|range| range.matches(&semver::Version::new(1, 1, 0)));
    let uses_1_2 = manifest.permissions.iter().any(Permission::since_1_2)
        || source.as_ref().is_some_and(|source| source.contains("timeout:") && source.contains("seconds"));
    // The keyboard (Host API 1.3) under a range a 1.2 gateway takes: there the right is unknown.
    let admits_1_2 =
        semver::VersionReq::parse(&manifest.api).is_ok_and(|range| range.matches(&semver::Version::new(1, 2, 0)));
    let uses_1_3 = manifest.permissions.iter().any(Permission::since_1_3);
    let wanted = handlers(&manifest);
    let has_code = source.is_some() || manifest.runtime != Runtime::Js;
    match (has_code, wanted.is_empty()) {
        (false, false) => report.code = Some("нужен backend.js: его функции обслуживают то, что плагин даёт".into()),
        (false, true) => {}
        (true, _) => {
            if let Some(source) = &source {
                report.warnings = code_warnings(source);
            }
            match start(folder, id, &manifest, source, Arc::new(Mutex::new(Map::new())), handle, &Trying::default())
                .await
            {
                Ok(started) => {
                    report.exports = started.instance.exports();
                    report.missing = wanted.into_iter().filter(|handler| !report.exports.contains(handler)).collect();
                    started.instance.stop();
                }
                Err(error) => report.code = Some(error),
            }
        }
    }
    if admits_1_0 && uses_1_1 {
        report.warnings.push(
            "Плагин пользуется Host API 1.1 (host.call, host.content, host.tools, host.smartHome, host.home или их права), а манифест пускает и шлюз 1.0: поставьте \"api\": \"^1.1\"."
                .into(),
        );
    }
    if admits_1_1 && uses_1_2 {
        report.warnings.push(
            "Плагин пользуется Host API 1.2 (таймер хода continue.timeout или право schedule), а манифест пускает и шлюз 1.1, где этого нет: поставьте \"api\": \"^1.2\"."
                .into(),
        );
    }
    if admits_1_2 && uses_1_3 {
        report.warnings.push(
            "Приложение слышит MIDI-клавиатуру (право midi, Host API 1.3), а манифест пускает и шлюз 1.2, где этого нет: поставьте \"api\": \"^1.3\"."
                .into(),
        );
    }
    if manifest.has(&Permission::Midi)
        && manifest.runs.iter().any(|placement| *placement != crate::core::availability::Placement::Pc)
    {
        report.warnings.push(
            "MIDI-клавиатура есть только у шлюза на компьютере: на колонке приложение будет работать без неё.".into(),
        );
    }
    // Before the «да» the tile and «Примерка» show the example: a draft that reaches the network needs one.
    match Examples::read(folder) {
        Err(why) => report.warnings.push(format!("Пример не читается: {why}.")),
        Ok(None) if !manifest.hosts().is_empty() && manifest.kind != PackageKind::Theme => report.warnings.push(format!(
            "Нужен пример: черновик ходит в сеть ({}), а {EXAMPLES} нет. До «да» плитка и «Примерка» показывают пример — без него там будет пусто.",
            manifest.hosts().join(", ")
        )),
        _ => {}
    }
    if manifest.contributes.panel.is_some() && !folder.join("panel.js").is_file() && !folder.join("panel.css").is_file()
    {
        report.warnings.push("contributes.panel без panel.js: в пульте плагину нечего показать.".into());
    }
    // The panel part runs in the browser: here only whether it parses.
    if let Ok(panel) = std::fs::read_to_string(folder.join("panel.js")) {
        // Web MIDI in the browser is the whole panel's once Chrome allowed it: an app takes the keyboard only with
        // its right (docs/plans/midi.md, «Запасной путь»).
        if !manifest.has(&Permission::Midi) && ["useMidi", "requestMIDIAccess"].iter().any(|word| panel.contains(word))
        {
            report.warnings.push(
                "panel.js берёт MIDI-клавиатуру (useMidi или requestMIDIAccess), а права midi в манифесте нет: добавьте \"midi\" в permissions.".into(),
            );
        }
        if let Err(error) = crate::plugins::quickjs::syntax_of("panel.js", &panel).await {
            report.code = Some(match report.code.take() {
                Some(before) => format!("{before}\npanel.js: {error}"),
                None => format!("panel.js: {error}"),
            });
        }
    }
    report.ok = report.manifest.is_empty() && report.code.is_none() && report.missing.is_empty();
    report.ms = began.elapsed().as_millis() as u64;
    report
}

pub async fn run(
    folder: &Path,
    id: &str,
    function: &str,
    input: Value,
    kv: Arc<Mutex<Map<String, Value>>>,
    handle: Option<tokio::runtime::Handle>,
    trying: &Trying,
) -> Result<RunReport, String> {
    let began = Instant::now();
    let (manifest, source) = read_draft(folder).map_err(|problems| problems.join("; "))?;
    let started = start(folder, id, &manifest, source, kv.clone(), handle, trying).await?;
    let answer = started.instance.call(function, input, CallKind::Tool).await;
    started.instance.stop();
    let (ok, result, error) = match answer {
        Ok(value) => (true, value, None),
        Err(error) => (false, Value::Null, Some(error.to_string())),
    };
    Ok(RunReport {
        example: trying.example,
        ok,
        result,
        error,
        log: taken(&started.log),
        said: taken(&started.said),
        kv: kv.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone(),
        ms: began.elapsed().as_millis() as u64,
    })
}

pub async fn try_phrase(
    folder: &Path,
    id: &str,
    text: &str,
    kv: Arc<Mutex<Map<String, Value>>>,
    handle: Option<tokio::runtime::Handle>,
    trying: &Trying,
) -> Result<PhraseReport, String> {
    let began = Instant::now();
    let (manifest, source) = read_draft(folder).map_err(|problems| problems.join("; "))?;
    let normalized = crate::skills::text::normalize(text);
    let found = manifest.contributes.intents.iter().find_map(|intent| {
        intent
            .phrases
            .iter()
            .find_map(|phrase| phrase.matches(&normalized).map(|slots| (intent, phrase.template.clone(), slots)))
    });
    let mut report = PhraseReport {
        example: trying.example,
        text: text.to_string(),
        normalized: normalized.clone(),
        matched: None,
        handler: None,
        capability: None,
        slots: Map::new(),
        answer: Value::Null,
        speech: None,
        play: Vec::new(),
        listen: false,
        let_go: false,
        error: None,
        warnings: Vec::new(),
        log: Vec::new(),
        said: Vec::new(),
        ms: 0,
    };
    let Some((intent, template, slots)) = found else {
        report.ms = began.elapsed().as_millis() as u64;
        return Ok(report);
    };
    report.matched = Some(template);
    report.handler = Some(intent.handler.clone());
    report.capability = Some(intent.capability.as_str().to_string());
    report.slots = slots.clone();
    let started = start(folder, id, &manifest, source, kv, handle, trying).await?;
    let mut input = slots;
    input.insert(
        REQUEST.into(),
        json!({ "text": text, "device": "sandbox", "timezone": "Europe/Moscow", "capability": intent.capability }),
    );
    let answer = started.instance.call(&intent.handler, Value::Object(input), CallKind::Voice).await;
    started.instance.stop();
    report.log = taken(&started.log);
    report.said = taken(&started.said);
    match answer {
        Err(error) => {
            report.error = Some(error.to_string());
            report.let_go = true;
        }
        Ok(answer) => {
            match &answer {
                Value::String(text) if !text.trim().is_empty() => report.speech = Some(text.clone()),
                Value::Object(object) => {
                    report.speech = object.get("say").and_then(Value::as_str).map(str::to_string);
                    if let Some(play) = object.get("play") {
                        report.play = media_items(id, kind_for(intent.capability), play, Some(SANDBOX_MEDIA))
                            .into_iter()
                            .map(|item| PlayView {
                                title: item.title,
                                url: item.url,
                                format: item.format,
                                r#type: item.r#type,
                            })
                            .collect();
                        if report.play.is_empty() {
                            report.warnings.push(
                                "play есть, но ни один элемент не годится: нужны id, title и url по http(s) или path"
                                    .into(),
                            );
                        }
                    }
                    report.listen = object.get("listen").and_then(Value::as_bool).unwrap_or(false);
                    report.warnings.extend(crate::plugins::skill::turn_warning(&answer));
                }
                _ => report.let_go = true,
            }
            report.answer = answer;
        }
    }
    report.warnings.extend(link_warnings(&report.play));
    report.ms = began.elapsed().as_millis() as u64;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(dir: &Path, manifest: Value, code: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
        std::fs::write(dir.join("backend.js"), code).unwrap();
    }

    fn manifest() -> Value {
        json!({
            "schema": 1, "id": "jazz", "version": "1.0.0", "title": "Джаз", "api": "^1", "runs": ["pc", "station", "stationServer"],
            "permissions": ["voice.intents", "kv"],
            "contributes": { "intents": [{ "phrases": ["включи джаз {mood}", "включи джаз"], "handler": "play", "capability": "radio" }] }
        })
    }

    #[tokio::test]
    async fn checks_runs_and_plays_a_phrase_in_the_sandbox_and_says_what_the_station_does_not_play() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("jazz");
        draft(
            &folder,
            manifest(),
            r#"
            export function play({ mood, request }) {
              host.kv.set("count", (host.kv.get("count") ?? 0) + 1);
              console.log(`настроение ${mood ?? "любое"}`);
              return { say: "Включаю джаз", play: [{ id: "j", title: "Джаз", url: "http://main.hostingradio.ru/jazz-128.mp3" }] };
            }
            "#,
        );
        let checked = check(&folder, "jazz", None).await;
        assert!(checked.ok, "{checked:?}");
        assert_eq!(checked.exports, ["play"]);
        assert!(checked.warnings[0].contains("по http"), "{:?}", checked.warnings);

        let kv = Arc::new(Mutex::new(Map::new()));
        let phrase = try_phrase(&folder, "jazz", "Алиса, включи джаз спокойный!", kv.clone(), None, &Trying::default())
            .await
            .unwrap();
        assert_eq!(phrase.matched.as_deref(), Some("включи джаз {mood}"));
        assert_eq!(phrase.slots["mood"], "спокойный");
        assert_eq!(phrase.speech.as_deref(), Some("Включаю джаз"));
        assert_eq!(phrase.play[0].url, "http://main.hostingradio.ru/jazz-128.mp3");
        assert!(phrase.warnings.iter().any(|warning| warning.contains("колонка её не сыграет")));
        assert_eq!(phrase.log[0].message, "настроение спокойный");
        let missed = try_phrase(&folder, "jazz", "включи рок", kv.clone(), None, &Trying::default()).await.unwrap();
        assert!(missed.matched.is_none());

        // The store is the draft's between runs.
        let ran = run(&folder, "jazz", "play", json!({}), kv.clone(), None, &Trying::default()).await.unwrap();
        assert!(ran.ok);
        assert_eq!(ran.kv["count"], 2);
        let missing = run(&folder, "jazz", "nothing", json!({}), kv, None, &Trying::default()).await.unwrap();
        assert!(!missing.ok && missing.error.unwrap().contains("nothing"));

        // A handler the code does not export, code that does not load.
        draft(&folder, manifest(), "export function other() {}");
        let checked = check(&folder, "jazz", None).await;
        assert_eq!((checked.ok, checked.missing.clone()), (false, vec!["play".to_string()]));
        draft(&folder, manifest(), "export function play( {");
        assert!(check(&folder, "jazz", None).await.code.is_some());

        // Host API 1.1's functions under a range a 1.0 gateway takes: a warning, gone with "^1.1".
        let newer = "export function play() { return host.content.search(\"music\", \"\", 5); }";
        draft(&folder, manifest(), newer);
        let checked = check(&folder, "jazz", None).await;
        assert!(checked.ok && checked.warnings.iter().any(|warning| warning.contains("^1.1")), "{checked:?}");
        let mut raised = manifest();
        raised["api"] = json!("^1.1");
        draft(&folder, raised, newer);
        assert!(check(&folder, "jazz", None).await.warnings.is_empty());
    }

    /// Stage 3 of docs/plans/voice-creation.md: «Дела на сегодня» without its key shows the example, and nothing goes
    /// to the network — an address the example lacks fails, a host without the right is refused, a missing example
    /// is said by «Проверить».
    #[tokio::test]
    async fn a_draft_runs_on_its_example_and_never_reaches_the_network() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("todo");
        draft(
            &folder,
            json!({
                "schema": 1, "id": "todo", "version": "0.1.0", "title": "Дела на сегодня", "api": "^1", "runs": ["pc"],
                "permissions": ["net:api.todoist.com"],
                "settings": [
                    { "key": "token", "type": "string", "title": "Ключ Todoist", "secret": true },
                    { "key": "count", "type": "number", "title": "Показывать", "default": 5 }
                ],
                "contributes": { "panel": { "slots": ["desk.widgets"], "calls": ["panelData", "other"] } }
            }),
            r#"
            export async function panelData() {
              const token = host.settings.get("token") ?? "";
              const answer = await host.http.fetch("https://api.todoist.com/rest/v2/tasks?filter=today",
                { headers: { Authorization: `Bearer ${token}` } });
              const tasks = await answer.json();
              return { status: answer.status, tasks: tasks.slice(0, host.settings.get("count")).map((task) => task.content) };
            }
            export async function other({ url }) { return (await host.http.fetch(url)).status; }
            "#,
        );
        std::fs::write(folder.join("panel.js"), "export default {};").unwrap();
        let checked = check(&folder, "todo", None).await;
        assert!(
            checked.ok && checked.warnings.iter().any(|warning| warning.starts_with("Нужен пример")),
            "{checked:?}"
        );

        // No example yet: in the example's mode nothing goes out, the run says what is missing.
        let kv = Arc::new(Mutex::new(Map::new()));
        let ran = run(&folder, "todo", "panelData", json!({}), kv.clone(), None, &Trying::example()).await.unwrap();
        assert!(!ran.ok && ran.example, "{ran:?}");
        assert!(ran
            .error
            .as_deref()
            .unwrap()
            .contains("нет примера для https://api.todoist.com/rest/v2/tasks?filter=today"));

        let examples = json!({ "fetch": [
            { "url": "https://api.todoist.com/rest/v2/", "status": 500, "text": "не то" },
            { "url": "https://api.todoist.com/rest/v2/tasks", "json": [
                { "content": "Купить хлеб" }, { "content": "Позвонить маме" }, { "content": "Полить цветы" }
            ] }
        ] });
        std::fs::write(folder.join(EXAMPLES), examples.to_string()).unwrap();
        assert!(check(&folder, "todo", None).await.warnings.is_empty());
        let mut count = Map::new();
        count.insert("count".into(), json!(2));
        let trying = Trying { example: true, settings: count };
        let ran = run(&folder, "todo", "panelData", json!({}), kv.clone(), None, &trying).await.unwrap();
        assert!(ran.ok, "{ran:?}");
        assert_eq!(
            ran.result,
            json!({ "status": 200, "tasks": ["Купить хлеб", "Позвонить маме"] }),
            "the longest start"
        );
        assert!(ran.log.iter().any(|line| line.message.ends_with("из примера")), "{:?}", ran.log);
        // An address the example lacks, a host without the right: refused, nothing goes out.
        let input = json!({ "url": "https://api.todoist.com/sync/v9" });
        let lacked = run(&folder, "todo", "other", input, kv.clone(), None, &Trying::example()).await.unwrap();
        assert!(lacked.error.as_deref().unwrap().contains("нет примера для https://api.todoist.com/sync/v9"));
        let input = json!({ "url": "https://evil.example/steal" });
        let refused = run(&folder, "todo", "other", input, kv.clone(), None, &Trying::example()).await.unwrap();
        assert!(refused.error.as_deref().unwrap().contains("не разрешён"), "{refused:?}");
        // Off the example, the network is the real one (none in this test).
        let real = run(&folder, "todo", "panelData", json!({}), kv, None, &Trying::default()).await.unwrap();
        assert!(!real.ok && !real.example && real.error.as_deref().unwrap().contains("сеть недоступна"), "{real:?}");

        std::fs::write(folder.join(EXAMPLES), "{ не json").unwrap();
        assert!(check(&folder, "todo", None)
            .await
            .warnings
            .iter()
            .any(|warning| warning.starts_with("Пример не читается")));
    }

    #[tokio::test]
    async fn the_quiz_template_passes_says_time_is_up_and_tells_how_long_the_station_listens() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("quiz");
        std::fs::create_dir_all(&folder).unwrap();
        for (path, text) in crate::plugins::workshop::templates::quiz("quiz", "Викторина") {
            std::fs::write(folder.join(path), text).unwrap();
        }
        let checked = check(&folder, "quiz", None).await;
        assert!(checked.ok && checked.warnings.is_empty(), "{checked:?}");
        let kv = Arc::new(Mutex::new(Map::new()));
        let phrase =
            try_phrase(&folder, "quiz", "Алиса, давай викторину", kv.clone(), None, &Trying::default()).await.unwrap();
        assert_eq!(phrase.answer["continue"]["timeout"]["seconds"], 10);
        assert!(phrase.speech.as_deref().is_some_and(|speech| speech.ends_with("Столица Австралии?")), "{phrase:?}");
        assert!(phrase.warnings.iter().any(|warning| warning.contains("около 5 с")), "{:?}", phrase.warnings);
        // Silence: the timer's function says time is up and asks the next question.
        let silence = json!({ "timeout": { "seconds": 10, "missed": 1 }, "request": { "device": "sandbox" } });
        let up = run(&folder, "quiz", "timeUp", silence, kv.clone(), None, &Trying::default()).await.unwrap();
        let said = up.result["say"].as_str().unwrap_or_default().to_string();
        assert!(said.starts_with("Время вышло! Правильный ответ: канберра.") && said.ends_with('?'), "{up:?}");
        // The same timer under a range a 1.1 gateway takes: a warning to raise it.
        let mut manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(folder.join("manifest.json")).unwrap()).unwrap();
        manifest["api"] = json!("^1.1");
        std::fs::write(folder.join("manifest.json"), manifest.to_string()).unwrap();
        assert!(check(&folder, "quiz", None).await.warnings.iter().any(|warning| warning.contains("^1.2")));
    }

    #[tokio::test]
    async fn the_midi_template_passes_reads_its_tunes_takes_ones_own_and_hears_a_name() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("uchitel");
        std::fs::create_dir_all(&folder).unwrap();
        for (path, text) in crate::plugins::workshop::templates::midi("uchitel", "Учитель «пианино»") {
            std::fs::write(folder.join(path), text).unwrap();
        }
        let checked = check(&folder, "uchitel", None).await;
        assert!(checked.ok && checked.warnings.is_empty(), "{checked:?}");
        let panel = std::fs::read_to_string(folder.join("panel.js")).unwrap();
        assert!(panel.contains(r#"title: "Учитель «пианино»""#) && !panel.contains("__"), "the title filled in");
        let kv = Arc::new(Mutex::new(Map::new()));
        let state = run(&folder, "uchitel", "state", json!({}), kv.clone(), None, &Trying::default()).await.unwrap();
        let melodies = state.result["melodies"].as_array().unwrap();
        assert_eq!(melodies.len(), 8);
        assert_eq!(state.result["current"], "oda");
        // Every tune reads into notes within two octaves: it fits 25 keys.
        for melody in melodies {
            let notes = melody["notes"].as_array().unwrap();
            assert!(notes.len() >= 10, "{}", melody["title"]);
            let pitches: Vec<i64> = notes.iter().map(|note| note[0].as_i64().unwrap()).collect();
            let span = pitches.iter().max().unwrap() - pitches.iter().min().unwrap();
            assert!(span <= 24, "{} spans {span}", melody["title"]);
        }
        assert_eq!(melodies[0]["notes"][0], json!([64, 0, 1]), "E4 first, at 0, a beat");
        // One's own tune from a file, then by voice.
        let added = run(
            &folder,
            "uchitel",
            "add",
            json!({ "title": "Своя", "tempo": 90, "notes": [[60, 0, 1], [62, 1, 1]] }),
            kv.clone(),
            None,
            &Trying::default(),
        )
        .await
        .unwrap();
        assert!(added.result["id"].as_str().unwrap().starts_with("own-"));
        let phrase = try_phrase(&folder, "uchitel", "Алиса, давай учить ёлочку", kv.clone(), None, &Trying::default())
            .await
            .unwrap();
        assert_eq!(phrase.speech.as_deref(), Some("Учим «В лесу родилась ёлочка». Клавиатура ждёт в пульте."));
        let done = run(
            &folder,
            "uchitel",
            "done",
            json!({ "id": "yolochka", "mistakes": 3 }),
            kv.clone(),
            None,
            &Trying::default(),
        )
        .await
        .unwrap();
        assert_eq!(done.result["times"], 1);
        // The keyboard's facts by its name.
        let board = run(
            &folder,
            "uchitel",
            "keyboard",
            json!({ "name": "Keys", "keys": 49, "low": 36 }),
            kv.clone(),
            None,
            &Trying::default(),
        )
        .await
        .unwrap();
        assert_eq!(board.result, json!({ "keys": 49, "low": 36 }));
        // 37 keys, as an Arturia KeyStep 37 has; «Клавиатура заново» forgets what was said of it.
        let keystep = run(
            &folder,
            "uchitel",
            "keyboard",
            json!({ "name": "Arturia KeyStep 37", "keys": 37, "low": 48 }),
            kv.clone(),
            None,
            &Trying::default(),
        )
        .await
        .unwrap();
        assert_eq!(keystep.result, json!({ "keys": 37, "low": 48 }));
        let forgotten = run(
            &folder,
            "uchitel",
            "keyboard",
            json!({ "name": "Arturia KeyStep 37", "reset": true }),
            kv.clone(),
            None,
            &Trying::default(),
        )
        .await
        .unwrap();
        assert_eq!(forgotten.result, json!({}));
        let state = run(&folder, "uchitel", "state", json!({}), kv, None, &Trying::default()).await.unwrap();
        assert_eq!(state.result["keyboards"], json!({ "Keys": { "keys": 49, "low": 36 } }), "the other one kept");
        // Without the right `midi` the check says so.
        let mut manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(folder.join("manifest.json")).unwrap()).unwrap();
        manifest["permissions"] = json!(["kv", "voice.intents"]);
        std::fs::write(folder.join("manifest.json"), manifest.to_string()).unwrap();
        assert!(check(&folder, "uchitel", None).await.warnings.iter().any(|warning| warning.contains("права midi")));
    }
}
