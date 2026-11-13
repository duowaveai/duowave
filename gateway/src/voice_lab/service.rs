//! The owner's own voices: engines installed by a button, takes recorded in the panel, imprints taken from them
//! (zero-shot: no training), and the servers that speak in them — all under the memory rules of memory_policy.rs, so
//! the household's recognition and voice keep their memory: a warning before a start that would press them, a
//! watchdog over an imprint, a ceiling against a runaway; there is no limit for the owner to set. Choosing a voice
//! goes through the models (activating the TTS card `own:<id>`), which asks [`VoiceLabService::prepare`] for the route.
//!
//! The `.ts` files named here are the Node gateway's, removed from the tree: git tag `node-gateway-last`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use futures::future::FutureExt;
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::clone_engines::{
    flavour, spec, verdict, Device, EngineId, EngineSpec, Flavour, Verdict, ENGINE_IDS, PACKAGES_MB,
};
use super::clone_servers::{
    CloneServers, CloneServersOptions, ProcessSpawner, ServerSpawner, ServerState, StartOptions,
};
use super::engine_installer::{install_engine, python_of, InstallPlan};
use super::jobs::{
    with_limits, JobContext, JobDeps, JobKind, JobOutcome, JobRunState, JobRunner, JobState, RunOptions,
};
use super::js;
use super::memory_policy::{admit, ceiling_mb, need_mb, reserve_mb, Admission, AdmissionVerdict};
use super::passages::{PASSAGES, SAMPLE_PHRASES};
use super::platform::{
    describe_platform, detect_platform, os_name, GpuKind, OsName, PlatformInfo, Probe, ProbeCell, SystemProbe,
};
use super::system_memory::{Pressure, SystemMemory};
use super::voice_store::{OwnVoice, TakeInfo, VoiceStore, SHORT_TAKE_SECONDS};
use crate::config::{AsrConfig, Config, VoiceLabConfig};
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::platform::http::ApiError;
use crate::speech::tts::own_speech::SpeechRoute;
use crate::utils::time::now_ms;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineStatus {
    Ready,
    Installing,
    Missing,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EngineNeed {
    pub imprint: u64,
    pub serve: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineCard {
    pub id: EngineId,
    pub title: &'static str,
    pub about: &'static str,
    pub license: &'static str,
    pub recommended: bool,
    pub status: EngineStatus,
    /// Where its voice speaks here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<Device>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slow: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// About how much the install takes, GB.
    pub disk_gb: u64,
    /// What an imprint and the voice's server need here, MB: the measured peaks with a margin, or the defaults.
    pub need_mb: EngineNeed,
    /// ready.json as the install wrote it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformSummary {
    pub summary: String,
    pub os: OsName,
    pub gpu: GpuKind,
    pub uv: bool,
    pub checked_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Draft {
    pub takes: Vec<TakeInfo>,
    pub seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ListedVoice {
    #[serde(flatten)]
    pub voice: OwnVoice,
    pub chosen: bool,
}

/// For the panel's own judgement before a start: tight when the pool less the reserve is below a need.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryView {
    pub total_mb: u64,
    pub pool_mb: u64,
    pub reserve_mb: u64,
    pub pressure: Pressure,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoiceLabView {
    /// Whether own voices can be made here; the reason when not.
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<PlatformSummary>,
    pub engines: Vec<EngineCard>,
    pub draft: Draft,
    pub voices: Vec<ListedVoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<JobState>,
    pub memory: MemoryView,
    pub servers: BTreeMap<EngineId, ServerState>,
    pub passages: &'static [&'static str],
}

/// An own voice as the models' cards need it, with its engine's server state.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OwnVoiceCard {
    #[serde(flatten)]
    pub voice: OwnVoice,
    /// Its engine's server answers.
    pub serving: bool,
    /// Its engine is installed.
    pub ready: bool,
}

/// A chosen own voice, ready to speak: the route to its engine's server (speech's SpeechRoute, see `route`).
#[derive(Debug, Clone)]
pub struct PreparedVoice {
    pub engine: EngineId,
    pub route: Arc<SpeechRoute>,
    pub name: String,
}

/// A stored choice at start: its route at once, its server coming up in the background.
#[derive(Debug, Clone)]
pub struct RestoredVoice {
    pub engine: EngineId,
    pub route: Arc<SpeechRoute>,
}

/// What the voice lab asks the speech module (src/speech/voice.service.ts): the pair our own engine is asked for now,
/// `(model, voice)` — for an own voice `(engine id, voice id)`. The gateway links its voice service at the build
/// (src/links); without one the env's pair (TTS_OWN_MODEL, TTS_OWN_VOICE) is the choice, as before any panel choice.
pub trait OwnSpeech: Send + Sync {
    fn own_choice(&self) -> Option<(String, String)>;
}

const IMPRINT_LABELS: &[(&str, &str)] = &[
    ("prepare", "Готовлю записи"),
    ("select", "Выбираю лучший фрагмент"),
    ("words", "Подписываю образец"),
    ("load", "Загружаю модель"),
    ("imprint", "Снимаю отпечаток"),
    ("samples", "Пробные фразы"),
    ("done", "Готово"),
];

/// Where an imprint's reference comes from.
pub enum ImprintSource {
    /// The owner's takes: the best 5-8 seconds become the reference, its words from our own recognition.
    Takes(Vec<PathBuf>),
    /// A reference whose words are known (a character's, a voice's own): taken whole, nothing recognised.
    Reference { file: PathBuf, words: String },
}

/// What an imprint does once its voice is there.
#[derive(Default)]
struct Finish {
    /// The draft's takes move into the voice.
    adopt: Vec<TakeInfo>,
    /// Another voice's recordings are copied into it.
    copy_takes: Option<String>,
    /// Merged into its meta.json.
    meta: Map<String, Value>,
}

/// Restarting a server that a synthesis found gone: not more often than this.
const RESTART_PAUSE: Duration = Duration::from_secs(30);
const LOCAL_HOSTS: &[&str] = &["127.0.0.1", "localhost", "::1", "[::1]"];

/// Short of room beside the household's reserve: the owner may start anyway (the panel asks «всё равно?» and sends
/// `anyway`). 428: the request needs the owner's consent first.
fn tight_memory(reason: String) -> ApiError {
    ApiError::new(StatusCode::PRECONDITION_REQUIRED, reason)
}

/// The gateway's environment minus its keys, as the model servers get it (models' server_env), and what Python needs
/// on top to speak UTF-8 lines at once.
fn child_env() -> HashMap<String, String> {
    let mut env: HashMap<String, String> = crate::models::server_env().into_iter().collect();
    env.insert("PYTHONUNBUFFERED".into(), "1".into());
    env.insert("PYTHONIOENCODING".into(), "utf-8".into());
    env.insert("HF_HUB_DISABLE_TELEMETRY".into(), "1".into());
    // Python's compiled files go to the data folder, not beside the code: in the app the code is the app's bundle
    // (Resources/clone-server), and a __pycache__ written there breaks the bundle's signature (seen on 0.5.1, 12.11).
    // A cache of its own keeps them all — the environments' packages too — so nothing is compiled twice.
    if let Some(data) = std::env::var_os("DATA_DIR").filter(|data| !data.is_empty()) {
        let cache = std::path::PathBuf::from(data).join("cache").join("python");
        env.insert("PYTHONPYCACHEPREFIX".into(), cache.to_string_lossy().into_owned());
    }
    env
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The Python side of the engines also counts on the CPU (VoxCPM loads the reference and builds its prompt cache
/// there), so half the cores, up to eight: four turned out to be half the speed of an M4 Max's eight.
fn worker_threads(info: &PlatformInfo) -> u32 {
    (info.cpus / 2).clamp(2, 8) as u32
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn local_url(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .is_some_and(|host| LOCAL_HOSTS.contains(&host.as_str()))
}

pub struct VoiceLabService {
    config: VoiceLabConfig,
    asr: AsrConfig,
    /// TTS_OWN_URL: our own voice server, without which no own voice is heard.
    own_speech: bool,
    env_own_choice: Option<(String, String)>,
    runtime_dir: PathBuf,
    server_dir: PathBuf,
    events: EventBus,
    probe: ProbeCell,
    info: Arc<RwLock<Option<PlatformInfo>>>,
    detecting: tokio::sync::Mutex<()>,
    pub store: VoiceStore,
    memory: Arc<SystemMemory>,
    jobs: JobRunner,
    servers: Arc<CloneServers>,
    routes: Mutex<HashMap<EngineId, Arc<SpeechRoute>>>,
    restarted_at: Mutex<HashMap<EngineId, Instant>>,
    speech: OnceLock<Arc<dyn OwnSpeech>>,
}

impl VoiceLabService {
    /// Built with the gateway; nothing is touched on disk or started until [`VoiceLabService::start`].
    pub fn new(config: &Config, events: EventBus) -> Arc<VoiceLabService> {
        VoiceLabService::with_spawner(config, events, Arc::new(ProcessSpawner))
    }

    /// The same, with its own way of starting the voice servers: tests.
    pub fn with_spawner(config: &Config, events: EventBus, spawner: Arc<dyn ServerSpawner>) -> Arc<VoiceLabService> {
        let lab = config.voice_lab.clone();
        let probe = ProbeCell::new(Arc::new(SystemProbe));
        let info: Arc<RwLock<Option<PlatformInfo>>> = Arc::new(RwLock::new(None));
        let read = |info: &Arc<RwLock<Option<PlatformInfo>>>| {
            info.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
        };
        let memory = {
            let (os_info, os_probe, nvidia_info) = (info.clone(), probe.clone(), info.clone());
            Arc::new(SystemMemory::new(
                probe.clone(),
                Box::new(move || {
                    read(&os_info).map(|info| info.os).unwrap_or_else(|| os_name(&os_probe.get().platform()))
                }),
                Box::new(move || read(&nvidia_info).is_some_and(|info| info.gpu.kind == GpuKind::Nvidia)),
                Box::new(now_ms),
            ))
        };
        let jobs = {
            let memory = memory.clone();
            JobRunner::new(JobDeps::new(Arc::new(move || {
                let memory = memory.clone();
                async move { memory.sample().await }.boxed()
            })))
        };
        let store = VoiceStore::new(&lab.dir);
        let runtime_dir = js::resolve(&lab.runtime_dir);
        let server_dir = js::resolve(&lab.server_dir);
        let logs_dir =
            js::resolve(&config.data_dir.as_ref().map(|dir| dir.join("logs")).unwrap_or_else(|| "logs".into()));
        let servers = Arc::new(CloneServers::new(CloneServersOptions {
            urls: BTreeMap::from([
                (EngineId::Omnivoice, lab.servers.omnivoice.clone()),
                (EngineId::Cosyvoice3, lab.servers.cosyvoice3.clone()),
                (EngineId::Voxcpm, lab.servers.voxcpm.clone()),
            ]),
            runtime_dir: runtime_dir.clone(),
            server_dir: server_dir.clone(),
            voices_dir: store.voices_dir.clone(),
            run_dir: store.run_dir.clone(),
            logs_dir,
            platform: {
                let info = info.clone();
                Arc::new(move || read(&info))
            },
            env: Arc::new(child_env),
            spawner,
            start_timeout_ms: 120_000,
        }));
        let own = config.gateway.tts.own.as_ref();
        Arc::new(VoiceLabService {
            config: lab,
            asr: config.speech.asr.clone(),
            own_speech: own.is_some(),
            env_own_choice: own.map(|own| (own.model.clone(), own.voice.clone())),
            runtime_dir,
            server_dir,
            events,
            probe,
            info,
            detecting: tokio::sync::Mutex::new(()),
            store,
            memory,
            jobs,
            servers,
            routes: Mutex::new(HashMap::new()),
            restarted_at: Mutex::new(HashMap::new()),
            speech: OnceLock::new(),
        })
    }

    /// At start: the folders, half-taken imprints swept, this computer checked in the background.
    pub fn start(self: &Arc<Self>) {
        match self.store.prepare() {
            Ok(removed) if !removed.is_empty() => {
                tracing::info!("voice lab: half-taken imprints removed: {}", removed.join(", "));
            }
            Ok(_) => {}
            Err(error) => tracing::warn!("voice lab: {}: {error}", self.store.root.display()),
        }
        let lab = self.clone();
        tokio::spawn(async move {
            let info = lab.platform(false).await;
            tracing::info!("own voices: {}", describe_platform(&info));
        });
    }

    /// The computer as detection sees it, from now on: a test or a tool gives its own.
    pub fn use_probe(&self, probe: Arc<dyn Probe>) {
        self.probe.set(probe);
        *self.info.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    /// The speech module: which own voice is chosen. Linked once, at the build.
    pub fn link_speech(&self, speech: Arc<dyn OwnSpeech>) {
        if self.speech.set(speech).is_err() {
            tracing::warn!("voice lab: the speech module is linked already");
        }
    }

    fn known_platform(&self) -> Option<PlatformInfo> {
        self.info.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// This computer, checked once (again on the panel's «Проверить снова»).
    pub async fn platform(&self, recheck: bool) -> PlatformInfo {
        if !recheck {
            if let Some(info) = self.known_platform() {
                return info;
            }
        }
        let _turn = self.detecting.lock().await;
        if !recheck {
            if let Some(info) = self.known_platform() {
                return info;
            }
        }
        let probe = self.probe.get();
        let info = detect_platform(&*probe, &self.runtime_dir).await;
        *self.info.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(info.clone());
        info
    }

    pub async fn view(&self) -> VoiceLabView {
        let info = Some(self.platform(false).await);
        let snapshot = self.memory.snapshot().await;
        let reserve = reserve_mb(snapshot.total_mb, self.config.reserve_mb);
        let takes = self.store.takes();
        let chosen = self.chosen_voice();
        let mut servers = BTreeMap::new();
        for engine in ENGINE_IDS {
            if !self.installed(engine) {
                continue;
            }
            let state = self.servers.state(engine).await;
            self.learn_serving(engine, state.device.as_deref(), state.peak_mb);
            servers.insert(engine, state);
        }
        let (available, reason) = self.availability(info.as_ref());
        let seconds = js::round_i64(takes.iter().map(|take| take.seconds).sum::<f64>());
        VoiceLabView {
            available,
            reason,
            platform: info.as_ref().map(|info| PlatformSummary {
                summary: describe_platform(info),
                os: info.os,
                gpu: info.gpu.kind,
                uv: info.tools.uv.is_some(),
                checked_at: info.checked_at.clone(),
            }),
            engines: ENGINE_IDS.iter().map(|engine| self.engine_card(*engine, info.as_ref())).collect(),
            draft: Draft { takes, seconds },
            voices: self
                .store
                .voices()
                .into_iter()
                .map(|voice| {
                    let chosen = chosen.as_deref() == Some(voice.id.as_str());
                    ListedVoice { voice, chosen }
                })
                .collect(),
            job: self.jobs.state(),
            memory: MemoryView {
                total_mb: snapshot.total_mb,
                pool_mb: snapshot.pool_mb,
                reserve_mb: reserve,
                pressure: snapshot.pressure,
            },
            servers,
            passages: &PASSAGES,
        }
    }

    pub async fn recheck(&self) -> VoiceLabView {
        self.platform(true).await;
        self.view().await
    }

    /// Installs an engine: copied from the bench when this computer has one, downloaded otherwise.
    pub async fn install(self: &Arc<Self>, engine: &str, actor: Option<Actor>) -> Result<JobState, ApiError> {
        let spec = self.spec(engine)?;
        let info = self.platform(false).await;
        let fits = verdict(spec, &info);
        if !fits.supported {
            return Err(ApiError::bad_request(fits.reason.unwrap_or_default()));
        }
        if self.installed(spec.id) {
            return Err(ApiError::conflict(format!("{} уже установлена.", spec.title)));
        }
        let bench =
            self.config.bench_dir.as_ref().map(|dir| js::resolve(&dir.join("models"))).filter(|dir| dir.exists());
        let (runtime_dir, server_dir) = (self.runtime_dir.clone(), self.server_dir.clone());
        let events = self.events.clone();
        let installer = actor.clone();
        self.start_job(
            JobKind::Install,
            spec.id,
            move |context: JobContext| async move {
                // The environment is read in the job, as the imprint's: whatever fails there is the job's error.
                let plan = InstallPlan {
                    engine: spec,
                    platform: info,
                    runtime_dir,
                    server_dir,
                    bench_models: bench,
                    env: child_env(),
                };
                let ready = install_engine(&context, &plan).await?;
                events.emit(GatewayEvent::VoiceEngineInstalled {
                    engine: spec.id.as_str().into(),
                    device: ready.device.clone(),
                    actor: installer,
                });
                tracing::info!(
                    "engine {} installed ({}), computes on {}",
                    spec.id,
                    ready.source.as_str(),
                    ready.device
                );
                Ok(Some(object(json!({ "device": ready.device, "source": ready.source }))))
            },
            actor,
        )
    }

    /// Decoded, measured and written off the runtime's threads: a 16 MB upload is tens of milliseconds of work.
    pub async fn add_take(self: &Arc<Self>, body: Vec<u8>, passage: Option<i64>) -> Result<TakeInfo, ApiError> {
        let lab = self.clone();
        tokio::task::spawn_blocking(move || lab.store.add_take(&body, passage))
            .await
            .map_err(ApiError::internal)?
            .map_err(ApiError::bad_request)
    }

    fn imprint_running(&self) -> bool {
        self.jobs.state().is_some_and(|job| job.state == JobRunState::Running && job.kind == JobKind::Imprint)
    }

    pub fn remove_take(&self, n: Option<i64>) -> Result<(), ApiError> {
        if self.imprint_running() {
            return Err(ApiError::conflict("Идёт отпечаток — дубли пока нельзя трогать."));
        }
        let missing = || ApiError::not_found("нет такого дубля");
        let n = n.and_then(|n| u32::try_from(n).ok()).ok_or_else(missing)?;
        // A take that could not be removed is the 500 Node answered: never «deleted» with the voice left on disk.
        if !self.store.remove_take(n).map_err(ApiError::internal)? {
            return Err(missing());
        }
        Ok(())
    }

    pub fn clear_draft(&self) -> Result<(), ApiError> {
        if self.imprint_running() {
            return Err(ApiError::conflict("Идёт отпечаток — дубли пока нельзя трогать."));
        }
        Ok(self.store.clear_draft()?)
    }

    pub fn take_path(&self, n: Option<i64>) -> Result<PathBuf, ApiError> {
        n.filter(|n| *n >= 1)
            .and_then(|n| u32::try_from(n).ok())
            .map(|n| self.store.take_file(n))
            .filter(|path| path.exists())
            .ok_or_else(|| ApiError::not_found("нет такого дубля"))
    }

    /// «Снять отпечаток»: the best 5-8 seconds of the takes become a voice, under the memory guard. Tight on memory,
    /// it starts only with `anyway` (the owner's «всё равно»); the watchdog stops it if the computer starts paging.
    pub async fn imprint(
        self: &Arc<Self>,
        engine: &str,
        name: Option<&Value>,
        actor: Option<Actor>,
        anyway: bool,
    ) -> Result<JobState, ApiError> {
        let spec = self.spec(engine)?;
        let info = self.ready_to_imprint(spec).await?;
        let takes = self.store.takes();
        let seconds: f64 = takes.iter().map(|take| take.seconds).sum();
        if takes.is_empty() || seconds < SHORT_TAKE_SECONDS {
            return Err(ApiError::bad_request(format!(
                "Нужна хотя бы одна запись от {SHORT_TAKE_SECONDS} секунд — лучше три по 40."
            )));
        }
        let label = match name.and_then(Value::as_str).map(js::trim).filter(|name| !name.is_empty()) {
            Some(name) => js::slice(name, 60).to_string(),
            None => "Мой голос".to_string(),
        };
        let files = takes.iter().map(|take| self.store.take_file(take.n)).collect();
        let finish = Finish { adopt: takes, ..Finish::default() };
        let id = self.store.new_voice_id();
        self.start_imprint(spec, info, ImprintSource::Takes(files), id, label, finish, anyway, actor).await
    }

    /// A voice of a package's character, imprinted from its reference and its known words — its package has no
    /// snapshot for this engine, or one of other weights. The voice gets the id it is asked for
    /// (`c-<package>-<character>-<engine>`) and `meta` (its origin) on top of what the imprint writes.
    #[allow(clippy::too_many_arguments)]
    pub async fn imprint_reference(
        self: &Arc<Self>,
        voice_id: &str,
        engine: &str,
        reference: &Path,
        words: &str,
        name: &str,
        meta: Map<String, Value>,
        anyway: bool,
        actor: Option<Actor>,
    ) -> Result<JobState, ApiError> {
        let spec = self.spec(engine)?;
        let info = self.ready_to_imprint(spec).await?;
        self.store.voice_dir(voice_id).map_err(ApiError::bad_request)?;
        if self.store.voice(voice_id).is_some() {
            return Err(ApiError::conflict(format!("Голос {voice_id} уже есть.")));
        }
        if !reference.is_file() || words.trim().is_empty() {
            return Err(ApiError::bad_request("Нет образца голоса или его слов — снять отпечаток не из чего."));
        }
        let source = ImprintSource::Reference { file: reference.to_path_buf(), words: words.trim().to_string() };
        let finish = Finish { meta, ..Finish::default() };
        self.start_imprint(
            spec,
            info,
            source,
            voice_id.to_string(),
            js::slice(name, 60).to_string(),
            finish,
            anyway,
            actor,
        )
        .await
    }

    /// «Переснять для другой модели»: an own voice imprinted anew under another engine, from its reference and its
    /// words when it keeps them, else from its takes. A new voice beside the old one, with the same name and the
    /// recordings copied: the old one stays for the computer that has its engine.
    pub async fn reimprint(
        self: &Arc<Self>,
        id: &str,
        engine: &str,
        anyway: bool,
        actor: Option<Actor>,
    ) -> Result<JobState, ApiError> {
        let Some(voice) = self.store.voice(id) else {
            return Err(ApiError::not_found("нет такого голоса"));
        };
        let spec = self.spec(engine)?;
        if spec.id == voice.engine {
            return Err(ApiError::bad_request(format!("Голос уже снят для {}.", spec.title)));
        }
        let info = self.ready_to_imprint(spec).await?;
        let dir = self.store.voice_dir(id).map_err(ApiError::bad_request)?;
        let source = match (&voice.words, voice.has_ref) {
            (Some(words), true) => ImprintSource::Reference { file: dir.join("ref.wav"), words: words.clone() },
            _ => {
                let takes = self.store.takes_of(id);
                if takes.is_empty() {
                    return Err(ApiError::bad_request(
                        "У голоса не сохранились ни образец, ни записи — переснять не из чего.",
                    ));
                }
                ImprintSource::Takes(takes)
            }
        };
        let mut meta = Map::new();
        meta.insert("reimprintedFrom".into(), Value::String(id.to_string()));
        for key in ["origin", "package", "character"] {
            let value = match key {
                "origin" => voice.origin.clone(),
                "package" => voice.package.clone(),
                _ => voice.character.clone(),
            };
            if let Some(value) = value {
                meta.insert(key.into(), Value::String(value));
            }
        }
        let finish = Finish { copy_takes: Some(id.to_string()), meta, ..Finish::default() };
        let new_id = self.store.new_voice_id();
        self.start_imprint(spec, info, source, new_id, voice.name.clone(), finish, anyway, actor).await
    }

    /// What every imprint needs first: own voices possible here, the engine runs here and is installed.
    async fn ready_to_imprint(&self, spec: &'static EngineSpec) -> Result<PlatformInfo, ApiError> {
        let info = self.platform(false).await;
        let (available, reason) = self.availability(Some(&info));
        if !available {
            return Err(ApiError::bad_request(reason.unwrap_or_default()));
        }
        let fits = verdict(spec, &info);
        if !fits.supported {
            return Err(ApiError::bad_request(fits.reason.unwrap_or_default()));
        }
        if !self.installed(spec.id) {
            return Err(ApiError::bad_request(format!("Сначала установите {}.", spec.title)));
        }
        Ok(info)
    }

    /// The imprint job: clone-server/imprint.py under the memory guard, into `voices/<id>`, then what `finish` says.
    #[allow(clippy::too_many_arguments)]
    async fn start_imprint(
        self: &Arc<Self>,
        spec: &'static EngineSpec,
        info: PlatformInfo,
        source: ImprintSource,
        id: String,
        label: String,
        finish: Finish,
        anyway: bool,
        actor: Option<Actor>,
    ) -> Result<JobState, ApiError> {
        if matches!(source, ImprintSource::Takes(_)) && (!self.asr.enabled || !local_url(&self.asr.base_url)) {
            return Err(ApiError::bad_request(
                "Для отпечатка нужно своё распознавание на этом компьютере: слова образца не отправляются в облако.",
            ));
        }
        let fits = verdict(spec, &info);
        let device = fits.imprint_device.unwrap_or(Device::Cpu);
        let key = format!("{}/imprint/{}", spec.id, device.as_str());
        let need = need_mb(self.store.learned(&key), spec.need_mb.imprint);
        let admission = admit(&self.memory.sample().await, self.reserve(), need, None);
        admitted(&admission, anyway)?;
        let runtime = self.runtime_dir.join(spec.id.as_str());
        let python = python_of(&runtime, info.os);
        let out = self.store.voices_dir.join(&id);
        let text = |path: &Path| path.to_string_lossy().into_owned();
        let mut args = vec![
            text(&self.server_dir.join("imprint.py")),
            "--engine".into(),
            spec.id.as_str().into(),
            "--runtime".into(),
            text(&runtime),
            "--takes".into(),
        ];
        match &source {
            ImprintSource::Takes(files) => {
                args.extend(files.iter().map(|file| text(file)));
                args.extend([
                    "--asr-url".into(),
                    self.asr.base_url.clone(),
                    "--asr-model".into(),
                    self.asr.model.clone(),
                ]);
            }
            ImprintSource::Reference { file, words } => {
                args.extend([text(file), "--ref-text".into(), words.clone()]);
            }
        }
        args.extend([
            "--out".into(),
            text(&out),
            "--name".into(),
            label,
            "--device".into(),
            device.as_str().into(),
            "--threads".into(),
            worker_threads(&info).to_string(),
            "--mem-limit-mb".into(),
            admission.ceiling_mb.to_string(),
            "--samples".into(),
            serde_json::to_string(&SAMPLE_PHRASES)?,
            "--die-with-stdin".into(),
        ]);
        let (command, args) = with_limits(&info, &text(&python), &args, Some(admission.ceiling_mb));
        let lab = self.clone();
        let creator = actor.clone();
        let ceiling = admission.ceiling_mb;
        self.start_job(
            JobKind::Imprint,
            spec.id,
            move |context: JobContext| async move {
                context.stage("prepare", "Готовлю записи", Some(1.0));
                let options = RunOptions {
                    cwd: Some(lab.server_dir.clone()),
                    env: Some(child_env()),
                    limit_mb: Some(ceiling),
                    anyway,
                    percent: Some((1.0, 99.0)),
                    labels: IMPRINT_LABELS,
                };
                let outcome: JobOutcome = async {
                    // imprint.py makes the voice's folders with the default modes: under a root that is the owner's.
                    lab.store.folders().map_err(|error| error.to_string())?;
                    let result = context.run(&command, &args, options).await?;
                    if !finish.adopt.is_empty() {
                        lab.store.adopt_takes(&id, &finish.adopt)?;
                    }
                    if let Some(from) = &finish.copy_takes {
                        lab.store.copy_takes(from, &id)?;
                    }
                    if !finish.meta.is_empty() {
                        lab.store.patch_meta(&id, finish.meta.clone())?;
                    }
                    let peak = result.get("peakMb").and_then(Value::as_f64).or(context.state().peak_mb);
                    if let Some(peak) = peak {
                        lab.store.learn(&key, peak);
                    }
                    lab.events.emit(GatewayEvent::VoiceOwnCreated {
                        id: id.clone(),
                        engine: spec.id.as_str().into(),
                        actor: creator,
                    });
                    let peak_text = peak.map(|peak| peak.to_string()).unwrap_or_else(|| "?".into());
                    tracing::info!(
                        "voice {id} imprinted with {} on {} (peak {peak_text} MB)",
                        spec.id,
                        device.as_str()
                    );
                    let mut done = object(json!({ "voiceId": id }));
                    if let Some(peak) = peak.filter(|peak| *peak != 0.0) {
                        done.insert("peakMb".into(), js::value(peak));
                    }
                    if let Some(rtf) = result.get("rtf").filter(|rtf| rtf.is_number()) {
                        done.insert("rtf".into(), rtf.clone());
                    }
                    Ok(Some(done))
                }
                .await;
                // A job killed from outside leaves its half-written folder: never a voice.
                let _ = std::fs::remove_dir_all(lab.store.voices_dir.join(format!(".partial-{id}")));
                outcome
            },
            actor,
        )
    }

    pub fn stop_job(&self) -> bool {
        self.jobs.stop()
    }

    pub fn rename(&self, id: &str, name: Option<&Value>, actor: Option<Actor>) -> Result<OwnVoice, ApiError> {
        if self.store.voice(id).is_none() {
            return Err(ApiError::not_found("нет такого голоса"));
        }
        let Some(name) = name.and_then(Value::as_str).map(js::trim).filter(|name| !name.is_empty()) else {
            return Err(ApiError::bad_request("Имя не может быть пустым."));
        };
        let renamed = self.store.rename(id, js::slice(name, 60)).map_err(ApiError::internal)?;
        self.events.emit(GatewayEvent::VoiceOwnRenamed { id: id.to_string(), actor });
        Ok(renamed)
    }

    pub fn remove(&self, id: &str, actor: Option<Actor>) -> Result<(), ApiError> {
        if self.store.voice(id).is_none() {
            return Err(ApiError::not_found("нет такого голоса"));
        }
        if self.chosen_voice().as_deref() == Some(id) {
            return Err(ApiError::conflict("Этот голос выбран — сначала выберите другой."));
        }
        self.store.remove(id).map_err(ApiError::internal)?;
        self.events.emit(GatewayEvent::VoiceOwnDeleted { id: id.to_string(), actor });
        tracing::info!("own voice {id} deleted with its takes");
        Ok(())
    }

    pub fn sample_path(&self, id: &str, n: Option<i64>) -> Result<PathBuf, ApiError> {
        let path = match (self.store.voice(id), n) {
            (Some(_), Some(n)) => self.store.sample_path(id, n),
            _ => None,
        };
        path.ok_or_else(|| ApiError::not_found("нет такой пробы"))
    }

    /// Every engine as the panel's card shows it here.
    pub async fn engine_cards(&self) -> Vec<EngineCard> {
        let info = self.platform(false).await;
        ENGINE_IDS.iter().map(|engine| self.engine_card(*engine, Some(&info))).collect()
    }

    /// The job running, else the last one.
    pub fn job(&self) -> Option<JobState> {
        self.jobs.state()
    }

    /// Whether an engine is installed.
    pub fn engine_installed(&self, engine: EngineId) -> bool {
        self.installed(engine)
    }

    /// The weights' revision of an installed engine: what its install wrote, else the one this gateway installs.
    pub fn installed_revision(&self, engine: EngineId) -> Option<String> {
        let ready = self.ready_info(engine)?;
        Some(ready.get("revision").and_then(Value::as_str).unwrap_or(spec(engine).revision).to_string())
    }

    /// How much an engine's install takes here, MB: the download (weights and packages) and the room on disk.
    pub async fn engine_size(&self, engine: EngineId) -> (u64, u64) {
        let spec = spec(engine);
        let info = self.platform(false).await;
        (spec.weights_mb + PACKAGES_MB, spec.disk_mb.of(flavour(&info)))
    }

    /// The own voices as the models' cards need them, with their engine's server state.
    pub async fn voices_for_cards(&self) -> Vec<OwnVoiceCard> {
        let mut states = HashMap::new();
        for engine in ENGINE_IDS {
            if !self.installed(engine) {
                continue;
            }
            let health = self.servers.health(engine, 1_500).await;
            if let Some(health) = &health {
                self.learn_serving(engine, health.device.as_deref(), health.peak_mb);
            }
            states.insert(engine, health.is_some());
        }
        self.store
            .voices()
            .into_iter()
            .map(|voice| OwnVoiceCard {
                ready: self.installed(voice.engine),
                serving: states.get(&voice.engine).copied().unwrap_or(false),
                voice,
            })
            .collect()
    }

    /// Whether the server of a voice's engine answers.
    pub async fn serving(&self, id: &str) -> bool {
        match self.store.voice(id) {
            Some(voice) => self.servers.health(voice.engine, 1_500).await.is_some(),
            None => false,
        }
    }

    /// For choosing a voice: its engine's server answering (started under the memory rules if needed: tight on
    /// memory, only with `anyway`) and the route the gateway speaks through. Other engines' servers are stopped
    /// afterwards: only the chosen voice holds memory.
    pub async fn prepare(self: &Arc<Self>, id: &str, anyway: bool) -> Result<PreparedVoice, ApiError> {
        let Some(voice) = self.store.voice(id) else {
            return Err(ApiError::not_found("нет такого голоса"));
        };
        if !self.installed(voice.engine) {
            return Err(ApiError::bad_request(format!("Модель {} не установлена.", spec(voice.engine).title)));
        }
        self.ensure(voice.engine, anyway).await?;
        for other in ENGINE_IDS.into_iter().filter(|other| *other != voice.engine) {
            let servers = self.servers.clone();
            tokio::spawn(async move { servers.stop(other).await });
        }
        Ok(PreparedVoice { engine: voice.engine, route: self.route(voice.engine), name: voice.name })
    }

    /// At start: the chosen voice's route at once, its server in the background — the owner chose it, so tight memory
    /// does not keep it silent; a computer short already does. None: the voice is gone.
    pub fn restore(self: &Arc<Self>, id: &str) -> Option<RestoredVoice> {
        let voice = self.store.voice(id)?;
        let lab = self.clone();
        let engine = voice.engine;
        let owner = id.to_string();
        tokio::spawn(async move {
            if let Err(error) = lab.ensure(engine, true).await {
                tracing::warn!(
                    "own voice {owner}: its server did not start ({}) — it is silent until it does",
                    error.message
                );
            }
        });
        Some(RestoredVoice { engine, route: self.route(engine) })
    }

    /// No own voice speaks any more: their servers give their memory back.
    pub async fn release(&self) {
        for engine in ENGINE_IDS {
            self.servers.stop(engine).await;
        }
    }

    async fn ensure(self: &Arc<Self>, engine: EngineId, anyway: bool) -> Result<(), ApiError> {
        let spec = spec(engine);
        let info = self.platform(false).await;
        let fits = verdict(spec, &info);
        if !fits.supported {
            return Err(ApiError::bad_request(fits.reason.unwrap_or_default()));
        }
        let device = fits.device.unwrap_or(Device::Cpu);
        let need =
            need_mb(self.store.learned(&format!("{engine}/serve/{}", device.as_str())), spec.need_mb.serve.on(device));
        let mut health = self.servers.health(engine, 1_500).await;
        if health.is_none() {
            admitted(&admit(&self.memory.sample().await, self.reserve(), need, None), anyway)?;
            // The ceiling is against a runaway only: a voice killed for a number is a silent station.
            let start = StartOptions { device, threads: worker_threads(&info), limit_mb: ceiling_mb(need) };
            self.servers.ensure(engine, start).await.map_err(ApiError::conflict)?;
            health = self.servers.health(engine, 1_500).await;
        }
        if let Some(health) = &health {
            self.learn_serving(engine, health.device.as_deref(), health.peak_mb);
        }
        Ok(())
    }

    /// A voice server's real memory, its peak as its system kept it (/health): the next start is judged by it instead
    /// of the engine's default (06.11: 6.3 GB guessed for OmniVoice on MPS from its imprint). Only ever raised: a fresh
    /// server's peak is lower than a long-running one's.
    fn learn_serving(&self, engine: EngineId, device: Option<&str>, peak_mb: Option<f64>) {
        let (Some(device), Some(peak)) = (device, peak_mb.filter(|peak| *peak != 0.0)) else { return };
        let key = format!("{engine}/serve/{device}");
        match self.store.learned(&key) {
            Some(known) if known != 0.0 && peak <= known => {}
            _ => self.store.learn(&key, peak),
        }
    }

    /// Where an own voice speaks: its engine's server, `POST {base_url}/audio/speech` with `{model: <engine>, voice:
    /// <id>, input, response_format: "wav"}` and no key. A cloned voice's model speaks one phrase at a time, so the
    /// route queues the phrases of an answer and starts a phrase's timeout when it is sent; a server that does not
    /// answer at all is started again, at most every 30 seconds. The same route for an engine every time: one queue.
    fn route(self: &Arc<Self>, engine: EngineId) -> Arc<SpeechRoute> {
        let mut routes = locked(&self.routes);
        routes
            .entry(engine)
            .or_insert_with(|| {
                let lab = Arc::downgrade(self);
                Arc::new(SpeechRoute::limited(
                    &self.servers.url(engine),
                    self.config.timeout_ms,
                    1,
                    Some(Box::new(move || {
                        if let Some(lab) = lab.upgrade() {
                            lab.revive(engine);
                        }
                    })),
                ))
            })
            .clone()
    }

    /// A synthesis found the server gone (crashed, killed by its memory ceiling, the Mac restarted): start it again,
    /// once in a while.
    fn revive(self: &Arc<Self>, engine: EngineId) {
        {
            let mut restarted = locked(&self.restarted_at);
            if restarted.get(&engine).is_some_and(|at| at.elapsed() < RESTART_PAUSE) {
                return;
            }
            restarted.insert(engine, Instant::now());
        }
        tracing::warn!("own voice server {engine} does not answer: starting it again");
        let lab = self.clone();
        tokio::spawn(async move {
            if let Err(error) = lab.ensure(engine, true).await {
                tracing::warn!("own voice server {engine}: {}", error.message);
            }
        });
    }

    fn start_job<W, F>(
        &self,
        kind: JobKind,
        engine: EngineId,
        work: W,
        actor: Option<Actor>,
    ) -> Result<JobState, ApiError>
    where
        W: FnOnce(JobContext) -> F + Send + 'static,
        F: std::future::Future<Output = JobOutcome> + Send + 'static,
    {
        let events = self.events.clone();
        let finished = move |state: &JobState| {
            let outcome = match state.state {
                JobRunState::Done => "done",
                JobRunState::Stopped => "stopped",
                _ => "error",
            };
            let reason =
                state.reason.filter(|_| state.state != JobRunState::Done).map(|reason| reason.as_str().to_string());
            events.emit(GatewayEvent::VoiceJobFinished {
                kind: kind.as_str().into(),
                engine: engine.as_str().into(),
                outcome: outcome.into(),
                reason,
                actor,
            });
            if state.state != JobRunState::Done {
                tracing::warn!("{} {engine}: {}", kind.as_str(), state.error.clone().unwrap_or_default());
            }
        };
        self.jobs
            .start(kind, engine.as_str(), work, Some(Box::new(finished)))
            .map_err(|busy| ApiError::conflict(format!("Уже идёт {} — дождитесь её.", busy.kind.title())))
    }

    fn engine_card(&self, engine: EngineId, info: Option<&PlatformInfo>) -> EngineCard {
        let spec = spec(engine);
        let fits = match info {
            Some(info) => verdict(spec, info),
            None => Verdict {
                supported: false, reason: Some("Проверяю компьютер…".into()), ..Verdict::default()
            },
        };
        let installing = self.jobs.state().is_some_and(|job| {
            job.state == JobRunState::Running && job.kind == JobKind::Install && job.engine == engine.as_str()
        });
        let ready = self.ready_info(engine);
        let status = if installing {
            EngineStatus::Installing
        } else if ready.is_some() {
            EngineStatus::Ready
        } else if fits.supported {
            EngineStatus::Missing
        } else {
            EngineStatus::Unsupported
        };
        let imprint_device = fits.imprint_device.unwrap_or(Device::Cpu);
        let serve_device = fits.device.unwrap_or(Device::Cpu);
        EngineCard {
            id: engine,
            title: spec.title,
            about: spec.about,
            license: spec.license,
            recommended: spec.recommended,
            status,
            device: fits.device,
            slow: fits.slow.then_some(true),
            reason: fits.reason.clone(),
            disk_gb: js::round(spec.disk_mb.of(info.map(flavour).unwrap_or(Flavour::Cpu)) as f64 / 1024.0) as u64,
            need_mb: EngineNeed {
                imprint: need_mb(
                    self.store.learned(&format!("{engine}/imprint/{}", imprint_device.as_str())),
                    spec.need_mb.imprint,
                ),
                serve: need_mb(
                    self.store.learned(&format!("{engine}/serve/{}", serve_device.as_str())),
                    spec.need_mb.serve.on(serve_device),
                ),
            },
            ready,
        }
    }

    fn availability(&self, info: Option<&PlatformInfo>) -> (bool, Option<String>) {
        if !self.own_speech {
            return (
                false,
                Some("Свой голосовой сервер не настроен (TTS_OWN_URL): свои голоса пока не зазвучат.".into()),
            );
        }
        let Some(info) = info else { return (false, Some("Проверяю компьютер…".into())) };
        if !ENGINE_IDS.iter().any(|engine| verdict(spec(*engine), info).supported) {
            return (false, verdict(spec(EngineId::Omnivoice), info).reason);
        }
        (true, None)
    }

    fn ready_info(&self, engine: EngineId) -> Option<Map<String, Value>> {
        let dir = self.runtime_dir.join(engine.as_str());
        let ready = std::fs::read_to_string(dir.join("ready.json")).ok()?;
        let Ok(Value::Object(ready)) = serde_json::from_str::<Value>(&ready) else { return None };
        let os = self.known_platform().map(|info| info.os).unwrap_or(OsName::Darwin);
        python_of(&dir, os).exists().then_some(ready)
    }

    fn installed(&self, engine: EngineId) -> bool {
        self.ready_info(engine).is_some()
    }

    fn spec(&self, engine: &str) -> Result<&'static EngineSpec, ApiError> {
        EngineId::parse(engine).map(spec).ok_or_else(|| ApiError::not_found(format!("нет модели {engine}")))
    }

    /// The own voice chosen to speak (whatever the mode): it cannot be deleted.
    fn chosen_voice(&self) -> Option<String> {
        let pair = match self.speech.get() {
            Some(speech) => speech.own_choice(),
            None => self.env_own_choice.clone(),
        };
        pair.filter(|(model, _)| EngineId::parse(model).is_some()).map(|(_, voice)| voice)
    }

    fn reserve(&self) -> u64 {
        reserve_mb(self.probe.get().total_mb(), self.config.reserve_mb)
    }
}

/// A warning stops the start unless the owner said «всё равно».
fn admitted(admission: &Admission, anyway: bool) -> Result<(), ApiError> {
    match admission.verdict {
        AdmissionVerdict::Tight if !anyway => {
            Err(tight_memory(admission.reason.clone().unwrap_or_else(|| "Памяти впритык.".into())))
        }
        _ => Ok(()),
    }
}

/// A gateway with the voice lab on a Mac that only pretends: its memory as a test sets it, an engine «installed» whose
/// Python is a script speaking as clone-server/imprint.py does (synthetic audio only, in a temporary folder), and a
/// voice server already up that answers /health and speaks a tone.
#[cfg(all(test, unix))]
pub(crate) mod testing {
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    use axum::routing::get;
    use axum::{Json, Router};
    use futures::future::BoxFuture;

    use super::*;
    use crate::app::App;
    use crate::config::Env;
    use crate::speech::audio::pcm::encode_wav;
    use crate::voice_lab::platform::CommandResult;
    use crate::voice_lab::voice_store::tests::{silence, voice};

    /// What the Mac's memory looks like to the sampling: 16 GB free and calm, until a test says otherwise.
    pub struct Memory {
        pub free_mb: AtomicU64,
        pub pressure_level: AtomicU64,
    }

    /// An Apple Silicon Mac, as detection and the memory sampling would see it.
    pub struct FakeMac {
        pub memory: Arc<Memory>,
    }

    impl Probe for FakeMac {
        fn platform(&self) -> String {
            "darwin".into()
        }
        fn arch(&self) -> String {
            "arm64".into()
        }
        fn env(&self, _name: &str) -> Option<String> {
            None
        }
        fn run(&self, command: &str, args: &[&str], _timeout_ms: u64) -> BoxFuture<'static, CommandResult> {
            let answer = |stdout: String| CommandResult { code: Some(0), stdout };
            let result = if command == "sysctl" && args.get(1) == Some(&"machdep.cpu.brand_string") {
                answer("Apple M4 Max\n".into())
            } else if command == "sysctl" {
                // By name, as `sysctl DARWIN_SYSCTL` answers: the free pages, the pressure level, no swap-ins yet.
                let free = self.memory.free_mb.load(Ordering::SeqCst) * 64;
                let level = self.memory.pressure_level.load(Ordering::SeqCst);
                answer(format!(
                    "hw.pagesize: 16384\nvm.page_free_count: {free}\nkern.memorystatus_vm_pressure_level: {level}\n\
                     vm.compressor.swapper.swapins_total: 0\n"
                ))
            } else if command == "uv" || command == "git" {
                answer("ok".into())
            } else {
                CommandResult { code: None, stdout: String::new() }
            };
            Box::pin(async move { result })
        }
        fn exists(&self, _path: &Path) -> bool {
            false
        }
        fn read(&self, _path: &Path) -> Option<String> {
            None
        }
        fn total_mb(&self) -> u64 {
            36_864
        }
        fn memory_bytes(&self) -> (u64, u64) {
            (16_384 * 1_048_576, 36_864 * 1_048_576)
        }
        fn cpus(&self) -> (usize, Option<String>) {
            (14, None)
        }
        fn free_disk_mb(&self, _path: &Path) -> u64 {
            80_000
        }
        fn home(&self) -> PathBuf {
            PathBuf::from("/Users/owner")
        }
    }

    /// The imprint job as the engine's Python would run it (clone-server/imprint.py), without a model: the same JSON
    /// lines and the same voice folder, made of the first take.
    pub const FAKE_PYTHON: &str = r#"#!/bin/sh
take=""; out=""; name=""; engine=""; device=""; samples=""; field=""
for arg in "$@"; do
  case "$arg" in
    --*) field="$arg"; continue ;;
  esac
  case "$field" in
    --takes) [ -z "$take" ] && take="$arg" ;;
    --out) out="$arg" ;;
    --name) name="$arg" ;;
    --engine) engine="$arg" ;;
    --device) device="$arg" ;;
    --samples) samples="$arg" ;;
  esac
done
first=$(printf '%s' "$samples" | sed 's/^\["\([^"]*\)".*$/\1/')
echo '{"stage":"select","percent":12}'
echo '{"memoryMb":900}'
echo '{"stage":"imprint","percent":55}'
mkdir -p "$out/samples"
cp "$take" "$out/ref.wav"
cp "$take" "$out/samples/1.wav"
printf 'imprint' > "$out/voice.pt"
printf '{"id":"%s","name":"%s","engine":"%s","device":"%s","createdAt":"2026-11-06T01:00:00+00:00","reference":{"take":1,"start":1,"end":7,"seconds":6,"snrDb":30,"words":"слова образца"},"samples":[{"n":1,"text":"%s","seconds":3.2}],"rtf":1.2,"peakMb":2100}' "$(basename "$out")" "$name" "$engine" "$device" "$first" > "$out/meta.json"
echo "{\"stage\":\"done\",\"percent\":100,\"peakMb\":2100,\"rtf\":1.2,\"refSeconds\":6,\"device\":\"$device\"}"
"#;

    /// 20 s of reading: phrases and pauses, 16 kHz.
    pub fn spoken_take() -> Vec<u8> {
        let mut pcm = Vec::new();
        for _ in 0..8 {
            pcm.extend(voice(2000, -20.0));
            pcm.extend(silence(500, -60.0));
        }
        encode_wav(&pcm, 16_000)
    }

    pub struct Lab {
        /// Kept for the lab's life: the folder goes with it.
        _dir: tempfile::TempDir,
        pub app: Arc<App>,
        pub memory: Arc<Memory>,
        /// The voice server's base URL, with /v1.
        pub clone_url: String,
        server: tokio::task::JoinHandle<()>,
    }

    impl Drop for Lab {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    /// The voice's server: already up (the gateway takes it over), speaking WAV.
    async fn clone_server() -> (String, tokio::task::JoinHandle<()>) {
        let health = json!({
            "server": "clone-server",
            "engine": "omnivoice",
            "device": "mps",
            "pid": 999_999,
            "instance": "stub",
            "voices": [],
            "memoryMb": 2_600,
            "peakMb": 3_000,
        });
        let wav = encode_wav(&voice(300, -20.0), 24_000);
        let router = Router::new().route("/health", get(move || async move { Json(health) })).fallback(move || {
            let wav = wav.clone();
            async move { ([(axum::http::header::CONTENT_TYPE, "audio/wav")], wav) }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        (url, server)
    }

    fn executable(path: &Path, script: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    pub async fn lab() -> Lab {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        executable(&root.join("runtime/omnivoice/env/bin/python"), FAKE_PYTHON);
        std::fs::write(
            root.join("runtime/omnivoice/ready.json"),
            r#"{"engine":"omnivoice","device":"mps","torch":"2.8.0","source":"bench","installedAt":"2026-11-06T00:00:00Z"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("clone-server")).unwrap();
        let (clone_url, server) = clone_server().await;
        // Nothing answers for the other engine: a closed port of this computer.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let text = |sub: &str| root.join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("DATA_DIR", text("data")),
            ("STATE_DIR", String::new()),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("VOICE_LAB_RUNTIME_DIR", text("runtime")),
            ("VOICE_LAB_BENCH_DIR", String::new()),
            ("VOICE_LAB_SERVER_URLS", format!("omnivoice={clone_url};cosyvoice3=http://127.0.0.1:{closed}/v1")),
            // Our voice server for Silero and Piper (the env's route); recognition on this computer.
            ("TTS_OWN_URL", clone_url.clone()),
            ("ASR_PROVIDER", "openai".into()),
            ("ASR_BASE_URL", "http://127.0.0.1:9/v1".into()),
        ]);
        let mut config = Config::load(&env).expect("the test's configuration");
        config.voice_lab.server_dir = root.join("clone-server");
        let app = App::build(config).unwrap();
        let memory = Arc::new(Memory { free_mb: AtomicU64::new(16_384), pressure_level: AtomicU64::new(1) });
        app.voice_lab.use_probe(Arc::new(FakeMac { memory: memory.clone() }));
        app.voice_lab.start();
        Lab { _dir: dir, app, memory, clone_url, server }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::testing::lab;
    use super::*;
    use crate::voice_lab::voice_store::tests::reading;

    async fn imprinted(lab: &Arc<VoiceLabService>) -> String {
        lab.add_take(reading(20.0, -20.0, -60.0), Some(1)).await.unwrap();
        lab.imprint("omnivoice", Some(&json!("Мой голос")), None, false).await.unwrap();
        for _ in 0..100 {
            if let Some(job) = lab.jobs.state().filter(|job| job.state != JobRunState::Running) {
                assert_eq!(job.state, JobRunState::Done, "{job:?}");
                return job.result.and_then(|result| result.get("voiceId").cloned()).unwrap().as_str().unwrap().into();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the imprint did not finish");
    }

    #[tokio::test]
    async fn gives_the_speech_module_a_route_to_the_voices_server_and_its_server_state_for_the_cards() {
        let lab = lab().await;
        let voice_lab = &lab.app.voice_lab;
        let id = imprinted(voice_lab).await;
        let cards = voice_lab.voices_for_cards().await;
        assert_eq!(cards.len(), 1);
        assert!(cards[0].serving && cards[0].ready);
        assert_eq!(cards[0].voice.samples.len(), 1);
        let prepared = voice_lab.prepare(&id, false).await.unwrap();
        assert_eq!((prepared.engine, prepared.name.as_str()), (EngineId::Omnivoice, "Мой голос"));
        assert_eq!(prepared.route.base_url(), lab.clone_url);
        assert_eq!((prepared.route.max_concurrent(), prepared.route.timeout_ms()), (Some(1), 30_000));
        let again = voice_lab.prepare(&id, false).await.unwrap();
        assert!(Arc::ptr_eq(&prepared.route, &again.route), "one route an engine: one queue");
        assert!(voice_lab.serving(&id).await);
        let restored = voice_lab.restore(&id).expect("the voice is there");
        assert!(Arc::ptr_eq(&restored.route, &prepared.route));
        assert!(voice_lab.restore("v-gone").is_none());
        assert_eq!(voice_lab.prepare("v-gone", false).await.unwrap_err().status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn knows_the_chosen_voice_from_the_speech_module_and_keeps_it() {
        let lab = lab().await;
        let voice_lab = &lab.app.voice_lab;
        let id = imprinted(voice_lab).await;
        assert!(!voice_lab.view().await.voices[0].chosen, "the env's pair is Silero's, not an own voice");
        // The gateway's own voice service, linked at the build (src/links): its choice is the lab's.
        lab.app.voice.use_own("omnivoice", &id, None, None).unwrap();
        assert!(voice_lab.view().await.voices[0].chosen);
        let refused = voice_lab.remove(&id, None).unwrap_err();
        assert_eq!(refused.status, StatusCode::CONFLICT);
        assert_eq!(refused.message, "Этот голос выбран — сначала выберите другой.");
    }

    #[tokio::test]
    async fn starts_a_server_a_synthesis_found_gone_again_at_most_every_30_seconds() {
        let lab = lab().await;
        let voice_lab = &lab.app.voice_lab;
        let id = imprinted(voice_lab).await;
        let route = voice_lab.prepare(&id, false).await.unwrap().route;
        route.unreachable();
        let first = *locked(&voice_lab.restarted_at).get(&EngineId::Omnivoice).expect("revived");
        route.unreachable();
        assert_eq!(locked(&voice_lab.restarted_at).get(&EngineId::Omnivoice), Some(&first), "not again so soon");
    }

    #[tokio::test]
    async fn says_why_an_imprint_cannot_start() {
        let lab = lab().await;
        let voice_lab = &lab.app.voice_lab;
        let no_takes = voice_lab.imprint("omnivoice", None, None, false).await.unwrap_err();
        assert_eq!(no_takes.message, "Нужна хотя бы одна запись от 15 секунд — лучше три по 40.");
        assert_eq!(
            voice_lab.imprint("chatterbox", None, None, false).await.unwrap_err().message,
            "нет модели chatterbox"
        );
        let missing = voice_lab.imprint("cosyvoice3", None, None, false).await.unwrap_err();
        assert_eq!(missing.message, "Сначала установите CosyVoice 3.");
        let installed = voice_lab.install("omnivoice", None).await.unwrap_err();
        assert_eq!(
            (installed.status, installed.message.as_str()),
            (StatusCode::CONFLICT, "OmniVoice уже установлена.")
        );
    }

    #[tokio::test]
    async fn answers_500_for_a_take_that_could_not_be_removed_and_keeps_it_listed() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: geteuid only reads the process's user id.
        if unsafe { libc::geteuid() } == 0 {
            return; // root removes from a read-only folder all the same
        }
        let lab = lab().await;
        let voice_lab = &lab.app.voice_lab;
        voice_lab.add_take(reading(16.0, -20.0, -60.0), None).await.unwrap();
        let draft = voice_lab.store.draft_dir.clone();
        let writable = |mode: u32| std::fs::set_permissions(&draft, std::fs::Permissions::from_mode(mode));
        writable(0o500).unwrap();
        let refused = voice_lab.remove_take(Some(1));
        writable(0o700).unwrap();
        assert_eq!(refused.unwrap_err().status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(voice_lab.view().await.draft.takes.len(), 1);
        voice_lab.remove_take(Some(1)).unwrap();
        assert_eq!(voice_lab.remove_take(Some(1)).unwrap_err().status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn hands_the_engines_the_gateways_paths_and_proxies_never_its_keys() {
        use crate::config::Env;
        use crate::models::model_servers::server_env_of;
        let kept = [
            "PATH",
            "HOME",
            "LANG",
            "lc_all",
            "HTTPS_PROXY",
            "HF_HOME",
            "HF_TOKEN",
            "DATA_DIR",
            "CUDA_VISIBLE_DEVICES",
        ];
        let dropped = ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "STATION_SSH_PASSWORD", "HA_TOKEN", "PATHX", "LC_"];
        let env = Env::from_pairs(kept.iter().chain(dropped.iter()).map(|name| (*name, "1")));
        let mut passed: Vec<String> = server_env_of(&env).into_iter().map(|(name, _)| name).collect();
        passed.sort();
        let mut expected: Vec<String> = kept.iter().map(|name| name.to_string()).collect();
        expected.sort();
        assert_eq!(passed, expected);
        assert_eq!(child_env().get("PYTHONIOENCODING").map(String::as_str), Some("utf-8"));
    }
}
