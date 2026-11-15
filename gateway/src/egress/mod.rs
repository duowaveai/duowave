//! «Выход в интернет» (docs/plans/own-server.md, этап 2): who of the gateway goes to the internet directly and who
//! through which of the person's own servers. The servers are an app's (the MagicPlace app «VPN»: its program keeps
//! the connections and gives each a local entry); the routes are the gateway's.
//!
//! - `registry` — who goes: the table gathers itself from what stands in the home.
//! - `rules` — where each row goes: three buttons, a row's own route, «Авто» by the home's country and the catalog's
//!   data; what a row does when its server is down.
//! - `connections` — the app's connections, by its letter `system/network` → `connections` (the owner's «да» to
//!   «VPN → Сеть шлюза» first; until then everything goes directly).
//! - `reach` — where services work from: the catalog's `reach.json`.
//! - `app_entry` — the gateway's entry for the apps' programs (`HTTPS_PROXY`) and the refusal of a call whose server
//!   is down.
//! - `routes` — the panel's screen «Выход в интернет».
//!
//! Where a decision applies: every reqwest client (`utils::http::client_builder`, `proxy_for` on each new connection),
//! Alice's talk with Yandex (`gateway/yandex/yandex_session.rs`, `alice`), the apps' programs (`apps/supervisor.rs`,
//! `entry_for`). Never through a server: this machine, the home network, the computer and the stations.
//!
//! The table is rebuilt when something it reads changes (a setting, a package, a provider, a connection's letter) and
//! every 15 s; the decisions are published for the HTTP clients in one snapshot (`TABLE`), read on each new
//! connection without a lock held across an await.

pub mod app_entry;
pub mod connections;
pub mod reach;
pub mod registry;
pub mod routes;
pub mod rules;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::config::{Config, DialogProvider};
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::plugins::controller::PluginHost;
use crate::plugins::manifest::{Manifest, Permission};
use crate::secrets::SecretsService;
use crate::settings::definitions::{
    EGRESS_CONNECTION, EGRESS_CONNECTIONS, EGRESS_CUSTOM, EGRESS_LEARNED, EGRESS_MODE, EGRESS_ON_DOWN, EGRESS_ROUTE,
    EGRESS_SEEN,
};
use crate::settings::SettingsService;
use crate::storage::Db;
use crate::utils::time::now_iso;

use app_entry::Hop;
use connections::{Connection, Connections};
use reach::ReachData;
use registry::{Goer, Sources};
use rules::{Context, Decision, Learned, Mode, OnDown, Route, RowRule, Why};

/// The bus's participant the connections come to.
pub const NETWORK: &str = "system/network";

/// One row as decided.
#[derive(Debug, Clone)]
pub struct Row {
    pub goer: Goer,
    pub decision: Decision,
    pub why: Why,
    pub route: Route,
    pub on_down: OnDown,
    /// The row set its own «если сервер не отвечает».
    pub on_down_own: bool,
    pub learned: Option<Learned>,
    pub reach: Option<rules::Reach>,
    /// Not seen in the panel yet.
    pub new: bool,
}

/// The decisions at one moment.
#[derive(Debug, Clone, Default)]
pub struct Table {
    pub mode: Mode,
    /// The home's country; None — not asked or «Не скажу».
    pub home: Option<String>,
    pub preferred: Option<String>,
    pub rows: Vec<Row>,
    pub connections: Vec<Connection>,
    /// Some connection was ever reported (by an app that is on).
    pub any_reported: bool,
    pub reach_updated: Option<String>,
}

impl Table {
    /// The row of a host: the row that names it, else «Остальное».
    pub fn row_for_host(&self, host: &str) -> Option<&Row> {
        let goers: Vec<Goer> = self.rows.iter().map(|row| row.goer.clone()).collect();
        let id = registry::row_of(&goers, host).map(|goer| goer.id.clone()).unwrap_or_else(|| registry::OTHER.into());
        self.row(&id)
    }

    pub fn row(&self, id: &str) -> Option<&Row> {
        self.rows.iter().find(|row| row.goer.id == id)
    }

    fn entry_of(&self, connection: &str) -> Option<url::Url> {
        self.connections.iter().find(|one| one.id == connection).and_then(|one| one.url.clone())
    }

    /// Where a decision sends a call.
    pub fn hop(&self, decision: &Decision) -> Hop {
        match decision {
            Decision::Direct | Decision::Need { wait: false, .. } => Hop::Direct,
            Decision::Via { connection } => match self.entry_of(connection) {
                Some(entry) => Hop::Via(entry),
                None => Hop::Refuse(crate::tr!("VPN не отвечает")),
            },
            Decision::Down { .. } => Hop::Refuse(crate::tr!("VPN не отвечает")),
            Decision::Need { .. } => Hop::Refuse(crate::tr!("Нужно подключение VPN в стране, где сервис работает")),
        }
    }
}

static TABLE: RwLock<Option<Arc<Table>>> = RwLock::new(None);
/// The entry's port; 0 — not running.
static ENTRY: AtomicU16 = AtomicU16::new(0);
static SERVICE: OnceLock<Weak<Egress>> = OnceLock::new();

fn published() -> Option<Arc<Table>> {
    TABLE.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

/// The proxy of a request, for reqwest's `Proxy::custom` (utils/http.rs): None — directly (or the system's proxy, as
/// before); a connection's entry; or the gateway's entry with the user `down` — the call is refused on this machine.
pub fn proxy_for(url: &url::Url) -> Option<url::Url> {
    let host = url.host_str()?;
    if registry::is_local(host) {
        return None;
    }
    let Some(table) = published() else { return env_proxy(url) };
    let Some(row) = table.row_for_host(host) else { return env_proxy(url) };
    match table.hop(&row.decision) {
        Hop::Direct => env_proxy(url),
        Hop::Via(entry) => Some(entry),
        Hop::Refuse(_) => Some(refusal(row.decision.connection().unwrap_or("none"))),
    }
}

/// The address that refuses: the gateway's entry with the user `down`; no entry — a closed port on this machine.
fn refusal(connection: &str) -> url::Url {
    let port = ENTRY.load(Ordering::Relaxed);
    let text = if port == 0 {
        "http://127.0.0.1:9".to_string()
    } else {
        format!("http://{}:{connection}@127.0.0.1:{port}", app_entry::DOWN)
    };
    url::Url::parse(&text).unwrap_or_else(|_| url::Url::parse("http://127.0.0.1:9").expect("a literal URL"))
}

/// The system's proxy a direct call keeps using, as before the routes: `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY`
/// (either case) unless `NO_PROXY` names the host.
fn env_proxy(url: &url::Url) -> Option<url::Url> {
    let var = |names: &[&str]| names.iter().find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()));
    let host = url.host_str()?.to_ascii_lowercase();
    if let Some(no) = var(&["NO_PROXY", "no_proxy"]) {
        let skip = no.split(',').map(str::trim).filter(|entry| !entry.is_empty()).any(|entry| {
            entry == "*" || {
                let entry = entry.trim_start_matches('.').trim_start_matches("*.").to_ascii_lowercase();
                host == entry || host.ends_with(&format!(".{entry}"))
            }
        });
        if skip {
            return None;
        }
    }
    let proxy = if url.scheme() == "https" {
        var(&["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"])
    } else {
        var(&["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"])
    }?;
    url::Url::parse(&proxy).ok()
}

/// Alice's talk with Yandex: where it goes now.
pub fn alice() -> Hop {
    let Some(table) = published() else { return Hop::Direct };
    match table.row(registry::ALICE) {
        Some(row) => table.hop(&row.decision),
        None => Hop::Direct,
    }
}

/// Whether a provider's calls are refused now because its server does not answer: «Мозг» then answers by Alice or in
/// honest words rather than wait out a timeout (dialog/dialog_service.rs). By the provider's host.
pub fn refused_host(host: &str) -> bool {
    published().and_then(|table| table.row_for_host(host).map(|row| row.decision.refuses())).unwrap_or(false)
}

/// Whether a row's calls are refused now (its server does not answer, or no connection fits and it has a key): the
/// station's session gives a question for such a model to Alice in «Вместе» (gateway/uniproxy/station_session.rs).
pub fn refused_row(id: &str) -> bool {
    published().and_then(|table| table.row(id).map(|row| row.decision.refuses())).unwrap_or(false)
}

/// The proxy address for an app's program (`HTTPS_PROXY`), with its id and token: None when the entry does not run,
/// or for the app that gives the connections (its own subscriptions go out by themselves).
pub fn entry_for(id: &str, token: &str, manifest: &Manifest) -> Option<String> {
    let port = ENTRY.load(Ordering::Relaxed);
    if port == 0 || gives_connections(manifest) {
        return None;
    }
    Some(format!("http://{id}:{token}@127.0.0.1:{port}"))
}

/// The app has the right to give the gateway its connections.
fn gives_connections(manifest: &Manifest) -> bool {
    manifest.permissions.iter().any(|permission| match permission {
        Permission::Bus { level, pattern } => *level >= crate::bus::Level::Call && pattern.to_string() == NETWORK,
        Permission::Net(_) => false,
    })
}

/// The service.
pub struct Egress {
    config: Arc<Config>,
    db: Arc<Db>,
    settings: Arc<SettingsService>,
    secrets: Arc<SecretsService>,
    events: EventBus,
    catalog_dir: PathBuf,
    plugins: OnceLock<Weak<PluginHost>>,
    connections: Connections,
    dirty: AtomicBool,
    table: RwLock<Arc<Table>>,
}

/// How often the table is rebuilt with nothing said: a connection not heard of goes down within this.
const TICK: Duration = Duration::from_secs(15);

impl Egress {
    pub fn new(
        config: Arc<Config>,
        db: Arc<Db>,
        settings: Arc<SettingsService>,
        secrets: Arc<SecretsService>,
        events: EventBus,
    ) -> Arc<Egress> {
        let kept = settings.get(&EGRESS_CONNECTIONS, None).unwrap_or_default();
        let egress = Arc::new(Egress {
            catalog_dir: config.plugins.catalog_dir.clone(),
            config,
            db,
            settings,
            secrets,
            events,
            plugins: OnceLock::new(),
            connections: Connections::from_kept(kept),
            dirty: AtomicBool::new(true),
            table: RwLock::new(Arc::new(Table::default())),
        });
        let _ = SERVICE.set(Arc::downgrade(&egress));
        egress
    }

    pub fn set_plugins(&self, plugins: &Arc<PluginHost>) {
        let _ = self.plugins.set(Arc::downgrade(plugins));
    }

    fn plugins(&self) -> Option<Arc<PluginHost>> {
        self.plugins.get().and_then(Weak::upgrade)
    }

    /// The table now, decided at once (the first decisions are there before any call goes out), the entry for the
    /// apps' programs, and the rebuilds on every change.
    pub async fn start(self: &Arc<Self>) {
        self.rebuild();
        let me = Arc::downgrade(self);
        let decide: app_entry::Decide = Arc::new(move |user, password, host| match me.upgrade() {
            Some(egress) => egress.entry_hop(user, password, host),
            None => Hop::Refuse(crate::tr!("шлюз останавливается")),
        });
        let me = Arc::downgrade(self);
        let failed: app_entry::Failed = Arc::new(move |user, host, why| {
            if let Some(egress) = me.upgrade() {
                egress.note_failure(user, host, why);
            }
        });
        match app_entry::start(decide, failed).await {
            Ok(port) => {
                ENTRY.store(port, Ordering::Relaxed);
                tracing::info!(target: "egress", "the apps' entry on 127.0.0.1:{port}");
            }
            Err(error) => tracing::warn!(target: "egress", "the apps' entry did not start: {error}"),
        }
        let me = Arc::downgrade(self);
        let mut events = self.events.subscribe();
        tokio::spawn(async move {
            let mut last = Instant::now();
            loop {
                let woke = tokio::time::timeout(Duration::from_secs(1), events.recv()).await;
                let Some(egress) = me.upgrade() else { return };
                if let Ok(Ok(event)) = woke {
                    if matters(&event) {
                        egress.dirty.store(true, Ordering::Relaxed);
                    }
                }
                if egress.dirty.swap(false, Ordering::Relaxed) || last.elapsed() >= TICK {
                    last = Instant::now();
                    let egress = egress.clone();
                    let _ = tokio::task::spawn_blocking(move || egress.rebuild()).await;
                }
            }
        });
    }

    /// The table as decided last.
    pub fn table(&self) -> Arc<Table> {
        self.table.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// What the table is gathered from, read now.
    fn sources(&self) -> Sources {
        let mut sources = Sources { alice_url: Some(self.config.gateway.yandex_url.clone()), ..Sources::default() };
        let providers: Vec<(String, String, String, Option<String>)> = self
            .db
            .with(|c| {
                let mut statement = c.prepare("SELECT id, title, preset, base_url FROM llm_providers ORDER BY id")?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
                rows.collect()
            })
            .unwrap_or_default();
        sources.providers = providers;
        let dialog = &self.config.dialog;
        let env = match dialog.provider {
            DialogProvider::Anthropic if dialog.anthropic_key.is_some() || dialog.api_key.is_some() => {
                Some(("claude", dialog.anthropic_base_url.clone().or_else(|| dialog.base_url.clone())))
            }
            DialogProvider::OpenAi if dialog.api_key.is_some() => Some(("openai", dialog.base_url.clone())),
            _ => None,
        };
        if let Some((preset, base_url)) = env {
            sources.providers.push(("env".into(), crate::tr!("Модель из .env"), preset.into(), base_url));
        }
        // ElevenLabs by the owner's key (capabilities/voices.rs, `eleven::KEY`; the module is the computer's only).
        if self.secrets.get("voice:elevenlabs:key").is_some() {
            sources.voices.push(("voice:elevenlabs".into(), "ElevenLabs".into(), "https://api.elevenlabs.io".into()));
        }
        if self.config.speech.asr.enabled {
            sources.voices.push((
                "hearing:asr".into(),
                crate::tr!("Слух: распознавание речи"),
                self.config.speech.asr.base_url.clone(),
            ));
        }
        if let Some(own) = &self.config.gateway.tts.own {
            sources.voices.push(("voice:own".into(), crate::tr!("Голос: синтез речи"), own.base_url.clone()));
        }
        let mcp: Vec<(String, String, String)> = self
            .db
            .with(|c| {
                let mut statement =
                    c.prepare("SELECT id, title, config FROM mcp_servers WHERE enabled = 1 ORDER BY id")?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
                rows.collect()
            })
            .unwrap_or_default();
        sources.mcp = mcp
            .into_iter()
            .map(|(id, title, config)| {
                let url = serde_json::from_str::<Value>(&config)
                    .ok()
                    .and_then(|config| config.get("url").and_then(Value::as_str).map(str::to_string));
                (id, title, url)
            })
            .collect();
        if let Some(plugins) = self.plugins() {
            for package in plugins.installed_all() {
                let manifest = &package.manifest;
                if gives_connections(manifest) {
                    continue;
                }
                let hosts: Vec<String> = manifest
                    .permissions
                    .iter()
                    .filter_map(|permission| match permission {
                        Permission::Net(host) => Some(host.to_ascii_lowercase()),
                        Permission::Bus { .. } => None,
                    })
                    .collect();
                let program = matches!(
                    manifest.runtime,
                    crate::plugins::manifest::Runtime::Process | crate::plugins::manifest::Runtime::Node
                );
                sources.apps.push((manifest.id.clone(), manifest.title.clone(), hosts, program));
            }
        }
        sources.custom = self.settings.get(&EGRESS_CUSTOM, None).unwrap_or_default();
        sources
    }

    /// Gathers the table and decides each row; publishes the decisions.
    pub fn rebuild(&self) {
        // The connections of an app switched off or removed go: the app off — everything directly.
        if let Some(plugins) = self.plugins() {
            if self.connections.keep_apps(|app| plugins.is_switched_on(app)) {
                self.keep_connections();
            }
        }
        let goers = registry::gather(&self.sources());
        let reach = ReachData::load(&self.catalog_dir);
        let mode = self.settings.get(&EGRESS_MODE, None).as_deref().and_then(Mode::parse).unwrap_or_default();
        let home = crate::locale::country::household(&self.settings);
        let preferred = self.settings.get(&EGRESS_CONNECTION, None);
        let connections = self.connections.list();
        let any_reported = !connections.is_empty();
        let routes = self.settings.all_for_apps(&EGRESS_ROUTE);
        let on_downs = self.settings.all_for_apps(&EGRESS_ON_DOWN);
        let learned = self.settings.all_for_apps(&EGRESS_LEARNED);
        let seen = self.settings.get(&EGRESS_SEEN, None);
        let context = Context {
            mode,
            home: home.as_deref(),
            connections: &connections,
            preferred: preferred.as_deref(),
            any_reported,
        };
        let rows = goers
            .into_iter()
            .map(|goer| {
                let rule = RowRule {
                    route: routes.get(&goer.id).and_then(|text| Route::parse(text)).unwrap_or_default(),
                    on_down: on_downs.get(&goer.id).and_then(|text| OnDown::parse(text)),
                    learned: learned.get(&goer.id).cloned(),
                };
                let found = reach.of(&goer);
                let (decision, why) = rules::decide(&goer, found.as_ref(), &rule, &context);
                // A fresh gateway has seen nothing: the first look marks all as seen (routes.rs), not «новое» each.
                let new = seen.as_ref().is_some_and(|seen| !seen.contains(&goer.id));
                Row {
                    on_down: rule.on_down.unwrap_or_else(|| OnDown::default_for(goer.section)),
                    on_down_own: rule.on_down.is_some(),
                    route: rule.route,
                    learned: rule.learned,
                    reach: found,
                    goer,
                    decision,
                    why,
                    new,
                }
            })
            .collect();
        let table =
            Arc::new(Table { mode, home, preferred, rows, connections, any_reported, reach_updated: reach.updated_at });
        *self.table.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = table.clone();
        *TABLE.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(table);
    }

    fn keep_connections(&self) {
        if let Err(error) = self.settings.set(&EGRESS_CONNECTIONS, &self.connections.kept(), None) {
            tracing::warn!(target: "egress", "the connections are not kept: {}", error.message);
        }
    }

    /// An app's letter `system/network` → `connections`.
    pub fn report(&self, app: &str, input: &Value) -> Result<Value, String> {
        let before = self.connections.kept();
        let changed = self.connections.report(app, input)?;
        if self.connections.kept() != before {
            self.keep_connections();
        }
        if changed {
            self.rebuild();
        }
        let table = self.table();
        Ok(serde_json::json!({ "mode": table.mode, "home": table.home }))
    }

    /// Where a call of the apps' entry goes: `down` — refused; an app by its token — by its row.
    fn entry_hop(&self, user: &str, password: &str, host: &str) -> Hop {
        if user == app_entry::DOWN {
            return Hop::Refuse(crate::tr!("VPN не отвечает"));
        }
        let known = self.plugins().and_then(|plugins| plugins.app_host(password)).is_some_and(|(id, _)| id == user);
        if !known {
            return Hop::Refuse(crate::tr!("нет такого приложения: токен не тот"));
        }
        if registry::is_local(host) {
            return Hop::Direct;
        }
        let table = self.table();
        match table.row(user).or_else(|| table.row(registry::OTHER)) {
            Some(row) => table.hop(&row.decision),
            None => Hop::Direct,
        }
    }

    /// A direct call of a row did not open (a timeout, a reset, 403, 451): «Авто» without the catalog's data learns
    /// that the service does not work from here, and the row goes through a server from now on. Only where it
    /// matters: «Где не работает», a home country, a connection to go through.
    pub fn note_failure(&self, row: &str, host: &str, why: &str) {
        if !matches!(why, "timeout" | "reset" | "403" | "451") {
            return;
        }
        let table = self.table();
        if table.mode != Mode::Blocked || table.home.is_none() || table.connections.is_empty() {
            return;
        }
        let row = table.row(row).or_else(|| table.row_for_host(host));
        let Some(row) = row.filter(|row| row.decision == Decision::Direct && row.why == Why::NoData) else { return };
        if row.goer.keyed {
            return;
        }
        let learned = Learned { blocked: true, why: why.to_string(), at: now_iso() };
        tracing::info!(target: "egress", "{}: did not open directly ({why}); through a server from now on", row.goer.id);
        if self.settings.set_for_app(&EGRESS_LEARNED, &row.goer.id, Some(&learned), None).is_ok() {
            self.rebuild();
        }
    }

    // The panel's changes (routes.rs): each rebuilds the table at once.

    pub fn set_mode(&self, mode: Mode, actor: Option<Actor>) -> Result<(), crate::platform::http::ApiError> {
        let text = match mode {
            Mode::Off => "off",
            Mode::Blocked => "blocked",
            Mode::All => "all",
        };
        self.settings.set(&EGRESS_MODE, &text.to_string(), actor)?;
        self.rebuild();
        Ok(())
    }

    pub fn set_preferred(&self, id: Option<&str>, actor: Option<Actor>) -> Result<(), crate::platform::http::ApiError> {
        match id {
            Some(id) => self.settings.set(&EGRESS_CONNECTION, &id.to_string(), actor)?,
            None => self.settings.forget(&EGRESS_CONNECTION)?,
        }
        self.rebuild();
        Ok(())
    }

    pub fn set_row(
        &self,
        id: &str,
        route: Option<Option<Route>>,
        on_down: Option<Option<OnDown>>,
        actor: Option<Actor>,
    ) -> Result<(), crate::platform::http::ApiError> {
        if let Some(route) = route {
            let text = route.filter(|route| *route != Route::Auto).map(|route| route.as_text().to_string());
            self.settings.set_for_app(&EGRESS_ROUTE, id, text.as_ref(), actor.clone())?;
        }
        if let Some(on_down) = on_down {
            let text = on_down.map(|on_down| match on_down {
                OnDown::Wait => "wait".to_string(),
                OnDown::Direct => "direct".to_string(),
            });
            self.settings.set_for_app(&EGRESS_ON_DOWN, id, text.as_ref(), actor)?;
        }
        self.rebuild();
        Ok(())
    }

    /// «Вернуть как было»: every row's own route and «если сервер не отвечает» gone; what the home learned stays.
    pub fn reset_rows(&self, actor: Option<Actor>) -> Result<(), crate::platform::http::ApiError> {
        for (id, _) in self.settings.all_for_apps(&EGRESS_ROUTE) {
            self.settings.set_for_app::<String>(&EGRESS_ROUTE, &id, None, actor.clone())?;
        }
        for (id, _) in self.settings.all_for_apps(&EGRESS_ON_DOWN) {
            self.settings.set_for_app::<String>(&EGRESS_ON_DOWN, &id, None, actor.clone())?;
        }
        self.rebuild();
        Ok(())
    }

    /// Whether any row has its own rules: the panel says «свои правила».
    pub fn has_own_rules(&self) -> bool {
        !self.settings.all_for_apps(&EGRESS_ROUTE).is_empty() || !self.settings.all_for_apps(&EGRESS_ON_DOWN).is_empty()
    }

    pub fn set_custom(&self, hosts: Vec<String>, actor: Option<Actor>) -> Result<(), crate::platform::http::ApiError> {
        self.settings.set(&EGRESS_CUSTOM, &hosts, actor)?;
        self.rebuild();
        Ok(())
    }

    pub fn custom(&self) -> Vec<String> {
        self.settings.get(&EGRESS_CUSTOM, None).unwrap_or_default()
    }

    /// The rows seen in the panel: «новое» off them.
    pub fn mark_seen(&self) -> Result<(), crate::platform::http::ApiError> {
        let ids: Vec<String> = self.table().rows.iter().map(|row| row.goer.id.clone()).collect();
        self.settings.set(&EGRESS_SEEN, &ids, None)?;
        self.rebuild();
        Ok(())
    }

    /// The app that gives the connections is installed: the panel offers its window.
    pub fn connections_app(&self) -> Option<String> {
        let plugins = self.plugins()?;
        plugins
            .installed_all()
            .into_iter()
            .find(|package| gives_connections(&package.manifest))
            .map(|package| package.manifest.id.clone())
    }

    /// For the panel: the app's own word on what the rows learned, by id.
    pub fn learned(&self) -> BTreeMap<String, Learned> {
        self.settings.all_for_apps(&EGRESS_LEARNED)
    }
}

/// An event that may change the table.
fn matters(event: &GatewayEvent) -> bool {
    match event {
        GatewayEvent::SettingsChanged { key, .. } => {
            !key.starts_with("egress.") || key == "egress.custom" || key == "egress.mode"
        }
        _ => {
            let name = event.name();
            name.starts_with("packages.")
                || name.starts_with("plugins.")
                || name.starts_with("mcp.")
                || name.starts_with("brain.")
        }
    }
}

/// The service once it stands: for the hooks that only learn (a plugin's fetch).
pub fn service() -> Option<Arc<Egress>> {
    SERVICE.get().and_then(Weak::upgrade)
}

#[cfg(test)]
mod tests;
