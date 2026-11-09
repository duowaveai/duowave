//! The servers that speak in the owner's voices (clone-server/server.py), one per engine, on its own port. Started by
//! the gateway, detached (they outlive its restart and are taken over by the instance token in run/<engine>.json),
//! stopped only when /health proves they are ours. No background polling: they are looked at when a voice is chosen,
//! when the gateway starts, when a synthesis finds the server gone, and when the panel asks.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::future::{BoxFuture, FutureExt, Shared};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::clone_engines::{Device, EngineId};
use super::engine_installer::python_of;
use super::jobs::{with_limits, PANICKED};
use super::js;
use super::platform::PlatformInfo;
use crate::utils::files::owner_only;
use crate::utils::ids::uuid;
use crate::utils::time::now_iso;

/// A clone server's /health: whose it is, its instance and pid, what it holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerHealth {
    pub server: Option<String>,
    pub engine: Option<String>,
    pub device: Option<String>,
    pub pid: Option<i64>,
    pub instance: Option<String>,
    pub voices: Option<Vec<String>>,
    /// Its memory now and its peak, MB, as its system counts them (MPS memory included).
    pub memory_mb: Option<f64>,
    pub peak_mb: Option<f64>,
}

impl ServerHealth {
    fn from_json(body: &Value) -> ServerHealth {
        let text = |name: &str| body.get(name).and_then(Value::as_str).map(str::to_string);
        ServerHealth {
            server: text("server"),
            engine: text("engine"),
            device: text("device").filter(|device| !device.is_empty()),
            pid: body.get("pid").and_then(Value::as_f64).filter(|pid| pid.fract() == 0.0).map(|pid| pid as i64),
            instance: text("instance"),
            voices: body
                .get("voices")
                .and_then(Value::as_array)
                .map(|voices| voices.iter().filter_map(Value::as_str).map(str::to_string).collect()),
            memory_mb: body.get("memoryMb").and_then(Value::as_f64),
            peak_mb: body.get("peakMb").and_then(Value::as_f64),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerState {
    pub engine: EngineId,
    pub url: String,
    pub up: bool,
    pub starting: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voices: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "js::opt_number")]
    pub memory_mb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "js::opt_number")]
    pub peak_mb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Started by this gateway (a foreign or remote server is used, never stopped).
    pub ours: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunFile {
    pid: u32,
    port: u16,
    instance: String,
    started_at: String,
}

#[derive(Debug, Clone, Copy)]
pub struct StartOptions {
    pub device: Device,
    pub limit_mb: u64,
    pub threads: u32,
}

/// A started server: its pid, and a future that ends when it exits.
pub struct Spawned {
    pub pid: Option<u32>,
    pub exited: BoxFuture<'static, ()>,
}

/// How a server is started: detached, its output into its log. Tests give their own.
pub trait ServerSpawner: Send + Sync {
    fn spawn(
        &self,
        command: &str,
        args: &[String],
        cwd: &Path,
        env: &HashMap<String, String>,
        log: std::fs::File,
    ) -> std::io::Result<Spawned>;
}

/// The real one: a process in its own group (Ctrl+C in the gateway's terminal does not reach it), never killed with
/// the gateway.
pub struct ProcessSpawner;

impl ServerSpawner for ProcessSpawner {
    fn spawn(
        &self,
        command: &str,
        args: &[String],
        cwd: &Path,
        env: &HashMap<String, String>,
        log: std::fs::File,
    ) -> std::io::Result<Spawned> {
        let mut server = tokio::process::Command::new(command);
        server
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(env)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .kill_on_drop(false);
        #[cfg(unix)]
        server.process_group(0);
        #[cfg(windows)]
        server.creation_flags(super::platform::CREATE_NO_WINDOW | 0x0000_0200); // and CREATE_NEW_PROCESS_GROUP
        let mut child = server.spawn()?;
        let pid = child.id();
        Ok(Spawned {
            pid,
            exited: async move {
                let _ = child.wait().await;
            }
            .boxed(),
        })
    }
}

pub type PlatformFact = Arc<dyn Fn() -> Option<PlatformInfo> + Send + Sync>;
pub type EnvFact = Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>;

pub struct CloneServersOptions {
    /// Base URLs with /v1, by engine.
    pub urls: BTreeMap<EngineId, String>,
    pub runtime_dir: PathBuf,
    /// clone-server, where server.py is.
    pub server_dir: PathBuf,
    pub voices_dir: PathBuf,
    pub run_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub platform: PlatformFact,
    pub env: EnvFact,
    pub spawner: Arc<dyn ServerSpawner>,
    pub start_timeout_ms: u64,
}

const LOCAL: &[&str] = &["127.0.0.1", "localhost", "::1", "[::1]"];

type Starting = Shared<BoxFuture<'static, Result<(), String>>>;

pub struct CloneServers {
    options: CloneServersOptions,
    starting: Mutex<HashMap<EngineId, Starting>>,
    errors: Mutex<HashMap<EngineId, String>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl CloneServers {
    pub fn new(options: CloneServersOptions) -> CloneServers {
        CloneServers { options, starting: Mutex::new(HashMap::new()), errors: Mutex::new(HashMap::new()) }
    }

    pub fn url(&self, engine: EngineId) -> String {
        self.options.urls.get(&engine).map(|url| url.trim_end_matches('/').to_string()).unwrap_or_default()
    }

    /// A server on this computer is ours to start; one elsewhere (a container, another host) is only used.
    pub fn local(&self, engine: EngineId) -> bool {
        url::Url::parse(&self.url(engine))
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .is_some_and(|host| LOCAL.contains(&host.as_str()))
    }

    pub async fn health(&self, engine: EngineId, timeout_ms: u64) -> Option<ServerHealth> {
        let url = self.url(engine);
        let base = url.strip_suffix("/v1").unwrap_or(&url);
        let response = crate::utils::http::client()
            .get(format!("{base}/health"))
            .timeout(Duration::from_millis(timeout_ms))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let body: Value = response.json().await.ok()?;
        let health = ServerHealth::from_json(&body);
        (health.server.as_deref() == Some("clone-server") && health.engine.as_deref() == Some(engine.as_str()))
            .then_some(health)
    }

    fn is_starting(&self, engine: EngineId) -> bool {
        locked(&self.starting).contains_key(&engine)
    }

    pub async fn state(&self, engine: EngineId) -> ServerState {
        let health = if self.is_starting(engine) { None } else { self.health(engine, 1_500).await };
        let run = self.read_run(engine);
        let error = locked(&self.errors).get(&engine).cloned();
        let ours = match (&health, &run) {
            (Some(health), Some(run)) => health.instance.as_deref() == Some(run.instance.as_str()),
            _ => false,
        };
        let up = health.is_some();
        let health = health.unwrap_or_default();
        ServerState {
            engine,
            url: self.url(engine),
            up,
            starting: self.is_starting(engine),
            device: health.device,
            voices: health.voices,
            memory_mb: health.memory_mb,
            peak_mb: health.peak_mb,
            error,
            ours,
        }
    }

    /// The engine's server answering, started if needed (a second call joins the first); why it did not come up.
    pub async fn ensure(self: &Arc<Self>, engine: EngineId, start: StartOptions) -> Result<(), String> {
        let running = locked(&self.starting).get(&engine).cloned();
        if let Some(running) = running {
            return running.await;
        }
        if self.health(engine, 1_500).await.is_some() {
            return Ok(());
        }
        if !self.local(engine) {
            return Err(format!("Сервер голоса {} не отвечает.", self.url(engine)));
        }
        let starting = {
            let mut starting = locked(&self.starting);
            match starting.get(&engine) {
                Some(running) => running.clone(),
                None => {
                    // The map is held while the task is made, so its own removal always comes after this insert. A start
                    // that panics is a start that failed (TS: its finally): never «starting» for good.
                    let servers = self.clone();
                    let task = tokio::spawn(async move {
                        let result = std::panic::AssertUnwindSafe(servers.start(engine, start))
                            .catch_unwind()
                            .await
                            .unwrap_or_else(|_| {
                                let why = format!("сервер голоса не запустился: {PANICKED}");
                                locked(&servers.errors).insert(engine, why.clone());
                                Err(why)
                            });
                        locked(&servers.starting).remove(&engine);
                        result
                    });
                    let shared =
                        async move { task.await.unwrap_or_else(|error| Err(error.to_string())) }.boxed().shared();
                    starting.insert(engine, shared.clone());
                    shared
                }
            }
        };
        starting.await
    }

    /// Stops the engine's server when it is the one this gateway started; a foreign one is left alone.
    pub async fn stop(&self, engine: EngineId) -> bool {
        let run = self.read_run(engine);
        let health = self.health(engine, 1_500).await;
        let (Some(run), Some(health)) = (run, health) else { return false };
        // Never a group or init: only a real process of ours.
        let Some(pid) = health.pid.filter(|pid| *pid > 1).and_then(|pid| u32::try_from(pid).ok()) else {
            return false;
        };
        if health.instance.as_deref() != Some(run.instance.as_str()) {
            return false;
        }
        kill(pid);
        for _ in 0..10 {
            if self.health(engine, 500).await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let _ = std::fs::remove_file(self.run_file(engine));
        true
    }

    async fn start(&self, engine: EngineId, start: StartOptions) -> Result<(), String> {
        let platform = (self.options.platform)().ok_or_else(|| "Система ещё не проверена.".to_string())?;
        locked(&self.errors).remove(&engine);
        let runtime = self.options.runtime_dir.join(engine.as_str());
        let python = python_of(&runtime, platform.os);
        if !python.exists() {
            return Err("Модель не установлена.".into());
        }
        let port = url::Url::parse(&self.url(engine)).ok().and_then(|url| url.port()).unwrap_or(80);
        let instance = uuid();
        let text = |path: &Path| path.to_string_lossy().into_owned();
        let args = vec![
            text(&self.options.server_dir.join("server.py")),
            "--engine".into(),
            engine.as_str().into(),
            "--runtime".into(),
            text(&runtime),
            "--voices".into(),
            text(&self.options.voices_dir),
            "--port".into(),
            port.to_string(),
            "--device".into(),
            start.device.as_str().into(),
            "--threads".into(),
            start.threads.to_string(),
            "--instance".into(),
            instance.clone(),
            "--mem-limit-mb".into(),
            start.limit_mb.to_string(),
        ];
        let (command, wrapped) = with_limits(&platform, &text(&python), &args, Some(start.limit_mb));
        std::fs::create_dir_all(&self.options.logs_dir).map_err(|error| error.to_string())?;
        let log_path = self.options.logs_dir.join(format!("clone-{engine}.log"));
        if log_path.exists() {
            let _ = std::fs::rename(&log_path, self.options.logs_dir.join(format!("clone-{engine}.log.1")));
        }
        let log =
            std::fs::OpenOptions::new().create(true).append(true).open(&log_path).map_err(|error| error.to_string())?;
        let exited = Arc::new(AtomicBool::new(false));
        let env = (self.options.env)();
        match self.options.spawner.spawn(&command, &wrapped, &self.options.server_dir, &env, log) {
            Ok(spawned) => {
                if let Some(pid) = spawned.pid {
                    self.write_run(engine, &RunFile { pid, port, instance: instance.clone(), started_at: now_iso() });
                }
                let exited = exited.clone();
                tokio::spawn(async move {
                    spawned.exited.await;
                    exited.store(true, Ordering::SeqCst);
                });
            }
            Err(error) => {
                tracing::warn!("voice server {engine}: {command} did not start ({error})");
                exited.store(true, Ordering::SeqCst);
            }
        }
        let deadline = tokio::time::Instant::now() + Duration::from_millis(self.options.start_timeout_ms);
        while tokio::time::Instant::now() < deadline {
            let health = self.health(engine, 1_000).await;
            if health.and_then(|health| health.instance).as_deref() == Some(instance.as_str()) {
                return Ok(());
            }
            if exited.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let tail = read_tail(&log_path);
        let why = if exited.load(Ordering::SeqCst) {
            if tail.is_empty() {
                "сервер голоса не запустился".to_string()
            } else {
                format!("сервер голоса не запустился: {tail}")
            }
        } else {
            "сервер голоса не ответил за две минуты — модель грузится слишком долго".to_string()
        };
        locked(&self.errors).insert(engine, why.clone());
        Err(why)
    }

    fn run_file(&self, engine: EngineId) -> PathBuf {
        self.options.run_dir.join(format!("{engine}.json"))
    }

    fn read_run(&self, engine: EngineId) -> Option<RunFile> {
        serde_json::from_str(&std::fs::read_to_string(self.run_file(engine)).ok()?).ok()
    }

    fn write_run(&self, engine: EngineId, run: &RunFile) {
        let written = std::fs::create_dir_all(&self.options.run_dir).and_then(|()| {
            let path = self.run_file(engine);
            std::fs::write(&path, serde_json::to_vec(run).unwrap_or_default())?;
            owner_only(&path);
            Ok(())
        });
        if let Err(error) = written {
            tracing::warn!("voice server {engine}: its run file was not written ({error})");
        }
    }
}

/// SIGTERM (Windows: taskkill of the tree).
fn kill(pid: u32) {
    #[cfg(windows)]
    super::jobs::taskkill(pid);
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(pid) {
        // SAFETY: kill only sends a signal; a process already gone answers ESRCH, which is fine.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
    #[cfg(not(any(unix, windows)))]
    let _ = pid;
}

/// The last meaningful line of a server's log: why it did not start.
fn read_tail(path: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(path) else { return String::new() };
    let last = text.split('\n').map(js::trim).rfind(|line| !line.is_empty() && !line.starts_with("Loading weights"));
    js::slice(last.unwrap_or(""), 200).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice_lab::platform::{GpuInfo, GpuKind, OsName, Tools};
    use axum::routing::get;
    use axum::{Json, Router};

    fn platform() -> PlatformInfo {
        PlatformInfo {
            os: OsName::Darwin,
            arch: "arm64".into(),
            ram_mb: 36_864,
            cpus: 8,
            cpu_name: None,
            docker: false,
            gpu: GpuInfo { kind: GpuKind::Apple, name: None, vram_mb: None, driver: None },
            disk_free_mb: 80_000,
            tools: Tools { uv: None, git: true, taskpolicy: false, systemd_run: false },
            checked_at: String::new(),
        }
    }

    /// A clone server's /health as the stub says it: whose instance, which pid.
    fn health_server(listener: std::net::TcpListener, body: Arc<Mutex<Value>>) -> tokio::task::JoinHandle<()> {
        listener.set_nonblocking(true).unwrap();
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        let router = Router::new()
            .route("/health", get(move || async move { Json(body.lock().unwrap().clone()) }))
            .fallback(|| async { Json(serde_json::json!({})) });
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        })
    }

    struct Fixture {
        root: tempfile::TempDir,
        port: u16,
        /// The port, held from the start so no other test takes it; whoever serves on it takes the listener.
        listener: Arc<Mutex<Option<std::net::TcpListener>>>,
    }

    impl Fixture {
        fn new() -> Fixture {
            let root = tempfile::tempdir().unwrap();
            let python = python_of(&root.path().join("runtime").join("omnivoice"), OsName::Darwin);
            std::fs::create_dir_all(python.parent().unwrap()).unwrap();
            std::fs::write(&python, "").unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            Fixture { root, port, listener: Arc::new(Mutex::new(Some(listener))) }
        }

        fn path(&self, sub: &str) -> PathBuf {
            self.root.path().join(sub)
        }

        fn make(&self, spawner: Arc<dyn ServerSpawner>) -> Arc<CloneServers> {
            self.make_with(spawner, Arc::new(HashMap::new))
        }

        fn make_with(&self, spawner: Arc<dyn ServerSpawner>, env: EnvFact) -> Arc<CloneServers> {
            let urls = BTreeMap::from([
                (EngineId::Omnivoice, format!("http://127.0.0.1:{}/v1", self.port)),
                (EngineId::Cosyvoice3, "http://example.invalid:8094/v1".to_string()),
            ]);
            Arc::new(CloneServers::new(CloneServersOptions {
                urls,
                runtime_dir: self.path("runtime"),
                server_dir: self.path("clone-server"),
                voices_dir: self.path("voices"),
                run_dir: self.path("run"),
                logs_dir: self.path("logs"),
                platform: Arc::new(|| Some(platform())),
                env,
                spawner,
                start_timeout_ms: 5_000,
            }))
        }

        fn answer(&self, body: Value) -> (Arc<Mutex<Value>>, tokio::task::JoinHandle<()>) {
            let body = Arc::new(Mutex::new(body));
            let listener = self.listener.lock().unwrap().take().expect("the port is free to serve");
            (body.clone(), health_server(listener, body))
        }
    }

    type Call = (String, Vec<String>);

    type Act = Box<dyn Fn(&str, &[String]) -> Spawned + Send + Sync>;

    /// Records what would start, and does what the test says instead.
    struct FakeSpawner {
        calls: Mutex<Vec<Call>>,
        act: Act,
    }

    impl FakeSpawner {
        fn new(act: impl Fn(&str, &[String]) -> Spawned + Send + Sync + 'static) -> Arc<FakeSpawner> {
            Arc::new(FakeSpawner { calls: Mutex::new(Vec::new()), act: Box::new(act) })
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ServerSpawner for FakeSpawner {
        fn spawn(
            &self,
            command: &str,
            args: &[String],
            _cwd: &Path,
            _env: &HashMap<String, String>,
            _log: std::fs::File,
        ) -> std::io::Result<Spawned> {
            self.calls.lock().unwrap().push((command.to_string(), args.to_vec()));
            Ok((self.act)(command, args))
        }
    }

    fn never() -> BoxFuture<'static, ()> {
        futures::future::pending().boxed()
    }

    fn value_after<'a>(args: &'a [String], name: &str) -> &'a str {
        let at = args.iter().position(|arg| arg == name).unwrap();
        &args[at + 1]
    }

    #[tokio::test]
    async fn takes_over_a_server_already_answering_for_its_engine_and_starts_nothing() {
        let fixture = Fixture::new();
        let _server = fixture.answer(
            serde_json::json!({"server": "clone-server", "engine": "omnivoice", "instance": "earlier", "pid": 1}),
        );
        let spawner = FakeSpawner::new(|_, _| Spawned { pid: None, exited: never() });
        let start = StartOptions { device: Device::Mps, limit_mb: 6000, threads: 4 };
        fixture.make(spawner.clone()).ensure(EngineId::Omnivoice, start).await.unwrap();
        assert!(spawner.calls().is_empty());
    }

    #[tokio::test]
    async fn does_not_take_a_server_of_another_engine_or_another_program_for_its_own() {
        let fixture = Fixture::new();
        let _server = fixture.answer(serde_json::json!({"server": "clone-server", "engine": "cosyvoice3"}));
        let spawner = FakeSpawner::new(|_, _| Spawned { pid: None, exited: never() });
        assert_eq!(fixture.make(spawner).health(EngineId::Omnivoice, 1_500).await, None);
    }

    #[tokio::test]
    async fn starts_the_engines_python_with_its_voices_port_memory_ceiling_and_instance_and_waits_for_that_instance() {
        let fixture = Fixture::new();
        let servers = Arc::new(Mutex::new(Vec::new()));
        let started = servers.clone();
        let port = fixture.listener.clone();
        let spawner = FakeSpawner::new(move |_, args| {
            let instance = value_after(args, "--instance").to_string();
            let body =
                serde_json::json!({"server": "clone-server", "engine": "omnivoice", "instance": instance, "pid": 4242});
            let listener = port.lock().unwrap().take().unwrap();
            assert_eq!(listener.local_addr().unwrap().port().to_string(), value_after(args, "--port"));
            started.lock().unwrap().push(health_server(listener, Arc::new(Mutex::new(body))));
            Spawned { pid: Some(4242), exited: never() }
        });
        let clone = fixture.make(spawner.clone());
        clone
            .ensure(EngineId::Omnivoice, StartOptions { device: Device::Mps, limit_mb: 6000, threads: 4 })
            .await
            .unwrap();
        let calls = spawner.calls();
        assert_eq!(calls.len(), 1);
        let (command, args) = &calls[0];
        assert!(command.ends_with("env/bin/python") || command.ends_with("env\\Scripts\\python.exe"), "{command}");
        let server_py = fixture.path("clone-server").join("server.py").to_string_lossy().into_owned();
        for expected in
            [server_py.as_str(), "--engine", "omnivoice", "--port", &fixture.port.to_string(), "--device", "mps"]
        {
            assert!(args.iter().any(|arg| arg == expected), "{expected} in {args:?}");
        }
        assert_eq!(value_after(args, "--voices"), fixture.path("voices").to_string_lossy());
        assert_eq!(value_after(args, "--mem-limit-mb"), "6000");
        let state = clone.state(EngineId::Omnivoice).await;
        assert_eq!((state.up, state.starting, state.ours), (true, false, true));
    }

    #[tokio::test]
    async fn says_why_a_server_did_not_come_up() {
        let fixture = Fixture::new();
        let log = fixture.path("logs").join("clone-omnivoice.log");
        let spawner = FakeSpawner::new(move |_, _| {
            std::fs::write(&log, "Loading weights: 10%\nModuleNotFoundError: No module named omnivoice\n").unwrap();
            Spawned { pid: Some(4243), exited: tokio::time::sleep(Duration::from_millis(50)).boxed() }
        });
        let start = StartOptions { device: Device::Cpu, limit_mb: 2000, threads: 4 };
        assert_eq!(
            fixture.make(spawner).ensure(EngineId::Omnivoice, start).await,
            Err("сервер голоса не запустился: ModuleNotFoundError: No module named omnivoice".into())
        );
    }

    #[tokio::test]
    async fn a_start_that_panics_fails_and_the_next_one_starts_again() {
        let fixture = Fixture::new();
        let reads = std::sync::atomic::AtomicUsize::new(0);
        let env: EnvFact = Arc::new(move || {
            if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("the environment could not be read");
            }
            HashMap::new()
        });
        let spawner = FakeSpawner::new(|_, _| Spawned { pid: None, exited: futures::future::ready(()).boxed() });
        let clone = fixture.make_with(spawner.clone(), env);
        let start = StartOptions { device: Device::Mps, limit_mb: 6000, threads: 4 };
        let failed = clone.ensure(EngineId::Omnivoice, start).await.unwrap_err();
        assert_eq!(failed, format!("сервер голоса не запустился: {PANICKED}"));
        let state = clone.state(EngineId::Omnivoice).await;
        assert_eq!((state.up, state.starting, state.error), (false, false, Some(failed)));
        assert!(spawner.calls().is_empty());
        assert_eq!(clone.ensure(EngineId::Omnivoice, start).await, Err("сервер голоса не запустился".into()));
        assert_eq!(spawner.calls().len(), 1, "started anew, not the first start's error again");
    }

    #[tokio::test]
    async fn uses_a_server_elsewhere_but_never_starts_one_there() {
        let fixture = Fixture::new();
        let spawner = FakeSpawner::new(|_, _| Spawned { pid: None, exited: never() });
        let start = StartOptions { device: Device::Cuda, limit_mb: 4000, threads: 4 };
        assert_eq!(
            fixture.make(spawner.clone()).ensure(EngineId::Cosyvoice3, start).await,
            Err("Сервер голоса http://example.invalid:8094/v1 не отвечает.".into())
        );
        assert!(spawner.calls().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stops_only_the_server_it_started_the_instance_in_its_run_file_must_answer() {
        use std::os::unix::process::ExitStatusExt;

        let fixture = Fixture::new();
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let (body, _server) = fixture.answer(
            serde_json::json!({"server": "clone-server", "engine": "omnivoice", "instance": "stranger", "pid": pid}),
        );
        std::fs::create_dir_all(fixture.path("run")).unwrap();
        let run = serde_json::json!({"pid": pid, "port": fixture.port, "instance": "ours", "startedAt": ""});
        std::fs::write(fixture.path("run").join("omnivoice.json"), run.to_string()).unwrap();
        let clone = fixture.make(FakeSpawner::new(|_, _| Spawned { pid: None, exited: never() }));
        assert!(!clone.stop(EngineId::Omnivoice).await);
        assert!(child.try_wait().unwrap().is_none(), "a stranger's server keeps running");

        body.lock().unwrap()["instance"] = Value::from("ours");
        // The stub keeps answering after the kill, so stop() waits its few seconds: the kill is what matters here.
        let stopping = clone.clone();
        tokio::spawn(async move { stopping.stop(EngineId::Omnivoice).await });
        let status = tokio::task::spawn_blocking(move || child.wait()).await.unwrap().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGTERM));
    }

    #[test]
    fn reads_the_last_meaningful_line_of_a_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("x.log");
        std::fs::write(&log, "a\n  b  \nLoading weights: 90%\n\n").unwrap();
        assert_eq!(read_tail(&log), "b");
        assert_eq!(read_tail(&dir.path().join("none.log")), "");
    }
}
