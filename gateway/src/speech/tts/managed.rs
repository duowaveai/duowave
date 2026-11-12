//! The own voice's server (tts-server: Silero and Piper) set up and kept by the desktop app itself
//! (docs/plans/speech-servers.md, stage 5). Nothing to install by hand on a new computer:
//!
//! - The first own voice chosen installs its Python: uv (the app's own, DUOWAVE_UV) makes an environment in
//!   DATA_DIR/speech/tts with PyTorch for this computer (PyPI's on a Mac, the processor's build elsewhere), the
//!   server's packages and Piper's environment beside it (Piper is GPL-3.0: its own program, never in the server's
//!   process). The folders carry the requirements' hash (`env-1a2b3c4d`): an app with other pins installs beside the
//!   old ones — never moved, uv's launchers know their folder — and `ready.json`, written last, says which serve.
//! - While an own voice is chosen the server runs: started with the gateway, raised again after a crash (2 s, 10 s;
//!   three failures in five minutes pause it until «Повторить»), stopped when another voice speaks.
//! - It ends with the app: started with a pipe on its stdin (`--exit-with-parent`), it ends when the pipe closes —
//!   the app's quit or crash alike.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::core::events::{EventBus, GatewayEvent};

/// The id the panel's «Голосу нужна модель …» and a pending voice know the environment by.
pub const RUNTIME_ID: &str = "tts-runtime";
pub const RUNTIME_TITLE: &str = "Свой голос (Silero и Piper)";
/// Where the app's own uv is (set by the app before the gateway starts).
const BUNDLED_UV_ENV: &str = crate::voice_lab::platform::BUNDLED_UV_ENV;
const PYTHON: &str = "3.12";
/// The server's /health within this long after its start, or the start failed.
const START_TIMEOUT: Duration = Duration::from_secs(90);
const FAILURE_WINDOW: Duration = Duration::from_secs(300);
const FAILURE_LIMIT: usize = 3;
const LOOK_EVERY: Duration = Duration::from_secs(30);
const LOG_ROTATE_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TtsPhase {
    /// Not installed: nothing asked for it yet.
    Missing,
    Installing,
    /// Installed, no own voice chosen: the server is not running.
    Idle,
    Starting,
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsStatus {
    pub phase: TtsPhase,
    /// The install's step, for the panel: «Ставлю PyTorch».
    pub stage: Option<String>,
    pub percent: u32,
    pub error: Option<String>,
    pub paused: bool,
    pub port: u16,
}

pub struct ManagedTtsOptions {
    /// DATA_DIR/speech/tts: the environments, ready.json, the logs.
    pub dir: PathBuf,
    /// tts-server: server.py and its requirements (the app's resources, the repository's folder elsewhere).
    pub server_dir: PathBuf,
    /// The voices' folder (TTS_MODELS_DIR): the gateway downloads into it, the server reads it.
    pub models_dir: PathBuf,
    /// Where Piper's environment goes: a short path. espeak-ng inside piper-tts keeps the path to its data in 160
    /// bytes, and DATA_DIR on a Mac (~/Library/Application Support/app.duowave.desktop/…) leaves no room for it.
    pub piper_root: PathBuf,
    pub port: u16,
}

#[derive(Default)]
struct State {
    installing: bool,
    stage: Option<String>,
    percent: u32,
    error: Option<String>,
    wanted: bool,
    running: bool,
    starting: bool,
    paused: bool,
    failures: VecDeque<Instant>,
}

pub struct ManagedTts {
    options: ManagedTtsOptions,
    events: EventBus,
    state: Mutex<State>,
    child: tokio::sync::Mutex<Option<tokio::process::Child>>,
    wake: Arc<Notify>,
    stop: CancellationToken,
}

impl Drop for ManagedTts {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The environment's programs, where uv puts them on each system.
fn program(env: &Path, name: &str) -> PathBuf {
    if cfg!(windows) {
        env.join("Scripts").join(format!("{name}.exe"))
    } else {
        env.join("bin").join(name)
    }
}

/// PyTorch's processor build outside a Mac (PyPI's is the CUDA one there: gigabytes); a Mac takes PyPI's.
fn torch_index() -> Option<&'static str> {
    (!cfg!(target_os = "macos")).then_some("https://download.pytorch.org/whl/cpu")
}

/// The pinned `torch==…` line of a requirements file.
fn torch_pin(requirements: &str) -> Option<String> {
    requirements.lines().map(str::trim).find(|line| line.starts_with("torch==")).map(str::to_string)
}

impl ManagedTts {
    pub fn new(options: ManagedTtsOptions, events: EventBus) -> Arc<ManagedTts> {
        let log = options.dir.join("logs").join("tts-server.log");
        if std::fs::metadata(&log).is_ok_and(|meta| meta.len() > LOG_ROTATE_BYTES) {
            let _ = std::fs::rename(&log, log.with_extension("log.1"));
        }
        Arc::new(ManagedTts {
            options,
            events,
            state: Mutex::new(State::default()),
            child: tokio::sync::Mutex::new(None),
            wake: Arc::new(Notify::new()),
            stop: CancellationToken::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        locked(&self.state)
    }

    pub fn port(&self) -> u16 {
        self.options.port
    }

    /// ready.json: which environments serve, for which requirements.
    fn ready(&self) -> Option<serde_json::Value> {
        let text = std::fs::read_to_string(self.options.dir.join("ready.json")).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn ready_dir(&self, key: &str) -> PathBuf {
        let name =
            self.ready().and_then(|ready| ready.get(key).and_then(serde_json::Value::as_str).map(str::to_string));
        self.options.dir.join(name.unwrap_or_else(|| key.to_string()))
    }

    fn env_dir(&self) -> PathBuf {
        self.ready_dir("env")
    }

    fn piper_dir(&self) -> PathBuf {
        let path = self
            .ready()
            .and_then(|ready| ready.get("piperPath").and_then(serde_json::Value::as_str).map(PathBuf::from));
        path.unwrap_or_else(|| self.ready_dir("piper"))
    }

    fn logs(&self) -> PathBuf {
        self.options.dir.join("logs")
    }

    /// The requirements this app carries, hashed: an environment of other pins is installed anew.
    fn requirements_hash(&self) -> String {
        let mut hasher = Sha256::new();
        for name in ["requirements.txt", "requirements-piper.txt"] {
            hasher.update(std::fs::read(self.options.server_dir.join(name)).unwrap_or_default());
        }
        hasher.update(PYTHON.as_bytes());
        hasher.update(torch_index().unwrap_or("pypi").as_bytes());
        hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// The environment is here, for these requirements.
    pub fn installed(&self) -> bool {
        let Some(ready) = self.ready() else { return false };
        ready.get("requirements").and_then(serde_json::Value::as_str) == Some(self.requirements_hash().as_str())
            && program(&self.env_dir(), "python").exists()
    }

    pub fn installing(&self) -> bool {
        self.lock().installing
    }

    pub fn status(&self) -> TtsStatus {
        let installed = self.installed();
        let state = self.lock();
        let phase = if state.installing {
            TtsPhase::Installing
        } else if state.running && !state.starting {
            TtsPhase::Ready
        } else if state.starting {
            TtsPhase::Starting
        } else if state.error.is_some() {
            TtsPhase::Failed
        } else if installed {
            TtsPhase::Idle
        } else {
            TtsPhase::Missing
        };
        TtsStatus {
            phase,
            stage: state.stage.clone(),
            percent: state.percent,
            error: state.error.clone(),
            paused: state.paused,
            port: self.options.port,
        }
    }

    /// About what the install takes here, MB: downloaded and on the disk.
    pub fn sizes() -> (u64, u64) {
        if cfg!(target_os = "macos") {
            (250, 900)
        } else {
            (450, 1_600)
        }
    }

    /// Whether an own voice is chosen: the server runs while it is, and stops when another voice speaks.
    pub fn want(&self, wanted: bool) {
        {
            let mut state = self.lock();
            if state.wanted == wanted {
                return;
            }
            state.wanted = wanted;
        }
        self.wake.notify_one();
    }

    /// The gateway stops: the server ends now, an install stops, and nothing starts again (the desktop app's restart
    /// of the gateway keeps the process, and the old gateway's parts are not always dropped).
    pub async fn shutdown(&self) {
        self.stop.cancel();
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
            tracing::info!("own voice server: stopped with the gateway");
        }
        self.lock().running = false;
    }

    /// «Повторить» / «Перезапустить»: the server started anew, the failures forgotten; an install that failed runs
    /// again.
    pub async fn restart(self: &Arc<Self>) {
        let reinstall = {
            let mut state = self.lock();
            state.paused = false;
            state.failures.clear();
            state.error = None;
            !state.installing && !self.installed() && state.wanted
        };
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
        }
        self.lock().running = false;
        if reinstall {
            self.install();
        }
        self.wake.notify_one();
    }

    /// The background that keeps the server as the choice wants it; it holds the service weakly.
    pub fn start(self: &Arc<Self>) {
        let weak: Weak<ManagedTts> = Arc::downgrade(self);
        let (wake, stop) = (self.wake.clone(), self.stop.clone());
        tokio::spawn(async move {
            loop {
                let Some(me) = weak.upgrade() else { break };
                me.settle().await;
                drop(me);
                tokio::select! {
                    _ = wake.notified() => {}
                    _ = tokio::time::sleep(LOOK_EVERY) => {}
                    _ = stop.cancelled() => break,
                }
            }
        });
    }

    async fn settle(self: &Arc<Self>) {
        let (wanted, paused, installing) = {
            let state = self.lock();
            (state.wanted, state.paused, state.installing)
        };
        let mut child = self.child.lock().await;
        if !wanted {
            if let Some(mut running) = child.take() {
                let _ = running.kill().await;
                tracing::info!("own voice server: stopped (no own voice speaks)");
            }
            self.lock().running = false;
            return;
        }
        if installing || paused || self.stop.is_cancelled() || !self.installed() {
            return;
        }
        if let Some(running) = child.as_mut() {
            match running.try_wait() {
                Ok(None) => return,
                _ => {
                    child.take();
                    self.lock().running = false;
                    let tail = self.log_tail();
                    self.note_failure(format!("сервер голоса закончился сам{tail}"));
                    if self.lock().paused {
                        return;
                    }
                }
            }
        }
        let pause = match self.lock().failures.len() {
            0 => Duration::ZERO,
            1 => Duration::from_secs(2),
            _ => Duration::from_secs(10),
        };
        if !pause.is_zero() {
            tokio::select! {
                _ = tokio::time::sleep(pause) => {}
                _ = self.stop.cancelled() => return,
            }
        }
        match self.spawn().await {
            Ok(started) => {
                *child = Some(started);
                drop(child);
                self.lock().starting = true;
                let me = self.clone();
                tokio::spawn(async move { me.await_health().await });
            }
            Err(error) => self.note_failure(error),
        }
    }

    async fn spawn(&self) -> Result<tokio::process::Child, String> {
        let logs = self.logs();
        std::fs::create_dir_all(&logs).map_err(|error| format!("{}: {error}", logs.display()))?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(logs.join("tts-server.log"))
            .map_err(|error| error.to_string())?;
        let err = log.try_clone().map_err(|error| error.to_string())?;
        let python = program(&self.env_dir(), "python");
        let mut command = tokio::process::Command::new(&python);
        command
            .arg("-u")
            .arg(self.options.server_dir.join("server.py"))
            .args(["--host", "127.0.0.1", "--port", &self.options.port.to_string(), "--exit-with-parent"])
            .arg("--models")
            .arg(&self.options.models_dir)
            .arg("--piper")
            .arg(program(&self.piper_dir(), "piper"))
            // Compiled files in a cache of the app's own, never beside the code in the app's bundle (its signature).
            .env("PYTHONPYCACHEPREFIX", self.options.dir.join("pycache"))
            .stdin(Stdio::piped())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err))
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let child = command.spawn().map_err(|error| format!("сервер голоса не запустился: {error}"))?;
        tracing::info!("own voice server: starting on port {}", self.options.port);
        Ok(child)
    }

    /// The new server's /health, or why it is not up.
    async fn await_health(self: Arc<Self>) {
        let url = format!("http://127.0.0.1:{}/health", self.options.port);
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            let up = crate::utils::http::client()
                .get(&url)
                .timeout(Duration::from_millis(1500))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success());
            let exited = {
                let mut child = self.child.lock().await;
                match child.as_mut() {
                    Some(child) => !matches!(child.try_wait(), Ok(None)),
                    None => true,
                }
            };
            if up && !exited {
                let mut state = self.lock();
                state.starting = false;
                state.running = true;
                state.error = None;
                tracing::info!("own voice server: up on port {}", self.options.port);
                return;
            }
            if exited || Instant::now() > deadline {
                self.lock().starting = false;
                let tail = self.log_tail();
                let why = if exited {
                    format!("сервер голоса не запустился{tail}")
                } else {
                    format!("сервер голоса не ответил за {} с{tail}", START_TIMEOUT.as_secs())
                };
                if let Some(mut child) = self.child.lock().await.take() {
                    let _ = child.kill().await;
                }
                self.note_failure(why);
                self.wake.notify_one();
                return;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                _ = self.stop.cancelled() => return,
            }
        }
    }

    fn note_failure(&self, why: String) {
        tracing::warn!("own voice server: {why}");
        let mut state = self.lock();
        let now = Instant::now();
        state.failures.push_back(now);
        while state.failures.front().is_some_and(|at| now.duration_since(*at) > FAILURE_WINDOW) {
            state.failures.pop_front();
        }
        state.error = Some(why.clone());
        if state.failures.len() >= FAILURE_LIMIT {
            state.paused = true;
            state.error = Some(format!("сервер голоса падал {} раза за 5 минут: {why}", state.failures.len()));
        }
    }

    /// The server log's last line, as «: …», or nothing.
    fn log_tail(&self) -> String {
        let text = std::fs::read_to_string(self.logs().join("tts-server.log")).unwrap_or_default();
        text.lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty())
            .map(|line| format!(": {}", line.chars().take(200).collect::<String>()))
            .unwrap_or_default()
    }

    /// Installs the environment in the background (a second call while one runs does nothing). Its end wakes a
    /// pending voice (VoiceEngineInstalled with RUNTIME_ID).
    pub fn install(self: &Arc<Self>) {
        {
            let mut state = self.lock();
            if state.installing {
                return;
            }
            state.installing = true;
            state.error = None;
            state.stage = Some("Готовлю установку".into());
            state.percent = 1;
        }
        let me = self.clone();
        tokio::spawn(async move {
            let result = me.install_now().await;
            {
                let mut state = me.lock();
                state.installing = false;
                state.stage = None;
                match &result {
                    Ok(()) => {
                        state.percent = 100;
                        state.error = None;
                    }
                    Err(why) => {
                        state.percent = 0;
                        state.error = Some(why.clone());
                    }
                }
            }
            if result.is_ok() {
                // A server of the old environment, if one runs: the next look starts it on the new one.
                if let Some(mut child) = me.child.lock().await.take() {
                    let _ = child.kill().await;
                }
                me.lock().running = false;
            }
            match result {
                Ok(()) => {
                    tracing::info!("own voice server: its environment is installed");
                    me.events.emit(GatewayEvent::VoiceEngineInstalled {
                        engine: RUNTIME_ID.into(),
                        device: "cpu".into(),
                        actor: None,
                    });
                }
                Err(why) => {
                    tracing::warn!("own voice server: the install failed: {why}");
                    // A pending voice learns of it the same way and fails with the reason.
                    me.events.emit(GatewayEvent::VoiceEngineInstalled {
                        engine: RUNTIME_ID.into(),
                        device: String::new(),
                        actor: None,
                    });
                }
            }
            me.wake.notify_one();
        });
    }

    fn stage(&self, label: &str, percent: u32) {
        let mut state = self.lock();
        state.stage = Some(label.into());
        state.percent = percent;
    }

    async fn install_now(&self) -> Result<(), String> {
        let uv = find_uv().await.ok_or_else(|| {
            "Не нашёл uv — установщик Python. В приложении он свой; без приложения: https://docs.astral.sh/uv/"
                .to_string()
        })?;
        let dir = &self.options.dir;
        std::fs::create_dir_all(self.logs()).map_err(|error| format!("{}: {error}", dir.display()))?;
        let free = crate::voice_lab::platform::free_disk_bytes(dir) / 1_000_000;
        let need = Self::sizes().1 + 500;
        if free > 0 && free < need {
            return Err(format!("На диске свободно {free} МБ, а нужно около {need} МБ."));
        }
        let requirements = self.options.server_dir.join("requirements.txt");
        let piper_requirements = self.options.server_dir.join("requirements-piper.txt");
        let pins =
            std::fs::read_to_string(&requirements).map_err(|error| format!("{}: {error}", requirements.display()))?;
        let hash = self.requirements_hash();
        let tag = &hash[..8];
        let (env_name, piper_name) = (format!("env-{tag}"), format!("piper-{tag}"));
        let (env_new, piper_new) = (dir.join(&env_name), self.options.piper_root.join(&piper_name));
        // A try that stopped halfway leaves its folders: from the start.
        for old in [&env_new, &piper_new] {
            let _ = std::fs::remove_dir_all(old);
        }
        let run = |args: Vec<String>, label: &'static str, percent: u32| {
            let uv = uv.clone();
            async move {
                self.stage(label, percent);
                self.uv(&uv, &args).await
            }
        };
        let path = |p: &Path| p.to_string_lossy().into_owned();
        run(vec!["venv".into(), "--python".into(), PYTHON.into(), path(&env_new)], "Ставлю Python", 5).await?;
        let python = path(&program(&env_new, "python"));
        if let (Some(index), Some(torch)) = (torch_index(), torch_pin(&pins)) {
            run(
                vec![
                    "pip".into(),
                    "install".into(),
                    "-p".into(),
                    python.clone(),
                    torch,
                    "--index-url".into(),
                    index.into(),
                ],
                "Ставлю PyTorch",
                15,
            )
            .await?;
        }
        run(
            vec!["pip".into(), "install".into(), "-p".into(), python.clone(), "-r".into(), path(&requirements)],
            "Ставлю библиотеки голоса",
            45,
        )
        .await?;
        run(vec!["venv".into(), "--python".into(), PYTHON.into(), path(&piper_new)], "Ставлю Piper", 75).await?;
        let piper_python = path(&program(&piper_new, "python"));
        run(
            vec!["pip".into(), "install".into(), "-p".into(), piper_python, "-r".into(), path(&piper_requirements)],
            "Ставлю Piper",
            80,
        )
        .await?;
        self.stage("Проверяю установку", 92);
        let check = tokio::process::Command::new(&python)
            .args(["-c", "import torch, numpy, scipy; print(torch.__version__)"])
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|error| format!("проверка не запустилась: {error}"))?;
        if !check.status.success() {
            return Err(format!(
                "библиотеки голоса не загрузились: {}",
                String::from_utf8_lossy(&check.stderr).lines().last().unwrap_or("")
            ));
        }
        let ready = serde_json::json!({
            "requirements": hash,
            "env": env_name,
            "piper": piper_name,
            "piperPath": piper_new.to_string_lossy(),
            "torch": String::from_utf8_lossy(&check.stdout).trim(),
            "installedAt": crate::utils::time::now_iso(),
        });
        std::fs::write(dir.join("ready.json"), serde_json::to_string_pretty(&ready).unwrap_or_default())
            .map_err(|error| error.to_string())?;
        // The wheels are in the environments now: the download cache only takes the disk. Older environments go —
        // one a running server still holds stays until the next install (Windows does not let it go).
        let _ = std::fs::remove_dir_all(dir.join("cache"));
        for root in [dir.as_path(), self.options.piper_root.as_path()] {
            let Ok(entries) = std::fs::read_dir(root) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let old = (name.starts_with("env-") && name != env_name)
                    || (name.starts_with("piper-") && name != piper_name);
                if old {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        Ok(())
    }

    /// One uv command with its Python and cache inside DATA_DIR/speech/tts; its output into the install log.
    async fn uv(&self, uv: &str, args: &[String]) -> Result<(), String> {
        let dir = &self.options.dir;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.logs().join("install.log"))
            .map_err(|error| error.to_string())?;
        let err = log.try_clone().map_err(|error| error.to_string())?;
        let mut command = tokio::process::Command::new(uv);
        command
            .args(args)
            .env("UV_PYTHON_INSTALL_DIR", dir.join("python"))
            .env("UV_CACHE_DIR", dir.join("cache"))
            .env("UV_PYTHON_PREFERENCE", "only-managed")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err))
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let mut child = command.spawn().map_err(|error| format!("uv не запустился: {error}"))?;
        let status = tokio::select! {
            status = child.wait() => status.map_err(|error| error.to_string())?,
            _ = self.stop.cancelled() => {
                let _ = child.kill().await;
                return Err("остановлено".into());
            }
        };
        if status.success() {
            return Ok(());
        }
        let text = std::fs::read_to_string(self.logs().join("install.log")).unwrap_or_default();
        let last = text.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or("");
        Err(format!(
            "uv {}: {}",
            args.first().map(String::as_str).unwrap_or(""),
            last.chars().take(200).collect::<String>()
        ))
    }
}

/// The app's own uv (DUOWAVE_UV), else uv on PATH.
async fn find_uv() -> Option<String> {
    if let Some(bundled) = std::env::var_os(BUNDLED_UV_ENV).map(PathBuf::from).filter(|path| path.is_file()) {
        return Some(bundled.to_string_lossy().into_owned());
    }
    let answered = tokio::process::Command::new("uv")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success());
    answered.then(|| "uv".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tts(dir: &Path) -> Arc<ManagedTts> {
        let server = dir.join("tts-server");
        std::fs::create_dir_all(&server).unwrap();
        std::fs::write(server.join("requirements.txt"), "numpy==2.5.4\ntorch==2.15.0\nscipy==1.18.1\n").unwrap();
        std::fs::write(server.join("requirements-piper.txt"), "piper-tts==1.8.0\n").unwrap();
        ManagedTts::new(
            ManagedTtsOptions {
                dir: dir.join("speech").join("tts"),
                server_dir: server,
                models_dir: dir.join("voices"),
                piper_root: dir.join("piper"),
                port: 18091,
            },
            EventBus::new(),
        )
    }

    #[test]
    fn the_torch_pin_is_read_from_the_requirements() {
        assert_eq!(torch_pin("# x\nnumpy==2.5.4\ntorch==2.15.0\n").as_deref(), Some("torch==2.15.0"));
        assert_eq!(torch_pin("numpy==2.5.4\n"), None);
    }

    #[test]
    fn installed_means_ready_json_for_these_very_requirements() {
        let dir = tempfile::tempdir().unwrap();
        let tts = tts(dir.path());
        assert!(!tts.installed());
        assert_eq!(tts.status().phase, TtsPhase::Missing);
        let speech = dir.path().join("speech").join("tts");
        std::fs::create_dir_all(program(&speech.join("env-1a2b3c4d"), "python").parent().unwrap()).unwrap();
        std::fs::write(program(&speech.join("env-1a2b3c4d"), "python"), "").unwrap();
        let ready = serde_json::json!({ "requirements": tts.requirements_hash(), "env": "env-1a2b3c4d", "piper": "piper-1a2b3c4d" });
        std::fs::write(speech.join("ready.json"), ready.to_string()).unwrap();
        assert!(tts.installed());
        assert_eq!(tts.status().phase, TtsPhase::Idle);
        // The app's new pins: installed anew.
        std::fs::write(dir.path().join("tts-server").join("requirements.txt"), "torch==2.16.0\n").unwrap();
        assert!(!tts.installed());
    }

    #[tokio::test]
    async fn not_wanted_nothing_runs_and_wanted_without_an_install_nothing_starts() {
        let dir = tempfile::tempdir().unwrap();
        let tts = tts(dir.path());
        tts.settle().await;
        assert!(tts.child.lock().await.is_none());
        tts.want(true);
        tts.settle().await;
        assert!(tts.child.lock().await.is_none());
        assert_eq!(tts.status().phase, TtsPhase::Missing);
    }
}
