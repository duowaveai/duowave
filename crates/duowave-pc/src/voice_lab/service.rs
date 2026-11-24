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
    LIMITER_REFUSED_WITHIN,
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
    pub about: String,
    pub license: String,
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

/// The labels are the source texts: jobs.rs puts them through the catalog as the stages come.
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
/// The warning's numbers ride in the error's `errors` (`{state: "tight", memory}`): a voice switched on later keeps them
/// (models' `voice_pending.memory`) and writes the words again for whoever reads them.
fn tight_memory(admission: &Admission) -> ApiError {
    let reason = admission.reason.clone().unwrap_or_else(|| crate::tr!("ОЗУ впритык."));
    let mut error = ApiError::new(StatusCode::PRECONDITION_REQUIRED, reason);
    error.errors = admission.tight.map(|tight| json!({ "state": "tight", "memory": tight }));
    error
}

/// The gateway's environment minus its keys, as the model servers get it (models' server_env), and what Python needs
/// on top to speak UTF-8 lines at once.
pub fn child_env() -> HashMap<String, String> {
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

pub fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
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
    pub jobs: JobRunner,
    servers: Arc<CloneServers>,
    routes: Mutex<HashMap<EngineId, Arc<SpeechRoute>>>,
    pub restarted_at: Mutex<HashMap<EngineId, Instant>>,
    speech: OnceLock<Arc<dyn OwnSpeech>>,
    /// Where imprint.py hears the words of a take when recognition runs inside this program (ASR_PROVIDER=local, the
    /// desktop app): this gateway on the loopback, set when it listens. Nobody listens on ASR_BASE_URL then.
    local_base: OnceLock<String>,
    /// The key in the loopback path of that recognition (`/api/v1/voice-lab/asr/<key>/v1`): imprint.py sends no
    /// sign-in, so the path itself is the pass — random per run of the gateway, only for this computer.
    asr_key: String,
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
            local_base: OnceLock::new(),
            asr_key: crate::utils::ids::uuid().replace('-', ""),
        })
    }

    /// The gateway listens on this port of the loopback: imprint.py reaches recognition inside this program there.
    pub fn set_local_port(&self, port: u16) {
        let _ = self.local_base.set(format!("http://127.0.0.1:{port}"));
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn asr_key_for_tests(&self) -> String {
        self.asr_key.clone()
    }

    /// Whether `key` is the pass of the loopback recognition (controller.rs).
    pub fn asr_key_is(&self, key: &str) -> bool {
        !key.is_empty() && key == self.asr_key
    }

    /// The OpenAI-compatible recognition imprint.py asks the words of a take: inside this program — this gateway's
    /// loopback route with its key (clean-path check 18.11, item 38: ASR_BASE_URL was handed over while nothing
    /// listened there, «Connection refused»); else the recognition server, which must be on this computer.
    pub fn imprint_asr_url(&self) -> Result<String, ApiError> {
        if self.asr.local.is_some() {
            return self
                .local_base
                .get()
                .map(|base| format!("{base}/api/v1/voice-lab/asr/{}/v1", self.asr_key))
                .ok_or_else(|| {
                    ApiError::unavailable(crate::tr!(
                        "Распознавание этого компьютера ещё не готово — попробуйте через минуту."
                    ))
                });
        }
        if !self.asr.enabled || !local_url(&self.asr.base_url) {
            return Err(ApiError::bad_request(crate::tr!(
                "Для отпечатка нужно своё распознавание на этом компьютере: слова образца не отправляются в облако."
            )));
        }
        Ok(self.asr.base_url.clone())
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
            return Err(ApiError::conflict(crate::tr!("{title} уже установлена.", title = spec.title)));
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
                    watch: super::engine_installer::WATCH,
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
            return Err(ApiError::conflict(crate::tr!("Идёт отпечаток — дубли пока нельзя трогать.")));
        }
        let missing = || ApiError::not_found(crate::tr!("нет такого дубля"));
        let n = n.and_then(|n| u32::try_from(n).ok()).ok_or_else(missing)?;
        // A take that could not be removed is the 500 Node answered: never «deleted» with the voice left on disk.
        if !self.store.remove_take(n).map_err(ApiError::internal)? {
            return Err(missing());
        }
        Ok(())
    }

    pub fn clear_draft(&self) -> Result<(), ApiError> {
        if self.imprint_running() {
            return Err(ApiError::conflict(crate::tr!("Идёт отпечаток — дубли пока нельзя трогать.")));
        }
        Ok(self.store.clear_draft()?)
    }

    pub fn take_path(&self, n: Option<i64>) -> Result<PathBuf, ApiError> {
        n.filter(|n| *n >= 1)
            .and_then(|n| u32::try_from(n).ok())
            .map(|n| self.store.take_file(n))
            .filter(|path| path.exists())
            .ok_or_else(|| ApiError::not_found(crate::tr!("нет такого дубля")))
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
            return Err(ApiError::bad_request(crate::tr!(
                "Нужна хотя бы одна запись от {seconds} секунд — лучше три по 40.",
                seconds = SHORT_TAKE_SECONDS
            )));
        }
        let label = match name.and_then(Value::as_str).map(js::trim).filter(|name| !name.is_empty()) {
            Some(name) => js::slice(name, 60).to_string(),
            None => crate::tr!("Мой голос").to_string(),
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
            return Err(ApiError::conflict(crate::tr!("Голос {id} уже есть.", id = voice_id)));
        }
        if !reference.is_file() || words.trim().is_empty() {
            return Err(ApiError::bad_request(crate::tr!(
                "Нет образца голоса или его слов — снять отпечаток не из чего."
            )));
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
            return Err(ApiError::not_found(crate::tr!("нет такого голоса")));
        };
        let spec = self.spec(engine)?;
        if spec.id == voice.engine {
            return Err(ApiError::bad_request(crate::tr!("Голос уже снят для {title}.", title = spec.title)));
        }
        let info = self.ready_to_imprint(spec).await?;
        let dir = self.store.voice_dir(id).map_err(ApiError::bad_request)?;
        let source = match (&voice.words, voice.has_ref) {
            (Some(words), true) => ImprintSource::Reference { file: dir.join("ref.wav"), words: words.clone() },
            _ => {
                let takes = self.store.takes_of(id);
                if takes.is_empty() {
                    return Err(ApiError::bad_request(crate::tr!(
                        "У голоса не сохранились ни образец, ни записи — переснять не из чего."
                    )));
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
            return Err(ApiError::bad_request(crate::tr!("Сначала установите {title}.", title = spec.title)));
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
        let asr_url = match &source {
            ImprintSource::Takes(_) => Some(self.imprint_asr_url()?),
            ImprintSource::Reference { .. } => None,
        };
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
                    asr_url.clone().unwrap_or_default(),
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
        let plain = (text(&python), args.clone());
        let (command, args) = with_limits(&info, &plain.0, &args, Some(admission.ceiling_mb));
        let lab = self.clone();
        let creator = actor.clone();
        let ceiling = admission.ceiling_mb;
        self.start_job(
            JobKind::Imprint,
            spec.id,
            move |context: JobContext| async move {
                context.stage("prepare", &crate::tr!("Готовлю записи"), Some(1.0));
                let options = RunOptions {
                    cwd: Some(lab.server_dir.clone()),
                    env: Some(child_env()),
                    limit_mb: Some(ceiling),
                    anyway,
                    percent: Some((1.0, 99.0)),
                    labels: IMPRINT_LABELS,
                    cancel: None,
                };
                let outcome: JobOutcome = async {
                    // imprint.py makes the voice's folders with the default modes: under a root that is the owner's.
                    lab.store.folders().map_err(|error| error.to_string())?;
                    // A limiter that refuses here ends the program at once: then once more as it is.
                    let began = std::time::Instant::now();
                    let result = match context.run(&command, &args, options.clone()).await {
                        Err(error) if command != plain.0 && began.elapsed() < LIMITER_REFUSED_WITHIN => {
                            tracing::warn!(
                                "imprint: under {command} it ended at once ({}) — started again without the limiter",
                                error.message
                            );
                            context.run(&plain.0, &plain.1, options).await?
                        }
                        other => other?,
                    };
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
            return Err(ApiError::not_found(crate::tr!("нет такого голоса")));
        }
        let Some(name) = name.and_then(Value::as_str).map(js::trim).filter(|name| !name.is_empty()) else {
            return Err(ApiError::bad_request(crate::tr!("Имя не может быть пустым.")));
        };
        let renamed = self.store.rename(id, js::slice(name, 60)).map_err(ApiError::internal)?;
        self.events.emit(GatewayEvent::VoiceOwnRenamed { id: id.to_string(), actor });
        Ok(renamed)
    }

    pub fn remove(&self, id: &str, actor: Option<Actor>) -> Result<(), ApiError> {
        if self.store.voice(id).is_none() {
            return Err(ApiError::not_found(crate::tr!("нет такого голоса")));
        }
        if self.chosen_voice().as_deref() == Some(id) {
            return Err(ApiError::conflict(crate::tr!("Этот голос выбран — сначала выберите другой.")));
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
        path.ok_or_else(|| ApiError::not_found(crate::tr!("нет такой пробы")))
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

    /// Free room on the disk where the engines are installed, MB.
    pub fn free_disk_mb(&self) -> u64 {
        self.probe.get().free_disk_mb(&self.runtime_dir)
    }

    /// The memory a voice's server may take now without pressing the household's, MB: the pool less the reserve.
    pub async fn free_memory_mb(&self) -> u64 {
        let snapshot = self.memory.snapshot().await;
        snapshot.pool_mb.saturating_sub(reserve_mb(snapshot.total_mb, self.config.reserve_mb))
    }

    /// An own voice's name by its id (a character's voice: the character's title).
    pub fn voice_name(&self, id: &str) -> Option<String> {
        self.store.voices().into_iter().find(|voice| voice.id == id).map(|voice| voice.name)
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
            return Err(ApiError::not_found(crate::tr!("нет такого голоса")));
        };
        if !self.installed(voice.engine) {
            return Err(ApiError::bad_request(crate::tr!(
                "Модель {title} не установлена.",
                title = spec(voice.engine).title
            )));
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
            .map_err(|busy| ApiError::conflict(crate::tr!("Уже идёт {job} — дождитесь её.", job = busy.kind.title())))
    }

    fn engine_card(&self, engine: EngineId, info: Option<&PlatformInfo>) -> EngineCard {
        let spec = spec(engine);
        let fits = match info {
            Some(info) => verdict(spec, info),
            None => Verdict {
                supported: false, reason: Some(crate::tr!("Проверяю компьютер…")), ..Verdict::default()
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
            about: crate::locale::text::translate(spec.about, &[]),
            license: crate::locale::text::translate(spec.license, &[]),
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
                Some(crate::tr!("Свой голосовой сервер не настроен (TTS_OWN_URL): свои голоса пока не зазвучат.")),
            );
        }
        let Some(info) = info else {
            return (false, Some(crate::tr!("Проверяю компьютер…")));
        };
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
        EngineId::parse(engine)
            .map(spec)
            .ok_or_else(|| ApiError::not_found(crate::tr!("нет модели {engine}", engine = engine)))
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
        AdmissionVerdict::Tight if !anyway => Err(tight_memory(admission)),
        _ => Ok(()),
    }
}
