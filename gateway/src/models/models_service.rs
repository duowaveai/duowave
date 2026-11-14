//! The panel's model shop: an orchestrator over the catalog (CatalogDb: what is offered), the files on disk
//! (ModelFiles: what is installed) and the Python servers (ModelServers: what is loaded). Choices refer to catalog
//! rows by id and go to the voice (whose voice answers: src/speech's VoiceService) and the settings — the config is
//! never written. A voice switches with no restart anywhere; a new voice file reaches the voice server by a rescan,
//! a recognition model by a load over HTTP, and an older server asks for its restart (a Mac's only). As in Handy: a
//! card, a size, one button.
//!
//! src/voice-lab is the Node gateway's, removed from the tree: git tag `node-gateway-last`.

use std::any::Any;
use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use bytes::Bytes;
use chrono::Datelike;
use futures::future::{BoxFuture, FutureExt, Shared};
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::catalog_db::{AsrModelRecord, CatalogDb, VoiceEntry, VoiceModelRecord};
use super::model_files::{safe_name, ModelFiles, Progress};
use super::model_servers::{ModelServers, ModelServersOptions, ServerName, TtsLoaded, HEALTH};
use crate::config::{Config, TtsProvider, UniproxyMode};
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::platform::http::ApiError;
use crate::settings::definitions::{
    PanelMode, PendingWait, VoicePending, VoiceRecipe, ASR_MODEL, MODE, VOICE_CHOICE, VOICE_PENDING, VOICE_PROVIDER,
};
use crate::settings::SettingsService;
use crate::speech::asr::local::catalog as local_catalog;
use crate::speech::asr::local::supervisor::{LocalAsr, LocalStatus};
use crate::speech::tts::managed::{ManagedTts, TtsStatus as ManagedTtsStatus, RUNTIME_ID, RUNTIME_TITLE};
use crate::storage::Db;
use crate::utils::time::now_iso;

const SILERO: &str = "silero";
const PIPER: &str = "piper";

/// An own voice's card id: `own:<voice id>`.
pub const OWN_PREFIX: &str = "own:";

/// Why a voice file is not downloaded from here (Silero's are given out by hand).
fn by_hand() -> String {
    crate::tr!("Этот голос не скачивается отсюда: файл кладут вручную.")
}

/// The speakers a Silero model keeps inside; a Piper model is one voice.
const SILERO_VOICES: &[&str] = &["aidar", "baya", "kseniya", "xenia", "eugene"];

/// The registry is re-read in the background at most this often; the database serves it in between.
const REGISTRY_REFRESH: Duration = Duration::from_secs(6 * 3600);
const REGISTRY_PAGE_TIMEOUT: Duration = Duration::from_secs(20);
const REGISTRY_PAGES: usize = 20;

/// The showcase (showcase.json): what seeds the catalog on every start, upserted by id.
const SHOWCASE_JSON: &str = include_str!("showcase.json");

#[derive(Debug, Clone, Deserialize)]
struct Showcase {
    asr: Vec<ShowcaseAsr>,
    tts: Vec<ShowcaseTts>,
}

#[derive(Debug, Clone, Deserialize)]
struct ShowcaseAsr {
    id: String,
    model: String,
    title: String,
    description: String,
    badge: Option<String>,
    license: Option<String>,
    source: Option<String>,
    /// Shown in «Советуем»: the models we advise (GigaAM v3, the best Russian hearing); the rest behind «ещё».
    #[serde(default)]
    recommended: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ShowcaseTts {
    id: String,
    /// yandex, silero or piper.
    engine: String,
    title: String,
    description: String,
    badge: Option<String>,
    license: Option<String>,
    locale: Option<String>,
    quality: Option<String>,
    size: Option<String>,
    url: Option<String>,
    file: Option<String>,
    voice: Option<String>,
}

static SHOWCASE: LazyLock<Showcase> =
    LazyLock::new(|| serde_json::from_str(SHOWCASE_JSON).expect("the built-in showcase parses"));

// What the shop asks of the modules it drives (src/speech, src/voice-lab): the gateway implements these traits over
// them (src/links); without them (a test) the shop works and says so where they are needed.

/// Whose voice the panel shows: Alice's, or ours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Whose {
    Yandex,
    Own,
}

/// A voice as our engine is asked for it: the model (silero, piper, an own voice's engine) and the voice in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoicePair {
    pub model: String,
    pub voice: String,
}

/// A choice by catalog ids, as voice.changed and the settings carry it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceIds {
    pub model_id: String,
    pub voice_id: String,
}

/// The way to an own voice's own server (speech's SpeechRoute: its address and its one-phrase-at-a-time queue).
/// The voice lab makes it and the voice takes it; the shop only carries it from one to the other (src/links puts it
/// in and takes it out).
pub type OwnRoute = Arc<dyn Any + Send + Sync>;

/// The voice that answers (src/speech: VoiceService) — whose voice speaks, and which of ours. The env gives the
/// start; the shop's choice overrides it at runtime.
pub trait Voice: Send + Sync {
    /// Our engine is configured (TTS_OWN_URL): only then can a voice of ours be chosen.
    fn has_own_speech(&self) -> bool;
    /// The provider as chosen: the env's, or the panel's.
    fn chosen_provider(&self) -> TtsProvider;
    /// Whose voice the panel shows, with Yandex allowed or not («Свой» forbids it).
    fn whose(&self, yandex_allowed: bool) -> Whose;
    /// The pair our engine is asked for now.
    fn own_choice(&self) -> Option<VoicePair>;
    /// Alice's voice from the next answer on (voice.changed).
    fn use_yandex(&self);
    /// Our voice from the next answer on; an own voice brings the route to its server (voice.changed).
    fn use_own(&self, own: VoicePair, ids: VoiceIds, route: Option<OwnRoute>) -> Result<(), ApiError>;
    /// At start: what the settings say, without an event (nothing changed).
    fn restore(&self, provider: Option<TtsProvider>, own: Option<VoicePair>, route: Option<OwnRoute>);
}

/// One of the owner's own voices as its card needs it (the voice lab's voicesForCards).
#[derive(Debug, Clone, PartialEq)]
pub struct OwnVoiceCard {
    pub id: String,
    pub name: String,
    /// The engine it speaks with (omnivoice, cosyvoice3), its title and licence.
    pub engine: String,
    pub engine_title: String,
    pub engine_license: String,
    pub created_at: String,
    pub ref_seconds: Option<f64>,
    pub samples: usize,
    /// Its engine is installed: the card can be chosen.
    pub ready: bool,
    /// Its engine's server answers.
    pub serving: bool,
    /// It keeps what a reimprint under another engine needs: its reference and words, or its takes.
    pub reimprint: bool,
    /// `package`: a character's voice its package brought.
    pub origin: Option<String>,
}

/// An engine of the own voices as it stands here: what choosing a voice of it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineState {
    pub id: String,
    pub title: String,
    /// ready, installing, missing, unsupported.
    pub status: String,
    /// Why it does not run here.
    pub reason: Option<String>,
    /// The install's download and its room on disk, MB.
    pub download_mb: u64,
    pub disk_mb: u64,
}

/// The voice lab's job, running or the last one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabJob {
    /// install, imprint.
    pub kind: String,
    pub engine: String,
    pub running: bool,
    pub percent: u32,
    /// Why it failed or was stopped.
    pub failed: Option<String>,
}

/// An own voice made ready to speak: its engine's server answering, and the route to it.
pub struct PreparedVoice {
    pub engine: String,
    pub route: OwnRoute,
    pub name: String,
}

/// An own voice at start: the route at once, its server coming up in the background.
pub struct RestoredVoice {
    pub engine: String,
    pub route: OwnRoute,
}

/// The owner's own voices (src/voice-lab): their cards, and a chosen one's server and route.
pub trait OwnVoices: Send + Sync {
    fn voices_for_cards(&self) -> BoxFuture<'_, Vec<OwnVoiceCard>>;
    /// Its engine's server answering (tight on memory only with `anyway`: 428 otherwise), other engines' stopped.
    fn prepare(&self, id: String, anyway: bool) -> BoxFuture<'_, Result<PreparedVoice, ApiError>>;
    /// None: the voice is gone.
    fn restore(&self, id: &str) -> Option<RestoredVoice>;
    /// No own voice speaks any more: their servers give their memory back (in the background).
    fn release(&self);
    /// The engines and how they stand here.
    fn engines(&self) -> BoxFuture<'_, Vec<EngineState>>;
    /// An engine's install started (the voice lab's job).
    fn install_engine(&self, engine: String, actor: Option<Actor>) -> BoxFuture<'_, Result<(), ApiError>>;
    /// A voice imprinted from a reference with known words (a character's): the voice lab's job.
    fn imprint(&self, recipe: VoiceRecipe, anyway: bool, actor: Option<Actor>) -> BoxFuture<'_, Result<(), ApiError>>;
    /// The voice lab's job running, else its last one.
    fn job(&self) -> Option<LabJob>;
    /// An own voice's engine; None: no such voice.
    fn voice_engine(&self, id: &str) -> Option<String>;
}

/// The modules the shop drives besides its own parts; None in a test that gives none.
#[derive(Clone, Default)]
pub struct ModelsLinks {
    pub voice: Option<Arc<dyn Voice>>,
    pub own_voices: Option<Arc<dyn OwnVoices>>,
    /// Recognition on this computer itself (ASR_PROVIDER=local): the model choice goes to it, not to a server.
    pub local_asr: Option<Arc<LocalAsr>>,
    /// The own voice's server the app sets up and keeps (src/speech/tts/managed.rs): its environment is a voice's
    /// need like a voice lab engine, and it runs while a Silero or Piper voice is chosen.
    pub managed_tts: Option<Arc<ManagedTts>>,
}

/// The shop's own parts: tests give their own folder and servers.
pub struct ModelsParts {
    pub files: ModelFiles,
    pub servers: ModelServers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadPhase {
    Downloading,
    Done,
    Error,
    Canceled,
}

/// What a download looks like to the panel. A finished one leaves the list: the card's `installed` says the rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DownloadState {
    pub id: String,
    pub state: DownloadPhase,
    pub bytes: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardVoice {
    pub id: String,
    pub name: String,
}

/// An own voice's details: its id in the voice lab, when it was taken, its reference and samples.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnDetails {
    pub id: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_seconds: Option<f64>,
    pub samples: usize,
    pub engine_title: String,
    pub serving: bool,
}

/// A voice card as the panel sees it: the catalog row with the world laid over it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsCard {
    pub id: String,
    /// yandex, silero, piper, or an own voice's engine.
    pub engine: String,
    /// own: one of the owner's own voices (an imprint, src/voice-lab), shown after the recommended cards.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// builtin: a recommended card of the showcase; registry: the open Piper voices.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub own: Option<OwnDetails>,
    pub title: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The short word of the first voice — for search and lists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// The voices inside the model (Silero keeps five, Piper one): what a synthesis request asks for.
    pub voices: Vec<CardVoice>,
    /// The files are on disk, all of them (a Piper voice needs its .onnx.json too).
    pub installed: bool,
    /// The file is on disk and the voice server has it loaded: only such a voice can really speak.
    pub loaded: bool,
    pub active: bool,
    /// Can be downloaded from here (Silero's files are given out by hand).
    pub downloadable: bool,
    /// ready, needs (its model or file is not here), installing (it is coming: `progress`), unsupported (`reason`).
    pub state: CardState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What choosing it needs first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs: Option<VoiceNeeds>,
}

/// Whether a voice can speak here now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CardState {
    Ready,
    Needs,
    Installing,
    Unsupported,
}

/// Another way to a voice whose engine is not here: an own voice imprinted anew under an engine that is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Alternative {
    pub kind: &'static str,
    pub engine: String,
}

/// What a voice needs before it speaks (docs/plans/characters.md, section 5): an engine of the own voices, or a voice
/// file (Piper, Silero). The packages' catalog speaks the same language in its `needs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceNeeds {
    /// engine, file.
    pub kind: &'static str,
    pub id: String,
    pub title: String,
    pub download_mb: Option<u64>,
    pub disk_mb: Option<u64>,
    /// About how long, for the panel's «~2 минуты».
    pub minutes: Option<u64>,
    pub alternatives: Vec<Alternative>,
}

/// About how many minutes a download takes: measured, OmniVoice's 4 GB came down in 1 min 46 s.
fn minutes_for(download_mb: u64) -> u64 {
    download_mb.div_ceil(2_400).max(1)
}

/// An engine's needs as a voice's.
pub fn engine_needs(engine: &EngineState, alternatives: Vec<Alternative>) -> VoiceNeeds {
    VoiceNeeds {
        kind: "engine",
        id: engine.id.clone(),
        title: engine.title.clone(),
        download_mb: Some(engine.download_mb),
        disk_mb: Some(engine.disk_mb),
        minutes: Some(minutes_for(engine.download_mb)),
        alternatives,
    }
}

/// A voice file's needs: its size as the catalog writes it («60 МБ»).
fn file_needs(record: &VoiceModelRecord) -> VoiceNeeds {
    let megabytes = record
        .size
        .as_deref()
        .and_then(|size| size.split_whitespace().next())
        .and_then(|number| number.replace(',', ".").parse::<f64>().ok())
        .map(|number| number.round() as u64);
    VoiceNeeds {
        kind: "file",
        id: record.id.clone(),
        title: record.title.clone(),
        download_mb: megabytes,
        disk_mb: megabytes,
        minutes: Some(1),
        alternatives: Vec::new(),
    }
}

/// A pending voice as the panel shows it (`ttsPending`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingView {
    pub model_id: String,
    pub voice_id: Option<String>,
    pub wait_for: PendingWait,
    /// waiting, tight (memory: it waits for the owner's «всё равно?»), failed.
    pub state: String,
    pub progress: Option<u32>,
    pub reason: Option<String>,
    pub since: String,
}

/// A choice of a voice: by catalog ids (or the older pair), with the owner's answers — download what it needs and
/// switch it on then (`install`), start it on tight memory (`anyway`). `recipe`: a character's voice its package
/// gives no snapshot for, imprinted from its reference first — never from the panel's body.
#[derive(Debug, Clone, Default)]
pub struct VoiceRequest {
    pub body: Value,
    pub install: bool,
    pub anyway: bool,
    pub recipe: Option<VoiceRecipe>,
}

impl VoiceRequest {
    /// A card by its id.
    pub fn card(model_id: &str) -> VoiceRequest {
        VoiceRequest { body: json!({ "modelId": model_id }), ..VoiceRequest::default() }
    }

    /// The panel's body: `{modelId, voiceId?, install?, anyway?}`.
    pub fn of(body: &Value) -> VoiceRequest {
        VoiceRequest {
            body: body.clone(),
            install: body.get("install") == Some(&Value::Bool(true)),
            anyway: body.get("anyway") == Some(&Value::Bool(true)),
            recipe: None,
        }
    }
}

/// What a choice came to: the voice speaks from the next answer, or it is pending until what it needs is there.
#[derive(Debug, Clone, PartialEq)]
pub enum Chosen {
    Active(Activated),
    Pending(PendingView),
}

/// A recognition card: `model` is what the recognition server loads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrCard {
    #[serde(flatten)]
    pub record: AsrModelRecord,
    /// One of the models we advise: the panel shows them in «Советуем».
    pub recommended: bool,
    pub active: bool,
    /// Chosen in the panel but not serving yet: the recognition server needs its restart.
    pub pending: bool,
    /// With recognition on this computer: whether the model runs here, its file's size and whether it is here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalCard>,
}

/// A recognition model as this computer's own engine sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalCard {
    /// It has a GGUF for transcribe.cpp; false: only an external recognition server runs it.
    pub runs_here: bool,
    /// Its file, bytes (0 when it does not run here).
    pub size: u64,
    pub installed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsView {
    pub asr: Vec<AsrCard>,
    pub tts: Vec<TtsCard>,
    /// What speaks now, by catalog ids: `yandex` — Alice's own voice.
    pub tts_active: VoiceIds,
    /// Whose voice is chosen: Yandex's or ours (what «Свой» warns about).
    pub voice: Whose,
    /// Whose voice speaks after the mode: in «Свой» Alice's gives way to ours.
    pub speaking: Whose,
    /// The station's mode: in relay Yandex answers and hears everything itself.
    pub mode: PanelMode,
    /// The recognition server's reality: which model it loaded; null when it is down.
    pub asr_loaded: Option<String>,
    /// Recognition on this computer itself: its download, load, device and failure. None: a server recognises.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_asr: Option<LocalStatus>,
    /// The own voice's server the app keeps: its install, start and failure. None: a server of one's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tts_managed: Option<ManagedTtsStatus>,
    /// The voice server answers its /health.
    pub tts_up: bool,
    pub downloads: Vec<DownloadState>,
    /// Whether a new model or voice can be put to work from here: a server that takes it over HTTP, or the Mac's
    /// restart.
    pub can_restart: bool,
    /// Whether our voice server is configured at all (TTS_OWN_URL).
    pub own_voice_server: bool,
    /// A restart that is running now: its server's name.
    pub restarting: Option<ServerName>,
    /// A voice chosen before its model or file was here: switched on when they are.
    pub tts_pending: Option<PendingView>,
}

/// What a choice answers: whether the server has yet to take it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Activated {
    pub needs_restart: bool,
}

/// The voice of ours chosen in the panel, by catalog ids and as the voice server names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChosenVoice {
    pub model_id: String,
    pub voice_id: String,
    /// silero, piper, or an own voice's engine.
    pub engine: String,
    pub voice: String,
}

/// The choice in force, by catalog ids and as the voice server names it; an own voice brings its server's route.
#[derive(Clone)]
struct Choice {
    model_id: String,
    voice_id: String,
    engine: String,
    voice: String,
    route: Option<OwnRoute>,
}

impl Choice {
    fn ids(&self) -> VoiceIds {
        VoiceIds { model_id: self.model_id.clone(), voice_id: self.voice_id.clone() }
    }
}

/// A choice as the settings keep it: `{modelId, voiceId}`.
fn stored_ids(ids: &VoiceIds) -> Map<String, Value> {
    let mut stored = Map::new();
    stored.insert("modelId".into(), Value::String(ids.model_id.clone()));
    stored.insert("voiceId".into(), Value::String(ids.voice_id.clone()));
    stored
}

/// The voice id of an own voice's card id (`own:<id>`).
fn own_id(model_id: Option<&Value>) -> Option<String> {
    model_id
        .and_then(Value::as_str)
        .and_then(|id| id.strip_prefix(OWN_PREFIX))
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

struct Download {
    id: String,
    progress: Progress,
    cancel: CancellationToken,
    phase: Mutex<(DownloadPhase, Option<String>)>,
}

impl Download {
    fn new(id: &str) -> Download {
        Download {
            id: id.to_string(),
            progress: Progress::default(),
            cancel: CancellationToken::new(),
            phase: Mutex::new((DownloadPhase::Downloading, None)),
        }
    }

    fn phase(&self) -> DownloadPhase {
        lock(&self.phase).0
    }

    fn set(&self, phase: DownloadPhase, error: Option<String>) {
        *lock(&self.phase) = (phase, error);
    }

    fn state(&self) -> DownloadState {
        let (state, error) = lock(&self.phase).clone();
        DownloadState { id: self.id.clone(), state, bytes: self.progress.bytes(), total: self.progress.total(), error }
    }
}

struct Restarting {
    which: ServerName,
    done: Shared<BoxFuture<'static, Result<(), String>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Clears the one Silero install at a time however the install ends.
struct SileroSlot<'a>(&'a Mutex<Option<String>>);

impl Drop for SileroSlot<'_> {
    fn drop(&mut self) {
        *lock(self.0) = None;
    }
}

pub struct ModelsService {
    catalog: CatalogDb,
    settings: Arc<SettingsService>,
    events: EventBus,
    files: ModelFiles,
    servers: Arc<ModelServers>,
    links: ModelsLinks,
    /// The env's voice before any panel choice (TTS_PROVIDER, TTS_OWN_MODEL/VOICE) and the station's mode before any
    /// panel switch (UNIPROXY_MODE).
    env_provider: TtsProvider,
    env_own: Option<VoicePair>,
    env_mode: PanelMode,
    registry_url: String,
    /// Where the registry's next pages may be: its own host.
    registry_origin: String,
    downloads: Mutex<Vec<Arc<Download>>>,
    /// The catalog as one list, re-read from the database after a registry refresh lands.
    models: Mutex<Option<Arc<Vec<VoiceModelRecord>>>>,
    registry_at: Mutex<Option<Instant>>,
    /// One registry refresh at a time.
    refreshing: Mutex<Option<Shared<BoxFuture<'static, ()>>>>,
    /// The first fetch of a catalog without the registry: everyone waits for it.
    first_refresh: Mutex<Option<Shared<BoxFuture<'static, ()>>>>,
    /// One Silero install at a time: two generations on disk would race for the server's first .pt.
    silero_install: Mutex<Option<String>>,
    /// One server restart at a time: a second click joins the first.
    restarting: Mutex<Option<Restarting>>,
    /// Our voice by catalog ids, when it is one of the catalog's (the env's pair may be none of them).
    choice: Mutex<Option<Choice>>,
    asr_chosen: Mutex<Option<AsrModelRecord>>,
    /// One look at the pending voice at a time.
    pending_turn: tokio::sync::Mutex<()>,
}

impl ModelsService {
    /// The shop over the configured folder and servers: TTS_MODELS_DIR, ASR_BASE_URL, TTS_OWN_URL.
    pub fn new(
        config: &Config,
        db: Arc<Db>,
        events: EventBus,
        settings: Arc<SettingsService>,
        links: ModelsLinks,
    ) -> ModelsService {
        let asr = &config.speech.asr;
        let own = config.gateway.tts.own.as_ref();
        let servers = ModelServers::new(
            // Recognition on this computer has no server to ask or restart.
            (asr.enabled && asr.local.is_none()).then_some(asr.base_url.as_str()),
            own.map(|own| own.base_url.as_str()),
            None,
            ModelServersOptions {
                asr_api_key: asr.api_key.clone(),
                tts_api_key: own.and_then(|own| own.api_key.clone()),
                mac_deploy: config.models.mac_deploy_dir.clone(),
                ..Default::default()
            },
        );
        let parts = ModelsParts { files: ModelFiles::new(&config.models.tts_dir), servers };
        ModelsService::with_parts(config, db, events, settings, links, parts)
    }

    pub fn with_parts(
        config: &Config,
        db: Arc<Db>,
        events: EventBus,
        settings: Arc<SettingsService>,
        links: ModelsLinks,
        parts: ModelsParts,
    ) -> ModelsService {
        let registry_url = config.models.piper_registry.clone();
        let registry_origin = url::Url::parse(&registry_url)
            .map(|url| format!("{}/", url.origin().ascii_serialization()))
            .unwrap_or_else(|_| "https://huggingface.co/".into());
        ModelsService {
            catalog: CatalogDb::new(db),
            settings,
            events,
            files: parts.files,
            servers: Arc::new(parts.servers),
            links,
            env_provider: config.gateway.tts.provider,
            env_own: config
                .gateway
                .tts
                .own
                .as_ref()
                .map(|own| VoicePair { model: own.model.clone(), voice: own.voice.clone() }),
            env_mode: match config.gateway.mode {
                UniproxyMode::Gateway => PanelMode::Gateway,
                UniproxyMode::Relay => PanelMode::Relay,
            },
            registry_url,
            registry_origin,
            downloads: Mutex::new(Vec::new()),
            models: Mutex::new(None),
            registry_at: Mutex::new(None),
            refreshing: Mutex::new(None),
            first_refresh: Mutex::new(None),
            silero_install: Mutex::new(None),
            restarting: Mutex::new(None),
            choice: Mutex::new(None),
            asr_chosen: Mutex::new(None),
            pending_turn: tokio::sync::Mutex::new(()),
        }
    }

    /// The catalog's repository.
    pub fn catalog(&self) -> &CatalogDb {
        &self.catalog
    }

    /// The local recognition and voice servers: their addresses and what they loaded.
    pub fn servers(&self) -> &ModelServers {
        &self.servers
    }

    /// Our voice as chosen in the panel (or the env's pair, when it is one of the catalog's).
    pub fn chosen_voice(&self) -> Option<ChosenVoice> {
        lock(&self.choice).as_ref().map(|choice| ChosenVoice {
            model_id: choice.model_id.clone(),
            voice_id: choice.voice_id.clone(),
            engine: choice.engine.clone(),
            voice: choice.voice.clone(),
        })
    }

    /// The recognition card chosen in the panel; its `model` is what the recognition server loads.
    pub fn chosen_asr(&self) -> Option<AsrModelRecord> {
        lock(&self.asr_chosen).clone()
    }

    /// At start: what a crash left mid-download goes, the showcase seeds the catalog, the stored choices come back.
    pub async fn start(self: &Arc<Self>) -> Result<(), ApiError> {
        let removed = self.files.cleanup();
        if !removed.is_empty() {
            tracing::info!("models: half-downloaded files removed: {}", removed.join(", "));
        }
        // The showcase seeds the catalog: re-seeding upserts by id, so nothing ever duplicates.
        let voices: Vec<VoiceModelRecord> =
            SHOWCASE.tts.iter().filter(|entry| entry.engine != "yandex").map(showcase_voice_model).collect();
        self.catalog.save_voice_models(&voices)?;
        let asr: Vec<AsrModelRecord> = SHOWCASE
            .asr
            .iter()
            .filter(|entry| entry.model != "yandex")
            .map(|entry| AsrModelRecord {
                id: entry.id.clone(),
                model: entry.model.clone(),
                title: entry.title.clone(),
                description: entry.description.clone(),
                badge: entry.badge.clone(),
                license: entry.license.clone(),
                source: entry.source.clone(),
            })
            .collect();
        self.catalog.save_asr(&asr)?;
        self.restore().await?;
        // A pending voice is looked at again whenever an install, a download or an imprint ends.
        let me = Arc::downgrade(self);
        self.events.on("voice-pending", move |event| {
            let ended = matches!(
                &*event,
                GatewayEvent::VoiceEngineInstalled { .. }
                    | GatewayEvent::ModelInstalled { .. }
                    | GatewayEvent::VoiceJobFinished { .. }
                    | GatewayEvent::VoiceOwnCreated { .. }
            );
            if let (true, Some(shop)) = (ended, me.upgrade()) {
                shop.look_at_pending();
            }
        });
        // One from before a restart: what it waited for ended with the gateway.
        self.look_at_pending();
        Ok(())
    }

    async fn restore(self: &Arc<Self>) -> Result<(), ApiError> {
        let stored_value = self.settings.get(&VOICE_CHOICE, None).map(Value::Object);
        let stored = match &stored_value {
            Some(value) => self.resolve_choice(value, true).await?,
            None => None,
        };
        let lost_own = stored_value.as_ref().and_then(|value| own_id(value.get("modelId")));
        if let (Some(lost), None) = (&lost_own, &stored) {
            tracing::error!(
                "own voice {lost} is chosen but gone from the voice lab: the env's voice speaks until another is chosen"
            );
        }
        let provider = self.settings.get(&VOICE_PROVIDER, None).and_then(|provider| TtsProvider::parse(&provider));
        // A stored «ours» without a voice server of ours cannot speak: the env's provider stays.
        let usable = provider
            .filter(|provider| matches!(provider, TtsProvider::Yandex | TtsProvider::None) || self.has_own_speech());
        if let (Some(provider), None) = (provider, usable) {
            tracing::warn!(
                "stored tts provider {} needs TTS_OWN_URL; the env's {} stays",
                provider.as_str(),
                self.chosen_provider().as_str()
            );
        }
        if let Some(voice) = &self.links.voice {
            voice.restore(
                usable,
                stored.as_ref().map(|stored| VoicePair { model: stored.engine.clone(), voice: stored.voice.clone() }),
                stored.as_ref().and_then(|stored| stored.route.clone()),
            );
        }
        let choice = match stored {
            Some(stored) => {
                tracing::info!("tts: {}/{} speaks (from the settings)", stored.model_id, stored.voice_id);
                Some(stored)
            }
            None => match &self.env_own {
                Some(pair) => self.resolve_choice(&json!({"model": pair.model, "voice": pair.voice}), false).await?,
                None => None,
            },
        };
        *lock(&self.choice) = choice;
        // The app's own voice server runs while a Silero or Piper voice speaks (an own voice of the voice lab has a
        // server of its own): started with the gateway, so the voice is there after a restart of the computer.
        if let Some(tts) = &self.links.managed_tts {
            let catalog_voice =
                lock(&self.choice).as_ref().is_some_and(|choice| !choice.model_id.starts_with(OWN_PREFIX));
            let speaks = self
                .links
                .voice
                .as_ref()
                .is_some_and(|voice| matches!(voice.chosen_provider(), TtsProvider::Own | TtsProvider::Mixed));
            tts.want(catalog_voice && speaks);
            // A Silero or Piper voice chosen before this app set up its server (a server of one's own then, or an
            // update with other pins): its environment is installed now, and the voice speaks once it is.
            if catalog_voice && speaks && !tts.installed() && !tts.installing() {
                tracing::info!("tts: the chosen voice needs the own voice server's environment: installing it");
                tts.install();
            }
        }

        if let Some(asr) = self.settings.get(&ASR_MODEL, None) {
            let chosen = self.catalog.list_asr()?.into_iter().find(|entry| entry.id == asr || entry.model == asr);
            *lock(&self.asr_chosen) = chosen;
        }
        // Recognition on this computer serves the chosen model, or GigaAM v3 on a new computer. A choice it cannot
        // run (an external server's model) leaves the default serving here.
        if let Some(local) = &self.links.local_asr {
            let chosen = self.chosen_asr().map(|chosen| chosen.model);
            let model = match chosen.as_deref().filter(|model| local_catalog::find(model).is_some()) {
                Some(model) => model.to_string(),
                None => {
                    if let Some(other) = chosen {
                        tracing::warn!(
                            "asr: {other} runs only on a recognition server; this computer hears with {}",
                            local_catalog::DEFAULT_MODEL
                        );
                    }
                    local_catalog::DEFAULT_MODEL.to_string()
                }
            };
            local.choose(&model);
        }
        // The choice lives in the settings, the Mac's run script reads a file: for a server that takes a new model
        // only with its restart, the file follows the choice, or a restart by launchd brings the default model back.
        if let Some(chosen) = self.chosen_asr() {
            if self.servers.mac_restart() {
                let servers = self.servers.clone();
                tokio::spawn(async move { sync_asr_file(&servers, &chosen.model).await });
            }
        }
        Ok(())
    }

    /// The voice catalog: the showcase rows plus the open registry, from the database. The first run has no
    /// registry rows and fetches HuggingFace once (the panel's first open waits it out); later the database serves
    /// the list at once, and the registry is re-read in the background every few hours.
    pub async fn voice_models(self: &Arc<Self>) -> Result<Arc<Vec<VoiceModelRecord>>, ApiError> {
        let first = {
            let mut models = lock(&self.models);
            if models.is_none() {
                let listed = self.catalog.list_voice_models()?;
                let without_registry = !listed.iter().any(|model| model.source == "registry");
                *models = Some(Arc::new(listed));
                if without_registry {
                    *lock(&self.first_refresh) = Some(self.refresh_registry());
                }
            }
            lock(&self.first_refresh).clone()
        };
        // Whoever asks while the first fetch runs waits for it; once it is over this is at once.
        if let Some(first) = first {
            first.await;
        }
        let stale = lock(&self.registry_at).is_none_or(|at| at.elapsed() > REGISTRY_REFRESH);
        if stale {
            // In the background: the database serves meanwhile.
            drop(self.refresh_registry());
        }
        Ok(lock(&self.models).clone().unwrap_or_default())
    }

    /// The catalog with reality laid over it: what the servers loaded, what is installed, what downloads.
    pub async fn view(self: &Arc<Self>) -> Result<ModelsView, ApiError> {
        let own_voices = async {
            match &self.links.own_voices {
                Some(lab) => (lab.voices_for_cards().await, lab.engines().await),
                None => (Vec::new(), Vec::new()),
            }
        };
        let (asr_status, tts_status, models, own_voices) = tokio::join!(
            self.servers.asr_status(HEALTH),
            self.servers.tts_status(HEALTH),
            self.voice_models(),
            own_voices
        );
        let models = models?;
        let (own_voices, engines) = own_voices;
        let job = self.links.own_voices.as_ref().and_then(|lab| lab.job());
        let downloads: Vec<DownloadState> = lock(&self.downloads).iter().map(|download| download.state()).collect();
        let asr_cards = self.catalog.list_asr()?;
        let local_asr = self.links.local_asr.as_ref().map(|local| local.status());
        let asr_loaded = match &local_asr {
            Some(local) => local.serving.clone(),
            None => asr_status.as_ref().and_then(|status| status.model.clone()),
        };
        let tts_loaded = tts_status.as_ref().map(|status| status.loaded.clone());
        let yandex_allowed = self.yandex_allowed();
        let yandex_voice = self.whose(true) == Whose::Yandex;
        let choice = lock(&self.choice).clone();
        let own = if self.whose(yandex_allowed) == Whose::Own { choice.clone() } else { None };
        let card = |record: &VoiceModelRecord| {
            let installed = self.files.installed(&record.id, record.file.as_deref());
            let downloadable = self.files.allows(record.url.as_deref());
            let (state, progress, reason, needs) = file_state(record, installed, downloadable, &downloads);
            TtsCard {
                id: record.id.clone(),
                engine: record.engine.clone(),
                group: None,
                source: Some(record.source.clone()),
                own: None,
                title: record.title.clone(),
                description: record.description.clone(),
                badge: record.badge.clone(),
                license: record.license.clone(),
                locale: record.locale.clone(),
                quality: record.quality.clone(),
                size: record.size.clone(),
                url: record.url.clone(),
                file: record.file.clone(),
                voice: record.voices.first().map(|voice| voice.short.clone().unwrap_or_else(|| voice.name.clone())),
                voices: record
                    .voices
                    .iter()
                    .map(|voice| CardVoice {
                        id: format!("{}/{}", record.id, voice.name),
                        name: voice.short.clone().unwrap_or_else(|| voice.name.clone()),
                    })
                    .collect(),
                installed,
                loaded: installed && is_loaded(record, tts_loaded.as_ref()),
                active: own.as_ref().is_some_and(|own| own.model_id == record.id),
                downloadable,
                state,
                progress,
                reason,
                needs,
            }
        };
        // The owner's own voices right after the recommended ones: a chosen one starts its server, so «loaded» is
        // «its engine is installed» — the card can always be chosen.
        let own_cards: Vec<TtsCard> = own_voices
            .iter()
            .map(|voice| {
                let id = format!("{OWN_PREFIX}{}", voice.id);
                let (state, progress, reason, needs) = engine_state(voice, &engines, job.as_ref());
                let character = voice.origin.as_deref() == Some("package");
                TtsCard {
                    id: id.clone(),
                    engine: voice.engine.clone(),
                    group: Some(if character { "character" } else { "own" }.into()),
                    source: None,
                    own: Some(OwnDetails {
                        id: voice.id.clone(),
                        created_at: voice.created_at.clone(),
                        ref_seconds: voice.ref_seconds,
                        samples: voice.samples,
                        engine_title: voice.engine_title.clone(),
                        serving: voice.serving,
                    }),
                    title: voice.name.clone(),
                    description: if character {
                        crate::tr!("Голос персонажа · {engine}", engine = voice.engine_title)
                    } else {
                        crate::tr!(
                            "Ваш голос · {engine} · отпечаток от {date}",
                            engine = voice.engine_title,
                            date = day_and_month(&voice.created_at)
                        )
                    },
                    badge: Some(if character { crate::tr!("персонаж") } else { crate::tr!("свой") }),
                    license: Some(voice.engine_license.clone()),
                    locale: None,
                    quality: None,
                    size: None,
                    url: None,
                    file: None,
                    voice: Some(voice.name.clone()),
                    voices: vec![CardVoice { id: id.clone(), name: voice.name.clone() }],
                    installed: voice.ready,
                    loaded: voice.ready,
                    active: own.as_ref().is_some_and(|own| own.model_id == id),
                    downloadable: false,
                    state,
                    progress,
                    reason,
                    needs,
                }
            })
            .collect();
        let own_chosen =
            choice.as_ref().and_then(|choice| choice.model_id.strip_prefix(OWN_PREFIX)).filter(|id| !id.is_empty());
        let mut registry: Vec<&VoiceModelRecord> = models.iter().filter(|model| model.source == "registry").collect();
        registry.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| locale_compare(&a.title, &b.title)));
        // Alice's voice comes through a station: while the household talks by the computer's microphone (src/talk) its
        // card is not sent.
        let mut tts = Vec::new();
        if !crate::talk::input::on_computer(&self.settings) {
            tts.push(yandex_card(yandex_voice));
        }
        tts.extend(models.iter().filter(|model| model.source == "builtin").map(&card));
        tts.extend(own_cards);
        tts.extend(registry.into_iter().map(&card));
        let asr_chosen = self.chosen_asr();
        Ok(ModelsView {
            asr: asr_cards
                .into_iter()
                .map(|entry| AsrCard {
                    recommended: SHOWCASE
                        .asr
                        .iter()
                        .find(|card| card.id == entry.id)
                        .is_some_and(|card| card.recommended),
                    active: asr_loaded.as_deref() == Some(entry.model.as_str()),
                    pending: asr_chosen.as_ref().is_some_and(|chosen| chosen.model == entry.model)
                        && asr_loaded.as_deref() != Some(entry.model.as_str()),
                    local: local_asr.as_ref().map(|local| {
                        let gguf = local_catalog::find(&entry.model);
                        LocalCard {
                            runs_here: gguf.is_some(),
                            size: gguf.map_or(0, |gguf| gguf.size),
                            installed: local.installed.contains(&entry.model),
                        }
                    }),
                    record: entry,
                })
                .collect(),
            tts,
            tts_active: if yandex_voice && yandex_allowed {
                VoiceIds { model_id: "yandex".into(), voice_id: String::new() }
            } else {
                match &own {
                    Some(own) => own.ids(),
                    None => VoiceIds { model_id: String::new(), voice_id: String::new() },
                }
            },
            voice: self.whose(true),
            speaking: self.whose(yandex_allowed),
            mode: self.mode(),
            asr_loaded,
            // Recognition on this computer and the app's own voice server restart from here everywhere.
            can_restart: local_asr.is_some()
                || self.links.managed_tts.is_some()
                || self.servers.can_restart(asr_status.as_ref(), tts_status.as_ref()),
            local_asr,
            tts_managed: self.links.managed_tts.as_ref().map(|tts| tts.status()),
            // An own voice speaks through its own server: that is the one that must answer.
            tts_up: match own_chosen {
                Some(own_chosen) => {
                    own_voices.iter().find(|voice| voice.id == own_chosen).is_some_and(|voice| voice.serving)
                }
                None => tts_status.is_some(),
            },
            tts_pending: self.pending_view(job.as_ref(), &downloads),
            downloads,
            own_voice_server: self.has_own_speech(),
            restarting: lock(&self.restarting).as_ref().map(|restarting| restarting.which),
        })
    }

    /// Downloads a voice — showcase or registry — into tts-server/models/<id>/. A second click joins the first.
    pub async fn install_tts(self: &Arc<Self>, id: &str, actor: Option<Actor>) -> Result<DownloadState, ApiError> {
        let record = self.voice_model(id).await?;
        let Some((record, file)) = record.and_then(|record| {
            let file = record.file.clone().filter(|file| safe_name(file) && safe_name(&record.id))?;
            Some((record, file))
        }) else {
            return Err(ApiError::not_found(crate::tr!("нет скачиваемого голоса {id}", id = id)));
        };
        if !self.files.allows(record.url.as_deref()) {
            return Err(ApiError::bad_request(by_hand()));
        }
        let url = record.url.clone().unwrap_or_default();
        let download = {
            let mut downloads = lock(&self.downloads);
            if let Some(running) =
                downloads.iter().find(|download| download.id == id && download.phase() == DownloadPhase::Downloading)
            {
                return Ok(running.state());
            }
            if record.engine == SILERO {
                let mut silero = lock(&self.silero_install);
                if silero.as_deref().is_some_and(|other| other != id) {
                    return Err(ApiError::conflict(crate::tr!("Уже скачивается другая модель Silero — дождитесь её.")));
                }
                *silero = Some(id.to_string());
            }
            let download = Arc::new(Download::new(id));
            // A new download of the same voice takes its place in the list, as a Map's set did.
            match downloads.iter_mut().find(|entry| entry.id == id) {
                Some(entry) => *entry = download.clone(),
                None => downloads.push(download.clone()),
            }
            download
        };
        let this = self.clone();
        let running = download.clone();
        tokio::spawn(async move {
            let outcome = match this.files.download(&record.id, &file, &url, &running.progress, &running.cancel).await {
                Ok(()) => {
                    running.set(DownloadPhase::Done, None);
                    match this.after_install(&record).await {
                        Ok(()) => {
                            let heard = is_loaded(&record, this.reread_voices().await.as_ref());
                            // Done: the card's `installed` says it now; a 'done' kept here would outlive a later delete.
                            this.forget_download(&running);
                            this.events.emit(GatewayEvent::ModelInstalled {
                                kind: "tts".into(),
                                id: record.id.clone(),
                                actor,
                            });
                            tracing::info!(
                                "voice {} downloaded ({} МБ) — {}",
                                record.id,
                                (running.progress.total() as f64 / 1_048_576.0).round(),
                                if heard { "the voice server has it" } else { "restart the voice server to hear it" }
                            );
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                }
                Err(error) => Err(error),
            };
            if let Err(error) = outcome {
                if running.cancel.is_cancelled() {
                    running.set(DownloadPhase::Canceled, None);
                    tracing::info!("voice {}: download canceled", record.id);
                } else {
                    tracing::warn!("voice {}: {error}", record.id);
                    running.set(DownloadPhase::Error, Some(error));
                }
                // A voice waiting for this file is told it will not come.
                this.look_at_pending();
            }
            let mut silero = lock(&this.silero_install);
            if silero.as_deref() == Some(running.id.as_str()) {
                *silero = None;
            }
        });
        Ok(download.state())
    }

    /// Stops a running download; its half file leaves with it.
    pub fn cancel_tts(&self, id: &str) {
        let downloads = lock(&self.downloads);
        for download in downloads.iter().filter(|download| download.id == id) {
            download.cancel.cancel();
        }
    }

    /// Deletes a downloaded voice — not the one speaking, not one downloading. The voice server forgets it at once
    /// when it can read its folder again (an older one, after its restart).
    pub async fn uninstall_tts(self: &Arc<Self>, id: &str, actor: Option<Actor>) -> Result<(), ApiError> {
        let record = self.voice_model(id).await?;
        let Some((record, file)) = record.and_then(|record| {
            let file = record.file.clone()?;
            Some((record, file))
        }) else {
            return Err(ApiError::not_found(crate::tr!("нет скачиваемого голоса {id}", id = id)));
        };
        if lock(&self.downloads)
            .iter()
            .any(|download| download.id == id && download.phase() == DownloadPhase::Downloading)
        {
            return Err(ApiError::conflict(crate::tr!("Голос скачивается — сначала отмените скачивание.")));
        }
        if !self.files.installed(&record.id, Some(&file)) {
            return Err(ApiError::not_found(crate::tr!("Этот голос не скачан.")));
        }
        if self.speaks(&record) {
            return Err(ApiError::conflict(crate::tr!("Этот голос говорит сейчас — сначала выберите другой.")));
        }
        self.files.remove(&record.id, &file).map_err(ApiError::internal)?;
        lock(&self.downloads).retain(|download| download.id != id);
        self.events.emit(GatewayEvent::ModelRemoved { kind: "tts".into(), id: record.id.clone(), actor });
        let forgotten = self.reread_voices().await.is_some();
        tracing::info!(
            "voice {} deleted{}",
            record.id,
            if forgotten { "" } else { "; the voice server forgets it after its restart" }
        );
        Ok(())
    }

    /// A Silero model file, placed by hand: it is not downloadable from a catalog (the models are given out on
    /// silero.ai under CC BY-NC), so the panel sends the file itself, into its card's folder. Only the files the
    /// showcase knows (v4_ru.pt, v5_ru.pt): what is on disk is always a card of the catalog.
    pub async fn place_silero<S, E>(
        self: &Arc<Self>,
        name: Option<&str>,
        body: S,
        actor: Option<Actor>,
    ) -> Result<(), ApiError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: std::fmt::Display,
    {
        let card = SHOWCASE
            .tts
            .iter()
            .find(|entry| entry.engine == SILERO && entry.file.is_some() && entry.file.as_deref() == name)
            .and_then(|entry| Some((entry.id.clone(), entry.file.clone()?)));
        let Some((card_id, card_file)) = card else {
            let known: Vec<&str> = SHOWCASE
                .tts
                .iter()
                .filter(|entry| entry.engine == SILERO)
                .map(|entry| entry.file.as_deref().unwrap_or(""))
                .collect();
            return Err(ApiError::bad_request(crate::tr!(
                "Ожидается файл Silero из каталога: {files}.",
                files = known.join(", ")
            )));
        };
        let placed = {
            {
                let mut silero = lock(&self.silero_install);
                if silero.is_some() {
                    return Err(ApiError::conflict(crate::tr!("Модель Silero уже скачивается — дождитесь её.")));
                }
                *silero = Some(card_id.clone());
            }
            let _slot = SileroSlot(&self.silero_install);
            async {
                self.files.place(&card_id, &card_file, body).await?;
                if let Some(record) = self.voice_model(&card_id).await.map_err(|error| error.message)? {
                    self.after_install(&record).await?;
                }
                Ok::<(), String>(())
            }
            .await
        };
        if let Err(error) = placed {
            return Err(ApiError::bad_request(crate::tr!("Файл не принят: {error}", error = error)));
        }
        self.events.emit(GatewayEvent::ModelInstalled { kind: "tts".into(), id: card_id, actor });
        let heard =
            self.reread_voices().await.and_then(|loaded| loaded.silero_file).as_deref() == Some(card_file.as_str());
        tracing::info!(
            "silero model file placed: {card_file} — {}",
            if heard { "the voice server has its voices" } else { "restart the voice server for its voices" }
        );
        Ok(())
    }

    /// The synthesis choice from the next request on, never pending (the older callers): see [`ModelsService::choose`].
    pub async fn activate_tts(self: &Arc<Self>, body: &Value, actor: Option<Actor>) -> Result<Activated, ApiError> {
        let request = VoiceRequest { install: false, ..VoiceRequest::of(body) };
        match self.choose(request, actor).await? {
            Chosen::Active(activated) => Ok(activated),
            Chosen::Pending(_) => Err(ApiError::internal("a choice without install came out pending")),
        }
    }

    /// The synthesis choice from the next request on: a voice of ours by catalog ids (the older {model, voice} pair
    /// still resolves), or `yandex` — Alice's own voice, as the station came. One the server has not loaded yet asks
    /// for its restart. A voice whose engine or file is not here answers 409 with what it `needs` (in the error's
    /// `errors`); with `install` that is installed or downloaded and the choice waits for it (setting voice_pending)
    /// — then it is switched on by itself. Any choice that succeeds forgets a pending one.
    pub async fn choose(self: &Arc<Self>, request: VoiceRequest, actor: Option<Actor>) -> Result<Chosen, ApiError> {
        let body = &request.body;
        let voice = self.links.voice.clone().ok_or_else(|| {
            ApiError::unavailable(crate::tr!("Синтез речи ещё не подключён к этому шлюзу — голос не переключить."))
        })?;
        if body.get("modelId").and_then(Value::as_str) == Some("yandex") {
            // Alice speaks: the app's own voice server gives its memory back.
            if let Some(tts) = &self.links.managed_tts {
                tts.want(false);
            }
            voice.use_yandex();
            self.settings.set(&VOICE_PROVIDER, &TtsProvider::Yandex.as_str().to_string(), actor.clone())?;
            self.clear_pending(actor);
            tracing::info!("tts: Alice (Yandex) speaks now");
            return Ok(Chosen::Active(Activated { needs_restart: false }));
        }
        if !voice.has_own_speech() {
            return Err(ApiError::bad_request(crate::tr!("Свой голосовой сервер не настроен (TTS_OWN_URL).")));
        }
        if let Some(own) = own_id(body.get("modelId")) {
            if let Some(pending) = self.own_needs(&own, &request, actor.clone()).await? {
                return Ok(Chosen::Pending(pending));
            }
            // An own voice: its engine's server answering first (started under the memory rules: tight on memory,
            // only with the owner's «всё равно»), then the route.
            let lab =
                self.links.own_voices.clone().ok_or_else(|| ApiError::not_found(crate::tr!("нет такого голоса")))?;
            let prepared = lab.prepare(own.clone(), request.anyway).await?;
            let model_id = format!("{OWN_PREFIX}{own}");
            let choice = Choice {
                model_id: model_id.clone(),
                voice_id: model_id,
                engine: prepared.engine.clone(),
                voice: own.clone(),
                route: Some(prepared.route.clone()),
            };
            let ids = choice.ids();
            *lock(&self.choice) = Some(choice);
            voice.use_own(
                VoicePair { model: prepared.engine.clone(), voice: own.clone() },
                ids.clone(),
                Some(prepared.route),
            )?;
            self.settings.set(&VOICE_CHOICE, &stored_ids(&ids), actor.clone())?;
            self.settings.set(&VOICE_PROVIDER, &voice.chosen_provider().as_str().to_string(), actor.clone())?;
            self.clear_pending(actor);
            if let Some(tts) = &self.links.managed_tts {
                tts.want(false);
            }
            tracing::info!("tts: own voice {own} («{}», {}) speaks now", prepared.name, prepared.engine);
            return Ok(Chosen::Active(Activated { needs_restart: false }));
        }
        let by_id = match body.get("modelId").and_then(Value::as_str) {
            Some(model_id) => self.choice_by_id(model_id, body.get("voiceId").and_then(Value::as_str)).await?,
            None => None,
        };
        let choice = match by_id {
            Some(choice) => Some(choice),
            None => {
                self.resolve_choice(&json!({"model": body.get("model"), "voice": body.get("voice")}), false).await?
            }
        };
        let Some(choice) = choice else {
            return Err(ApiError::not_found(crate::tr!("нет такого голоса")));
        };
        let record = self
            .voice_model(&choice.model_id)
            .await?
            .ok_or_else(|| ApiError::not_found(crate::tr!("нет такого голоса")))?;
        // The app's own voice server: its Python first — a voice's need like a voice lab engine. With `install`
        // it is installed in the background and the voice waits for it (then its file, then it speaks).
        if let Some(tts) = &self.links.managed_tts {
            if !tts.installed() {
                let (download_mb, disk_mb) = ManagedTts::sizes();
                let needs = VoiceNeeds {
                    kind: "engine",
                    id: RUNTIME_ID.into(),
                    title: crate::locale::text::translate(RUNTIME_TITLE, &[]).to_string(),
                    download_mb: Some(download_mb),
                    disk_mb: Some(disk_mb),
                    minutes: Some(minutes_for(download_mb)),
                    alternatives: Vec::new(),
                };
                if !request.install {
                    return Err(needed(&needs, tts.installing()));
                }
                tts.install();
                let wait = PendingWait { kind: "engine".into(), id: RUNTIME_ID.into() };
                let pending = self.set_pending(
                    &choice.model_id,
                    Some(choice.voice_id.clone()),
                    wait,
                    request.anyway,
                    None,
                    actor,
                )?;
                return Ok(Chosen::Pending(pending));
            }
            tts.want(true);
        }
        if !self.files.installed(&record.id, record.file.as_deref()) {
            return self.file_needed(&record, &choice, &request, actor).await.map(Chosen::Pending);
        }
        let ids = choice.ids();
        let pair = VoicePair { model: choice.engine.clone(), voice: choice.voice.clone() };
        *lock(&self.choice) = Some(choice);
        voice.use_own(pair, ids.clone(), None)?;
        self.settings.set(&VOICE_CHOICE, &stored_ids(&ids), actor.clone())?;
        self.settings.set(&VOICE_PROVIDER, &voice.chosen_provider().as_str().to_string(), actor.clone())?;
        self.clear_pending(actor);
        tracing::info!("tts: {}/{} speaks now", ids.model_id, ids.voice_id);
        // No own voice speaks any more: its server gives its memory back.
        if let Some(lab) = &self.links.own_voices {
            lab.release();
        }
        if is_loaded(&record, self.servers.tts_loaded().await.as_ref()) {
            return Ok(Chosen::Active(Activated { needs_restart: false }));
        }
        // On disk, not in the server (put there by hand, or the rescan after its download failed): it reads its
        // folder again rather than restart; an older server still needs the restart.
        Ok(Chosen::Active(Activated { needs_restart: !is_loaded(&record, self.reread_voices().await.as_ref()) }))
    }

    /// An own voice whose engine is not here, or a character's voice not imprinted yet: 409 with what it needs, or —
    /// with `install` — the engine's install or the imprint started and the choice pending. None: it can speak now.
    async fn own_needs(
        self: &Arc<Self>,
        own: &str,
        request: &VoiceRequest,
        actor: Option<Actor>,
    ) -> Result<Option<PendingView>, ApiError> {
        let Some(lab) = self.links.own_voices.clone() else { return Ok(None) };
        let exists = lab.voice_engine(own);
        let recipe = request.recipe.clone().filter(|recipe| recipe.voice == own);
        let Some(engine_id) = exists.clone().or_else(|| recipe.as_ref().map(|recipe| recipe.engine.clone())) else {
            return Err(ApiError::not_found(crate::tr!("нет такого голоса")));
        };
        let engines = lab.engines().await;
        let Some(engine) = engines.iter().find(|engine| engine.id == engine_id) else {
            // The voice lab says nothing of its engines (a test): the voice's own preparation decides.
            return Ok(None);
        };
        let voices = lab.voices_for_cards().await;
        let reimprint = voices.iter().any(|voice| voice.id == own && voice.reimprint);
        let alternatives: Vec<Alternative> = engines
            .iter()
            .filter(|other| reimprint && other.id != engine.id && other.status == "ready")
            .map(|other| Alternative { kind: "reimprint", engine: other.id.clone() })
            .collect();
        let model_id = format!("{OWN_PREFIX}{own}");
        match engine.status.as_str() {
            "unsupported" => {
                let reason = engine
                    .reason
                    .clone()
                    .unwrap_or_else(|| crate::tr!("{title} здесь не работает.", title = engine.title));
                Err(ApiError {
                    status: axum::http::StatusCode::CONFLICT,
                    message: reason.clone(),
                    errors: Some(json!({ "state": "unsupported", "reason": reason })),
                })
            }
            "ready" if exists.is_some() => Ok(None),
            "ready" => {
                // A character's voice its package has no snapshot for: imprinted from its reference first.
                let recipe = recipe.ok_or_else(|| ApiError::not_found(crate::tr!("нет такого голоса")))?;
                lab.imprint(recipe.clone(), request.anyway, actor.clone()).await?;
                let wait = PendingWait { kind: "imprint".into(), id: own.to_string() };
                Ok(Some(self.set_pending(&model_id, None, wait, request.anyway, Some(recipe), actor)?))
            }
            status => {
                let needs = engine_needs(engine, alternatives);
                if !request.install {
                    return Err(needed(&needs, status == "installing"));
                }
                if status != "installing" {
                    lab.install_engine(engine.id.clone(), actor.clone()).await?;
                }
                let wait = PendingWait { kind: "engine".into(), id: engine.id.clone() };
                Ok(Some(self.set_pending(&model_id, None, wait, request.anyway, recipe, actor)?))
            }
        }
    }

    /// A catalog voice whose file is not here: 409 with what it needs, or — with `install` — its download started
    /// (or joined) and the choice pending.
    async fn file_needed(
        self: &Arc<Self>,
        record: &VoiceModelRecord,
        choice: &Choice,
        request: &VoiceRequest,
        actor: Option<Actor>,
    ) -> Result<PendingView, ApiError> {
        let needs = file_needs(record);
        if !self.files.allows(record.url.as_deref()) {
            return Err(ApiError {
                status: axum::http::StatusCode::CONFLICT,
                message: by_hand(),
                errors: Some(json!({ "state": "needs", "needs": needs, "reason": by_hand() })),
            });
        }
        let downloading = lock(&self.downloads)
            .iter()
            .any(|download| download.id == record.id && download.phase() == DownloadPhase::Downloading);
        if !request.install {
            return Err(needed(&needs, downloading));
        }
        if !downloading {
            self.install_tts(&record.id, actor.clone()).await?;
        }
        let wait = PendingWait { kind: "file".into(), id: record.id.clone() };
        self.set_pending(&choice.model_id, Some(choice.voice_id.clone()), wait, request.anyway, None, actor)
    }

    /// The pending voice: kept in the settings, told to the panel.
    fn set_pending(
        &self,
        model_id: &str,
        voice_id: Option<String>,
        wait_for: PendingWait,
        anyway: bool,
        recipe: Option<VoiceRecipe>,
        actor: Option<Actor>,
    ) -> Result<PendingView, ApiError> {
        let pending = VoicePending {
            model_id: model_id.to_string(),
            voice_id,
            wait_for,
            anyway,
            state: "waiting".into(),
            reason: None,
            recipe,
            since: now_iso(),
        };
        self.settings.set(&VOICE_PENDING, &pending, actor)?;
        tracing::info!("tts: {} waits for its {} {}", pending.model_id, pending.wait_for.kind, pending.wait_for.id);
        Ok(pending_view(&pending, None))
    }

    /// The card of the voice waiting for its engine or file, if one waits.
    pub fn pending_card(&self) -> Option<String> {
        self.settings.get(&VOICE_PENDING, None).map(|pending| pending.model_id)
    }

    /// Forgets the pending voice: another was chosen, or the owner said no (`DELETE /api/v1/models/tts/pending`).
    pub fn clear_pending(&self, actor: Option<Actor>) {
        if self.settings.get(&VOICE_PENDING, None).is_none() {
            return;
        }
        if let Err(error) = self.settings.forget(&VOICE_PENDING) {
            tracing::warn!("tts: the pending voice is not forgotten: {}", error.message);
            return;
        }
        self.events.emit(GatewayEvent::SettingsChanged { key: VOICE_PENDING.key.into(), user_id: None, actor });
    }

    /// The pending voice as the panel sees it, with the progress of what it waits for.
    fn pending_view(&self, job: Option<&LabJob>, downloads: &[DownloadState]) -> Option<PendingView> {
        let pending = self.settings.get(&VOICE_PENDING, None)?;
        let progress = match pending.wait_for.kind.as_str() {
            "file" => downloads
                .iter()
                .find(|download| download.id == pending.wait_for.id && download.state == DownloadPhase::Downloading)
                .and_then(|download| (download.bytes * 100).checked_div(download.total).map(|p| p.min(100) as u32)),
            "engine" if pending.wait_for.id == RUNTIME_ID => self
                .links
                .managed_tts
                .as_ref()
                .map(|tts| tts.status())
                .filter(|status| status.phase == crate::speech::tts::managed::TtsPhase::Installing)
                .map(|status| status.percent),
            "engine" => job
                .filter(|job| job.running && job.kind == "install" && job.engine == pending.wait_for.id)
                .map(|job| job.percent),
            _ => job.filter(|job| job.running && job.kind == "imprint").map(|job| job.percent),
        };
        Some(pending_view(&pending, progress))
    }

    /// The pending voice, looked at again after an install, a download or an imprint ended: what it waited for is
    /// here — switched on (a character's voice imprinted from its reference first); tight on memory — it waits for
    /// the owner's «всё равно» (`tight`); what it waited for failed — `failed` with why.
    pub async fn try_pending(self: &Arc<Self>) {
        let _turn = self.pending_turn.lock().await;
        let Some(mut pending) = self.settings.get(&VOICE_PENDING, None) else { return };
        if pending.state != "waiting" {
            return;
        }
        let lab = self.links.own_voices.clone();
        let job = lab.as_ref().and_then(|lab| lab.job());
        let running = job.as_ref().is_some_and(|job| job.running);
        let failed_job = |kind: &str, engine: Option<&str>| {
            job.as_ref()
                .filter(|job| !job.running && job.kind == kind && engine.is_none_or(|engine| job.engine == engine))
                .and_then(|job| job.failed.clone())
        };
        let own = own_id(Some(&Value::String(pending.model_id.clone())));
        match pending.wait_for.kind.as_str() {
            // The app's own voice server's environment: installed — the voice goes on (its file next); failed —
            // the voice fails with the reason.
            "engine" if pending.wait_for.id == RUNTIME_ID => {
                let Some(tts) = &self.links.managed_tts else { return };
                if tts.installing() {
                    return;
                }
                if !tts.installed() {
                    let why = tts.status().error.unwrap_or_else(|| crate::tr!("установка голоса прервалась"));
                    self.pending_failed(pending, why);
                    return;
                }
            }
            "engine" => {
                let Some(lab) = &lab else { return };
                let engines = lab.engines().await;
                let status = engines.iter().find(|engine| engine.id == pending.wait_for.id).map(|e| e.status.clone());
                match status.as_deref() {
                    Some("ready") => {
                        let missing = own.as_deref().is_some_and(|own| lab.voice_engine(own).is_none());
                        if let (true, Some(recipe)) = (missing, pending.recipe.clone()) {
                            // The engine's job may still be ending: the next event tries again.
                            if running {
                                return;
                            }
                            match lab.imprint(recipe, pending.anyway, None).await {
                                Ok(()) => {
                                    pending.wait_for =
                                        PendingWait { kind: "imprint".into(), id: own.unwrap_or_default() };
                                    let _ = self.settings.set(&VOICE_PENDING, &pending, None);
                                }
                                Err(error) => self.pending_ended(pending, &error),
                            }
                            return;
                        }
                    }
                    Some("installing") => return,
                    _ => {
                        let why = failed_job("install", Some(&pending.wait_for.id))
                            .unwrap_or_else(|| crate::tr!("установка модели прервалась"));
                        self.pending_failed(pending, why);
                        return;
                    }
                }
            }
            "imprint" => {
                let made =
                    own.as_deref().is_some_and(|own| lab.as_ref().is_some_and(|lab| lab.voice_engine(own).is_some()));
                if !made {
                    if running {
                        return;
                    }
                    let why = failed_job("imprint", None).unwrap_or_else(|| crate::tr!("отпечаток не снялся"));
                    self.pending_failed(pending, why);
                    return;
                }
            }
            "file" => {
                let record = match self.voice_model(&pending.wait_for.id).await {
                    Ok(Some(record)) => record,
                    _ => return self.pending_failed(pending, crate::tr!("нет такого голоса")),
                };
                if !self.files.installed(&record.id, record.file.as_deref()) {
                    let download =
                        lock(&self.downloads).iter().find(|download| download.id == record.id).map(|d| d.state());
                    match download {
                        Some(download) if download.state == DownloadPhase::Downloading => return,
                        Some(download) => {
                            let why = download.error.unwrap_or_else(|| crate::tr!("скачивание остановлено"));
                            self.pending_failed(pending, why);
                        }
                        None => self.pending_failed(pending, crate::tr!("скачивание прервалось")),
                    }
                    return;
                }
            }
            _ => return,
        }
        let mut body = json!({ "modelId": pending.model_id });
        if let Some(voice_id) = &pending.voice_id {
            body["voiceId"] = json!(voice_id);
        }
        // After the app's own voice server's environment the voice's file may still be missing: the choice goes on
        // with its download (and waits for it) instead of failing for want of it.
        let next_step = pending.wait_for.kind == "engine" && pending.wait_for.id == RUNTIME_ID;
        let request = VoiceRequest { body, install: next_step, anyway: pending.anyway, recipe: None };
        match self.choose(request, None).await {
            Ok(Chosen::Active(_)) => tracing::info!("tts: the pending {} speaks now", pending.model_id),
            Ok(Chosen::Pending(next)) => {
                tracing::info!("tts: the pending {} goes on: it waits for its {}", pending.model_id, next.wait_for.kind)
            }
            Err(error) => self.pending_ended(pending, &error),
        }
    }

    /// [`ModelsService::try_pending`] in the background. Boxed: the look may choose a voice, and a choice may start
    /// the download whose end looks again.
    fn look_at_pending(self: &Arc<Self>) {
        let shop = self.clone();
        let look: BoxFuture<'static, ()> = Box::pin(async move { shop.try_pending().await });
        tokio::spawn(look);
    }

    /// A pending voice that could not be switched on: tight memory waits for the owner, anything else failed.
    fn pending_ended(&self, mut pending: VoicePending, error: &ApiError) {
        if error.status == axum::http::StatusCode::PRECONDITION_REQUIRED {
            pending.state = "tight".into();
            pending.reason = Some(error.message.clone());
            tracing::info!("tts: the pending {} waits for «всё равно»: {}", pending.model_id, error.message);
            let _ = self.settings.set(&VOICE_PENDING, &pending, None);
        } else {
            self.pending_failed(pending, error.message.clone());
        }
    }

    fn pending_failed(&self, mut pending: VoicePending, why: String) {
        tracing::warn!("tts: the pending {} is not switched on: {why}", pending.model_id);
        pending.state = "failed".into();
        pending.reason = Some(why);
        let _ = self.settings.set(&VOICE_PENDING, &pending, None);
    }

    /// A catalog voice by its card id: its title and whether its file is here (a character's fallback voice).
    /// From the catalog in the database as it is: asking never waits for the open registry.
    pub fn voice_card(&self, id: &str) -> Option<(String, bool)> {
        let cached = lock(&self.models).clone();
        let record = match cached {
            Some(models) => models.iter().find(|record| record.id == id).cloned(),
            None => self.catalog.list_voice_models().ok()?.into_iter().find(|record| record.id == id),
        }?;
        let installed = self.files.installed(&record.id, record.file.as_deref());
        Some((record.title, installed))
    }

    /// Notes the recognition model by its card's id (the model string still resolves); it serves after the server
    /// loads it («перезапустить»: over HTTP, or the Mac's restart).
    pub async fn activate_asr(&self, id_or_model: &str, actor: Option<Actor>) -> Result<Activated, ApiError> {
        let card =
            self.catalog.list_asr()?.into_iter().find(|entry| entry.id == id_or_model || entry.model == id_or_model);
        let Some(card) = card else {
            return Err(ApiError::not_found(format!("no model {id_or_model}")));
        };
        if !safe_name(&card.model) {
            return Err(ApiError::bad_request(format!("not a model name: {}", card.model)));
        }
        // On this computer: the model is fetched if it is not here and takes over once it is in — no restart.
        if let Some(local) = &self.links.local_asr {
            if local_catalog::find(&card.model).is_none() {
                return Err(ApiError::bad_request(crate::tr!(
                    "Эта модель работает только на отдельном сервере распознавания — на этом компьютере её нет."
                )));
            }
            *lock(&self.asr_chosen) = Some(card.clone());
            self.settings.set(&ASR_MODEL, &card.id, actor)?;
            local.choose(&card.model);
            return Ok(Activated { needs_restart: false });
        }
        *lock(&self.asr_chosen) = Some(card.clone());
        self.settings.set(&ASR_MODEL, &card.id, actor)?;
        let status = self.servers.asr_status(HEALTH).await;
        // The Mac's run script file follows the choice at once only for a server that takes a model with its
        // restart; one that loads over HTTP gets it written when the model has loaded.
        if self.servers.mac_restart() && !status.as_ref().is_some_and(|status| status.can("load")) {
            self.servers.choose_asr(&card.model).map_err(ApiError::internal)?;
        }
        Ok(Activated { needs_restart: status.and_then(|status| status.model).as_deref() != Some(card.model.as_str()) })
    }

    /// Puts the chosen recognition model or the voices on disk to work: over HTTP when the server can, otherwise by
    /// the Mac's restart (macOS only; elsewhere it says it cannot). One at a time: a second click joins the running
    /// one. The recognition model is taken when the click comes, so a choice changed during a long load waits for
    /// the next click instead of slipping in unchecked. The restart runs on even if the asking request goes away.
    pub async fn restart_server(self: &Arc<Self>, which: ServerName, actor: Option<Actor>) -> Result<(), ApiError> {
        let done = {
            let mut restarting = lock(&self.restarting);
            if let Some(running) = &*restarting {
                if running.which != which {
                    return Err(ApiError::conflict(crate::tr!("Перезапускается другой сервер — дождитесь его.")));
                }
                running.done.clone()
            } else {
                let model = match which {
                    ServerName::Asr => self.chosen_asr().map(|chosen| chosen.model),
                    ServerName::Tts => None,
                };
                let this = self.clone();
                let task = tokio::spawn(async move {
                    let result = match (&this.links.local_asr, &this.links.managed_tts, which) {
                        // On this computer: a new worker, the failures forgotten, the GPU tried again.
                        (Some(local), _, ServerName::Asr) => {
                            local.restart().await;
                            Ok(())
                        }
                        // The app's own voice server: started anew (an install that failed runs again).
                        (_, Some(tts), ServerName::Tts) => {
                            tts.restart().await;
                            Ok(())
                        }
                        _ => this.servers.restart(which, model.as_deref()).await,
                    };
                    if result.is_ok() {
                        this.events.emit(GatewayEvent::ModelServerRestarted { which: which.as_str().into(), actor });
                        let what = match (&model, which) {
                            (Some(model), _) => format!("model {model} serves"),
                            (None, ServerName::Tts) => "the voices on disk are put to work".into(),
                            (None, ServerName::Asr) => "restarted".into(),
                        };
                        tracing::info!("{} server: {what}", which.as_str());
                    }
                    *lock(&this.restarting) = None;
                    result
                });
                let done = async move { task.await.unwrap_or_else(|error| Err(error.to_string())) }.boxed().shared();
                *restarting = Some(Restarting { which, done: done.clone() });
                done
            }
        };
        done.await.map_err(ApiError::bad_request)
    }

    /// The open registry into the catalog (upsert by id), and the fresh list served. One at a time; it runs to its
    /// end in the background whether anyone waits for it or not.
    fn refresh_registry(self: &Arc<Self>) -> Shared<BoxFuture<'static, ()>> {
        let mut refreshing = lock(&self.refreshing);
        if let Some(running) = &*refreshing {
            return running.clone();
        }
        *lock(&self.registry_at) = Some(Instant::now());
        let this = self.clone();
        let task = tokio::spawn(async move { this.read_registry().await });
        let done = async move {
            let _ = task.await;
        }
        .boxed()
        .shared();
        *refreshing = Some(done.clone());
        done
    }

    async fn read_registry(&self) {
        let builtin_files: HashSet<String> = self
            .catalog
            .list_voice_models()
            .map(|models| {
                models.into_iter().filter(|model| model.source == "builtin").filter_map(|model| model.file).collect()
            })
            .unwrap_or_default();
        let mut seen = HashSet::new();
        let mut records = Vec::new();
        let read = async {
            let mut url = Some(self.registry_url.clone());
            let mut page = 0;
            while let Some(current) = url.take() {
                if page >= REGISTRY_PAGES {
                    break;
                }
                page += 1;
                let response = crate::utils::http::client()
                    .get(&current)
                    .timeout(REGISTRY_PAGE_TIMEOUT)
                    .send()
                    .await
                    .map_err(|error| error.to_string())?;
                if !response.status().is_success() {
                    break;
                }
                let link = response
                    .headers()
                    .get("link")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                let entries: Vec<RegistryFile> = response.json().await.map_err(|error| error.to_string())?;
                for file in &entries {
                    records.extend(registry_record(file, &mut seen, &builtin_files));
                }
                url = next_page(&link).filter(|next| next.starts_with(&self.registry_origin));
            }
            Ok::<(), String>(())
        };
        match read.await {
            Ok(()) => match self.catalog.save_voice_models(&records) {
                Ok(()) => tracing::info!("piper registry: {} voices upserted into the catalog", records.len()),
                Err(error) => tracing::warn!("piper registry: not saved ({error}); serving what the database has"),
            },
            Err(error) => {
                tracing::warn!("piper registry refresh failed ({error}); serving what the database has");
            }
        }
        if let Ok(models) = self.catalog.list_voice_models() {
            *lock(&self.models) = Some(Arc::new(models));
        }
        *lock(&self.refreshing) = None;
    }

    /// After a voice lands: one Silero generation stays (the server loads the first .pt), and the choice follows it.
    /// A failure says its cause: the download's card and the upload's answer show it.
    async fn after_install(self: &Arc<Self>, record: &VoiceModelRecord) -> Result<(), String> {
        if record.engine != SILERO {
            return Ok(());
        }
        for other in &SHOWCASE.tts {
            if other.engine != SILERO || other.id == record.id {
                continue;
            }
            let Some(file) = &other.file else {
                continue;
            };
            if self.files.installed(&other.id, Some(file)) {
                self.files.remove(&other.id, file)?;
            }
        }
        let current = lock(&self.choice).clone();
        if let Some(current) = current.filter(|choice| choice.engine == SILERO && choice.model_id != record.id) {
            let moved = self
                .choice_by_id(&record.id, Some(&format!("{}/{}", record.id, current.voice)))
                .await
                .map_err(|error| error.message)?;
            if let Some(moved) = moved {
                let ids = moved.ids();
                *lock(&self.choice) = Some(moved);
                self.settings.set(&VOICE_CHOICE, &stored_ids(&ids), None).map_err(|error| error.message)?;
                tracing::info!("tts: the choice follows the new Silero model to {}", ids.voice_id);
            }
        }
        Ok(())
    }

    /// The voice server reads its folder again after a voice came or went: it speaks, or is forgotten, with no
    /// restart. What the server has loaded then; None when it cannot (an older one: its restart still applies).
    async fn reread_voices(&self) -> Option<TtsLoaded> {
        match self.servers.rescan_tts().await {
            Ok(loaded) => loaded,
            Err(error) => {
                tracing::warn!("voice server: {error}");
                None
            }
        }
    }

    /// A finished download leaves the list — that one, not a newer one of the same voice.
    fn forget_download(&self, download: &Arc<Download>) {
        lock(&self.downloads).retain(|entry| !Arc::ptr_eq(entry, download));
    }

    /// Whether a model is the one our voice speaks with now: deleting it would silence the answers.
    fn speaks(&self, record: &VoiceModelRecord) -> bool {
        if self.whose(self.yandex_allowed()) == Whose::Yandex {
            return false;
        }
        if lock(&self.choice).as_ref().is_some_and(|choice| choice.model_id == record.id) {
            return true;
        }
        let Some(pair) = self.links.voice.as_ref().and_then(|voice| voice.own_choice()) else {
            return false;
        };
        if record.engine == SILERO {
            return pair.model == SILERO;
        }
        record.voices.iter().any(|voice| voice.name == pair.voice)
    }

    /// A voice model by id, the catalog's truth for it.
    async fn voice_model(self: &Arc<Self>, id: &str) -> Result<Option<VoiceModelRecord>, ApiError> {
        Ok(self.voice_models().await?.iter().find(|record| record.id == id).cloned())
    }

    /// A choice by catalog ids; a Piper model's single voice needs no voiceId.
    async fn choice_by_id(
        self: &Arc<Self>,
        model_id: &str,
        voice_id: Option<&str>,
    ) -> Result<Option<Choice>, ApiError> {
        let Some(record) = self.voice_model(model_id).await? else {
            return Ok(None);
        };
        Ok(choice_in(&record, |voice| voice_id == Some(format!("{}/{}", record.id, voice.name).as_str())))
    }

    /// What a stored choice resolves to. New values are catalog ids; the older pair {model, voice} — written when
    /// the choice was names, and the env's TTS_OWN_MODEL/VOICE — resolves against the catalog by file, then by the
    /// voice's full or short name, so a choice made before keeps speaking.
    async fn resolve_choice(self: &Arc<Self>, value: &Value, restoring: bool) -> Result<Option<Choice>, ApiError> {
        if !value.is_object() {
            return Ok(None);
        }
        if let Some(own) = own_id(value.get("modelId")) {
            // At start the route comes at once and the voice's server in the background: the gateway does not wait
            // for a model.
            return Ok(if restoring { self.restored_own(&own) } else { None });
        }
        let models = self.voice_models().await?;
        Ok(choice_among(&models, value))
    }

    /// An own voice the settings keep, from the voice lab: its route at once, its server in the background.
    fn restored_own(&self, own: &str) -> Option<Choice> {
        let found = self.links.own_voices.as_ref()?.restore(own)?;
        Some(Choice {
            model_id: format!("{OWN_PREFIX}{own}"),
            voice_id: format!("{OWN_PREFIX}{own}"),
            engine: found.engine,
            voice: own.to_string(),
            route: Some(found.route),
        })
    }

    /// The stored choice (`{modelId, voiceId}`, or the older `{model, voice}`) as the catalog in the database and the
    /// voice lab name it, at once: what speech's voice restores when the gateway is built (speech's VoiceCatalog,
    /// src/links), before the shop's start has read the registry. None: no such voice.
    pub fn stored_voice(&self, stored: &Map<String, Value>) -> Option<(ChosenVoice, Option<OwnRoute>)> {
        let value = Value::Object(stored.clone());
        let choice = match own_id(value.get("modelId")) {
            Some(own) => self.restored_own(&own)?,
            None => choice_among(&self.catalog.list_voice_models().ok()?, &value)?,
        };
        let chosen = ChosenVoice {
            model_id: choice.model_id,
            voice_id: choice.voice_id,
            engine: choice.engine,
            voice: choice.voice,
        };
        Some((chosen, choice.route))
    }

    fn has_own_speech(&self) -> bool {
        self.links.voice.as_ref().is_some_and(|voice| voice.has_own_speech())
    }

    fn chosen_provider(&self) -> TtsProvider {
        self.links.voice.as_ref().map_or(self.env_provider, |voice| voice.chosen_provider())
    }

    /// Whose voice speaks. Without the voice linked in nothing of ours speaks: the configured provider is all there is.
    fn whose(&self, yandex_allowed: bool) -> Whose {
        match &self.links.voice {
            Some(voice) => voice.whose(yandex_allowed),
            None if self.env_provider == TtsProvider::Yandex && yandex_allowed => Whose::Yandex,
            None => Whose::Own,
        }
    }

    /// The station's mode: the panel's last switch (the mode keeps it in the `gateway_mode` setting), or the env's.
    fn mode(&self) -> PanelMode {
        self.settings.get(&MODE, None).map_or(self.env_mode, |stored| stored.mode)
    }

    /// «Свой» is the gateway with Yandex forbidden.
    fn yandex_allowed(&self) -> bool {
        self.mode() != PanelMode::Own
    }
}

/// What a stored value names among the catalog's voices: catalog ids, or the older pair {model, voice} by file, then
/// by the voice's full or short name. An own voice is not the catalog's.
fn choice_among(models: &[VoiceModelRecord], value: &Value) -> Option<Choice> {
    if let Some(model_id) = value.get("modelId").and_then(Value::as_str) {
        let voice_id = value.get("voiceId").and_then(Value::as_str);
        let record = models.iter().find(|record| record.id == model_id)?;
        return choice_in(record, |voice| voice_id == Some(format!("{}/{}", record.id, voice.name).as_str()));
    }
    let (Some(model), Some(wanted)) =
        (value.get("model").and_then(Value::as_str), value.get("voice").and_then(Value::as_str))
    else {
        return None;
    };
    let engine = if model == SILERO { SILERO } else { PIPER };
    let of_engine: Vec<&VoiceModelRecord> = models.iter().filter(|record| record.engine == engine).collect();
    let file = format!("{wanted}.onnx");
    let record = of_engine
        .iter()
        .find(|record| record.file.as_deref() == Some(file.as_str()))
        .or_else(|| of_engine.iter().find(|record| record.voices.iter().any(|voice| voice.name == wanted)))
        .or_else(|| {
            of_engine.iter().find(|record| record.voices.iter().any(|voice| voice.short.as_deref() == Some(wanted)))
        })?;
    choice_in(record, |voice| voice.name == wanted || voice.short.as_deref() == Some(wanted))
}

/// A choice of a model's voice: the one `wanted` picks, or its first.
fn choice_in(record: &VoiceModelRecord, wanted: impl Fn(&VoiceEntry) -> bool) -> Option<Choice> {
    let voice = record.voices.iter().find(|voice| wanted(voice)).or(record.voices.first())?;
    Some(Choice {
        model_id: record.id.clone(),
        voice_id: format!("{}/{}", record.id, voice.name),
        engine: record.engine.clone(),
        voice: voice.name.clone(),
        route: None,
    })
}

/// The run script's file after the choice — for a server that answers and cannot load a model over HTTP. One that
/// can keeps the file itself, with models that have loaded; about one that is down nothing is guessed: a model that
/// never loaded must not become what the Mac starts with.
async fn sync_asr_file(servers: &ModelServers, model: &str) {
    let Some(status) = servers.asr_status(HEALTH).await else {
        return;
    };
    if status.can("load") {
        return;
    }
    match servers.sync_asr(model) {
        Ok(true) => tracing::info!("asr: {model} noted for the run script"),
        Ok(false) => {}
        Err(error) => tracing::warn!("asr: the chosen model is not noted for the run script: {error}"),
    }
}

/// Whether the voice server loaded a model: Silero by its file; Piper by its file's stem when the server names them
/// (`piperFiles`) — speakers repeat across qualities (amy low and medium) — by the short name otherwise.
fn is_loaded(record: &VoiceModelRecord, loaded: Option<&TtsLoaded>) -> bool {
    let Some(loaded) = loaded else {
        return false;
    };
    if record.engine == SILERO {
        return matches!((&loaded.silero_file, &record.file), (Some(loaded), Some(file)) if loaded == file);
    }
    let stem = record.file.as_deref().map(|file| file.strip_suffix(".onnx").unwrap_or(file));
    if let Some(files) = &loaded.piper_files {
        return stem.is_some_and(|stem| !stem.is_empty() && files.iter().any(|file| file == stem));
    }
    let short = record.voices.first().map(|voice| voice.short.as_deref().unwrap_or(&voice.name)).unwrap_or("");
    loaded.piper.iter().any(|name| name == short)
}

/// The registry's order: our languages first, English next, the world alphabetically after.
fn rank(record: &VoiceModelRecord) -> u8 {
    match record.locale.as_deref().and_then(|locale| locale.split('_').next()) {
        Some("ru") => 0,
        Some("en") => 1,
        _ => 2,
    }
}

/// Titles in a reader's order, as localeCompare puts them: letters before case.
fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| b.cmp(a))
}

/// `6 ноября`: a date as toLocaleDateString('ru-RU', {day: 'numeric', month: 'long'}) writes it, in local time.
fn day_and_month(iso: &str) -> String {
    let months = [
        crate::tr!("января"),
        crate::tr!("февраля"),
        crate::tr!("марта"),
        crate::tr!("апреля"),
        crate::tr!("мая"),
        crate::tr!("июня"),
        crate::tr!("июля"),
        crate::tr!("августа"),
        crate::tr!("сентября"),
        crate::tr!("октября"),
        crate::tr!("ноября"),
        crate::tr!("декабря"),
    ];
    match chrono::DateTime::parse_from_rfc3339(iso) {
        Ok(at) => {
            let local = at.with_timezone(&chrono::Local);
            format!("{} {}", local.day(), months[local.month0() as usize])
        }
        Err(_) => "Invalid Date".into(),
    }
}

/// A file of the registry's tree listing.
#[derive(Debug, Deserialize)]
struct RegistryFile {
    path: String,
    #[serde(default)]
    size: Option<f64>,
}

/// The next page of a `Link: <…>; rel="next"` header.
fn next_page(link: &str) -> Option<String> {
    if !link.contains(r#"rel="next""#) {
        return None;
    }
    let first = link.split(';').next().unwrap_or_default();
    let mut chars = first.chars();
    chars.next();
    chars.next_back();
    Some(chars.as_str().to_string())
}

/// encodeURIComponent: everything but letters, digits and `-_.!~*'()` as %XX of its UTF-8.
fn encode_uri_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// A registry file as a catalog row: <lang>/<lang>_<LOC>/<name>/<quality>/<lang>_<LOC>-<name>-<quality>.onnx.
fn registry_record(
    file: &RegistryFile,
    seen: &mut HashSet<String>,
    builtin_files: &HashSet<String>,
) -> Option<VoiceModelRecord> {
    if !file.path.ends_with(".onnx") {
        return None;
    }
    let parts: Vec<&str> = file.path.split('/').collect();
    let [lang, locale, name, quality, filename] = parts[..] else {
        return None;
    };
    let id = format!("piper-{locale}-{name}-{quality}");
    if !safe_name(&id) || !safe_name(filename) || name.is_empty() {
        return None;
    }
    if seen.contains(filename) || builtin_files.contains(filename) {
        return None;
    }
    seen.insert(filename.to_string());
    let mut title = String::new();
    let mut letters = name.chars();
    if let Some(first) = letters.next() {
        title.extend(first.to_uppercase());
        title.push_str(letters.as_str());
    }
    let quality_title = match quality {
        "high" => crate::tr!("Высокое"),
        "medium" => crate::tr!("Среднее"),
        _ => crate::tr!("Лёгкое"),
    };
    Some(VoiceModelRecord {
        id,
        engine: PIPER.into(),
        title: format!("{title} · {lang}"),
        description: crate::tr!("{quality} качество · {locale}", quality = quality_title, locale = locale),
        badge: None,
        // The registry lists no licences: each voice has its own (CC0, CC BY, CC BY-NC-SA, unknown) in its MODEL_CARD.
        license: Some(crate::tr!("лицензия — в MODEL_CARD голоса на Hugging Face")),
        locale: Some(locale.into()),
        quality: Some(quality.into()),
        size: file
            .size
            .filter(|size| *size != 0.0)
            .map(|size| crate::tr!("{size} МБ", size = (size / 1_048_576.0).round().max(1.0))),
        url: Some(format!(
            "https://huggingface.co/rhasspy/piper-voices/resolve/main/{}",
            parts.iter().map(|part| encode_uri_component(part)).collect::<Vec<_>>().join("/")
        )),
        file: Some(filename.into()),
        source: "registry".into(),
        voices: vec![VoiceEntry {
            name: filename.strip_suffix(".onnx").unwrap_or(filename).into(),
            short: Some(name.into()),
        }],
    })
}

/// The showcase entry as catalog data: the Silero generations keep five voices, a Piper file is one voice.
fn showcase_voice_model(entry: &ShowcaseTts) -> VoiceModelRecord {
    let voices = if entry.engine == SILERO {
        SILERO_VOICES.iter().map(|name| VoiceEntry { name: name.to_string(), short: None }).collect()
    } else {
        let file = entry.file.as_deref().unwrap_or_default();
        vec![VoiceEntry { name: file.strip_suffix(".onnx").unwrap_or(file).into(), short: entry.voice.clone() }]
    };
    VoiceModelRecord {
        id: entry.id.clone(),
        engine: entry.engine.clone(),
        title: entry.title.clone(),
        description: entry.description.clone(),
        badge: entry.badge.clone(),
        license: entry.license.clone(),
        locale: entry.locale.clone(),
        quality: entry.quality.clone(),
        size: entry.size.clone(),
        url: entry.url.clone(),
        file: entry.file.clone(),
        source: "builtin".into(),
        voices,
    }
}

/// Alice's own voice: not a catalog row — the provider flips to Yandex and the station answers itself.
fn yandex_card(active: bool) -> TtsCard {
    TtsCard {
        id: "yandex".into(),
        engine: "yandex".into(),
        group: None,
        source: None,
        own: None,
        title: crate::tr!("Яндекс — голос Алисы"),
        description: crate::tr!("Как на колонке из коробки: отвечает голосом Алисы, своим движком Яндекса."),
        badge: Some(crate::tr!("как было")),
        license: None,
        locale: None,
        quality: None,
        size: None,
        url: None,
        file: None,
        voice: None,
        voices: Vec::new(),
        installed: true,
        loaded: true,
        active,
        downloadable: false,
        state: CardState::Ready,
        progress: None,
        reason: None,
        needs: None,
    }
}

/// A catalog voice's state: its file here, downloading, or to download (Silero's by hand).
fn file_state(
    record: &VoiceModelRecord,
    installed: bool,
    downloadable: bool,
    downloads: &[DownloadState],
) -> (CardState, Option<u32>, Option<String>, Option<VoiceNeeds>) {
    if installed {
        return (CardState::Ready, None, None, None);
    }
    if let Some(download) =
        downloads.iter().find(|download| download.id == record.id && download.state == DownloadPhase::Downloading)
    {
        let percent = (download.bytes * 100).checked_div(download.total).map(|percent| percent.min(100) as u32);
        return (CardState::Installing, percent.or(Some(0)), None, Some(file_needs(record)));
    }
    let reason = (!downloadable).then(by_hand);
    (CardState::Needs, None, reason, Some(file_needs(record)))
}

/// The 409 of a voice whose model or file is not here: what it needs in the error's `errors`.
fn needed(needs: &VoiceNeeds, installing: bool) -> ApiError {
    let message = match (needs.kind, installing) {
        (_, true) => crate::tr!("{title} ещё ставится — голос включится, когда встанет.", title = needs.title),
        ("engine", _) => crate::tr!("Голосу нужна модель {title}.", title = needs.title),
        _ => crate::tr!("Голос «{title}» ещё не скачан.", title = needs.title),
    };
    ApiError {
        status: axum::http::StatusCode::CONFLICT,
        message,
        errors: Some(json!({ "state": if installing { "installing" } else { "needs" }, "needs": needs })),
    }
}

fn pending_view(pending: &VoicePending, progress: Option<u32>) -> PendingView {
    PendingView {
        model_id: pending.model_id.clone(),
        voice_id: pending.voice_id.clone(),
        wait_for: pending.wait_for.clone(),
        state: pending.state.clone(),
        progress,
        reason: pending.reason.clone(),
        since: pending.since.clone(),
    }
}

/// An own voice's state by its engine's: installed, installing, to install, or not here at all; another engine that
/// is here is offered as a reimprint.
fn engine_state(
    voice: &OwnVoiceCard,
    engines: &[EngineState],
    job: Option<&LabJob>,
) -> (CardState, Option<u32>, Option<String>, Option<VoiceNeeds>) {
    let Some(engine) = engines.iter().find(|engine| engine.id == voice.engine) else {
        // Nothing known of the engines (a test without them): as the card's own word.
        return if voice.ready { (CardState::Ready, None, None, None) } else { (CardState::Needs, None, None, None) };
    };
    let alternatives = || {
        engines
            .iter()
            .filter(|other| voice.reimprint && other.id != engine.id && other.status == "ready")
            .map(|other| Alternative { kind: "reimprint", engine: other.id.clone() })
            .collect()
    };
    match engine.status.as_str() {
        "ready" => (CardState::Ready, None, None, None),
        "installing" => {
            let percent = job.filter(|job| job.running && job.kind == "install").map(|job| job.percent);
            (CardState::Installing, percent, None, Some(engine_needs(engine, alternatives())))
        }
        "unsupported" => (CardState::Unsupported, None, engine.reason.clone(), None),
        _ => (CardState::Needs, None, None, Some(engine_needs(engine, alternatives()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use axum::extract::State;
    use axum::http::header;
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use axum::Router;
    use tokio::sync::broadcast;

    use crate::config::Env;
    use crate::models::model_files::UPLOAD_DEADLINE;
    use crate::models::model_servers::{Finished, Launcher, RunError};

    /// The voice as VoiceService decides it, without the synthesis: whose voice, which pair, what it was told.
    struct FakeVoice {
        own_speech: bool,
        state: Mutex<(TtsProvider, Option<VoicePair>)>,
        calls: Mutex<Vec<String>>,
        routes: Mutex<Vec<String>>,
    }

    impl FakeVoice {
        fn new(own_speech: bool) -> Arc<FakeVoice> {
            Arc::new(FakeVoice {
                own_speech,
                state: Mutex::new((TtsProvider::Yandex, None)),
                calls: Mutex::default(),
                routes: Mutex::default(),
            })
        }

        fn calls(&self) -> Vec<String> {
            lock(&self.calls).clone()
        }

        /// The routes it was given, as the voice lab made them (a String in these tests).
        fn take_route(&self, route: Option<OwnRoute>) {
            if let Some(route) = route.and_then(|route| route.downcast::<String>().ok()) {
                lock(&self.routes).push((*route).clone());
            }
        }
    }

    impl Voice for FakeVoice {
        fn has_own_speech(&self) -> bool {
            self.own_speech
        }

        fn chosen_provider(&self) -> TtsProvider {
            lock(&self.state).0
        }

        fn whose(&self, yandex_allowed: bool) -> Whose {
            let chosen = lock(&self.state).0;
            let provider = match chosen {
                TtsProvider::Yandex if !yandex_allowed && self.own_speech => TtsProvider::Mixed,
                TtsProvider::Yandex if !yandex_allowed => TtsProvider::None,
                other => other,
            };
            if provider == TtsProvider::Yandex {
                Whose::Yandex
            } else {
                Whose::Own
            }
        }

        fn own_choice(&self) -> Option<VoicePair> {
            lock(&self.state).1.clone()
        }

        fn use_yandex(&self) {
            lock(&self.state).0 = TtsProvider::Yandex;
            lock(&self.calls).push("yandex".into());
        }

        fn use_own(&self, own: VoicePair, ids: VoiceIds, route: Option<OwnRoute>) -> Result<(), ApiError> {
            if !self.own_speech {
                return Err(ApiError::internal("TTS_OWN_URL is not set"));
            }
            lock(&self.calls).push(format!("own {}/{} as {} {}", own.model, own.voice, ids.model_id, ids.voice_id));
            self.take_route(route);
            let mut state = lock(&self.state);
            if state.0 != TtsProvider::Own {
                state.0 = TtsProvider::Mixed;
            }
            state.1 = Some(own);
            Ok(())
        }

        fn restore(&self, provider: Option<TtsProvider>, own: Option<VoicePair>, route: Option<OwnRoute>) {
            lock(&self.calls).push(format!(
                "restore {} {}",
                provider.map_or("-", TtsProvider::as_str),
                own.as_ref().map_or("-".into(), |own| format!("{}/{}", own.model, own.voice))
            ));
            self.take_route(route);
            let mut state = lock(&self.state);
            if let Some(own) = own {
                state.1 = Some(own);
            }
            if let Some(provider) = provider {
                state.0 = provider;
            }
        }
    }

    /// The voice lab with one own voice, «Мой голос»: its server starts only with «всё равно» (memory is tight).
    /// OmniVoice is installed unless a test says otherwise; installs and imprints are noted, not run.
    #[derive(Default)]
    struct FakeLab {
        released: Mutex<usize>,
        /// OmniVoice's status; none: ready.
        omnivoice: Mutex<Option<&'static str>>,
        installs: Mutex<Vec<String>>,
        imprints: Mutex<Vec<String>>,
        /// The imprints that came out: voices from now on.
        made: Mutex<Vec<String>>,
    }

    impl FakeLab {
        fn omnivoice(&self, status: &'static str) {
            *lock(&self.omnivoice) = Some(status);
        }
    }

    impl OwnVoices for FakeLab {
        fn voices_for_cards(&self) -> BoxFuture<'_, Vec<OwnVoiceCard>> {
            Box::pin(async {
                vec![OwnVoiceCard {
                    id: "v1".into(),
                    name: "Мой голос".into(),
                    engine: "omnivoice".into(),
                    engine_title: "OmniVoice".into(),
                    engine_license: "код Apache-2.0 · веса CC BY-NC 4.0, некоммерчески".into(),
                    created_at: "2026-11-06T10:00:00.000Z".into(),
                    ref_seconds: Some(12.5),
                    samples: 1,
                    ready: lock(&self.omnivoice).is_none_or(|status| status == "ready"),
                    serving: true,
                    reimprint: true,
                    origin: None,
                }]
            })
        }

        fn prepare(&self, id: String, anyway: bool) -> BoxFuture<'_, Result<PreparedVoice, ApiError>> {
            Box::pin(async move {
                if id != "v1" && !lock(&self.made).contains(&id) {
                    return Err(ApiError::not_found("нет такого голоса"));
                }
                if !anyway {
                    return Err(ApiError::new(axum::http::StatusCode::PRECONDITION_REQUIRED, "Памяти впритык."));
                }
                Ok(PreparedVoice {
                    engine: "omnivoice".into(),
                    route: Arc::new("route to omnivoice".to_string()),
                    name: "Мой голос".into(),
                })
            })
        }

        fn restore(&self, id: &str) -> Option<RestoredVoice> {
            (id == "v1").then(|| RestoredVoice {
                engine: "omnivoice".into(),
                route: Arc::new("route to omnivoice".to_string()),
            })
        }

        fn release(&self) {
            *lock(&self.released) += 1;
        }

        fn engines(&self) -> BoxFuture<'_, Vec<EngineState>> {
            let engine = |id: &str, title: &str, status: &str, reason: Option<&str>| EngineState {
                id: id.into(),
                title: title.into(),
                status: status.into(),
                reason: reason.map(str::to_string),
                download_mb: 4_134,
                disk_mb: 5_600,
            };
            let omnivoice = lock(&self.omnivoice).unwrap_or("ready");
            let reason = (omnivoice == "unsupported").then_some("Мак на Intel: PyTorch новее 2.2 для него не выходит.");
            Box::pin(async move {
                vec![
                    engine("omnivoice", "OmniVoice", omnivoice, reason),
                    engine("cosyvoice3", "CosyVoice 3", "ready", None),
                ]
            })
        }

        fn install_engine(&self, engine: String, _actor: Option<Actor>) -> BoxFuture<'_, Result<(), ApiError>> {
            lock(&self.installs).push(engine);
            self.omnivoice("installing");
            Box::pin(async { Ok(()) })
        }

        fn imprint(
            &self,
            recipe: VoiceRecipe,
            _anyway: bool,
            _actor: Option<Actor>,
        ) -> BoxFuture<'_, Result<(), ApiError>> {
            lock(&self.imprints).push(recipe.voice);
            Box::pin(async { Ok(()) })
        }

        fn job(&self) -> Option<LabJob> {
            None
        }

        fn voice_engine(&self, id: &str) -> Option<String> {
            (id == "v1" || lock(&self.made).iter().any(|made| made == id)).then(|| "omnivoice".to_string())
        }
    }

    /// Nothing is started from these tests.
    struct NoLaunch;

    impl Launcher for NoLaunch {
        fn run(
            &self,
            _program: PathBuf,
            _args: Vec<String>,
            _timeout: Option<Duration>,
        ) -> BoxFuture<'static, Result<Finished, RunError>> {
            Box::pin(async { Err(RunError::Start("nothing starts in tests".into())) })
        }

        fn start_detached(
            &self,
            _program: PathBuf,
            _args: Vec<String>,
            _env: Vec<(String, String)>,
        ) -> Result<(), String> {
            Err("nothing starts in tests".into())
        }
    }

    /// The recognition server as the fake plays it: the model it serves, the one it loads (served after two
    /// looks at /health), and every load it was asked for.
    #[derive(Default)]
    struct Asr {
        model: Option<String>,
        loading: Option<String>,
        looks: u32,
        loads: Vec<String>,
        /// How many looks a load lasts beyond the first two: a test that needs it under way gives it time.
        slow: u32,
    }

    #[derive(Clone)]
    struct WorldState {
        asr: Arc<Mutex<Asr>>,
        models: PathBuf,
        base: String,
    }

    /// The world around the shop on one local server: the registry's two pages, a host of a voice's files, the
    /// recognition server, and a voice server that reads the models folder as tts-server does.
    struct World {
        root: tempfile::TempDir,
        base: String,
        asr: Arc<Mutex<Asr>>,
    }

    impl World {
        fn models(&self) -> PathBuf {
            self.root.path().join("models")
        }
    }

    fn page_one() -> Value {
        json!([
            {"type": "directory", "path": "ru", "size": 0},
            {"type": "file", "path": "ru/ru_RU/denis/medium/ru_RU-denis-medium.onnx", "size": 63_201_294},
            {"type": "file", "path": "ru/ru_RU/denis/medium/ru_RU-denis-medium.onnx.json", "size": 4886},
            {"type": "file", "path": "en/en_US/amy/medium/en_US-amy-medium.onnx", "size": 63_201_294},
            {"type": "file", "path": "de/de_DE/thorsten/high/de_DE-thorsten-high.onnx", "size": 113_895_201},
            {"type": "file", "path": "README.md", "size": 100},
        ])
    }

    fn page_two() -> Value {
        json!([
            {"type": "file", "path": "en/en_GB/alan/low/en_GB-alan-low.onnx", "size": 1000},
            {"type": "file", "path": "ru/ru_RU/irina/low/ru_RU-irina-low.onnx", "size": 28_130_000},
            {"type": "file", "path": "en/en_US/amy/medium/en_US-amy-medium.onnx", "size": 63_201_294},
            {"type": "file", "path": "x/y.onnx"},
        ])
    }

    /// What the voice server has loaded: what is in the folder, as its rescan reads it.
    fn loaded_from(models: &Path) -> Value {
        let mut silero = Value::Null;
        let (mut piper, mut files) = (Vec::new(), Vec::new());
        for folder in std::fs::read_dir(models).into_iter().flatten().flatten() {
            for file in std::fs::read_dir(folder.path()).into_iter().flatten().flatten() {
                let name = file.file_name().to_string_lossy().into_owned();
                if let Some(stem) = name.strip_suffix(".onnx") {
                    piper.push(stem.split('-').nth(1).unwrap_or(stem).to_string());
                    files.push(stem.to_string());
                } else if name.ends_with(".pt") {
                    silero = json!(name);
                }
            }
        }
        json!({"silero": silero, "piper": piper, "piperFiles": files})
    }

    async fn world() -> World {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("models")).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let asr = Arc::new(Mutex::new(Asr { model: Some("v3_e2e_rnnt".into()), ..Default::default() }));
        let state = WorldState { asr: asr.clone(), models: root.path().join("models"), base: base.clone() };
        async fn tree(State(state): State<WorldState>) -> axum::response::Response {
            ([(header::LINK, format!("<{}/tree2>; rel=\"next\"", state.base))], axum::Json(page_one())).into_response()
        }
        async fn voice_file(axum::extract::Path(name): axum::extract::Path<String>) -> axum::response::Response {
            if name.ends_with(".onnx.json") {
                return axum::Json(json!({"audio": {"sample_rate": 22050}})).into_response();
            }
            if name.ends_with(".onnx") || name.ends_with(".pt") {
                return Bytes::from(vec![7u8; 64 * 1024]).into_response();
            }
            axum::http::StatusCode::NOT_FOUND.into_response()
        }
        async fn asr_health(State(state): State<WorldState>) -> axum::Json<Value> {
            let mut asr = lock(&state.asr);
            if asr.loading.is_some() {
                asr.looks += 1;
                if asr.looks > 2 + asr.slow {
                    asr.model = asr.loading.take();
                }
            }
            axum::Json(
                json!({"status": "ok", "model": asr.model, "loading": asr.loading, "error": null, "features": ["load"]}),
            )
        }
        async fn asr_load(
            State(state): State<WorldState>,
            axum::Json(body): axum::Json<Value>,
        ) -> axum::response::Response {
            let model = body["model"].as_str().unwrap_or_default().to_string();
            let mut asr = lock(&state.asr);
            asr.loads.push(model.clone());
            asr.loading = Some(model.clone());
            asr.looks = 0;
            (axum::http::StatusCode::ACCEPTED, axum::Json(json!({"status": "loading", "model": model}))).into_response()
        }
        async fn tts_health(State(state): State<WorldState>) -> axum::Json<Value> {
            axum::Json(json!({"status": "ok", "loaded": loaded_from(&state.models), "features": ["rescan"]}))
        }
        let router = Router::new()
            .route("/tree", get(tree))
            .route("/tree2", get(|| async { axum::Json(page_two()) }))
            .route("/voices/{name}", get(voice_file))
            .route("/asr/health", get(asr_health))
            .route("/asr/v1/models/load", post(asr_load))
            .route("/tts/health", get(tts_health))
            .route("/tts/v1/models/rescan", post(tts_health))
            .with_state(state);
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        World { root, base, asr }
    }

    struct Shop {
        shop: Arc<ModelsService>,
        events: broadcast::Receiver<Arc<GatewayEvent>>,
        db: Arc<Db>,
    }

    impl Shop {
        /// The names of the events so far.
        fn events(&mut self) -> Vec<&'static str> {
            let mut names = Vec::new();
            while let Ok(event) = self.events.try_recv() {
                names.push(event.name());
            }
            names
        }
    }

    struct Setup<'a> {
        voice: Option<Arc<FakeVoice>>,
        lab: Option<Arc<FakeLab>>,
        tts: bool,
        env: &'a [(&'a str, &'a str)],
        db: Option<Arc<Db>>,
    }

    impl Default for Setup<'_> {
        fn default() -> Self {
            Setup { voice: Some(FakeVoice::new(true)), lab: None, tts: false, env: &[], db: None }
        }
    }

    fn open_shop(world: &World, setup: Setup<'_>) -> Shop {
        let mut config = Config::for_tests(&Env::from_pairs(setup.env.iter().copied()));
        config.models.piper_registry = format!("{}/tree", world.base);
        let db = setup.db.unwrap_or_else(|| Arc::new(Db::memory()));
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let base = world.base.clone();
        let files = ModelFiles::with_check(world.models(), Arc::new(move |url: &str| url.starts_with(&base)));
        let tts = format!("{}/tts/v1", world.base);
        let servers = ModelServers::new(
            Some(&format!("{}/asr/v1", world.base)),
            setup.tts.then_some(tts.as_str()),
            Some(world.root.path().join("chosen-model")),
            ModelServersOptions {
                platform: Some("linux".into()),
                mac_deploy: Some(world.root.path().join("deploy")),
                launch_daemons: Some(world.root.path().join("LaunchDaemons")),
                asr_python: Some(world.root.path().join("no-python")),
                load_poll: Some(Duration::from_millis(10)),
                launcher: Some(Arc::new(NoLaunch)),
                ..Default::default()
            },
        );
        let links = ModelsLinks {
            voice: setup.voice.map(|voice| voice as Arc<dyn Voice>),
            own_voices: setup.lab.map(|lab| lab as Arc<dyn OwnVoices>),
            local_asr: None,
            managed_tts: None,
        };
        let receiver = events.subscribe();
        let shop =
            ModelsService::with_parts(&config, db.clone(), events, settings, links, ModelsParts { files, servers });
        Shop { shop: Arc::new(shop), events: receiver, db }
    }

    /// A voice of the registry whose files the world hosts.
    fn hosted_voice(world: &World) -> VoiceModelRecord {
        VoiceModelRecord {
            id: "piper-xx_XX-test-low".into(),
            engine: PIPER.into(),
            title: "Test · xx".into(),
            description: "Лёгкое качество · xx_XX".into(),
            badge: None,
            license: None,
            locale: Some("xx_XX".into()),
            quality: Some("low".into()),
            size: Some("1 МБ".into()),
            url: Some(format!("{}/voices/xx_XX-test-low.onnx", world.base)),
            file: Some("xx_XX-test-low.onnx".into()),
            source: "registry".into(),
            voices: vec![VoiceEntry { name: "xx_XX-test-low".into(), short: Some("test".into()) }],
        }
    }

    /// A voice put on disk by hand, as the voice server finds it.
    fn put_on_disk(world: &World, id: &str, file: &str) {
        let folder = world.models().join(id);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(file), b"model").unwrap();
        if file.ends_with(".onnx") {
            std::fs::write(folder.join(format!("{file}.json")), b"{}").unwrap();
        }
    }

    /// Until the download of `id` is over: what it left in the list (None: done and gone).
    async fn settled(shop: &ModelsService, id: &str) -> Option<DownloadState> {
        for _ in 0..500 {
            let state =
                lock(&shop.downloads).iter().find(|download| download.id == id).map(|download| download.state());
            match state {
                Some(state) if state.state == DownloadPhase::Downloading || state.state == DownloadPhase::Done => {}
                other => return other,
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the download of {id} did not end");
    }

    fn ids(model_id: &str, voice_id: &str) -> VoiceIds {
        VoiceIds { model_id: model_id.into(), voice_id: voice_id.into() }
    }

    #[test]
    fn reads_the_built_in_showcase() {
        assert_eq!(SHOWCASE.asr.len(), 18);
        assert_eq!(SHOWCASE.asr.iter().filter(|card| card.recommended).count(), 4);
        assert_eq!(SHOWCASE.tts.iter().filter(|card| card.engine == SILERO).count(), 2);
    }

    #[tokio::test]
    async fn seeds_the_showcase_and_lays_the_world_over_the_catalog() {
        let world = world().await;
        let Shop { shop, .. } = open_shop(&world, Setup::default());
        shop.start().await.unwrap();
        shop.start().await.unwrap();
        let view = shop.view().await.unwrap();

        let cards: Vec<&str> = view.tts.iter().map(|card| card.id.as_str()).collect();
        assert_eq!(
            cards,
            [
                "yandex",
                "silero",
                "silero-v5",
                "piper-denis",
                "piper-dmitri",
                "piper-irina",
                "piper-ruslan",
                // The registry: ours first, English next, the world after; the showcase's own files are not repeated.
                "piper-ru_RU-irina-low",
                "piper-en_GB-alan-low",
                "piper-en_US-amy-medium",
                "piper-de_DE-thorsten-high",
            ],
            "seeded twice, listed once"
        );
        assert!(view.tts[0].active, "Alice speaks as the station came");
        let silero = &view.tts[1];
        assert_eq!(silero.voices.len(), 5);
        assert_eq!(silero.voice.as_deref(), Some("aidar"));
        assert_eq!(silero.source.as_deref(), Some("builtin"));
        let denis = &view.tts[3];
        assert_eq!(denis.voices, vec![CardVoice { id: "piper-denis/ru_RU-denis-medium".into(), name: "denis".into() }]);
        assert!(!denis.installed && !denis.loaded && !denis.active);
        let amy = view.tts.iter().find(|card| card.id == "piper-en_US-amy-medium").unwrap();
        assert_eq!(amy.title, "Amy · en");
        assert_eq!(amy.description, "Среднее качество · en_US");
        assert_eq!(amy.size.as_deref(), Some("60 МБ"));
        assert_eq!(
            amy.url.as_deref(),
            Some("https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/amy/medium/en_US-amy-medium.onnx")
        );
        assert_eq!(
            amy.voices,
            vec![CardVoice { id: "piper-en_US-amy-medium/en_US-amy-medium".into(), name: "amy".into() }]
        );
        let alan = view.tts.iter().find(|card| card.id == "piper-en_GB-alan-low").unwrap();
        assert_eq!(alan.size.as_deref(), Some("1 МБ"), "never 0 МБ");

        assert_eq!(view.asr.len(), 17, "Yandex's recognition is the mode, not a card");
        assert!(view.asr[0].recommended);
        assert!(view.asr[0].active, "the server serves v3_e2e_rnnt");
        assert!(!view.asr.iter().find(|card| card.record.id == "whisper-tiny").unwrap().recommended);
        assert_eq!(view.asr_loaded.as_deref(), Some("v3_e2e_rnnt"));
        assert_eq!(view.tts_active, ids("yandex", ""));
        assert_eq!((view.voice, view.speaking, view.mode), (Whose::Yandex, Whose::Yandex, PanelMode::Relay));
        assert!(!view.tts_up, "no voice server of ours");
        assert!(view.can_restart, "the recognition server loads over HTTP");
        assert!(view.own_voice_server);
        assert_eq!(view.restarting, None);

        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["ttsActive"], json!({"modelId": "yandex", "voiceId": ""}));
        assert_eq!(json["asrLoaded"], "v3_e2e_rnnt");
        assert_eq!(json["restarting"], Value::Null);
        assert_eq!(json["mode"], "relay");
        assert_eq!(json["voice"], "yandex");
        assert_eq!(
            json["asr"][0],
            json!({
                "id": "gigaam-v3-e2e",
                "model": "v3_e2e_rnnt",
                "title": "GigaAM v3 — конец-в-конец",
                "description": "Пунктуация и заглавные из коробки, самый точный из быстрых. Рекомендуемый.",
                "badge": "рекомендуем",
                "license": "MIT · Сбережение (Сбер)",
                "source": "huggingface:salute-developers/GigaAM",
                "recommended": true,
                "active": true,
                "pending": false,
            })
        );
        assert_eq!(json["tts"][0]["voices"], json!([]));
        assert!(json["tts"][0].get("source").is_none());
        assert!(json["tts"][3].get("badge").is_none(), "absent, not null: {}", json["tts"][3]);
    }

    #[tokio::test]
    async fn downloads_a_voice_whole_and_deletes_it_when_it_does_not_speak() {
        let world = world().await;
        let mut shop = open_shop(&world, Setup::default());
        shop.shop.start().await.unwrap();
        shop.shop.catalog.save_voice_models(&[hosted_voice(&world)]).unwrap();
        let id = "piper-xx_XX-test-low";

        let started = shop.shop.install_tts(id, None).await.unwrap();
        assert_eq!(started.state, DownloadPhase::Downloading);
        assert_eq!(settled(&shop.shop, id).await, None, "a finished download leaves the list");
        assert!(world.models().join(id).join("xx_XX-test-low.onnx").exists());
        assert!(world.models().join(id).join("xx_XX-test-low.onnx.json").exists());
        let card = shop.shop.view().await.unwrap().tts.into_iter().find(|card| card.id == id).unwrap();
        assert!(card.installed && card.downloadable);
        assert!(!card.loaded, "no voice server has it");
        assert!(shop.events().contains(&"model.installed"));

        shop.shop.uninstall_tts(id, None).await.unwrap();
        assert!(!world.models().join(id).exists());
        assert!(shop.events().contains(&"model.removed"));
        let again = shop.shop.uninstall_tts(id, None).await.unwrap_err();
        assert_eq!((again.status.as_u16(), again.message.as_str()), (404, "Этот голос не скачан."));

        let unknown = shop.shop.install_tts("nope", None).await.unwrap_err();
        assert_eq!((unknown.status.as_u16(), unknown.message.as_str()), (404, "нет скачиваемого голоса nope"));
        let elsewhere = shop.shop.install_tts("piper-denis", None).await.unwrap_err();
        assert_eq!(elsewhere.message, "Этот голос не скачивается отсюда: файл кладут вручную.");
    }

    #[tokio::test]
    async fn a_voice_chosen_before_its_file_is_here_is_downloaded_and_switched_on_by_itself() {
        let world = world().await;
        let voice = FakeVoice::new(true);
        let shop = open_shop(&world, Setup { voice: Some(voice.clone()), ..Default::default() });
        shop.shop.start().await.unwrap();
        shop.shop.catalog.save_voice_models(&[hosted_voice(&world)]).unwrap();
        let id = "piper-xx_XX-test-low";
        let refused = shop.shop.choose(VoiceRequest::card(id), None).await.unwrap_err();
        assert_eq!(refused.status.as_u16(), 409);
        assert_eq!(refused.errors.unwrap()["needs"]["downloadMb"], 1);
        let request = VoiceRequest::of(&json!({"modelId": id, "install": true}));
        let Chosen::Pending(waiting) = shop.shop.choose(request, None).await.unwrap() else { panic!("not pending") };
        assert_eq!(waiting.wait_for, PendingWait { kind: "file".into(), id: id.into() });
        // The download ends, the voice switches on, the pending one is forgotten.
        for _ in 0..200 {
            if pending(&shop).is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(pending(&shop).is_none(), "{:?}", pending(&shop));
        assert_eq!(shop.shop.chosen_voice().unwrap().model_id, id);
        assert!(world.models().join(id).join("xx_XX-test-low.onnx").exists());
    }

    #[tokio::test]
    async fn a_failed_download_says_why_and_a_canceled_one_says_so() {
        let world = world().await;
        let shop = open_shop(&world, Setup::default());
        shop.shop.start().await.unwrap();
        let mut missing = hosted_voice(&world);
        missing.id = "piper-xx_XX-gone-low".into();
        missing.url = Some(format!("{}/voices/xx_XX-gone-low.bin", world.base));
        missing.file = Some("xx_XX-gone-low.onnx".into());
        shop.shop.catalog.save_voice_models(&[missing]).unwrap();
        shop.shop.install_tts("piper-xx_XX-gone-low", None).await.unwrap();
        let failed = settled(&shop.shop, "piper-xx_XX-gone-low").await.unwrap();
        assert_eq!(failed.state, DownloadPhase::Error);
        assert!(failed.error.unwrap().starts_with("HTTP 404 for"));
        assert!(!world.models().join("piper-xx_XX-gone-low").exists());

        shop.shop.catalog.save_voice_models(&[hosted_voice(&world)]).unwrap();
        *lock(&shop.shop.models) = None;
        shop.shop.install_tts("piper-xx_XX-test-low", None).await.unwrap();
        shop.shop.cancel_tts("piper-xx_XX-test-low");
        let canceled = settled(&shop.shop, "piper-xx_XX-test-low").await.unwrap();
        assert_eq!(canceled.state, DownloadPhase::Canceled);
        assert!(!world.models().join("piper-xx_XX-test-low").exists());
    }

    #[tokio::test]
    async fn chooses_our_voice_by_catalog_ids_only_once_it_is_downloaded() {
        let world = world().await;
        let voice = FakeVoice::new(true);
        let shop = open_shop(&world, Setup { voice: Some(voice.clone()), ..Default::default() });
        let shop = shop.shop;
        shop.start().await.unwrap();

        let refused = shop.activate_tts(&json!({"modelId": "piper-denis"}), None).await.unwrap_err();
        assert_eq!((refused.status.as_u16(), refused.message.as_str()), (409, by_hand().as_str()));
        assert_eq!(refused.errors.unwrap()["needs"]["kind"], "file", "what it needs, in the error's errors");
        put_on_disk(&world, "piper-denis", "ru_RU-denis-medium.onnx");
        let chosen = shop.activate_tts(&json!({"modelId": "piper-denis"}), None).await.unwrap();
        assert!(chosen.needs_restart, "no voice server has it yet");
        assert_eq!(
            voice.calls(),
            ["restore - -", "own piper/ru_RU-denis-medium as piper-denis piper-denis/ru_RU-denis-medium"]
        );
        let stored = shop.settings.get(&VOICE_CHOICE, None).map(Value::Object);
        assert_eq!(stored, Some(json!({"modelId": "piper-denis", "voiceId": "piper-denis/ru_RU-denis-medium"})));
        assert_eq!(shop.settings.get(&VOICE_PROVIDER, None).as_deref(), Some("mixed"));
        let view = shop.view().await.unwrap();
        assert_eq!(view.tts_active, ids("piper-denis", "piper-denis/ru_RU-denis-medium"));
        assert!(view.tts.iter().find(|card| card.id == "piper-denis").unwrap().active);
        assert_eq!(view.voice, Whose::Own);

        let speaking = shop.uninstall_tts("piper-denis", None).await.unwrap_err();
        assert_eq!(speaking.message, "Этот голос говорит сейчас — сначала выберите другой.");
        assert_eq!(
            shop.activate_tts(&json!({"modelId": "yandex"}), None).await.unwrap(),
            Activated { needs_restart: false }
        );
        assert_eq!(shop.settings.get(&VOICE_PROVIDER, None).as_deref(), Some("yandex"));
        assert_eq!(shop.view().await.unwrap().tts_active, ids("yandex", ""));

        // The older pair still resolves: by the voice's short name.
        shop.activate_tts(&json!({"model": "piper", "voice": "denis"}), None).await.unwrap();
        assert_eq!(shop.chosen_voice().map(|chosen| chosen.model_id), Some("piper-denis".into()));
        let unknown = shop.activate_tts(&json!({"modelId": "nope"}), None).await.unwrap_err();
        assert_eq!((unknown.status.as_u16(), unknown.message.as_str()), (404, "нет такого голоса"));
        shop.activate_tts(&json!({"modelId": "yandex"}), None).await.unwrap();
        shop.uninstall_tts("piper-denis", None).await.unwrap();
    }

    #[tokio::test]
    async fn a_voice_the_server_has_loaded_speaks_at_once() {
        let world = world().await;
        let shop = open_shop(&world, Setup { tts: true, ..Default::default() }).shop;
        shop.start().await.unwrap();
        put_on_disk(&world, "piper-dmitri", "ru_RU-dmitri-medium.onnx");
        let chosen = shop.activate_tts(&json!({"modelId": "piper-dmitri"}), None).await.unwrap();
        assert!(!chosen.needs_restart);
        let view = shop.view().await.unwrap();
        let card = view.tts.iter().find(|card| card.id == "piper-dmitri").unwrap();
        assert!(card.installed && card.loaded && card.active);
        assert!(view.tts_up);
    }

    #[tokio::test]
    async fn without_a_voice_server_of_ours_or_the_voice_itself_nothing_of_ours_is_chosen() {
        let world = world().await;
        let without_server = open_shop(&world, Setup { voice: Some(FakeVoice::new(false)), ..Default::default() }).shop;
        without_server.start().await.unwrap();
        let refused = without_server.activate_tts(&json!({"modelId": "piper-denis"}), None).await.unwrap_err();
        assert_eq!(refused.message, "Свой голосовой сервер не настроен (TTS_OWN_URL).");
        assert!(!without_server.view().await.unwrap().own_voice_server);

        let unlinked = open_shop(&world, Setup { voice: None, ..Default::default() }).shop;
        unlinked.start().await.unwrap();
        let refused = unlinked.activate_tts(&json!({"modelId": "yandex"}), None).await.unwrap_err();
        assert_eq!(refused.status.as_u16(), 503);
        let view = unlinked.view().await.unwrap();
        assert_eq!((view.voice, view.own_voice_server), (Whose::Yandex, false), "the env's provider: yandex");
    }

    #[tokio::test]
    async fn restores_the_stored_choices_at_start() {
        let world = world().await;
        let db = Arc::new(Db::memory());
        let settings = SettingsService::new(db.clone(), EventBus::new());
        let choice = stored_ids(&ids("silero-v5", "silero-v5/baya"));
        settings.set(&VOICE_CHOICE, &choice, None).unwrap();
        settings.set(&VOICE_PROVIDER, &"mixed".to_string(), None).unwrap();
        settings.set(&ASR_MODEL, &"v3_ctc".to_string(), None).unwrap();

        let voice = FakeVoice::new(true);
        let shop =
            open_shop(&world, Setup { voice: Some(voice.clone()), db: Some(db.clone()), ..Default::default() }).shop;
        shop.start().await.unwrap();
        assert_eq!(voice.calls(), ["restore mixed silero/baya"]);
        assert_eq!(
            shop.chosen_voice(),
            Some(ChosenVoice {
                model_id: "silero-v5".into(),
                voice_id: "silero-v5/baya".into(),
                engine: "silero".into(),
                voice: "baya".into()
            })
        );
        assert_eq!(shop.chosen_asr().map(|card| card.id), Some("gigaam-v3-ctc".into()), "the model's name resolves");
        let pending =
            shop.view().await.unwrap().asr.into_iter().find(|card| card.record.id == "gigaam-v3-ctc").unwrap();
        assert!(pending.pending && !pending.active);

        // A stored «ours» without a voice server of ours cannot speak: the env's provider stays.
        let mute = FakeVoice::new(false);
        let shop = open_shop(&world, Setup { voice: Some(mute.clone()), db: Some(db), ..Default::default() }).shop;
        shop.start().await.unwrap();
        assert_eq!(mute.calls(), ["restore - silero/baya"]);

        // Nothing stored: the env's pair, when it is one of the catalog's.
        let env = [
            ("TTS_OWN_URL", "http://127.0.0.1:9/v1"),
            ("TTS_OWN_MODEL", "piper"),
            ("TTS_OWN_VOICE", "ru_RU-dmitri-medium"),
        ];
        let shop = open_shop(&world, Setup { env: &env, ..Default::default() }).shop;
        shop.start().await.unwrap();
        assert_eq!(shop.chosen_voice().map(|chosen| chosen.voice_id), Some("piper-dmitri/ru_RU-dmitri-medium".into()));
    }

    #[tokio::test]
    async fn chooses_an_own_voice_through_its_server_and_its_route() {
        let world = world().await;
        let voice = FakeVoice::new(true);
        let lab = Arc::new(FakeLab::default());
        let setup = Setup { voice: Some(voice.clone()), lab: Some(lab.clone()), ..Default::default() };
        let mut shop = open_shop(&world, setup);
        shop.shop.start().await.unwrap();
        let view = shop.shop.view().await.unwrap();
        let card = view.tts.iter().find(|card| card.id == "own:v1").unwrap();
        assert_eq!(view.tts.iter().position(|card| card.id == "own:v1"), Some(7), "right after the recommended ones");
        let json = serde_json::to_value(card).unwrap();
        assert_eq!(json["engine"], "omnivoice");
        assert_eq!(json["group"], "own");
        assert_eq!(json["badge"], "свой");
        assert_eq!(json["description"], "Ваш голос · OmniVoice · отпечаток от 6 ноября");
        assert_eq!(
            json["own"],
            json!({"id": "v1", "createdAt": "2026-11-06T10:00:00.000Z", "refSeconds": 12.5, "samples": 1, "engineTitle": "OmniVoice", "serving": true})
        );
        assert_eq!((card.installed, card.active), (true, false));

        let tight = shop.shop.activate_tts(&json!({"modelId": "own:v1"}), None).await.unwrap_err();
        assert_eq!(tight.status.as_u16(), 428, "memory is tight: the owner says «всё равно» first");
        let chosen = shop.shop.activate_tts(&json!({"modelId": "own:v1", "anyway": true}), None).await.unwrap();
        assert!(!chosen.needs_restart);
        assert_eq!(voice.calls(), ["restore - -", "own omnivoice/v1 as own:v1 own:v1"]);
        assert_eq!(lock(&voice.routes).clone(), ["route to omnivoice"], "the lab's route reaches the voice");
        let view = shop.shop.view().await.unwrap();
        assert_eq!(view.tts_active, ids("own:v1", "own:v1"));
        assert!(view.tts_up, "its own server answers");
        assert!(view.tts.iter().find(|card| card.id == "own:v1").unwrap().active);
        assert!(shop.events().contains(&"settings.changed"));

        // Choosing a catalog voice gives the own voice's memory back.
        put_on_disk(&world, "piper-denis", "ru_RU-denis-medium.onnx");
        shop.shop.activate_tts(&json!({"modelId": "piper-denis"}), None).await.unwrap();
        assert_eq!(*lock(&lab.released), 1);

        // At start the own voice comes back with its route.
        shop.shop.activate_tts(&json!({"modelId": "own:v1", "anyway": true}), None).await.unwrap();
        let restored = FakeVoice::new(true);
        let setup =
            Setup { voice: Some(restored.clone()), lab: Some(lab), db: Some(shop.db.clone()), ..Default::default() };
        open_shop(&world, setup).shop.start().await.unwrap();
        assert_eq!(restored.calls(), ["restore mixed omnivoice/v1"]);
        assert_eq!(lock(&restored.routes).clone(), ["route to omnivoice"]);
    }

    fn pending(shop: &Shop) -> Option<VoicePending> {
        SettingsService::new(shop.db.clone(), EventBus::new()).get(&VOICE_PENDING, None)
    }

    #[tokio::test]
    async fn an_own_voice_without_its_engine_says_what_it_needs_and_switches_on_once_it_is_installed() {
        let world = world().await;
        let voice = FakeVoice::new(true);
        let lab = Arc::new(FakeLab::default());
        lab.omnivoice("missing");
        let setup = Setup { voice: Some(voice.clone()), lab: Some(lab.clone()), ..Default::default() };
        let shop = open_shop(&world, setup);
        shop.shop.start().await.unwrap();
        let card = shop.shop.view().await.unwrap().tts.into_iter().find(|card| card.id == "own:v1").unwrap();
        assert_eq!(card.state, CardState::Needs);
        let needs = serde_json::to_value(card.needs.unwrap()).unwrap();
        assert_eq!(
            needs,
            json!({"kind": "engine", "id": "omnivoice", "title": "OmniVoice", "downloadMb": 4134, "diskMb": 5600,
                   "minutes": 2, "alternatives": [{"kind": "reimprint", "engine": "cosyvoice3"}]})
        );

        // Without the engine: 409 with what it needs, nothing installed.
        let refused = shop.shop.choose(VoiceRequest::of(&json!({"modelId": "own:v1"})), None).await.unwrap_err();
        assert_eq!(refused.status.as_u16(), 409);
        assert_eq!(refused.errors.as_ref().unwrap()["needs"]["kind"], "engine");
        assert_eq!(refused.errors.unwrap()["state"], "needs");
        assert!(lock(&lab.installs).is_empty() && pending(&shop).is_none());

        // «Скачать и включить»: the install starts, the choice waits.
        let request = VoiceRequest::of(&json!({"modelId": "own:v1", "install": true}));
        let Chosen::Pending(waiting) = shop.shop.choose(request, None).await.unwrap() else { panic!("not pending") };
        assert_eq!(waiting.wait_for, PendingWait { kind: "engine".into(), id: "omnivoice".into() });
        assert_eq!(lock(&lab.installs).clone(), ["omnivoice"]);
        let view = shop.shop.view().await.unwrap();
        assert_eq!(view.tts_pending.as_ref().map(|pending| pending.state.as_str()), Some("waiting"));
        assert_eq!(view.tts.iter().find(|card| card.id == "own:v1").unwrap().state, CardState::Installing);
        // Still installing: nothing yet.
        shop.shop.try_pending().await;
        assert_eq!(pending(&shop).unwrap().state, "waiting");

        // Installed: memory is tight, so it waits for «всё равно»; the owner's answer switches it on and forgets it.
        lab.omnivoice("ready");
        shop.shop.try_pending().await;
        let tight = pending(&shop).unwrap();
        assert_eq!((tight.state.as_str(), tight.reason.as_deref()), ("tight", Some("Памяти впритык.")));
        shop.shop.activate_tts(&json!({"modelId": "own:v1", "anyway": true}), None).await.unwrap();
        assert!(pending(&shop).is_none(), "a choice that succeeds forgets the pending one");
        assert!(shop.shop.view().await.unwrap().tts_pending.is_none());

        // Pending again, then switched on by itself once the engine is there (the owner said «всё равно» already).
        lab.omnivoice("missing");
        let request = VoiceRequest::of(&json!({"modelId": "own:v1", "install": true, "anyway": true}));
        assert!(matches!(shop.shop.choose(request, None).await.unwrap(), Chosen::Pending(_)));
        lab.omnivoice("ready");
        shop.shop.try_pending().await;
        assert!(pending(&shop).is_none());
        assert_eq!(shop.shop.chosen_voice().unwrap().model_id, "own:v1");
    }

    #[tokio::test]
    async fn another_choice_or_the_owner_forgets_a_pending_voice_and_a_failed_install_says_so() {
        let world = world().await;
        let lab = Arc::new(FakeLab::default());
        lab.omnivoice("missing");
        let shop = open_shop(&world, Setup { lab: Some(lab.clone()), ..Default::default() });
        shop.shop.start().await.unwrap();
        let wait = || VoiceRequest::of(&json!({"modelId": "own:v1", "install": true}));
        shop.shop.choose(wait(), None).await.unwrap();
        assert!(pending(&shop).is_some());
        shop.shop.activate_tts(&json!({"modelId": "yandex"}), None).await.unwrap();
        assert!(pending(&shop).is_none(), "another voice chosen");
        lab.omnivoice("missing");
        shop.shop.choose(wait(), None).await.unwrap();
        shop.shop.clear_pending(None);
        assert!(pending(&shop).is_none(), "the owner's «не надо»");
        // The install ended without the engine: the pending voice says it failed.
        lab.omnivoice("missing");
        shop.shop.choose(wait(), None).await.unwrap();
        lab.omnivoice("missing");
        shop.shop.try_pending().await;
        let failed = pending(&shop).unwrap();
        assert_eq!((failed.state.as_str(), failed.reason.as_deref()), ("failed", Some("установка модели прервалась")));
    }

    #[tokio::test]
    async fn a_voice_whose_engine_cannot_run_here_offers_no_download_and_a_missing_file_says_what_it_needs() {
        let world = world().await;
        let lab = Arc::new(FakeLab::default());
        lab.omnivoice("unsupported");
        let shop = open_shop(&world, Setup { lab: Some(lab.clone()), ..Default::default() });
        shop.shop.start().await.unwrap();
        let card = shop.shop.view().await.unwrap().tts.into_iter().find(|card| card.id == "own:v1").unwrap();
        assert_eq!((card.state, card.needs.is_none()), (CardState::Unsupported, true));
        let request = VoiceRequest::of(&json!({"modelId": "own:v1", "install": true}));
        let refused = shop.shop.choose(request, None).await.unwrap_err();
        assert_eq!(refused.status.as_u16(), 409);
        assert!(refused.message.starts_with("Мак на Intel"));
        assert_eq!(refused.errors.unwrap()["state"], "unsupported");
        assert!(lock(&lab.installs).is_empty(), "nothing is offered to download");

        // A catalog voice not downloaded: 409 with the file it needs.
        let refused = shop.shop.choose(VoiceRequest::card("piper-denis"), None).await.unwrap_err();
        assert_eq!(refused.status.as_u16(), 409);
        let needs = &refused.errors.unwrap()["needs"];
        assert_eq!(
            (needs["kind"].as_str(), needs["id"].as_str(), needs["downloadMb"].as_u64()),
            (Some("file"), Some("piper-denis"), Some(60))
        );
        let card = shop.shop.view().await.unwrap().tts.into_iter().find(|card| card.id == "piper-denis").unwrap();
        assert_eq!(card.state, CardState::Needs);
    }

    #[tokio::test]
    async fn a_characters_voice_without_a_snapshot_is_imprinted_from_its_reference_then_switched_on() {
        let world = world().await;
        let lab = Arc::new(FakeLab::default());
        let shop = open_shop(&world, Setup { lab: Some(lab.clone()), ..Default::default() });
        shop.shop.start().await.unwrap();
        let recipe = VoiceRecipe {
            voice: "c-arthur-arthur-omnivoice".into(),
            engine: "omnivoice".into(),
            reference: "/packages/arthur/1.2.0/assets/voices/ref.wav".into(),
            text: "Чай подан, сэр.".into(),
            name: "Артур".into(),
            package: "arthur".into(),
            version: "1.2.0".into(),
            character: "arthur".into(),
        };
        // Without its recipe the voice is unknown.
        let unknown = shop.shop.choose(VoiceRequest::card("own:c-arthur-arthur-omnivoice"), None).await.unwrap_err();
        assert_eq!(unknown.status.as_u16(), 404);
        let request =
            VoiceRequest { recipe: Some(recipe), anyway: true, ..VoiceRequest::card("own:c-arthur-arthur-omnivoice") };
        let Chosen::Pending(waiting) = shop.shop.choose(request, None).await.unwrap() else { panic!("not pending") };
        assert_eq!(waiting.wait_for.kind, "imprint");
        assert_eq!(lock(&lab.imprints).clone(), ["c-arthur-arthur-omnivoice"]);
        // Not there yet and no job running: the imprint did not come out.
        lock(&lab.made).push("c-arthur-arthur-omnivoice".into());
        shop.shop.try_pending().await;
        assert!(pending(&shop).is_none());
        assert_eq!(shop.shop.chosen_voice().unwrap().model_id, "own:c-arthur-arthur-omnivoice");
    }

    #[tokio::test]
    async fn a_new_silero_generation_replaces_the_other_and_the_choice_follows_it() {
        let world = world().await;
        let mut shop = open_shop(&world, Setup::default());
        let body = || futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"torch"))]);
        shop.shop.start().await.unwrap();
        shop.shop.place_silero(Some("v4_ru.pt"), body(), None).await.unwrap();
        assert!(world.models().join("silero/v4_ru.pt").exists());
        assert!(shop.events().contains(&"model.installed"));
        shop.shop.activate_tts(&json!({"modelId": "silero", "voiceId": "silero/xenia"}), None).await.unwrap();

        shop.shop.place_silero(Some("v5_ru.pt"), body(), None).await.unwrap();
        assert!(!world.models().join("silero").exists(), "one generation on disk");
        assert!(world.models().join("silero-v5/v5_ru.pt").exists());
        assert_eq!(shop.shop.chosen_voice().map(|chosen| chosen.voice_id), Some("silero-v5/xenia".into()));
        let stored = shop.shop.settings.get(&VOICE_CHOICE, None).map(Value::Object);
        assert_eq!(stored, Some(json!({"modelId": "silero-v5", "voiceId": "silero-v5/xenia"})));

        let unknown = shop.shop.place_silero(Some("evil.pt"), body(), None).await.unwrap_err();
        assert_eq!(unknown.message, "Ожидается файл Silero из каталога: v4_ru.pt, v5_ru.pt.");
        *lock(&shop.shop.silero_install) = Some("silero".into());
        let busy = shop.shop.place_silero(Some("v4_ru.pt"), body(), None).await.unwrap_err();
        assert_eq!(
            (busy.status.as_u16(), busy.message.as_str()),
            (409, "Модель Silero уже скачивается — дождитесь её.")
        );
        *lock(&shop.shop.silero_install) = None;
        let broken = futures::stream::iter([Err::<Bytes, _>(std::io::Error::other("the connection dropped"))]);
        let rejected = shop.shop.place_silero(Some("v4_ru.pt"), broken, None).await.unwrap_err();
        assert_eq!(rejected.message, "Файл не принят: the connection dropped");
        assert_eq!(*lock(&shop.shop.silero_install), None, "the slot is free again");
    }

    /// The folder's permissions back however the test ends, so the temporary folder can go.
    #[cfg(unix)]
    struct Writable(PathBuf);

    #[cfg(unix)]
    impl Drop for Writable {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_silero_install_that_cannot_clear_the_other_generation_says_why() {
        use std::os::unix::fs::PermissionsExt;
        let world = world().await;
        let shop = open_shop(&world, Setup::default());
        let body = || futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"torch"))]);
        shop.shop.start().await.unwrap();
        shop.shop.place_silero(Some("v4_ru.pt"), body(), None).await.unwrap();
        // v4's folder takes no changes: its file cannot go (EACCES).
        let v4 = world.models().join("silero");
        std::fs::set_permissions(&v4, std::fs::Permissions::from_mode(0o555)).unwrap();
        let _writable = Writable(v4.clone());
        if std::fs::write(v4.join("probe"), b"").is_ok() {
            return; // root: nothing is locked for it
        }
        let cause = format!("{}: Permission denied (os error 13)", v4.join("v4_ru.pt").display());

        let rejected = shop.shop.place_silero(Some("v5_ru.pt"), body(), None).await.unwrap_err();
        assert_eq!((rejected.status.as_u16(), rejected.message), (400, format!("Файл не принят: {cause}")));
        assert_eq!(*lock(&shop.shop.silero_install), None, "the slot is free again");

        // A download says the same on its card.
        let mut v5 = shop.shop.voice_model("silero-v5").await.unwrap().unwrap();
        v5.url = Some(format!("{}/voices/v5_ru.pt", world.base));
        shop.shop.catalog.save_voice_models(&[v5]).unwrap();
        *lock(&shop.shop.models) = None;
        shop.shop.install_tts("silero-v5", None).await.unwrap();
        let failed = settled(&shop.shop, "silero-v5").await.unwrap();
        assert_eq!((failed.state, failed.error), (DownloadPhase::Error, Some(cause)));
    }

    #[tokio::test(start_paused = true)]
    async fn a_silero_upload_that_stalls_frees_its_slot_and_its_half_file_by_the_deadline() {
        let world = world().await;
        let shop = open_shop(&world, Setup::default());
        let started = tokio::time::Instant::now();
        // A sender that went silent mid-file: no more bytes, no end.
        let stalled = tokio::spawn({
            let shop = shop.shop.clone();
            async move {
                let silent = futures::stream::pending::<Result<Bytes, std::io::Error>>();
                shop.place_silero(Some("v5_ru.pt"), silent, None).await
            }
        });
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_eq!(lock(&shop.shop.silero_install).as_deref(), Some("silero-v5"));
        let body = futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"torch"))]);
        let busy = shop.shop.place_silero(Some("v4_ru.pt"), body, None).await.unwrap_err();
        assert_eq!(busy.status.as_u16(), 409);

        let rejected = stalled.await.unwrap().unwrap_err();
        assert_eq!(
            (rejected.status.as_u16(), rejected.message.as_str()),
            (400, "Файл не принят: не пришёл целиком за 300 с")
        );
        assert!(started.elapsed() >= UPLOAD_DEADLINE);
        assert_eq!(*lock(&shop.shop.silero_install), None, "the slot is free again");
        assert!(!world.models().join("silero-v5").exists(), "no half file stays");
    }

    #[tokio::test]
    async fn notes_the_recognition_model_and_puts_it_to_work_one_restart_at_a_time() {
        let world = world().await;
        lock(&world.asr).slow = 30;
        let mut shop = open_shop(&world, Setup::default());
        shop.shop.start().await.unwrap();
        let noted = shop.shop.activate_asr("gigaam-v3-ctc", None).await.unwrap();
        assert!(noted.needs_restart);
        assert_eq!(shop.shop.settings.get(&ASR_MODEL, None).as_deref(), Some("gigaam-v3-ctc"));
        let unknown = shop.shop.activate_asr("nope", None).await.unwrap_err();
        assert_eq!((unknown.status.as_u16(), unknown.message.as_str()), (404, "no model nope"));

        let first = tokio::spawn({
            let shop = shop.shop.clone();
            async move { shop.restart_server(ServerName::Asr, None).await }
        });
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(shop.shop.view().await.unwrap().restarting, Some(ServerName::Asr));
        let other = shop.shop.restart_server(ServerName::Tts, None).await.unwrap_err();
        assert_eq!(other.message, "Перезапускается другой сервер — дождитесь его.");
        shop.shop.restart_server(ServerName::Asr, None).await.unwrap();
        first.await.unwrap().unwrap();
        assert_eq!(lock(&world.asr).loads, ["v3_ctc"], "the second click joined the first");
        assert!(shop.events().contains(&"model.server.restarted"));
        let view = shop.shop.view().await.unwrap();
        let card = view.asr.iter().find(|card| card.record.id == "gigaam-v3-ctc").unwrap();
        assert!(card.active && !card.pending);
        assert_eq!(view.restarting, None);
        assert_eq!(shop.shop.activate_asr("v3_ctc", None).await.unwrap(), Activated { needs_restart: false });
    }

    #[test]
    fn reads_the_registrys_names_and_pages() {
        let builtin = HashSet::from(["ru_RU-denis-medium.onnx".to_string()]);
        let mut seen = HashSet::new();
        let file = |path: &str| RegistryFile { path: path.into(), size: Some(0.0) };
        let voice =
            registry_record(&file("ar/ar_JO/kareem/x_low/ar_JO-kareem-x_low.onnx"), &mut seen, &builtin).unwrap();
        assert_eq!(voice.id, "piper-ar_JO-kareem-x_low");
        assert_eq!(voice.title, "Kareem · ar");
        assert_eq!(voice.description, "Лёгкое качество · ar_JO");
        assert_eq!(voice.size, None, "a size of 0 is no size");
        assert!(registry_record(&file("ar/ar_JO/kareem/x_low/ar_JO-kareem-x_low.onnx"), &mut seen, &builtin).is_none());
        assert!(registry_record(&file("ru/ru_RU/denis/medium/ru_RU-denis-medium.onnx"), &mut seen, &builtin).is_none());
        assert!(registry_record(&file("ru/ru_RU/x y/low/ru_RU-x y-low.onnx"), &mut seen, &builtin).is_none());
        assert_eq!(encode_uri_component("ru_RU-x y-ё.onnx"), "ru_RU-x%20y-%D1%91.onnx");
        assert_eq!(
            next_page(r#"<https://huggingface.co/api/next>; rel="next""#).as_deref(),
            Some("https://huggingface.co/api/next")
        );
        assert_eq!(next_page(""), None);
        assert_eq!(day_and_month("not a date"), "Invalid Date");
    }
}
