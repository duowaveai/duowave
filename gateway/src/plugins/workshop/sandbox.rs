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
use crate::plugins::manifest::{self, Manifest, PackageKind, Runtime};
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

/// `examples.json`: `{ "fetch": [ { "url": "https://…", "status": 200, "json": … } ],
/// "bus": [ { "to": "system/smart-home", "handle": "devices", "result": … } ] }` — the network's answers, and the
/// answers of other participants of the bus, which the sandbox has none of.
#[derive(Debug, Clone, Default, serde::Deserialize)]
struct Examples {
    #[serde(default)]
    fetch: Vec<FetchExample>,
    #[serde(default)]
    bus: Vec<BusExample>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct BusExample {
    to: String,
    handle: String,
    #[serde(default)]
    result: Value,
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
                return Err(crate::tr!(
                    "{EXAMPLES} больше {max_examples} КБ",
                    EXAMPLES = EXAMPLES,
                    max_examples = MAX_EXAMPLES / 1024
                ));
            }
            Ok(_) => {}
        }
        let text = std::fs::read_to_string(&file)
            .map_err(|error| crate::tr!("{EXAMPLES} не читается: {error}", EXAMPLES = EXAMPLES, error = error))?;
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| crate::tr!("{EXAMPLES} не читается: {error}", EXAMPLES = EXAMPLES, error = error))
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
                crate::tr!(
                    "нет примера для {list}: допишите его в {EXAMPLES}",
                    list = text.chars().take(200).collect::<String>(),
                    EXAMPLES = EXAMPLES
                )
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
    /// The bus's answers from the example in any mode: the sandbox has no bus.
    bus: Vec<BusExample>,
}

/// The draft's own folder in the sandbox: kept in its `kv` under this mark, as long as the draft's `kv` lives.
const FOLDER: &str = "\u{1}folder:";

impl SandboxHost {
    /// Its own store: always its own (Host API 2).
    fn kv_allowed(&self) -> Result<(), String> {
        Ok(())
    }

    /// Whether the draft asked to reach `to` in its manifest: the sandbox's whole bus is what it asked for.
    fn reaches(&self, to: &str) -> Result<(), String> {
        if self.manifest.asks(to) > crate::bus::Level::None {
            Ok(())
        } else {
            Err(crate::tr!("нет такого: {to} — приложение не просило связь (permissions: call:{to})", to = to))
        }
    }

    /// A path in the draft's own folder: `tmp/a.txt`, or `files/app/<id>/…`; the household's folders are not here.
    fn own_path(&self, at: &str) -> Result<String, String> {
        let own = format!("files/app/{}/", self.manifest.id);
        let path = match at.strip_prefix(&own) {
            Some(rest) => rest.to_string(),
            None if at.starts_with("files/") => {
                return Err(crate::tr!("в песочнице только своя папка: {at} — после установки", at = at))
            }
            None => at.trim_matches('/').to_string(),
        };
        if path.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
            return Err(crate::tr!("путь «{at}» не читается", at = at));
        }
        Ok(path)
    }

    fn folder(&self, name: &str, input: &Value) -> Result<Value, String> {
        let at = input.get("at").and_then(Value::as_str).unwrap_or_default();
        // A folder of the home folder (docs/plans/shared-folders.md): the sandbox has none — it answers from the
        // example (`examples.json`, «bus»: `{to: "files/home/Фото", handle: "list", result}`), for what the draft asked.
        let own = format!("files/app/{}", self.manifest.id);
        if at.starts_with("files/") && at != own && !at.starts_with(&format!("{own}/")) {
            let handle = name.trim_start_matches("folders.");
            if self.manifest.asks(at) == crate::bus::Level::None {
                return Err(crate::tr!(
                    "нет такого: {at} — приложение не просило связь (permissions: call:{at})",
                    at = at
                ));
            }
            return self
                .bus
                .iter()
                .find(|example| example.to == at && example.handle == handle)
                .map(|example| {
                    self.note("files", crate::tr!("{at}: {handle} — из примера", at = at, handle = handle));
                    example.result.clone()
                })
                .ok_or_else(|| {
                    crate::tr!("в «Примерке» домашней папки нет: ответ «{at}» на {handle} — допишите пример в {EXAMPLES}, «bus»", at = at, handle = handle, EXAMPLES = EXAMPLES)
                });
        }
        let mut kv = self.kv.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if name == "folders.list" {
            let base = if at.is_empty() { String::new() } else { format!("{}/", self.own_path(at)?) };
            let mut items: Vec<Value> = Vec::new();
            for (key, value) in kv.iter() {
                let Some(rest) = key.strip_prefix(FOLDER).and_then(|path| path.strip_prefix(&base)) else { continue };
                let (first, deeper) = rest.split_once('/').map_or((rest, false), |(first, _)| (first, true));
                if items.iter().any(|item| item["name"] == first) {
                    continue;
                }
                let size = value.get("size").and_then(Value::as_u64).unwrap_or(0);
                items.push(json!({ "name": first, "folder": deeper, "size": if deeper { 0 } else { size } }));
            }
            return Ok(json!({ "items": items }));
        }
        let path = self.own_path(at)?;
        let key = format!("{FOLDER}{path}");
        match name {
            "folders.write" => {
                let (text, base64) =
                    (input.get("text").and_then(Value::as_str), input.get("base64").and_then(Value::as_str));
                let size = text.map(str::len).or_else(|| base64.map(|b| b.len() * 3 / 4)).unwrap_or(0);
                kv.insert(key, json!({ "text": text, "base64": base64, "size": size }));
                Ok(json!({ "at": format!("files/app/{}/{path}", self.manifest.id), "size": size }))
            }
            "folders.read" => {
                let file = kv.get(&key).ok_or_else(|| crate::tr!("нет такого: {at}", at = at))?;
                Ok(json!({ "text": file.get("text"), "base64": file.get("base64"), "size": file.get("size") }))
            }
            "folders.stat" => match kv.get(&key) {
                Some(file) => Ok(json!({ "size": file.get("size"), "folder": false })),
                None if kv.keys().any(|known| known.starts_with(&format!("{key}/"))) => Ok(json!({ "folder": true })),
                None => Err(crate::tr!("нет такого: {at}", at = at)),
            },
            "folders.remove" => {
                let inside = format!("{key}/");
                let before = kv.len();
                kv.retain(|known, _| *known != key && !known.starts_with(&inside));
                Ok(json!({ "removed": kv.len() < before, "bin": false }))
            }
            "folders.move" => {
                let to = input.get("to").and_then(Value::as_str).unwrap_or_default();
                let target = format!("{FOLDER}{}", self.own_path(to)?);
                let file = kv.remove(&key).ok_or_else(|| crate::tr!("нет такого: {at}", at = at))?;
                kv.insert(target, file);
                Ok(json!({ "moved": true }))
            }
            "folders.mkdir" => Ok(json!({ "made": true })),
            other => Err(crate::tr!("нет функции {other}", other = other)),
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
            self.note("net", crate::tr!("fetch {list} — из примера", list = url.chars().take(200).collect::<String>()));
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
            return Err(crate::tr!("ключ: от 1 до 200 байт"));
        }
        if value.to_string().len() > 64 * 1024 {
            return Err(crate::tr!("значение больше 64 КБ"));
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
        Ok(kv.keys().filter(|key| key.starts_with(prefix) && !key.starts_with(FOLDER)).cloned().collect())
    }

    /// Speech without a request is a letter to `system/voice`: what was said goes to the report.
    fn say(&self, text: String, _device: Option<String>) -> BoxFuture<'static, Result<bool, String>> {
        let allowed = self.reaches("system/voice");
        let said = self.said.clone();
        Box::pin(async move {
            allowed?;
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

    /// Host API 2: letters on the bus and the own folder. The sandbox has no bus: what the draft asked for answers from
    /// the example (`examples.json`, «bus»), the gateway's own services say in the journal what they would do, the
    /// schedule is checked as the gateway checks it and set nowhere; the own folder lives in the draft's `kv`.
    fn call(&self, name: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        use crate::plugins::schedule;
        let field = |key: &str| input.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        let answer = match name {
            "bus.call" => {
                let (to, handle) = (field("to"), field("handle"));
                let letter = input.get("input").cloned().unwrap_or(Value::Null);
                self.reaches(&to).and_then(|()| match (to.as_str(), handle.as_str()) {
                    ("system/voice", "say") => {
                        let text = letter.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
                        self.said.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(text);
                        Ok(Value::Bool(true))
                    }
                    ("system/notify", "send") => {
                        let text = letter.get("text").and_then(Value::as_str).unwrap_or_default();
                        self.note("notify", crate::tr!("прислала бы владельцу: «{text}»", text = text));
                        Ok(Value::Bool(true))
                    }
                    ("system/schedule", "set") => {
                        schedule::try_task(&letter, &self.manifest.permissions).inspect(|set| {
                            self.note(
                                "schedule",
                                crate::tr!("поставлено бы: {set} — {letter}", set = set, letter = letter),
                            )
                        })
                    }
                    ("system/schedule", "list") => Ok(Value::Array(Vec::new())),
                    ("system/schedule", "cancel") => Ok(Value::Bool(false)),
                    ("system/schedule", "parse") => {
                        Ok(schedule::parse_now(letter.get("text").and_then(Value::as_str).unwrap_or_default()))
                    }
                    _ => self
                        .bus
                        .iter()
                        .find(|example| example.to == to && example.handle == handle)
                        .map(|example| {
                            self.note("bus", crate::tr!("{to}: {handle} — из примера", to = to, handle = handle));
                            example.result.clone()
                        })
                        .ok_or_else(|| {
                            crate::tr!(
                                "в песочнице шины нет: ответ «{to}» на {handle} — допишите пример в {EXAMPLES}, «bus»",
                                to = to,
                                handle = handle,
                                EXAMPLES = EXAMPLES
                            )
                        }),
                })
            }
            "bus.ask" => {
                let to = field("to");
                self.reaches(&to).and_then(|()| {
                    self.bus
                        .iter()
                        .find(|example| example.to == to && example.handle == "ask")
                        .map(|example| example.result.clone())
                        .ok_or_else(|| {
                            crate::tr!(
                                "в песочнице шины нет: ответ «{to}» словами — пример в {EXAMPLES}, «bus», handle «ask»",
                                to = to,
                                EXAMPLES = EXAMPLES
                            )
                        })
                })
            }
            "bus.tell" => {
                let event = field("event");
                if self.manifest.contributes.bus.emits.contains(&event) {
                    self.note("bus", crate::tr!("рассказала бы новость «{event}»", event = event));
                    Ok(Value::Bool(true))
                } else {
                    Err(crate::tr!("новость «{event}» не объявлена в contributes.bus.emits", event = event))
                }
            }
            "bus.list" => Ok(Value::Array(
                self.manifest
                    .permissions
                    .iter()
                    .filter_map(|permission| match permission {
                        crate::plugins::manifest::Permission::Bus { pattern, .. } => {
                            Some(json!({ "address": pattern.as_str(), "title": pattern.as_str(), "handles": [] }))
                        }
                        _ => None,
                    })
                    .collect(),
            )),
            "folders.download" => {
                let url = field("url");
                let host =
                    url::Url::parse(&url).ok().and_then(|url| url.host_str().map(str::to_string)).unwrap_or_default();
                if !crate::plugins::manifest::host_allowed(&self.manifest.hosts(), &host) {
                    Err(crate::tr!("хоста {host} нет в правах net:", host = host))
                } else {
                    self.note("files", crate::tr!("скачала бы картинку {url}", url = url));
                    Ok(json!({ "name": input.get("name").cloned().unwrap_or(Value::Null), "size": 0, "type": "?" }))
                }
            }
            folder if folder.starts_with("folders.") => self.folder(folder, &input),
            other => Err(crate::tr!(
                "нет функции {other}: с Host API 2 всё вне приложения — письма шины, host.bus.call",
                other = other
            )),
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
    /// Languages (docs/plans/i18n.md): what is not translated yet — advice to the author, not a fault.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
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
/// Languages (docs/plans/i18n.md): an app speaks the household's languages from the start — Russian and English at
/// least, the household's own when it is another. What is said: a language missing from `languages`, a language with
/// no translation of the manifest's texts, a setting or schedule left untranslated, people's words in the code with
/// no dictionary.
pub fn language_warnings(manifest: &Manifest, source: Option<&str>, household: Option<&str>) -> Vec<String> {
    let mut warnings = Vec::new();
    if manifest.kind == PackageKind::Theme {
        return warnings;
    }
    let speaks = |code: &str| manifest.languages.iter().any(|language| language == code);
    for code in ["ru", "en"].into_iter().chain(household) {
        if !speaks(code) {
            warnings.push(crate::tr!("Языки: приложение не говорит на «{code}» — добавьте его в languages и переводы в locales.{code} (docs: authoring → «Языки»).", code = code));
        }
    }
    for code in manifest.languages.iter().skip(1) {
        let Some(texts) = manifest.locales.get(code) else {
            warnings.push(crate::tr!(
                "Языки: «{code}» в languages, а locales.{code} нет — название и настройки останутся непереведёнными.",
                code = code
            ));
            continue;
        };
        let mut missing: Vec<String> = Vec::new();
        if texts.title.is_none() {
            missing.push("title".into());
        }
        if manifest.description.is_some() && texts.description.is_none() {
            missing.push("description".into());
        }
        for setting in &manifest.settings {
            if texts.settings.get(&setting.key).is_none_or(|(title, _)| title.is_none()) {
                missing.push(format!("settings.{}", setting.key));
            }
        }
        for card in manifest.cards_in(&[]) {
            let id = card.get("id").and_then(Value::as_str).unwrap_or_default();
            if !texts.cards.contains_key(id) {
                missing.push(format!("cards.{id}"));
            }
        }
        for schedule in &manifest.contributes.schedules {
            if !texts.schedules.contains_key(&schedule.id) {
                missing.push(format!("schedules.{}", schedule.id));
            }
        }
        if !missing.is_empty() {
            warnings.push(crate::tr!(
                "Языки: в locales.{code} нет перевода: {missing}.",
                code = code,
                missing = missing.join(", ")
            ));
        }
    }
    // People's words straight in the code: a string literal with letters of another alphabet than the ASCII one, and
    // no `host.i18n` to pick it by the language.
    if let Some(source) = source {
        let worded = source.lines().any(|line| {
            let code = line.trim_start();
            !code.starts_with("//")
                && !code.starts_with('*')
                && code.chars().any(|c| c.is_alphabetic() && !c.is_ascii())
        });
        if worded && !source.contains("host.i18n") {
            warnings.push(
                crate::tr!("Языки: в backend.js текст для людей прямо в коде — вынесите его в словарь { ru: {…}, en: {…} } и берите host.i18n.t(T, key, params)."),
            );
        }
    }
    warnings
}

pub fn code_warnings(source: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    if source.contains("\"http://") || source.contains("'http://") || source.contains("`http://") {
        warnings.push(
            crate::tr!("Ссылка по http: поток из интернета по http колонка не играет и молчит — нужен https (проверено 07.11.2026)."),
        );
    }
    let lower = source.to_ascii_lowercase();
    if lower.contains(".aac") || lower.contains("aacp") {
        warnings.push(
            crate::tr!("AAC колонка не играет: шлюз перекодирует его в MP3 на компьютере, на колонке — только с флагом MEDIA_TRANSCODE_ON_STATION. Лучше MP3 или HLS."),
        );
    }
    if source.contains("setTimeout") || source.contains("setInterval") {
        warnings.push(crate::tr!("setTimeout и setInterval нет: код работает, только когда его позвали."));
    }
    if source.lines().any(|line| line.trim_start().starts_with("import ")) {
        warnings.push(crate::tr!("import не поддерживается: backend.js — один модуль (собирайте esbuild)."));
    }
    let mut unawaited: Vec<(String, Vec<usize>)> = Vec::new();
    for (name, line) in unawaited_calls(source) {
        match unawaited.iter_mut().find(|(known, _)| *known == name) {
            Some((_, lines)) => lines.push(line),
            None => unawaited.push((name, vec![line])),
        }
    }
    if !unawaited.is_empty() {
        let calls: Vec<String> = unawaited
            .iter()
            .map(|(name, lines)| {
                let lines: Vec<String> = lines.iter().map(usize::to_string).collect();
                crate::tr!("{name} (строка {lines})", name = name, lines = lines.join(", "))
            })
            .collect();
        warnings.push(crate::tr!("Вызов шлюза без await: {calls}. Он отвечает обещанием — без await функция ответит раньше, чем он кончится: картинка не скачается, файл не запишется, отказ пропадёт молча. Пишите await, а функцию — async.", calls = calls.join("; ")));
    }
    warnings
}

/// Host calls that answer with a promise (`host.http.fetch`, `host.bus.*`, `host.files.*`) left as a statement or a
/// value without `await`, with their lines: the function answers before they end. Read by what stands before each
/// call on its line; a call handed on (`return`, `=>`, an argument, an element of `Promise.all`'s array) is awaited
/// where it goes.
fn unawaited_calls(source: &str) -> Vec<(String, usize)> {
    let mut found = Vec::new();
    for (number, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("/*") || trimmed.ends_with(',')
        {
            continue;
        }
        for (at, _) in line.match_indices("host.") {
            if line[..at].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '.'))
            {
                continue;
            }
            let rest = &line[at + "host.".len()..];
            let name = if rest.starts_with("http.fetch") {
                "http.fetch".to_string()
            } else if let Some(part) = ["bus.", "files."].into_iter().find(|part| rest.starts_with(part)) {
                let method: String =
                    rest[part.len()..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                if method.is_empty() {
                    continue;
                }
                format!("{part}{method}")
            } else {
                continue;
            };
            if !rest[name.len()..].trim_start().starts_with('(') {
                continue;
            }
            let before = line[..at].trim_end();
            let assigned = before.ends_with('=') && !["==", "!=", "<=", ">="].iter().any(|op| before.ends_with(op));
            let statement = before.is_empty() || [";", "{", "}", ")", "else"].iter().any(|end| before.ends_with(end));
            if statement || assigned {
                found.push((format!("host.{name}"), number + 1));
            }
        }
    }
    found
}

/// What the workshop says of a draft with pictures while the gateway runs on the station (the owner, 12.11.2026:
/// «предупреждать об этом в мастерской, когда мы понимаем, что мы на колонке»).
pub fn station_warnings(folder: &Path) -> Vec<String> {
    let Ok((manifest, source)) = read_draft(folder) else { return Vec::new() };
    let pictures =
        source.as_ref().is_some_and(|source| source.contains("host.files.") || source.contains("files.download"));
    if !pictures || manifest.prefer_home {
        return Vec::new();
    }
    vec![crate::tr!("Шлюз на колонке, а места там мало: картинка — до 1 МБ, все картинки плагинов — до 5 МБ. Лучше \"prefer\": \"home\" — тогда картинки живут на компьютере.")]
}

/// What a link of an item says about the station.
fn link_warnings(play: &[PlayView]) -> Vec<String> {
    let mut warnings = Vec::new();
    for item in play {
        if item.url.starts_with("http://") && !item.url.starts_with(SANDBOX_MEDIA) {
            warnings
                .push(crate::tr!("«{item}»: ссылка по http — колонка её не сыграет, нужен https.", item = item.title));
        }
        let path = item.url.split(['?', '#']).next().unwrap_or_default().to_ascii_lowercase();
        if path.ends_with(".aac") || path.ends_with(".aacp") {
            warnings.push(crate::tr!(
                "«{item}»: AAC — колонка не играет, шлюз перекодирует только на компьютере.",
                item = item.title
            ));
        }
    }
    warnings
}

/// The draft's manifest and code, read.
pub(super) fn read_draft(folder: &Path) -> Result<(Manifest, Option<String>), Vec<String>> {
    let text =
        std::fs::read_to_string(folder.join("manifest.json")).map_err(|_| vec![crate::tr!("нет manifest.json")])?;
    let manifest = manifest::parse(&text).map_err(|error| error.problems)?;
    let code = folder.join("backend.js");
    let source = match std::fs::metadata(&code) {
        Ok(meta) if meta.len() > MAX_CODE => {
            return Err(vec![crate::tr!("backend.js больше {max_code} КБ", max_code = MAX_CODE / 1024)])
        }
        Ok(_) => Some(
            std::fs::read_to_string(&code)
                .map_err(|error| vec![crate::tr!("backend.js не читается: {error}", error = error)])?,
        ),
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
            code.source = source.ok_or_else(|| crate::tr!("нет backend.js"))?;
            Ok((Arc::new(QuickJs), code))
        }
        #[cfg(feature = "wasm")]
        Runtime::Wasm => {
            code.bytes = std::fs::read(folder.join("backend.wasm")).map_err(|_| crate::tr!("нет backend.wasm"))?;
            Ok((Arc::new(crate::plugins::wasm::Wasm), code))
        }
        #[cfg(not(feature = "wasm"))]
        Runtime::Wasm => {
            let _ = folder;
            Err(crate::tr!("в этой сборке шлюза нет WebAssembly"))
        }
        Runtime::Mcp => Ok((Arc::new(crate::plugins::mcp_runtime::McpRuntime), code)),
        Runtime::Process => Err(crate::tr!("приложение проверяется своим запуском, не в песочнице плагинов")),
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
    let bus = Examples::read(folder).ok().flatten().map(|examples| examples.bus).unwrap_or_default();
    let host = Arc::new(SandboxHost {
        manifest: Arc::new(manifest.clone()),
        kv,
        log: log.clone(),
        said: said.clone(),
        handle,
        settings: manifest.settings_values(Some(&trying.settings)),
        examples,
        bus,
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
        report.manifest.push(crate::tr!("id «{id}» в манифесте, а черновик — «{id}»", id = manifest.id));
    }
    if let Some(themes) = manifest.contributes.themes.as_ref().and_then(Value::as_array) {
        for theme in themes {
            if let Err(why) = theme_form(theme) {
                report.manifest.push(crate::tr!("тема: {why}", why = why));
            }
        }
    }
    if manifest.kind == PackageKind::Theme
        && manifest.contributes.themes.as_ref().and_then(Value::as_array).is_none_or(Vec::is_empty)
    {
        report.manifest.push(crate::tr!("тема без contributes.themes"));
    }
    let wanted = handlers(&manifest);
    let has_code = source.is_some() || manifest.runtime != Runtime::Js;
    // Host API 2: what the code reaches is a letter on the bus, and the manifest must ask for it.
    let letters = source.as_ref().map(|source| bus_warnings(&manifest, source)).unwrap_or_default();
    let languages = language_warnings(&manifest, source.as_deref(), None);
    let card_handle = handle.clone();
    match (has_code, wanted.is_empty()) {
        (false, false) => {
            report.code = Some(crate::tr!("нужен backend.js: его функции обслуживают то, что плагин даёт"))
        }
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
    report.warnings.extend(letters);
    report.languages = languages;
    if manifest.asks("device/midi") >= crate::bus::Level::Listen
        && manifest.runs.iter().any(|placement| *placement != crate::core::availability::Placement::Pc)
    {
        report.warnings.push(crate::tr!(
            "MIDI-клавиатура есть только у шлюза на компьютере: на колонке приложение будет работать без неё."
        ));
    }
    // Before the «да» the tile and «Примерка» show the example: a draft that reaches the network needs one.
    match Examples::read(folder) {
        Err(why) => report.warnings.push(crate::tr!("Пример не читается: {why}.", why = why)),
        Ok(None) if !manifest.hosts().is_empty() && manifest.kind != PackageKind::Theme => report.warnings.push(crate::tr!("Нужен пример: черновик ходит в сеть ({hosts}), а {EXAMPLES} нет. До «да» плитка и «Примерка» показывают пример — без него там будет пусто.", hosts = manifest.hosts().join(", "), EXAMPLES = EXAMPLES)),
        _ => {}
    }
    let cards = manifest
        .contributes
        .panel
        .as_ref()
        .and_then(|panel| panel.get("cards"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let calls = crate::bus::providers::apps::panel_calls(&manifest);
    if manifest.contributes.panel.is_some()
        && cards.is_empty()
        && !folder.join("panel.js").is_file()
        && !folder.join("panel.css").is_file()
    {
        report.warnings.push(
            crate::tr!("contributes.panel без карточек и без panel.js: в пульте приложению нечего показать (карточка — contributes.panel.cards)."),
        );
    }
    // Cards (docs/plugins/cards.md): each one's function among panel.calls, its answer a card of the panel's own parts,
    // its taps the app's own panel functions — run on the example, as the tile shows it.
    for (index, spec) in cards.iter().enumerate() {
        let name = spec.get("id").and_then(Value::as_str).unwrap_or_default();
        let label = if name.is_empty() {
            crate::tr!("карточка {index}", index = index + 1)
        } else {
            crate::tr!("карточка «{name}»", name = name)
        };
        if !crate::plugins::manifest::identifier(name) {
            report.manifest.push(crate::tr!("{label}: id — латиница, цифры, «_» и «-»", label = label));
        }
        if spec.get("size").and_then(Value::as_str).is_some_and(|size| !matches!(size, "small" | "large")) {
            report.warnings.push(crate::tr!("{label}: size — small или large", label = label));
        }
        let Some(function) = spec.get("function").and_then(Value::as_str) else {
            report
                .manifest
                .push(crate::tr!("{label}: нет function — функции backend.js, что отдаёт её данные", label = label));
            continue;
        };
        if !calls.iter().any(|call| call == function) {
            report.manifest.push(crate::tr!(
                "{label}: функции {function} нет в contributes.panel.calls — пульт её не позовёт",
                label = label,
                function = function
            ));
            continue;
        }
        if report.code.is_some() || !report.missing.is_empty() {
            continue;
        }
        let kv = Arc::new(Mutex::new(Map::new()));
        match run(folder, id, function, json!({}), kv, card_handle.clone(), &Trying::example()).await {
            Ok(ran) if ran.ok => {
                let card = crate::bus::cards::unwrap(ran.result);
                for problem in crate::bus::cards::check(&card) {
                    report.warnings.push(format!("{label}: {problem}"));
                }
                if card.is_object() && card.get("look").is_none() {
                    report.warnings.push(crate::tr!("{label}: без вида карточка — простая серая. Дайте ей look: сцену и цвета по смыслу (погода — небо за окном, музыка — волны, покупки — пузырьки), docs cards, «Вид».", label = label));
                }
                for tapped in crate::bus::cards::functions(&card) {
                    if !calls.contains(&tapped) {
                        report.warnings.push(crate::tr!(
                            "{label}: нажатие зовёт {tapped}, а её нет в contributes.panel.calls — пульт её не позовёт",
                            label = label,
                            tapped = tapped
                        ));
                    }
                }
            }
            Ok(ran) => report.warnings.push(crate::tr!(
                "{label}: {function} не отработала на примере: {error}",
                label = label,
                function = function,
                error = ran.error.unwrap_or_else(|| crate::tr!("без ответа"))
            )),
            Err(error) => report.warnings.push(format!("{label}: {error}")),
        }
    }
    // The panel part runs in the browser, in a window of its own: here only whether it parses, and whether it needs
    // to be a window at all.
    if let Ok(panel) = std::fs::read_to_string(folder.join("panel.js")) {
        if let Some(advice) = could_be_card(&panel) {
            report.warnings.push(advice);
        }
        // Web MIDI in the browser is the whole panel's once Chrome allowed it: an app takes the keyboard only with
        // its right (docs/plans/midi.md, «Запасной путь»).
        if manifest.asks("device/midi") < crate::bus::Level::Listen
            && ["useMidi", "requestMIDIAccess"].iter().any(|word| panel.contains(word))
        {
            report.warnings.push(
                crate::tr!("panel.js берёт MIDI-клавиатуру (useMidi или requestMIDIAccess), а связи нет: добавьте \"listen:device/midi\" в permissions."),
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

/// What Host API 2 says of the code (docs/plans/bus.md, «Один способ»): the older `host.*` names are gone, every
/// reach beyond the app is a letter, and a letter goes only where the manifest asked to reach.
fn bus_warnings(manifest: &Manifest, source: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    let older: Vec<&str> = [
        "host.call(",
        "host.content.",
        "host.tools.",
        "host.smartHome.",
        "host.home.",
        "host.notify(",
        "host.schedule.",
        "host.voice.",
    ]
    .into_iter()
    .filter(|word| source.contains(word))
    .collect();
    if !older.is_empty() {
        warnings.push(crate::tr!("Код зовёт Host API 1 ({older}): в Host API 2 этого нет — всё вне приложения письма шины, например host.bus.call(\"system/smart-home\", \"act\", { device, action }), host.bus.call(\"system/voice\", \"say\", { text }).", older = older.join(", ")));
    }
    let mut named = Vec::new();
    for call in ["host.bus.call(", "host.bus.ask("] {
        for (start, _) in source.match_indices(call) {
            let rest = source[start + call.len()..].trim_start();
            let Some(quote) = rest.chars().next().filter(|c| matches!(c, '"' | '\'' | '`')) else { continue };
            let Some(end) = rest[1..].find(quote) else { continue };
            let address = &rest[1..1 + end];
            if !address.contains('$') && !named.iter().any(|known: &String| known == address) {
                named.push(address.to_string());
            }
        }
    }
    for address in named {
        if crate::bus::Address::parse(&address).is_ok() && manifest.asks(&address) == crate::bus::Level::None {
            warnings.push(crate::tr!("Код пишет «{address}», а манифест не просит связь с ним: добавьте \"call:{address}\" в permissions — иначе для приложения его нет.", address = address));
        }
    }
    warnings
}

/// What only a window can do (docs/plugins/cards.md, «Карточка или окошко»): its own drawing, input finer than a
/// field and buttons, motion faster than once a second, sound, the keyboard. A `panel.js` with none of it shows rows
/// and buttons — a card does that lighter, safer and everywhere (a phone, a chat, the voice).
fn could_be_card(panel: &str) -> Option<String> {
    const WINDOW_ONLY: &[&str] = &[
        "canvas",
        "<svg",
        "\"svg\"",
        "'svg'",
        "useMidi",
        "PianoKeys",
        "useSynth",
        "AudioContext",
        "requestAnimationFrame",
        "onPointer",
        "pointerdown",
        "pointermove",
        "onDrag",
        "draggable",
        "setInterval",
        "onKeyDown",
        "keydown",
        "sections:",
    ];
    // Advice for a widget of the desk only: a section or a part of another slot is a window by its nature.
    if !panel.contains("desk.widgets") || WINDOW_ONLY.iter().any(|word| panel.contains(word)) {
        return None;
    }
    Some(
        crate::tr!("panel.js только показывает строки и кнопки — это может быть карточкой (contributes.panel.cards): она легче, безопаснее и видна везде — на столе, в телефоне, в чате, голосом. Окошко нужно для своего рисования, игры, клавиатуры, звука, движения."),
    )
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
                            report.warnings.push(crate::tr!(
                                "play есть, но ни один элемент не годится: нужны id, title и url по http(s) или path"
                            ));
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
            "schema": 1, "id": "jazz", "version": "1.0.0", "title": "Джаз", "api": "^2", "runs": ["pc", "station", "stationServer"],
            "permissions": [],
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

        // Host API 1's names are gone: the check says what a letter is now.
        let older = "export function play() { return host.content.search(\"music\", \"\", 5); }";
        draft(&folder, manifest(), older);
        let checked = check(&folder, "jazz", None).await;
        assert!(checked.warnings.iter().any(|warning| warning.contains("Host API 1")), "{checked:?}");
        // A letter to what the manifest did not ask for: the check names it; asked for, nothing to say.
        let letter = "export function play() { return host.bus.call(\"system/content\", \"music\", { query: \"\" }); }";
        draft(&folder, manifest(), letter);
        let checked = check(&folder, "jazz", None).await;
        assert!(checked.warnings.iter().any(|warning| warning.contains("call:system/content")), "{checked:?}");
        let mut asked = manifest();
        asked["permissions"] = json!(["call:system/content"]);
        draft(&folder, asked, letter);
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
                "schema": 1, "id": "todo", "version": "0.1.0", "title": "Дела на сегодня", "api": "^2", "runs": ["pc"],
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
        // The same quiz for Host API 1: refused, with what to do.
        let mut manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(folder.join("manifest.json")).unwrap()).unwrap();
        manifest["api"] = json!("^1.2");
        std::fs::write(folder.join("manifest.json"), manifest.to_string()).unwrap();
        let older = check(&folder, "quiz", None).await;
        assert!(!older.ok && older.manifest.iter().any(|problem| problem.contains("Host API 1")), "{older:?}");
    }

    #[tokio::test]
    async fn the_album_template_passes_and_shows_the_example_folders_photo_of_the_day() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("album");
        std::fs::create_dir_all(&folder).unwrap();
        for (path, text) in crate::plugins::workshop::templates::folders("album", "Фотоальбом") {
            std::fs::write(folder.join(path), text).unwrap();
        }
        let checked = check(&folder, "album", None).await;
        assert!(checked.ok && checked.warnings.is_empty(), "{checked:?}");
        let kv = Arc::new(Mutex::new(Map::new()));
        let card = run(&folder, "album", "today", json!({}), kv.clone(), None, &Trying::example()).await.unwrap();
        let address = card.result["picture"]["address"].as_str().unwrap().to_string();
        assert!(address.starts_with("files/home/Фото/"), "{}", card.result);
        assert_eq!(card.result["cap"], "Фото дня · 3 в папке");
        assert!(crate::bus::cards::check(&card.result).is_empty(), "{:?}", crate::bus::cards::check(&card.result));
        let other = run(&folder, "album", "next", json!({}), kv.clone(), None, &Trying::example()).await.unwrap();
        assert_ne!(other.result["picture"]["address"], json!(address), "«Другое» shows another");
        let page = run(&folder, "album", "page", json!({ "from": 0 }), kv, None, &Trying::example()).await.unwrap();
        assert_eq!(page.result["total"], 3);
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
        // Without the link to the keyboard the check says so.
        let mut manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(folder.join("manifest.json")).unwrap()).unwrap();
        manifest["permissions"] = json!([]);
        std::fs::write(folder.join("manifest.json"), manifest.to_string()).unwrap();
        let warned = check(&folder, "uchitel", None).await;
        assert!(warned.warnings.iter().any(|warning| warning.contains("listen:device/midi")), "{warned:?}");
    }

    #[test]
    fn warns_of_a_host_call_left_without_await_and_lets_one_handed_on_be() {
        let source = r#"
export function card() {
  try {
    host.files.download(`https://picsum.photos/800/600`, "live");
  } catch (e) {}
  const text = host.files.read("tmp/a.txt");
  if (!text) host.bus.call("system/notify", "send", { text: "x" });
  return { picture: { name: "live.jpg" } };
}
export async function fine() {
  await host.files.download("https://picsum.photos/1", "a");
  const [a, b] = await Promise.all([host.files.read("a"), host.files.read("b")]);
  await Promise.all([
    host.files.download("https://picsum.photos/2", "b"),
  ]);
  const answer = await host.http.fetch("https://example.com");
  host.kv.set("k", 1);
  // host.files.download(old) in a comment
  return host.bus.call("app/x", "y", {});
}
"#;
        assert_eq!(
            unawaited_calls(source),
            vec![("host.files.download".into(), 4), ("host.files.read".into(), 6), ("host.bus.call".into(), 7)]
        );
        let warning = code_warnings(source).into_iter().find(|warning| warning.contains("без await")).unwrap();
        assert!(warning.contains("host.files.download (строка 4)"), "{warning}");
        assert!(code_warnings("export async function a() { await host.files.download(u, \"a\"); }").is_empty());
    }
}
