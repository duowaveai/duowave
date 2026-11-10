//! The plugin host: the packages in PACKAGES_DIR (of every kind; it runs the plugins), the admin's choices, and the
//! code of the plugins that are on.
//!
//! - A plugin's code is loaded at its first call (a phrase, a tool, a source) and unloaded after
//!   `PLUGINS_IDLE_UNLOAD_S` without one: a plugin nobody uses costs nothing but its place on disk — and nothing of
//!   the station's memory: a context takes its declared budget only while it is loaded (budget.rs).
//! - On the station a context loads only when its budget fits what is free; if it does not, idle contexts unload
//!   first, the longest idle first, one in a call never; if it still does not, the call fails and says why.
//! - Switching one on starts its code first: what does not start stays off, and the admin reads why. Memory never
//!   keeps a plugin off: one that has no room to start now is on, and loads at its first call.
//! - A reload starts the newest version on disk beside the one running. It starts — it takes over; it does not —
//!   the one running stays, and its error is in the log and the panel. After a restart the same: a version that
//!   does not start gives way to the one that last did (kept in the database).
//! - What the plugins that are on give is rebuilt on every change: their phrases (the router asks `matched`), their
//!   tools (`tool_contributions`), their media sources.
//! - The safe mode (PLUGINS_DISABLED=1) runs none of it, whatever is switched on.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Map, Value};

use super::budget::{self, Idle, MemoryGauge, MemoryNow, MemoryProbe, PluginShare, PressureAction, ResourcesView};
use super::bundled::{self, BundledPackage};
use super::host_api::{storage_used, DataNews, PluginHostApi, SpeakerSource};
use super::manifest::{self, Budget, Manifest, PackageKind, Permission, SettingSpec, ToolSpec};
use super::media::PluginMediaSource;
use super::phrases::Phrase;
use super::registry::{self, Candidate, Origin, Package};
use super::runtime::{CallKind, MemoryStats, PluginError, PluginInstance, PluginRuntime};
use crate::config::PluginsConfig;
use crate::core::availability::Placement;
use crate::core::connectors::{ToolContext, ToolContribution};
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::core::identity::permissions::use_of;
use crate::dialog::llm::LlmTool;
use crate::operations::system_memory::Pressure;
use crate::platform::http::ApiError;
use crate::settings::definitions::PACKAGE_SETTINGS;
use crate::settings::SettingsService;
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::media::playback::{PlaybackNews, PlaybackReport, PlaybackWatch};
use crate::skills::media::player::MediaPlayer;
use crate::skills::media::sources::MediaSources;
use crate::skills::quick_commands::QuickMatch;
use crate::skills::router::{ExtraRules, EXECUTOR};
use crate::storage::Db;
use crate::utils::time::now_iso;

/// What the model reads of a tool's answer at most.
const TOOL_ANSWER: usize = 16 * 1024;
/// Lines of a plugin's own journal kept in memory.
const JOURNAL_LINES: usize = 300;

/// A line of a plugin's own journal (for the panel and the workshop): its console, its calls, its loads, and what the
/// station said of its items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JournalLine {
    pub at: String,
    /// info, warn, error — its console; call, load, player — the host's.
    pub level: String,
    pub message: String,
}

/// Where the gateway runs now: a plugin runs only where its `runs` says.
pub type PlacementNow = Arc<dyn Fn() -> Placement + Send + Sync>;

/// The home server as the plugins see it (stage 7, «колонка + сервер»): its address and key while it is up — a
/// plugin with `prefer: home` runs there then.
pub trait HomeLink: Send + Sync {
    fn up(&self) -> Option<(String, String)>;
}

/// A call to the home server's copy of a plugin: `POST <url>/home/v1/plugins/<id>/call`.
async fn call_home(
    url: &str,
    key: &str,
    id: &str,
    function: &str,
    input: &Value,
    kind: CallKind,
) -> Result<Value, String> {
    let kind_name = match kind {
        CallKind::Voice => "voice",
        CallKind::Tool => "tool",
        CallKind::Background => "background",
    };
    let response = crate::utils::http::client()
        .post(format!("{}/home/v1/plugins/{id}/call", url.trim_end_matches('/')))
        .bearer_auth(key)
        .timeout(kind.limit())
        .json(&json!({ "function": function, "input": input, "kind": kind_name }))
        .send()
        .await
        .map_err(|error| format!("компьютер не ответил: {}", error.without_url()))?;
    let status = response.status();
    let body: Value = response.json().await.map_err(|_| format!("компьютер ответил не JSON (HTTP {status})"))?;
    if !status.is_success() {
        let message = body.get("message").and_then(Value::as_str).unwrap_or("отказ").to_string();
        return Err(format!("компьютер: {message}"));
    }
    Ok(body.pointer("/message/result").cloned().or_else(|| body.get("result").cloned()).unwrap_or(Value::Null))
}

pub struct PluginsDeps {
    pub config: PluginsConfig,
    pub db: Arc<Db>,
    pub settings: Arc<SettingsService>,
    pub events: EventBus,
    /// The media sources the media skills read: the plugins' join and leave here.
    pub media: Arc<MediaSources>,
    pub placement: PlacementNow,
    /// How plugins speak without a request (`host.voice.say`): the station link's speaker, once it stands.
    pub speaker: SpeakerSource,
    /// The engine; QuickJS unless a test gives its own.
    pub runtime: Arc<dyn PluginRuntime>,
    /// The machine's memory and disk: the plugins' budget (budget.rs).
    pub memory: Arc<dyn MemoryProbe>,
    /// The plugins' free memory for the availability registry (`Need::Memory`).
    pub gauge: Arc<MemoryGauge>,
    /// The packages built into the gateway (bundled.rs), laid out at the start; none in tests.
    pub bundled: Vec<BundledPackage>,
    /// Our player and its watch: a tool that acts plays through the player; a plugin answers a silent station with
    /// what to play instead through the watch (stage 5а). None in tests that do not play.
    pub player: Option<Arc<MediaPlayer>>,
    pub watch: Option<Arc<PlaybackWatch>>,
    /// The home server, on the station with one: where a plugin with `prefer: home` runs while it is up.
    pub home: Option<Arc<dyn HomeLink>>,
    /// Where an app's program reaches the gateway's Host API: `http://127.0.0.1:<port>` (src/apps).
    pub gateway_url: String,
    /// The gateway's services behind `host.call` (content, tools, smart home…); none in tests that need none.
    pub services: Option<Arc<dyn super::host_api::HostServices>>,
}

/// The last thing that went wrong with a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LastError {
    pub at: String,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
struct Stats {
    calls: u64,
    errors: u64,
    last_error: Option<LastError>,
    last_call_at: Option<String>,
}

#[derive(Default)]
struct State {
    /// Every version on disk, newest first.
    found: Vec<Candidate>,
    enabled: bool,
    /// The version that last started well.
    good: Option<String>,
    /// The version in force while it is on: its manifest gives the phrases, the tools and the sources.
    active: Option<Arc<Package>>,
    /// Why it does not run, or why the newest version does not: for the panel.
    problem: Option<String>,
    /// Where its package came from (catalog, local) and whether the catalog vouched for it.
    source: String,
    verified: bool,
}

struct Loaded {
    version: String,
    instance: Arc<dyn PluginInstance>,
    /// What it takes of the plugins' memory budget while loaded (`share_of`).
    share_mb: u64,
}

/// What a package's context takes of the plugins' memory budget while it is loaded: a plugin's declared
/// `budget.memoryMb`, its engine's own hard limit. An app's program is a process of its own, counted by what it holds
/// (budget.rs): it takes no share.
fn share_of(manifest: &Manifest) -> u64 {
    if manifest.kind == PackageKind::Plugin {
        manifest.budget.memory_mb
    } else {
        0
    }
}

struct Plugin {
    id: String,
    state: Mutex<State>,
    /// Its code, while loaded; held while it loads, so two first calls load it once.
    slot: tokio::sync::Mutex<Option<Loaded>>,
    stats: Mutex<Stats>,
    /// Milliseconds since the host began, at its last call.
    used: AtomicU64,
    /// Its calls running now: a context in a call is never unloaded to make room.
    busy: AtomicUsize,
    /// What its loaded context takes of the budget (`Loaded::share_mb`); none while nothing is loaded.
    taken_mb: AtomicU64,
    /// What a context of it that is loading now took of the budget until it is in its slot (`Loading`).
    loading_mb: AtomicU64,
    /// What its engine held at the last look, MB: kept so a load's count asks no engine (a busy one answers late).
    held_mb: AtomicU64,
}

impl Plugin {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn stats(&self) -> std::sync::MutexGuard<'_, Stats> {
        self.stats.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The version in force, while it is on.
    fn active(&self) -> Option<Arc<Package>> {
        let state = self.state();
        state.active.clone().filter(|_| state.enabled)
    }

    /// Its context into the slot: its share of the budget is taken while it is there. The one it replaces, if any.
    fn hold(&self, slot: &mut Option<Loaded>, loaded: Loaded) -> Option<Loaded> {
        self.taken_mb.store(loaded.share_mb, Ordering::Relaxed);
        self.held_mb.store(0, Ordering::Relaxed);
        slot.replace(loaded)
    }

    /// Its context out of the slot (to stop it): its share of the budget is free again.
    fn release(&self, slot: &mut Option<Loaded>) -> Option<Loaded> {
        self.taken_mb.store(0, Ordering::Relaxed);
        self.held_mb.store(0, Ordering::Relaxed);
        slot.take()
    }
}

/// A call of a plugin under way: while it lasts its context is not unloaded to make room for another's.
struct Busy<'a>(&'a Plugin);

impl<'a> Busy<'a> {
    fn new(plugin: &'a Plugin) -> Busy<'a> {
        plugin.busy.fetch_add(1, Ordering::Relaxed);
        Busy(plugin)
    }
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.busy.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The room a context took while it loads: counted with the loaded ones, so two loads at once do not count on the same
/// megabytes; given back once the context is in its slot (`Plugin::hold` takes it there) or did not load.
struct Loading<'a> {
    plugin: &'a Plugin,
    mb: u64,
}

impl<'a> Loading<'a> {
    fn take(plugin: &'a Plugin, mb: u64) -> Loading<'a> {
        plugin.loading_mb.fetch_add(mb, Ordering::Relaxed);
        Loading { plugin, mb }
    }
}

impl Drop for Loading<'_> {
    fn drop(&mut self) {
        self.plugin.loading_mb.fetch_sub(self.mb, Ordering::Relaxed);
    }
}

/// A phrase of a plugin that is on.
struct Rule {
    plugin: String,
    /// `plugin:<id>`, as the journal names the rule.
    name: &'static str,
    phrase: Phrase,
    handler: String,
    capability: CapabilityName,
}

/// A tool of a plugin that is on.
struct Offered {
    plugin: String,
    title: String,
    spec: ToolSpec,
    outside: bool,
}

/// A rule's name for the journal, made once per plugin: the router's rule names live as long as the program.
fn rule_name(plugin: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut names = NAMES.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    names.entry(plugin.to_string()).or_insert_with(|| Box::leak(format!("plugin:{plugin}").into_boxed_str()))
}

/// The newest version on disk that runs here, and why a newer one does not (or why none does).
fn choose(found: &[Candidate], placement: Placement) -> (Option<Arc<Package>>, Option<String>) {
    let mut problem = None;
    for candidate in found {
        match candidate {
            Candidate::Ready(package) => {
                let why = package.manifest.kind.not_a_plugin().map(str::to_string);
                match why.or_else(|| package.manifest.incompatible_with(placement)) {
                    None => return (Some(package.clone()), problem),
                    Some(why) => {
                        problem.get_or_insert_with(|| format!("{}: {why}", package.version()));
                    }
                }
            }
            Candidate::Broken { version, problems } => {
                problem.get_or_insert_with(|| format!("{version}: {}", problems.join("; ")));
            }
        }
    }
    (None, Some(problem.unwrap_or_else(|| "нет ни одной версии в папке".into())))
}

pub struct PluginHost {
    config: PluginsConfig,
    db: Arc<Db>,
    settings: Arc<SettingsService>,
    events: EventBus,
    runtime: Arc<dyn PluginRuntime>,
    placement: PlacementNow,
    speaker: SpeakerSource,
    /// The gateway's runtime: the plugins' requests run there.
    handle: Option<tokio::runtime::Handle>,
    media: Arc<MediaSources>,
    me: Weak<PluginHost>,
    player: Option<Arc<MediaPlayer>>,
    watch: Option<Arc<PlaybackWatch>>,
    home: Option<Arc<dyn HomeLink>>,
    /// The apps' engine (src/apps): their programs under the supervisor, and the tokens they reach the Host API with.
    apps: Arc<dyn PluginRuntime>,
    apps_engine: Arc<crate::apps::ProcessRuntime>,
    services: Option<Arc<dyn super::host_api::HostServices>>,
    app_tokens: Arc<crate::apps::AppTokens>,
    began: Instant,
    plugins: RwLock<BTreeMap<String, Arc<Plugin>>>,
    rules: RwLock<Arc<Vec<Rule>>>,
    tools: RwLock<Arc<Vec<Offered>>>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    memory: Arc<dyn MemoryProbe>,
    gauge: Arc<MemoryGauge>,
    /// Held while a load's room is counted and taken: two loads at once do not count on the same megabytes.
    admitting: Mutex<()>,
    /// Under pressure: the plugins' work in the background waits (`deferred`) until it eases.
    deferring: AtomicBool,
    deferred: Mutex<VecDeque<(String, String, Value)>>,
    /// The last count told on the bus: free MB and the pressure.
    told: Mutex<Option<(u64, Pressure)>>,
    /// Each plugin's own journal, the last lines.
    journals: Mutex<HashMap<String, VecDeque<JournalLine>>>,
    /// The plugins' writes to their `kv`, told the panel (`packages.data`): its widgets read their data again.
    data_news: Arc<DataNews>,
}

impl PluginHost {
    /// The host with what is on disk and in the database; no code is loaded until it is called.
    pub fn new(deps: PluginsDeps) -> Arc<PluginHost> {
        let PluginsDeps {
            config,
            db,
            settings,
            events,
            media,
            placement,
            speaker,
            runtime,
            memory,
            gauge,
            bundled,
            player,
            watch,
            home,
            gateway_url,
            services,
        } = deps;
        let app_tokens = Arc::new(crate::apps::AppTokens::default());
        let give_up_to = std::sync::Arc::new(std::sync::OnceLock::<Weak<PluginHost>>::new());
        let apps_engine = {
            let give_up_to = give_up_to.clone();
            Arc::new(crate::apps::ProcessRuntime::new(
                gateway_url,
                app_tokens.clone(),
                Arc::new(move |id: &str, why: &str| {
                    if let Some(host) = give_up_to.get().and_then(Weak::upgrade) {
                        host.give_up(id, why);
                    }
                }),
            ))
        };
        bundled::lay_out(&config.packages_dir, &db, &bundled);
        let handle = tokio::runtime::Handle::try_current().ok();
        let data_news = DataNews::new(events.clone(), handle.clone());
        let host = Arc::new_cyclic(|me| PluginHost {
            config,
            db,
            settings,
            events,
            runtime,
            placement,
            speaker,
            handle,
            media,
            me: me.clone(),
            player,
            watch,
            home,
            apps: apps_engine.clone(),
            apps_engine,
            services,
            app_tokens,
            began: Instant::now(),
            plugins: RwLock::new(BTreeMap::new()),
            rules: RwLock::new(Arc::new(Vec::new())),
            tools: RwLock::new(Arc::new(Vec::new())),
            tasks: Mutex::new(Vec::new()),
            memory,
            gauge,
            admitting: Mutex::new(()),
            deferring: AtomicBool::new(false),
            deferred: Mutex::new(VecDeque::new()),
            told: Mutex::new(None),
            journals: Mutex::new(HashMap::new()),
            data_news,
        });
        let _ = give_up_to.set(Arc::downgrade(&host));
        host.rescan();
        if host.config.disabled {
            tracing::warn!(target: "plugins", "safe mode (PLUGINS_DISABLED): no plugin runs");
        } else {
            let on: Vec<String> =
                host.list().iter().filter(|plugin| plugin.active().is_some()).map(|p| p.id.clone()).collect();
            tracing::info!(target: "plugins", "{}: {} found, on: {}", host.config.packages_dir.display(), host.list().len(), if on.is_empty() { "none".into() } else { on.join(", ") });
        }
        host
    }

    /// The idle unloading and the events the plugins follow. Inside the runtime.
    pub fn start(self: &Arc<Self>) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let me = Arc::downgrade(self);
        let every = Duration::from_secs((self.config.idle_unload_s / 4).clamp(1, 30));
        tasks.push(tokio::spawn(async move {
            let mut ticks = tokio::time::interval(every);
            loop {
                ticks.tick().await;
                let Some(host) = me.upgrade() else { return };
                host.unload_idle();
            }
        }));
        let me = Arc::downgrade(self);
        tasks.push(self.events.on("plugins.events", move |event| {
            if let Some(host) = me.upgrade() {
                host.deliver(&event);
            }
        }));
        // The memory watch: often on the station, where memory is short; now and then on a computer.
        let me = Arc::downgrade(self);
        let every = Duration::from_secs(if (self.placement)().on_station() { 10 } else { 30 });
        tasks.push(tokio::spawn(async move {
            let mut ticks = tokio::time::interval(every);
            loop {
                ticks.tick().await;
                let Some(host) = me.upgrade() else { return };
                host.watch_memory().await;
            }
        }));
    }

    /// Every plugin's code unloaded, the tasks stopped: at shutdown.
    pub fn shutdown(&self) {
        for task in self.tasks.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).drain(..) {
            task.abort();
        }
        for plugin in self.list() {
            if let Ok(mut slot) = plugin.slot.try_lock() {
                if let Some(loaded) = plugin.release(&mut slot) {
                    loaded.instance.stop();
                }
            }
        }
    }

    pub fn safe_mode(&self) -> bool {
        self.config.disabled
    }

    fn now_ms(&self) -> u64 {
        self.began.elapsed().as_millis() as u64
    }

    fn list(&self) -> Vec<Arc<Plugin>> {
        self.plugins.read().unwrap_or_else(|poisoned| poisoned.into_inner()).values().cloned().collect()
    }

    fn plugin(&self, id: &str) -> Option<Arc<Plugin>> {
        self.plugins.read().unwrap_or_else(|poisoned| poisoned.into_inner()).get(id).cloned()
    }

    /// Reads the folder again: new packages appear, new versions are seen (they take over at a reload), a plugin
    /// whose package is gone stops. The version in force of a plugin that is on stays while it is on disk.
    pub fn rescan(&self) {
        let found = registry::scan(&self.config.packages_dir);
        let choices = registry::choices(&self.db);
        let placement = (self.placement)();
        let ids: Vec<String> = found.keys().chain(choices.keys()).cloned().collect();
        {
            let mut plugins = self.plugins.write().unwrap_or_else(|poisoned| poisoned.into_inner());
            for id in ids {
                let plugin = plugins.entry(id.clone()).or_insert_with(|| {
                    Arc::new(Plugin {
                        id: id.clone(),
                        state: Mutex::new(State::default()),
                        slot: tokio::sync::Mutex::new(None),
                        stats: Mutex::new(Stats::default()),
                        used: AtomicU64::new(0),
                        busy: AtomicUsize::new(0),
                        taken_mb: AtomicU64::new(0),
                        loading_mb: AtomicU64::new(0),
                        held_mb: AtomicU64::new(0),
                    })
                });
                let choice = choices.get(&id).cloned().unwrap_or_default();
                let mut state = plugin.state();
                state.found = found.get(&id).cloned().unwrap_or_default();
                state.enabled = choice.enabled;
                state.good = choice.good;
                state.source = choice.source;
                state.verified = choice.verified;
                let (newest, problem) = choose(&state.found, placement);
                let kept = state.active.as_ref().and_then(|active| {
                    state.found.iter().find_map(|candidate| match candidate {
                        Candidate::Ready(package) if package.version() == active.version() => Some(package.clone()),
                        _ => None,
                    })
                });
                state.active = if state.enabled { kept.or(newest) } else { None };
                state.problem = problem;
            }
            // A package gone from the folder and from the database (removed) is forgotten here too.
            plugins.retain(|id, plugin| {
                let mut state = plugin.state();
                if !found.contains_key(id) && !choices.contains_key(id) {
                    state.found.clear();
                    state.enabled = false;
                    state.active = None;
                }
                !state.found.is_empty() || state.enabled
            });
        }
        self.rebuild();
    }

    /// The phrases, the tools and the media sources of the plugins that are on, anew.
    fn rebuild(&self) {
        let mut rules = Vec::new();
        let mut tools: Vec<Offered> = Vec::new();
        let mut sources = Vec::new();
        if !self.config.disabled {
            for plugin in self.list() {
                let Some(package) = plugin.active() else { continue };
                let manifest = &package.manifest;
                for intent in &manifest.contributes.intents {
                    for phrase in &intent.phrases {
                        rules.push(Rule {
                            plugin: plugin.id.clone(),
                            name: rule_name(&plugin.id),
                            phrase: phrase.clone(),
                            handler: intent.handler.clone(),
                            capability: intent.capability,
                        });
                    }
                }
                for spec in &manifest.contributes.tools {
                    if let Some(taken) = tools.iter().find(|offered| offered.spec.name == spec.name) {
                        tracing::warn!(target: "plugins", "{}: tool {} is {}'s already, left out", plugin.id, spec.name, taken.plugin);
                        continue;
                    }
                    tools.push(Offered {
                        plugin: plugin.id.clone(),
                        title: manifest.title.clone(),
                        spec: spec.clone(),
                        outside: !manifest.hosts().is_empty(),
                    });
                }
                for spec in &manifest.contributes.media {
                    sources.push(PluginMediaSource::new(self.me.clone(), &plugin.id, spec.clone()));
                }
            }
        }
        *self.rules.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(rules);
        *self.tools.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(tools);
        self.media.remove_prefixed("plugin:");
        for source in sources {
            self.media.add(Arc::new(source));
        }
    }

    /// Whether a plugin is on and runs here (no code loaded to tell).
    pub fn is_active(&self, id: &str) -> bool {
        !self.config.disabled && self.plugin(id).is_some_and(|plugin| plugin.active().is_some())
    }

    /// Whether any plugin that is on takes phrases.
    pub fn has_phrases(&self) -> bool {
        !self.rules.read().unwrap_or_else(|poisoned| poisoned.into_inner()).is_empty()
    }

    /// The package in force of a plugin that is on.
    pub fn package(&self, id: &str) -> Option<Arc<Package>> {
        self.plugin(id).and_then(|plugin| plugin.active()).filter(|_| !self.config.disabled)
    }

    /// The installed version of a package of any kind: the newest on disk that reads as one (as of the last scan).
    pub fn installed(&self, id: &str) -> Option<Arc<Package>> {
        self.plugin(id).and_then(|plugin| registry::newest_ready(&plugin.state().found))
    }

    /// Every installed package, by id: the newest version of each that reads as one.
    pub fn installed_all(&self) -> Vec<Arc<Package>> {
        self.list().iter().filter_map(|plugin| registry::newest_ready(&plugin.state().found)).collect()
    }

    /// Where a package came from (catalog, local) and whether the catalog vouched for it.
    pub fn origin(&self, id: &str) -> (String, bool) {
        match self.plugin(id) {
            Some(plugin) => {
                let state = plugin.state();
                (state.source.clone(), state.verified)
            }
            None => (registry::SOURCE_LOCAL.into(), false),
        }
    }

    /// A package's kind by where it came from; one the host does not know is one's own.
    pub fn origin_of(&self, id: &str) -> Origin {
        self.plugin(id).map(|plugin| Origin::of(&plugin.state().source)).unwrap_or(Origin::User)
    }

    /// `panel.js` or `panel.css` of a plugin that is on, of the version in force: what the panel loads.
    pub fn panel_file(&self, id: &str, version: &str, file: &str) -> Option<std::path::PathBuf> {
        if !matches!(file, "panel.js" | "panel.css") {
            return None;
        }
        let package = self.package(id).filter(|package| package.version() == version)?;
        package.manifest.contributes.panel.as_ref()?;
        let path = package.dir.join(file);
        path.is_file().then_some(path)
    }

    /// The panel parts of the plugins that are on: what every panel loads (its module and styles), with the settings
    /// as the panel sees them — a secret one only as «••••••» (whoever sees the station loads these).
    pub fn panel_parts(&self) -> Vec<PanelPart> {
        if self.config.disabled {
            return Vec::new();
        }
        let stored = self.settings.get(&PACKAGE_SETTINGS, None).unwrap_or_default();
        self.list()
            .iter()
            .filter_map(|plugin| {
                let package = plugin.active()?;
                package.manifest.contributes.panel.as_ref()?;
                let part = |file: &str| {
                    package
                        .dir
                        .join(file)
                        .is_file()
                        .then(|| format!("/panel-plugins/{}/{}/{file}", plugin.id, package.version()))
                };
                let module = part("panel.js");
                let styles = part("panel.css");
                (module.is_some() || styles.is_some()).then(|| PanelPart {
                    id: plugin.id.clone(),
                    title: package.manifest.title.clone(),
                    version: package.version(),
                    module,
                    styles,
                    settings: package.manifest.settings_shown(stored.get(&plugin.id)),
                })
            })
            .collect()
    }

    /// A call of a plugin's panel part to its own backend (the SDK's `useApi`): only the functions its manifest lists
    /// in `contributes.panel.calls`, as a tool would be called.
    pub async fn panel_call(&self, id: &str, function: &str, input: Value) -> Result<Value, ApiError> {
        self.writable()?;
        let package = self.package(id).ok_or_else(|| ApiError::conflict(format!("Плагин {id} выключен.")))?;
        let allowed = package
            .manifest
            .contributes
            .panel
            .as_ref()
            .and_then(|panel| panel.get("calls"))
            .and_then(Value::as_array)
            .is_some_and(|calls| calls.iter().any(|call| call.as_str() == Some(function)));
        if !allowed {
            return Err(ApiError::forbidden(format!(
                "Пульт плагина зовёт только функции из contributes.panel.calls, а {function} там нет."
            )));
        }
        self.call(id, function, input, CallKind::Tool)
            .await
            .map_err(|error| ApiError::bad_request(format!("Плагин не ответил: {error}.")))
    }

    /// The themes of the packages installed here (a theme package's, a plugin's that is on), for the panel's choice:
    /// each checked to be a theme in form; the panel fills the tokens a theme leaves out from «Стекло».
    pub fn themes(&self) -> Vec<ThemeView> {
        let mut themes = Vec::new();
        for plugin in self.list() {
            let package = {
                let state = plugin.state();
                registry::newest_ready(&state.found)
            };
            let Some(package) = package else { continue };
            let usable = match package.manifest.kind {
                PackageKind::Theme => true,
                PackageKind::Plugin => self.is_active(&plugin.id),
                _ => false,
            };
            if !usable {
                continue;
            }
            for theme in package
                .manifest
                .contributes
                .themes
                .iter()
                .flat_map(|themes| themes.as_array().cloned().unwrap_or_default())
            {
                match theme_form(&theme) {
                    Ok(()) => themes.push(ThemeView {
                        package: plugin.id.clone(),
                        version: package.version(),
                        origin: Origin::of(&plugin.state().source),
                        theme,
                    }),
                    Err(why) => tracing::warn!(target: "plugins", "{}: a theme left out: {why}", plugin.id),
                }
            }
        }
        themes
    }

    /// The files /media/packages/<id>/ serves: a plugin's while it is on; a character's or a theme's while it is
    /// installed (they are data, nothing of them runs).
    pub fn media_assets(&self, id: &str) -> Option<std::path::PathBuf> {
        if let Some(package) = self.package(id) {
            return Some(package.assets());
        }
        let package = self.installed(id)?;
        matches!(package.manifest.kind, PackageKind::Character | PackageKind::Theme).then(|| package.assets())
    }

    /// The folder of the packages.
    /// The gateway's settings: the workshop keeps its agent's model there.
    pub fn settings(&self) -> &Arc<SettingsService> {
        &self.settings
    }

    pub fn packages_dir(&self) -> &std::path::Path {
        &self.config.packages_dir
    }

    /// What is free of the plugins' memory, as the last look counted it (on the station).
    pub fn free_memory_mb(&self) -> Option<u64> {
        self.gauge.get()
    }

    /// Where the gateway runs now.
    pub fn placement_now(&self) -> Placement {
        (self.placement)()
    }

    /// A package put in by the workshop or by hand: where it came from noted, the folder read again.
    pub fn note_installed(&self, id: &str, source: &str, actor: Option<Actor>) -> Result<(), ApiError> {
        registry::set_installed(&self.db, id, source, false, actor.as_ref().map(|actor| actor.name.as_str()))?;
        self.rescan();
        Ok(())
    }

    /// A package's code stopped and its choice forgotten, before its folder goes: the installer's removal.
    pub async fn forget(&self, id: &str) -> Result<(), ApiError> {
        if let Some(plugin) = self.plugin(id) {
            {
                let mut state = plugin.state();
                state.enabled = false;
                state.active = None;
            }
            self.rebuild();
            let loaded = plugin.release(&mut *plugin.slot.lock().await);
            if let Some(loaded) = loaded {
                loaded.instance.stop();
            }
        }
        registry::forget(&self.db, id)?;
        Ok(())
    }

    /// The installer's news on the bus: installed, updated, removed.
    pub fn announce(&self, id: &str, change: &str, version: Option<String>, actor: Option<Actor>) {
        self.changed(id, change, version, None, actor);
    }

    /// The address the gateway's server listens on: what a starting app is told (src/apps).
    pub fn set_gateway_url(&self, url: String) {
        self.apps_engine.set_gateway_url(url);
    }

    /// What a package that is on may play besides links and its own files: the household's folders of its `content:`
    /// rights, and an app's own sound.
    pub fn play_rights(&self, id: &str) -> super::media::PlayRights {
        let Some(package) = self.package(id) else { return super::media::PlayRights::default() };
        super::media::PlayRights {
            content: package
                .manifest
                .permissions
                .iter()
                .filter_map(|permission| match permission {
                    Permission::Content(folder) => Some(folder.clone()),
                    _ => None,
                })
                .collect(),
            streams: package.manifest.kind == PackageKind::App,
        }
    }

    /// Where the app's own sound at `path` is now (its `/stream/<path>` on localhost), while it is on and runs here:
    /// our /media gives it on to the station at `/media/apps/<id>/<path>` (docs/plans/plugins.md, «Звук приложения на
    /// колонку»). Nothing else of the app's server is reachable that way.
    pub async fn app_stream(&self, id: &str, path: &str) -> Option<String> {
        self.package(id).filter(|package| package.manifest.kind == PackageKind::App)?;
        let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        if parts.is_empty() || parts.iter().any(|part| part.starts_with('.')) {
            return None;
        }
        let plugin = self.plugin(id)?;
        let base = plugin.slot.lock().await.as_ref().and_then(|loaded| loaded.instance.streams())?;
        let path: Vec<String> = parts.iter().map(|part| crate::skills::js::encode_uri_component(part)).collect();
        Some(format!("{base}{}", path.join("/")))
    }

    /// The watch over our player (stage 5а), when the gateway has one: a plugin's long speech plays before its music.
    pub fn playback_watch(&self) -> Option<Arc<PlaybackWatch>> {
        self.watch.clone()
    }

    /// The Host API of a running app, by its token (src/apps/host_http.rs).
    pub fn app_host(&self, token: &str) -> Option<crate::apps::supervisor::AppHost> {
        if token.is_empty() || self.config.disabled {
            return None;
        }
        self.app_tokens.host(token)
    }

    /// An app that fell five times in a row: switched off with the reason, for the panel.
    fn give_up(&self, id: &str, why: &str) {
        let (me, id, why) = (self.me.clone(), id.to_string(), why.to_string());
        tokio::spawn(async move {
            let Some(host) = me.upgrade() else { return };
            if host.disable(&id, None).await.is_ok() {
                if let Some(plugin) = host.plugin(&id) {
                    plugin.state().problem = Some(why.clone());
                    host.note_error(&plugin, &why);
                }
                host.changed(&id, "failed", None, Some(why), None);
            }
        });
    }

    /// A line in a plugin's own journal.
    pub fn note_line(&self, id: &str, level: &str, message: &str) {
        let mut journals = self.journals.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let journal = journals.entry(id.to_string()).or_default();
        if journal.len() >= JOURNAL_LINES {
            journal.pop_front();
        }
        journal.push_back(JournalLine {
            at: now_iso(),
            level: level.to_string(),
            message: message.chars().take(1000).collect(),
        });
    }

    /// A plugin's own journal, oldest first: its console, its calls, what the station said of its items.
    pub fn journal(&self, id: &str) -> Vec<JournalLine> {
        let journals = self.journals.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        journals.get(id).map(|journal| journal.iter().cloned().collect()).unwrap_or_default()
    }

    // Calls.

    /// Calls a plugin's function, its code loaded first if it is not — on the station only when there is room for it
    /// (`admit`). A failure is noted (the panel's last error); one after which its context is not to be trusted (time,
    /// memory) unloads it.
    pub async fn call(&self, id: &str, function: &str, input: Value, kind: CallKind) -> Result<Value, PluginError> {
        if self.config.disabled {
            return Err(PluginError::Unavailable("безопасный режим: плагины не работают (PLUGINS_DISABLED)".into()));
        }
        let plugin = self.plugin(id).ok_or_else(|| PluginError::Unavailable(format!("нет плагина {id}")))?;
        let _busy = Busy::new(&plugin);
        plugin.used.store(self.now_ms(), Ordering::Relaxed);
        let began = Instant::now();
        // «Колонка + сервер»: a plugin that prefers home runs on the computer while it is up; away, here if it fits.
        let away = (self.placement)() == Placement::StationServer
            && plugin.active().is_some_and(|package| package.manifest.prefer_home);
        if away {
            if let Some((url, key)) = self.home.as_ref().and_then(|home| home.up()) {
                match call_home(&url, &key, id, function, &input, kind).await {
                    Ok(answer) => {
                        self.note_line(
                            id,
                            "call",
                            &format!("{function}: на компьютере, {} мс", began.elapsed().as_millis()),
                        );
                        let mut stats = plugin.stats();
                        stats.calls += 1;
                        stats.last_call_at = Some(now_iso());
                        drop(stats);
                        // What it plays from the computer — an app's sound, the household's files — is played from
                        // the computer's /media.
                        return Ok(super::media::from_home(answer, &url, id, &self.play_rights(id)));
                    }
                    Err(why) => self.note_line(id, "warn", &format!("{function}: {why} — работает на колонке")),
                }
            }
        }
        let result = match self.instance(&plugin).await {
            Ok(instance) => instance.call(function, input, kind).await,
            Err(PluginError::NoRoom(why)) if away => {
                Err(PluginError::NoRoom(format!("недоступно, пока компьютер не на связи: {why}")))
            }
            Err(error) => Err(error),
        };
        let how = match kind {
            CallKind::Voice => "фраза",
            CallKind::Tool => "инструмент",
            CallKind::Background => "в фоне",
        };
        match &result {
            Ok(_) => self.note_line(
                id,
                "call",
                &format!("{function} ({how}): ответил за {} мс", began.elapsed().as_millis()),
            ),
            Err(error) => self.note_line(id, "error", &format!("{function} ({how}): {error}")),
        }
        plugin.used.store(self.now_ms(), Ordering::Relaxed);
        {
            let mut stats = plugin.stats();
            stats.calls += 1;
            stats.last_call_at = Some(now_iso());
        }
        if let Err(error) = &result {
            let message = format!("{function}: {error}");
            tracing::warn!(target: "plugins", "{id}: {message}");
            self.note_error(&plugin, &message);
            if error.poisons() {
                self.unload(&plugin).await;
            }
        }
        result
    }

    fn note_error(&self, plugin: &Plugin, message: &str) {
        let mut stats = plugin.stats();
        stats.errors += 1;
        stats.last_error = Some(LastError { at: now_iso(), message: message.chars().take(500).collect() });
    }

    /// The code of a version, started.
    async fn start_package(&self, package: &Package) -> Result<Arc<dyn PluginInstance>, PluginError> {
        let manifest = package.manifest.clone();
        let (runtime, code) = super::runtime::engine_for(&manifest, package, &self.runtime, &self.apps)?;
        let me = self.me.clone();
        let host = Arc::new(
            PluginHostApi::new(
                manifest,
                self.db.clone(),
                self.settings.clone(),
                self.handle.clone(),
                self.speaker.clone(),
            )
            .with_services(self.services.clone())
            .with_data_news(self.data_news.clone())
            .with_sink(Arc::new(move |id: &str, level: &str, message: &str| {
                if let Some(host) = me.upgrade() {
                    host.note_line(id, level, message);
                }
            })),
        );
        let started = Instant::now();
        let instance = runtime.start(code, host).await?;
        tracing::info!(target: "plugins", "{} {}: loaded in {} ms", package.manifest.id, package.version(), started.elapsed().as_millis());
        Ok(instance)
    }

    /// The plugin's code, loaded now if it is not and there is room for it (`admit`). A version that does not start
    /// gives way to the one that last did, when there is one.
    async fn instance(&self, plugin: &Arc<Plugin>) -> Result<Arc<dyn PluginInstance>, PluginError> {
        let mut slot = plugin.slot.lock().await;
        let (package, good) = {
            let state = plugin.state();
            (state.active.clone().filter(|_| state.enabled), state.good.clone())
        };
        let package = package.ok_or_else(|| PluginError::Unavailable("плагин выключен".into()))?;
        let version = package.version();
        if let Some(loaded) = slot.as_ref() {
            if loaded.version == version {
                return Ok(loaded.instance.clone());
            }
        }
        if let Some(loaded) = plugin.release(&mut slot) {
            loaded.instance.stop();
        }
        let loading = self.admit(plugin, &package.manifest).await?;
        match self.start_package(&package).await {
            Ok(instance) => {
                let share_mb = share_of(&package.manifest);
                plugin.hold(&mut slot, Loaded { version: version.clone(), instance: instance.clone(), share_mb });
                drop((slot, loading));
                if good.as_deref() != Some(version.as_str()) {
                    let _ = registry::set_good(&self.db, &plugin.id, &version);
                    plugin.state().good = Some(version.clone());
                }
                self.changed(&plugin.id, "loaded", Some(version), None, None);
                Ok(instance)
            }
            Err(error) => {
                drop(loading);
                let previous = good.filter(|good| *good != version).and_then(|good| {
                    plugin.state().found.iter().find_map(|candidate| match candidate {
                        Candidate::Ready(package) if package.version() == good => Some(package.clone()),
                        _ => None,
                    })
                });
                let message = format!("версия {version} не поднялась: {error}");
                if let Some(previous) =
                    previous.filter(|previous| previous.manifest.incompatible_with((self.placement)()).is_none())
                {
                    // Room for it as for any load: without room the failure stands.
                    let loading = self.admit(plugin, &previous.manifest).await.ok();
                    let started = if loading.is_some() { self.start_package(&previous).await.ok() } else { None };
                    if let Some(instance) = started {
                        let message = format!("{message}; работает прежняя {}", previous.version());
                        let share_mb = share_of(&previous.manifest);
                        plugin.hold(
                            &mut slot,
                            Loaded { version: previous.version(), instance: instance.clone(), share_mb },
                        );
                        drop((slot, loading));
                        {
                            let mut state = plugin.state();
                            state.active = Some(previous.clone());
                            state.problem = Some(message.clone());
                        }
                        tracing::warn!(target: "plugins", "{}: {message}", plugin.id);
                        self.note_error(plugin, &message);
                        self.rebuild();
                        self.changed(&plugin.id, "failed", Some(previous.version()), Some(message), None);
                        return Ok(instance);
                    }
                }
                tracing::warn!(target: "plugins", "{}: {message}", plugin.id);
                self.changed(&plugin.id, "failed", Some(version), Some(message), None);
                Err(error)
            }
        }
    }

    async fn unload(&self, plugin: &Plugin) {
        let loaded = plugin.release(&mut *plugin.slot.lock().await);
        if let Some(loaded) = loaded {
            loaded.instance.stop();
            self.changed(&plugin.id, "unloaded", Some(loaded.version), None, None);
        }
    }

    /// Unloads the code of plugins idle longer than PLUGINS_IDLE_UNLOAD_S; one in a call is not idle.
    fn unload_idle(&self) {
        self.unload_idle_for(self.config.idle_unload_s * 1000);
    }

    /// Unloads the code of plugins idle longer than `idle` ms; one in a call stays.
    fn unload_idle_for(&self, idle: u64) {
        let now = self.now_ms();
        for plugin in self.list() {
            let idle_for = now.saturating_sub(plugin.used.load(Ordering::Relaxed));
            if idle_for < idle || plugin.busy.load(Ordering::Relaxed) > 0 {
                continue;
            }
            let Ok(mut slot) = plugin.slot.try_lock() else { continue };
            if let Some(loaded) = plugin.release(&mut slot) {
                drop(slot);
                loaded.instance.stop();
                tracing::info!(target: "plugins", "{}: unloaded after {} s without a call", plugin.id, idle / 1000);
                self.changed(&plugin.id, "unloaded", Some(loaded.version), None, None);
            }
        }
    }

    /// An event of the bus to the plugins that follow it: each in the background, not waited for.
    fn deliver(&self, event: &GatewayEvent) {
        if self.config.disabled {
            return;
        }
        let name = event.name();
        for plugin in self.list() {
            let Some(package) = plugin.active() else { continue };
            if !package.manifest.has(&Permission::Events) {
                continue;
            }
            for spec in package.manifest.contributes.events.iter().filter(|spec| spec.name == name) {
                let payload = json!({ "name": name, "payload": event.payload() });
                self.background(&plugin.id, &spec.handler, payload);
            }
        }
    }

    /// What the station said of a plugin's own item (stage 5а), to that plugin when it follows the event; a silence
    /// or a failure it may answer with what to play instead (`{play: […]}`), which plays at once if the person has not
    /// gone on meanwhile.
    pub fn player_news(&self, report: &PlaybackReport) {
        if self.config.disabled {
            return;
        }
        let Some((plugin, own)) = report.item.id.split_once('/') else { return };
        let Some(package) = self.package(plugin) else { return };
        self.note_line(plugin, "player", &report.news.note(&report.item.title, report.via_gateway));
        if !package.manifest.has(&Permission::Events) {
            return;
        }
        let name = report.news.event();
        let Some(spec) = package.manifest.contributes.events.iter().find(|spec| spec.name == name) else { return };
        let (seconds, retrying) = match report.news {
            PlaybackNews::Silent { seconds, retrying } => (Some(seconds), retrying),
            _ => (None, false),
        };
        let payload = json!({
            "name": name,
            "payload": {
                "device": report.device,
                "item": { "id": own, "title": report.item.title, "url": report.item.url },
                "viaGateway": report.via_gateway,
                "seconds": seconds,
                // The gateway tries its own way next: nothing to answer yet.
                "retrying": retrying,
            }
        });
        let answerable = matches!(report.news, PlaybackNews::Failed | PlaybackNews::Silent { retrying: false, .. });
        let kind = package.manifest.contributes.media.first().map(|media| media.kind).unwrap_or_else(|| {
            super::media::kind_for(
                package.manifest.contributes.intents.first().map(|i| i.capability).unwrap_or(CapabilityName::Plugins),
            )
        });
        let (me, id, handler) = (self.me.clone(), plugin.to_string(), spec.handler.clone());
        let (device, play_id) = (report.device.clone(), report.play_id.clone());
        tokio::spawn(async move {
            let Some(host) = me.upgrade() else { return };
            let answer = host.call(&id, &handler, payload, CallKind::Background).await;
            let (Ok(Value::Object(answer)), true) = (answer, answerable) else { return };
            let Some(play) = answer.get("play") else { return };
            let base = host.player.as_ref().and_then(|player| player.media_base(&device));
            let items = super::media::media_items_with(&id, kind, play, base.as_deref(), &host.play_rights(&id));
            if let (Some(watch), false) = (host.watch.as_ref(), items.is_empty()) {
                if watch.replace(&device, &play_id, items) {
                    tracing::info!(target: "plugins", "{id}: the station was silent, the plugin chose what to play instead");
                }
            }
        });
    }

    /// A call in the background: now, or once the pressure eases.
    fn background(&self, id: &str, handler: &str, payload: Value) {
        if self.deferring.load(Ordering::Relaxed) {
            let mut deferred = self.deferred.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if deferred.len() >= budget::DEFERRED_MAX {
                deferred.pop_front();
            }
            deferred.push_back((id.to_string(), handler.to_string(), payload));
            return;
        }
        let (me, id, handler) = (self.me.clone(), id.to_string(), handler.to_string());
        tokio::spawn(async move {
            if let Some(host) = me.upgrade() {
                let _ = host.call(&id, &handler, payload, CallKind::Background).await;
            }
        });
    }

    // Memory.

    /// The machine's memory now, read off the runtime's threads (a Mac asks `sysctl`).
    async fn memory_now(&self) -> Option<MemoryNow> {
        let probe = self.memory.clone();
        tokio::task::spawn_blocking(move || probe.now()).await.ok().flatten()
    }

    /// A plugin's share now, from what the host keeps — no engine is asked: what it declared, what it takes of the
    /// budget (while its context is loaded or loading), whether it is in a call, what its engine held at the last
    /// look. None for a package of another kind: an app is counted by what its process holds, the rest run nothing.
    fn share_now(&self, plugin: &Plugin) -> Option<PluginShare> {
        let (enabled, manifest) = {
            let state = plugin.state();
            let shown = state.active.clone().or_else(|| registry::newest_ready(&state.found));
            (state.enabled && state.active.is_some(), shown.map(|package| package.manifest.clone()))
        };
        let manifest = manifest.filter(|manifest| manifest.kind == PackageKind::Plugin)?;
        let loading_mb = plugin.loading_mb.load(Ordering::Relaxed);
        let taken_mb = plugin.taken_mb.load(Ordering::Relaxed) + loading_mb;
        let held_mb = plugin.held_mb.load(Ordering::Relaxed);
        Some(PluginShare {
            id: plugin.id.clone(),
            title: manifest.title.clone(),
            enabled: enabled && !self.config.disabled,
            loaded: taken_mb > 0,
            busy: loading_mb > 0 || plugin.busy.load(Ordering::Relaxed) > 0,
            budget_mb: manifest.budget.memory_mb,
            taken_mb,
            actual_mb: (taken_mb > 0 && held_mb > 0).then_some(held_mb),
        })
    }

    /// Each plugin's share, its engine asked first what it holds now (for the panel and the memory watch).
    async fn shares(&self) -> Vec<PluginShare> {
        let plugins = self.list();
        for plugin in &plugins {
            let instance = plugin.slot.lock().await.as_ref().map(|loaded| loaded.instance.clone());
            let Some(instance) = instance else { continue };
            if let Some(memory) = instance.memory().await {
                plugin.held_mb.store(memory.allocated.div_ceil(1_048_576), Ordering::Relaxed);
            }
        }
        plugins.iter().filter_map(|plugin| self.share_now(plugin)).collect()
    }

    /// The plugins' memory and room, for the panel.
    pub async fn resources(&self) -> ResourcesView {
        let memory = self.memory_now().await;
        let plugins = self.shares().await;
        let count = budget::count(memory, self.config.reserve_mb, &plugins);
        let placement = (self.placement)();
        let pressure = memory.map(|memory| memory.pressure).unwrap_or(Pressure::Normal);
        self.gauge.set(memory.map(|_| count.room_mb));
        let dir = self.config.packages_dir.clone();
        let probe = self.memory.clone();
        let (free_disk, packages_mb) = tokio::task::spawn_blocking(move || {
            (probe.free_disk_mb(&dir), crate::operations::system_memory::folder_mb(&dir))
        })
        .await
        .unwrap_or((None, 0));
        ResourcesView {
            placement,
            enforced: placement.on_station(),
            measured: memory.is_some(),
            total_mb: memory.map(|memory| memory.total_mb).unwrap_or(0),
            available_mb: memory.map(|memory| memory.available_mb).unwrap_or(0),
            reserve_mb: self.config.reserve_mb,
            gateway_mb: memory.and_then(|memory| memory.gateway_mb),
            plugins_mb: count.plugins_mb,
            budget_mb: count.budget_mb,
            committed_mb: count.committed_mb,
            free_mb: count.free_mb,
            room_mb: count.room_mb,
            loaded: count.loaded,
            pressure,
            deferring: self.deferring.load(Ordering::Relaxed),
            action: self.config.pressure_action,
            disk: budget::DiskView { free_mb: free_disk, reserve_mb: self.config.storage_reserve_mb, packages_mb },
            note: budget::note(&count, memory.is_some(), pressure),
            plugins,
        }
    }

    /// Room for a context about to load (budget.rs): on the station its declared budget must fit what is free now. If
    /// it does not, idle contexts unload first, the longest idle first, one in a call never; if it still does not, the
    /// load is refused with what it needs and what is free. On a computer, or while the machine does not tell its
    /// memory, nothing is refused. The room is taken until the context is in its slot (`Loading`); the context's own
    /// share, if an older version of it is loaded, is left out — the new one takes its place.
    async fn admit<'a>(&self, plugin: &'a Plugin, manifest: &Manifest) -> Result<Loading<'a>, PluginError> {
        let memory = if (self.placement)().on_station() { self.memory_now().await } else { None };
        // Counted and taken at once, nothing awaited meanwhile.
        let _admitting = self.admitting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(memory) = memory {
            self.make_room(plugin, manifest.budget.memory_mb, memory)?;
        }
        Ok(Loading::take(plugin, share_of(manifest)))
    }

    /// The count for a load of `need_mb` and the idle contexts it unloads, if it must; why not when there is no room.
    fn make_room(&self, plugin: &Plugin, need_mb: u64, memory: MemoryNow) -> Result<(), PluginError> {
        let others: Vec<Arc<Plugin>> = self.list().into_iter().filter(|other| other.id != plugin.id).collect();
        let mut shares: Vec<PluginShare> = others.iter().filter_map(|other| self.share_now(other)).collect();
        // What its own loaded version holds comes back to the budget, as any loaded context's does; its share is not
        // counted twice.
        if let Some(own) = self.share_now(plugin) {
            shares.push(PluginShare { taken_mb: 0, busy: false, ..own });
        }
        let count = budget::count(Some(memory), self.config.reserve_mb, &shares);
        let now = self.now_ms();
        let idle: Vec<Idle> = others
            .iter()
            .filter_map(|other| {
                let share = shares.iter().find(|share| share.id == other.id)?;
                (share.loaded && !share.busy).then(|| Idle {
                    id: other.id.clone(),
                    taken_mb: share.taken_mb,
                    idle_ms: now.saturating_sub(other.used.load(Ordering::Relaxed)),
                })
            })
            .collect();
        let refused = |free_mb: u64| {
            let why = budget::does_not_fit(need_mb, free_mb).unwrap_or_default();
            tracing::warn!(target: "plugins", "{}: not loaded, no room — {need_mb} MB needed, {free_mb} MB free (the budget {} MB, {} MB taken by the loaded)", plugin.id, count.budget_mb, count.committed_mb);
            PluginError::NoRoom(why)
        };
        let chosen = budget::to_unload(need_mb, count.free_mb, &idle).map_err(refused)?;
        let mut free_mb = count.free_mb;
        for other in chosen.iter().filter_map(|id| others.iter().find(|other| other.id == *id)) {
            free_mb += self.unload_for(other, &plugin.id).unwrap_or(0);
        }
        // One that went into a call meanwhile stayed loaded.
        if need_mb > free_mb {
            return Err(refused(free_mb));
        }
        Ok(())
    }

    /// An idle context unloaded to make room for `for_id`'s: the share it gave back, or none when it went into a call
    /// (or is loading) meanwhile.
    fn unload_for(&self, plugin: &Plugin, for_id: &str) -> Option<u64> {
        let Ok(mut slot) = plugin.slot.try_lock() else { return None };
        if plugin.busy.load(Ordering::Relaxed) > 0 {
            return None;
        }
        let loaded = plugin.release(&mut slot)?;
        drop(slot);
        loaded.instance.stop();
        tracing::info!(target: "plugins", "{}: unloaded to make room for {for_id}", plugin.id);
        self.note_line(&plugin.id, "load", &format!("выгружен: память понадобилась плагину {for_id}"));
        self.changed(&plugin.id, "unloaded", Some(loaded.version), None, None);
        Some(loaded.share_mb)
    }

    /// Whether a package of `need_mb` fits on the disk, with the reserve for the gateway's update; on the station only.
    pub fn room_for(&self, need_mb: u64) -> Result<(), String> {
        if !(self.placement)().on_station() {
            return Ok(());
        }
        let free = self.memory.free_disk_mb(&self.config.packages_dir);
        match budget::no_room(need_mb, free, self.config.storage_reserve_mb) {
            Some(why) => Err(why),
            None => Ok(()),
        }
    }

    /// One look at the memory: under pressure the idle plugins give their memory back and their background work
    /// waits; at «critical» on the station the app, else the plugin, switched on last among those loaded goes off (or
    /// the panel is told, if the owner chose `warn`). The built-in skills and the voice path are never touched.
    async fn watch_memory(&self) {
        let Some(memory) = self.memory_now().await else {
            self.gauge.set(None);
            return;
        };
        let low = memory.available_mb < self.config.reserve_mb || memory.pressure >= Pressure::Warn;
        if low {
            self.unload_idle_for(budget::PRESSED_IDLE_MS);
        }
        let was = self.deferring.swap(low, Ordering::Relaxed);
        if was && !low {
            let waiting: Vec<(String, String, Value)> =
                self.deferred.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).drain(..).collect();
            if !waiting.is_empty() {
                tracing::info!(target: "plugins", "the memory pressure eased: {} background call(s) go now", waiting.len());
            }
            for (id, handler, payload) in waiting {
                self.background(&id, &handler, payload);
            }
        } else if low && !was {
            tracing::warn!(target: "plugins", "memory is short ({} MB available, the reserve {} MB, pressure {}): idle plugins unload, background work waits", memory.available_mb, self.config.reserve_mb, memory.pressure.as_str());
        }
        let critical = memory.pressure == Pressure::Critical || memory.available_mb < self.config.reserve_mb / 2;
        if critical && (self.placement)().on_station() && !self.config.disabled {
            self.relieve(memory).await;
        }
        let plugins = self.shares().await;
        let count = budget::count(Some(memory), self.config.reserve_mb, &plugins);
        // What a plugin could get, the idle contexts unloaded: what «would it go here» is asked against.
        self.gauge.set(Some(count.room_mb));
        let now = (count.free_mb, memory.pressure);
        let told = {
            let mut told = self.told.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let changed = told.is_none_or(|(free, pressure)| pressure != now.1 || free.abs_diff(now.0) >= 2);
            if changed {
                *told = Some(now);
            }
            changed
        };
        if told {
            self.events.emit(GatewayEvent::PluginsResources {
                free_mb: count.free_mb,
                pressure: memory.pressure.as_str().into(),
            });
        }
    }

    /// At «critical» on the station: the app switched on last goes off — an app weighs more than a plugin and starts
    /// again as easily (docs/plans/plugins.md, «Надзор за приложениями») — else the plugin switched on last; or the
    /// log and the panel are told. Only one that is loaded: one that holds nothing frees nothing by going off.
    async fn relieve(&self, memory: MemoryNow) {
        let mut order: Vec<String> = Vec::new();
        for id in registry::enabled_order(&self.db) {
            if !(self.is_active(&id) && self.may_switch_off(&id)) {
                continue;
            }
            let Some(plugin) = self.plugin(&id) else { continue };
            if plugin.slot.lock().await.is_some() {
                order.push(id);
            }
        }
        let is_app = |id: &&String| self.package(id).is_some_and(|package| package.manifest.kind == PackageKind::App);
        let Some(id) = order.iter().find(is_app).or_else(|| order.first()).cloned() else { return };
        let why = format!(
            "колонке не хватало памяти: свободно {} МБ при запасе {} МБ, давление {}",
            memory.available_mb,
            self.config.reserve_mb,
            memory.pressure.as_str()
        );
        match self.config.pressure_action {
            PressureAction::Warn => {
                tracing::warn!(target: "plugins", "{id}: {why}; PLUGINS_PRESSURE_ACTION=warn, it stays on");
                self.changed(&id, "pressure", None, Some(why), None);
            }
            PressureAction::Disable => {
                tracing::warn!(target: "plugins", "{id}: switched off — {why}");
                if self.disable(&id, None).await.is_ok() {
                    if let Some(plugin) = self.plugin(&id) {
                        plugin.state().problem = Some(format!("выключен сам: {why}"));
                        self.note_error(&plugin, &format!("выключен сам: {why}"));
                    }
                    self.changed(&id, "pressure", None, Some(format!("выключен: {why}")), None);
                }
            }
        }
    }

    /// Whether a plugin's calls go to the home server now: it prefers home, the station has one, and it is up.
    fn runs_at_home(&self, package: &Package) -> bool {
        package.manifest.prefer_home
            && (self.placement)() == Placement::StationServer
            && self.home.as_ref().and_then(|home| home.up()).is_some()
    }

    /// Whether the host may switch a plugin off by itself: not a system package.
    fn may_switch_off(&self, id: &str) -> bool {
        self.origin_of(id).can_disable()
    }

    /// «Сделать свою копию»: a built-in's or a catalog's package copied under an id of one's own (`radio` → `my-radio`),
    /// the original switched off, the copy on in its place when the original was. The copy is one's own: it changes in
    /// the workshop, and an update of the gateway or the catalog leaves it as it is. Its settings come along.
    pub async fn fork(&self, id: &str, new_id: Option<&str>, actor: Option<Actor>) -> Result<PackageView, ApiError> {
        self.writable()?;
        self.rescan();
        let plugin = self.known(id)?;
        let origin = Origin::of(&plugin.state().source);
        if origin == Origin::System {
            return Err(ApiError::conflict("Системный пакет не копируют: он часть шлюза."));
        }
        let package = registry::newest_ready(&plugin.state().found)
            .ok_or_else(|| ApiError::bad_request(format!("У {id} нет версии, которая читается.")))?;
        let copy_id = match new_id.map(str::trim).filter(|new_id| !new_id.is_empty()) {
            Some(new_id) => new_id.to_string(),
            None => format!("my-{id}").chars().take(32).collect::<String>().trim_end_matches('-').to_string(),
        };
        if !manifest::valid_id(&copy_id) {
            return Err(ApiError::field("id", "строчные латинские буквы, цифры и «-», с буквы, до 32 знаков"));
        }
        if self.config.packages_dir.join(&copy_id).exists() || self.plugin(&copy_id).is_some() {
            return Err(ApiError::conflict(format!("Пакет {copy_id} уже есть — назовите копию иначе.")));
        }
        let version = package.version();
        let target = self.config.packages_dir.join(&copy_id).join(&version);
        let title = format!("{} (моя копия)", package.manifest.title).chars().take(80).collect::<String>();
        let (from, to, new) = (package.dir.clone(), target.clone(), copy_id.clone());
        tokio::task::spawn_blocking(move || copy_package(&from, &to, &new, &title))
            .await
            .map_err(ApiError::internal)?
            .map_err(|error| ApiError::internal(format!("копия не записалась: {error}")))?;
        registry::set_installed(
            &self.db,
            &copy_id,
            registry::SOURCE_LOCAL,
            false,
            actor.as_ref().map(|a| a.name.as_str()),
        )?;
        // Its settings come along.
        let mut stored = self.settings.get(&PACKAGE_SETTINGS, None).unwrap_or_default();
        if let Some(own) = stored.get(id).cloned() {
            stored.insert(copy_id.clone(), own);
            self.settings.set(&PACKAGE_SETTINGS, &stored, actor.clone())?;
        }
        let was_on = plugin.state().enabled;
        if was_on && origin.can_disable() {
            self.disable(id, actor.clone()).await?;
        }
        self.rescan();
        tracing::info!(target: "plugins", "{id}: copied as {copy_id} {version}");
        self.changed(&copy_id, "installed", Some(version), None, actor.clone());
        if was_on {
            if let Err(error) = self.enable(&copy_id, actor).await {
                tracing::warn!(target: "plugins", "{copy_id}: the copy stays off: {}", error.message);
            }
        }
        self.view_one(&copy_id).await
    }

    fn changed(&self, id: &str, change: &str, version: Option<String>, error: Option<String>, actor: Option<Actor>) {
        self.events.emit(GatewayEvent::PackagesChanged {
            change: change.into(),
            package_id: id.into(),
            version,
            error,
            actor,
        });
    }

    // The admin's actions.

    fn writable(&self) -> Result<(), ApiError> {
        if self.config.disabled {
            return Err(ApiError::conflict("Безопасный режим: плагины выключены (PLUGINS_DISABLED)."));
        }
        Ok(())
    }

    fn known(&self, id: &str) -> Result<Arc<Plugin>, ApiError> {
        self.plugin(id).ok_or_else(|| ApiError::not_found(format!("Нет пакета {id}.")))
    }

    /// A tool of this package already given by another plugin that is on.
    fn tool_taken(&self, id: &str, package: &Package) -> Option<String> {
        let tools = self.tools.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        package.manifest.contributes.tools.iter().find_map(|spec| {
            tools
                .iter()
                .find(|offered| offered.spec.name == spec.name && offered.plugin != id)
                .map(|offered| format!("инструмент {} уже даёт плагин {}", spec.name, offered.plugin))
        })
    }

    /// Switches a plugin on: its newest version that runs here starts first; what does not start stays off. Memory
    /// never keeps it off: with no room to start it now (budget.rs) it is on unstarted, and loads at its first call.
    pub async fn enable(&self, id: &str, actor: Option<Actor>) -> Result<PackageView, ApiError> {
        self.writable()?;
        self.rescan();
        let plugin = self.known(id)?;
        let (package, problem) = choose(&plugin.state().found, (self.placement)());
        let Some(package) = package else {
            return Err(ApiError::bad_request(format!("Плагин не включить: {}.", problem.unwrap_or_default())));
        };
        if let Some(taken) = self.tool_taken(id, &package) {
            return Err(ApiError::conflict(format!("Плагин не включить: {taken}.")));
        }
        // «Колонка + сервер»: a plugin that prefers home runs on the computer while it is up — nothing of it starts
        // on the station, and the station's memory is not asked.
        let at_home = self.runs_at_home(&package);
        let version = package.version();
        // Whether it is known to start: it started now, or it runs on the computer.
        let mut started = at_home;
        if !at_home {
            let mut slot = plugin.slot.lock().await;
            if let Some(loaded) = plugin.release(&mut slot) {
                loaded.instance.stop();
            }
            match self.admit(&plugin, &package.manifest).await {
                Ok(loading) => {
                    let instance = self.start_package(&package).await.map_err(|error| {
                        self.note_error(&plugin, &format!("не запустился: {error}"));
                        ApiError::bad_request(format!("Плагин не запустился, он остаётся выключенным: {error}."))
                    })?;
                    let share_mb = share_of(&package.manifest);
                    plugin.hold(&mut slot, Loaded { version: version.clone(), instance, share_mb });
                    drop(loading);
                    started = true;
                }
                Err(why) => {
                    tracing::info!(target: "plugins", "{id} {version}: on, not started — {why}");
                    self.note_line(
                        id,
                        "load",
                        &format!(
                            "включён, но пока не загружен: {why}. Загрузится при первом вызове, когда хватит памяти"
                        ),
                    );
                }
            }
        }
        plugin.used.store(self.now_ms(), Ordering::Relaxed);
        let good = started.then_some(version.as_str());
        registry::set_enabled(&self.db, id, true, good, actor.as_ref().map(|actor| actor.name.as_str()))?;
        {
            let mut state = plugin.state();
            state.enabled = true;
            if started {
                state.good = Some(version.clone());
            }
            state.active = Some(package);
            state.problem = problem;
        }
        self.rebuild();
        tracing::info!(target: "plugins", "{id} {version}: switched on");
        self.changed(id, "enabled", Some(version), None, actor);
        Ok(self.view_of(&plugin).await)
    }

    pub async fn disable(&self, id: &str, actor: Option<Actor>) -> Result<PackageView, ApiError> {
        self.writable()?;
        let plugin = self.known(id)?;
        if !Origin::of(&plugin.state().source).can_disable() {
            return Err(ApiError::conflict(format!(
                "«{id}» — системный пакет: без него колонка не работает как надо, его не выключить."
            )));
        }
        registry::set_enabled(&self.db, id, false, None, actor.as_ref().map(|actor| actor.name.as_str()))?;
        {
            let mut state = plugin.state();
            state.enabled = false;
            state.active = None;
        }
        self.rebuild();
        let loaded = plugin.release(&mut *plugin.slot.lock().await);
        if let Some(loaded) = loaded {
            loaded.instance.stop();
        }
        tracing::info!(target: "plugins", "{id}: switched off");
        self.changed(id, "disabled", None, None, actor);
        Ok(self.view_of(&plugin).await)
    }

    /// Starts the newest version on disk beside the running one: it takes over if it starts; if not, the running
    /// one stays and the answer says why.
    pub async fn reload(&self, id: &str, actor: Option<Actor>) -> Result<PackageView, ApiError> {
        self.writable()?;
        self.rescan();
        let plugin = self.known(id)?;
        let (enabled, running, found) = {
            let state = plugin.state();
            (state.enabled, state.active.as_ref().map(|active| active.version()), state.found.clone())
        };
        if !enabled {
            // Nothing runs: the folder was read again, which is all a reload of a plugin that is off does.
            return Ok(self.view_of(&plugin).await);
        }
        let (package, problem) = choose(&found, (self.placement)());
        let Some(package) = package else {
            let message = format!("Новой версии нет: {}.", problem.unwrap_or_default());
            return Err(ApiError::bad_request(message));
        };
        if let Some(taken) = self.tool_taken(id, &package) {
            return Err(ApiError::conflict(format!("Новая версия не включена: {taken}.")));
        }
        let version = package.version();
        let running = running.unwrap_or_else(|| "—".into());
        let loading = self.admit(&plugin, &package.manifest).await.map_err(|why| {
            ApiError::conflict(format!("Новая версия {version} не загрузилась: {why}. Работает прежняя {running}."))
        })?;
        match self.start_package(&package).await {
            Ok(instance) => {
                let loaded = Loaded { version: version.clone(), instance, share_mb: share_of(&package.manifest) };
                let old = plugin.hold(&mut *plugin.slot.lock().await, loaded);
                drop(loading);
                if let Some(old) = old {
                    old.instance.stop();
                }
                plugin.used.store(self.now_ms(), Ordering::Relaxed);
                let _ = registry::set_good(&self.db, id, &version);
                {
                    let mut state = plugin.state();
                    state.good = Some(version.clone());
                    state.active = Some(package);
                    state.problem = problem;
                }
                self.rebuild();
                tracing::info!(target: "plugins", "{id}: reloaded, {version} runs");
                self.changed(id, "reloaded", Some(version), None, actor);
                Ok(self.view_of(&plugin).await)
            }
            Err(error) => {
                let message = format!("версия {version} не поднялась: {error}; работает прежняя {running}");
                tracing::warn!(target: "plugins", "{id}: {message}");
                self.note_error(&plugin, &message);
                self.changed(id, "failed", Some(version.clone()), Some(message.clone()), actor);
                Err(ApiError::bad_request(format!(
                    "Новая версия {version} не поднялась: {error}. Работает прежняя {running}."
                )))
            }
        }
    }

    /// Back to the version before (a catalog's update or one's own install that did not go well): the newest version's
    /// folder goes, the one before takes over. Only what may be removed: a built-in goes with the gateway's version.
    pub async fn rollback(&self, id: &str, actor: Option<Actor>) -> Result<PackageView, ApiError> {
        self.writable()?;
        self.rescan();
        let plugin = self.known(id)?;
        let origin = Origin::of(&plugin.state().source);
        if !origin.removable() {
            return Err(ApiError::conflict(format!("«{id}» — {} пакет: его версия — версия шлюза.", origin.title())));
        }
        let ready: Vec<Arc<Package>> = plugin
            .state()
            .found
            .iter()
            .filter_map(|candidate| match candidate {
                Candidate::Ready(package) => Some(package.clone()),
                Candidate::Broken { .. } => None,
            })
            .collect();
        let [newest, before, ..] = ready.as_slice() else {
            return Err(ApiError::conflict("Откатываться не на что: версия одна."));
        };
        let (gone, back) = (newest.version(), before.version());
        let dir = newest.dir.clone();
        tokio::task::spawn_blocking(move || std::fs::remove_dir_all(dir))
            .await
            .map_err(ApiError::internal)?
            .map_err(ApiError::internal)?;
        if plugin.state().good.as_deref() == Some(gone.as_str()) {
            let _ = registry::set_good(&self.db, id, &back);
            plugin.state().good = Some(back.clone());
        }
        {
            let mut state = plugin.state();
            if state.active.as_ref().is_some_and(|active| active.version() == gone) {
                state.active = None;
            }
        }
        self.rescan();
        if plugin.state().enabled {
            self.reload(id, actor.clone()).await?;
        }
        tracing::info!(target: "plugins", "{id}: rolled back from {gone} to {back}");
        self.note_line(id, "load", &format!("откат: {gone} → {back}"));
        self.changed(id, "rolledBack", Some(back), None, actor);
        self.view_one(id).await
    }

    /// The admin's values for the plugin's settings: each must be one its manifest declares, of its type. A secret one
    /// that comes back as the panel was shown it («••••••») stays as it is stored.
    pub async fn set_settings(
        &self,
        id: &str,
        values: &Map<String, Value>,
        actor: Option<Actor>,
    ) -> Result<PackageView, ApiError> {
        let plugin = self.known(id)?;
        let package = {
            let state = plugin.state();
            state.active.clone().or_else(|| choose(&state.found, (self.placement)()).0)
        };
        let package = package.ok_or_else(|| ApiError::bad_request("У плагина нет версии, которая работает здесь."))?;
        let specs = &package.manifest.settings;
        let mut stored = self.settings.get(&PACKAGE_SETTINGS, None).unwrap_or_default();
        let mut own = stored.remove(id).unwrap_or_default();
        for (key, value) in values {
            let spec = specs
                .iter()
                .find(|spec| &spec.key == key)
                .ok_or_else(|| ApiError::field(key, "нет такой настройки"))?;
            if spec.secret && value.as_str() == Some(manifest::SECRET_SHOWN) {
                continue;
            }
            if value.is_null() {
                own.remove(key);
                continue;
            }
            if !spec.accepts(value) {
                return Err(ApiError::field(key, "значение не подходит к настройке"));
            }
            own.insert(key.clone(), value.clone());
        }
        stored.insert(id.to_string(), own);
        self.settings.set(&PACKAGE_SETTINGS, &stored, actor.clone())?;
        self.changed(id, "settings", Some(package.version()), None, actor);
        Ok(self.view_of(&plugin).await)
    }

    /// Runs a plugin's tool as «Мозг» would, for the panel's check: its answer as the model reads it.
    pub async fn try_tool(&self, id: &str, name: &str, input: Value) -> Result<String, ApiError> {
        self.writable()?;
        let package = self.package(id).ok_or_else(|| ApiError::conflict(format!("Плагин {id} выключен.")))?;
        let spec = package
            .manifest
            .contributes
            .tools
            .iter()
            .find(|spec| spec.name == name)
            .ok_or_else(|| ApiError::not_found(format!("Нет инструмента {name}.")))?;
        let answer = self
            .call(id, &spec.handler, input, CallKind::Tool)
            .await
            .map_err(|error| ApiError::bad_request(format!("Инструмент не ответил: {error}.")))?;
        Ok(tool_text(answer))
    }

    // What the plugins give the core.

    /// The plugins' tools for «Мозг», in MCP's form, with the right each needs (`use:<capability>`).
    pub fn tool_contributions(&self) -> Vec<ToolContribution> {
        if self.config.disabled {
            return Vec::new();
        }
        let tools = self.tools.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        tools
            .iter()
            .map(|offered| {
                let (me, plugin, handler) = (self.me.clone(), offered.plugin.clone(), offered.spec.handler.clone());
                let capability = offered.spec.capability;
                ToolContribution {
                    tool: LlmTool {
                        name: offered.spec.name.clone(),
                        description: format!("[{}] {}", offered.title, offered.spec.description)
                            .chars()
                            .take(1024)
                            .collect(),
                        parameters: offered.spec.input.clone(),
                    },
                    permission: use_of(offered.spec.capability),
                    run: Arc::new(move |input: Map<String, Value>, context: ToolContext| {
                        let (me, plugin, handler) = (me.clone(), plugin.clone(), handler.clone());
                        Box::pin(async move {
                            let host = me.upgrade().ok_or_else(|| anyhow::anyhow!("plugins are gone"))?;
                            let answer = host.call(&plugin, &handler, Value::Object(input), CallKind::Tool).await?;
                            Ok(host.act(&plugin, capability, answer, context.station.as_ref()))
                        })
                    }),
                    read_only: offered.spec.read_only,
                    outside: offered.outside,
                }
            })
            .collect()
    }

    /// A tool's answer that acts (docs/plans/plugins.md: «Инструмент плагина не только находит, но и делает»): an
    /// answer of a phrase's form — `{say, play, index}` — plays at once in the station's answer, and the model reads
    /// what was done; anything else the model reads as it is. Outside a station's answer (the panel's check) nothing
    /// plays and the model reads that.
    fn act(
        &self,
        plugin: &str,
        capability: CapabilityName,
        answer: Value,
        station: Option<&crate::core::connectors::StationAct>,
    ) -> String {
        let Value::Object(object) = &answer else { return tool_text(answer) };
        let Some(play) = object.get("play") else { return tool_text(answer) };
        let say = object.get("say").and_then(Value::as_str).map(str::trim).filter(|say| !say.is_empty());
        let kind = super::media::kind_for(capability);
        let base = station.and_then(|station| station.media_base_url.clone());
        let items = super::media::media_items_with(plugin, kind, play, base.as_deref(), &self.play_rights(plugin));
        let Some(first) = items.first().cloned() else {
            return "Nothing to play: the tool found nothing playable.".into();
        };
        let (Some(station), Some(player)) = (station, self.player.as_ref()) else {
            return format!("Would play «{}» (no station here: nothing played).", first.title);
        };
        let index = object.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let directives = player.play(&station.device, items, index, 0);
        if !(station.add)(directives) {
            return "error: the answer went out already; ask the user to say it again".into();
        }
        tracing::info!(target: "plugins", "{plugin}: its tool plays «{}»", first.title);
        match say {
            Some(say) => format!("Playing «{}» now. Tell the user: {say}", first.title),
            None => format!("Playing «{}» now. Tell the user briefly what is on.", first.title),
        }
    }

    // For the panel.

    async fn view_of(&self, plugin: &Plugin) -> PackageView {
        let (package, newest, enabled, problem, versions) = {
            let state = plugin.state();
            let newest = state.found.iter().find_map(|candidate| match candidate {
                Candidate::Ready(package) => Some(package.clone()),
                Candidate::Broken { .. } => None,
            });
            let versions = state
                .found
                .iter()
                .map(|candidate| VersionView {
                    version: candidate.version(),
                    problems: match candidate {
                        Candidate::Ready(package) => {
                            let why = package.manifest.kind.not_a_plugin().map(str::to_string);
                            why.or_else(|| package.manifest.incompatible_with((self.placement)())).into_iter().collect()
                        }
                        Candidate::Broken { problems, .. } => problems.clone(),
                    },
                })
                .collect::<Vec<_>>();
            (state.active.clone().filter(|_| state.enabled), newest, state.enabled, state.problem.clone(), versions)
        };
        let loaded = {
            let slot = plugin.slot.lock().await;
            slot.as_ref().map(|loaded| (loaded.version.clone(), loaded.instance.clone()))
        };
        let memory = match &loaded {
            Some((_, instance)) => instance.memory().await,
            None => None,
        };
        let stats = plugin.stats().clone();
        let shown = package.clone().or(newest);
        let manifest = shown.as_ref().map(|package| package.manifest.clone());
        // As the panel sees them: a secret one only as «••••••».
        let stored = self.settings.get(&PACKAGE_SETTINGS, None).unwrap_or_default();
        let values =
            manifest.as_ref().map(|manifest| manifest.settings_shown(stored.get(&plugin.id))).unwrap_or_default();
        let state = if self.config.disabled {
            "safeMode"
        } else if !enabled {
            "off"
        } else if package.is_none() {
            "error"
        } else if loaded.is_some() {
            "loaded"
        } else {
            "ready"
        };
        let (source, verified) = {
            let state = plugin.state();
            (state.source.clone(), state.verified)
        };
        let origin = Origin::of(&source);
        let runs_at = package.as_ref().filter(|package| package.manifest.prefer_home).map(|package| {
            if self.runs_at_home(package) {
                "home"
            } else {
                "here"
            }
        });
        let panel_part = |file: &str| {
            package
                .as_ref()
                .filter(|package| package.manifest.contributes.panel.is_some() && package.dir.join(file).is_file())
                .filter(|_| !self.config.disabled)
                .map(|package| format!("/panel-plugins/{}/{}/{file}", plugin.id, package.version()))
        };
        PackageView {
            runs_at,
            origin,
            removable: origin.removable(),
            can_disable: origin.can_disable(),
            editable: origin.editable(),
            panel_module: panel_part("panel.js"),
            panel_styles: panel_part("panel.css"),
            kind: manifest.as_ref().map(|manifest| manifest.kind),
            id: plugin.id.clone(),
            title: manifest.as_ref().map(|manifest| manifest.title.clone()).unwrap_or_else(|| plugin.id.clone()),
            description: manifest.as_ref().and_then(|manifest| manifest.description.clone()),
            license: manifest.as_ref().and_then(|manifest| manifest.license.clone()),
            source,
            verified,
            version: shown.as_ref().map(|package| package.version()),
            versions,
            enabled,
            state,
            loaded: loaded.is_some(),
            problem,
            budget: manifest.as_ref().map(|manifest| manifest.budget),
            memory,
            storage_used: storage_used(&self.db, &plugin.id),
            permissions: manifest
                .as_ref()
                .map(|manifest| manifest.permissions.iter().map(Permission::name).collect())
                .unwrap_or_default(),
            runs: manifest
                .as_ref()
                .map(|manifest| manifest.runs.iter().map(|runs| runs.as_str()).collect())
                .unwrap_or_default(),
            calls: stats.calls,
            errors: stats.errors,
            last_error: stats.last_error,
            last_call_at: stats.last_call_at,
            contributes: manifest
                .as_ref()
                .map(|manifest| ContributesView {
                    phrases: manifest
                        .contributes
                        .intents
                        .iter()
                        .flat_map(|intent| intent.phrases.iter().map(|phrase| phrase.template.clone()))
                        .collect(),
                    tools: manifest.contributes.tools.iter().map(|tool| tool.name.clone()).collect(),
                    media: manifest
                        .contributes
                        .media
                        .iter()
                        .map(|media| format!("{} ({:?})", media.id, media.kind).to_lowercase())
                        .collect(),
                    events: manifest.contributes.events.iter().map(|event| event.name.clone()).collect(),
                    panel: manifest.contributes.panel.is_some(),
                    themes: manifest
                        .contributes
                        .themes
                        .as_ref()
                        .is_some_and(|themes| themes.as_array().is_some_and(|all| !all.is_empty())),
                })
                .unwrap_or_default(),
            settings: manifest
                .as_ref()
                .map(|manifest| {
                    manifest
                        .settings
                        .iter()
                        .map(|spec| SettingView { value: values.get(&spec.key).cloned(), spec: spec.clone() })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Every package found, as the panel lists them. The folder is read again first: a package copied in shows.
    pub async fn view(&self) -> PackagesView {
        self.rescan();
        let mut packages = Vec::new();
        for plugin in self.list() {
            packages.push(self.view_of(&plugin).await);
        }
        PackagesView {
            safe_mode: self.config.disabled,
            dir: self.config.packages_dir.display().to_string(),
            placement: (self.placement)(),
            idle_unload_s: self.config.idle_unload_s,
            packages,
        }
    }

    pub async fn view_one(&self, id: &str) -> Result<PackageView, ApiError> {
        let plugin = self.known(id)?;
        Ok(self.view_of(&plugin).await)
    }
}

/// A package's folder copied under a new id: every file, the manifest's id and title changed.
fn copy_package(from: &std::path::Path, to: &std::path::Path, id: &str, title: &str) -> std::io::Result<()> {
    fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                copy_dir(&entry.path(), &to.join(entry.file_name()))?;
            } else if kind.is_file() {
                std::fs::copy(entry.path(), to.join(entry.file_name()))?;
            }
        }
        Ok(())
    }
    copy_dir(from, to)?;
    let file = to.join("manifest.json");
    let mut manifest: Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
    manifest["id"] = json!(id);
    manifest["title"] = json!(title);
    std::fs::write(file, serde_json::to_string_pretty(&manifest)?)
}

/// A theme as a package gives it (docs/design/glass/THEMES.md): an id, a title, the palettes of both modes as text
/// tokens; the layout, if given, one the panel knows the name of. What it leaves out the panel takes from «Стекло».
pub fn theme_form(theme: &Value) -> Result<(), String> {
    let id = theme.get("id").and_then(Value::as_str).unwrap_or_default();
    if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err("id темы — строчные латинские буквы, цифры и «-»".into());
    }
    if theme.get("title").and_then(Value::as_str).is_none_or(|title| title.trim().is_empty()) {
        return Err(format!("{id}: нет title"));
    }
    for mode in ["dark", "light"] {
        let palette = theme.pointer(&format!("/palette/{mode}")).and_then(Value::as_object);
        let Some(palette) = palette else { return Err(format!("{id}: нет palette.{mode}")) };
        if let Some((token, _)) = palette.iter().find(|(token, value)| {
            !value.is_string()
                || token.is_empty()
                || !token.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        }) {
            return Err(format!(
                "{id}: palette.{mode}.{token} — токен из строчных букв, цифр и «-» со строкой-значением"
            ));
        }
    }
    match theme.get("layout").and_then(Value::as_str) {
        // «desk» — стол и док (docs/plans/desk.md); «tabbar» прежних тем — тоже стол.
        None | Some("desk") | Some("tabbar") | Some("sidebar") => Ok(()),
        Some(other) => Err(format!("{id}: раскладка «{other}» — это код оболочки, плагин пульта, а не тема")),
    }
}

/// A plugin's part of the panel, to load.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelPart {
    pub id: String,
    pub title: String,
    pub version: String,
    pub module: Option<String>,
    pub styles: Option<String>,
    /// The values of its settings, for `useSettings`: a secret one only as «••••••».
    pub settings: Map<String, Value>,
}

/// A theme of a package, for the panel's choice.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeView {
    pub package: String,
    pub version: String,
    pub origin: Origin,
    pub theme: Value,
}

/// A tool's answer as the model reads it: text as it is, anything else as JSON; cut to what a turn can carry.
fn tool_text(answer: Value) -> String {
    let text = match answer {
        Value::String(text) => text,
        Value::Null => "null".into(),
        other => other.to_string(),
    };
    if text.len() <= TOOL_ANSWER {
        return text;
    }
    let mut end = TOOL_ANSWER;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// The plugins' phrases for the router: the first that takes the whole command wins, in the plugins' order.
impl ExtraRules for PluginHost {
    fn matched(&self, normalized: &str) -> Option<QuickMatch> {
        let rules = self.rules.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        rules.iter().find_map(|rule| {
            let slots = rule.phrase.matches(normalized)?;
            let mut intent = Intent::new(rule.capability, &rule.handler);
            intent.params.insert(EXECUTOR.into(), json!(CapabilityName::Plugins.as_str()));
            intent.params.insert("plugin".into(), json!(rule.plugin));
            intent.params.insert("handler".into(), json!(rule.handler));
            intent.params.insert("slots".into(), Value::Object(slots));
            Some(QuickMatch { intent, rule: rule.name })
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionView {
    pub version: String,
    /// Why it cannot run here; empty when it can.
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContributesView {
    pub phrases: Vec<String>,
    pub tools: Vec<String>,
    pub media: Vec<String>,
    pub events: Vec<String>,
    pub panel: bool,
    pub themes: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingView {
    #[serde(flatten)]
    pub spec: SettingSpec,
    pub value: Option<Value>,
}

/// A package as the panel shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageView {
    /// The package's kind: a plugin, or a package of another kind this host lists and does not run.
    pub kind: Option<PackageKind>,
    /// Where it came from, as the gateway knows: system, bundled, user, catalog.
    pub origin: Origin,
    /// For a plugin that prefers home: where its calls go now — home (the computer) or here.
    pub runs_at: Option<&'static str>,
    /// Its folder may go: one's own and a catalog's; a built-in is updated with the gateway.
    pub removable: bool,
    /// It may be switched off: all but a system one.
    pub can_disable: bool,
    /// It changes in place, in the workshop: one's own only.
    pub editable: bool,
    /// While it is on: its part of the panel (`panel.js`) and its styles, to load.
    pub panel_module: Option<String>,
    pub panel_styles: Option<String>,
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub license: Option<String>,
    /// catalog: installed from the catalog by a button; local: copied in by hand.
    pub source: String,
    /// The catalog vouched for it; one put in by hand never is.
    pub verified: bool,
    /// The version in force, or the newest found while it is off.
    pub version: Option<String>,
    pub versions: Vec<VersionView>,
    pub enabled: bool,
    /// off, ready (on, its code not loaded), loaded, error (on, but nothing of it runs here), safeMode.
    pub state: &'static str,
    pub loaded: bool,
    pub problem: Option<String>,
    pub budget: Option<Budget>,
    /// While loaded: what its engine counts.
    pub memory: Option<MemoryStats>,
    /// Its `kv` store, bytes.
    pub storage_used: u64,
    pub permissions: Vec<String>,
    pub runs: Vec<&'static str>,
    pub calls: u64,
    pub errors: u64,
    pub last_error: Option<LastError>,
    pub last_call_at: Option<String>,
    pub contributes: ContributesView,
    pub settings: Vec<SettingView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackagesView {
    /// PLUGINS_DISABLED: the packages are listed, no plugin runs.
    pub safe_mode: bool,
    pub dir: String,
    pub placement: Placement,
    pub idle_unload_s: u64,
    pub packages: Vec<PackageView>,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::core::identity::principal::system;
    use crate::plugins::quickjs::QuickJs;
    use crate::plugins::registry::tests::write_package;
    use crate::skills::media::media_types::MediaKind;
    use tokio_util::sync::CancellationToken;

    struct Fixture {
        host: Arc<PluginHost>,
        media: Arc<MediaSources>,
        events: EventBus,
    }

    thread_local! {
        static PLAYER: std::cell::RefCell<Option<Arc<MediaPlayer>>> = const { std::cell::RefCell::new(None) };
        static HOME: std::cell::RefCell<Option<Arc<dyn HomeLink>>> = const { std::cell::RefCell::new(None) };
    }

    /// The player the test's watch watches (a test sets it before it builds the host).
    fn player_of_tests() -> Arc<MediaPlayer> {
        PLAYER.with(|player| player.borrow().clone()).expect("a test that plays sets its player")
    }

    fn host_on(dir: &Path, db: Arc<Db>, disabled: bool, placement: Placement) -> Fixture {
        host_with(dir, db, disabled, placement, crate::plugins::budget::fake::plenty())
    }

    fn host_with(
        dir: &Path,
        db: Arc<Db>,
        disabled: bool,
        placement: Placement,
        memory: Arc<crate::plugins::budget::fake::FixedMemory>,
    ) -> Fixture {
        host_playing(dir, db, disabled, placement, memory, None)
    }

    fn host_playing(
        dir: &Path,
        db: Arc<Db>,
        disabled: bool,
        placement: Placement,
        memory: Arc<crate::plugins::budget::fake::FixedMemory>,
        watch: Option<Arc<PlaybackWatch>>,
    ) -> Fixture {
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let media = MediaSources::new(Vec::new());
        let host = PluginHost::new(PluginsDeps {
            config: PluginsConfig {
                packages_dir: dir.to_path_buf(),
                disabled,
                idle_unload_s: 1,
                feed_url: None,
                catalog_dir: dir.join(".catalog"),
                dev_dir: dir.join(".dev"),
                catalog_key: None,
                reserve_mb: 40,
                storage_reserve_mb: 15,
                pressure_action: crate::plugins::budget::PressureAction::Disable,
                bundled: false,
            },
            db,
            settings,
            events: events.clone(),
            media: media.clone(),
            placement: Arc::new(move || placement),
            speaker: Arc::new(|| None),
            runtime: Arc::new(QuickJs),
            memory,
            gauge: Default::default(),
            bundled: Vec::new(),
            player: watch.as_ref().map(|_| player_of_tests()),
            watch,
            home: HOME.with(|home| home.borrow().clone()),
            gateway_url: "http://127.0.0.1:9".into(),
            services: None,
        });
        Fixture { host, media, events }
    }

    fn echo(version: &str) -> Value {
        json!({
            "schema": 1, "id": "echo", "version": version, "title": "Эхо", "api": "^1",
            "runs": ["pc", "station", "stationServer"],
            "budget": { "memoryMb": 2 },
            "permissions": ["voice.intents", "voice.say", "llm.tools", "media.source", "kv", "net:allowed.example"],
            "contributes": {
                "intents": [{ "phrases": ["скажи эхо {word}"], "handler": "say" }],
                "tools": [{
                    "name": "echo_word", "description": "Повторяет слово", "handler": "say",
                    "input": { "type": "object", "properties": { "word": { "type": "string" } } }
                }],
                "media": [{ "id": "tracks", "kind": "music" }]
            }
        })
    }

    const CODE: &str = r#"
        let calls = 0;
        export function say({ word }) { calls += 1; return `${word} (${calls})`; }
        export function tracks({ query }) { return [{ id: "a", title: `Трек ${query ?? ""}`, url: "https://music.example/a.mp3" }]; }
        export function spin() { while (true) {} }
        export function remember({ value }) { host.kv.set("last", value); return host.kv.get("last"); }
        export async function reach({ url }) { return (await host.http.fetch(url)).status; }
        export async function speak({ text }) { return await host.voice.say(text); }
    "#;

    fn said(answer: Result<Value, PluginError>) -> String {
        answer.unwrap().as_str().unwrap().to_string()
    }

    fn changes(events: &mut tokio::sync::broadcast::Receiver<Arc<GatewayEvent>>) -> Vec<String> {
        let mut found = Vec::new();
        while let Ok(event) = events.try_recv() {
            if let GatewayEvent::PackagesChanged { change, .. } = event.as_ref() {
                found.push(change.clone());
            }
        }
        found
    }

    #[tokio::test]
    async fn switches_a_plugin_on_and_off_and_its_phrases_tools_and_sources_with_it() {
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &echo("1.0.0"), Some(CODE));
        let fixture = host_on(dir.path(), Arc::new(Db::memory()), false, Placement::Pc);
        let (host, mut events) = (fixture.host.clone(), fixture.events.subscribe());
        assert_eq!(host.view().await.packages[0].state, "off");
        assert!(host.matched("скажи эхо привет").is_none());
        assert!(host.tool_contributions().is_empty() && fixture.media.of(MediaKind::Music).is_empty());

        let view = host.enable("echo", None).await.unwrap();
        assert_eq!((view.state, view.version.as_deref(), view.loaded), ("loaded", Some("1.0.0"), true));
        let found = host.matched("скажи эхо привет").unwrap();
        assert_eq!((found.rule, found.intent.capability), ("plugin:echo", CapabilityName::Plugins));
        assert_eq!(found.intent.params["slots"], json!({"word": "привет"}));
        assert_eq!(found.intent.params[EXECUTOR], "plugins");
        assert_eq!(said(host.call("echo", "say", json!({"word": "а"}), CallKind::Voice).await), "а (1)");
        // Speech without a request: no station on the line here, so nothing was said.
        assert_eq!(
            host.call("echo", "speak", json!({"text": "Звонок в дверь"}), CallKind::Background).await,
            Ok(json!(false))
        );

        let tools = host.tool_contributions();
        assert_eq!(tools.len(), 1);
        let tool = &tools[0];
        assert_eq!((tool.tool.name.as_str(), tool.permission.as_str()), ("echo_word", "use:plugins"));
        assert_eq!(tool.tool.description, "[Эхо] Повторяет слово");
        assert!(tool.outside && !tool.read_only, "a plugin with net: rights brings text from outside");
        let input = json!({"word": "б"}).as_object().cloned().unwrap();
        let context = ToolContext { principal: system(), cancel: CancellationToken::new(), station: None };
        assert_eq!((tool.run)(input, context).await.unwrap(), "б (2)");

        let sources = fixture.media.of(MediaKind::Music);
        assert_eq!(sources.len(), 1);
        assert!(sources[0].available());
        let items = sources[0].search(Some("джаз".into()), None).await;
        assert_eq!((items[0].id.as_str(), items[0].title.as_str()), ("echo/a", "Трек джаз"));

        host.disable("echo", None).await.unwrap();
        assert!(host.matched("скажи эхо привет").is_none());
        assert!(host.tool_contributions().is_empty() && fixture.media.of(MediaKind::Music).is_empty());
        assert!(matches!(host.call("echo", "say", json!({}), CallKind::Voice).await, Err(PluginError::Unavailable(_))));
        assert_eq!(changes(&mut events), ["enabled", "disabled"]);
    }

    #[tokio::test]
    async fn loads_its_code_at_the_first_call_and_unloads_it_after_a_while_without_one() {
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &echo("1.0.0"), Some(CODE));
        let db = Arc::new(Db::memory());
        let first = host_on(dir.path(), db.clone(), false, Placement::Pc);
        first.host.enable("echo", None).await.unwrap();
        assert_eq!(said(first.host.call("echo", "say", json!({"word": "а"}), CallKind::Voice).await), "а (1)");
        tokio::time::sleep(Duration::from_millis(1100)).await;
        first.host.unload_idle();
        let view = first.host.view_one("echo").await.unwrap();
        assert_eq!((view.state, view.loaded), ("ready", false));
        // Loaded again at the next call, a fresh context.
        assert_eq!(said(first.host.call("echo", "say", json!({"word": "б"}), CallKind::Voice).await), "б (1)");
        assert!(first.host.view_one("echo").await.unwrap().memory.is_some_and(|memory| memory.used > 0));

        // After a restart it is on, and nothing is loaded until it is called.
        let again = host_on(dir.path(), db, false, Placement::Pc);
        let view = again.host.view_one("echo").await.unwrap();
        assert_eq!((view.enabled, view.state, view.loaded), (true, "ready", false));
        assert!(again.host.matched("скажи эхо привет").is_some());
        assert_eq!(said(again.host.call("echo", "say", json!({"word": "в"}), CallKind::Voice).await), "в (1)");
    }

    #[tokio::test]
    async fn keeps_the_running_version_when_a_new_one_does_not_start_after_a_restart_too() {
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &echo("1.0.0"), Some(CODE));
        let db = Arc::new(Db::memory());
        let fixture = host_on(dir.path(), db.clone(), false, Placement::Pc);
        let host = fixture.host.clone();
        host.enable("echo", None).await.unwrap();
        assert_eq!(said(host.call("echo", "say", json!({"word": "а"}), CallKind::Voice).await), "а (1)");

        write_package(dir.path(), &echo("1.1.0"), Some("export function say( {"));
        let refused = host.reload("echo", None).await.unwrap_err();
        assert!(
            refused.message.contains("1.1.0 не поднялась") && refused.message.contains("прежняя 1.0.0"),
            "{}",
            refused.message
        );
        let view = host.view_one("echo").await.unwrap();
        assert_eq!((view.version.as_deref(), view.state), (Some("1.0.0"), "loaded"));
        assert!(view.last_error.unwrap().message.contains("1.1.0"));
        // The old code goes on, its state too.
        assert_eq!(said(host.call("echo", "say", json!({"word": "б"}), CallKind::Voice).await), "б (2)");

        // A restart takes the newest version on disk; it does not start, so the one that last did runs.
        let restarted = host_on(dir.path(), db, false, Placement::Pc);
        assert_eq!(said(restarted.host.call("echo", "say", json!({"word": "в"}), CallKind::Voice).await), "в (1)");
        let view = restarted.host.view_one("echo").await.unwrap();
        assert_eq!(view.version.as_deref(), Some("1.0.0"));
        assert!(
            view.problem.as_deref().is_some_and(|problem| problem.contains("1.1.0 не поднялась")),
            "{:?}",
            view.problem
        );

        // A version that starts takes over.
        write_package(dir.path(), &echo("1.2.0"), Some(CODE));
        let view = host.reload("echo", None).await.unwrap();
        assert_eq!(view.version.as_deref(), Some("1.2.0"));
        assert_eq!(said(host.call("echo", "say", json!({"word": "г"}), CallKind::Voice).await), "г (1)");
    }

    /// The owner, 10.11: an item added to the list by voice did not show in the widget until a reload. A plugin's
    /// write to its store tells the panel (`packages.data`): the widget reads its data again.
    #[tokio::test]
    async fn a_plugins_write_to_its_store_tells_the_panel() {
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &echo("1.0.0"), Some(CODE));
        let fixture = host_on(dir.path(), Arc::new(Db::memory()), false, Placement::Pc);
        let (host, mut events) = (fixture.host.clone(), fixture.events.subscribe());
        host.enable("echo", None).await.unwrap();
        let remembered = host.call("echo", "remember", json!({"value": "молоко"}), CallKind::Voice).await;
        assert_eq!(remembered, Ok(json!("молоко")));
        let mut told = Vec::new();
        while let Ok(event) = events.try_recv() {
            if let GatewayEvent::PackageDataChanged { package } = event.as_ref() {
                told.push(package.clone());
            }
        }
        assert_eq!(told, ["echo"]);
    }

    #[tokio::test]
    async fn refuses_what_the_manifest_does_not_allow() {
        let dir = tempfile::tempdir().unwrap();
        let mut strict = echo("1.0.0");
        strict["id"] = json!("strict");
        strict["permissions"] = json!(["voice.intents", "net:allowed.example"]);
        strict["contributes"] = json!({ "intents": [{ "phrases": ["проверь права"], "handler": "say" }] });
        write_package(dir.path(), &strict, Some(CODE));
        let mut greedy = echo("1.0.0");
        greedy["id"] = json!("greedy");
        greedy["permissions"] = json!(["voice.intents"]);
        write_package(dir.path(), &greedy, Some(CODE));
        let host = host_on(dir.path(), Arc::new(Db::memory()), false, Placement::Pc).host;
        host.enable("strict", None).await.unwrap();

        let failure = |answer: Result<Value, PluginError>| match answer {
            Err(PluginError::Script(text)) => text,
            other => panic!("{other:?}"),
        };
        let kv = failure(host.call("strict", "remember", json!({"value": 1}), CallKind::Voice).await);
        assert!(kv.contains("нет права kv"), "{kv}");
        let other =
            failure(host.call("strict", "reach", json!({"url": "https://other.example/x"}), CallKind::Voice).await);
        assert!(other.contains("хост other.example не разрешён"), "{other}");
        let local = failure(host.call("strict", "reach", json!({"url": "http://127.0.0.1:1/"}), CallKind::Voice).await);
        assert!(local.contains("адрес этого компьютера"), "{local}");
        let scheme = failure(host.call("strict", "reach", json!({"url": "file:///etc/passwd"}), CallKind::Voice).await);
        assert!(scheme.contains("только http и https"), "{scheme}");
        let speech = failure(host.call("strict", "speak", json!({"text": "Привет"}), CallKind::Background).await);
        assert!(speech.contains("нет права voice.say"), "{speech}");
        let view = host.view_one("strict").await.unwrap();
        assert_eq!(view.errors, 5);
        assert!(view.last_error.unwrap().message.starts_with("speak: "), "the last error names the function");

        // Tools for «Мозг» without llm.tools, a source without media.source: the package is refused whole.
        let refused = host.enable("greedy", None).await.unwrap_err();
        assert!(
            refused.message.contains("нужно право llm.tools") && refused.message.contains("нужно право media.source"),
            "{}",
            refused.message
        );
        assert!(host.tool_contributions().is_empty());
    }

    #[tokio::test]
    async fn starts_afresh_after_a_call_that_ran_out_of_time() {
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &echo("1.0.0"), Some(CODE));
        let host = host_on(dir.path(), Arc::new(Db::memory()), false, Placement::Pc).host;
        host.enable("echo", None).await.unwrap();
        assert_eq!(said(host.call("echo", "say", json!({"word": "а"}), CallKind::Voice).await), "а (1)");
        assert_eq!(host.call("echo", "spin", json!({}), CallKind::Voice).await, Err(PluginError::Timeout(2000)));
        assert!(!host.view_one("echo").await.unwrap().loaded, "a context that ran away is not trusted");
        assert_eq!(said(host.call("echo", "say", json!({"word": "б"}), CallKind::Voice).await), "б (1)");
    }

    #[tokio::test]
    async fn runs_nothing_in_the_safe_mode_whatever_is_switched_on() {
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &echo("1.0.0"), Some(CODE));
        let db = Arc::new(Db::memory());
        registry::set_enabled(&db, "echo", true, Some("1.0.0"), None).unwrap();
        let fixture = host_on(dir.path(), db, true, Placement::Pc);
        let host = fixture.host.clone();
        let view = host.view().await;
        assert!(view.safe_mode);
        assert_eq!((view.packages[0].enabled, view.packages[0].state), (true, "safeMode"));
        assert!(host.matched("скажи эхо привет").is_none());
        assert!(host.tool_contributions().is_empty() && fixture.media.of(MediaKind::Music).is_empty());
        let answer = host.call("echo", "say", json!({}), CallKind::Voice).await;
        assert!(matches!(answer, Err(PluginError::Unavailable(text)) if text.contains("безопасный режим")));
        assert_eq!(host.enable("echo", None).await.unwrap_err().status, axum::http::StatusCode::CONFLICT);
        assert!(!host.is_active("echo"));
    }

    #[tokio::test]
    async fn refuses_to_switch_on_what_does_not_run_where_the_gateway_runs() {
        let dir = tempfile::tempdir().unwrap();
        let mut pc_only = echo("1.0.0");
        pc_only["runs"] = json!(["pc"]);
        write_package(dir.path(), &pc_only, Some(CODE));
        let host = host_on(dir.path(), Arc::new(Db::memory()), false, Placement::Station).host;
        let refused = host.enable("echo", None).await.unwrap_err();
        assert!(refused.message.contains("На колонке"), "{}", refused.message);
        let view = host.view_one("echo").await.unwrap();
        assert!(view.versions[0].problems[0].contains("не работает здесь"));
    }

    #[tokio::test]
    async fn lists_a_package_of_another_kind_without_code_and_does_not_switch_it_on() {
        let dir = tempfile::tempdir().unwrap();
        let character = json!({
            "schema": 1, "kind": "character", "id": "ded-moroz", "version": "1.0.0", "title": "Дед Мороз", "api": "^1",
            "runs": ["pc", "station", "stationServer"],
            "contributes": { "characters": [{ "id": "ded-moroz" }] }
        });
        write_package(dir.path(), &character, None);
        let host = host_on(dir.path(), Arc::new(Db::memory()), false, Placement::Pc).host;
        let view = host.view_one("ded-moroz").await.unwrap();
        assert_eq!((view.kind, view.state), (Some(PackageKind::Character), "off"));
        assert!(view.versions[0].problems[0].contains("персонаж"));
        let refused = host.enable("ded-moroz", None).await.unwrap_err();
        assert!(refused.message.contains("персонаж — не плагин"), "{}", refused.message);
    }

    /// A small plugin of its own id: one phrase, its own `memoryMb`, the echo's code.
    fn small(id: &str, memory_mb: u64) -> Value {
        json!({
            "schema": 1, "id": id, "version": "1.0.0", "title": id, "api": "^1",
            "runs": ["pc", "station", "stationServer"],
            "budget": { "memoryMb": memory_mb },
            "permissions": ["voice.intents"],
            "contributes": { "intents": [{ "phrases": [format!("{id} {{word}}")], "handler": "say" }] }
        })
    }

    /// The station with `available_mb` of memory available: with the reserve of 40, the rest is the plugins' budget.
    fn station(available_mb: u64) -> crate::plugins::budget::MemoryNow {
        crate::plugins::budget::MemoryNow {
            total_mb: 238,
            available_mb,
            pressure: Pressure::Normal,
            gateway_mb: Some(33),
        }
    }

    async fn is_loaded(host: &PluginHost, id: &str) -> bool {
        host.view_one(id).await.unwrap().loaded
    }

    /// The owner, 10.11: a small plugin from the workshop did not go on — «нужно 4 МБ, свободно 0» — because every
    /// plugin that was on took its declared budget. Now only a loaded context takes its share, and memory never keeps
    /// a plugin off.
    #[tokio::test]
    async fn switches_a_plugin_on_whatever_the_memory_and_counts_only_what_is_loaded() {
        let dir = tempfile::tempdir().unwrap();
        for (id, memory_mb) in [("alpha", 4), ("beta", 4), ("big", 30)] {
            write_package(dir.path(), &small(id, memory_mb), Some(CODE));
        }
        // 60 MB available, the reserve 40: 20 for the plugins' contexts.
        let memory = crate::plugins::budget::fake::plenty();
        memory.set(station(60));
        let fixture = host_with(dir.path(), Arc::new(Db::memory()), false, Placement::Station, memory.clone());
        let host = fixture.host.clone();
        // 38 MB declared in all, more than the 20, and none refused.
        assert!(host.enable("alpha", None).await.unwrap().loaded);
        assert!(host.enable("beta", None).await.unwrap().loaded);
        let big = host.enable("big", None).await.unwrap();
        assert_eq!((big.enabled, big.state, big.loaded), (true, "ready", false), "on, its context waits for room");
        assert!(
            host.journal("big").iter().any(|line| line.message.contains("нужно 30 МБ")),
            "the journal says why it did not start"
        );
        assert!(host.is_active("big"), "its phrases and the rest are in force");

        // Only the loaded contexts take their share: 4 + 4 of the budget, the plugin that is on but not loaded nothing.
        let resources = host.resources().await;
        assert!(resources.enforced && resources.measured);
        assert_eq!((resources.committed_mb, resources.loaded), (8, 2), "{resources:?}");
        assert!(resources.free_mb >= 12 && resources.room_mb >= 20, "{resources:?}");
        assert!(
            resources.note.starts_with("Загружено плагинов: 2, под них занято 8 МБ. Можно загрузить ещё"),
            "{}",
            resources.note
        );
        let share = |id: &str| resources.plugins.iter().find(|share| share.id == id).cloned().unwrap();
        assert_eq!((share("alpha").taken_mb, share("big").taken_mb, share("big").enabled), (4, 0, true));

        // Its load: 30 MB do not fit even with the idle contexts unloaded, so none of them is unloaded for nothing.
        let refused = host.call("big", "say", json!({"word": "а"}), CallKind::Voice).await.unwrap_err();
        assert!(matches!(&refused, PluginError::NoRoom(why) if why.contains("нужно 30 МБ, свободно")), "{refused}");
        assert!(is_loaded(&host, "alpha").await && is_loaded(&host, "beta").await);
        assert!(host.view_one("big").await.unwrap().last_error.unwrap().message.contains("не хватает памяти"));

        // More memory: it loads at its call.
        memory.set(station(80));
        assert_eq!(said(host.call("big", "say", json!({"word": "б"}), CallKind::Voice).await), "б (1)");
        assert_eq!(host.resources().await.committed_mb, 38);
    }

    #[tokio::test]
    async fn unloads_the_longest_idle_contexts_to_make_room_and_never_one_in_a_call() {
        let dir = tempfile::tempdir().unwrap();
        for id in ["alpha", "beta", "gamma", "delta"] {
            write_package(dir.path(), &small(id, 4), Some(CODE));
        }
        // 12 MB for the plugins: three contexts of 4.
        let memory = crate::plugins::budget::fake::plenty();
        memory.set(station(52));
        let fixture = host_with(dir.path(), Arc::new(Db::memory()), false, Placement::Station, memory);
        let (host, mut events) = (fixture.host.clone(), fixture.events.subscribe());
        for id in ["alpha", "beta", "gamma"] {
            assert!(host.enable(id, None).await.unwrap().loaded);
        }
        // alpha idle the longest, then beta; gamma is in a call, however long it has been.
        for (id, used) in [("alpha", 1), ("beta", 2), ("gamma", 0)] {
            host.plugin(id).unwrap().used.store(used, Ordering::Relaxed);
        }
        let gamma = host.plugin("gamma").unwrap();
        let in_call = Busy::new(&gamma);
        changes(&mut events);

        // delta switched on: alpha, the longest idle, makes room — only it.
        assert!(host.enable("delta", None).await.unwrap().loaded);
        let loaded = [
            is_loaded(&host, "alpha").await,
            is_loaded(&host, "beta").await,
            is_loaded(&host, "gamma").await,
            is_loaded(&host, "delta").await,
        ];
        assert_eq!(loaded, [false, true, true, true]);
        assert_eq!(changes(&mut events), ["unloaded", "enabled"]);
        assert!(host.journal("alpha").iter().any(|line| line.message.contains("понадобилась плагину delta")));

        // alpha called again: beta is the longest idle now; gamma, in its call, stays.
        host.plugin("delta").unwrap().used.store(4, Ordering::Relaxed);
        assert_eq!(said(host.call("alpha", "say", json!({"word": "а"}), CallKind::Voice).await), "а (1)");
        assert!(!is_loaded(&host, "beta").await);
        assert!(is_loaded(&host, "alpha").await && is_loaded(&host, "gamma").await && is_loaded(&host, "delta").await);

        // Every loaded context in a call: nothing can be freed, the load is refused and nothing is unloaded.
        let (alpha, delta) = (host.plugin("alpha").unwrap(), host.plugin("delta").unwrap());
        let calls = [Busy::new(&alpha), Busy::new(&delta)];
        let refused = host.call("beta", "say", json!({"word": "б"}), CallKind::Voice).await.unwrap_err();
        assert_eq!(
            refused,
            PluginError::NoRoom("колонке не хватает памяти, чтобы загрузить плагин: нужно 4 МБ, свободно 0 МБ".into())
        );
        assert!(is_loaded(&host, "alpha").await && is_loaded(&host, "gamma").await && is_loaded(&host, "delta").await);
        let view = host.view_one("beta").await.unwrap();
        assert_eq!((view.enabled, view.loaded), (true, false), "refused a load, it stays on");
        // The idle unloading leaves a context in a call too.
        host.unload_idle_for(0);
        assert!(is_loaded(&host, "alpha").await && is_loaded(&host, "gamma").await && is_loaded(&host, "delta").await);

        // The calls over, the next load makes room again.
        drop((calls, in_call));
        assert_eq!(said(host.call("beta", "say", json!({"word": "б"}), CallKind::Voice).await), "б (1)");
    }

    #[tokio::test]
    async fn gives_memory_back_under_pressure_and_switches_off_only_what_is_loaded() {
        use crate::plugins::budget::MemoryNow;
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), &echo("1.0.0"), Some(CODE));
        write_package(dir.path(), &small("big", 30), Some(CODE));
        // The station: 60 MB available, the reserve 40 — 20 for plugins.
        let memory = crate::plugins::budget::fake::plenty();
        memory.set(station(60));
        let fixture = host_with(dir.path(), Arc::new(Db::memory()), false, Placement::Station, memory.clone());
        let (host, mut events) = (fixture.host.clone(), fixture.events.subscribe());
        host.enable("echo", None).await.unwrap();
        assert!(!host.enable("big", None).await.unwrap().loaded, "on, no room to load");

        // Pressure: an idle plugin unloads, background work waits; at «critical» the plugin switched on last among
        // those loaded goes off — big, switched on later, holds nothing and stays on.
        memory.set(MemoryNow { available_mb: 15, pressure: Pressure::Critical, ..station(60) });
        host.plugin("echo").unwrap().used.store(0, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(5)).await;
        host.watch_memory().await;
        assert!(host.deferring.load(Ordering::Relaxed));
        let view = host.view_one("echo").await.unwrap();
        assert_eq!((view.enabled, view.loaded), (false, false));
        assert!(view.problem.unwrap().contains("колонке не хватало памяти"));
        assert!(host.view_one("big").await.unwrap().enabled);
        let told: Vec<String> = changes(&mut events);
        assert!(told.contains(&"pressure".to_string()), "{told:?}");
        // Again: nothing is loaded, nothing more goes off.
        host.watch_memory().await;
        assert!(host.view_one("big").await.unwrap().enabled);

        // On a computer the budget is shown, not kept.
        let pc = host_on(dir.path(), Arc::new(Db::memory()), false, Placement::Pc).host;
        assert!(!pc.resources().await.enforced);
        assert!(pc.enable("big", None).await.unwrap().loaded);
    }

    #[tokio::test]
    async fn a_secret_setting_reaches_the_plugin_as_it_is_and_the_panel_only_as_dots() {
        let dir = tempfile::tempdir().unwrap();
        let mut calendar = echo("1.0.0");
        calendar["settings"] = json!([
            { "key": "ics", "type": "string", "title": "Ссылка iCal", "secret": true },
            { "key": "days", "type": "number", "title": "Дней", "default": 7 }
        ]);
        calendar["contributes"]["panel"] = json!({ "slots": ["desk.widgets"] });
        write_package(dir.path(), &calendar, Some(CODE));
        std::fs::write(dir.path().join("echo/1.0.0/panel.js"), "export default {};").unwrap();
        let host = host_on(dir.path(), Arc::new(Db::memory()), false, Placement::Pc).host;
        host.enable("echo", None).await.unwrap();
        let stored = |key: &str| {
            let all = host.settings().get(&PACKAGE_SETTINGS, None).unwrap_or_default();
            all.get("echo").and_then(|own| own.get(key)).cloned()
        };
        let link = "https://calendar.example/private-0123/basic.ics";

        let values = json!({ "ics": link, "days": 3 });
        let view = host.set_settings("echo", values.as_object().unwrap(), None).await.unwrap();
        let shown: Vec<Option<Value>> = view.settings.iter().map(|setting| setting.value.clone()).collect();
        assert_eq!(shown, [Some(json!(manifest::SECRET_SHOWN)), Some(json!(3))]);
        assert_eq!(serde_json::to_value(&view).unwrap()["settings"][0]["secret"], true);
        let part = host.panel_parts().into_iter().find(|part| part.id == "echo").unwrap();
        assert_eq!(part.settings["ics"], manifest::SECRET_SHOWN, "the panel parts — every viewer of the station");
        assert_eq!(host.view_one("echo").await.unwrap().settings[0].value, Some(json!(manifest::SECRET_SHOWN)));
        // The plugin itself reads the link as it is.
        let package = host.package("echo").unwrap();
        let own = host.settings().get(&PACKAGE_SETTINGS, None).unwrap_or_default();
        assert_eq!(package.manifest.settings_values(own.get("echo"))["ics"], link);

        // The panel saves back what it was shown: the link stays; a new one takes its place; null — none.
        let back = json!({ "ics": manifest::SECRET_SHOWN, "days": 5 });
        host.set_settings("echo", back.as_object().unwrap(), None).await.unwrap();
        assert_eq!((stored("ics"), stored("days")), (Some(json!(link)), Some(json!(5))));
        let other = "https://calendar.example/private-4567/basic.ics";
        host.set_settings("echo", json!({ "ics": other }).as_object().unwrap(), None).await.unwrap();
        assert_eq!(stored("ics"), Some(json!(other)));
        host.set_settings("echo", json!({ "ics": null }).as_object().unwrap(), None).await.unwrap();
        assert_eq!(stored("ics"), None);
        assert_eq!(host.view_one("echo").await.unwrap().settings[0].value, None, "not set: nothing shown");
    }

    #[tokio::test]
    async fn keeps_each_kinds_rules_copies_a_built_in_and_lists_the_packages_themes() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::memory());
        let mut with_panel = echo("1.0.0");
        with_panel["contributes"]["panel"] = json!({ "slots": ["home.cards"] });
        write_package(dir.path(), &with_panel, Some(CODE));
        std::fs::write(dir.path().join("echo/1.0.0/panel.js"), "export default {};").unwrap();
        registry::set_installed(&db, "echo", registry::SOURCE_BUNDLED, true, None).unwrap();
        let mut core = echo("1.0.0");
        core["id"] = json!("core");
        core["permissions"] = json!(["voice.intents"]);
        core["contributes"] = json!({ "intents": [{ "phrases": ["ядро"], "handler": "say" }] });
        write_package(dir.path(), &core, Some(CODE));
        registry::set_installed(&db, "core", registry::SOURCE_SYSTEM, true, None).unwrap();
        let theme = json!({
            "schema": 1, "kind": "theme", "id": "forest", "version": "1.0.0", "title": "Лес", "api": "^1",
            "runs": ["pc", "station", "stationServer"],
            "contributes": { "themes": [
                { "id": "forest", "title": "Лес", "layout": "sidebar", "palette": { "dark": { "bg": "#0b1a10" }, "light": { "bg": "#eef7ee" } } },
                { "id": "grid", "title": "Плитки", "layout": "grid", "palette": { "dark": {}, "light": {} } }
            ] }
        });
        write_package(dir.path(), &theme, None);
        let fixture = host_on(dir.path(), db, false, Placement::Pc);
        let host = fixture.host.clone();
        host.enable("core", None).await.unwrap();
        let view = host.enable("echo", None).await.unwrap();
        assert_eq!(
            (view.origin, view.removable, view.can_disable, view.editable),
            (Origin::Bundled, false, true, false)
        );
        assert_eq!(view.panel_module.as_deref(), Some("/panel-plugins/echo/1.0.0/panel.js"));
        assert!(host.panel_file("echo", "1.0.0", "panel.js").is_some());
        assert!(host.panel_file("echo", "1.0.0", "backend.js").is_none(), "only the panel's part is served");
        assert!(host.panel_file("echo", "0.9.0", "panel.js").is_none());

        let system = host.disable("core", None).await.unwrap_err();
        assert!(system.message.contains("системный"), "{}", system.message);
        assert!(!host.view_one("core").await.unwrap().can_disable);

        // Themes: the package's that is in form; a layout of code is the panel's plugin, not a theme.
        let themes = host.themes();
        assert_eq!(themes.len(), 1);
        assert_eq!((themes[0].package.as_str(), themes[0].theme["id"].as_str()), ("forest", Some("forest")));

        // A copy of one's own: the built-in goes off, the copy is on in its place, one's own to change.
        let copy = host.fork("echo", None, None).await.unwrap();
        assert_eq!((copy.id.as_str(), copy.origin, copy.editable, copy.enabled), ("my-echo", Origin::User, true, true));
        assert!(copy.title.contains("моя копия"));
        assert!(!host.view_one("echo").await.unwrap().enabled);
        assert_eq!(said(host.call("my-echo", "say", json!({"word": "к"}), CallKind::Voice).await), "к (1)");
        assert!(host.fork("echo", None, None).await.unwrap_err().message.contains("уже есть"));
        assert!(host.fork("core", None, None).await.unwrap_err().message.contains("Системный"));
    }

    #[tokio::test]
    async fn a_tool_that_acts_plays_at_once_and_a_silent_station_is_answered_by_its_plugin() {
        use crate::core::connectors::StationAct;
        use crate::gateway::protocol::protocol_types::Directive;
        use crate::skills::media::playback::{PlaybackNews, PlaybackReport, Pusher};

        struct Pushed(Mutex<Vec<String>>);
        impl Pusher for Pushed {
            fn push(&self, _device: &str, directive: Directive) -> bool {
                self.0.lock().unwrap().push(directive.name);
                true
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let manifest = json!({
            "schema": 1, "id": "tuner", "version": "1.0.0", "title": "Тюнер", "api": "^1", "runs": ["pc"],
            "permissions": ["llm.tools", "events", "media.source"],
            "contributes": {
                "tools": [{ "name": "tuner_play", "description": "Включает станцию", "handler": "play", "capability": "radio" }],
                "media": [{ "id": "stations", "kind": "radio" }],
                "events": [{ "name": "player.silent", "handler": "silent" }]
            }
        });
        let code = r#"
            const A = { id: "a", title: "Станция А", url: "https://a.example/a.mp3" };
            const B = { id: "b", title: "Станция Б", url: "https://b.example/b.mp3" };
            export function play() { return { say: "Включаю станцию А", play: [A, B] }; }
            export function stations() { return []; }
            export function silent({ payload }) { return payload.retrying ? null : { play: [B] }; }
        "#;
        write_package(dir.path(), &manifest, Some(code));
        let player = Arc::new(MediaPlayer::new());
        PLAYER.with(|cell| *cell.borrow_mut() = Some(player.clone()));
        let watch = PlaybackWatch::with(player.clone(), Duration::from_secs(3600));
        let pushed = Arc::new(Pushed(Mutex::new(Vec::new())));
        watch.set_pusher(pushed.clone());
        let fixture = host_playing(
            dir.path(),
            Arc::new(Db::memory()),
            false,
            Placement::Pc,
            crate::plugins::budget::fake::plenty(),
            Some(watch.clone()),
        );
        let host = fixture.host.clone();
        host.enable("tuner", None).await.unwrap();

        // The model calls the tool inside the station's answer: it plays at once, the model reads what is on.
        let added: Arc<Mutex<Vec<Directive>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = added.clone();
        let station = StationAct {
            device: "kitchen".into(),
            media_base_url: None,
            add: Arc::new(move |directives| {
                sink.lock().unwrap().extend(directives);
                true
            }),
        };
        let tool = host.tool_contributions().into_iter().find(|tool| tool.tool.name == "tuner_play").unwrap();
        assert_eq!(tool.permission, "use:radio");
        let context = ToolContext { principal: system(), cancel: CancellationToken::new(), station: Some(station) };
        let said = (tool.run)(Map::new(), context).await.unwrap();
        assert!(said.contains("Playing «Станция А» now") && said.contains("Включаю станцию А"), "{said}");
        assert_eq!(added.lock().unwrap()[0].name, "audio_play");
        // Without a station (the panel's check) nothing plays.
        let check = ToolContext { principal: system(), cancel: CancellationToken::new(), station: None };
        assert!((tool.run)(Map::new(), check).await.unwrap().contains("nothing played"));

        // The station kept silent on it, through the gateway too: the plugin answers with the next station.
        let (item, state) = player.current("kitchen").unwrap();
        host.player_news(&PlaybackReport {
            device: "kitchen".into(),
            play_id: state.play_id.clone(),
            request: None,
            item,
            news: PlaybackNews::Silent { seconds: 5, retrying: false },
            via_gateway: true,
        });
        for _ in 0..50 {
            if !pushed.0.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(pushed.0.lock().unwrap().as_slice(), ["vgw_media_retry"]);
        assert_eq!(player.current("kitchen").unwrap().0.title, "Станция Б");
    }

    #[tokio::test]
    async fn runs_a_plugin_that_prefers_home_on_the_computer_and_here_only_when_it_fits() {
        use crate::plugins::budget::MemoryNow;
        use axum::routing::post;

        struct Home(Mutex<Option<(String, String)>>);
        impl HomeLink for Home {
            fn up(&self) -> Option<(String, String)> {
                self.0.lock().unwrap().clone()
            }
        }

        // The computer: its copy of the plugin answers.
        let computer = axum::Router::new().route(
            "/home/v1/plugins/{id}/call",
            post(|axum::extract::Path(id): axum::extract::Path<String>, body: axum::Json<Value>| async move {
                assert_eq!(body["function"], "say");
                axum::Json(json!({ "result": format!("{id} с компьютера") }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, computer).await.unwrap() });

        let dir = tempfile::tempdir().unwrap();
        let mut heavy = echo("1.0.0");
        heavy["prefer"] = json!("home");
        heavy["budget"] = json!({ "memoryMb": 30 });
        write_package(dir.path(), &heavy, Some(CODE));
        let home = Arc::new(Home(Mutex::new(Some((url, "key".into())))));
        HOME.with(|cell| *cell.borrow_mut() = Some(home.clone() as Arc<dyn HomeLink>));
        PLAYER.with(|cell| *cell.borrow_mut() = Some(Arc::new(MediaPlayer::new())));
        let memory = crate::plugins::budget::fake::plenty();
        let fixture =
            host_playing(dir.path(), Arc::new(Db::memory()), false, Placement::StationServer, memory.clone(), None);
        let host = fixture.host.clone();
        host.enable("echo", None).await.unwrap();
        host.unload_idle_for(0);
        assert_eq!(said(host.call("echo", "say", json!({"word": "а"}), CallKind::Voice).await), "echo с компьютера");
        assert!(!host.view_one("echo").await.unwrap().loaded, "nothing of it loads on the station");

        // The computer is away: here, when it fits.
        *home.0.lock().unwrap() = None;
        assert_eq!(said(host.call("echo", "say", json!({"word": "б"}), CallKind::Voice).await), "б (1)");
        host.unload_idle_for(0);
        // It does not fit: unavailable until the computer is back.
        memory.set(MemoryNow { total_mb: 238, available_mb: 50, pressure: Pressure::Normal, gateway_mb: Some(14) });
        let refused = host.call("echo", "say", json!({"word": "в"}), CallKind::Voice).await.unwrap_err();
        assert!(refused.to_string().contains("недоступно, пока компьютер не на связи"), "{refused}");
        HOME.with(|cell| *cell.borrow_mut() = None);
    }
}
