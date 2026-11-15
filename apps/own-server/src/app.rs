//! The program's state and work: the connections with their servers, the engine's settings built from them, the checks
//! (is the chosen server alive, which is current, its country), the subscriptions, and the functions the window and
//! the gateway call. The window's calls must answer within the gateway's 2 s: anything longer (reading a subscription,
//! a check through a far server) starts here and finishes in the background — the window asks `status` again.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::config::{self, Access, Entrance};
use crate::engine::Engine;
use crate::gateway::Gateway;
use crate::i18n::t;
use crate::links::{self, Server, Skip, Source};
use crate::net::{self, Ask, Via};
use crate::store::{self, Connection, Learned, State, Store};

pub const MAX_CONNECTIONS: usize = 8;
const SUBSCRIPTION_EVERY: Duration = Duration::from_secs(6 * 3600);
const CHECK_EVERY: Duration = Duration::from_secs(60);
const ALL_SERVERS_EVERY: Duration = Duration::from_secs(600);
const TRACE_URL: &str = "https://www.cloudflare.com/cdn-cgi/trace";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Starting,
    Up,
    Down,
}

impl Health {
    fn word(self) -> &'static str {
        match self {
            Health::Starting => "starting",
            Health::Up => "up",
            Health::Down => "down",
        }
    }
}

/// A connection as the program runs it.
#[derive(Debug, Clone)]
pub struct Live {
    pub connection: Connection,
    pub servers: Vec<Server>,
    pub skipped: Vec<Skip>,
    /// For a subscription: its host, whether it is being read, the last error's detail.
    pub subscription: Option<SubscriptionView>,
    pub health: Health,
    /// The server in use now (the chosen or the fastest).
    pub current: Option<String>,
    /// Server name → its delay: Some(ms) answered, None did not; absent — not probed yet.
    pub delays: BTreeMap<String, Option<u64>>,
    pub delays_at: u64,
}

#[derive(Debug, Clone, Default)]
pub struct SubscriptionView {
    pub url: String,
    pub reading: bool,
    /// The last reading's failure: a key of the dictionaries and its detail.
    pub error: Option<(String, String)>,
}

pub struct App {
    pub store: Store,
    pub gateway: Gateway,
    pub engine: Arc<Engine>,
    pub access: Access,
    config_path: PathBuf,
    inner: Mutex<Inner>,
    /// Something people or the gateway see changed: the letter goes now, not in 30 s.
    pub changed: Notify,
    /// Run the checks now.
    pub recheck: Notify,
    /// The engine's settings are rebuilt one at a time.
    building: tokio::sync::Mutex<()>,
}

struct Inner {
    lives: Vec<Live>,
    state: State,
    /// The last letter's items: a change is sent at once.
    sent: Option<Vec<Value>>,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").and_then(|listener| listener.local_addr()).map(|a| a.port()).unwrap_or(0)
}

fn bindable(port: u16) -> bool {
    port != 0 && std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

impl Live {
    fn of(connection: Connection, learned: Option<&Learned>) -> Live {
        let mut live = Live {
            connection,
            servers: Vec::new(),
            skipped: Vec::new(),
            subscription: None,
            health: Health::Starting,
            current: None,
            delays: BTreeMap::new(),
            delays_at: 0,
        };
        match links::classify(&live.connection.source) {
            Source::Inline(text) => {
                let (servers, skipped) = links::parse_text(&text);
                live.servers = servers;
                live.skipped = skipped;
            }
            Source::Subscription(url) => {
                if let Some(kept) = learned.and_then(|learned| learned.subscription.as_ref()) {
                    let (servers, skipped) = links::parse_text(&kept.body);
                    live.servers = servers;
                    live.skipped = skipped;
                }
                live.subscription = Some(SubscriptionView { url, ..SubscriptionView::default() });
            }
        }
        live
    }
}

impl App {
    pub fn new(data: PathBuf, gateway: Gateway) -> Arc<App> {
        let store = Store::new(&data);
        let mut state = store.state();
        let connections = store.connections();
        // The engine is not running yet: the remembered ports are checked, a taken one replaced.
        if !bindable(state.clash_port) {
            state.clash_port = free_port();
        }
        for connection in &connections {
            let learned = state.connections.entry(connection.id.clone()).or_default();
            if !bindable(learned.port) {
                learned.port = free_port();
            }
        }
        state.connections.retain(|id, _| connections.iter().any(|one| &one.id == id));
        let _ = store.save_state(&state);
        let lives = connections.into_iter().map(|one| {
            let learned = state.connections.get(&one.id).cloned();
            Live::of(one, learned.as_ref())
        });
        let access = Access {
            user: "duowave".into(),
            password: store::secret(),
            clash_port: state.clash_port,
            clash_secret: store::secret(),
        };
        let config_path = data.join("shoes.yaml");
        Arc::new(App {
            engine: Engine::new(config_path.clone()),
            store,
            gateway,
            access,
            config_path,
            inner: Mutex::new(Inner { lives: lives.collect(), state, sent: None }),
            changed: Notify::new(),
            recheck: Notify::new(),
            building: tokio::sync::Mutex::new(()),
        })
    }

    fn entrances(inner: &Inner) -> Vec<Entrance> {
        inner
            .lives
            .iter()
            .map(|live| {
                let learned = inner.state.connections.get(&live.connection.id);
                Entrance {
                    id: live.connection.id.clone(),
                    port: learned.map(|learned| learned.port).unwrap_or(0),
                    servers: live.servers.clone(),
                    choice: learned.map(|learned| learned.choice.clone()).unwrap_or_else(|| "auto".into()),
                }
            })
            .collect()
    }

    fn entrance(&self, id: &str) -> Option<Entrance> {
        let inner = self.inner.lock().unwrap();
        App::entrances(&inner).into_iter().find(|one| one.id == id)
    }

    fn via(&self, id: &str) -> Option<Via> {
        let entrance = self.entrance(id)?;
        (!entrance.servers.is_empty() && self.engine.running()).then(|| Via {
            port: entrance.port,
            user: self.access.user.clone(),
            password: self.access.password.clone(),
        })
    }

    /// Whether shoes takes these settings: read and compiled here, as its own dry run does, without a second process.
    async fn accepted(path: &std::path::Path) -> Result<(), String> {
        let configs =
            shoes::config::load_configs(&[path.to_string_lossy().into_owned()]).await.map_err(|e| e.to_string())?;
        shoes::config::compile_configs(configs).await.map(|_| ()).map_err(|e| e.to_string())
    }

    /// The engine's settings again from the connections: checked first; a server the engine refuses is set aside with
    /// the reason, the rest go on.
    pub async fn rebuild(&self) {
        let _one = self.building.lock().await;
        let entrances = App::entrances(&self.inner.lock().unwrap());
        let candidate = self.store.dir().join("shoes.next.yaml");
        let mut refused: Vec<(String, String)> = Vec::new();
        let mut usable = entrances.clone();
        let full = config::yaml(&config::build(&usable, &self.access));
        let first = store::write_private(&candidate, full.as_bytes()).map_err(|e| e.to_string());
        let verdict = match first {
            Ok(()) => App::accepted(&candidate).await,
            Err(error) => Err(error),
        };
        if let Err(error) = verdict {
            eprintln!(
                "own-server: the engine refuses the settings ({}); checking the servers one by one",
                redact(&error)
            );
            let probe = self.store.dir().join("shoes.check.yaml");
            for entrance in &mut usable {
                let mut kept = Vec::new();
                for server in entrance.servers.drain(..) {
                    let one = config::yaml(&config::single(&server, &self.access));
                    let fine =
                        store::write_private(&probe, one.as_bytes()).is_ok() && App::accepted(&probe).await.is_ok();
                    if fine {
                        kept.push(server);
                    } else {
                        eprintln!("own-server: {} «{}» set aside: the engine refuses it", server.scheme, server.name);
                        refused.push((entrance.id.clone(), server.name.clone()));
                    }
                }
                entrance.servers = kept;
            }
            let _ = std::fs::remove_file(&probe);
            let full = config::yaml(&config::build(&usable, &self.access));
            if store::write_private(&candidate, full.as_bytes()).is_err() || App::accepted(&candidate).await.is_err() {
                eprintln!("own-server: the settings are still refused; the engine keeps the ones it has");
                let _ = std::fs::remove_file(&candidate);
                return;
            }
        }
        {
            let mut inner = self.inner.lock().unwrap();
            for (id, name) in &refused {
                if let Some(live) = inner.lives.iter_mut().find(|live| &live.connection.id == id)
                    && let Some(at) = live.servers.iter().position(|server| &server.name == name)
                {
                    let server = live.servers.remove(at);
                    live.skipped.push(Skip {
                        scheme: server.scheme.to_string(),
                        reason: "engine_refused".into(),
                        detail: String::new(),
                    });
                }
            }
        }
        if let Err(error) = std::fs::rename(&candidate, &self.config_path) {
            eprintln!("own-server: the settings file is not written: {error}");
            return;
        }
        if usable.iter().any(|one| !one.servers.is_empty()) {
            self.engine.apply();
        } else {
            self.engine.stop();
            let mut inner = self.inner.lock().unwrap();
            for live in &mut inner.lives {
                live.health = Health::Down;
                live.current = None;
            }
        }
        self.changed.notify_one();
    }

    // The Clash API of the engine.

    fn clash_url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.access.clash_port)
    }

    async fn clash(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        wait: Duration,
    ) -> Result<net::Answer, String> {
        let url = self.clash_url(path);
        let mut ask = match body {
            Some(body) => Ask::json(method, &url, &body),
            None => Ask { method, ..Ask::get(&url) },
        };
        ask = ask.header("authorization", format!("Bearer {}", self.access.clash_secret)).timeout(wait);
        net::send(ask).await
    }

    async fn delay(&self, member: &str, wait_ms: u64) -> Option<u64> {
        let path = format!(
            "/proxies/{member}/delay?timeout={wait_ms}&url={}",
            percent_encoding::utf8_percent_encode(config::PROBE_URL, percent_encoding::NON_ALPHANUMERIC)
        );
        let answer = self.clash("GET", &path, None, Duration::from_millis(wait_ms + 1500)).await.ok()?;
        answer.json()?.get("delay")?.as_u64().filter(|_| answer.status == 200)
    }

    async fn now_of(&self, group: &str) -> Option<String> {
        let answer = self.clash("GET", &format!("/proxies/{group}"), None, Duration::from_secs(2)).await.ok()?;
        answer.json()?.get("now")?.as_str().map(str::to_string)
    }

    /// The person's choices into the engine, after it (re)started or reread its settings: the selectors' `default`
    /// is in the file, but a choice made over the API before stays in the engine and may point at another server now.
    pub async fn apply_choices(&self) {
        for _ in 0..40 {
            if self.clash("GET", "/version", None, Duration::from_millis(500)).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let entrances = App::entrances(&self.inner.lock().unwrap());
        for entrance in entrances.iter().filter(|one| !one.servers.is_empty()) {
            let group = config::choice_group(&entrance.id);
            let body = json!({ "name": entrance.chosen_member() });
            for _ in 0..20 {
                match self.clash("PUT", &format!("/proxies/{group}"), Some(body.clone()), Duration::from_secs(2)).await
                {
                    Ok(answer) if answer.status == 204 || answer.status == 200 => break,
                    // A reload in progress: the group is not there yet.
                    _ => tokio::time::sleep(Duration::from_millis(250)).await,
                }
            }
        }
        self.recheck.notify_one();
    }

    /// The chosen way of each connection: alive or not, which server is in use, its country.
    pub async fn check(&self) {
        let entrances = App::entrances(&self.inner.lock().unwrap());
        for entrance in entrances {
            if entrance.servers.is_empty() || !self.engine.running() {
                continue;
            }
            let choice = config::choice_group(&entrance.id);
            let delay = self.delay(&choice, 5000).await;
            let mut member = self.now_of(&choice).await;
            if member.as_deref() == Some(config::auto_group(&entrance.id).as_str()) {
                member = self.now_of(&config::auto_group(&entrance.id)).await;
            }
            let current =
                member.as_deref().and_then(|member| entrance.server_of(member)).map(|server| server.name.clone());
            if let (Some(name), Some(_)) = (&current, delay) {
                self.learn_country(&entrance.id, name, false).await;
            }
            let mut inner = self.inner.lock().unwrap();
            if let Some(live) = inner.lives.iter_mut().find(|live| live.connection.id == entrance.id) {
                live.health = if delay.is_some() { Health::Up } else { Health::Down };
                if let Some(name) = &current {
                    live.delays.insert(name.clone(), delay);
                }
                live.current = current;
            }
        }
        self.changed.notify_one();
    }

    /// Every server's delay, for the window.
    pub async fn probe_all(&self) {
        let entrances = App::entrances(&self.inner.lock().unwrap());
        for entrance in entrances {
            if entrance.servers.is_empty() || !self.engine.running() {
                continue;
            }
            let path = format!(
                "/group/{}/delay?timeout=5000&url={}",
                config::auto_group(&entrance.id),
                percent_encoding::utf8_percent_encode(config::PROBE_URL, percent_encoding::NON_ALPHANUMERIC)
            );
            let Ok(answer) = self.clash("GET", &path, None, Duration::from_secs(8)).await else { continue };
            let found = answer.json().and_then(|body| body.as_object().cloned()).unwrap_or_default();
            let mut inner = self.inner.lock().unwrap();
            if let Some(live) = inner.lives.iter_mut().find(|live| live.connection.id == entrance.id) {
                for (index, server) in entrance.servers.iter().enumerate() {
                    let delay = found.get(&config::server_group(&entrance.id, index)).and_then(Value::as_u64);
                    live.delays.insert(server.name.clone(), delay);
                }
                live.delays_at = now();
            }
        }
    }

    /// A server's country: its flag, what the trace found before, or the trace through its connection now.
    async fn learn_country(&self, id: &str, server: &str, fresh: bool) -> Option<String> {
        if let Some(code) = crate::country::from_name(server) {
            return Some(code);
        }
        if !fresh {
            let inner = self.inner.lock().unwrap();
            if let Some(code) = inner.state.connections.get(id).and_then(|learned| learned.countries.get(server)) {
                return Some(code.clone());
            }
        }
        let via = self.via(id)?;
        let answer = net::send(Ask::get(TRACE_URL).via(Some(&via)).timeout(Duration::from_secs(8))).await.ok()?;
        let code = crate::country::from_trace(&answer.text())?;
        let mut inner = self.inner.lock().unwrap();
        if let Some(learned) = inner.state.connections.get_mut(id) {
            learned.countries.insert(server.to_string(), code.clone());
            let _ = self.store.save_state(&inner.state);
        }
        Some(code)
    }

    fn country_of(inner: &Inner, id: &str, server: &str) -> Option<String> {
        crate::country::from_name(server)
            .or_else(|| inner.state.connections.get(id).and_then(|learned| learned.countries.get(server).cloned()))
    }

    /// Reads a connection's subscription: directly, then through the connection itself. A good list is kept and
    /// replaces the servers; a bad reading keeps the old ones and says why.
    pub async fn read_subscription(&self, id: &str) {
        let url = {
            let mut inner = self.inner.lock().unwrap();
            let Some(live) = inner.lives.iter_mut().find(|live| live.connection.id == id) else { return };
            let Some(view) = live.subscription.as_mut() else { return };
            view.reading = true;
            view.url.clone()
        };
        let mut result = net::send(Ask::get(&url)).await;
        if result.as_ref().map(|answer| answer.status >= 400).unwrap_or(true)
            && let Some(via) = self.via(id)
        {
            result = net::send(Ask::get(&url).via(Some(&via))).await;
        }
        let outcome = match result {
            Ok(answer) if (200..300).contains(&answer.status) => {
                let body = crate::subscription::decode_body(&answer.text());
                let (servers, skipped) = links::parse_text(&body);
                if servers.is_empty() {
                    Err(("subscription_empty".to_string(), String::new()))
                } else {
                    let title =
                        answer.header("profile-title").and_then(|value| crate::subscription::profile_title(&value));
                    Ok((body, title, servers, skipped))
                }
            }
            Ok(answer) => Err(("subscription_failed".to_string(), format!("HTTP {}", answer.status))),
            Err(error) => Err(("subscription_failed".to_string(), error)),
        };
        let changed = {
            let mut inner = self.inner.lock().unwrap();
            let Some(at) = inner.lives.iter().position(|live| live.connection.id == id) else { return };
            match outcome {
                Ok((body, title, servers, skipped)) => {
                    let names =
                        |list: &[Server]| list.iter().map(|s| (s.name.clone(), s.hop.clone())).collect::<Vec<_>>();
                    let changed = names(&inner.lives[at].servers) != names(&servers);
                    let live = &mut inner.lives[at];
                    live.servers = servers;
                    live.skipped = skipped;
                    if let Some(view) = live.subscription.as_mut() {
                        view.reading = false;
                        view.error = None;
                    }
                    if live.connection.name.trim().is_empty() {
                        live.connection.name = title.clone().unwrap_or_else(|| crate::subscription::host_of(&url));
                        let all: Vec<Connection> = inner.lives.iter().map(|live| live.connection.clone()).collect();
                        let _ = self.store.save_connections(&all);
                    }
                    if let Some(learned) = inner.state.connections.get_mut(id) {
                        learned.subscription = Some(store::Subscription { body, title, at: now() });
                    }
                    let _ = self.store.save_state(&inner.state);
                    changed
                }
                Err(error) => {
                    eprintln!(
                        "own-server: the subscription of «{}» ({}) is not read: {} {}",
                        inner.lives[at].connection.name,
                        crate::subscription::host_of(&url),
                        error.0,
                        redact(&error.1)
                    );
                    if let Some(view) = inner.lives[at].subscription.as_mut() {
                        view.reading = false;
                        view.error = Some(error);
                    }
                    false
                }
            }
        };
        if changed {
            self.rebuild().await;
        }
        self.changed.notify_one();
    }

    pub fn subscriptions(&self) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        inner.lives.iter().filter(|live| live.subscription.is_some()).map(|live| live.connection.id.clone()).collect()
    }

    /// The letter's items: each connection's entrance and state.
    pub fn items(&self) -> Vec<Value> {
        let inner = self.inner.lock().unwrap();
        let running = self.engine.running();
        inner
            .lives
            .iter()
            .map(|live| {
                let id = &live.connection.id;
                let port = inner.state.connections.get(id).map(|learned| learned.port).unwrap_or(0);
                let up = running && live.health == Health::Up && !live.servers.is_empty();
                json!({
                    "id": id,
                    "name": live.connection.name,
                    "country": live.current.as_deref().and_then(|server| App::country_of(&inner, id, server)),
                    "url": format!("http://{}:{}@127.0.0.1:{port}", self.access.user, self.access.password),
                    "state": if up { "up" } else { "down" },
                    "current": live.current,
                })
            })
            .collect()
    }

    /// The letter now when the items differ from the last one sent, or `always` (the 30 s «alive»).
    pub async fn tell_gateway(&self, always: bool) {
        let items = self.items();
        let same = self.inner.lock().unwrap().sent.as_ref() == Some(&items);
        if same && !always {
            return;
        }
        if self.gateway.connections(items.clone(), Duration::from_secs(5)).await {
            self.inner.lock().unwrap().sent = Some(items);
        }
    }

    // The window's and the gateway's functions.

    pub async fn call(self: &Arc<App>, name: &str, input: &Value) -> Result<Value, (String, String)> {
        let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
        match name {
            "status" => {
                let stale = {
                    let inner = self.inner.lock().unwrap();
                    inner.lives.iter().any(|live| now().saturating_sub(live.delays_at) > 300)
                };
                if stale {
                    let app = self.clone();
                    tokio::spawn(async move { app.probe_all().await });
                }
                Ok(self.status().await)
            }
            "add" => {
                let source = input.get("source").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                if source.is_empty() {
                    return Err(("empty_source".into(), String::new()));
                }
                let mut connections = self.store.connections();
                if connections.len() >= MAX_CONNECTIONS {
                    return Err(("too_many".into(), MAX_CONNECTIONS.to_string()));
                }
                if let Source::Inline(text) = links::classify(&source) {
                    let (servers, skipped) = links::parse_text(&text);
                    if servers.is_empty() {
                        let locale = self.gateway.locale().await;
                        let why: Vec<String> =
                            skipped.iter().take(3).map(|skip| t(&locale, &skip.reason, &skip.detail)).collect();
                        return Err(("nothing_understood".into(), why.join("; ")));
                    }
                }
                let id = store::new_id(&connections);
                let connection = Connection { id: id.clone(), name: text("name"), source };
                connections.push(connection.clone());
                self.store.save_connections(&connections).map_err(|e| ("bad_input".to_string(), e.to_string()))?;
                {
                    let mut inner = self.inner.lock().unwrap();
                    let learned = Learned { choice: "auto".into(), port: free_port(), ..Learned::default() };
                    inner.state.connections.insert(id.clone(), learned.clone());
                    let _ = self.store.save_state(&inner.state);
                    let mut live = Live::of(connection, Some(&learned));
                    if live.connection.name.is_empty() && live.subscription.is_none() {
                        live.connection.name = live.servers.first().map(|s| s.name.clone()).unwrap_or_default();
                        let mut all: Vec<Connection> = inner.lives.iter().map(|l| l.connection.clone()).collect();
                        all.push(live.connection.clone());
                        let _ = self.store.save_connections(&all);
                    }
                    if let Some(view) = live.subscription.as_mut() {
                        view.reading = true;
                    }
                    inner.lives.push(live);
                }
                let app = self.clone();
                let fresh = id.clone();
                tokio::spawn(async move {
                    if app.subscriptions().contains(&fresh) {
                        app.read_subscription(&fresh).await;
                    } else {
                        app.rebuild().await;
                    }
                });
                Ok(json!({ "id": id }))
            }
            "remove" => {
                let id = text("connection");
                self.forget(&id)?;
                let app = self.clone();
                tokio::spawn(async move { app.rebuild().await });
                Ok(json!({ "removed": id }))
            }
            "rename" => {
                let id = text("connection");
                let name = text("name");
                let mut inner = self.inner.lock().unwrap();
                let live = inner
                    .lives
                    .iter_mut()
                    .find(|live| live.connection.id == id)
                    .ok_or(("no_connection".to_string(), String::new()))?;
                live.connection.name = name;
                let all: Vec<Connection> = inner.lives.iter().map(|live| live.connection.clone()).collect();
                self.store.save_connections(&all).map_err(|e| ("bad_input".to_string(), e.to_string()))?;
                self.changed.notify_one();
                Ok(json!({ "renamed": id }))
            }
            "select" => {
                let id = text("connection");
                let name = input.get("name").and_then(Value::as_str).unwrap_or("auto").to_string();
                let entrance = self.entrance(&id).ok_or(("no_connection".to_string(), String::new()))?;
                if name != "auto" && !entrance.servers.iter().any(|server| server.name == name) {
                    return Err(("no_server".into(), name));
                }
                {
                    let mut inner = self.inner.lock().unwrap();
                    if let Some(learned) = inner.state.connections.get_mut(&id) {
                        learned.choice = name.clone();
                    }
                    let _ = self.store.save_state(&inner.state);
                }
                let entrance = self.entrance(&id).ok_or(("no_connection".to_string(), String::new()))?;
                if self.engine.running() {
                    let body = json!({ "name": entrance.chosen_member() });
                    let group = config::choice_group(&id);
                    let _ =
                        self.clash("PUT", &format!("/proxies/{group}"), Some(body), Duration::from_millis(800)).await;
                }
                let app = self.clone();
                tokio::spawn(async move { app.check().await });
                Ok(json!({ "connection": id, "mode": if name == "auto" { "auto" } else { "manual" } }))
            }
            "check" => {
                let id = text("connection");
                let entrance = self.entrance(&id).ok_or(("no_connection".to_string(), String::new()))?;
                if entrance.servers.is_empty() || !self.engine.running() {
                    return Err(("not_running".into(), String::new()));
                }
                let app = self.clone();
                let work = tokio::spawn(async move {
                    let delay = app.delay(&config::choice_group(&id), 5000).await;
                    app.check().await;
                    let current = app
                        .inner
                        .lock()
                        .unwrap()
                        .lives
                        .iter()
                        .find(|l| l.connection.id == id)
                        .and_then(|l| l.current.clone());
                    let country = match (&current, delay) {
                        (Some(server), Some(_)) => app.learn_country(&id, server, true).await,
                        _ => None,
                    };
                    (delay, current, country)
                });
                // The window waits 2 s at most: a slow answer comes with the next `status`.
                match tokio::time::timeout(Duration::from_millis(1700), work).await {
                    Ok(Ok((Some(delay), current, country))) => {
                        Ok(json!({ "delayMs": delay, "current": current, "country": country }))
                    }
                    Ok(Ok((None, _, _))) => Err(("server_down".into(), String::new())),
                    _ => Ok(json!({ "pending": true })),
                }
            }
            "refresh" => {
                let only = text("connection");
                let ids: Vec<String> =
                    self.subscriptions().into_iter().filter(|id| only.is_empty() || *id == only).collect();
                {
                    let mut inner = self.inner.lock().unwrap();
                    for live in inner.lives.iter_mut().filter(|live| ids.contains(&live.connection.id)) {
                        if let Some(view) = live.subscription.as_mut() {
                            view.reading = true;
                        }
                    }
                }
                let app = self.clone();
                let reading = ids.clone();
                tokio::spawn(async move {
                    for id in reading {
                        app.read_subscription(&id).await;
                    }
                    app.probe_all().await;
                });
                Ok(json!({ "reading": ids }))
            }
            _ => Err(("not_found".into(), name.to_string())),
        }
    }

    fn forget(&self, id: &str) -> Result<(), (String, String)> {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.lives.len();
        inner.lives.retain(|live| live.connection.id != id);
        if inner.lives.len() == before {
            return Err(("no_connection".into(), String::new()));
        }
        inner.state.connections.remove(id);
        let all: Vec<Connection> = inner.lives.iter().map(|live| live.connection.clone()).collect();
        self.store.save_connections(&all).map_err(|e| ("bad_input".to_string(), e.to_string()))?;
        let _ = self.store.save_state(&inner.state);
        Ok(())
    }

    /// What the window shows: never a link, a password or a subscription's path.
    pub async fn status(&self) -> Value {
        let locale = self.gateway.locale().await;
        let inner = self.inner.lock().unwrap();
        let running = self.engine.running();
        let connections: Vec<Value> = inner
            .lives
            .iter()
            .map(|live| {
                let id = &live.connection.id;
                let learned = inner.state.connections.get(id);
                let choice = learned.map(|learned| learned.choice.clone()).unwrap_or_else(|| "auto".into());
                let manual = choice != "auto" && live.servers.iter().any(|server| server.name == choice);
                let health = if live.servers.is_empty() {
                    if live.subscription.as_ref().is_some_and(|view| view.reading) {
                        Health::Starting
                    } else {
                        Health::Down
                    }
                } else if running {
                    live.health
                } else {
                    Health::Starting
                };
                let servers: Vec<Value> = live
                    .servers
                    .iter()
                    .map(|server| {
                        let delay = live.delays.get(&server.name).copied();
                        json!({
                            "name": server.name,
                            "scheme": server.scheme,
                            "country": App::country_of(&inner, id, &server.name),
                            "delayMs": delay.flatten(),
                            "up": delay.map(|delay| delay.is_some()),
                        })
                    })
                    .collect();
                let subscription = live.subscription.as_ref().map(|view| {
                    json!({
                        "host": crate::subscription::host_of(&view.url),
                        "updatedAt": learned.and_then(|l| l.subscription.as_ref()).map(|s| s.at),
                        "reading": view.reading,
                        "error": view.error.as_ref().map(|(key, detail)| t(&locale, key, &redact(detail))),
                    })
                });
                let skipped: Vec<Value> = live
                    .skipped
                    .iter()
                    .map(|skip| json!({ "scheme": skip.scheme, "reason": t(&locale, &skip.reason, &skip.detail) }))
                    .collect();
                json!({
                    "id": id,
                    "name": live.connection.name,
                    "country": live.current.as_deref().and_then(|server| App::country_of(&inner, id, server)),
                    "state": health.word(),
                    "mode": if manual { "manual" } else { "auto" },
                    "choice": if manual { Value::String(choice) } else { Value::String("auto".into()) },
                    "current": live.current,
                    "servers": servers,
                    "subscription": subscription,
                    "skipped": skipped,
                })
            })
            .collect();
        json!({ "connections": connections, "max": MAX_CONNECTIONS })
    }

    /// The loops: checks, all servers' delays, subscriptions, the letters.
    pub fn start(self: &Arc<App>) {
        tokio::spawn(self.engine.clone().supervise());
        let app = self.clone();
        tokio::spawn(async move {
            loop {
                app.engine.started.notified().await;
                app.apply_choices().await;
            }
        });
        let app = self.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(CHECK_EVERY);
            loop {
                tokio::select! {
                    _ = every.tick() => {}
                    _ = app.recheck.notified() => tokio::time::sleep(Duration::from_millis(500)).await,
                }
                app.check().await;
            }
        });
        let app = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(20)).await;
            let mut every = tokio::time::interval(ALL_SERVERS_EVERY);
            loop {
                every.tick().await;
                app.probe_all().await;
            }
        });
        let app = self.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(SUBSCRIPTION_EVERY);
            loop {
                every.tick().await;
                for id in app.subscriptions() {
                    app.read_subscription(&id).await;
                }
            }
        });
        let app = self.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(30));
            loop {
                let always = tokio::select! {
                    _ = every.tick() => true,
                    _ = app.changed.notified() => false,
                };
                app.tell_gateway(always).await;
            }
        });
    }
}

/// An error's words for the journal and the window, without anything that looks like a key: long runs of base64 or
/// hex (passwords, uuids, keys) become `…`.
pub fn redact(text: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.len() >= 16 {
            out.push('…');
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_') {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_hides_keys() {
        assert_eq!(
            redact("config[3]: invalid key b85798ef-e9dc-46a4-9a87-8da4499d36d0 for x"),
            "config[3]: invalid key … for x"
        );
        assert_eq!(redact("timed out"), "timed out");
    }
}
