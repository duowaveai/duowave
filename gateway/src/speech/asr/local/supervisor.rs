//! The gateway's side of recognition on this computer: [`LocalAsr`] owns the worker process and keeps it serving.
//!
//! - The chosen model's file is fetched once (download.rs) into DATA_DIR/speech/models.
//! - A worker (this program with `--asr-worker`) loads it; recognition goes to it over its stdin and stdout.
//! - A worker that dies, hangs or loses its GPU is ended and replaced, the phrase asked again once. A GPU at fault
//!   puts the next workers on the processor until «Повторить». Three failures in five minutes pause the restarts:
//!   the panel shows why, and «Повторить» starts again.
//! - A new model loads in a new worker while the old one goes on serving; it takes over once it is in.
//! - The worker ends with the gateway: it reads its stdin, and the gateway's end closes it, a crash too.
//!
//! The way Handy keeps its engine (engine_supervisor/supervisor.rs, MIT), cut to what a gateway needs: one model,
//! whole phrases, no streaming.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::catalog::{self, GgufModel};
use super::download::{self, DownloadError, Expected};
use super::protocol::{receive, send, Request, Response};
use super::{CPU_ONLY_FLAG, WORKER_FLAG};

/// A load: the first Metal start compiles its shaders (~15 s after an install or update), a big model reads ~1 GB.
const LOAD_TIMEOUT: Duration = Duration::from_secs(180);
/// A phrase: at least this long, longer for long audio (a slow processor, the first run after a load).
const RUN_FLOOR: Duration = Duration::from_secs(30);
const RUN_PER_AUDIO_SECOND: Duration = Duration::from_secs(4);
/// Failures counted within this window; this many pause the restarts.
const FAILURE_WINDOW: Duration = Duration::from_secs(300);
const FAILURE_LIMIT: usize = 3;
/// How often the background looks at the worker without being woken.
const LOOK_EVERY: Duration = Duration::from_secs(30);
/// A log above this size moves to `.1` when the gateway starts.
const LOG_ROTATE_BYTES: u64 = 5 * 1024 * 1024;
const SAMPLE_RATE: u32 = 16_000;
/// A phrase that comes while the model is still loading (the gateway's first seconds) waits this long for it.
const LOADING_WAIT: Duration = Duration::from_secs(15);
/// Megabytes as people count them (the files' sizes on Hugging Face).
const MB: u64 = 1_000_000;

/// A started worker's two pipes, and the process to end it by (none for a test's worker in memory).
pub struct Conn {
    pub reader: Box<dyn AsyncRead + Send + Unpin>,
    pub writer: Box<dyn AsyncWrite + Send + Unpin>,
    pub child: Option<tokio::process::Child>,
}

impl Conn {
    async fn call(&mut self, request: &Request, pcm: &[f32], deadline: Duration) -> Result<Response, Broken> {
        let exchange = async {
            send(&mut self.writer, request, pcm).await?;
            receive::<Response>(&mut self.reader).await.map(|(response, _)| response)
        };
        match tokio::time::timeout(deadline, exchange).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => Err(Broken::Died(error.to_string())),
            Err(_) => Err(Broken::Hung(deadline)),
        }
    }

    /// Whether the process is still there (a test's worker always is: its end shows on the next call).
    fn alive(&mut self) -> bool {
        match &mut self.child {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => true,
        }
    }
}

/// How a worker went wrong in a call.
#[derive(Debug)]
enum Broken {
    Died(String),
    Hung(Duration),
}

impl std::fmt::Display for Broken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Broken::Died(why) => write!(f, "исполнитель распознавания упал ({why})"),
            Broken::Hung(after) => write!(f, "исполнитель распознавания завис (нет ответа {} с)", after.as_secs()),
        }
    }
}

/// How a worker is started: this program again, or a test's worker in memory.
pub trait Launch: Send + Sync {
    fn launch(&self, cpu_only: bool) -> std::io::Result<Conn>;
}

/// This program with `--asr-worker`: its stderr into the worker's log, no console window on Windows, ended when
/// dropped.
pub struct ProcessLaunch {
    pub exe: PathBuf,
    pub log: PathBuf,
}

impl Launch for ProcessLaunch {
    fn launch(&self, cpu_only: bool) -> std::io::Result<Conn> {
        if let Some(dir) = self.log.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let log = std::fs::OpenOptions::new().create(true).append(true).open(&self.log)?;
        let mut command = tokio::process::Command::new(&self.exe);
        command.arg(WORKER_FLAG);
        if cpu_only {
            command.arg(CPU_ONLY_FLAG);
        }
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::from(log)).kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = command.spawn()?;
        let reader = child.stdout.take().ok_or_else(|| std::io::Error::other("no stdout"))?;
        let writer = child.stdin.take().ok_or_else(|| std::io::Error::other("no stdin"))?;
        Ok(Conn { reader: Box::new(reader), writer: Box::new(writer), child: Some(child) })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// No model chosen yet.
    Idle,
    Downloading,
    Loading,
    Ready,
    /// The last try failed; `error` says why. Restarts are paused after too many failures.
    Failed,
}

/// What the panel and health see.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalStatus {
    /// The chosen model (the catalog's name).
    pub model: Option<String>,
    pub phase: Phase,
    /// The download, while there is one.
    pub bytes: u64,
    pub total: u64,
    /// The model the worker holds now: the old one while a new one downloads or loads.
    pub serving: Option<String>,
    pub backend: Option<String>,
    pub device: Option<String>,
    pub error: Option<String>,
    /// A GPU failed: the processor recognises until «Повторить».
    pub gpu_failed: bool,
    /// Restarts paused after too many failures in a row.
    pub paused: bool,
    /// The models whose file is here.
    pub installed: Vec<String>,
}

pub struct LocalAsrOptions {
    /// DATA_DIR/speech: models/ and the worker's log live here.
    pub dir: PathBuf,
    pub mirror: Option<String>,
    /// ASR_LANGUAGE: the hint for a many-language model.
    pub language: Option<String>,
    /// Whether a GPU may be used at all (ASR_LOCAL_GPU).
    pub gpu: bool,
    /// How long a phrase may take at the least (30 s; a test's is short).
    pub run_floor: Duration,
    /// The pauses before a new worker after one and after two recent failures (2 s, 10 s).
    pub pauses: [Duration; 2],
}

impl LocalAsrOptions {
    pub fn new(dir: PathBuf, mirror: Option<String>, language: Option<String>, gpu: bool) -> LocalAsrOptions {
        LocalAsrOptions {
            dir,
            mirror,
            language,
            gpu,
            run_floor: RUN_FLOOR,
            pauses: [Duration::from_secs(2), Duration::from_secs(10)],
        }
    }
}

struct Live {
    conn: Conn,
    model: String,
    cpu_only: bool,
    backend: String,
    device: String,
}

#[derive(Default)]
struct State {
    chosen: Option<String>,
    phase: Option<Phase>,
    bytes: u64,
    total: u64,
    error: Option<String>,
    gpu_failed: bool,
    paused: bool,
    failures: VecDeque<Instant>,
    /// The download under way and the model it is for: a new choice stops it.
    downloading: Option<(String, CancellationToken)>,
}

pub struct LocalAsr {
    options: LocalAsrOptions,
    launch: Arc<dyn Launch>,
    state: Mutex<State>,
    live: Arc<tokio::sync::Mutex<Option<Live>>>,
    /// What `serving` says without waiting for the worker's lock: model, backend, device.
    serving: Mutex<Option<(String, String, String)>>,
    wake: Arc<Notify>,
    stop: CancellationToken,
}

impl Drop for LocalAsr {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some((_, cancel)) = self.lock().downloading.take() {
            cancel.cancel();
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl LocalAsr {
    /// The real one: workers are this program again.
    pub fn new(options: LocalAsrOptions) -> Result<Arc<LocalAsr>> {
        let exe = std::env::current_exe().map_err(|error| anyhow!("where this program is: {error}"))?;
        let log = log_path(&options.dir);
        rotate(&log);
        Ok(LocalAsr::with_launch(options, Arc::new(ProcessLaunch { exe, log })))
    }

    pub fn with_launch(options: LocalAsrOptions, launch: Arc<dyn Launch>) -> Arc<LocalAsr> {
        Arc::new(LocalAsr {
            options,
            launch,
            state: Mutex::new(State::default()),
            live: Arc::new(tokio::sync::Mutex::new(None)),
            serving: Mutex::new(None),
            wake: Arc::new(Notify::new()),
            stop: CancellationToken::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        locked(&self.state)
    }

    pub fn models_dir(&self) -> PathBuf {
        self.options.dir.join("models")
    }

    pub fn log_path(&self) -> PathBuf {
        log_path(&self.options.dir)
    }

    fn file_of(&self, entry: &GgufModel) -> PathBuf {
        self.models_dir().join(entry.file)
    }

    /// The background that keeps the chosen model serving: woken by a choice, a retry or a failure, and every
    /// 30 s besides to see the worker is still there. It holds the service weakly: the gateway's end ends it.
    pub fn start(self: &Arc<Self>) {
        let weak: Weak<LocalAsr> = Arc::downgrade(self);
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

    /// The model to serve. A new one stops a download of another; the old model serves until the new one is in.
    pub fn choose(&self, model: &str) {
        {
            let mut state = self.lock();
            if state.chosen.as_deref() == Some(model) {
                return;
            }
            state.chosen = Some(model.to_string());
            state.error = None;
            state.paused = false;
            state.failures.clear();
            if let Some((other, cancel)) = state.downloading.take() {
                if other != model {
                    cancel.cancel();
                } else {
                    state.downloading = Some((other, cancel));
                }
            }
        }
        self.wake.notify_one();
    }

    pub fn chosen(&self) -> Option<String> {
        self.lock().chosen.clone()
    }

    /// «Повторить»: the failures forgotten, the GPU tried again, the chosen model fetched or loaded anew.
    pub fn retry(&self) {
        {
            let mut state = self.lock();
            state.error = None;
            state.paused = false;
            state.gpu_failed = false;
            state.failures.clear();
        }
        self.wake.notify_one();
    }

    /// The gateway stops: the background ends, a download stops, the worker ends now, and nothing starts another.
    /// Called by the gateway's own shutdown — in the desktop app a restart of the gateway keeps the process, and the
    /// old gateway's parts are not always dropped (12.11: a worker with its model stayed behind every restart).
    pub async fn shutdown(&self) {
        self.stop.cancel();
        if let Some((_, cancel)) = self.lock().downloading.take() {
            cancel.cancel();
        }
        if let Some(mut live) = self.live.lock().await.take() {
            if let Some(child) = live.conn.child.as_mut() {
                let _ = child.kill().await;
            }
        }
        *locked(&self.serving) = None;
        tracing::info!(target: "speech", "local recognition: stopped with the gateway");
    }

    /// «Перезапустить»: a new worker, as after «Повторить».
    pub async fn restart(&self) {
        self.retry();
        self.live.lock().await.take();
        *locked(&self.serving) = None;
        self.wake.notify_one();
    }

    pub fn status(&self) -> LocalStatus {
        let serving = locked(&self.serving).clone();
        let installed = catalog::MODELS
            .iter()
            .filter(|entry| download::present(&self.file_of(entry), entry.size))
            .map(|entry| entry.model.to_string())
            .collect();
        let state = self.lock();
        let phase = match (state.phase, &state.chosen) {
            (Some(phase), _) => phase,
            (None, None) => Phase::Idle,
            (None, Some(_)) => Phase::Loading,
        };
        LocalStatus {
            model: state.chosen.clone(),
            phase,
            bytes: state.bytes,
            total: state.total,
            serving: serving.as_ref().map(|(model, _, _)| model.clone()),
            backend: serving.as_ref().map(|(_, backend, _)| backend.clone()),
            device: serving.as_ref().map(|(_, _, device)| device.clone()).filter(|device| !device.is_empty()),
            error: state.error.clone(),
            gpu_failed: state.gpu_failed,
            paused: state.paused,
            installed,
        }
    }

    /// The model that serves now, else the chosen one.
    pub fn model(&self) -> String {
        locked(&self.serving)
            .as_ref()
            .map(|(model, _, _)| model.clone())
            .or_else(|| self.chosen())
            .unwrap_or_else(|| catalog::DEFAULT_MODEL.to_string())
    }

    /// Whether a phrase can be recognised now.
    pub fn ready(&self) -> bool {
        locked(&self.serving).is_some()
    }

    fn set_phase(&self, phase: Phase) {
        self.lock().phase = Some(phase);
    }

    fn fail(&self, why: String) {
        let mut state = self.lock();
        state.phase = Some(Phase::Failed);
        state.error = Some(why);
    }

    /// A worker failed: counted; too many in the window pause the restarts. `gpu`: the GPU was at fault.
    fn note_failure(&self, why: &str, gpu: bool) {
        let mut state = self.lock();
        let now = Instant::now();
        state.failures.push_back(now);
        while state.failures.front().is_some_and(|at| now.duration_since(*at) > FAILURE_WINDOW) {
            state.failures.pop_front();
        }
        if gpu && !state.gpu_failed {
            state.gpu_failed = true;
            tracing::warn!(target: "speech", "local recognition: the GPU failed ({why}); the processor recognises now");
        } else {
            tracing::warn!(target: "speech", "local recognition: {why}");
        }
        if state.failures.len() >= FAILURE_LIMIT {
            state.paused = true;
            state.phase = Some(Phase::Failed);
            state.error = Some(format!("распознавание падало {} раза за 5 минут: {why}", state.failures.len()));
        }
    }

    fn may_restart(&self) -> bool {
        !self.lock().paused
    }

    fn cpu_only(&self) -> bool {
        !self.options.gpu || self.lock().gpu_failed
    }

    fn language_for(&self, entry: &GgufModel) -> Option<String> {
        entry.language.map(str::to_string).or_else(|| self.options.language.clone().filter(|lang| !lang.is_empty()))
    }

    /// One look: the chosen model fetched if missing, loaded if not serving, a dead worker replaced.
    async fn settle(&self) {
        let Some(model) = self.chosen() else { return };
        if self.lock().paused {
            return;
        }
        let Some(entry) = catalog::find(&model) else {
            self.fail(format!("Модели «{model}» нет для распознавания на этом компьютере — выберите другую."));
            return;
        };
        let path = self.file_of(entry);
        if !download::present(&path, entry.size) {
            if let Err(error) = self.fetch(entry, &path).await {
                if error != DownloadError::Cancelled {
                    self.fail(format!("Модель не скачалась: {error}"));
                }
                return;
            }
        }
        {
            let mut live = self.live.lock().await;
            if let Some(worker) = live.as_mut() {
                if worker.conn.alive() && worker.model == model {
                    *locked(&self.serving) =
                        Some((worker.model.clone(), worker.backend.clone(), worker.device.clone()));
                    let mut state = self.lock();
                    state.phase = Some(Phase::Ready);
                    state.error = None;
                    return;
                }
                if !worker.conn.alive() {
                    live.take();
                    *locked(&self.serving) = None;
                    drop(live);
                    self.note_failure("исполнитель распознавания закончился сам", false);
                    if !self.may_restart() {
                        return;
                    }
                }
            }
        }
        // A pause after recent failures: 2 s after the first, 10 s after the second.
        let pause = match self.lock().failures.len() {
            0 => Duration::ZERO,
            1 => self.options.pauses[0],
            _ => self.options.pauses[1],
        };
        if !pause.is_zero() {
            tokio::select! {
                _ = tokio::time::sleep(pause) => {}
                _ = self.stop.cancelled() => return,
            }
        }
        self.set_phase(Phase::Loading);
        match self.spawn_loaded(entry, &path).await {
            Ok(worker) => {
                tracing::info!(
                    target: "speech",
                    "local recognition: {} loaded on {}{}",
                    worker.model,
                    worker.backend,
                    if worker.device.is_empty() { String::new() } else { format!(" ({})", worker.device) }
                );
                // Stopped while it loaded: it ends here (its process is killed when dropped).
                if self.stop.is_cancelled() {
                    return;
                }
                *locked(&self.serving) = Some((worker.model.clone(), worker.backend.clone(), worker.device.clone()));
                // The old worker, if any, ends here (its process is killed when dropped).
                *self.live.lock().await = Some(worker);
                let mut state = self.lock();
                state.phase = Some(Phase::Ready);
                state.error = None;
            }
            Err(error) => {
                // A file that will not load may be damaged on the disk (its size alone made it «here»): checked by
                // its hash, and a damaged one is fetched again instead of failing for good.
                if !file_matches(&path, entry.sha256).await {
                    tracing::warn!(target: "speech", "local recognition: {} is damaged on the disk; fetching it again", entry.file);
                    let _ = tokio::fs::remove_file(&path).await;
                    let mut state = self.lock();
                    state.failures.clear();
                    state.error = None;
                    drop(state);
                    self.wake.notify_one();
                    return;
                }
                let why = error.to_string().replace(&*path.to_string_lossy(), entry.file);
                self.fail(why);
                // Asked again after the pause, unless the failures paused it.
                if self.may_restart() {
                    self.wake.notify_one();
                }
            }
        }
    }

    async fn fetch(&self, entry: &GgufModel, path: &Path) -> Result<(), DownloadError> {
        let cancel = CancellationToken::new();
        {
            let mut state = self.lock();
            state.downloading = Some((entry.model.to_string(), cancel.clone()));
            state.phase = Some(Phase::Downloading);
            state.bytes = 0;
            state.total = entry.size;
            state.error = None;
        }
        tracing::info!(target: "speech", "local recognition: fetching {} ({} MB)", entry.model, entry.size / MB);
        let urls = entry.urls(self.options.mirror.as_deref());
        let progress = |bytes: u64, total: u64| {
            let mut state = self.lock();
            state.bytes = bytes;
            state.total = total;
        };
        let stop = self.stop.clone();
        let both = cancel.clone();
        let watch = tokio::spawn(async move {
            stop.cancelled().await;
            both.cancel();
        });
        let result =
            download::fetch(&urls, path, Expected { size: entry.size, sha256: entry.sha256 }, &progress, &cancel).await;
        watch.abort();
        let mut state = self.lock();
        if state.downloading.as_ref().is_some_and(|(model, _)| model == entry.model) {
            state.downloading = None;
        }
        if result.is_ok() {
            tracing::info!(target: "speech", "local recognition: {} is here", entry.model);
        }
        result
    }

    /// A new worker with the model in it; the GPU's failure in the load puts it on the processor and tries again.
    async fn spawn_loaded(&self, entry: &GgufModel, path: &Path) -> Result<Live> {
        if self.stop.is_cancelled() {
            bail!("распознавание остановлено вместе со шлюзом");
        }
        let cpu_only = self.cpu_only();
        match self.try_spawn_loaded(entry, path, cpu_only).await {
            Ok(worker) => Ok(worker),
            Err((why, gpu)) if gpu && !cpu_only => {
                self.note_failure(&why, true);
                self.try_spawn_loaded(entry, path, true).await.map_err(|(why, _)| anyhow!(why))
            }
            Err((why, _)) => {
                self.note_failure(&why, false);
                Err(anyhow!(why))
            }
        }
    }

    /// Err: why, and whether the GPU is to blame (a GPU worker that crashed, hung or reported its backend).
    async fn try_spawn_loaded(&self, entry: &GgufModel, path: &Path, cpu_only: bool) -> Result<Live, (String, bool)> {
        let mut conn = self
            .launch
            .launch(cpu_only)
            .map_err(|error| (format!("исполнитель распознавания не запустился: {error}"), false))?;
        let request = Request::Load { path: path.to_path_buf() };
        match conn.call(&request, &[], LOAD_TIMEOUT).await {
            Ok(Response::Loaded { backend, device }) => {
                Ok(Live { conn, model: entry.model.to_string(), cpu_only, backend, device })
            }
            Ok(Response::Error { message, backend }) => Err((message, backend && !cpu_only)),
            Ok(other) => Err((format!("исполнитель ответил не то: {other:?}"), false)),
            Err(broken) => Err((self.with_log_tail(broken.to_string()), !cpu_only)),
        }
    }

    /// The reason with the worker log's last line, when there is one.
    fn with_log_tail(&self, why: String) -> String {
        match log_tail(&self.log_path()) {
            Some(tail) => format!("{why}: {tail}"),
            None => why,
        }
    }

    /// Recognises one phrase. A worker that fails is replaced and the phrase asked once more; a cancel leaves the
    /// worker to finish the phrase (its answer is read and dropped), so the stream never loses its frame.
    pub async fn recognize(
        self: &Arc<Self>,
        pcm: &[i16],
        sample_rate: u32,
        cancel: &CancellationToken,
    ) -> Result<String> {
        self.wait_loading(cancel).await;
        let audio = to_f32_16k(pcm, sample_rate);
        let me = self.clone();
        let task = tokio::spawn(async move { me.recognize_now(audio).await });
        tokio::select! {
            result = task => result.map_err(|error| anyhow!("recognition task: {error}"))?,
            _ = cancel.cancelled() => bail!("recognition cancelled"),
        }
    }

    /// While a model is being chosen or loaded and none serves yet, a phrase waits a little for it instead of
    /// failing at once; a download or a failure does not make it wait.
    async fn wait_loading(&self, cancel: &CancellationToken) {
        let deadline = Instant::now() + LOADING_WAIT;
        while !self.ready() && Instant::now() < deadline && !cancel.is_cancelled() {
            let loading = matches!(self.lock().phase, None | Some(Phase::Idle) | Some(Phase::Loading));
            if !loading {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn recognize_now(&self, audio: Vec<f32>) -> Result<String> {
        if self.stop.is_cancelled() {
            bail!("распознавание остановлено вместе со шлюзом");
        }
        for attempt in 0..2 {
            let mut live = self.live.clone().lock_owned().await;
            let Some(worker) = live.as_mut() else {
                drop(live);
                self.wake.notify_one();
                bail!(self.not_ready());
            };
            let Some(entry) = catalog::find(&worker.model) else { bail!("unknown model {}", worker.model) };
            let request = Request::Run { language: self.language_for(entry) };
            let seconds = audio.len() as u32 / SAMPLE_RATE;
            let deadline = self.options.run_floor.max(RUN_PER_AUDIO_SECOND * seconds);
            let gpu = !worker.cpu_only;
            let why = match worker.conn.call(&request, &audio, deadline).await {
                Ok(Response::Text { text }) => return Ok(text),
                Ok(Response::Error { message, backend: false }) => bail!(message),
                Ok(Response::Error { message, backend: true }) => message,
                Ok(other) => format!("исполнитель ответил не то: {other:?}"),
                Err(broken) => self.with_log_tail(broken.to_string()),
            };
            // The worker is unusable: it ends here, another one comes.
            let model = worker.model.clone();
            live.take();
            *locked(&self.serving) = None;
            drop(live);
            self.note_failure(&why, gpu);
            if attempt == 1 || !self.may_restart() {
                self.wake.notify_one();
                bail!(why);
            }
            let Some(entry) = catalog::find(&model) else { bail!(why) };
            self.set_phase(Phase::Loading);
            match self.spawn_loaded(entry, &self.file_of(entry)).await {
                Ok(worker) => {
                    *locked(&self.serving) =
                        Some((worker.model.clone(), worker.backend.clone(), worker.device.clone()));
                    *self.live.lock().await = Some(worker);
                    self.set_phase(Phase::Ready);
                }
                Err(error) => {
                    self.fail(error.to_string());
                    self.wake.notify_one();
                    bail!(error);
                }
            }
        }
        bail!(self.not_ready())
    }

    fn not_ready(&self) -> String {
        let state = self.lock();
        match (state.phase, &state.error) {
            (Some(Phase::Downloading), _) => {
                format!("модель распознавания ещё скачивается ({} из {} МБ)", state.bytes / MB, state.total / MB)
            }
            (_, Some(error)) => error.clone(),
            (Some(Phase::Idle), _) | (None, _) => "модель распознавания не выбрана".into(),
            _ => "модель распознавания загружается".into(),
        }
    }
}

/// Whether the file on disk has the expected SHA-256 (read on a blocking thread: hundreds of megabytes).
async fn file_matches(path: &Path, sha256: &str) -> bool {
    let (path, sha256) = (path.to_path_buf(), sha256.to_string());
    tokio::task::spawn_blocking(move || {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        let Ok(mut file) = std::fs::File::open(&path) else { return false };
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => hasher.update(&buffer[..read]),
                Err(_) => return false,
            }
        }
        let hex: String = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
        hex.eq_ignore_ascii_case(&sha256)
    })
    .await
    .unwrap_or(false)
}

fn log_path(dir: &Path) -> PathBuf {
    dir.join("logs").join("asr-worker.log")
}

fn rotate(log: &Path) {
    if std::fs::metadata(log).is_ok_and(|meta| meta.len() > LOG_ROTATE_BYTES) {
        let _ = std::fs::rename(log, log.with_extension("log.1"));
    }
}

/// The log's last meaningful line, cut short.
fn log_tail(log: &Path) -> Option<String> {
    let text = std::fs::read_to_string(log).ok()?;
    let line = text.lines().map(str::trim).rfind(|line| !line.is_empty())?;
    Some(line.chars().take(200).collect())
}

/// 16-bit PCM at any rate as 16 kHz f32 in [-1, 1] (linear resampling; the stations already send 16 kHz).
pub fn to_f32_16k(pcm: &[i16], sample_rate: u32) -> Vec<f32> {
    let samples: Vec<f32> = pcm.iter().map(|&sample| f32::from(sample) / 32768.0).collect();
    if sample_rate == SAMPLE_RATE || sample_rate == 0 || samples.is_empty() {
        return samples;
    }
    let ratio = f64::from(sample_rate) / f64::from(SAMPLE_RATE);
    let count = ((samples.len() as f64) / ratio).floor() as usize;
    (0..count)
        .map(|index| {
            let at = index as f64 * ratio;
            let left = at.floor() as usize;
            let right = (left + 1).min(samples.len() - 1);
            let fraction = (at - left as f64) as f32;
            samples[left] * (1.0 - fraction) + samples[right] * fraction
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::speech::asr::local::protocol::{read_message, write_message};

    /// What a worker in memory does with each run, in order; past the list it hears «привет».
    #[derive(Clone, Copy, Debug)]
    enum Act {
        Hear,
        Crash,
        GpuFault,
        Hang,
    }

    struct MemoryLaunch {
        acts: Mutex<VecDeque<Act>>,
        launches: AtomicUsize,
        cpu_only_launches: AtomicUsize,
        /// The load fails on a GPU worker with a backend error.
        gpu_load_fails: bool,
        /// Every load fails as a bad file does.
        load_fails: bool,
    }

    impl MemoryLaunch {
        fn new(acts: &[Act]) -> Arc<MemoryLaunch> {
            Arc::new(MemoryLaunch {
                acts: Mutex::new(acts.iter().copied().collect()),
                launches: AtomicUsize::new(0),
                cpu_only_launches: AtomicUsize::new(0),
                gpu_load_fails: false,
                load_fails: false,
            })
        }
    }

    impl Launch for MemoryLaunch {
        fn launch(&self, cpu_only: bool) -> std::io::Result<Conn> {
            self.launches.fetch_add(1, Ordering::SeqCst);
            if cpu_only {
                self.cpu_only_launches.fetch_add(1, Ordering::SeqCst);
            }
            let (gateway_side, worker_side) = tokio::io::duplex(1 << 20);
            let (gateway_read, gateway_write) = tokio::io::split(gateway_side);
            let act = self.acts.lock().unwrap().pop_front();
            let gpu_load_fails = self.gpu_load_fails && !cpu_only;
            let load_fails = self.load_fails;
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                runtime.block_on(async move {
                    use tokio::io::AsyncReadExt;
                    let (mut reader, mut writer) = tokio::io::split(worker_side);
                    loop {
                        let mut buffer = Vec::new();
                        // One frame: lengths are read as the real worker would.
                        let mut head = [0u8; 4];
                        if reader.read_exact(&mut head).await.is_err() {
                            return;
                        }
                        let json_len = u32::from_le_bytes(head) as usize;
                        let mut json = vec![0u8; json_len];
                        reader.read_exact(&mut json).await.unwrap();
                        reader.read_exact(&mut head).await.unwrap();
                        let mut audio = vec![0u8; u32::from_le_bytes(head) as usize];
                        reader.read_exact(&mut audio).await.unwrap();
                        buffer.extend_from_slice(&(json_len as u32).to_le_bytes());
                        buffer.extend_from_slice(&json);
                        buffer.extend_from_slice(&(audio.len() as u32).to_le_bytes());
                        buffer.extend_from_slice(&audio);
                        let (request, _) = read_message::<Request>(&mut buffer.as_slice()).unwrap().unwrap();
                        let response = match request {
                            Request::Load { .. } if gpu_load_fails => {
                                Response::Error { message: "no GPU".into(), backend: true }
                            }
                            Request::Load { path } if load_fails => Response::Error {
                                message: format!("модель не загрузилась: load {}: gguf load error", path.display()),
                                backend: false,
                            },
                            Request::Load { .. } => Response::Loaded {
                                backend: if cpu_only { "cpu".into() } else { "metal".into() },
                                device: String::new(),
                            },
                            Request::Run { .. } => match act {
                                Some(Act::Crash) => return,
                                Some(Act::Hang) => std::future::pending().await,
                                Some(Act::GpuFault) => Response::Error { message: "device lost".into(), backend: true },
                                Some(Act::Hear) | None => Response::Text { text: "привет".into() },
                            },
                        };
                        let mut out = Vec::new();
                        write_message(&mut out, &response, &[]).unwrap();
                        use tokio::io::AsyncWriteExt;
                        if writer.write_all(&out).await.is_err() {
                            return;
                        }
                    }
                });
            });
            Ok(Conn { reader: Box::new(gateway_read), writer: Box::new(gateway_write), child: None })
        }
    }

    /// A service with the default model's file in place (a few bytes of the right size are not needed: `present`
    /// only checks the size, so a sparse file of that size stands in).
    fn service(launch: Arc<MemoryLaunch>, gpu: bool) -> (Arc<LocalAsr>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let entry = catalog::find(catalog::DEFAULT_MODEL).unwrap();
        let models = dir.path().join("models");
        std::fs::create_dir_all(&models).unwrap();
        let file = std::fs::File::create(models.join(entry.file)).unwrap();
        file.set_len(entry.size).unwrap();
        let mut options = LocalAsrOptions::new(dir.path().to_path_buf(), None, None, gpu);
        options.run_floor = Duration::from_millis(300);
        options.pauses = [Duration::ZERO, Duration::ZERO];
        (LocalAsr::with_launch(options, launch), dir)
    }

    async fn ready(asr: &Arc<LocalAsr>) {
        asr.choose(catalog::DEFAULT_MODEL);
        asr.settle().await;
        assert_eq!(asr.status().phase, Phase::Ready, "{:?}", asr.status());
    }

    #[tokio::test]
    async fn a_chosen_model_on_disk_loads_in_a_worker_and_a_phrase_is_heard() {
        let launch = MemoryLaunch::new(&[Act::Hear]);
        let (asr, _dir) = service(launch.clone(), true);
        assert_eq!(asr.status().phase, Phase::Idle);
        ready(&asr).await;
        let status = asr.status();
        assert_eq!(status.serving.as_deref(), Some(catalog::DEFAULT_MODEL));
        assert_eq!(status.backend.as_deref(), Some("metal"));
        assert_eq!(status.installed, vec![catalog::DEFAULT_MODEL.to_string()]);
        let text = asr.recognize(&[0i16; 16_000], 16_000, &CancellationToken::new()).await.unwrap();
        assert_eq!(text, "привет");
        assert_eq!(launch.launches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_worker_that_crashes_is_replaced_and_the_phrase_asked_again() {
        let launch = MemoryLaunch::new(&[Act::Crash]);
        let (asr, _dir) = service(launch.clone(), true);
        ready(&asr).await;
        let text = asr.recognize(&[0i16; 16_000], 16_000, &CancellationToken::new()).await.unwrap();
        assert_eq!(text, "привет");
        assert_eq!(launch.launches.load(Ordering::SeqCst), 2);
        // A crash on a GPU worker is the GPU's: the new one is on the processor.
        assert_eq!(launch.cpu_only_launches.load(Ordering::SeqCst), 1);
        assert!(asr.status().gpu_failed);
        assert_eq!(asr.status().backend.as_deref(), Some("cpu"));
    }

    #[tokio::test]
    async fn a_gpu_fault_moves_recognition_to_the_processor_until_retry() {
        let launch = MemoryLaunch::new(&[Act::GpuFault]);
        let (asr, _dir) = service(launch.clone(), true);
        ready(&asr).await;
        assert_eq!(asr.recognize(&[0i16; 8_000], 16_000, &CancellationToken::new()).await.unwrap(), "привет");
        assert!(asr.status().gpu_failed);
        asr.restart().await;
        assert!(!asr.status().gpu_failed);
        asr.settle().await;
        assert_eq!(asr.status().backend.as_deref(), Some("metal"));
    }

    #[tokio::test]
    async fn a_hung_worker_is_ended_after_its_deadline_and_another_answers() {
        let launch = MemoryLaunch::new(&[Act::Hang]);
        let (asr, _dir) = service(launch.clone(), false);
        ready(&asr).await;
        let text = asr.recognize(&[0i16; 1_600], 16_000, &CancellationToken::new()).await.unwrap();
        assert_eq!(text, "привет");
        assert_eq!(launch.launches.load(Ordering::SeqCst), 2);
        // GPU not allowed: every worker was on the processor, so it is not the GPU's fault.
        assert!(!asr.status().gpu_failed);
    }

    #[tokio::test]
    async fn three_failures_in_five_minutes_pause_the_restarts_until_retry() {
        let launch = MemoryLaunch::new(&[Act::Crash, Act::Crash, Act::Crash]);
        let (asr, _dir) = service(launch.clone(), false);
        ready(&asr).await;
        // The phrase fails twice: the worker and its replacement both crash.
        assert!(asr.recognize(&[0i16; 1_600], 16_000, &CancellationToken::new()).await.is_err());
        // The background brings a third worker; it crashes too, and the restarts stop.
        asr.settle().await;
        assert!(asr.recognize(&[0i16; 1_600], 16_000, &CancellationToken::new()).await.is_err());
        let status = asr.status();
        assert!(status.paused, "{status:?}");
        assert_eq!(status.phase, Phase::Failed);
        assert!(status.error.unwrap().contains("падало"));
        asr.retry();
        asr.settle().await;
        assert_eq!(asr.status().phase, Phase::Ready);
        assert_eq!(asr.recognize(&[0i16; 1_600], 16_000, &CancellationToken::new()).await.unwrap(), "привет");
    }

    #[tokio::test]
    async fn a_load_that_fails_on_the_gpu_is_done_again_on_the_processor() {
        let launch = Arc::new(MemoryLaunch {
            acts: Mutex::new(VecDeque::new()),
            launches: AtomicUsize::new(0),
            cpu_only_launches: AtomicUsize::new(0),
            gpu_load_fails: true,
            load_fails: false,
        });
        let (asr, _dir) = service(launch.clone(), true);
        ready(&asr).await;
        assert_eq!(asr.status().backend.as_deref(), Some("cpu"));
        assert!(asr.status().gpu_failed);
    }

    #[tokio::test]
    async fn a_file_that_will_not_load_and_is_not_whole_is_deleted_to_be_fetched_again() {
        let launch = Arc::new(MemoryLaunch {
            acts: Mutex::new(VecDeque::new()),
            launches: AtomicUsize::new(0),
            cpu_only_launches: AtomicUsize::new(0),
            gpu_load_fails: false,
            load_fails: true,
        });
        let (asr, dir) = service(launch, false);
        asr.choose(catalog::DEFAULT_MODEL);
        asr.settle().await;
        let entry = catalog::find(catalog::DEFAULT_MODEL).unwrap();
        // The stand-in file is zeros: its hash is not the model's, so it goes, and no failure is left standing.
        assert!(!dir.path().join("models").join(entry.file).exists());
        let status = asr.status();
        assert_eq!(status.error, None);
        assert!(!status.paused);
    }

    #[tokio::test]
    async fn a_model_without_gguf_says_so_and_nothing_starts() {
        let launch = MemoryLaunch::new(&[]);
        let (asr, _dir) = service(launch.clone(), true);
        asr.choose("v2_rnnt");
        asr.settle().await;
        let status = asr.status();
        assert_eq!(status.phase, Phase::Failed);
        assert!(status.error.unwrap().contains("v2_rnnt"));
        assert_eq!(launch.launches.load(Ordering::SeqCst), 0);
        let error = asr.recognize(&[0i16; 1_600], 16_000, &CancellationToken::new()).await.unwrap_err();
        assert!(error.to_string().contains("v2_rnnt"), "{error}");
    }

    #[tokio::test]
    async fn a_stopped_engine_ends_its_worker_and_starts_no_other() {
        let launch = MemoryLaunch::new(&[]);
        let (asr, _dir) = service(launch.clone(), true);
        ready(&asr).await;
        asr.shutdown().await;
        assert!(asr.live.lock().await.is_none());
        assert!(!asr.ready());
        assert!(asr.recognize(&[0i16; 1_600], 16_000, &CancellationToken::new()).await.is_err());
        asr.settle().await;
        assert_eq!(launch.launches.load(Ordering::SeqCst), 1, "no worker after the stop");
    }

    #[tokio::test]
    async fn a_phrase_in_the_first_seconds_waits_for_the_model_instead_of_failing() {
        let launch = MemoryLaunch::new(&[]);
        let (asr, _dir) = service(launch, true);
        let early = {
            let asr = asr.clone();
            tokio::spawn(async move { asr.recognize(&[0i16; 1_600], 16_000, &CancellationToken::new()).await })
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        ready(&asr).await;
        assert_eq!(early.await.unwrap().unwrap(), "привет");
    }

    #[test]
    fn audio_at_another_rate_comes_to_16_khz_and_16_khz_stays_as_it_is() {
        assert_eq!(to_f32_16k(&[16384, -16384], 16_000), vec![0.5, -0.5]);
        assert_eq!(to_f32_16k(&[0; 48_000], 48_000).len(), 16_000);
        assert_eq!(to_f32_16k(&[0; 8_000], 8_000).len(), 16_000);
    }
}
