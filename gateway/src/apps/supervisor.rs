//! The apps' supervisor (docs/plans/plugins.md, «Надзор за приложениями», stage 9): an app is a program of its own beside
//! the gateway — Spotify through librespot, AirPlay, DLNA — that a script cannot be. Its package carries a build per
//! platform (`targets`, each with its SHA-256); the plugin host switches it on like a plugin, and this engine runs it:
//!
//! - the build for this platform, its hash checked before every start, started with the gateway's address, a port
//!   the gateway chose and a token of its own in its environment (nothing else of the gateway's);
//! - its functions — phrases, tools, sources, as a plugin's — are called over localhost HTTP (`POST /call/<name>`), and
//!   it reaches the Host API the same way (`/api/v1/apps/host/…` with its token): the same rights as a plugin's;
//! - it is asked whether it is well (`health`), restarted after a fall with a growing pause, and switched off after
//!   five falls in a row; its memory is counted for real (RSS) against its budget — three looks over it is a fall;
//!   on Linux `RLIMIT_AS` keeps a built program from more than eight times its budget, never under 4 GiB (a Go program
//!   reserves about 1.1 GiB of addresses at its start and dies without them), and `GOMEMLIMIT` tells Go's collector the
//!   budget (not Node: V8 reserves far more address space than it uses, its heap is held by `--max-old-space-size`);
//! - it is asked to stop (SIGTERM) and given three seconds before it is killed: a receiver lets go of the sound card's
//!   mixer and its ports itself;
//! - its output goes to the gateway's log and the plugin's journal under its name;
//! - its own sound — `/stream/<path>` of its server — reaches the station through our /media
//!   (`/media/apps/<id>/<path>`), and nothing else of its server does.
//!
//! Apps stop before plugins under pressure: they are heavier and easier to start again.
//!
//! Besides its own data (`DUOWAVE_APP_DATA`, beside its versions) it is told its own folder on the bus,
//! `DUOWAVE_APP_FILES` (`files/app/<id>`): what it puts there its window plays (docs/plans/feed.md, stage 3).
//!
//! An app of `runtime: node` is the same under the supervisor, only its program is the gateway's own Node with its
//! script (`node.entry`) — Node downloaded at the first need, the app's dependencies put from its lock (node.rs).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, BufReader};

use super::node::NodeHome;
use crate::plugins::manifest::{platform, Manifest, Runtime};
use crate::plugins::runtime::{CallKind, HostApi, MemoryStats, PluginCode, PluginError, PluginInstance, PluginRuntime};

/// How long an app has to say it is well after a start.
const START_LIMIT: Duration = Duration::from_secs(15);
/// Falls in a row before it is switched off.
const MAX_FALLS: u32 = 5;
/// Well this long, its falls are forgiven.
const FORGIVEN_AFTER: Duration = Duration::from_secs(600);
/// The least address space a built program gets on Linux: Go reserves its page summaries (~600 MiB) and its scavenger's
/// index (512 MiB) at its start (`runtime/mpagealloc_64bit.go`) and throws without them — eight budgets of 32 MB are
/// 256 MiB. What it really holds is counted by RSS against its budget anyway.
#[cfg(target_os = "linux")]
const ADDRESS_SPACE_FLOOR: u64 = 4 << 30;
/// How long a program asked to stop (SIGTERM) has before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(3);
/// Looks over its memory budget in a row that make a fall.
const OVER_BUDGET_LOOKS: u32 = 3;
const MEMORY_EVERY: Duration = Duration::from_secs(10);

/// Who is told an app gave up (five falls): the plugin host switches it off with the reason.
pub type GiveUp = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Who is told an app's program is gone — it fell, was stopped or unloaded — with the reason in words: the long tasks
/// it ran are interrupted (src/tasks).
pub type Gone = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// What the household calls the station this gateway runs on, asked at each start of an app (`DUOWAVE_STATION_NAME`);
/// None off a station.
pub type StationName = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// An app that runs: its id and its Host API, with its rights.
pub type AppHost = (String, Arc<dyn HostApi>);

/// The apps that run now, by their token: the Host API over HTTP finds an app's rights here.
#[derive(Default)]
pub struct AppTokens(Mutex<HashMap<String, AppHost>>);

impl AppTokens {
    pub fn host(&self, token: &str) -> Option<AppHost> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(token).cloned()
    }

    fn insert(&self, token: &str, id: &str, host: Arc<dyn HostApi>) {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(token.to_string(), (id.to_string(), host));
    }

    fn remove(&self, token: &str) {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(token);
    }
}

/// The engine of `runtime: process`.
pub struct ProcessRuntime {
    /// Where an app reaches the gateway's Host API (`http://127.0.0.1:<port>`): the configured port, then the one the
    /// server really listens on.
    gateway_url: Mutex<String>,
    tokens: Arc<AppTokens>,
    give_up: GiveUp,
    /// Set once the long tasks stand.
    gone: Arc<OnceLock<Gone>>,
    /// The Node the apps of `runtime: node` run on.
    node: Arc<NodeHome>,
    /// Where the apps' own folders are (`files/app/<id>`): each is told its own, `DUOWAVE_APP_FILES`.
    files_root: Mutex<Option<PathBuf>>,
    /// The station's name, once the placement stands.
    station_name: OnceLock<StationName>,
}

impl ProcessRuntime {
    pub fn new(gateway_url: String, tokens: Arc<AppTokens>, give_up: GiveUp, node: Arc<NodeHome>) -> ProcessRuntime {
        ProcessRuntime {
            gateway_url: Mutex::new(gateway_url),
            tokens,
            give_up,
            gone: Arc::default(),
            node,
            files_root: Mutex::new(None),
            station_name: OnceLock::new(),
        }
    }

    /// Where the station's name comes from: a receiver shows itself to phones by it.
    pub fn set_station_name(&self, name: StationName) {
        let _ = self.station_name.set(name);
    }

    /// Where the apps' own folders are: what its window plays is what it writes there.
    pub fn set_files_root(&self, root: PathBuf) {
        *self.files_root.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(root);
    }

    /// Who hears that an app's program is gone.
    pub fn set_gone(&self, gone: Gone) {
        let _ = self.gone.set(gone);
    }

    /// The address the gateway's server listens on, once it does.
    pub fn set_gateway_url(&self, url: String) {
        *self.gateway_url.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = url;
    }
}

/// The build of an app for this machine, its hash checked: its path, or why it cannot run here.
pub fn checked_build(dir: &Path, manifest: &Manifest) -> Result<PathBuf, String> {
    let key = platform();
    let target = manifest.targets.get(&key).ok_or_else(|| {
        let known: Vec<&str> = manifest.targets.keys().map(String::as_str).collect();
        crate::tr!("нет сборки под эту платформу ({key}); есть {known}", key = key, known = known.join(", "))
    })?;
    let file = dir.join(&target.file);
    let bytes = std::fs::read(&file).map_err(|_| crate::tr!("нет файла {file}", file = target.file))?;
    let actual = hex::encode(Sha256::digest(&bytes));
    if !actual.eq_ignore_ascii_case(&target.sha256) {
        return Err(crate::tr!(
            "{file}: хеш не совпал с манифестом — сборка подменена или испорчена",
            file = target.file
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755));
    }
    Ok(file)
}

fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|error| crate::tr!("нет свободного порта: {error}", error = error))?;
    listener.local_addr().map(|address| address.port()).map_err(|error| error.to_string())
}

/// What one start of the program needs.
#[derive(Clone)]
struct Launch {
    id: String,
    program: PathBuf,
    /// What the program is started with: a Node app's script.
    args: Vec<std::ffi::OsString>,
    /// PATH, when not the gateway's own: a Node app's has our Node first.
    path: Option<std::ffi::OsString>,
    data: PathBuf,
    /// Its own folder on the bus (`files/app/<id>`), when the gateway has one: its window sees it.
    files: Option<PathBuf>,
    port: u16,
    token: String,
    gateway_url: String,
    memory_mb: u64,
    health: String,
    /// The station's name, on a station.
    station_name: Option<String>,
    /// The gateway's entry for the program's own requests (`HTTPS_PROXY`, src/egress/app_entry.rs): its id and token
    /// in it, so the gateway goes by this app's row of «Выход в интернет». None — the entry does not run, or the app
    /// gives the connections itself.
    proxy: Option<String>,
}

fn spawn(launch: &Launch, host: &Arc<dyn HostApi>) -> Result<tokio::process::Child, String> {
    std::fs::create_dir_all(&launch.data).map_err(|error| format!("{}: {error}", launch.data.display()))?;
    if let Some(files) = &launch.files {
        std::fs::create_dir_all(files).map_err(|error| format!("{}: {error}", files.display()))?;
    }
    let mut command = tokio::process::Command::new(&launch.program);
    command
        .args(&launch.args)
        .env_clear()
        .env("PATH", launch.path.clone().or_else(|| std::env::var_os("PATH")).unwrap_or_default())
        .env("PORT", launch.port.to_string())
        .env("DUOWAVE_URL", &launch.gateway_url)
        .env("DUOWAVE_APP_TOKEN", &launch.token)
        .env("DUOWAVE_APP_ID", &launch.id)
        .env("DUOWAVE_APP_DATA", &launch.data)
        .current_dir(&launch.data)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    super::node::keep_system_env(&mut command);
    if launch.path.is_some() {
        command.env("NODE_ENV", "production");
    }
    if let Some(files) = &launch.files {
        command.env("DUOWAVE_APP_FILES", files);
    }
    if let Some(name) = launch.station_name.as_deref().map(str::trim).filter(|name| !name.is_empty()) {
        command.env("DUOWAVE_STATION_NAME", name);
    }
    if let Some(proxy) = &launch.proxy {
        // Go and Rust (reqwest) read these themselves; Node's fetch with NODE_USE_ENV_PROXY.
        for name in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
            command.env(name, proxy);
        }
        for name in ["NO_PROXY", "no_proxy"] {
            command.env(name, "127.0.0.1,localhost,::1");
        }
        if launch.path.is_some() {
            command.env("NODE_USE_ENV_PROXY", "1");
        }
    }
    if launch.path.is_none() {
        // Go's collector keeps the heap within the budget itself; any other program ignores it.
        command.env("GOMEMLIMIT", format!("{}MiB", launch.memory_mb.max(4)));
    }
    // Not for Node (`launch.path` is set only for it): V8 reserves gigabytes of address space at start, far over eight
    // budgets, and under RLIMIT_AS it never says it is healthy (CI on Linux, 14.11). Its heap is held by
    // --max-old-space-size and its real memory (RSS) is counted against its budget like any app's.
    #[cfg(target_os = "linux")]
    if launch.path.is_none() {
        let limit = (launch.memory_mb.max(32) * 8 * 1024 * 1024).max(ADDRESS_SPACE_FLOOR) as libc::rlim_t;
        // SAFETY: setrlimit is async-signal-safe and only touches the child before exec.
        unsafe {
            command.pre_exec(move || {
                let rlimit = libc::rlimit { rlim_cur: limit, rlim_max: limit };
                libc::setrlimit(libc::RLIMIT_AS, &rlimit);
                Ok(())
            });
        }
    }
    let mut child = command.spawn().map_err(|error| crate::tr!("не запустилось: {error}", error = error))?;
    for (stream, level) in [
        (child.stdout.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), "info"),
        (child.stderr.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), "warn"),
    ] {
        let Some(stream) = stream else { continue };
        let host = host.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                host.log(level, &line);
            }
        });
    }
    Ok(child)
}

/// Waits until the app says it is well, or the time is out.
async fn well(client: &reqwest::Client, launch: &Launch, within: Duration) -> bool {
    let url = format!("http://127.0.0.1:{}{}", launch.port, launch.health);
    let until = Instant::now() + within;
    while Instant::now() < until {
        if client
            .get(&url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .is_ok_and(|answer| answer.status().is_success())
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    false
}

struct Shared {
    launch: Launch,
    host: Arc<dyn HostApi>,
    stopping: AtomicBool,
    falls: AtomicU32,
    pid: Mutex<Option<u32>>,
    child: tokio::sync::Mutex<Option<tokio::process::Child>>,
    client: reqwest::Client,
    give_up: GiveUp,
    gone: Arc<OnceLock<Gone>>,
    handlers: Vec<String>,
}

impl Shared {
    fn note(&self, level: &str, message: &str) {
        self.host.log(level, message);
    }

    /// The program is gone: whatever it was at is over.
    fn gone(&self, why: &str) {
        if let Some(gone) = self.gone.get() {
            gone(&self.launch.id, why);
        }
    }

    async fn kill(&self) {
        if let Some(mut child) = self.child.lock().await.take() {
            // Asked first: it lets go of what it holds (a receiver — the sound card's mixer, its ports).
            #[cfg(unix)]
            if let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
                // SAFETY: a signal to our own child, whose pid is held (not reaped) while `child` lives.
                unsafe { libc::kill(pid, libc::SIGTERM) };
                if tokio::time::timeout(STOP_GRACE, child.wait()).await.is_ok() {
                    *self.pid.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
                    return;
                }
            }
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
        }
        *self.pid.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    async fn launch(&self) -> Result<(), String> {
        let child = spawn(&self.launch, &self.host)?;
        *self.pid.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = child.id();
        *self.child.lock().await = Some(child);
        if well(&self.client, &self.launch, START_LIMIT).await {
            Ok(())
        } else {
            self.kill().await;
            Err(crate::tr!("не сказало, что здорово, за {seconds} с", seconds = START_LIMIT.as_secs()))
        }
    }

    /// Its watch: a fall is answered with a start again after a growing pause; five in a row — it gives up. Its
    /// health and its memory are looked at meanwhile.
    async fn watch(self: Arc<Self>) {
        let mut healthy_since = Instant::now();
        let mut over_budget = 0;
        let mut sick = 0;
        let health_every = Duration::from_secs(15);
        let mut last_health = Instant::now();
        loop {
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }
            let exited = {
                let mut child = self.child.lock().await;
                match child.as_mut() {
                    Some(child) => child.try_wait().ok().flatten().map(|status| status.to_string()),
                    None => Some(crate::tr!("не запущено")),
                }
            };
            let mut fell = exited.map(|status| crate::tr!("упало ({status})", status = status));
            if fell.is_none() && last_health.elapsed() >= health_every {
                last_health = Instant::now();
                let url = format!("http://127.0.0.1:{}{}", self.launch.port, self.launch.health);
                let ok = self
                    .client
                    .get(&url)
                    .timeout(Duration::from_secs(3))
                    .send()
                    .await
                    .is_ok_and(|answer| answer.status().is_success());
                sick = if ok { 0 } else { sick + 1 };
                if sick >= 3 {
                    fell = Some(crate::tr!("три раза подряд не ответило, здорово ли"));
                }
                let pid = *self.pid.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(rss) = pid.and_then(|pid| crate::operations::system_memory::process_rss_mb(Some(pid))) {
                    over_budget = if rss > self.launch.memory_mb { over_budget + 1 } else { 0 };
                    if over_budget >= OVER_BUDGET_LOOKS {
                        fell = Some(crate::tr!(
                            "держит {rss} МБ при бюджете {budget} МБ",
                            rss = rss,
                            budget = self.launch.memory_mb
                        ));
                    }
                }
            }
            if let Some(why) = fell {
                if self.stopping.load(Ordering::SeqCst) {
                    return;
                }
                self.kill().await;
                self.gone(&crate::tr!("Приложение {why}.", why = why));
                if healthy_since.elapsed() > FORGIVEN_AFTER {
                    self.falls.store(0, Ordering::SeqCst);
                }
                let falls = self.falls.fetch_add(1, Ordering::SeqCst) + 1;
                if falls >= MAX_FALLS {
                    let reason = crate::tr!(
                        "приложение падало {falls} раз подряд ({why}) — выключено",
                        falls = falls,
                        why = why
                    );
                    self.note("error", &reason);
                    (self.give_up)(&self.launch.id, &reason);
                    return;
                }
                let pause = Duration::from_secs(1 << (falls - 1).min(5));
                self.note(
                    "warn",
                    &crate::tr!("{why}; новый запуск через {seconds} с", why = why, seconds = pause.as_secs()),
                );
                tokio::time::sleep(pause).await;
                if self.stopping.load(Ordering::SeqCst) {
                    return;
                }
                match self.launch().await {
                    Ok(()) => {
                        healthy_since = Instant::now();
                        over_budget = 0;
                        sick = 0;
                        self.note("info", &crate::tr!("запущено снова"));
                    }
                    Err(error) => self.note("error", &error),
                }
                continue;
            }
            tokio::time::sleep(Duration::from_millis(500).min(MEMORY_EVERY)).await;
        }
    }
}

struct Instance {
    shared: Arc<Shared>,
    tokens: Arc<AppTokens>,
}

impl PluginInstance for Instance {
    fn call(&self, function: &str, input: Value, kind: CallKind) -> BoxFuture<'static, Result<Value, PluginError>> {
        let shared = self.shared.clone();
        let function = function.to_string();
        Box::pin(async move {
            if shared.stopping.load(Ordering::SeqCst) || shared.falls.load(Ordering::SeqCst) >= MAX_FALLS {
                return Err(PluginError::Unavailable(crate::tr!("приложение остановлено")));
            }
            let limit = kind.limit();
            let url = format!("http://127.0.0.1:{}/call/{function}", shared.launch.port);
            let answer = shared
                .client
                .post(url)
                .bearer_auth(&shared.launch.token)
                .timeout(limit)
                .json(&input)
                .send()
                .await
                .map_err(|error| {
                    if error.is_timeout() {
                        PluginError::Timeout(limit.as_millis() as u64)
                    } else {
                        PluginError::Script(crate::tr!("приложение не ответило: {error}", error = error.without_url()))
                    }
                })?;
            match answer.status().as_u16() {
                404 => Err(PluginError::Missing(function)),
                code if !(200..300).contains(&code) => {
                    let text = answer.text().await.unwrap_or_default();
                    // The app's own words for people (`{"error": "…"}` with 400) go as they are: they are already in
                    // the household's language (docs/plugins/AUTHORING.md, «Приложения»).
                    let words = serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|value| value.get("error").and_then(Value::as_str).map(str::to_string))
                        .filter(|words| code == 400 && !words.trim().is_empty());
                    Err(PluginError::Script(match words {
                        Some(words) => words.chars().take(300).collect(),
                        None => crate::tr!(
                            "приложение: HTTP {code} {text}",
                            code = code,
                            text = text.chars().take(300).collect::<String>()
                        ),
                    }))
                }
                _ => {
                    let bytes = answer.bytes().await.map_err(|error| PluginError::Script(error.to_string()))?;
                    if bytes.is_empty() {
                        return Ok(Value::Null);
                    }
                    serde_json::from_slice(&bytes)
                        .map_err(|_| PluginError::Answer(crate::tr!("ответ приложения — не JSON")))
                }
            }
        })
    }

    fn memory(&self) -> BoxFuture<'static, Option<MemoryStats>> {
        let shared = self.shared.clone();
        Box::pin(async move {
            let pid = (*shared.pid.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))?;
            let rss = tokio::task::spawn_blocking(move || crate::operations::system_memory::process_rss_mb(Some(pid)))
                .await
                .ok()
                .flatten()?;
            let bytes = rss * 1024 * 1024;
            Some(MemoryStats { allocated: bytes, used: bytes, limit: shared.launch.memory_mb * 1024 * 1024 })
        })
    }

    fn exports(&self) -> Vec<String> {
        self.shared.handlers.clone()
    }

    fn streams(&self) -> Option<String> {
        let gone = self.shared.stopping.load(Ordering::SeqCst) || self.shared.falls.load(Ordering::SeqCst) >= MAX_FALLS;
        (!gone).then(|| format!("http://127.0.0.1:{}/stream/", self.shared.launch.port))
    }

    fn stop(&self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        self.tokens.remove(&self.shared.launch.token);
        self.shared.gone(&crate::tr!("Приложение остановлено."));
        let shared = self.shared.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move { shared.kill().await });
        }
    }
}

impl PluginRuntime for ProcessRuntime {
    fn name(&self) -> &'static str {
        "process"
    }

    fn start(
        &self,
        code: PluginCode,
        host: Arc<dyn HostApi>,
    ) -> BoxFuture<'static, Result<Arc<dyn PluginInstance>, PluginError>> {
        let gateway_url = self.gateway_url.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let (tokens, give_up, gone) = (self.tokens.clone(), self.give_up.clone(), self.gone.clone());
        let node_home = self.node.clone();
        let files_root = self.files_root.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let station_name = self.station_name.get().and_then(|name| name());
        Box::pin(async move {
            let manifest =
                code.manifest.clone().ok_or_else(|| PluginError::Load(crate::tr!("нет манифеста приложения")))?;
            let dir = code.dir.clone().ok_or_else(|| PluginError::Load(crate::tr!("нет папки приложения")))?;
            let (program, args, path) = if manifest.runtime == Runtime::Node {
                let entry = manifest.node_entry.clone().unwrap_or_else(|| "index.js".into());
                let script = dir.join(&entry);
                if !script.is_file() {
                    return Err(PluginError::Load(format!("нет файла {entry} (node.entry)")));
                }
                let tell = |line: &str| host.log("info", line);
                let node = node_home.node(&tell).await.map_err(PluginError::Load)?;
                node_home.dependencies(&node, &dir, &tell).await.map_err(PluginError::Load)?;
                // Its heap within its budget: Node says it is out of memory before the supervisor has to.
                let heap = format!("--max-old-space-size={}", manifest.budget.memory_mb.max(64));
                let args = vec![heap.into(), script.into_os_string()];
                (node.program.clone(), args, Some(super::node::path_with(&node.bin_dir)))
            } else {
                let (dir, manifest) = (dir.clone(), manifest.clone());
                let program = tokio::task::spawn_blocking(move || checked_build(&dir, &manifest))
                    .await
                    .map_err(|error| PluginError::Load(error.to_string()))?
                    .map_err(PluginError::Load)?;
                (program, Vec::new(), None)
            };
            // Its own data outlives a version: `<id>/.data` beside the versions (a dot folder is no version).
            let data = dir.parent().map(|parent| parent.join(".data")).unwrap_or_else(|| dir.join(".data"));
            let token = uuid::Uuid::new_v4().simple().to_string();
            let proxy = crate::egress::entry_for(&code.id, &token, &manifest);
            let launch = Launch {
                id: code.id.clone(),
                program,
                args,
                path,
                files: files_root.map(|root| root.join(&code.id)),
                data,
                port: free_port().map_err(PluginError::Load)?,
                token,
                gateway_url,
                memory_mb: manifest.budget.memory_mb,
                health: manifest.health.path.clone(),
                station_name,
                proxy,
            };
            let contributes = &manifest.contributes;
            let mut handlers: Vec<String> = contributes
                .intents
                .iter()
                .map(|intent| intent.handler.clone())
                .chain(contributes.tools.iter().map(|tool| tool.handler.clone()))
                .chain(contributes.media.iter().map(|media| media.handler.clone()))
                .chain(contributes.events.iter().map(|event| event.handler.clone()))
                .chain(contributes.bus.on.iter().map(|on| on.handler.clone()))
                .collect();
            handlers.sort();
            handlers.dedup();
            let shared = Arc::new(Shared {
                launch: launch.clone(),
                host: host.clone(),
                stopping: AtomicBool::new(false),
                falls: AtomicU32::new(0),
                pid: Mutex::new(None),
                child: tokio::sync::Mutex::new(None),
                client: crate::utils::http::client(),
                give_up,
                gone,
                handlers,
            });
            tokens.insert(&launch.token, &launch.id, host.clone());
            if let Err(error) = shared.launch().await {
                tokens.remove(&launch.token);
                return Err(PluginError::Load(error));
            }
            host.log("info", &crate::tr!("приложение запущено на порту {port}", port = launch.port));
            tokio::spawn(shared.clone().watch());
            Ok(Arc::new(Instance { shared, tokens }) as Arc<dyn PluginInstance>)
        })
    }
}
