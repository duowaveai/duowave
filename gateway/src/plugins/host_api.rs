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

use super::manifest::{host_allowed, Manifest};
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
                return Err(crate::tr!("{host}: нет адреса вне этого компьютера", host = host).into());
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
        .map_err(|_| crate::tr!("адрес «{url}» не читается", url = text.chars().take(200).collect::<String>()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(crate::tr!("только http и https, не {scheme}", scheme = url.scheme()));
    }
    let host = match url.host() {
        Some(url::Host::Domain(name)) => name.to_string(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => ip.to_string(),
        None => return Err(crate::tr!("в адресе нет хоста")),
    };
    if host.parse::<IpAddr>().is_ok_and(forbidden) {
        return Err(crate::tr!("{host} — адрес этого компьютера, плагинам туда нельзя", host = host));
    }
    if hosts.is_empty() {
        return Err(crate::tr!("нет права net: плагин не ходит в сеть"));
    }
    if !host_allowed(hosts, &host) {
        return Err(crate::tr!("хост {host} не разрешён: в манифесте нет права net:{host}", host = host));
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
            other => return Err(crate::tr!("метод {other} не поддерживается", other = other)),
        },
    };
    let mut headers = Vec::new();
    if let Some(given) = request.get("headers").and_then(Value::as_object) {
        for (name, value) in given.iter().take(50) {
            let Some(value) = value.as_str() else {
                return Err(crate::tr!("заголовок {name}: строка", name = name));
            };
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
        return Err(crate::tr!("тело запроса больше {max} КБ", max = MAX_REQUEST / 1024));
    }
    let timeout =
        request.get("timeoutMs").and_then(Value::as_u64).unwrap_or(DEFAULT_TIMEOUT_MS).clamp(1, MAX_TIMEOUT_MS);
    Ok(Fetch { url, method, headers, body, timeout: Duration::from_millis(timeout) })
}

/// The request, its redirects checked one by one; the answer as `{status, url, headers, body}`.
async fn send(hosts: Vec<String>, fetch: Fetch) -> Result<Value, String> {
    let (status, final_url, answer_headers, bytes) = send_raw(hosts, fetch, MAX_RESPONSE).await?;
    Ok(json!({
        "status": status,
        "url": final_url,
        "headers": answer_headers,
        "body": String::from_utf8_lossy(&bytes),
    }))
}

/// The request, its redirects checked one by one; the answer's status, address, headers and bytes, `max` at most.
async fn send_raw(
    hosts: Vec<String>,
    fetch: Fetch,
    max: usize,
) -> Result<(u16, String, Map<String, Value>, Vec<u8>), String> {
    let Fetch { mut url, mut method, headers, mut body, timeout } = fetch;
    let deadline = tokio::time::Instant::now() + timeout;
    let late = || crate::tr!("нет ответа за {ms} мс", ms = timeout.as_millis());
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
            .map_err(|error| crate::tr!("запрос не прошёл: {error}", error = without_url(error)))?;
        if !response.status().is_redirection() {
            break response;
        }
        redirects += 1;
        let location = response.headers().get(reqwest::header::LOCATION).and_then(|value| value.to_str().ok());
        let (Some(location), true) = (location, redirects <= MAX_REDIRECTS) else {
            break response;
        };
        let next = url.join(location).map_err(|_| crate::tr!("перенаправление не читается"))?;
        url = checked_url(&hosts, next.as_str()).map_err(|why| crate::tr!("перенаправление: {why}", why = why))?;
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
        let chunk = chunk.map_err(|error| crate::tr!("ответ оборвался: {error}", error = without_url(error)))?;
        if bytes.len() + chunk.len() > max {
            return Err(crate::tr!("ответ больше {max} КБ", max = max / 1024));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((status, final_url, answer_headers, bytes))
}

/// `files.download`: a picture's bytes from a host of the manifest's rights, its redirects checked too, on the
/// gateway's runtime, `max` at most (plugins/files.rs).
pub fn download_checked(
    hosts: Vec<String>,
    url: &str,
    max: usize,
    handle: Option<tokio::runtime::Handle>,
) -> BoxFuture<'static, Result<Vec<u8>, String>> {
    let prepared = fetch_of(&hosts, &json!({ "url": url, "timeoutMs": MAX_TIMEOUT_MS }));
    Box::pin(async move {
        let fetch = prepared?;
        let Some(handle) = handle else { return Err(crate::tr!("сеть недоступна")) };
        let (status, _, _, bytes) =
            handle.spawn(send_raw(hosts, fetch, max)).await.map_err(|_| crate::tr!("запрос прервался"))??;
        if status != 200 {
            return Err(crate::tr!("картинка не скачалась: ответ {status}", status = status));
        }
        Ok(bytes)
    })
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
        let Some(handle) = handle else { return Err(crate::tr!("сеть недоступна")) };
        handle.spawn(send(hosts, fetch)).await.map_err(|_| crate::tr!("запрос прервался"))?
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

/// The gateway's services behind the bus's `system/…` participants (src/bus/providers/system.rs) and the bus itself:
/// a plugin reaches them only by letters (`host.bus`, Host API 2), each decided by the bus.
pub trait HostServices: Send + Sync {
    /// The household's own music or tales (`music`, `tales`): items with their `content` path, the best first.
    fn content(&self, folder: &str, query: Option<String>, limit: usize) -> BoxFuture<'static, Vec<Value>>;
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
    /// Said aloud by a station without a request, for the plugin `caller` (`system/voice`): whether it was said —
    /// no station on the line, nothing is.
    fn say(&self, caller: &str, text: String, device: Option<String>) -> BoxFuture<'static, Result<bool, String>> {
        let _ = (caller, text, device);
        Box::pin(async { Ok(false) })
    }
    /// A message to the owner from the plugin `caller` (`system/notify`, plugins/schedule.rs).
    fn notify(&self, caller: &str, text: String) -> BoxFuture<'static, Result<Value, String>> {
        let _ = (caller, text);
        Box::pin(async { Err(crate::tr!("сообщений владельцу здесь нет")) })
    }
    /// The plugin's pictures in its own folder (plugins/files.rs): `put` (the downloaded bytes), `list`, `delete`.
    fn files(
        &self,
        caller: &str,
        action: &str,
        input: Value,
        bytes: Option<Vec<u8>>,
    ) -> BoxFuture<'static, Result<Value, String>> {
        let _ = (caller, action, input, bytes);
        Box::pin(async { Err(crate::tr!("картинок здесь нет")) })
    }
    /// The plugin's own schedule (`system/schedule`, plugins/schedule.rs): `set`, `list`, `cancel`, `parse`.
    fn schedule(&self, caller: &str, action: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        let _ = (caller, action, input);
        Box::pin(async { Err(crate::tr!("расписаний здесь нет")) })
    }
    /// A letter on the bus for the plugin `caller` in its call's trail (src/bus): `bus.call`, `bus.tell`, `bus.list`,
    /// `bus.ask`, `folders.*`. None where there is no bus.
    fn bus(
        &self,
        caller: &str,
        name: &str,
        input: Value,
        trail: Option<crate::bus::Trail>,
    ) -> Option<BoxFuture<'static, Result<Value, String>>> {
        let _ = (caller, name, input, trail);
        None
    }
}

/// What the gateway gives a plugin's code.
pub struct PluginHostApi {
    id: String,
    manifest: Arc<Manifest>,
    db: Arc<Db>,
    settings: Arc<SettingsService>,
    /// The gateway's runtime: a plugin's requests and speech run there, beside the rest of the gateway's.
    handle: Option<tokio::runtime::Handle>,
    log_window: AtomicU64,
    log_count: AtomicU64,
    /// The plugin's own journal (`id`, level, message).
    sink: Option<LogSink>,
    /// The gateway's services behind `host.call`.
    services: Option<Arc<dyn HostServices>>,
    /// Who tells the panel that the plugin's data changed.
    data: Option<Arc<DataNews>>,
    /// The gateway's safe: the plugin's private settings, and what its log lines must not show.
    secrets: Option<Arc<crate::secrets::SecretsService>>,
}

impl PluginHostApi {
    pub fn new(
        manifest: Arc<Manifest>,
        db: Arc<Db>,
        settings: Arc<SettingsService>,
        handle: Option<tokio::runtime::Handle>,
    ) -> PluginHostApi {
        PluginHostApi {
            id: manifest.id.clone(),
            manifest,
            db,
            settings,
            handle,
            log_window: AtomicU64::new(0),
            log_count: AtomicU64::new(0),
            sink: None,
            services: None,
            data: None,
            secrets: None,
        }
    }

    /// The same, its private settings read from the gateway's safe and its log lines without them.
    pub fn with_secrets(mut self, secrets: Option<Arc<crate::secrets::SecretsService>>) -> PluginHostApi {
        self.secrets = secrets;
        self
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

    /// Its own store: always its own, no right needed (Host API 2).
    fn kv_allowed(&self) -> Result<(), String> {
        Ok(())
    }

    fn key(key: &str) -> Result<(), String> {
        if key.is_empty() || key.len() > MAX_KEY {
            return Err(crate::tr!("ключ: от 1 до {max} байт", max = MAX_KEY));
        }
        Ok(())
    }
}

/// A plugin's store as it is, key and stored text, by key: the history's snapshot of its data (history/data.rs).
pub fn kv_rows(db: &Db, plugin: &str) -> rusqlite::Result<Vec<(String, String)>> {
    db.with(|c| {
        let mut statement = c.prepare("SELECT key, value FROM plugin_kv WHERE plugin_id = ?1 ORDER BY key")?;
        let rows =
            statement.query_map(params![plugin], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        rows.collect()
    })
}

/// A plugin's store replaced whole: the history brings back its data (history/data.rs).
pub fn kv_replace(db: &Db, plugin: &str, rows: &[(String, String)]) -> rusqlite::Result<()> {
    let now = now_iso();
    db.transaction(|tx| {
        tx.execute("DELETE FROM plugin_kv WHERE plugin_id = ?1", params![plugin])?;
        for (key, value) in rows {
            tx.execute(
                "INSERT INTO plugin_kv (plugin_id, key, value, updated_at) VALUES (?1, ?2, ?3, ?4)",
                params![plugin, key, value, now],
            )?;
        }
        Ok(())
    })
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
            .map_err(|_| crate::tr!("хранилище не читается"))?;
        Ok(text.and_then(|text| serde_json::from_str(&text).ok()))
    }

    fn kv_set(&self, key: &str, value: Value) -> Result<(), String> {
        self.kv_allowed()?;
        PluginHostApi::key(key)?;
        let text = value.to_string();
        if text.len() > MAX_VALUE {
            return Err(crate::tr!("значение больше {max} КБ", max = MAX_VALUE / 1024));
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
            return Err(crate::tr!(
                "хранилище плагина заполнено: бюджет {mb} МБ",
                mb = self.manifest.budget.storage_mb
            ));
        }
        self.db
            .with(|c| {
                c.execute(
                    "INSERT INTO plugin_kv (plugin_id, key, value, updated_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (plugin_id, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                    params![self.id, key, text, now_iso()],
                )
            })
            .map_err(|_| crate::tr!("хранилище не записалось"))?;
        self.data_changed();
        Ok(())
    }

    fn kv_delete(&self, key: &str) -> Result<(), String> {
        self.kv_allowed()?;
        PluginHostApi::key(key)?;
        let removed = self
            .db
            .with(|c| c.execute("DELETE FROM plugin_kv WHERE plugin_id = ?1 AND key = ?2", params![self.id, key]))
            .map_err(|_| crate::tr!("хранилище не записалось"))?;
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
            .map_err(|_| crate::tr!("хранилище не читается"))?;
        Ok(keys)
    }

    /// Speech without a request is a letter to `system/voice` (Host API 2): the bus decides it like any other.
    fn say(&self, text: String, device: Option<String>) -> BoxFuture<'static, Result<bool, String>> {
        let letter = json!({ "to": "system/voice", "handle": "say", "input": { "text": text, "device": device } });
        let call = self.call("bus.call", letter);
        Box::pin(async move { call.await.map(|said| said.as_bool().unwrap_or(false)) })
    }

    /// The plugin's own values, a secret one as it is, from the gateway's safe (the panel sees only «••••••»,
    /// plugins/manifest.rs).
    fn settings(&self) -> Map<String, Value> {
        let stored = self.settings.get(&PACKAGE_SETTINGS, None).unwrap_or_default();
        match &self.secrets {
            Some(secrets) => {
                self.manifest.settings_values(Some(&self.manifest.stored_with_secrets(stored.get(&self.id), secrets)))
            }
            None => self.manifest.settings_values(stored.get(&self.id)),
        }
    }

    fn locale(&self) -> String {
        crate::locale::current(&self.settings)
    }

    fn languages(&self) -> Vec<String> {
        self.manifest.languages.clone()
    }

    /// Everything beyond the plugin itself is a letter on the bus (Host API 2): `bus.*` and its folders, `folders.*`.
    /// `folders.download` fetches a picture by the plugin's `net:` hosts here, then keeps it in its own folder.
    fn call(&self, name: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        let manifest = self.manifest.clone();
        let services = self.services.clone();
        let handle = self.handle.clone();
        let (name, id) = (name.to_string(), self.id.clone());
        // The trail of the call this runs in: the engine's thread holds it while the call goes (src/bus/trail.rs).
        let trail = crate::bus::trail::current();
        Box::pin(async move {
            let services = services.ok_or_else(|| crate::tr!("служб шлюза здесь нет"))?;
            if name == "folders.download" {
                let url = input.get("url").and_then(Value::as_str).unwrap_or_default().to_string();
                if url.is_empty() {
                    return Err(crate::tr!("files.download(url, name): нужна ссылка"));
                }
                let max = crate::plugins::files::FILE_MAX as usize;
                let bytes = download_checked(manifest.hosts(), &url, max, handle).await?;
                return services.files(&id, "put", input, Some(bytes)).await;
            }
            if name.starts_with("bus.") || name.starts_with("folders.") {
                let letter = services.bus(&id, &name, input, trail).ok_or_else(|| crate::tr!("шины здесь нет"))?;
                return letter.await;
            }
            Err(crate::tr!("нет функции {name}: с Host API 2 всё вне приложения — письма шины, host.bus.call(адрес, умение, данные)", name = name))
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
        let mut message: String = message.chars().take(1000).collect();
        // A plugin may print its own key: neither its journal (which the workshop's agent reads) nor the gateway's
        // log keeps it.
        if let Some(secrets) = &self.secrets {
            message = secrets.redact(&message);
        }
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
            "schema": 1, "kind": "plugin", "id": "shopping", "version": "1.0.0", "title": "Покупки", "api": "^2",
            "runs": ["pc", "station"], "permissions": []
        }))
        .unwrap();
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let api = PluginHostApi::new(Arc::new(manifest), db, settings, None)
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
