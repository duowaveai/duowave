//! Host API v1 for one plugin: what its code may ask of the gateway, each call checked against the rights in its
//! manifest — the ones the admin saw and approved by switching it on.
//!
//! - `http.fetch` reaches only the hosts of its `net:` rights, a redirect too; never this computer's own addresses
//!   (the gateway's API answers its own machine more openly); within a time limit and a size limit, through the
//!   gateway's HTTP client (rustls, the same on the station).
//! - `kv` is its own store in the gateway's database, as big as its `budget.storageMb`; a write that changes it or a
//!   removal tells the panel (`packages.data`): the package's widgets read their data again.
//! - `settings` are the values of the settings its manifest declares, as the admin set them.
//! - `log` goes to the gateway's log under its name, a burst cut short.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures::future::BoxFuture;
use futures::StreamExt;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Map, Value};

use super::manifest::{host_allowed, Manifest, Permission};
use super::runtime::HostApi;
use crate::core::events::{EventBus, GatewayEvent};
use crate::settings::definitions::PACKAGE_SETTINGS;
use crate::settings::SettingsService;
use crate::skills::own::reminders_skill::Speaker;
use crate::storage::Db;
use crate::utils::time::{now_iso, now_ms};

/// A response bigger than this is cut off with an error; a request body too.
pub const MAX_RESPONSE: usize = 1024 * 1024;
const MAX_REQUEST: usize = 256 * 1024;
/// A request's own time limit unless it asks for less; the call's limit holds above it anyway.
const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const MAX_TIMEOUT_MS: u64 = 10_000;
const MAX_REDIRECTS: usize = 5;
/// A key, and a value as JSON, at most this big.
const MAX_KEY: usize = 200;
const MAX_VALUE: usize = 64 * 1024;
const MAX_KEYS: usize = 1000;
/// Log lines a plugin may write in a window; the rest are dropped with a note.
const LOG_BURST: u64 = 50;
const LOG_WINDOW_MS: u64 = 10_000;
/// A package's data changes are told the panel at most this often.
const DATA_NEWS_EVERY: Duration = Duration::from_secs(1);

/// A plugin's data changed (`kv`): the panel's widgets of its package read theirs again (`packages.data`) — a list
/// filled by voice shows without a reload. Told at once, then at most once per `every` for the package: the writes
/// within it are told once at its end, so the last of a burst is never lost. One for all the plugins (the host's).
pub struct DataNews {
    events: EventBus,
    /// Where the end of a second is waited for; none (a tool, a test without a runtime) — each write told.
    handle: Option<tokio::runtime::Handle>,
    every: Duration,
    /// The packages told within the last `every`, and whether a write came since (told at its end).
    cooling: Arc<Mutex<HashMap<String, bool>>>,
}

impl DataNews {
    pub fn new(events: EventBus, handle: Option<tokio::runtime::Handle>) -> Arc<DataNews> {
        DataNews::with(events, handle, DATA_NEWS_EVERY)
    }

    pub fn with(events: EventBus, handle: Option<tokio::runtime::Handle>, every: Duration) -> Arc<DataNews> {
        Arc::new(DataNews { events, handle, every, cooling: Arc::new(Mutex::new(HashMap::new())) })
    }

    /// A write of the package's: told now, or at the end of the `every` it falls in.
    pub fn changed(&self, package: &str) {
        let Some(handle) = &self.handle else {
            tell(&self.events, package);
            return;
        };
        {
            let mut cooling = self.cooling.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(again) = cooling.get_mut(package) {
                *again = true;
                return;
            }
            cooling.insert(package.to_string(), false);
        }
        tell(&self.events, package);
        let (events, cooling, every, package) =
            (self.events.clone(), self.cooling.clone(), self.every, package.to_string());
        handle.spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let again = {
                    let mut cooling = cooling.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    match cooling.get_mut(&package) {
                        Some(again) if *again => {
                            *again = false;
                            true
                        }
                        _ => {
                            cooling.remove(&package);
                            false
                        }
                    }
                };
                if !again {
                    return;
                }
                tell(&events, &package);
            }
        });
    }
}

fn tell(events: &EventBus, package: &str) {
    events.emit(GatewayEvent::PackageDataChanged { package: package.to_string() });
}

/// The addresses a plugin never reaches whatever its rights: this machine, its link-local neighbours, nowhere.
fn forbidden(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast()
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (ip.segments()[0] & 0xffc0) == 0xfe80
                || ip.to_ipv4_mapped().is_some_and(|ip| forbidden(IpAddr::V4(ip)))
        }
    }
}

/// Resolves names as the system does, leaving out the addresses a plugin never reaches: a name of an allowed host
/// that points at 127.0.0.1 gets nowhere.
struct OutsideOnly;

impl reqwest::dns::Resolve for OutsideOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let found: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.filter(|address| !forbidden(address.ip())).collect();
            if found.is_empty() {
                return Err(format!("{host}: нет адреса вне этого компьютера").into());
            }
            Ok(Box::new(found.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// The plugins' HTTP client: the gateway's TLS, no redirects of its own (each one is checked here), names resolved
/// without this machine's addresses.
fn client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            crate::utils::http::client_builder()
                .redirect(reqwest::redirect::Policy::none())
                .dns_resolver(Arc::new(OutsideOnly))
                .build()
                .expect("the plugins' HTTP client builds")
        })
        .clone()
}

/// The URL if the plugin may reach it, or why not.
pub fn checked_url(hosts: &[String], text: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(text)
        .map_err(|_| format!("адрес «{}» не читается", text.chars().take(200).collect::<String>()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("только http и https, не {}", url.scheme()));
    }
    let host = match url.host() {
        Some(url::Host::Domain(name)) => name.to_string(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => ip.to_string(),
        None => return Err("в адресе нет хоста".into()),
    };
    if host.parse::<IpAddr>().is_ok_and(forbidden) {
        return Err(format!("{host} — адрес этого компьютера, плагинам туда нельзя"));
    }
    if hosts.is_empty() {
        return Err("нет права net: плагин не ходит в сеть".into());
    }
    if !host_allowed(hosts, &host) {
        return Err(format!("хост {host} не разрешён: в манифесте нет права net:{host}"));
    }
    Ok(url)
}

struct Fetch {
    url: url::Url,
    method: reqwest::Method,
    headers: Vec<(String, String)>,
    body: Option<String>,
    timeout: Duration,
}

fn fetch_of(hosts: &[String], request: &Value) -> Result<Fetch, String> {
    let url = checked_url(hosts, request.get("url").and_then(Value::as_str).unwrap_or_default())?;
    let method = match request.get("method").and_then(Value::as_str).map(|method| method.to_ascii_uppercase()) {
        None => reqwest::Method::GET,
        Some(method) => match method.as_str() {
            "GET" => reqwest::Method::GET,
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "PATCH" => reqwest::Method::PATCH,
            "DELETE" => reqwest::Method::DELETE,
            "HEAD" => reqwest::Method::HEAD,
            other => return Err(format!("метод {other} не поддерживается")),
        },
    };
    let mut headers = Vec::new();
    if let Some(given) = request.get("headers").and_then(Value::as_object) {
        for (name, value) in given.iter().take(50) {
            let Some(value) = value.as_str() else { return Err(format!("заголовок {name}: строка")) };
            // The host and the length are the client's to set.
            if ["host", "content-length", "connection", "transfer-encoding"]
                .contains(&name.to_ascii_lowercase().as_str())
            {
                continue;
            }
            headers.push((name.clone(), value.to_string()));
        }
    }
    let body = request.get("body").and_then(Value::as_str).map(str::to_string);
    if body.as_ref().is_some_and(|body| body.len() > MAX_REQUEST) {
        return Err(format!("тело запроса больше {} КБ", MAX_REQUEST / 1024));
    }
    let timeout =
        request.get("timeoutMs").and_then(Value::as_u64).unwrap_or(DEFAULT_TIMEOUT_MS).clamp(1, MAX_TIMEOUT_MS);
    Ok(Fetch { url, method, headers, body, timeout: Duration::from_millis(timeout) })
}

/// The request, its redirects checked one by one; the answer as `{status, url, headers, body}`.
async fn send(hosts: Vec<String>, fetch: Fetch) -> Result<Value, String> {
    let Fetch { mut url, mut method, headers, mut body, timeout } = fetch;
    let deadline = tokio::time::Instant::now() + timeout;
    let late = || format!("нет ответа за {} мс", timeout.as_millis());
    let mut redirects = 0;
    let response = loop {
        let mut request = client().request(method.clone(), url.clone());
        for (name, value) in &headers {
            request = request.header(name, value);
        }
        if let Some(body) = &body {
            request = request.body(body.clone());
        }
        let response = tokio::time::timeout_at(deadline, request.send())
            .await
            .map_err(|_| late())?
            .map_err(|error| format!("запрос не прошёл: {}", without_url(error)))?;
        if !response.status().is_redirection() {
            break response;
        }
        redirects += 1;
        let location = response.headers().get(reqwest::header::LOCATION).and_then(|value| value.to_str().ok());
        let (Some(location), true) = (location, redirects <= MAX_REDIRECTS) else {
            break response;
        };
        let next = url.join(location).map_err(|_| "перенаправление не читается".to_string())?;
        url = checked_url(&hosts, next.as_str()).map_err(|why| format!("перенаправление: {why}"))?;
        // 303, and a POST's 301 and 302: the next request is a GET without the body, as browsers do.
        let status = response.status().as_u16();
        if status == 303 || ((status == 301 || status == 302) && method == reqwest::Method::POST) {
            method = reqwest::Method::GET;
            body = None;
        }
    };
    let status = response.status().as_u16();
    let final_url = response.url().to_string();
    let mut answer_headers = Map::new();
    for (name, value) in response.headers().iter().take(50) {
        if let Ok(value) = value.to_str() {
            answer_headers.insert(name.as_str().to_string(), Value::String(value.to_string()));
        }
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::time::timeout_at(deadline, stream.next()).await.map_err(|_| late())?;
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|error| format!("ответ оборвался: {}", without_url(error)))?;
        if bytes.len() + chunk.len() > MAX_RESPONSE {
            return Err(format!("ответ больше {} КБ", MAX_RESPONSE / 1024));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(json!({
        "status": status,
        "url": final_url,
        "headers": answer_headers,
        "body": String::from_utf8_lossy(&bytes),
    }))
}

/// `http.fetch` for a manifest's hosts: the request checked, its redirects too, on the gateway's runtime. The
/// workshop's sandbox fetches the same way as a plugin that is on.
pub fn fetch_checked(
    hosts: Vec<String>,
    request: &Value,
    handle: Option<tokio::runtime::Handle>,
) -> BoxFuture<'static, Result<Value, String>> {
    let prepared = fetch_of(&hosts, request);
    Box::pin(async move {
        let fetch = prepared?;
        let Some(handle) = handle else { return Err("сеть недоступна".into()) };
        handle.spawn(send(hosts, fetch)).await.map_err(|_| "запрос прервался".to_string())?
    })
}

/// reqwest's error without the URL it carries (a key may be in its query).
fn without_url(error: reqwest::Error) -> String {
    error.without_url().to_string()
}

/// How the gateway speaks without a request (its reminders, its calls): through the live station of a device.
pub type SpeakerSource = Arc<dyn Fn() -> Option<Arc<dyn Speaker>> + Send + Sync>;

/// Where a plugin's log lines go besides the gateway's log: the plugin's own journal for the panel and the workshop.
pub type LogSink = Arc<dyn Fn(&str, &str, &str) + Send + Sync>;

/// The gateway's services a plugin reaches through `host.call` (Host API 1.1, stage 10 of docs/plans/plugins.md):
/// what the built-in skills used, so they can move into plugins. Each is behind its right, checked by the host API.
pub trait HostServices: Send + Sync {
    /// The household's own music or tales (`music`, `tales`): items with their `content` path, the best first.
    fn content(&self, folder: &str, query: Option<String>, limit: usize) -> BoxFuture<'static, Vec<Value>>;
    /// Another plugin's tool for «Мозг», by its name: its answer as the model reads it.
    fn call_tool(
        &self,
        caller: &str,
        name: &str,
        input: Map<String, Value>,
    ) -> BoxFuture<'static, Result<String, String>>;
    /// The smart home's devices: `[{id, name, kind, state}]`.
    fn smart_home_devices(&self) -> BoxFuture<'static, Result<Value, String>>;
    /// A device does something: `on`, `off`, `set_brightness`, `set_temperature`, `scene`.
    fn smart_home_act(
        &self,
        device: String,
        action: String,
        value: Option<f64>,
    ) -> BoxFuture<'static, Result<String, String>>;
    /// The weather's usual place, as the panel set it.
    fn weather_place(&self) -> Option<String>;
    /// The news feeds switched on in the panel: `[{url, topic}]`.
    fn news_feeds(&self) -> Vec<Value>;
}

/// What the gateway gives a plugin's code.
pub struct PluginHostApi {
    id: String,
    manifest: Arc<Manifest>,
    db: Arc<Db>,
    settings: Arc<SettingsService>,
    /// The gateway's runtime: a plugin's requests and speech run there, beside the rest of the gateway's.
    handle: Option<tokio::runtime::Handle>,
    speaker: SpeakerSource,
    log_window: AtomicU64,
    log_count: AtomicU64,
    /// The plugin's own journal (`id`, level, message).
    sink: Option<LogSink>,
    /// The gateway's services behind `host.call`.
    services: Option<Arc<dyn HostServices>>,
    /// Who tells the panel that the plugin's data changed.
    data: Option<Arc<DataNews>>,
}

impl PluginHostApi {
    pub fn new(
        manifest: Arc<Manifest>,
        db: Arc<Db>,
        settings: Arc<SettingsService>,
        handle: Option<tokio::runtime::Handle>,
        speaker: SpeakerSource,
    ) -> PluginHostApi {
        PluginHostApi {
            id: manifest.id.clone(),
            manifest,
            db,
            settings,
            handle,
            speaker,
            log_window: AtomicU64::new(0),
            log_count: AtomicU64::new(0),
            sink: None,
            services: None,
            data: None,
        }
    }

    /// The same, with the gateway's services behind `host.call`.
    pub fn with_services(mut self, services: Option<Arc<dyn HostServices>>) -> PluginHostApi {
        self.services = services;
        self
    }

    /// The same, its log lines also in the plugin's own journal.
    pub fn with_sink(mut self, sink: LogSink) -> PluginHostApi {
        self.sink = Some(sink);
        self
    }

    /// The same, its writes to `kv` told the panel (`packages.data`).
    pub fn with_data_news(mut self, data: Arc<DataNews>) -> PluginHostApi {
        self.data = Some(data);
        self
    }

    fn data_changed(&self) {
        if let Some(data) = &self.data {
            data.changed(&self.id);
        }
    }

    fn kv_allowed(&self) -> Result<(), String> {
        if self.manifest.has(&Permission::Kv) {
            Ok(())
        } else {
            Err("нет права kv: в манифесте его нет".into())
        }
    }

    fn key(key: &str) -> Result<(), String> {
        if key.is_empty() || key.len() > MAX_KEY {
            return Err(format!("ключ: от 1 до {MAX_KEY} байт"));
        }
        Ok(())
    }
}

/// How much of the store a plugin uses, keys and values, in bytes.
pub fn storage_used(db: &Db, plugin: &str) -> u64 {
    db.with(|c| {
        c.query_row(
            "SELECT COALESCE(SUM(LENGTH(CAST(key AS BLOB)) + LENGTH(CAST(value AS BLOB))), 0) FROM plugin_kv WHERE plugin_id = ?1",
            params![plugin],
            |row| row.get::<_, i64>(0),
        )
    })
    .map(|used| used.max(0) as u64)
    .unwrap_or(0)
}

impl HostApi for PluginHostApi {
    fn fetch(&self, request: Value) -> BoxFuture<'static, Result<Value, String>> {
        fetch_checked(self.manifest.hosts(), &request, self.handle.clone())
    }

    fn kv_get(&self, key: &str) -> Result<Option<Value>, String> {
        self.kv_allowed()?;
        PluginHostApi::key(key)?;
        let text: Option<String> = self
            .db
            .with(|c| {
                c.query_row(
                    "SELECT value FROM plugin_kv WHERE plugin_id = ?1 AND key = ?2",
                    params![self.id, key],
                    |row| row.get(0),
                )
                .optional()
            })
            .map_err(|_| "хранилище не читается".to_string())?;
        Ok(text.and_then(|text| serde_json::from_str(&text).ok()))
    }

    fn kv_set(&self, key: &str, value: Value) -> Result<(), String> {
        self.kv_allowed()?;
        PluginHostApi::key(key)?;
        let text = value.to_string();
        if text.len() > MAX_VALUE {
            return Err(format!("значение больше {} КБ", MAX_VALUE / 1024));
        }
        let stored: Option<String> = self
            .db
            .with(|c| {
                c.query_row(
                    "SELECT value FROM plugin_kv WHERE plugin_id = ?1 AND key = ?2",
                    params![self.id, key],
                    |row| row.get(0),
                )
                .optional()
            })
            .ok()
            .flatten();
        // The value it holds already: nothing changes, nothing is written or told — a plugin that writes what it read
        // would make its widgets read again for nothing.
        if stored.as_deref().and_then(|stored| serde_json::from_str::<Value>(stored).ok()).as_ref() == Some(&value) {
            return Ok(());
        }
        let budget = self.manifest.budget.storage_mb * 1024 * 1024;
        let before = stored.map(|stored| (key.len() + stored.len()) as u64).unwrap_or(0);
        let after = storage_used(&self.db, &self.id).saturating_sub(before) + (key.len() + text.len()) as u64;
        if after > budget {
            return Err(format!("хранилище плагина заполнено: бюджет {} МБ", self.manifest.budget.storage_mb));
        }
        self.db
            .with(|c| {
                c.execute(
                    "INSERT INTO plugin_kv (plugin_id, key, value, updated_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (plugin_id, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                    params![self.id, key, text, now_iso()],
                )
            })
            .map_err(|_| "хранилище не записалось".to_string())?;
        self.data_changed();
        Ok(())
    }

    fn kv_delete(&self, key: &str) -> Result<(), String> {
        self.kv_allowed()?;
        PluginHostApi::key(key)?;
        let removed = self
            .db
            .with(|c| c.execute("DELETE FROM plugin_kv WHERE plugin_id = ?1 AND key = ?2", params![self.id, key]))
            .map_err(|_| "хранилище не записалось".to_string())?;
        if removed > 0 {
            self.data_changed();
        }
        Ok(())
    }

    fn kv_keys(&self, prefix: &str) -> Result<Vec<String>, String> {
        self.kv_allowed()?;
        let keys: Vec<String> = self
            .db
            .with(|c| {
                let mut statement = c.prepare(
                    "SELECT key FROM plugin_kv WHERE plugin_id = ?1 AND substr(key, 1, ?2) = ?3 ORDER BY key LIMIT ?4",
                )?;
                let rows = statement
                    .query_map(params![self.id, prefix.chars().count() as i64, prefix, MAX_KEYS as i64], |row| {
                        row.get(0)
                    })?;
                rows.collect()
            })
            .map_err(|_| "хранилище не читается".to_string())?;
        Ok(keys)
    }

    fn say(&self, text: String, device: Option<String>) -> BoxFuture<'static, Result<bool, String>> {
        let allowed = self.manifest.has(&Permission::VoiceSay);
        let (speaker, handle, id) = ((self.speaker)(), self.handle.clone(), self.id.clone());
        Box::pin(async move {
            if !allowed {
                return Err("нет права voice.say: в манифесте его нет".into());
            }
            let text: String = text.trim().chars().take(500).collect();
            if text.is_empty() {
                return Err("нечего сказать".into());
            }
            // No station on the line, or no station link at all: nothing is said, and the plugin is told so.
            let (Some(speaker), Some(handle)) = (speaker, handle) else { return Ok(false) };
            tracing::info!(target: "plugins", "{id}: says «{text}»");
            let device = device.filter(|device| !device.is_empty()).unwrap_or_else(|| "*".into());
            handle.spawn(speaker.say(device, text, None)).await.map_err(|_| "речь прервалась".to_string())
        })
    }

    /// The plugin's own values, a secret one as it is (the panel sees only «••••••», plugins/manifest.rs).
    fn settings(&self) -> Map<String, Value> {
        let stored = self.settings.get(&PACKAGE_SETTINGS, None).unwrap_or_default();
        self.manifest.settings_values(stored.get(&self.id))
    }

    fn call(&self, name: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        let manifest = self.manifest.clone();
        let services = self.services.clone();
        let (name, id) = (name.to_string(), self.id.clone());
        Box::pin(async move {
            let services = services.ok_or_else(|| "служб шлюза здесь нет".to_string())?;
            let need = |permission: Permission| {
                if manifest.has(&permission) {
                    Ok(())
                } else {
                    Err(format!("нет права {}: в манифесте его нет", permission.name()))
                }
            };
            let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_string);
            match name.as_str() {
                "content.search" => {
                    let folder = text("folder").or_else(|| text("kind")).unwrap_or_default();
                    need(Permission::Content(folder.clone()))?;
                    let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(50).clamp(1, 200) as usize;
                    Ok(Value::Array(services.content(&folder, text("query").filter(|q| !q.is_empty()), limit).await))
                }
                "tools.call" => {
                    need(Permission::ToolsCall)?;
                    let tool = text("name").unwrap_or_default();
                    let arguments = input.get("input").and_then(Value::as_object).cloned().unwrap_or_default();
                    // An answer in JSON comes back as such: the `{say, play}` of a tool that acts can be played on.
                    services.call_tool(&id, &tool, arguments).await.map(|answer| match serde_json::from_str(&answer) {
                        Ok(value @ (Value::Object(_) | Value::Array(_))) => value,
                        _ => Value::String(answer),
                    })
                }
                "smartHome.devices" => {
                    need(Permission::SmartHome)?;
                    services.smart_home_devices().await
                }
                "smartHome.act" => {
                    need(Permission::SmartHome)?;
                    let value = input.get("value").and_then(Value::as_f64);
                    services
                        .smart_home_act(text("device").unwrap_or_default(), text("action").unwrap_or_default(), value)
                        .await
                        .map(Value::String)
                }
                "weather.place" => Ok(services.weather_place().map(Value::String).unwrap_or(Value::Null)),
                "news.feeds" => Ok(Value::Array(services.news_feeds())),
                other => Err(format!("нет функции {other}")),
            }
        })
    }

    fn log(&self, level: &str, message: &str) {
        let window = now_ms() / LOG_WINDOW_MS;
        if self.log_window.swap(window, Ordering::Relaxed) != window {
            self.log_count.store(0, Ordering::Relaxed);
        }
        let count = self.log_count.fetch_add(1, Ordering::Relaxed) + 1;
        if count > LOG_BURST {
            if count == LOG_BURST + 1 {
                tracing::warn!(target: "plugins", "{}: too many log lines, the rest of this burst is dropped", self.id);
            }
            return;
        }
        let message: String = message.chars().take(1000).collect();
        if let Some(sink) = &self.sink {
            sink(&self.id, level, &message);
        }
        match level {
            "error" => tracing::error!(target: "plugins", "{}: {message}", self.id),
            "warn" => tracing::warn!(target: "plugins", "{}: {message}", self.id),
            _ => tracing::info!(target: "plugins", "{}: {message}", self.id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn told(heard: &mut tokio::sync::broadcast::Receiver<Arc<GatewayEvent>>) -> Vec<String> {
        let mut packages = Vec::new();
        while let Ok(event) = heard.try_recv() {
            if let GatewayEvent::PackageDataChanged { package } = event.as_ref() {
                packages.push(package.clone());
            }
        }
        packages
    }

    /// The owner, 10.11: «добавь молоко в список покупок» did not show in the widget until a reload. A plugin's write
    /// is told at once; a burst within a second once more at its end — the last write is never lost — and each
    /// package on its own.
    #[tokio::test(start_paused = true)]
    async fn a_plugins_writes_are_told_at_once_then_at_most_once_a_second_the_last_never_lost() {
        let events = EventBus::new();
        let mut heard = events.subscribe();
        let news = DataNews::new(events.clone(), Some(tokio::runtime::Handle::current()));
        news.changed("shopping");
        news.changed("shopping");
        news.changed("shopping");
        news.changed("radio");
        assert_eq!(told(&mut heard), ["shopping", "radio"], "the first of each at once");

        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert_eq!(told(&mut heard), ["shopping"], "the rest of the burst at the second's end, once");
        news.changed("shopping");
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(told(&mut heard).is_empty(), "within the next second: held");
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(told(&mut heard), ["shopping"], "and told at its end");

        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(told(&mut heard).is_empty(), "nothing written, nothing told");
        news.changed("shopping");
        assert_eq!(told(&mut heard), ["shopping"], "a quiet package is told at once again");

        // Without a runtime to wait on, each write is told.
        let plain = DataNews::new(events.clone(), None);
        plain.changed("radio");
        plain.changed("radio");
        assert_eq!(told(&mut heard), ["radio", "radio"]);
    }

    /// Review (packages-data-loop): a widget that wrote what it had read made every widget on the desk read again each
    /// second. The value a key holds already, written again, changes nothing: nothing is told. Another value is, and a
    /// removal of what was there.
    #[test]
    fn the_value_a_key_holds_written_again_tells_the_panel_nothing() {
        let events = EventBus::new();
        let mut heard = events.subscribe();
        let manifest = crate::plugins::manifest::check(&json!({
            "schema": 1, "kind": "plugin", "id": "shopping", "version": "1.0.0", "title": "Покупки", "api": "^1",
            "runs": ["pc", "station"], "permissions": ["kv"]
        }))
        .unwrap();
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let api = PluginHostApi::new(Arc::new(manifest), db, settings, None, Arc::new(|| None))
            .with_data_news(DataNews::new(events.clone(), None));

        api.kv_set("list", json!({ "items": ["молоко"], "at": 1 })).unwrap();
        assert_eq!(told(&mut heard), ["shopping"]);
        api.kv_set("list", json!({ "at": 1, "items": ["молоко"] })).unwrap();
        assert!(told(&mut heard).is_empty(), "the same list, its keys in another order: nothing told");
        api.kv_set("list", json!({ "items": ["молоко", "хлеб"], "at": 2 })).unwrap();
        assert_eq!(told(&mut heard), ["shopping"]);
        assert_eq!(api.kv_get("list").unwrap(), Some(json!({ "items": ["молоко", "хлеб"], "at": 2 })));
        api.kv_delete("list").unwrap();
        api.kv_delete("list").unwrap();
        assert_eq!(told(&mut heard), ["shopping"], "removed once: the second removes nothing");
    }
}
