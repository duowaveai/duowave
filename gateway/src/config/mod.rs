//! The gateway's configuration: the environment (and `.env` files) read once at start into typed sections. The
//! variable names, defaults and meanings are the Node gateway's (`.env.example` documents them); a value that is
//! not one of the allowed ones stops the start with every problem listed, as Joi did.

pub mod data_dir;
mod env;

use std::path::PathBuf;

pub use env::Env;

use crate::skills::capabilities::{is_capability, CapabilityName};
use data_dir::data_path;

#[derive(Debug, Clone)]
pub struct Config {
    pub app: AppConfig,
    pub gateway: GatewayConfig,
    pub speech: SpeechConfig,
    pub dialog: DialogConfig,
    pub skills: SkillsConfig,
    pub calls: CallsConfig,
    pub operations: OperationsConfig,
    pub voice_lab: VoiceLabConfig,
    pub updates: UpdatesConfig,
    pub models: ModelsConfig,
    /// Where the gateway runs (src/placement): the station's build, the home server, the pair's key.
    pub placement: PlacementConfig,
    /// The connectors the panel configures, as the environment gave them: their first values only.
    pub connectors: ConnectorsSeed,
    /// The plugins' packages and their limits (src/plugins).
    pub plugins: PluginsConfig,
    /// DATA_DIR, when set: the one folder for everything written.
    pub data_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct PluginsConfig {
    /// PACKAGES_DIR: the packages of every kind — plugins, apps, characters, themes — as `<id>/<version>/` with a
    /// manifest.json ($DATA_DIR/packages; data/packages without it).
    pub packages_dir: PathBuf,
    /// PLUGINS_DISABLED=1: the safe mode — the gateway runs without any plugin, whatever is switched on.
    pub disabled: bool,
    /// PLUGINS_IDLE_UNLOAD_S: a plugin's code leaves memory after this long without a call; the next call loads it
    /// again. Ten minutes on a computer, two on the station.
    pub idle_unload_s: u64,
    /// PACKAGES_FEED_URL: the packages' catalog (`catalog.json`) — our own built in at release
    /// (DUOWAVE_PACKAGES_FEED), another one here; `off`: the gateway never asks the network for it.
    pub feed_url: Option<String>,
    /// The catalog's last good copy and its previews ($DATA_DIR/packages-catalog; data/packages-catalog without it).
    pub catalog_dir: PathBuf,
    /// The workshop's drafts ($DATA_DIR/packages-dev; data/packages-dev without it): a plugin or a theme being written,
    /// one folder per id, installed into PACKAGES_DIR as a new version by the workshop's «Поставить».
    pub dev_dir: PathBuf,
    /// PACKAGES_PUBKEY: the catalog's signing key (minisign), else the one built in at release
    /// (DUOWAVE_PACKAGES_PUBKEY). With it and a valid `catalog.json.minisig`, plugins and apps install from the
    /// catalog (docs/plans/plugins.md, stage 8); without, only packages without code do.
    pub catalog_key: Option<String>,
    /// PLUGINS_RESERVE_MB: the memory kept for the system and Alice, out of the plugins' budget (docs/plans/plugins.md,
    /// «Бюджет»; 40 MB until it is measured under load — the owner's open decision 2).
    pub reserve_mb: u64,
    /// PLUGINS_STORAGE_RESERVE_MB: the room on the disk kept for the gateway's own update (`.previous` is 12 MB).
    pub storage_reserve_mb: u64,
    /// PLUGINS_PRESSURE_ACTION: `disable` (default: the plugin switched on last goes off at «critical» on the station)
    /// or `warn` (only the log and the panel) — the owner's open decision 3.
    pub pressure_action: crate::plugins::budget::PressureAction,
    /// PLUGINS_BUNDLED: the packages built into the gateway (packages/bundled.json) are laid out into PACKAGES_DIR at
    /// the start, on the first time switched on; `0` leaves them out and the built-in skills answer alone. Off in tests
    /// without a data folder of their own (`Config::for_tests`), which would lay them out into the working copy.
    pub bundled: bool,
}

/// The gateway's port when PORT is not set: an uncommon one, so it is free on most computers (80, 3000, 8080 rarely
/// are). The station dials whatever port its uniProxyUrl names, so nothing needs 80.
pub const DEFAULT_PORT: u16 = 7780;

#[derive(Debug, Clone)]
pub struct AppConfig {
    /// NODE_ENV (kept for the name): development, production.
    pub env: String,
    pub port: u16,
    /// The port the station dials, when it is not PORT: Docker publishes the container's PORT on another port of the
    /// host (compose.yaml's GATEWAY_PORT), and the gateway cannot see that from inside (PUBLIC_PORT).
    pub public_port: Option<u16>,
    /// A second listener with TLS (HTTPS_PORT, TLS_CERT_FILE, TLS_KEY_FILE): the call page from a phone needs it.
    pub tls: Option<TlsConfig>,
    /// The hosts a request may name besides IPs and local names: ALLOWED_HOSTS and the public URLs' hosts.
    pub allowed_hosts: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TlsConfig {
    pub port: u16,
    pub cert_file: PathBuf,
    pub key_file: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UniproxyMode {
    /// We answer the station: recognition, dialog, speech.
    Gateway,
    /// The station's session goes to Yandex; every frame is journaled.
    Relay,
}

impl UniproxyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            UniproxyMode::Gateway => "gateway",
            UniproxyMode::Relay => "relay",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StationPairing {
    /// The first station is trusted, later ones wait for the admin.
    Tofu,
    Closed,
    Open,
}

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub uniproxy_path: String,
    pub mode: UniproxyMode,
    /// Yandex's uniproxy: our speech session in gateway mode, the station's whole session in relay mode.
    pub yandex_url: String,
    /// Relay mode: ping Yandex this often and cut a connection that has not answered by the next ping; 0 = off.
    pub relay_ping_ms: u64,
    /// Our own session with Yandex: ping it this often and reopen it when silent until the next ping; 0 = off.
    pub yandex_ping_ms: u64,
    /// The station's own socket, in both modes: ping it this often, and cut it once the station has sent nothing at
    /// all (its own pings and the pongs to ours count) for three of these in a row — a connection left half-open by
    /// a reboot or a lost network, which otherwise stays «connected» until we happen to write into it; 0 = off.
    pub station_ping_ms: u64,
    /// Yandex's pushes go to the station (pass) or are only journaled (drop).
    pub yandex_pushes_pass: bool,
    /// For Yandex's response on the Yandex route, then between the frames of its speech.
    pub yandex_timeout_ms: u64,
    /// Wall-clock safety net per voice request; VAD normally ends it much earlier.
    pub max_utterance_ms: u64,
    /// How long, from its start, an answer may keep the station waiting for its speech (silence keeps the station
    /// from giving up); then its stream closes and its words follow later, through our push.
    pub answer_wait_max_ms: u64,
    /// How long after its start the words of such a deferred answer are still said; later they are dropped.
    pub deferred_max_ms: u64,
    /// Interim AsrResult while the station records; 0 turns them off.
    pub partial_interval_ms: u64,
    /// Recognise the command so far at most once per this much new audio; 0 turns it off.
    pub interim_interval_ms: u64,
    /// Speech VAD does not hear: recognise all audio after the wake word, on the fly and once more at the end.
    pub quiet_speech: bool,
    /// Quiet speech ends when its recognised text has not changed for this much audio.
    pub quiet_end_ms: u64,
    pub recorder: RecorderConfig,
    pub debug_api: bool,
    /// The home networks (CIDR, "private"): the debug API, the station's HTTP, the first-run setup.
    pub debug_api_allow: Vec<String>,
    /// PANEL_REMOTE: the panel's sign-in and API answer from outside the home too.
    pub panel_remote: bool,
    pub station_pairing: StationPairing,
    /// How the gateway reaches a station's shell to connect it from the panel.
    pub station_ssh: StationSsh,
    pub session_events_limit: usize,
    pub tts: TtsConfig,
}

#[derive(Debug, Clone)]
pub struct StationSsh {
    pub port: u16,
    pub user: String,
    pub password: String,
}

#[derive(Debug, Clone)]
pub struct RecorderConfig {
    pub enabled: bool,
    pub dir: PathBuf,
    pub audio: bool,
    pub redact: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsProvider {
    Yandex,
    Mixed,
    Own,
    None,
}

impl TtsProvider {
    pub fn parse(value: &str) -> Option<TtsProvider> {
        match value {
            "yandex" => Some(TtsProvider::Yandex),
            "mixed" => Some(TtsProvider::Mixed),
            "own" => Some(TtsProvider::Own),
            "none" => Some(TtsProvider::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TtsProvider::Yandex => "yandex",
            TtsProvider::Mixed => "mixed",
            TtsProvider::Own => "own",
            TtsProvider::None => "none",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TtsConfig {
    /// Whose voice speaks in gateway mode (relay mode: Yandex's, always).
    pub provider: TtsProvider,
    /// Our engine, when configured (TTS_OWN_URL and the rest).
    pub own: Option<OwnSpeechConfig>,
    /// Empty: the session's default voice, which is Alice.
    pub voice: Option<String>,
    pub lang: String,
    /// For the connection and for each phrase: until audio starts, and between audio frames.
    pub timeout_ms: u64,
    /// Synthesis markup in our speech: on, it goes to Yandex's synthesis; off, it is taken out.
    pub markup: bool,
}

/// An OpenAI-compatible /audio/speech: tts-server/, OpenAI, …
#[derive(Debug, Clone)]
pub struct OwnSpeechConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub voice: String,
    /// wav or opus.
    pub format: String,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone)]
pub struct SpeechConfig {
    pub vad: VadConfig,
    pub endpoint: EndpointConfig,
    pub gain: GainConfig,
    pub asr: AsrConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadEngine {
    Silero,
    Energy,
}

#[derive(Debug, Clone)]
pub struct VadConfig {
    pub engine: VadEngine,
    pub model_path: PathBuf,
    pub energy_threshold_dbfs: f64,
}

#[derive(Debug, Clone)]
pub struct EndpointConfig {
    /// Speech probability at or above which a window is speech.
    pub threshold: f64,
    /// Below this a window is silence; in between it neither starts speech nor breaks silence.
    pub neg_threshold: f64,
    /// After SpotterEnd: the wake word still sounds this long past the mark.
    pub guard_ms: u64,
    pub speech_start_ms: u64,
    pub end_silence_ms: u64,
    /// No command this long after SpotterEnd: end with an empty result.
    pub no_speech_ms: u64,
    pub max_speech_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GainMode {
    Auto,
    Fixed,
}

#[derive(Debug, Clone)]
pub struct GainConfig {
    pub mode: GainMode,
    pub fixed_db: f64,
    pub max_db: f64,
    /// Auto mode: level of the loudest 32 ms window of the wake word after gain, dBFS RMS.
    pub target_dbfs: f64,
}

#[derive(Debug, Clone)]
pub struct AsrConfig {
    /// openai (any OpenAI-compatible /audio/transcriptions) or none.
    pub enabled: bool,
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub timeout_ms: u64,
    pub wake_words: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogProvider {
    Anthropic,
    OpenAi,
    Echo,
    None,
}

impl DialogProvider {
    pub fn parse(value: &str) -> Option<DialogProvider> {
        match value {
            "anthropic" => Some(DialogProvider::Anthropic),
            "openai" => Some(DialogProvider::OpenAi),
            "echo" => Some(DialogProvider::Echo),
            "none" => Some(DialogProvider::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DialogProvider::Anthropic => "anthropic",
            DialogProvider::OpenAi => "openai",
            DialogProvider::Echo => "echo",
            DialogProvider::None => "none",
        }
    }
}

pub const LLM_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// fast: short replies; smart: hard questions and the choice of a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelProfile {
    Fast,
    Smart,
}

/// When the station keeps listening without the wake word after an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenMode {
    Question,
    Always,
    Never,
}

#[derive(Debug, Clone)]
pub struct ModelPair {
    pub fast: String,
    pub smart: String,
}

#[derive(Debug, Clone)]
pub struct DialogConfig {
    pub provider: DialogProvider,
    /// LLM_MODEL: the model of both profiles unless they name their own.
    pub model: String,
    pub models: ModelPair,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    /// What Claude's SDK read from the environment itself, for the .env model only (never a panel provider):
    /// ANTHROPIC_API_KEY, else ANTHROPIC_AUTH_TOKEN (both go as the bearer token), and ANTHROPIC_BASE_URL.
    pub anthropic_key: Option<String>,
    pub anthropic_base_url: Option<String>,
    pub max_tokens: u32,
    /// Claude only: thinking depth (low, medium, high, xhigh, max).
    pub effort: String,
    /// Until the first phrase of the answer; the station gives up on a response after ~15 s.
    pub timeout_ms: u64,
    /// Read on every question, so edits apply without a restart. Empty: the built-in prompt.
    pub system_prompt_file: Option<PathBuf>,
    pub history_turns: usize,
    pub history_ttl_ms: u64,
    pub listen: ListenMode,
    pub early_response: bool,
    pub jingle: bool,
    pub hold_response_ms: u64,
    pub whisper_reply: bool,
}

#[derive(Debug, Clone)]
pub struct SkillsConfig {
    /// ROUTE_DEFAULT: own, yandex, off or a list (own,yandex).
    pub route_default: String,
    /// ROUTES: exceptions per capability, `radio=yandex;timers=own`.
    pub routes: String,
    pub unsupported_phrase: String,
    /// A Yandex dialog or our game takes follow-ups for this long after its last answer.
    pub continuation_ms: u64,
    /// Capabilities whose requests go to Yandex as the station's voice, not as our text.
    pub yandex_voice: Vec<CapabilityName>,
    /// The database, its key and the stored state; None keeps everything in memory (STATE_DIR set empty).
    pub state_dir: Option<PathBuf>,
    /// Our content, served at /media: radio.json, music/, tales/.
    pub content_dir: PathBuf,
    /// How the station reaches /media; None: the host it connects to for /uni.ws.
    pub media_base_url: Option<String>,
    /// MEDIA_TRANSCODE_ON_STATION=1: the gateway on the station may itself turn an AAC stream into MP3 for the
    /// station's player — the gateway's fallback for streams the station keeps silent on (docs/plans/plugins.md,
    /// decision 9, stage 5а; the station plays MP3 and HLS, not AAC, checked by voice 07.11.2026).
    ///
    /// Off by default, and only a guess so far: decoding AAC and encoding MP3 in real time may load the station's weak
    /// processor and compete with Alice's own voice processing (echo cancelling, the spotter). Off, the station's
    /// gateway leaves transcoding to the computer (its home server) or skips the AAC station. Passing a stream through
    /// unchanged is cheap (tens of KB/s) and needs no flag. On a computer the flag means nothing: it may always
    /// transcode.
    ///
    /// For the panel: when transcoding exists, a `media.transcode` row in core/availability.rs reads this flag, and
    /// «Что доступно» says «на колонке: нет — большая нагрузка» while it is off.
    /// To decide: measure the station's processor and memory while it plays an AAC stream through its gateway.
    pub transcode_on_station: bool,
    /// The forecast's services; the usual place is the panel's (src/connectors).
    pub weather: WeatherConfig,
}

#[derive(Debug, Clone)]
pub struct WeatherConfig {
    pub forecast_url: String,
    pub geocode_url: String,
}

/// Home Assistant as the skills and the checks use it: set up in the panel (src/connectors).
#[derive(Clone, PartialEq, Eq)]
pub struct HomeAssistantConfig {
    pub url: String,
    pub token: String,
}

impl std::fmt::Debug for HomeAssistantConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HomeAssistantConfig").field("url", &self.url).field("token", &Hidden).finish()
    }
}

/// What a secret prints as: a log line or a panic never carries one.
struct Hidden;

impl std::fmt::Debug for Hidden {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("•••")
    }
}

#[derive(Debug, Clone)]
pub struct CallsConfig {
    pub station_name: String,
    /// CALLS_STATION_MIC_PORT: where station clients bridge the station's audio service; None: none.
    pub station_mic_port: Option<u16>,
    /// Where the bot keeps the voice messages it receives (content/messages). The bot and the bridges themselves are
    /// set up in the panel (src/connectors).
    pub telegram_inbox: PathBuf,
}

/// The SIP bridge (Asterisk ARI) as the calls use it: set up in the panel (src/connectors).
#[derive(Clone, PartialEq, Eq)]
pub struct SipConfig {
    pub url: String,
    pub user: String,
    pub password: String,
    pub app: String,
    pub rtp_host: String,
    pub rtp_port: u16,
    /// Names (lowercase) to numbers.
    pub contacts: Contacts,
}

impl std::fmt::Debug for SipConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SipConfig")
            .field("url", &self.url)
            .field("user", &self.user)
            .field("password", &Hidden)
            .field("app", &self.app)
            .field("rtp_host", &self.rtp_host)
            .field("rtp_port", &self.rtp_port)
            .field("contacts", &self.contacts.len())
            .finish()
    }
}

/// Names (lowercase) to numbers or chat ids, in the order the variable lists them, as the TS Record kept them:
/// lookups take the first that fits.
pub type Contacts = Vec<(String, String)>;

/// The Telegram bot as the calls and the alerts use it: set up in the panel (src/connectors).
#[derive(Clone, PartialEq, Eq)]
pub struct TelegramConfig {
    pub token: String,
    /// Names (lowercase) to chat ids.
    pub contacts: Contacts,
    pub inbox_dir: PathBuf,
    /// Where the station watch sends its alerts: a contact's chat or any other.
    pub alert_chat: Option<String>,
}

impl std::fmt::Debug for TelegramConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramConfig")
            .field("token", &Hidden)
            .field("contacts", &self.contacts.len())
            .field("inbox_dir", &self.inbox_dir)
            .field("alert_chat", &self.alert_chat.as_ref().map(|_| Hidden))
            .finish()
    }
}

/// The LiveKit room as the calls use it: set up in the panel (src/connectors).
#[derive(Clone, PartialEq, Eq)]
pub struct LiveKitConfig {
    pub url: String,
    pub public_url: Option<String>,
    pub page_url: Option<String>,
    pub api_key: String,
    pub api_secret: String,
    pub room: String,
}

impl std::fmt::Debug for LiveKitConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveKitConfig")
            .field("url", &self.url)
            .field("public_url", &self.public_url)
            .field("page_url", &self.page_url)
            .field("api_key", &Hidden)
            .field("api_secret", &Hidden)
            .field("room", &self.room)
            .finish()
    }
}

/// The connectors the panel configures (src/connectors), as the environment gave them. Only their first values: at
/// a start the database has no row for one (a fresh install, the first start of this version), its values here move
/// in once; after that the panel's win and these are not read. Nothing else reads this.
#[derive(Debug, Clone, Default)]
pub struct ConnectorsSeed {
    pub home_assistant: Option<HomeAssistantConfig>,
    /// TELEGRAM_BOT_TOKEN and TELEGRAM_CONTACTS; ALERT_TELEGRAM_CHAT as its alert chat.
    pub telegram: Option<TelegramConfig>,
    pub livekit: Option<LiveKitConfig>,
    pub sip: Option<SipConfig>,
    /// WEATHER_LOCATION.
    pub weather_location: Option<String>,
    /// NEWS_FEEDS: RSS or Atom feeds.
    pub news_feeds: Vec<String>,
    /// The connectors' variables set in the environment: the log and the panel say which lines are left to remove.
    pub set: Vec<&'static str>,
}

/// The variables of the connectors the panel configures: the seed reads them, the panel names those left.
pub const CONNECTOR_VARIABLES: &[&str] = &[
    "HA_URL",
    "HA_TOKEN",
    "TELEGRAM_BOT_TOKEN",
    "TELEGRAM_CONTACTS",
    "ALERT_TELEGRAM_CHAT",
    "LIVEKIT_URL",
    "LIVEKIT_PUBLIC_URL",
    "LIVEKIT_PAGE_URL",
    "LIVEKIT_API_KEY",
    "LIVEKIT_API_SECRET",
    "LIVEKIT_ROOM",
    "ARI_URL",
    "ARI_USER",
    "ARI_PASSWORD",
    "ARI_APP",
    "SIP_RTP_HOST",
    "SIP_RTP_PORT",
    "SIP_CONTACTS",
    "WEATHER_LOCATION",
    "NEWS_FEEDS",
];

#[derive(Debug, Clone)]
pub struct OperationsConfig {
    /// STATION_OFFLINE_ALERT_MIN: tell when no station has been connected this long; 0 turns it off.
    pub offline_alert_ms: u64,
    /// A macOS notification on this Mac.
    pub notify: bool,
    pub webhook_url: Option<String>,
    /// RECORDER_RETENTION_DAYS: days of journals and audio kept; 0 keeps everything. Two on the station.
    pub retention_days: u64,
    /// RECORDER_MAX_MB: the whole journal at most this big, in bytes; 0 is no cap. 10 MB on the station, whose
    /// /data has some 60 MB (src/operations/retention.rs).
    pub journal_cap: u64,
}

#[derive(Debug, Clone)]
pub struct VoiceLabConfig {
    /// VOICE_LAB_DIR: the takes and the voices — people's voices, never in git.
    pub dir: PathBuf,
    /// VOICE_LAB_RUNTIME_DIR: the engines' environments and weights.
    pub runtime_dir: PathBuf,
    /// clone-server: the voice server and the imprint job.
    pub server_dir: PathBuf,
    pub bench_dir: Option<PathBuf>,
    pub timeout_ms: u64,
    pub reserve_mb: Option<u64>,
    pub servers: CloneServers,
}

#[derive(Debug, Clone)]
pub struct CloneServers {
    pub omnivoice: String,
    pub cosyvoice3: String,
    pub voxcpm: String,
}

/// New in the Rust gateway: where it learns of a new version (src/updates).
#[derive(Debug, Clone)]
pub struct UpdatesConfig {
    /// UPDATE_FEED_URL: the release's latest.json; None: no checks (the build's own feed when it has one).
    pub feed_url: Option<String>,
    /// UPDATE_CHECK_HOURS: how often to look (hourly by default: releases come several a day); 0 checks only when asked.
    pub check_hours: u64,
    /// UPDATE_PUBKEY: the minisign keys updates are signed with (one per line), when not those built in.
    pub public_key: Option<String>,
    /// UPDATE_AUTO_CRITICAL: an urgent release (the feed's critical_below) installs by itself, without «Обновить»,
    /// where the gateway installs its own updates; false leaves it to a click like any other. On by default.
    pub auto_critical: bool,
}

/// Where the gateway runs (src/placement, docs/plans/station-placement.md).
#[derive(Debug, Clone, Default)]
pub struct PlacementConfig {
    /// STATION_PAYLOAD_FILE: the station's build (duowave-station-aarch64.tar.gz, its .sig beside it) when this
    /// computer's build does not carry one.
    pub station_payload_file: Option<PathBuf>,
    /// HOME_SERVER_URL, HOME_SERVER_KEY: the station's gateway leans on the computer at this address with this key
    /// (the first values of the «Домашний сервер» connector; the panel changes them).
    pub home_server_url: Option<String>,
    pub home_server_key: Option<String>,
    /// PEER_KEY: the key the computer reaches the station's gateway with (the station's .env, written at install).
    pub peer_key: Option<String>,
    /// DUOWAVE_MANAGED: who installs this gateway's updates — desktop, docker, home-assistant, pc, yam, yse.
    pub managed_by: Option<String>,
    /// DUOWAVE_ROLE=station: a computer's build plays the station's gateway (its role, its placement) — to try the
    /// pair on one computer and in the tests. Unset: the build's own.
    pub role: Option<String>,
}

/// The local models (src/models): where the voices the gateway downloads go.
#[derive(Debug, Clone)]
pub struct ModelsConfig {
    /// TTS_MODELS_DIR: the voices' folder, the one the voice server reads ($DATA_DIR/voices; beside the working copy
    /// tts-server/models).
    pub tts_dir: PathBuf,
    /// MAC_DEPLOY_DIR: the macOS run scripts' folder (deploy/macos), for restarting the model servers when the
    /// gateway itself runs from elsewhere than the working copy — the desktop app's bundle. None: the working
    /// folder's own deploy/macos.
    pub mac_deploy_dir: Option<PathBuf>,
    /// The open Piper registry the voice catalog fills from: every voice of rhasspy/piper-voices on HuggingFace, a
    /// page of the tree at a time (no variable: tests give their own).
    pub piper_registry: String,
}

#[derive(Debug, thiserror::Error)]
#[error("configuration: {}", .problems.join("; "))]
pub struct ConfigError {
    pub problems: Vec<String>,
}

impl Config {
    /// The process environment and the .env files.
    pub fn from_process() -> Result<Config, ConfigError> {
        Config::load(&Env::from_process())
    }

    /// The configuration from these variables, or every value that is not allowed.
    pub fn load(env: &Env) -> Result<Config, ConfigError> {
        let problems = validate(env);
        if !problems.is_empty() {
            return Err(ConfigError { problems });
        }
        Ok(build(env))
    }

    /// The defaults with these variables on top: tests. Never fails; invalid values fall back. A test without a data
    /// folder of its own (no DATA_DIR, no PACKAGES_DIR) gets no built-in packages unless it asks (PLUGINS_BUNDLED=1):
    /// they would go into the working copy's data/packages, shared by every test running beside it.
    pub fn for_tests(env: &Env) -> Config {
        let mut config = build(env);
        if env.get("DATA_DIR").is_none() && env.get("PACKAGES_DIR").is_none() {
            config.plugins.bundled = env.flag("PLUGINS_BUNDLED", false);
        }
        config
    }
}

fn build(env: &Env) -> Config {
    let content_dir = env.get("CONTENT_DIR").map(PathBuf::from).unwrap_or_else(|| data_path(env, "content", "content"));
    let mac_deploy = env.get("MAC_DEPLOY_DIR").map(str::trim).filter(|dir| !dir.is_empty()).map(PathBuf::from);
    let threshold = env.number("VAD_THRESHOLD", 0.5);
    let media_base_url =
        env.get("MEDIA_BASE_URL").map(|url| url.trim_end_matches('/').to_string()).filter(|url| !url.is_empty());
    let mut allowed_hosts: Vec<String> =
        env.list("ALLOWED_HOSTS", &[]).into_iter().map(|name| name.to_ascii_lowercase()).collect();
    // LiveKit's public and page hosts are the panel's (src/connectors): the Host check takes them as they change.
    if let Some(host) = env.get("MEDIA_BASE_URL").and_then(host_of_url) {
        allowed_hosts.push(host);
    }
    Config {
        data_dir: env.get("DATA_DIR").map(str::trim).filter(|root| !root.is_empty()).map(PathBuf::from),
        app: AppConfig {
            env: env.string("NODE_ENV", "development"),
            port: env
                .get("PORT")
                .and_then(|port| port.trim().parse().ok())
                .filter(|port| *port > 0)
                .unwrap_or(DEFAULT_PORT),
            public_port: env.get("PUBLIC_PORT").and_then(|port| port.trim().parse().ok()).filter(|port| *port > 0),
            tls: match (env.get("TLS_CERT_FILE"), env.get("TLS_KEY_FILE")) {
                (Some(cert), Some(key)) => Some(TlsConfig {
                    port: env.integer("HTTPS_PORT", 443) as u16,
                    cert_file: PathBuf::from(cert),
                    key_file: PathBuf::from(key),
                }),
                _ => None,
            },
            allowed_hosts,
        },
        gateway: GatewayConfig {
            uniproxy_path: socket_path(&env.string("UNIPROXY_PATH", "/uni.ws")),
            mode: if env.get("UNIPROXY_MODE") == Some("gateway") { UniproxyMode::Gateway } else { UniproxyMode::Relay },
            yandex_url: env.string("YANDEX_UNIPROXY_URL", "wss://uniproxy.alice.yandex.net/uni.ws"),
            relay_ping_ms: env.integer("RELAY_PING_MS", 15_000),
            yandex_ping_ms: env.integer("YANDEX_PING_MS", 15_000),
            station_ping_ms: env.integer("STATION_PING_MS", 15_000),
            yandex_pushes_pass: env.get("YANDEX_PUSHES") != Some("drop"),
            yandex_timeout_ms: env.integer("YANDEX_TIMEOUT_MS", 10_000),
            max_utterance_ms: env.integer("VOICE_MAX_UTTERANCE_MS", 20_000),
            answer_wait_max_ms: env.integer("ANSWER_WAIT_MAX_MS", 45_000),
            deferred_max_ms: env.integer("DEFERRED_MAX_MS", 300_000),
            partial_interval_ms: env.integer("ASR_PARTIAL_INTERVAL_MS", 300),
            interim_interval_ms: env.integer("ASR_INTERIM_INTERVAL_MS", 500),
            quiet_speech: env.flag("ASR_QUIET_SPEECH", true),
            quiet_end_ms: env.integer("ASR_QUIET_END_MS", 2000),
            recorder: RecorderConfig {
                enabled: env.flag("RECORDER_ENABLED", true),
                dir: env
                    .get("RECORDER_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| data_path(env, "sessions", "data/sessions")),
                audio: env.flag("RECORDER_AUDIO", false),
                redact: env.flag("RECORDER_REDACT", true),
            },
            debug_api: env.flag("DEBUG_API_ENABLED", false),
            debug_api_allow: env.list("DEBUG_API_ALLOW", &["private"]),
            panel_remote: env.flag("PANEL_REMOTE", false),
            station_pairing: match env.get("STATION_PAIRING") {
                Some("open") => StationPairing::Open,
                Some("closed") => StationPairing::Closed,
                _ => StationPairing::Tofu,
            },
            station_ssh: StationSsh {
                port: env.integer("STATION_SSH_PORT", 2222) as u16,
                user: env.string("STATION_SSH_USER", "root"),
                password: env.raw("STATION_SSH_PASSWORD").unwrap_or("").to_string(),
            },
            session_events_limit: env
                .get("SESSION_EVENTS_LIMIT")
                .and_then(|n| n.parse().ok())
                .filter(|n| *n > 0)
                .unwrap_or(500),
            tts: TtsConfig {
                provider: env.get("TTS_PROVIDER").and_then(TtsProvider::parse).unwrap_or(TtsProvider::Yandex),
                // Без TTS_OWN_URL свой голос «не настроен» — но там, где лежат mac-скрипты запуска
                // (MAC_DEPLOY_DIR), сервер стартует из пульта, и он считается настроенным на их
                // фиксированный порт: у новой установки флоу не упирается в непрописанную переменную.
                own: env.get("TTS_OWN_URL").or_else(|| mac_deploy.as_ref().map(|_| "http://127.0.0.1:8091/v1")).map(
                    |url| OwnSpeechConfig {
                        base_url: url.to_string(),
                        api_key: env.optional("TTS_OWN_API_KEY"),
                        model: env.string("TTS_OWN_MODEL", "silero"),
                        voice: env.string("TTS_OWN_VOICE", "xenia"),
                        format: if env.get("TTS_OWN_FORMAT") == Some("opus") { "opus".into() } else { "wav".into() },
                        timeout_ms: env.integer("TTS_OWN_TIMEOUT_MS", 8000),
                    },
                ),
                voice: env.optional("TTS_VOICE"),
                lang: env.string("TTS_LANG", "ru-RU"),
                timeout_ms: env.integer("TTS_TIMEOUT_MS", 5000),
                markup: env.flag("TTS_MARKUP", false),
            },
        },
        dialog: dialog(env),
        skills: SkillsConfig {
            route_default: env.string("ROUTE_DEFAULT", "own,yandex"),
            routes: env.raw("ROUTES").unwrap_or("").to_string(),
            unsupported_phrase: env.string("UNSUPPORTED_PHRASE", "Этого я пока не умею."),
            continuation_ms: env.integer("DIALOG_CONTINUATION_S", 120) * 1000,
            yandex_voice: env.list("YANDEX_VOICE", &[]).iter().filter_map(|name| is_capability(name)).collect(),
            state_dir: match env.raw("STATE_DIR") {
                None => Some(data_path(env, "state", "data/state")),
                Some("") => None,
                Some(dir) => Some(PathBuf::from(dir)),
            },
            content_dir: content_dir.clone(),
            media_base_url,
            transcode_on_station: env.flag("MEDIA_TRANSCODE_ON_STATION", false),
            weather: WeatherConfig {
                forecast_url: env.string("WEATHER_FORECAST_URL", "https://api.open-meteo.com/v1/forecast"),
                geocode_url: env.string("WEATHER_GEOCODE_URL", "https://geocoding-api.open-meteo.com/v1/search"),
            },
        },
        connectors: connectors_seed(env, &content_dir),
        plugins: PluginsConfig {
            packages_dir: env
                .get("PACKAGES_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| data_path(env, "packages", "data/packages")),
            disabled: env.flag("PLUGINS_DISABLED", false),
            idle_unload_s: env.integer("PLUGINS_IDLE_UNLOAD_S", if cfg!(feature = "station") { 120 } else { 600 }),
            feed_url: env.optional("PACKAGES_FEED_URL").map(|url| url.trim().to_string()),
            catalog_dir: data_path(env, "packages-catalog", "data/packages-catalog"),
            dev_dir: data_path(env, "packages-dev", "data/packages-dev"),
            catalog_key: env
                .optional("PACKAGES_PUBKEY")
                .or_else(|| option_env!("DUOWAVE_PACKAGES_PUBKEY").map(str::to_string))
                .filter(|key| !key.trim().is_empty()),
            reserve_mb: env.integer("PLUGINS_RESERVE_MB", 40),
            storage_reserve_mb: env.integer("PLUGINS_STORAGE_RESERVE_MB", 15),
            pressure_action: env
                .optional("PLUGINS_PRESSURE_ACTION")
                .and_then(|action| crate::plugins::budget::PressureAction::parse(&action))
                .unwrap_or(crate::plugins::budget::PressureAction::Disable),
            bundled: env.flag("PLUGINS_BUNDLED", true),
        },
        voice_lab: VoiceLabConfig {
            dir: env
                .get("VOICE_LAB_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| data_path(env, "voice-lab", "data/voice-lab")),
            runtime_dir: env
                .get("VOICE_LAB_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| data_path(env, "clone-runtime", "clone-server/runtime")),
            server_dir: PathBuf::from("clone-server"),
            bench_dir: match env.raw("VOICE_LAB_BENCH_DIR") {
                Some("") => None,
                Some(dir) => Some(PathBuf::from(dir)),
                None => Some(PathBuf::from("../self-voice-tests")),
            },
            timeout_ms: env.integer("CLONE_TIMEOUT_MS", 30_000),
            reserve_mb: env.get("VOICE_LAB_RESERVE_MB").map(|_| env.integer("VOICE_LAB_RESERVE_MB", 0)),
            servers: clone_servers(env.raw("VOICE_LAB_SERVER_URLS").unwrap_or("")),
        },
        operations: OperationsConfig {
            offline_alert_ms: env.integer("STATION_OFFLINE_ALERT_MIN", 5) * 60_000,
            notify: env.flag("ALERT_MACOS_NOTIFY", cfg!(target_os = "macos")),
            webhook_url: env.optional("ALERT_WEBHOOK_URL"),
            retention_days: env.integer("RECORDER_RETENTION_DAYS", if cfg!(feature = "station") { 2 } else { 7 }),
            journal_cap: env.integer("RECORDER_MAX_MB", if cfg!(feature = "station") { 10 } else { 0 }) * 1_048_576,
        },
        calls: CallsConfig {
            station_name: env.string("CALLS_STATION_NAME", "Колонка"),
            station_mic_port: match env.raw("CALLS_STATION_MIC_PORT") {
                Some("") => None,
                _ => Some(env.integer("CALLS_STATION_MIC_PORT", 9851) as u16),
            },
            telegram_inbox: content_dir.join("messages"),
        },
        speech: SpeechConfig {
            vad: VadConfig {
                engine: if env.get("VAD_ENGINE") == Some("energy") { VadEngine::Energy } else { VadEngine::Silero },
                model_path: PathBuf::from(env.string("VAD_MODEL_PATH", "assets/silero_vad.onnx")),
                energy_threshold_dbfs: env.number("VAD_ENERGY_THRESHOLD_DBFS", -40.0),
            },
            endpoint: EndpointConfig {
                threshold,
                neg_threshold: (threshold - 0.15).max(0.01),
                guard_ms: env.integer("VAD_GUARD_MS", 320),
                speech_start_ms: env.integer("VAD_SPEECH_START_MS", 96),
                end_silence_ms: env.integer("VAD_END_SILENCE_MS", 700),
                no_speech_ms: env.integer("VAD_NO_SPEECH_MS", 5000),
                max_speech_ms: env.integer("VAD_MAX_SPEECH_MS", 15_000),
            },
            gain: gain(env),
            asr: AsrConfig {
                enabled: env.get("ASR_PROVIDER") != Some("none"),
                base_url: env.string("ASR_BASE_URL", "http://127.0.0.1:8090/v1"),
                api_key: env.optional("ASR_API_KEY"),
                model: env.string("ASR_MODEL", "gigaam-v3"),
                language: match env.raw("ASR_LANGUAGE") {
                    None => Some("ru".into()),
                    Some("") => None,
                    Some(language) => Some(language.to_string()),
                },
                prompt: env.optional("ASR_PROMPT"),
                timeout_ms: env.integer("ASR_TIMEOUT_MS", 8000),
                wake_words: env.list("WAKE_WORDS", &["алиса", "яндекс"]),
            },
        },
        updates: UpdatesConfig {
            feed_url: env.optional("UPDATE_FEED_URL"),
            check_hours: env.integer("UPDATE_CHECK_HOURS", 1),
            public_key: env.optional("UPDATE_PUBKEY"),
            auto_critical: env.flag("UPDATE_AUTO_CRITICAL", true),
        },
        placement: PlacementConfig {
            station_payload_file: env.get("STATION_PAYLOAD_FILE").map(PathBuf::from),
            home_server_url: env.optional("HOME_SERVER_URL").map(|url| url.trim_end_matches('/').to_string()),
            home_server_key: env.optional("HOME_SERVER_KEY"),
            peer_key: env.optional("PEER_KEY"),
            managed_by: env.optional("DUOWAVE_MANAGED"),
            role: env.optional("DUOWAVE_ROLE"),
        },
        models: ModelsConfig {
            tts_dir: env
                .get("TTS_MODELS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| data_path(env, "voices", "tts-server/models")),
            mac_deploy_dir: mac_deploy,
            piper_registry:
                "https://huggingface.co/api/models/rhasspy/piper-voices/tree/main?recursive=true&limit=1000".into(),
        },
    }
}

fn dialog(env: &Env) -> DialogConfig {
    let provider = env.get("DIALOG_PROVIDER").and_then(DialogProvider::parse).unwrap_or(DialogProvider::None);
    // Haiku while we test (the owner's decision of 01.11): quicker to the first words than Opus.
    let model = env.optional("LLM_MODEL").unwrap_or_else(|| {
        if provider == DialogProvider::Anthropic {
            "claude-haiku-5-5".into()
        } else {
            String::new()
        }
    });
    let listen = match env.get("DIALOG_LISTEN").map(str::trim) {
        Some("question") => ListenMode::Question,
        Some("always") => ListenMode::Always,
        Some("never") => ListenMode::Never,
        // Before DIALOG_LISTEN there was DIALOG_LISTEN_AFTER_QUESTION; false meant never.
        _ => {
            if env.flag("DIALOG_LISTEN_AFTER_QUESTION", true) {
                ListenMode::Question
            } else {
                ListenMode::Never
            }
        }
    };
    DialogConfig {
        provider,
        models: ModelPair {
            fast: env.optional("LLM_MODEL_FAST").unwrap_or_else(|| model.clone()),
            smart: env.optional("LLM_MODEL_SMART").unwrap_or_else(|| model.clone()),
        },
        model,
        base_url: env.optional("LLM_BASE_URL"),
        api_key: env.optional("LLM_API_KEY"),
        anthropic_key: env.optional("ANTHROPIC_API_KEY").or_else(|| env.optional("ANTHROPIC_AUTH_TOKEN")),
        anthropic_base_url: env.optional("ANTHROPIC_BASE_URL"),
        max_tokens: env.integer("LLM_MAX_TOKENS", 4096) as u32,
        effort: env.get("LLM_EFFORT").filter(|e| LLM_EFFORTS.contains(e)).unwrap_or("low").to_string(),
        timeout_ms: env.integer("LLM_TIMEOUT_MS", 10_000),
        system_prompt_file: env.get("DIALOG_SYSTEM_PROMPT_FILE").map(PathBuf::from),
        history_turns: env.integer("DIALOG_HISTORY_TURNS", 8) as usize,
        history_ttl_ms: env.integer("DIALOG_HISTORY_TTL_S", 300) * 1000,
        listen,
        early_response: env.flag("DIALOG_EARLY_RESPONSE", true),
        jingle: env.flag("DIALOG_JINGLE", true),
        hold_response_ms: env.integer("DIALOG_HOLD_RESPONSE_MS", 400),
        whisper_reply: env.flag("WHISPER_REPLY", true),
    }
}

/// The connectors' first values, as the variables give them: only complete ones, as before.
fn connectors_seed(env: &Env, content_dir: &std::path::Path) -> ConnectorsSeed {
    ConnectorsSeed {
        home_assistant: match (env.get("HA_URL"), env.get("HA_TOKEN")) {
            (Some(url), Some(token)) => Some(HomeAssistantConfig { url: url.to_string(), token: token.to_string() }),
            _ => None,
        },
        telegram: env.get("TELEGRAM_BOT_TOKEN").map(|token| TelegramConfig {
            token: token.to_string(),
            contacts: contacts(env.raw("TELEGRAM_CONTACTS")),
            inbox_dir: content_dir.join("messages"),
            alert_chat: env.optional("ALERT_TELEGRAM_CHAT"),
        }),
        livekit: match (env.get("LIVEKIT_URL"), env.get("LIVEKIT_API_KEY"), env.get("LIVEKIT_API_SECRET")) {
            (Some(url), Some(key), Some(secret)) => Some(LiveKitConfig {
                url: url.to_string(),
                public_url: env.optional("LIVEKIT_PUBLIC_URL"),
                page_url: env.optional("LIVEKIT_PAGE_URL"),
                api_key: key.to_string(),
                api_secret: secret.to_string(),
                room: env.string("LIVEKIT_ROOM", "home"),
            }),
            _ => None,
        },
        sip: match (env.get("ARI_URL"), env.get("ARI_USER"), env.get("ARI_PASSWORD")) {
            (Some(url), Some(user), Some(password)) => Some(SipConfig {
                url: url.to_string(),
                user: user.to_string(),
                password: password.to_string(),
                app: env.string("ARI_APP", "duowave"),
                rtp_host: env.string("SIP_RTP_HOST", "127.0.0.1"),
                rtp_port: env.integer("SIP_RTP_PORT", 40_000) as u16,
                contacts: contacts(env.raw("SIP_CONTACTS")),
            }),
            _ => None,
        },
        weather_location: env.optional("WEATHER_LOCATION"),
        news_feeds: env.list("NEWS_FEEDS", &[]),
        set: CONNECTOR_VARIABLES.iter().copied().filter(|name| env.get(name).is_some()).collect(),
    }
}

fn gain(env: &Env) -> GainConfig {
    let value = env.raw("AUDIO_GAIN").unwrap_or("auto").trim();
    let fixed =
        if value.is_empty() || value == "auto" { None } else { value.parse::<f64>().ok().filter(|n| n.is_finite()) };
    GainConfig {
        mode: if fixed.is_some() { GainMode::Fixed } else { GainMode::Auto },
        fixed_db: fixed.unwrap_or(0.0),
        max_db: env.number("AUDIO_GAIN_MAX_DB", 40.0),
        target_dbfs: env.number("AUDIO_GAIN_TARGET_DBFS", -20.0),
    }
}

/// `мама=101;папа=+79990000000`: names (lowercase) to numbers or chat ids, in this order. A name given again takes
/// the new value in its first place, as a key of the TS object did.
fn contacts(value: Option<&str>) -> Contacts {
    let mut result: Contacts = Vec::new();
    for entry in value.unwrap_or("").split([';', '\n']) {
        let mut parts = entry.splitn(2, '=').map(str::trim);
        if let (Some(name), Some(target)) = (parts.next(), parts.next()) {
            if !name.is_empty() && !target.is_empty() {
                let name = name.to_lowercase();
                match result.iter_mut().find(|(known, _)| *known == name) {
                    Some((_, known)) => *known = target.to_string(),
                    None => result.push((name, target.to_string())),
                }
            }
        }
    }
    result
}

/// VOICE_LAB_SERVER_URLS: `omnivoice=http://…/v1;cosyvoice3=http://…/v1`; each engine has its own port by default.
fn clone_servers(value: &str) -> CloneServers {
    let mut servers = CloneServers {
        omnivoice: "http://127.0.0.1:8093/v1".into(),
        cosyvoice3: "http://127.0.0.1:8094/v1".into(),
        voxcpm: "http://127.0.0.1:8095/v1".into(),
    };
    for entry in value.split(';') {
        let mut parts = entry.splitn(2, '=').map(str::trim);
        match (parts.next(), parts.next()) {
            (Some("omnivoice"), Some(url)) if !url.is_empty() => servers.omnivoice = url.into(),
            (Some("cosyvoice3"), Some(url)) if !url.is_empty() => servers.cosyvoice3 = url.into(),
            (Some("voxcpm"), Some(url)) if !url.is_empty() => servers.voxcpm = url.into(),
            _ => {}
        }
    }
    servers
}

/// UNIPROXY_PATH with its leading '/': the station's quasar.cfg is written with one too (station connect adds it),
/// and a route without one would stop the gateway at its start instead of refusing only the station's socket.
fn socket_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

/// The host of a configured URL, lowercase, for the allowlist.
pub fn host_of_url(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(|host| host.trim_matches(['[', ']']).to_ascii_lowercase()))
}

/// Every value the gateway would not take, as Joi listed them.
fn validate(env: &Env) -> Vec<String> {
    let mut problems = Vec::new();
    let mut one_of = |key: &str, allowed: &[&str]| {
        if let Some(value) = env.get(key) {
            if !allowed.contains(&value) {
                problems.push(format!("{key} must be one of {}", allowed.join(", ")));
            }
        }
    };
    one_of("UNIPROXY_MODE", &["gateway", "relay"]);
    one_of("YANDEX_PUSHES", &["pass", "drop"]);
    one_of("STATION_PAIRING", &["tofu", "closed", "open"]);
    one_of("VAD_ENGINE", &["silero", "energy"]);
    one_of("ASR_PROVIDER", &["openai", "none"]);
    one_of("DIALOG_PROVIDER", &["anthropic", "openai", "echo", "none"]);
    one_of("LLM_EFFORT", LLM_EFFORTS);
    one_of("DIALOG_LISTEN", &["question", "always", "never"]);
    one_of("TTS_PROVIDER", &["yandex", "mixed", "own", "none"]);
    one_of("TTS_OWN_FORMAT", &["wav", "opus"]);

    let mut number = |key: &str, min: Option<f64>, max: Option<f64>| {
        if let Some(value) = env.get(key) {
            match value.trim().parse::<f64>() {
                Ok(n) if n.is_finite() => {
                    if min.is_some_and(|min| n < min) || max.is_some_and(|max| n > max) {
                        let range = match (min, max) {
                            (Some(min), Some(max)) => format!("from {min} to {max}"),
                            (Some(min), None) => format!("at least {min}"),
                            (None, Some(max)) => format!("at most {max}"),
                            (None, None) => String::new(),
                        };
                        problems.push(format!("{key} must be {range}"));
                    }
                }
                _ => problems.push(format!("{key} must be a number")),
            }
        }
    };
    for key in [
        "PORT",
        "HTTPS_PORT",
        "RELAY_PING_MS",
        "YANDEX_PING_MS",
        "VOICE_MAX_UTTERANCE_MS",
        "SESSION_EVENTS_LIMIT",
        "TTS_OWN_TIMEOUT_MS",
        "SIP_RTP_PORT",
    ] {
        number(key, None, None);
    }
    number("YANDEX_TIMEOUT_MS", Some(1000.0), None);
    number("ANSWER_WAIT_MAX_MS", Some(1000.0), None);
    number("DEFERRED_MAX_MS", Some(0.0), None);
    number("STATION_SSH_PORT", Some(0.0), Some(65535.0));
    // An hour at most: three of them still fit the clock.
    number("STATION_PING_MS", Some(0.0), Some(3_600_000.0));
    number("RECORDER_RETENTION_DAYS", Some(0.0), None);
    number("RECORDER_MAX_MB", Some(0.0), None);
    number("STATION_OFFLINE_ALERT_MIN", Some(0.0), None);
    number("VAD_THRESHOLD", Some(0.05), Some(0.95));
    number("VAD_GUARD_MS", Some(0.0), None);
    number("VAD_SPEECH_START_MS", Some(32.0), None);
    number("VAD_END_SILENCE_MS", Some(100.0), None);
    number("VAD_NO_SPEECH_MS", Some(500.0), None);
    number("VAD_MAX_SPEECH_MS", Some(1000.0), None);
    number("VAD_ENERGY_THRESHOLD_DBFS", None, Some(0.0));
    number("AUDIO_GAIN_MAX_DB", Some(0.0), Some(60.0));
    number("AUDIO_GAIN_TARGET_DBFS", None, Some(0.0));
    number("ASR_TIMEOUT_MS", Some(500.0), None);
    number("ASR_PARTIAL_INTERVAL_MS", Some(0.0), None);
    number("ASR_INTERIM_INTERVAL_MS", Some(0.0), None);
    number("ASR_QUIET_END_MS", Some(500.0), None);
    number("LLM_MAX_TOKENS", Some(256.0), None);
    number("LLM_TIMEOUT_MS", Some(1000.0), None);
    number("DIALOG_HISTORY_TURNS", Some(0.0), None);
    number("DIALOG_HISTORY_TTL_S", Some(0.0), None);
    number("DIALOG_HOLD_RESPONSE_MS", Some(0.0), None);
    number("DIALOG_CONTINUATION_S", Some(0.0), None);
    number("TTS_TIMEOUT_MS", Some(500.0), None);
    number("VOICE_LAB_RESERVE_MB", Some(0.0), None);
    number("CLONE_TIMEOUT_MS", Some(1000.0), None);
    number("UPDATE_CHECK_HOURS", Some(0.0), None);
    number("PLUGINS_IDLE_UNLOAD_S", Some(10.0), None);

    if let Some(gain) = env.get("AUDIO_GAIN") {
        if gain.trim() != "auto" && gain.trim().parse::<f64>().is_err() {
            problems.push("AUDIO_GAIN must be auto or a number of dB".into());
        }
    }
    if let Some(port) = env.get("CALLS_STATION_MIC_PORT") {
        if !port.trim().parse::<u16>().is_ok_and(|port| port > 0) {
            problems.push("CALLS_STATION_MIC_PORT must be a port or empty".into());
        }
    }

    for key in [
        "RECORDER_ENABLED",
        "RECORDER_AUDIO",
        "RECORDER_REDACT",
        "DEBUG_API_ENABLED",
        "PANEL_REMOTE",
        "ALERT_MACOS_NOTIFY",
        "ASR_QUIET_SPEECH",
        "DIALOG_LISTEN_AFTER_QUESTION",
        "DIALOG_EARLY_RESPONSE",
        "DIALOG_JINGLE",
        "TTS_MARKUP",
        "WHISPER_REPLY",
    ] {
        if let Some(value) = env.get(key) {
            let value = value.trim().to_ascii_lowercase();
            if !["1", "0", "true", "false", "yes", "no", "on", "off"].contains(&value.as_str()) {
                problems.push(format!("{key} must be true or false"));
            }
        }
    }

    if let Some(feed) = env.get("PACKAGES_FEED_URL").map(str::trim).filter(|feed| !feed.eq_ignore_ascii_case("off")) {
        if !matches!(url::Url::parse(feed), Ok(url) if url.scheme() == "https" || url.scheme() == "http") {
            problems.push("PACKAGES_FEED_URL must be an https URL or off".into());
        }
    }
    let mut uri = |key: &str, schemes: &[&str]| {
        if let Some(value) = env.get(key) {
            match url::Url::parse(value) {
                Ok(url) if schemes.is_empty() || schemes.contains(&url.scheme()) => {}
                _ if schemes.is_empty() => problems.push(format!("{key} must be a URL")),
                _ => problems.push(format!("{key} must be a {} URL", schemes.join(" or "))),
            }
        }
    };
    uri("YANDEX_UNIPROXY_URL", &["ws", "wss"]);
    uri("LIVEKIT_URL", &["ws", "wss"]);
    uri("LIVEKIT_PUBLIC_URL", &["ws", "wss"]);
    uri("LIVEKIT_PAGE_URL", &["http", "https"]);
    for key in [
        "ALERT_WEBHOOK_URL",
        "ASR_BASE_URL",
        "LLM_BASE_URL",
        "MEDIA_BASE_URL",
        "WEATHER_FORECAST_URL",
        "WEATHER_GEOCODE_URL",
        "HA_URL",
        "TTS_OWN_URL",
        "ARI_URL",
        "UPDATE_FEED_URL",
    ] {
        uri(key, &[]);
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_node_gateways() {
        let config = Config::load(&Env::default()).unwrap();
        assert_eq!(config.app.port, 7780, "an uncommon port, not 80: the station dials whatever its address names");
        assert_eq!(config.app.public_port, None);
        assert_eq!(config.gateway.mode, UniproxyMode::Relay);
        assert!(!config.gateway.recorder.audio);
        assert!(!config.gateway.debug_api);
        assert_eq!(config.gateway.debug_api_allow, vec!["private"]);
        if !cfg!(feature = "station") {
            // The station keeps less (the_station_keeps_a_small_journal).
            assert_eq!(config.operations.retention_days, 7);
            assert_eq!(config.operations.journal_cap, 0, "no cap on a PC");
        }
        assert_eq!(config.dialog.provider, DialogProvider::None);
        assert_eq!(config.gateway.station_ssh.port, 2222);
        assert_eq!(config.gateway.station_ssh.user, "root");
        assert_eq!(config.skills.state_dir, Some(PathBuf::from("data/state")));
        assert_eq!(config.speech.asr.wake_words, vec!["алиса", "яндекс"]);
        assert_eq!(config.speech.endpoint.neg_threshold, 0.35);
        assert_eq!(config.dialog.listen, ListenMode::Question);
        assert_eq!(config.gateway.station_ping_ms, 15_000, "a cut after 45 s of silence; the station pings every 5 s");
    }

    #[cfg(feature = "station")]
    #[test]
    fn the_station_keeps_a_small_journal() {
        // /data on the station has some 60 MB: two days and 10 MB at most, unless its .env says otherwise.
        let config = Config::for_tests(&Env::default());
        assert_eq!(config.operations.retention_days, 2);
        assert_eq!(config.operations.journal_cap, 10 * 1_048_576);
        let set = Config::for_tests(&Env::from_pairs([("RECORDER_MAX_MB", "0")]));
        assert_eq!(set.operations.journal_cap, 0, "the .env turns the cap off");
    }

    #[test]
    fn gives_the_station_socket_path_its_leading_slash() {
        let bare = Config::load(&Env::from_pairs([("UNIPROXY_PATH", "uni.ws")])).unwrap();
        assert_eq!(bare.gateway.uniproxy_path, "/uni.ws");
        let given = Config::load(&Env::from_pairs([("UNIPROXY_PATH", "/station/uni.ws")])).unwrap();
        assert_eq!(given.gateway.uniproxy_path, "/station/uni.ws");
    }

    #[test]
    fn data_dir_moves_every_writable_path() {
        let config = Config::load(&Env::from_pairs([("DATA_DIR", "/data/duowave")])).unwrap();
        assert_eq!(config.skills.state_dir, Some(PathBuf::from("/data/duowave/state")));
        assert_eq!(config.gateway.recorder.dir, PathBuf::from("/data/duowave/sessions"));
        assert_eq!(config.skills.content_dir, PathBuf::from("/data/duowave/content"));
        assert_eq!(config.voice_lab.dir, PathBuf::from("/data/duowave/voice-lab"));
        assert_eq!(config.plugins.packages_dir, PathBuf::from("/data/duowave/packages"));
        assert_eq!(config.plugins.catalog_dir, PathBuf::from("/data/duowave/packages-catalog"));
        let memory = Config::load(&Env::from_pairs([("DATA_DIR", "/d"), ("STATE_DIR", "")])).unwrap();
        assert_eq!(memory.skills.state_dir, None);
    }

    #[test]
    fn mac_deploy_dir_names_the_run_scripts_for_a_gateway_from_elsewhere() {
        let config = Config::load(&Env::from_pairs([("MAC_DEPLOY_DIR", "/opt/duowave/deploy/macos")])).unwrap();
        assert_eq!(config.models.mac_deploy_dir, Some(PathBuf::from("/opt/duowave/deploy/macos")));
        // Blank: as not given — the working folder's deploy/macos is the default then.
        let blank = Config::load(&Env::from_pairs([("MAC_DEPLOY_DIR", "  ")])).unwrap();
        assert_eq!(blank.models.mac_deploy_dir, None);
    }

    #[test]
    fn own_voice_counts_as_configured_where_the_run_scripts_are() {
        // With the Mac's run scripts at hand the own voice server is «configured» on their fixed port: a new
        // install's flow does not stop at TTS_OWN_URL, the panel starts the server by those scripts.
        let with_scripts = Config::load(&Env::from_pairs([("MAC_DEPLOY_DIR", "/opt/duowave/deploy/macos")])).unwrap();
        assert_eq!(
            with_scripts.gateway.tts.own.as_ref().map(|own| own.base_url.as_str()),
            Some("http://127.0.0.1:8091/v1")
        );
        // An explicit URL wins; without the scripts nothing is assumed.
        let explicit = Config::load(&Env::from_pairs([
            ("MAC_DEPLOY_DIR", "/opt/duowave/deploy/macos"),
            ("TTS_OWN_URL", "https://tts.example/v1"),
        ]))
        .unwrap();
        assert_eq!(explicit.gateway.tts.own.as_ref().map(|own| own.base_url.as_str()), Some("https://tts.example/v1"));
        let bare = Config::load(&Env::from_pairs([("UNRELATED", "1")])).unwrap();
        assert!(bare.gateway.tts.own.is_none());
    }

    #[test]
    fn lists_every_value_it_would_not_take() {
        let error = Config::load(&Env::from_pairs([
            ("UNIPROXY_MODE", "proxy"),
            ("PORT", "eighty"),
            ("VAD_THRESHOLD", "2"),
            ("YANDEX_UNIPROXY_URL", "https://x"),
            ("PANEL_REMOTE", "maybe"),
            // A day between pings: three of them would not fit the clock.
            ("STATION_PING_MS", "86400000"),
        ]))
        .unwrap_err();
        assert_eq!(error.problems.len(), 6, "{:?}", error.problems);
        assert!(
            error.problems.contains(&"STATION_PING_MS must be from 0 to 3600000".to_string()),
            "{:?}",
            error.problems
        );
    }

    #[test]
    fn the_model_follows_the_provider_and_the_profiles_follow_the_model() {
        let claude = Config::load(&Env::from_pairs([("DIALOG_PROVIDER", "anthropic")])).unwrap();
        assert_eq!(claude.dialog.model, "claude-haiku-5-5");
        assert_eq!(claude.dialog.models.smart, "claude-haiku-5-5");
        let split = Config::load(&Env::from_pairs([
            ("DIALOG_PROVIDER", "openai"),
            ("LLM_MODEL", "llama"),
            ("LLM_MODEL_SMART", "qwen"),
        ]))
        .unwrap();
        assert_eq!(split.dialog.models.fast, "llama");
        assert_eq!(split.dialog.models.smart, "qwen");
        let old = Config::load(&Env::from_pairs([("DIALOG_LISTEN_AFTER_QUESTION", "false")])).unwrap();
        assert_eq!(old.dialog.listen, ListenMode::Never);
    }

    #[test]
    fn contacts_and_allowed_hosts() {
        let config = Config::load(&Env::from_pairs([
            ("TELEGRAM_BOT_TOKEN", "t"),
            ("TELEGRAM_CONTACTS", "Мама=1; папа = 2\nбад"),
            ("ALLOWED_HOSTS", "Gw.Example.com, other.example"),
            ("MEDIA_BASE_URL", "https://media.example.com/media/"),
        ]))
        .unwrap();
        let telegram = config.connectors.telegram.unwrap();
        let pair = |name: &str, target: &str| (name.to_string(), target.to_string());
        assert_eq!(telegram.contacts, vec![pair("мама", "1"), pair("папа", "2")]);
        assert_eq!(config.connectors.set, vec!["TELEGRAM_BOT_TOKEN", "TELEGRAM_CONTACTS"]);
        assert_eq!(config.app.allowed_hosts, vec!["gw.example.com", "other.example", "media.example.com"]);
        assert_eq!(config.skills.media_base_url.as_deref(), Some("https://media.example.com/media"));

        // In the order given, not sorted: lookups take the first that fits.
        let ordered = Config::load(&Env::from_pairs([
            ("TELEGRAM_BOT_TOKEN", "t"),
            ("TELEGRAM_CONTACTS", "мама света=1;мама=2;Мама света=3"),
        ]))
        .unwrap();
        assert_eq!(ordered.connectors.telegram.unwrap().contacts, vec![pair("мама света", "3"), pair("мама", "2")]);
    }

    #[test]
    fn secrets_never_print() {
        let config = Config::load(&Env::from_pairs([
            ("TELEGRAM_BOT_TOKEN", "123:secret-bot-token"),
            ("ALERT_TELEGRAM_CHAT", "4242"),
            ("HA_URL", "http://ha.local:8123"),
            ("HA_TOKEN", "secret-ha-token"),
            ("LIVEKIT_URL", "ws://127.0.0.1:7880"),
            ("LIVEKIT_API_KEY", "APIsecretkey"),
            ("LIVEKIT_API_SECRET", "secret-livekit"),
            ("ARI_URL", "http://127.0.0.1:8088/ari"),
            ("ARI_USER", "duowave"),
            ("ARI_PASSWORD", "secret-ari"),
        ]))
        .unwrap();
        let printed = format!("{config:?}");
        for secret in ["secret-bot-token", "4242", "secret-ha-token", "APIsecretkey", "secret-livekit", "secret-ari"] {
            assert!(!printed.contains(secret), "{secret} printed");
        }
        assert!(printed.contains("ha.local:8123"));
    }
}
