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
//!   on Linux `RLIMIT_AS` keeps it from more than eight times its budget;
//! - its output goes to the gateway's log and the plugin's journal under its name;
//! - its own sound — `/stream/<path>` of its server — reaches the station through our /media
//!   (`/media/apps/<id>/<path>`), and nothing else of its server does.
//!
//! Apps stop before plugins under pressure: they are heavier and easier to start again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::plugins::manifest::{platform, Manifest};
use crate::plugins::runtime::{CallKind, HostApi, MemoryStats, PluginCode, PluginError, PluginInstance, PluginRuntime};

/// How long an app has to say it is well after a start.
const START_LIMIT: Duration = Duration::from_secs(15);
/// Falls in a row before it is switched off.
const MAX_FALLS: u32 = 5;
/// Well this long, its falls are forgiven.
const FORGIVEN_AFTER: Duration = Duration::from_secs(600);
/// Looks over its memory budget in a row that make a fall.
const OVER_BUDGET_LOOKS: u32 = 3;
const MEMORY_EVERY: Duration = Duration::from_secs(10);

/// Who is told an app gave up (five falls): the plugin host switches it off with the reason.
pub type GiveUp = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Who is told an app's program is gone — it fell, was stopped or unloaded — with the reason in words: the long tasks
/// it ran are interrupted (src/tasks).
pub type Gone = Arc<dyn Fn(&str, &str) + Send + Sync>;

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
}

impl ProcessRuntime {
    pub fn new(gateway_url: String, tokens: Arc<AppTokens>, give_up: GiveUp) -> ProcessRuntime {
        ProcessRuntime { gateway_url: Mutex::new(gateway_url), tokens, give_up, gone: Arc::default() }
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
    data: PathBuf,
    port: u16,
    token: String,
    gateway_url: String,
    memory_mb: u64,
    health: String,
}

fn spawn(launch: &Launch, host: &Arc<dyn HostApi>) -> Result<tokio::process::Child, String> {
    std::fs::create_dir_all(&launch.data).map_err(|error| format!("{}: {error}", launch.data.display()))?;
    let mut command = tokio::process::Command::new(&launch.program);
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
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
    #[cfg(target_os = "linux")]
    {
        let limit = (launch.memory_mb.max(32) * 8 * 1024 * 1024) as libc::rlim_t;
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
                    Err(PluginError::Script(crate::tr!(
                        "приложение: HTTP {code} {text}",
                        code = code,
                        text = text.chars().take(300).collect::<String>()
                    )))
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
        Box::pin(async move {
            let manifest =
                code.manifest.clone().ok_or_else(|| PluginError::Load(crate::tr!("нет манифеста приложения")))?;
            let dir = code.dir.clone().ok_or_else(|| PluginError::Load(crate::tr!("нет папки приложения")))?;
            let program = {
                let (dir, manifest) = (dir.clone(), manifest.clone());
                tokio::task::spawn_blocking(move || checked_build(&dir, &manifest))
                    .await
                    .map_err(|error| PluginError::Load(error.to_string()))?
                    .map_err(PluginError::Load)?
            };
            // Its own data outlives a version: `<id>/.data` beside the versions (a dot folder is no version).
            let data = dir.parent().map(|parent| parent.join(".data")).unwrap_or_else(|| dir.join(".data"));
            let launch = Launch {
                id: code.id.clone(),
                program,
                data,
                port: free_port().map_err(PluginError::Load)?,
                token: uuid::Uuid::new_v4().simple().to_string(),
                gateway_url,
                memory_mb: manifest.budget.memory_mb,
                health: manifest.health.path.clone(),
            };
            let contributes = &manifest.contributes;
            let mut handlers: Vec<String> = contributes
                .intents
                .iter()
                .map(|intent| intent.handler.clone())
                .chain(contributes.tools.iter().map(|tool| tool.handler.clone()))
                .chain(contributes.media.iter().map(|media| media.handler.clone()))
                .chain(contributes.events.iter().map(|event| event.handler.clone()))
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
