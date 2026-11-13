//! A plugin's manifest (`manifest.json` beside its code): who it is, which Host API and placements it needs, its
//! budget, its rights and what it gives the gateway. It is checked whole before any of the plugin's code runs, and
//! every problem is told at once, in words the panel shows (docs/plugins/AUTHORING.md).

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::{json, Map, Value};

use super::phrases::Phrase;
use crate::core::availability::Placement;
use crate::mcp::mcp_tools::normalize_schema;
use crate::skills::capabilities::{is_capability, CapabilityName};
use crate::skills::media::media_types::MediaKind;
use crate::skills::router::STATION_CONTROLS;
use crate::skills::tools::tool_spec;

/// The manifest format this gateway reads.
pub const MANIFEST_SCHEMA: u64 = 1;
/// The Host API this gateway gives: `api` in a manifest is a requirement on it (`^1`).
/// 1.1 (stage 10): `host.call` — the household's content, other plugins' tools, the smart home — and answers said by
/// the model; additive, a plugin of `^1` still runs.
/// 1.2 (docs/plans/plugin-schedules.md): the turn timer — `continue.timeout`, «время вышло» said by the station
/// itself; additive, a plugin of `^1.1` still runs.
/// 1.3 (docs/plans/midi.md): the right `midi` — a MIDI keyboard at the computer, heard and played by the app's page;
/// additive, a plugin of `^1.2` still runs.
pub const HOST_API: semver::Version = semver::Version::new(2, 0, 0);

/// What a plugin may ask for. The budget is a limit the runtime keeps, not a hint.
pub const MAX_MEMORY_MB: u64 = 64;
pub const MAX_STORAGE_MB: u64 = 64;
const DEFAULT_MEMORY_MB: u64 = 4;
const DEFAULT_STORAGE_MB: u64 = 1;
const MAX_PHRASES: usize = 50;
const MAX_TOOLS: usize = 20;
const MAX_SETTINGS: usize = 30;
/// A licence line, as the catalog's card shows it.
pub const MAX_LICENSE: usize = 200;

/// Events a plugin may follow: the coarse ones, without anyone's words; and what the station said of the plugin's
/// own item our player plays (stage 5а) — `player.silent` and `player.failed` may be answered with what to play
/// instead.
pub const PLUGIN_EVENTS: &[&str] = &[
    "station.connected",
    "station.disconnected",
    "mode.changed",
    "homeServer.up",
    "homeServer.down",
    "player.started",
    "player.stopped",
    "player.failed",
    "player.silent",
];

/// What a package is. One format for every kind — the catalog will carry them all, by `kind`: a plugin runs its code
/// in the gateway; an app is a program of its own beside it (stage 9 of the plan); a character and a theme carry data
/// and need no code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageKind {
    Plugin,
    App,
    Character,
    Theme,
}

impl PackageKind {
    pub const ALL: [PackageKind; 4] =
        [PackageKind::Plugin, PackageKind::App, PackageKind::Character, PackageKind::Theme];

    pub fn parse(text: &str) -> Option<PackageKind> {
        PackageKind::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PackageKind::Plugin => "plugin",
            PackageKind::App => "app",
            PackageKind::Character => "character",
            PackageKind::Theme => "theme",
        }
    }

    /// Why the plugin host does not switch a package of this kind on; none for a plugin and an app (its program runs
    /// under the apps' supervisor, its functions are called over HTTP — stage 9).
    pub fn not_a_plugin(self) -> Option<&'static str> {
        match self {
            PackageKind::Plugin | PackageKind::App => None,
            PackageKind::Character => {
                Some("персонаж — не приложение: его выбирают среди персонажей, а не включают здесь")
            }
            PackageKind::Theme => Some("тема — не приложение: её выбирают в пульте, а не включают здесь"),
        }
    }
}

/// The engine a package's code runs in (`runtime`): JavaScript in QuickJS, WebAssembly in wasmi, an MCP server over
/// the network, or — an app's — a program of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Js,
    Wasm,
    Mcp,
    Process,
}

impl Runtime {
    pub fn as_str(self) -> &'static str {
        match self {
            Runtime::Js => "js",
            Runtime::Wasm => "wasm",
            Runtime::Mcp => "mcp",
            Runtime::Process => "process",
        }
    }

    /// The file of its code beside the manifest, if it has one.
    pub fn code_file(self) -> Option<&'static str> {
        match self {
            Runtime::Js => Some("backend.js"),
            Runtime::Wasm => Some("backend.wasm"),
            Runtime::Mcp | Runtime::Process => None,
        }
    }
}

/// A right of a plugin, as the manifest names it (Host API 2, docs/plans/bus.md, «Один способ»): only two kinds —
/// where on the internet it goes, and what in the house it reaches on the bus. What it gives (phrases, tools,
/// sources, events) is said in `contributes`, its own store and folder are its own: none of these is a right.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Permission {
    /// `net:<host>`: `http.fetch` to this host; `net:*.example.com` to its subdomains.
    Net(String),
    /// `see:<адрес>`, `listen:…`, `call:…`, `edit:…`: a step of the ladder towards a pattern of addresses on the bus
    /// (src/bus): `call:system/voice`, `call:app/notes`, `listen:device/midi`, `edit:files/home/Документы/*`. Asked
    /// of a person the first time it is needed, like a phone; a built-in app has its own from the start.
    Bus { level: crate::bus::Level, pattern: crate::bus::Pattern },
}

/// What a right of Host API 1 became (docs/plans/bus.md, «Один способ»): for the message that refuses the old word.
pub fn right_now(old: &str) -> Option<&'static str> {
    Some(match old {
        "voice.intents" | "llm.tools" | "media.source" | "events" => "не нужно — хватает contributes",
        "kv" | "files" => "не нужно — своё хранилище и своя папка без права",
        "voice.say" => "call:system/voice",
        "llm.chat" => "call:system/brain",
        "tools.call" => "call:app/<id> или call:app/*",
        "smart_home" => "call:system/smart-home",
        "content:music" | "content:tales" => "call:system/content",
        "notify" => "call:system/notify",
        "schedule" => "call:system/schedule",
        "midi" => "listen:device/midi",
        "lan.mdns" => "не нужно — своя программа",
        _ => return None,
    })
}

impl Permission {
    pub fn parse(text: &str) -> Option<Permission> {
        for (prefix, level) in [
            ("see:", crate::bus::Level::See),
            ("listen:", crate::bus::Level::Listen),
            ("call:", crate::bus::Level::Call),
            ("edit:", crate::bus::Level::Edit),
        ] {
            if let Some(pattern) = text.strip_prefix(prefix) {
                return crate::bus::Pattern::parse(pattern).ok().map(|pattern| Permission::Bus { level, pattern });
            }
        }
        let host = text.strip_prefix("net:")?;
        host_pattern(host).then(|| Permission::Net(host.to_string()))
    }

    pub fn name(&self) -> String {
        match self {
            Permission::Net(host) => format!("net:{host}"),
            Permission::Bus { level, pattern } => format!("{}:{pattern}", level.as_str()),
        }
    }
}

/// A host name in lower case, or `*.` and one: no scheme, no port, no path.
fn host_pattern(host: &str) -> bool {
    let name = host.strip_prefix("*.").unwrap_or(host);
    let labels: Vec<&str> = name.split('.').collect();
    !name.is_empty()
        && name.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

/// Whether a host is one of the permitted patterns.
pub fn host_allowed(patterns: &[String], host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    patterns.iter().any(|pattern| match pattern.strip_prefix("*.") {
        Some(domain) => host.len() > domain.len() + 1 && host.ends_with(&format!(".{domain}")),
        None => *pattern == host,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Budget {
    /// The runtime's memory limit: the plugin's calls fail above it, the gateway stays.
    pub memory_mb: u64,
    /// Its `kv` store at most this big.
    pub storage_mb: u64,
    /// Its own folder on the bus (`files/app/<id>`) at most this big on a computer; the station keeps all files within
    /// its 5 MB (src/bus/providers/files.rs).
    pub files_mb: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingType {
    String,
    Number,
    Boolean,
}

/// What the panel sees of a secret setting that is set: never the value itself. A save that brings it back keeps the
/// value stored.
pub const SECRET_SHOWN: &str = "••••••";

/// A setting the panel draws a field for; the plugin reads its value through `settings`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingSpec {
    pub key: String,
    pub r#type: SettingType,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// For a string: the values to pick from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// `"secret": true` (a string): a private link, a token. The plugin's backend reads it as it is; the panel — the
    /// package's view and the plugin's own part — sees only `SECRET_SHOWN`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secret: bool,
}

impl SettingSpec {
    /// Whether a value fits the setting.
    pub fn accepts(&self, value: &Value) -> bool {
        match (self.r#type, value) {
            (SettingType::String, Value::String(text)) => {
                text.chars().count() <= 1000 && self.options.as_ref().is_none_or(|options| options.contains(text))
            }
            (SettingType::Number, Value::Number(_)) | (SettingType::Boolean, Value::Bool(_)) => true,
            _ => false,
        }
    }
}

/// Phrases the station hears that go to one of the plugin's functions.
#[derive(Debug, Clone)]
pub struct IntentSpec {
    pub phrases: Vec<Phrase>,
    pub handler: String,
    /// The capability it serves: the speaker's right to it (`use:radio`) and its route apply. Without one, the
    /// plugins' own (`plugins`).
    pub capability: CapabilityName,
}

/// A tool for «Мозг» in MCP's form: a name, a description, a JSON Schema of its input.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input: Value,
    pub handler: String,
    pub capability: CapabilityName,
    /// MCP's `readOnlyHint`: it changes nothing, so it stays offered after a text from outside the house.
    pub read_only: bool,
    /// MCP's `destructiveHint` (it removes) or `openWorldHint` (it reaches outside): a dangerous handle on the bus.
    pub danger: Option<crate::bus::danger::Danger>,
}

/// A source of media for our player: its function takes `{query, limit}` and gives items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSpec {
    pub id: String,
    pub kind: MediaKind,
    pub handler: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSpec {
    pub name: String,
    pub handler: String,
}

/// A rhythm of `contributes.schedules` (docs/plans/plugin-schedules.md): when, and what — the plugin's function or a
/// ready answer the gateway says itself (the owner's decision 8, 12.11.2026: it never loads the plugin, so it costs
/// the station no memory and a phone can make it a system notification).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleSpec {
    pub id: String,
    pub title: String,
    pub when: super::schedule::When,
    pub then: super::schedule::Then,
}

#[derive(Debug, Clone, Default)]
pub struct Contributes {
    pub intents: Vec<IntentSpec>,
    pub tools: Vec<ToolSpec>,
    pub media: Vec<MediaSpec>,
    pub events: Vec<EventSpec>,
    /// Rhythms: it acts by itself in time (plugins/schedule.rs).
    pub schedules: Vec<ScheduleSpec>,
    /// Its part on the bus (src/bus, Host API 1.4): personal and dangerous handles, the news it tells, the news it
    /// listens to, the function that answers in words.
    pub bus: BusSpec,
    /// The panel's part, themes and characters: kept as they are for the panel (stage 5) and the characters
    /// (docs/plans/characters.md); the plugin host does not read them.
    pub panel: Option<Value>,
    pub themes: Option<Value>,
    pub characters: Option<Value>,
}

/// `contributes.bus` (src/bus, docs/plans/bus.md).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BusSpec {
    /// Handles open only to a link made for them (its tools and panel functions by name).
    pub private: Vec<String>,
    /// Handles it marks dangerous itself: a mark may only be added, never taken away.
    pub danger: std::collections::BTreeMap<String, crate::bus::danger::Danger>,
    /// The news it tells (`host.bus.tell`).
    pub emits: Vec<String>,
    /// The news it listens to: whose (a pattern), which, and its function that takes it.
    pub on: Vec<OnSpec>,
    /// Its function that answers a question in words (`host.bus.ask`).
    pub ask: Option<String>,
}

impl BusSpec {
    /// Nothing on the bus: no personal or dangerous handle, no news told or heard, no answer in words.
    pub fn is_empty(&self) -> bool {
        self.private.is_empty()
            && self.danger.is_empty()
            && self.emits.is_empty()
            && self.on.is_empty()
            && self.ask.is_none()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OnSpec {
    pub from: crate::bus::Pattern,
    pub event: String,
    pub handler: String,
}

/// A name of news: lower case letters, digits, `.`, `-`, `_`, a letter first.
pub fn news_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
}

#[derive(Debug, Clone)]
pub struct Manifest {
    /// `plugin` when the manifest does not say.
    pub kind: PackageKind,
    pub id: String,
    pub version: semver::Version,
    pub title: String,
    pub description: Option<String>,
    /// Its licence for people, one line («CC BY-NC 4.0, некоммерчески»): every kind may carry one.
    pub license: Option<String>,
    /// The Host API it needs, as written (`^1`).
    pub api: String,
    /// Its engine: `js` when the manifest does not say.
    pub runtime: Runtime,
    /// `runtime: mcp`: the MCP server's address (`mcp.url`), on a host of its `net:` rights.
    pub mcp_url: Option<String>,
    /// An app's programs by platform (`targets`), and how it is asked whether it is well (`health`).
    pub targets: std::collections::BTreeMap<String, Target>,
    pub health: HealthCheck,
    pub runs: Vec<Placement>,
    /// `"prefer": "home"`: on the station with a home server, its calls go to the computer while it is up (stage 7);
    /// the station runs it only when the computer is away and it fits there.
    pub prefer_home: bool,
    pub budget: Budget,
    pub permissions: Vec<Permission>,
    pub settings: Vec<SettingSpec>,
    pub contributes: Contributes,
    /// `dataVersion`: the kind of the data it keeps, raised by its author when that changes (docs/plans/
    /// package-history.md): a step back of the code alone across a change of it is marked risky.
    pub data_version: Option<u32>,
}

impl Manifest {
    pub fn has(&self, permission: &Permission) -> bool {
        self.permissions.contains(permission)
    }

    /// How far it asked to reach `address` on the bus: the highest step of its rights whose pattern matches.
    pub fn asks(&self, address: &str) -> crate::bus::Level {
        let Ok(address) = crate::bus::Address::parse(address) else { return crate::bus::Level::None };
        self.permissions
            .iter()
            .filter_map(|permission| match permission {
                Permission::Bus { level, pattern } if pattern.matches(&address) => Some(*level),
                _ => None,
            })
            .max()
            .unwrap_or(crate::bus::Level::None)
    }

    /// The hosts it may reach.
    pub fn hosts(&self) -> Vec<String> {
        self.permissions
            .iter()
            .filter_map(|permission| match permission {
                Permission::Net(host) => Some(host.clone()),
                _ => None,
            })
            .collect()
    }

    /// Why it cannot run here, if it cannot: the placement is not among its `runs`.
    pub fn incompatible_with(&self, placement: Placement) -> Option<String> {
        (!self.runs.contains(&placement)).then(|| {
            let runs: Vec<&str> = self.runs.iter().map(|runs| runs.as_str()).collect();
            format!(
                "не работает здесь: размещение «{}» ({}), а приложение — для {}",
                placement.title(),
                placement.as_str(),
                runs.join(", ")
            )
        })
    }

    /// The values of its settings: the stored ones that still fit, the defaults for the rest. The plugin's own: its
    /// backend reads them so; the panel — `settings_shown`.
    pub fn settings_values(&self, stored: Option<&Map<String, Value>>) -> Map<String, Value> {
        let mut values = Map::new();
        for setting in &self.settings {
            let value = stored.and_then(|stored| stored.get(&setting.key)).filter(|value| setting.accepts(value));
            if let Some(value) = value.or(setting.default.as_ref()) {
                values.insert(setting.key.clone(), value.clone());
            }
        }
        values
    }

    /// The values as the panel sees them: a secret one that is set only as «••••••» (an empty one stays empty, so the
    /// panel can tell it is not set).
    pub fn settings_shown(&self, stored: Option<&Map<String, Value>>) -> Map<String, Value> {
        let mut values = self.settings_values(stored);
        for setting in self.settings.iter().filter(|setting| setting.secret) {
            if let Some(value) = values.get_mut(&setting.key).filter(|value| value.as_str() != Some("")) {
                *value = Value::String(SECRET_SHOWN.into());
            }
        }
        values
    }

    /// The stored values with the private ones (`secret`) from the gateway's safe, `plugin:<package>:<key>`: what
    /// `settings_values` and `settings_shown` read. A private value still left among the settings (one stored before
    /// the safe kept them, a package of settings from an older gateway) is read until the host moves it into the safe.
    pub fn stored_with_secrets(
        &self,
        stored: Option<&Map<String, Value>>,
        secrets: &crate::secrets::SecretsService,
    ) -> Map<String, Value> {
        let mut all = stored.cloned().unwrap_or_default();
        for setting in self.settings.iter().filter(|setting| setting.secret) {
            if let Some(value) = secrets.get(&crate::secrets::plugin_secret(&self.id, &setting.key)) {
                all.insert(setting.key.clone(), Value::String(value));
            }
        }
        all
    }
}

/// An app's program for a platform: its file in the package and its SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Target {
    pub file: String,
    pub sha256: String,
}

/// How the supervisor asks an app whether it is well.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCheck {
    pub path: String,
    pub interval_s: u64,
}

/// This machine's platform as an app's `targets` names it: `darwin-arm64`, `linux-aarch64`, `linux-x86_64`.
pub fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match (os, std::env::consts::ARCH) {
        ("darwin", "aarch64") => "arm64",
        (_, other) => other,
    };
    format!("{os}-{arch}")
}

/// Every problem of a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError {
    pub problems: Vec<String>,
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.problems.join("; "))
    }
}

pub(crate) fn identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_' || first == '$')
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// A plugin's id: lower case letters, digits and dashes, a letter first; the folder bears it.
pub fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && id.len() <= 32
        && !id.ends_with('-')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A tool's name as the model sees it: a letter first, then letters, digits and `_`.
fn tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && name.len() <= 48
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Reads and checks a manifest's JSON.
pub fn parse(text: &str) -> Result<Manifest, ManifestError> {
    let value: Value = serde_json::from_str(text).map_err(|error| ManifestError {
        problems: vec![format!("manifest.json не читается как JSON: {error}")],
    })?;
    check(&value)
}

/// Checks a manifest, collecting every problem.
pub fn check(value: &Value) -> Result<Manifest, ManifestError> {
    let mut problems = Vec::new();
    let Some(root) = value.as_object() else {
        return Err(ManifestError { problems: vec!["manifest.json должен быть объектом".into()] });
    };
    let text = |key: &str| root.get(key).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty());

    match root.get("schema").and_then(Value::as_u64) {
        Some(MANIFEST_SCHEMA) => {}
        Some(other) => problems.push(format!("schema {other}: этот шлюз понимает манифесты версии {MANIFEST_SCHEMA}")),
        None => problems.push(format!("нет schema (нужно {MANIFEST_SCHEMA})")),
    }
    let kind = match text("kind") {
        None => PackageKind::Plugin,
        Some(name) => PackageKind::parse(name).unwrap_or_else(|| {
            let kinds = PackageKind::ALL.map(PackageKind::as_str).join(", ");
            problems.push(format!("kind «{name}»: такого вида пакетов нет; есть {kinds}"));
            PackageKind::Plugin
        }),
    };
    let id = text("id").unwrap_or_default().to_string();
    if !valid_id(&id) {
        problems.push("id: строчные латинские буквы, цифры и «-», с буквы, до 32 знаков".into());
    }
    let version = match text("version").map(semver::Version::parse) {
        Some(Ok(version)) => version,
        Some(Err(_)) | None => {
            problems.push("version: номер версии вида 1.2.0".into());
            semver::Version::new(0, 0, 0)
        }
    };
    let title: String = text("title").map(|title| title.chars().take(80).collect()).unwrap_or_default();
    if title.is_empty() {
        problems.push("нет title".into());
    }
    let description: Option<String> = text("description").map(|description| description.chars().take(500).collect());
    let license = text("license").map(str::to_string);
    if license.as_ref().is_some_and(|license| license.chars().count() > MAX_LICENSE) {
        problems.push(format!("license: одна строка до {MAX_LICENSE} знаков"));
    }

    let api = text("api").unwrap_or_default().to_string();
    // The Host API is code's: a character or a theme is data, any version of it reads.
    let coded = matches!(kind, PackageKind::Plugin | PackageKind::App);
    match semver::VersionReq::parse(&api) {
        Ok(requirement) if requirement.matches(&HOST_API) || !coded => {}
        Ok(_) if api.starts_with("^1") || api.starts_with("1") || api.starts_with(">=1") => problems.push(format!(
            "приложение для Host API 1 ({api}), а у шлюза {HOST_API}: переделайте его в мастерской («api»: «^2», права — связи шины) или удалите"
        )),
        Ok(_) => problems.push(format!("нужен Host API {api}, а у шлюза {}", HOST_API)),
        Err(_) => problems.push("api: требование к версии Host API, например «^2»".into()),
    }
    let runtime = match text("runtime") {
        None | Some("js") => Runtime::Js,
        Some("wasm") if kind == PackageKind::Plugin => Runtime::Wasm,
        Some("mcp") if kind == PackageKind::Plugin => Runtime::Mcp,
        // An app's program is its own (stage 9): a process beside the gateway.
        Some("process") if kind == PackageKind::App => Runtime::Process,
        Some(other) => {
            problems.push(format!("runtime «{other}» не подходит: у плагина js, wasm или mcp, у приложения — process"));
            Runtime::Js
        }
    };
    let mut targets = std::collections::BTreeMap::new();
    if let Some(given) = root.get("targets").and_then(Value::as_object) {
        for (key, target) in given {
            let file = target.get("file").and_then(Value::as_str).unwrap_or_default().trim().to_string();
            let sha256 = target.get("sha256").and_then(Value::as_str).unwrap_or_default().trim().to_ascii_lowercase();
            let plain = !file.is_empty()
                && file.split('/').all(|part| !part.is_empty() && !part.starts_with('.') && part.len() <= 80);
            if !plain {
                problems.push(format!("targets.{key}.file: путь внутри пакета, например bin/app-{key}"));
            } else if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
                problems.push(format!("targets.{key}.sha256: 64 шестнадцатеричных знака"));
            } else {
                targets.insert(key.clone(), Target { file, sha256 });
            }
        }
    }
    if kind == PackageKind::App && runtime == Runtime::Process && targets.is_empty() {
        problems.push("targets: сборки приложения под платформы (linux-aarch64, darwin-arm64, linux-x86_64)".into());
    }
    if kind == PackageKind::App && runtime != Runtime::Process {
        problems.push("приложение — это runtime «process»: своя программа рядом со шлюзом".into());
    }
    let health = HealthCheck {
        path: value
            .pointer("/health/path")
            .and_then(Value::as_str)
            .filter(|path| path.starts_with('/'))
            .unwrap_or("/health")
            .to_string(),
        interval_s: value.pointer("/health/intervalS").and_then(Value::as_u64).unwrap_or(15).clamp(5, 300),
    };
    let mcp_url = value.pointer("/mcp/url").and_then(Value::as_str).map(str::trim).map(str::to_string);
    if runtime == Runtime::Mcp {
        match mcp_url.as_deref().map(url::Url::parse) {
            Some(Ok(url)) if url.scheme() == "https" || url.scheme() == "http" => {}
            _ => problems.push("mcp.url: адрес MCP-сервера (https://…)".into()),
        }
    }

    let mut runs = Vec::new();
    match root.get("runs").and_then(Value::as_array) {
        Some(names) if !names.is_empty() => {
            for name in names {
                match name.as_str().and_then(Placement::parse) {
                    Some(placement) if !runs.contains(&placement) => runs.push(placement),
                    Some(_) => {}
                    None => problems.push(format!(
                        "runs: «{}» — нет такого размещения; есть {}",
                        crate::skills::js::string_of(name),
                        Placement::ALL.map(|placement| placement.as_str()).join(", ")
                    )),
                }
            }
        }
        _ => problems.push("runs: список размещений, где плагин работает (pc, station, stationServer)".into()),
    }

    let data_version = match root.get("dataVersion") {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_u64().filter(|n| *n <= u32::MAX as u64) {
            Some(n) => Some(n as u32),
            None => {
                problems.push("dataVersion: целое число — вид данных, который хранит плагин".into());
                None
            }
        },
    };
    let prefer_home = match text("prefer") {
        None | Some("here") => false,
        Some("home") => true,
        Some(other) => {
            problems.push(format!("prefer «{other}»: home (тяжёлое — на компьютере) или here"));
            false
        }
    };
    let budget = root.get("budget").and_then(Value::as_object);
    let limit = |key: &str, default: u64, max: u64, problems: &mut Vec<String>| match budget.and_then(|b| b.get(key)) {
        None => default,
        Some(value) => match value.as_u64().filter(|n| (1..=max).contains(n)) {
            Some(n) => n,
            None => {
                problems.push(format!("budget.{key}: целое от 1 до {max}"));
                default
            }
        },
    };
    let budget = Budget {
        memory_mb: limit("memoryMb", DEFAULT_MEMORY_MB, MAX_MEMORY_MB, &mut problems),
        storage_mb: limit("storageMb", DEFAULT_STORAGE_MB, MAX_STORAGE_MB, &mut problems),
        files_mb: limit("filesMb", crate::bus::providers::files::APP_FOLDER_MB, 4096, &mut problems),
    };

    let mut permissions = BTreeSet::new();
    match root.get("permissions") {
        None => {}
        Some(Value::Array(names)) => {
            for name in names {
                match name.as_str().and_then(Permission::parse) {
                    Some(permission) => {
                        permissions.insert(permission);
                    }
                    None => {
                        let text = crate::skills::js::string_of(name);
                        problems.push(match right_now(&text) {
                            Some(now) => format!("permissions: «{text}» — право Host API 1; теперь: {now}"),
                            None => format!(
                                "permissions: «{text}» — нет такого права: есть net:<хост> и связи see:/listen:/call:/edit:<адрес>"
                            ),
                        })
                    }
                }
            }
        }
        Some(_) => problems.push("permissions: список прав".into()),
    }
    let permissions: Vec<Permission> = permissions.into_iter().collect();

    let settings = settings(root.get("settings"), &mut problems);
    let contributes = contributes(root.get("contributes"), &permissions, &mut problems);
    // `at: "{key}"` — the time a person sets in the plugin's settings: a string setting of it.
    for spec in &contributes.schedules {
        let super::schedule::When::At { at, .. } = &spec.when else { continue };
        let Some(key) = at.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')) else { continue };
        if !settings.iter().any(|setting| setting.key == key && setting.r#type == SettingType::String) {
            problems.push(format!("contributes.schedules «{}»: at «{at}» — нет строковой настройки «{key}»", spec.id));
        }
    }
    // A keyboard is plugged into a computer (docs/plans/midi.md): an app that hears one must run there.
    let hears_midi = permissions.iter().any(|permission| {
        matches!(permission, Permission::Bus { level, pattern }
            if *level >= crate::bus::Level::Listen && crate::bus::Address::parse("device/midi").is_ok_and(|midi| pattern.matches(&midi)))
    });
    if hears_midi && !runs.contains(&Placement::Pc) {
        problems.push("midi: MIDI-клавиатура подключается к компьютеру — добавьте pc в runs".into());
    }
    // A character and a theme are data: what needs code is a plugin's to give.
    if matches!(kind, PackageKind::Character | PackageKind::Theme) {
        let coded = [
            ("intents", contributes.intents.len()),
            ("tools", contributes.tools.len()),
            ("media", contributes.media.len()),
            ("events", contributes.events.len()),
            ("schedules", contributes.schedules.len()),
        ];
        for (what, _) in coded.into_iter().filter(|(_, count)| *count > 0) {
            problems.push(format!("contributes.{what}: у пакета вида {} нет кода, это дело плагина", kind.as_str()));
        }
    }

    if !problems.is_empty() {
        return Err(ManifestError { problems });
    }
    Ok(Manifest {
        kind,
        id,
        version,
        title,
        description,
        license,
        api,
        runtime,
        mcp_url,
        targets,
        health,
        runs,
        prefer_home,
        budget,
        permissions,
        settings,
        contributes,
        data_version,
    })
}

fn settings(value: Option<&Value>, problems: &mut Vec<String>) -> Vec<SettingSpec> {
    let Some(value) = value else { return Vec::new() };
    let Some(items) = value.as_array() else {
        problems.push("settings: список настроек".into());
        return Vec::new();
    };
    if items.len() > MAX_SETTINGS {
        problems.push(format!("settings: не больше {MAX_SETTINGS}"));
    }
    let mut specs: Vec<SettingSpec> = Vec::new();
    for item in items.iter().take(MAX_SETTINGS) {
        let field = |key: &str| item.get(key).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty());
        let key = field("key").unwrap_or_default();
        if !identifier(key) || key.contains('$') {
            problems.push(format!("settings: ключ «{key}» — латинские буквы, цифры и _"));
            continue;
        }
        if specs.iter().any(|spec| spec.key == key) {
            problems.push(format!("settings: ключ «{key}» дважды"));
            continue;
        }
        let r#type = match field("type") {
            Some("string") => SettingType::String,
            Some("number") => SettingType::Number,
            Some("boolean") => SettingType::Boolean,
            _ => {
                problems.push(format!("settings.{key}: type — string, number или boolean"));
                continue;
            }
        };
        let options = match item.get("options") {
            None => None,
            Some(Value::Array(options)) if r#type == SettingType::String && !options.is_empty() => {
                Some(options.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>())
            }
            Some(_) => {
                problems.push(format!("settings.{key}: options — список строк, только у type string"));
                None
            }
        };
        let secret = match item.get("secret") {
            None | Some(Value::Bool(false)) => false,
            Some(Value::Bool(true)) if r#type == SettingType::String => true,
            Some(Value::Bool(true)) => {
                problems.push(format!("settings.{key}: secret — только у type string"));
                continue;
            }
            Some(_) => {
                problems.push(format!("settings.{key}: secret — true или false"));
                continue;
            }
        };
        let spec = SettingSpec {
            key: key.to_string(),
            r#type,
            title: field("title").unwrap_or(key).chars().take(80).collect(),
            description: field("description").map(|text| text.chars().take(300).collect()),
            default: item.get("default").cloned(),
            options,
            secret,
        };
        if let Some(default) = &spec.default {
            if !spec.accepts(default) {
                problems.push(format!("settings.{key}: default не подходит к type"));
                continue;
            }
        }
        specs.push(spec);
    }
    specs
}

fn handler_of(item: &Value, fallback: &str, at: &str, problems: &mut Vec<String>) -> String {
    let handler = item.get("handler").and_then(Value::as_str).unwrap_or(fallback).trim().to_string();
    if !identifier(&handler) {
        problems.push(format!("{at}: handler «{handler}» — имя функции, которую экспортирует backend.js"));
    }
    handler
}

fn capability_of(item: &Value, at: &str, problems: &mut Vec<String>) -> CapabilityName {
    match item.get("capability").and_then(Value::as_str) {
        None => CapabilityName::Plugins,
        Some(name) => match is_capability(name) {
            Some(capability) if STATION_CONTROLS.contains(&capability) => {
                problems.push(format!("{at}: «{name}» — управление колонкой, его ведут встроенные навыки"));
                CapabilityName::Plugins
            }
            Some(capability) => capability,
            None => {
                problems.push(format!("{at}: нет возможности «{name}»"));
                CapabilityName::Plugins
            }
        },
    }
}

/// Whether the rights reach `address` on the bus at least at `level`.
pub fn asks(permissions: &[Permission], address: &str, level: crate::bus::Level) -> bool {
    let Ok(address) = crate::bus::Address::parse(address) else { return false };
    permissions.iter().any(|permission| {
        matches!(permission, Permission::Bus { level: have, pattern } if *have >= level && pattern.matches(&address))
    })
}

fn list<'a>(contributes: &'a Map<String, Value>, key: &str, problems: &mut Vec<String>) -> &'a [Value] {
    match contributes.get(key) {
        None => &[],
        Some(Value::Array(items)) => items,
        Some(_) => {
            problems.push(format!("contributes.{key}: список"));
            &[]
        }
    }
}

fn contributes(value: Option<&Value>, permissions: &[Permission], problems: &mut Vec<String>) -> Contributes {
    let Some(value) = value else { return Contributes::default() };
    let Some(contributes) = value.as_object() else {
        problems.push("contributes: объект".into());
        return Contributes::default();
    };
    let mut out = Contributes {
        panel: contributes.get("panel").cloned(),
        themes: contributes.get("themes").cloned(),
        characters: contributes.get("characters").cloned(),
        ..Contributes::default()
    };

    let intents = list(contributes, "intents", problems);
    let mut phrases_seen = 0;
    for (index, item) in intents.iter().enumerate() {
        let at = format!("contributes.intents[{index}]");
        let handler = handler_of(item, "", &at, problems);
        let capability = capability_of(item, &at, problems);
        let mut phrases = Vec::new();
        match item.get("phrases").and_then(Value::as_array) {
            Some(templates) if !templates.is_empty() => {
                for template in templates {
                    phrases_seen += 1;
                    match template.as_str().map(Phrase::parse) {
                        Some(Ok(phrase)) => phrases.push(phrase),
                        Some(Err(problem)) => problems.push(format!("{at}: {problem}")),
                        None => problems.push(format!("{at}: фраза — строка")),
                    }
                }
            }
            _ => problems.push(format!("{at}: phrases — непустой список фраз")),
        }
        out.intents.push(IntentSpec { phrases, handler, capability });
    }
    if phrases_seen > MAX_PHRASES {
        problems.push(format!("contributes.intents: не больше {MAX_PHRASES} фраз"));
    }

    let tools = list(contributes, "tools", problems);
    if tools.len() > MAX_TOOLS {
        problems.push(format!("contributes.tools: не больше {MAX_TOOLS}"));
    }
    for (index, item) in tools.iter().enumerate().take(MAX_TOOLS) {
        let name = item.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
        let at = format!("contributes.tools[{index}]");
        if !tool_name(&name) {
            problems.push(format!("{at}: name — строчные латинские буквы, цифры и _, с буквы, до 48 знаков"));
        } else if tool_spec(&name).is_some() || name.starts_with("mcp_") {
            problems.push(format!("{at}: имя «{name}» занято встроенными инструментами"));
        } else if out.tools.iter().any(|tool| tool.name == name) {
            problems.push(format!("{at}: имя «{name}» дважды"));
        }
        let description: String = item
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .chars()
            .take(1024)
            .collect();
        if description.is_empty() {
            problems.push(format!("{at}: нужно description — по нему мозг выбирает инструмент"));
        }
        let input = match item.get("input").or_else(|| item.get("inputSchema")) {
            None => json!({ "type": "object", "properties": {} }),
            Some(schema) if schema.get("type") == Some(&json!("object")) => normalize_schema(schema),
            Some(_) => {
                problems.push(format!("{at}: input — JSON Schema объекта (\"type\": \"object\")"));
                json!({ "type": "object", "properties": {} })
            }
        };
        let read_only = item.pointer("/annotations/readOnlyHint").and_then(Value::as_bool).unwrap_or(false);
        let hint = |name: &str| item.pointer(&format!("/annotations/{name}")).and_then(Value::as_bool).unwrap_or(false);
        let danger = crate::bus::danger::Danger::of_annotations(hint("destructiveHint"), hint("openWorldHint"));
        out.tools.push(ToolSpec {
            handler: handler_of(item, &name, &at, problems),
            capability: capability_of(item, &at, problems),
            name,
            description,
            input,
            read_only,
            danger: if read_only { None } else { danger },
        });
    }

    let media = list(contributes, "media", problems);
    for (index, item) in media.iter().enumerate() {
        let at = format!("contributes.media[{index}]");
        let id = item.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        if !valid_id(&id.replace('_', "-")) {
            problems.push(format!("{at}: id — строчные латинские буквы, цифры, «-» и «_»"));
        } else if out.media.iter().any(|spec| spec.id == id) {
            problems.push(format!("{at}: id «{id}» дважды"));
        }
        let Some(kind) = item.get("kind").and_then(Value::as_str).and_then(MediaKind::parse) else {
            problems.push(format!("{at}: kind — radio, music или tales"));
            continue;
        };
        let handler = handler_of(item, &id, &at, problems);
        out.media.push(MediaSpec { id, kind, handler });
    }

    let events = list(contributes, "events", problems);
    for (index, item) in events.iter().enumerate() {
        let at = format!("contributes.events[{index}]");
        let name = item.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
        if !PLUGIN_EVENTS.contains(&name.as_str()) {
            problems.push(format!("{at}: событие «{name}» плагинам не отдаётся; есть {}", PLUGIN_EVENTS.join(", ")));
            continue;
        }
        let handler = handler_of(item, "", &at, problems);
        out.events.push(EventSpec { name, handler });
    }
    let schedules = list(contributes, "schedules", problems);
    if !schedules.is_empty() && !asks(permissions, "system/schedule", crate::bus::Level::Call) {
        problems.push("contributes.schedules: нужна связь call:system/schedule".into());
    }
    if schedules.len() > super::schedule::MAX_RHYTHMS {
        problems.push(format!("contributes.schedules: до {} ритмов", super::schedule::MAX_RHYTHMS));
    }
    for (index, item) in schedules.iter().enumerate().take(super::schedule::MAX_RHYTHMS) {
        let at = format!("contributes.schedules[{index}]");
        let Some(spec) = super::schedule::rhythm(item, &at, permissions, problems) else { continue };
        if out.schedules.iter().any(|known| known.id == spec.id) {
            problems.push(format!("{at}: id «{}» уже есть", spec.id));
            continue;
        }
        out.schedules.push(spec);
    }
    out.bus = bus_spec(contributes.get("bus"), permissions, problems);
    out
}

/// `contributes.bus`: every problem named.
fn bus_spec(value: Option<&Value>, permissions: &[Permission], problems: &mut Vec<String>) -> BusSpec {
    let Some(value) = value else { return BusSpec::default() };
    let Some(bus) = value.as_object() else {
        problems.push("contributes.bus: объект".into());
        return BusSpec::default();
    };
    let mut spec = BusSpec::default();
    let names = |key: &str, problems: &mut Vec<String>| -> Vec<String> {
        match bus.get(key) {
            None => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|item| match item.as_str() {
                    Some(name) if identifier(name) || news_name(name) => Some(name.to_string()),
                    _ => {
                        problems
                            .push(format!("contributes.bus.{key}: «{}» — не имя", crate::skills::js::string_of(item)));
                        None
                    }
                })
                .collect(),
            Some(_) => {
                problems.push(format!("contributes.bus.{key}: список имён"));
                Vec::new()
            }
        }
    };
    spec.private = names("private", problems);
    spec.emits = names("emits", problems);
    if let Some(marks) = bus.get("danger") {
        match marks.as_object() {
            Some(marks) => {
                for (handle, mark) in marks {
                    match mark.as_str().and_then(crate::bus::danger::Danger::parse) {
                        Some(danger) => {
                            spec.danger.insert(handle.clone(), danger);
                        }
                        None => problems.push(format!(
                            "contributes.bus.danger.{handle}: money, delete, outside, locks, camera или access"
                        )),
                    }
                }
            }
            None => problems.push("contributes.bus.danger: объект «умение → опасность»".into()),
        }
    }
    if let Some(ask) = bus.get("ask") {
        match ask.as_str().filter(|name| identifier(name)) {
            Some(name) => spec.ask = Some(name.to_string()),
            None => problems.push("contributes.bus.ask: имя функции, что отвечает словами".into()),
        }
    }
    let heard: Vec<&crate::bus::Pattern> = permissions
        .iter()
        .filter_map(|permission| match permission {
            Permission::Bus { level, pattern } if *level >= crate::bus::Level::Listen => Some(pattern),
            _ => None,
        })
        .collect();
    for (index, item) in bus.get("on").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let at = format!("contributes.bus.on[{index}]");
        let from = item.get("from").and_then(Value::as_str).map(crate::bus::Pattern::parse);
        let Some(Ok(from)) = from else {
            problems.push(format!("{at}: from — чей адрес (app/shopping, files/home/Фото/*)"));
            continue;
        };
        let event = item.get("event").and_then(Value::as_str).unwrap_or_default().to_string();
        if !news_name(&event) {
            problems.push(format!("{at}: event — имя новости"));
            continue;
        }
        if !heard.iter().any(|pattern| pattern.covers(&from)) {
            problems.push(format!("{at}: слушать «{from}» — нужно право listen:{from}"));
        }
        let handler = handler_of(item, "", &at, problems);
        spec.on.push(OnSpec { from, event, handler });
    }
    spec
}

#[cfg(test)]
mod tests {
    use super::*;

    fn radio() -> Value {
        json!({
            "schema": 1,
            "id": "radio",
            "version": "1.2.0",
            "title": "Радио со всего мира",
            "api": "^2",
            "runs": ["pc", "station", "stationServer"],
            "budget": { "memoryMb": 4, "storageMb": 2 },
            "permissions": ["net:*.api.radio-browser.info"],
            "settings": [{ "key": "country", "type": "string", "title": "Страна", "default": "RU" }],
            "contributes": {
                "intents": [{ "phrases": ["включи радио {station}"], "handler": "playStation", "capability": "radio" }],
                "tools": [{
                    "name": "radio_search",
                    "description": "Ищет радиостанции",
                    "input": { "type": "object", "properties": { "query": { "type": "string" } } },
                    "annotations": { "readOnlyHint": true }
                }],
                "media": [{ "id": "stations", "kind": "radio" }],
                "panel": { "sections": [{ "id": "radio", "title": "Радио", "icon": "music" }] },
                "themes": []
            }
        })
    }

    fn with(mut manifest: Value, pointer: &str, value: Value) -> Value {
        *manifest.pointer_mut(pointer).unwrap_or_else(|| panic!("{pointer}")) = value;
        manifest
    }

    fn problems(manifest: Value) -> Vec<String> {
        check(&manifest).unwrap_err().problems
    }

    #[test]
    fn reads_the_plans_example() {
        let manifest = check(&radio()).unwrap();
        assert_eq!((manifest.id.as_str(), manifest.version.to_string()), ("radio", "1.2.0".into()));
        assert_eq!(manifest.runs, [Placement::Pc, Placement::Station, Placement::StationServer]);
        assert_eq!(
            manifest.budget,
            Budget { memory_mb: 4, storage_mb: 2, files_mb: crate::bus::providers::files::APP_FOLDER_MB }
        );
        assert_eq!(manifest.hosts(), ["*.api.radio-browser.info"]);
        let intent = &manifest.contributes.intents[0];
        assert_eq!((intent.handler.as_str(), intent.capability), ("playStation", CapabilityName::Radio));
        let tool = &manifest.contributes.tools[0];
        assert_eq!((tool.name.as_str(), tool.handler.as_str(), tool.read_only), ("radio_search", "radio_search", true));
        assert_eq!(tool.capability, CapabilityName::Plugins);
        assert_eq!(
            manifest.contributes.media,
            [MediaSpec { id: "stations".into(), kind: MediaKind::Radio, handler: "stations".into() }]
        );
        assert_eq!(manifest.settings_values(None)["country"], "RU");
        let stored = json!({"country": "DE", "stale": 1});
        assert_eq!(manifest.settings_values(stored.as_object())["country"], "DE");
        let wrong = json!({"country": 5});
        assert_eq!(manifest.settings_values(wrong.as_object())["country"], "RU", "a value that no longer fits");
        assert!(manifest.incompatible_with(Placement::Station).is_none());
    }

    #[test]
    fn a_secret_setting_is_its_backends_as_it_is_and_the_panels_only_as_dots() {
        let settings = json!([
            { "key": "ics", "type": "string", "title": "Ссылка iCal", "secret": true },
            { "key": "country", "type": "string", "title": "Страна", "default": "RU", "secret": false }
        ]);
        let manifest = check(&with(radio(), "/settings", settings)).unwrap();
        assert_eq!(manifest.settings.iter().map(|setting| setting.secret).collect::<Vec<_>>(), [true, false]);
        let link = "https://calendar.example/private-0123/basic.ics";
        let stored = json!({ "ics": link });
        assert_eq!(manifest.settings_values(stored.as_object())["ics"], link);
        let shown = manifest.settings_shown(stored.as_object());
        assert_eq!((shown["ics"].as_str(), shown["country"].as_str()), (Some(SECRET_SHOWN), Some("RU")));
        assert!(!manifest.settings_shown(None).contains_key("ics"), "not set: nothing to hide");
        assert_eq!(manifest.settings_shown(json!({ "ics": "" }).as_object())["ics"], "", "empty stays empty");
        // The panel learns which one is secret; the others say nothing new.
        assert_eq!(serde_json::to_value(&manifest.settings[0]).unwrap()["secret"], true);
        assert!(serde_json::to_value(&manifest.settings[1]).unwrap().get("secret").is_none());

        let number = json!([{ "key": "pin", "type": "number", "title": "Код", "secret": true }]);
        assert_eq!(problems(with(radio(), "/settings", number)), ["settings.pin: secret — только у type string"]);
        let word = json!([{ "key": "ics", "type": "string", "title": "Ссылка", "secret": "yes" }]);
        assert_eq!(problems(with(radio(), "/settings", word)), ["settings.ics: secret — true или false"]);
    }

    #[test]
    fn refuses_another_host_api_or_a_placement_it_does_not_name_with_the_reason() {
        let newer = problems(with(radio(), "/api", json!("^3")));
        assert_eq!(newer, [format!("нужен Host API ^3, а у шлюза {HOST_API}")]);
        // An app for Host API 1 is told what to do with it.
        let older = problems(with(radio(), "/api", json!("^1.1")));
        assert!(older[0].contains("Host API 1") && older[0].contains("переделайте"), "{older:?}");
        // A character or a theme is data: any Host API reads.
        let character = json!({ "schema": 1, "kind": "character", "id": "ded", "version": "1.0.0", "title": "Дед", "api": "^1",
            "runs": ["pc"], "contributes": { "characters": [] } });
        assert!(check(&character).is_ok(), "{:?}", check(&character).err());
        assert!(check(&with(radio(), "/api", json!(">=1.0.0, <3"))).is_ok());
        let unknown = problems(with(radio(), "/runs", json!(["pc", "station+home"])));
        assert_eq!(unknown, ["runs: «station+home» — нет такого размещения; есть pc, station, stationServer"]);
        let pc_only = check(&with(radio(), "/runs", json!(["pc"]))).unwrap();
        let why = pc_only.incompatible_with(Placement::Station).unwrap();
        assert!(why.contains("На колонке") && why.contains("pc"), "{why}");
        assert!(pc_only.incompatible_with(Placement::Pc).is_none());
        assert_eq!(problems(with(radio(), "/schema", json!(2))), ["schema 2: этот шлюз понимает манифесты версии 1"]);
        let mut wasm = radio();
        wasm["runtime"] = json!("wasm");
        assert_eq!(check(&wasm).unwrap().runtime, Runtime::Wasm);
        let mut flash = radio();
        flash["runtime"] = json!("flash");
        assert!(problems(flash)[0].starts_with("runtime «flash» не подходит"));
        let mut mcp = radio();
        mcp["runtime"] = json!("mcp");
        assert_eq!(problems(mcp.clone())[0], "mcp.url: адрес MCP-сервера (https://…)");
        mcp["mcp"] = json!({ "url": "https://weather.example/mcp" });
        assert_eq!(check(&mcp).unwrap().mcp_url.as_deref(), Some("https://weather.example/mcp"));
    }

    #[test]
    fn lists_every_problem_at_once() {
        let manifest = json!({
            "schema": 1,
            "id": "Radio!",
            "version": "one",
            "api": "^2",
            "runs": [],
            "budget": { "memoryMb": 500 },
            "permissions": ["camera", "net:http://x.ru", "voice.say"],
            "contributes": {
                "intents": [{ "phrases": ["{station}"], "handler": "play station" }],
                "tools": [{ "name": "set_timer", "description": "" }],
                "media": [{ "id": "stations", "kind": "video" }],
                "events": [{ "name": "request.updated", "handler": "on" }]
            }
        });
        let found = problems(manifest);
        for expected in [
            "id:",
            "version:",
            "нет title",
            "runs:",
            "budget.memoryMb",
            "«camera»",
            "«net:http://x.ru»",
            "«voice.say» — право Host API 1; теперь: call:system/voice",
            "начинаться со слова",
            "handler «play station»",
            "«set_timer» занято",
            "нужно description",
            "kind — radio, music или tales",
            "«request.updated»",
        ] {
            assert!(found.iter().any(|problem| problem.contains(expected)), "{expected} in {found:#?}");
        }
    }

    #[test]
    fn reads_every_kind_of_package_in_one_format_and_keeps_code_to_plugins() {
        assert_eq!(check(&radio()).unwrap().kind, PackageKind::Plugin, "a plugin when it does not say");
        let character = json!({
            "schema": 1, "kind": "character", "id": "ded-moroz", "version": "1.0.0", "title": "Дед Мороз", "api": "^2",
            "runs": ["pc", "station", "stationServer"],
            "contributes": { "characters": [{ "id": "ded-moroz", "prompt": "…", "voices": { "omnivoice": "…" } }] }
        });
        let manifest = check(&character).unwrap();
        assert_eq!(manifest.kind, PackageKind::Character);
        assert_eq!(manifest.license, None);
        let mut licensed = character.clone();
        licensed["license"] = json!("CC BY-NC 4.0, некоммерчески");
        assert_eq!(check(&licensed).unwrap().license.as_deref(), Some("CC BY-NC 4.0, некоммерчески"));
        licensed["license"] = json!("x".repeat(201));
        assert_eq!(problems(licensed), ["license: одна строка до 200 знаков"]);
        assert_eq!(manifest.contributes.characters.unwrap()[0]["id"], "ded-moroz");
        assert!(PackageKind::Character.not_a_plugin().is_some() && PackageKind::Plugin.not_a_plugin().is_none());
        let unknown = with(character.clone(), "/kind", json!("skin"));
        assert_eq!(problems(unknown), ["kind «skin»: такого вида пакетов нет; есть plugin, app, character, theme"]);
        let mut talking = character.clone();
        talking["permissions"] = json!([]);
        talking["contributes"]["intents"] = json!([{ "phrases": ["скажи хо-хо"], "handler": "ho" }]);
        assert_eq!(problems(talking), ["contributes.intents: у пакета вида character нет кода, это дело плагина"]);
        let mut app = with(character, "/kind", json!("app"));
        app["runtime"] = json!("process");
        assert_eq!(
            problems(app.clone())[0],
            "targets: сборки приложения под платформы (linux-aarch64, darwin-arm64, linux-x86_64)"
        );
        app["targets"] = json!({ "linux-aarch64": { "file": "bin/app-aarch64", "sha256": "a".repeat(64) } });
        app["health"] = json!({ "path": "/ok", "intervalS": 30 });
        let parsed = check(&app).unwrap();
        assert_eq!((parsed.kind, parsed.runtime), (PackageKind::App, Runtime::Process));
        assert_eq!(parsed.targets["linux-aarch64"].file, "bin/app-aarch64");
        assert_eq!(parsed.health, HealthCheck { path: "/ok".into(), interval_s: 30 });
        app["targets"]["linux-aarch64"]["file"] = json!("../../etc/passwd");
        assert!(problems(app)[0].starts_with("targets.linux-aarch64.file"));
        assert!(platform().contains('-'), "{}", platform());
        let mut process = radio();
        process["runtime"] = json!("process");
        assert!(problems(process)[0].starts_with("runtime «process» не подходит"), "a plugin's code is not a program");
    }

    #[test]
    fn keeps_the_stations_controls_with_the_built_in_skills() {
        let volume = with(radio(), "/contributes/intents/0/capability", json!("volume"));
        assert!(problems(volume)[0].contains("управление колонкой"));
        let nothing = with(radio(), "/contributes/intents/0/capability", json!("teleport"));
        assert!(problems(nothing)[0].contains("нет возможности «teleport»"));
    }

    #[test]
    fn matches_hosts_exactly_or_by_their_domain() {
        let patterns = vec!["*.api.radio-browser.info".to_string(), "example.com".to_string()];
        assert!(host_allowed(&patterns, "de1.api.radio-browser.info"));
        assert!(host_allowed(&patterns, "DE1.api.radio-browser.info."));
        assert!(!host_allowed(&patterns, "api.radio-browser.info"), "the domain itself is not its subdomain");
        assert!(!host_allowed(&patterns, "evilapi.radio-browser.info"));
        assert!(host_allowed(&patterns, "example.com"));
        assert!(!host_allowed(&patterns, "www.example.com"));
        assert!(!host_allowed(&patterns, "example.com.evil.org"));
        assert!(Permission::parse("net:localhost").is_none(), "a bare name is no host on the internet");
        assert!(Permission::parse("net:*.ru").is_none(), "a whole zone is too much");
    }
}
