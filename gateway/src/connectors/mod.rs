//! The gateway's connectors in one registry (core/connectors.rs): the built-in ones say how they are set up and how
//! they are doing; MCP servers and agents register the same way and give the model tools.
//!
//! The gateway registers every built-in at its build, in the Node gateway's order (`ConnectorsService::with_links`):
//! three of them read other modules — `builtin::yandex` the mode and the station sessions, `builtin::llm` the dialog,
//! `builtin::tts` the voice (src/links). Home Assistant, Telegram, LiveKit, SIP, the weather and the news are set up
//! in the panel (settings.rs) and read their state from there on every look. The station sessions come from the
//! station link when it stands (`set_station_sessions`); until then the Yandex connector sees none. The MCP servers
//! added in the panel come from `set_mcp` (src/mcp).
//!
//! connectors.module.ts is the Node gateway's, removed from the tree: git tag `node-gateway-last`.

pub mod checks;
pub mod controller;
pub mod settings;

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use futures::future::BoxFuture;
use futures::FutureExt;
use serde::Serialize;

use crate::app::App;
use crate::config::Config;
use crate::core::connectors::{
    Connector, ConnectorKind, ConnectorManifest, ConnectorRegistry, ConnectorSetup, ConnectorStatus, ConnectorView,
    ToolContribution, CONNECTOR_KINDS,
};
use crate::core::identity::permissions::CONNECTORS_VIEW;
use crate::core::identity::Principal;
use crate::operations::networks::is_local_url;
use crate::platform::http::{ok, ApiResult, Caller};
use crate::requests::ModeSource;
use crate::skills::capabilities::CapabilityName;
use settings::{ConnectorId, ConnectorSettings};

/// The built-ins in the order the panel lists them, whenever each registered.
const BUILT_IN_ORDER: &[&str] = &[
    "yandex",
    "llm",
    "asr",
    "tts",
    "home-assistant",
    "telegram",
    "livekit",
    "sip",
    "weather",
    "news",
    "piper-registry",
];

/// An MCP server's state as the connectors list shows it (src/mcp, ported apart).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpState {
    Off,
    Connecting,
    Ready,
    Error,
    Login,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerState {
    pub id: String,
    pub title: String,
    pub cloud: bool,
    pub state: McpState,
    /// Tools switched on.
    pub tools: usize,
    pub detail: Option<String>,
}

/// The MCP servers added in the panel (src/mcp's McpService, through src/links).
pub trait McpServers: Send + Sync {
    fn states(&self) -> Vec<McpServerState>;
}

/// What the built-ins that read other modules are given at the build, as connectors.module.ts injected them: the
/// mode, the dialog, the voice. The station sessions join later (`set_station_sessions`).
pub struct ConnectorLinks {
    pub modes: Arc<dyn ModeSource>,
    pub dialog: Arc<dyn builtin::DialogInfo>,
    pub voice: Arc<dyn builtin::VoiceInfo>,
}

/// The station sessions once the station link gives them: none until then.
#[derive(Default)]
struct LateSessions(OnceLock<Arc<dyn builtin::StationSessions>>);

impl builtin::StationSessions for LateSessions {
    fn open(&self) -> usize {
        self.0.get().map_or(0, |sessions| sessions.open())
    }

    fn yandex_connected(&self) -> bool {
        self.0.get().is_some_and(|sessions| sessions.yandex_connected())
    }
}

/// A built-in's place in the panel's list; anything else after them.
fn rank(id: &str) -> usize {
    BUILT_IN_ORDER.iter().position(|known| *known == id).unwrap_or(BUILT_IN_ORDER.len())
}

pub struct ConnectorsService {
    registry: ConnectorRegistry,
    mcp: OnceLock<Arc<dyn McpServers>>,
    /// What the Yandex connector reads of the station sessions.
    sessions: Arc<LateSessions>,
}

impl ConnectorsService {
    /// The registry with the built-ins that read the configuration and the panel's settings.
    pub fn new(config: &Config, settings: Arc<ConnectorSettings>) -> ConnectorsService {
        ConnectorsService::registered(builtin::from_config(config, settings), Arc::default())
    }

    /// Every built-in, registered in the Node gateway's order (connectors.module.ts): Yandex, the model,
    /// recognition, our voice, then the rest.
    pub fn with_links(config: &Config, settings: Arc<ConnectorSettings>, links: ConnectorLinks) -> ConnectorsService {
        let sessions: Arc<LateSessions> = Arc::default();
        let mut builtins = builtin::from_config(config, settings);
        builtins.push(builtin::yandex(links.modes.clone(), sessions.clone()));
        builtins.push(builtin::llm(links.dialog));
        builtins.push(builtin::tts(config, links.voice, links.modes));
        builtins.sort_by_key(|connector| rank(&connector.manifest().id));
        ConnectorsService::registered(builtins, sessions)
    }

    fn registered(builtins: Vec<Arc<dyn Connector>>, sessions: Arc<LateSessions>) -> ConnectorsService {
        let registry = ConnectorRegistry::new();
        for connector in builtins {
            if let Err(error) = registry.register(connector) {
                tracing::error!("{error}");
            }
        }
        ConnectorsService { registry, mcp: OnceLock::new(), sessions }
    }

    /// A connector joins: a built-in whose module just landed, a plug-in.
    pub fn register(&self, connector: Arc<dyn Connector>) -> anyhow::Result<()> {
        self.registry.register(connector)
    }

    pub fn set_mcp(&self, servers: Arc<dyn McpServers>) {
        if self.mcp.set(servers).is_err() {
            tracing::warn!("connectors: the MCP servers are set already");
        }
    }

    /// The station link's sessions, for the Yandex connector: once, when the session registry stands.
    pub fn set_station_sessions(&self, sessions: Arc<dyn builtin::StationSessions>) {
        if self.sessions.0.set(sessions).is_err() {
            tracing::warn!("connectors: the station sessions are set already");
        }
    }

    pub fn registry(&self) -> &ConnectorRegistry {
        &self.registry
    }

    /// The built-in connectors, then the MCP servers added in the panel, each with its state now.
    pub async fn views(&self) -> Vec<ConnectorView> {
        let mut views = self.registry.views().await;
        views.sort_by_key(|view| rank(&view.manifest.id));
        if let Some(mcp) = self.mcp.get() {
            views.extend(mcp.states().into_iter().map(mcp_view));
        }
        views
    }

    /// The tools the connectors give, of those the principal may use.
    pub fn tools(&self, principal: &Principal) -> Vec<ToolContribution> {
        self.registry.tools(principal)
    }
}

/// The dialog's way to the connectors' tools: the plugins' tools reach «Мозг» through here.
impl crate::core::connectors::ContributedTools for ConnectorsService {
    fn tools(&self, principal: &Principal) -> Vec<ToolContribution> {
        self.registry.tools(principal)
    }
}

fn mcp_view(server: McpServerState) -> ConnectorView {
    let status = match server.state {
        McpState::Ready => ConnectorStatus::ready(format!("инструментов включено: {}", server.tools)),
        McpState::Off => ConnectorStatus::off("выключен"),
        McpState::Connecting => ConnectorStatus::configured(Some("подключается".into())),
        McpState::Login => ConnectorStatus::error("нужно войти"),
        McpState::Error => ConnectorStatus::error(server.detail.unwrap_or_else(|| "не отвечает".into())),
    };
    ConnectorView {
        manifest: ConnectorManifest {
            id: format!("mcp:{}", server.id),
            kind: ConnectorKind::Mcp,
            title: server.title,
            description: "MCP-сервер: инструменты для модели".into(),
            cloud: server.cloud,
            env: Vec::new(),
            capabilities: None,
            setup: None,
        },
        status,
    }
}

/// The host (and port) of a configured URL, for people; the text itself when it is not a URL.
pub fn host_of(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => match (parsed.host_str(), parsed.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_string(),
            _ => url.to_string(),
        },
        Err(_) => url.to_string(),
    }
}

/// A local model server answers its /health.
pub async fn probe(base_url: &str) -> bool {
    let base = base_url.trim_end_matches('/');
    let url = format!("{}/health", base.strip_suffix("/v1").unwrap_or(base));
    match crate::utils::http::client().get(url).timeout(Duration::from_millis(1500)).send().await {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    }
}

pub mod builtin {
    //! The built-in connectors: the configuration's, and those that read modules ported apart.

    use super::*;
    use crate::settings::definitions::PanelMode;

    type StatusFn = Arc<dyn Fn() -> BoxFuture<'static, ConnectorStatus> + Send + Sync>;

    /// A built-in: a manifest (its cloud read on every look) and a status check.
    pub struct Builtin {
        manifest: ConnectorManifest,
        cloud: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
        status: StatusFn,
    }

    impl Connector for Builtin {
        fn manifest(&self) -> ConnectorManifest {
            let mut manifest = self.manifest.clone();
            if let Some(cloud) = &self.cloud {
                manifest.cloud = cloud();
            }
            manifest
        }

        fn status(&self) -> BoxFuture<'_, ConnectorStatus> {
            (self.status)()
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn manifest(
        id: &str,
        kind: ConnectorKind,
        title: &str,
        description: &str,
        cloud: bool,
        env: &[&str],
        capabilities: &[CapabilityName],
        setup: Option<ConnectorSetup>,
    ) -> ConnectorManifest {
        ConnectorManifest {
            id: id.into(),
            kind,
            title: title.into(),
            description: description.into(),
            cloud,
            env: env.iter().map(|name| name.to_string()).collect(),
            capabilities: (!capabilities.is_empty()).then(|| capabilities.to_vec()),
            setup,
        }
    }

    fn fixed(manifest: ConnectorManifest, status: ConnectorStatus) -> Arc<dyn Connector> {
        Arc::new(Builtin {
            manifest,
            cloud: None,
            status: Arc::new(move || futures::future::ready(status.clone()).boxed()),
        })
    }

    /// One the panel sets up (settings.rs): its state and whether it leaves the house, read on every look.
    fn panel(
        settings: &Arc<ConnectorSettings>,
        id: ConnectorId,
        kind: ConnectorKind,
        description: &str,
        capabilities: &[CapabilityName],
    ) -> Arc<dyn Connector> {
        let (statuses, clouds) = (settings.clone(), settings.clone());
        let setup = id.available().then_some(ConnectorSetup::Panel);
        Arc::new(Builtin {
            manifest: manifest(id.as_str(), kind, id.title(), description, false, &[], capabilities, setup),
            cloud: Some(Arc::new(move || clouds.cloud(id))),
            status: Arc::new(move || futures::future::ready(statuses.status(id)).boxed()),
        })
    }

    /// The built-ins that read the configuration and the panel's settings, in the panel's order.
    pub fn from_config(config: &Config, settings: Arc<ConnectorSettings>) -> Vec<Arc<dyn Connector>> {
        let asr = config.speech.asr.clone();
        let asr_status: StatusFn = Arc::new(move || {
            let asr = asr.clone();
            async move {
                if !asr.enabled {
                    return ConnectorStatus::off("своё распознавание не используется");
                }
                if probe(&asr.base_url).await {
                    ConnectorStatus::ready(asr.base_url.clone())
                } else {
                    ConnectorStatus::error(format!("{} не отвечает", asr.base_url))
                }
            }
            .boxed()
        });
        vec![
            Arc::new(Builtin {
                manifest: manifest(
                    "asr",
                    ConnectorKind::Stt,
                    "Распознавание речи",
                    "Свой сервер распознавания (GigaAM, Whisper, Parakeet) или OpenAI-совместимый сервис.",
                    !is_local_url(Some(&config.speech.asr.base_url)),
                    &["ASR_PROVIDER", "ASR_BASE_URL", "ASR_MODEL"],
                    &[],
                    Some(ConnectorSetup::Voice),
                ),
                cloud: None,
                status: asr_status,
            }),
            panel(
                &settings,
                ConnectorId::HomeAssistant,
                ConnectorKind::SmartHome,
                "Свет, розетки, климат и сценарии дома.",
                &[CapabilityName::SmartHome],
            ),
            panel(
                &settings,
                ConnectorId::Telegram,
                ConnectorKind::Messenger,
                "Голосовые сообщения семье через своего бота, ссылки на звонки и оповещения о колонке.",
                &[CapabilityName::Calls],
            ),
            panel(
                &settings,
                ConnectorId::LiveKit,
                ConnectorKind::Calls,
                "Звонки на колонку из браузера, дома и вне дома.",
                &[CapabilityName::Calls],
            ),
            panel(
                &settings,
                ConnectorId::Sip,
                ConnectorKind::Calls,
                "Звонки на обычные телефоны через Asterisk.",
                &[CapabilityName::Calls],
            ),
            panel(
                &settings,
                ConnectorId::Weather,
                ConnectorKind::Lookup,
                "Прогноз Open-Meteo без ключа.",
                &[CapabilityName::Weather],
            ),
            panel(
                &settings,
                ConnectorId::News,
                ConnectorKind::Lookup,
                "Ленты RSS и Atom: из каталога и свои.",
                &[CapabilityName::News],
            ),
            fixed(
                manifest(
                    "piper-registry",
                    ConnectorKind::Catalog,
                    "Каталог голосов Piper",
                    "Открытые голоса rhasspy/piper-voices на HuggingFace для магазина моделей.",
                    true,
                    &[],
                    &[],
                    None,
                ),
                ConnectorStatus::configured(Some("huggingface.co".into())),
            ),
        ]
    }

    /// The station sessions (src/gateway/uniproxy), as the Yandex connector reads them.
    pub trait StationSessions: Send + Sync {
        /// Sessions with a station open now.
        fn open(&self) -> usize;
        /// Whether one of them has our own session with Yandex up (gateway mode).
        fn yandex_connected(&self) -> bool;
    }

    /// Yandex: Alice's voice, her skills and whatever we cannot do yet, through a session with the station's
    /// credentials.
    pub fn yandex(modes: Arc<dyn ModeSource>, sessions: Arc<dyn StationSessions>) -> Arc<dyn Connector> {
        let status: StatusFn = Arc::new(move || {
            let mode = modes.current();
            let status = if mode == PanelMode::Own {
                ConnectorStatus::off("режим «Свой»: без Яндекса")
            } else if mode == PanelMode::Relay {
                let state = if sessions.open() > 0 {
                    crate::core::connectors::ConnectorState::Ready
                } else {
                    crate::core::connectors::ConnectorState::Configured
                };
                ConnectorStatus::new(state, Some("колонка говорит с Яндексом сама".into()))
            } else if sessions.yandex_connected() {
                ConnectorStatus::ready("сессия открыта")
            } else {
                ConnectorStatus::configured(Some("сессия откроется с колонкой".into()))
            };
            futures::future::ready(status).boxed()
        });
        Arc::new(Builtin {
            manifest: manifest(
                "yandex",
                ConnectorKind::Station,
                "Яндекс",
                "Голос Алисы, её навыки и всё, чего мы пока не умеем: через сессию с учётными данными колонки.",
                true,
                &["UNIPROXY_MODE", "YANDEX_UNIPROXY_URL", "ROUTES"],
                &[],
                Some(ConnectorSetup::Mode),
            ),
            cloud: None,
            status,
        })
    }

    /// The dialog (src/dialog), as the model's connector reads it.
    pub trait DialogInfo: Send + Sync {
        /// Whether a model answers (DIALOG_PROVIDER or the panel's choice).
        fn enabled(&self) -> bool;
        /// The model and provider, for people.
        fn description(&self) -> String;
        /// Whether the question leaves the house.
        fn cloud(&self) -> bool;
    }

    /// The model for conversation: answers questions and picks tools.
    pub fn llm(dialog: Arc<dyn DialogInfo>) -> Arc<dyn Connector> {
        let checked = dialog.clone();
        let status: StatusFn = Arc::new(move || {
            let status = if checked.enabled() {
                ConnectorStatus::configured(Some(checked.description()))
            } else {
                ConnectorStatus::unset("модель не выбрана")
            };
            futures::future::ready(status).boxed()
        });
        Arc::new(Builtin {
            manifest: manifest(
                "llm",
                ConnectorKind::Llm,
                "Модель для разговора",
                "Отвечает на вопросы и выбирает инструменты: Claude или любой OpenAI-совместимый сервис.",
                false,
                &["DIALOG_PROVIDER", "LLM_MODEL", "LLM_BASE_URL", "LLM_API_KEY"],
                &[CapabilityName::Chat],
                Some(ConnectorSetup::Brain),
            ),
            // Read on every look: the panel may switch the model between home and the cloud.
            cloud: Some(Arc::new(move || dialog.cloud())),
            status,
        })
    }

    /// Our voice (src/speech), as its connector describes it.
    pub trait VoiceInfo: Send + Sync {
        /// Whose voice speaks, after the mode (Alice's gives way to ours when Yandex is not allowed).
        fn describe(&self, yandex_allowed: bool) -> String;
    }

    /// The gateway's voice server (Silero, Piper) or an OpenAI-compatible synthesis.
    pub fn tts(config: &Config, voice: Arc<dyn VoiceInfo>, modes: Arc<dyn ModeSource>) -> Arc<dyn Connector> {
        let url = config.gateway.tts.own.as_ref().map(|own| own.base_url.clone());
        let cloud = !is_local_url(url.as_deref());
        let status: StatusFn = Arc::new(move || {
            let (url, voice, modes) = (url.clone(), voice.clone(), modes.clone());
            async move {
                let Some(url) = url else {
                    return ConnectorStatus::unset("свой голосовой сервер не задан");
                };
                if probe(&url).await {
                    ConnectorStatus::ready(voice.describe(modes.current() != PanelMode::Own))
                } else {
                    ConnectorStatus::error(format!("{url} не отвечает"))
                }
            }
            .boxed()
        });
        Arc::new(Builtin {
            manifest: manifest(
                "tts",
                ConnectorKind::Tts,
                "Свой голос",
                "Голосовой сервер шлюза (Silero, Piper) или OpenAI-совместимый синтез.",
                cloud,
                &["TTS_PROVIDER", "TTS_OWN_URL", "TTS_OWN_MODEL", "TTS_OWN_VOICE"],
                &[],
                Some(ConnectorSetup::Voice),
            ),
            cloud: None,
            status,
        })
    }
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/connectors", get(list)).merge(controller::routes())
}

#[derive(Debug, Serialize)]
struct Kind {
    id: ConnectorKind,
    title: &'static str,
}

#[derive(Debug, Serialize)]
struct Connectors {
    connectors: Vec<ConnectorView>,
    kinds: Vec<Kind>,
}

/// Every connector with its state, and the kinds the gateway knows (MCP and agents come next).
async fn list(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Connectors> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(Connectors {
        connectors: app.connectors.views().await,
        kinds: CONNECTOR_KINDS.iter().map(|(id, title)| Kind { id: *id, title }).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::builtin::{DialogInfo, StationSessions, VoiceInfo};
    use super::*;
    use crate::config::Env;
    use crate::core::connectors::ConnectorState;
    use crate::settings::definitions::PanelMode;
    use std::sync::Mutex;

    struct Mode(Mutex<PanelMode>);

    impl ModeSource for Mode {
        fn current(&self) -> PanelMode {
            *self.0.lock().unwrap()
        }
    }

    struct Sessions(usize, bool);

    impl StationSessions for Sessions {
        fn open(&self) -> usize {
            self.0
        }
        fn yandex_connected(&self) -> bool {
            self.1
        }
    }

    struct Dialog;

    impl DialogInfo for Dialog {
        fn enabled(&self) -> bool {
            true
        }
        fn description(&self) -> String {
            "anthropic claude-haiku-5-5".into()
        }
        fn cloud(&self) -> bool {
            true
        }
    }

    struct Voice;

    impl VoiceInfo for Voice {
        fn describe(&self, yandex_allowed: bool) -> String {
            if yandex_allowed { "Алиса" } else { "свой: baya" }.into()
        }
    }

    fn config(pairs: &[(&str, &str)]) -> Config {
        Config::for_tests(&Env::from_pairs(pairs.iter().copied()))
    }

    /// The panel's settings over a database in memory, seeded from the config's env.
    fn panel(config: &Config) -> Arc<ConnectorSettings> {
        use crate::secrets::SecretsService;
        use crate::settings::SettingsService;
        let db = Arc::new(crate::storage::Db::memory());
        let events = crate::core::events::EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let secrets = Arc::new(SecretsService::new(db, None).unwrap());
        Arc::new(ConnectorSettings::new(
            &config.connectors,
            settings,
            secrets,
            events,
            std::path::PathBuf::from("messages"),
        ))
    }

    fn service(pairs: &[(&str, &str)]) -> ConnectorsService {
        let config = config(pairs);
        ConnectorsService::new(&config, panel(&config))
    }

    fn ids(views: &[ConnectorView]) -> Vec<&str> {
        views.iter().map(|view| view.manifest.id.as_str()).collect()
    }

    #[tokio::test]
    async fn describes_what_is_set_up_and_says_what_is_not() {
        let service = service(&[
            ("ASR_PROVIDER", "none"),
            ("HA_URL", "http://192.168.1.5:8123"),
            ("HA_TOKEN", "t"),
            ("NEWS_FEEDS", "https://a/rss,https://b/atom"),
        ]);
        let views = service.views().await;
        assert_eq!(
            ids(&views),
            ["asr", "home-assistant", "telegram", "livekit", "sip", "weather", "news", "piper-registry"]
        );
        let status = |id: &str| views.iter().find(|view| view.manifest.id == id).unwrap().status.clone();
        let manifest = |id: &str| views.iter().find(|view| view.manifest.id == id).unwrap().manifest.clone();
        assert_eq!(status("asr"), ConnectorStatus::off("своё распознавание не используется"));
        assert_eq!(manifest("asr").setup, Some(ConnectorSetup::Voice));
        assert_eq!(status("home-assistant"), ConnectorStatus::configured(Some("192.168.1.5:8123".into())));
        if cfg!(feature = "pc") {
            assert_eq!(status("telegram"), ConnectorStatus::unset("нет токена бота"));
            assert_eq!(manifest("telegram").setup, Some(ConnectorSetup::Panel));
        } else {
            assert_eq!(status("telegram"), ConnectorStatus::off("в сборке для колонки звонков нет"));
            assert_eq!(manifest("telegram").setup, None);
        }
        // Frozen calls: Sip is «скоро» wherever it runs, LiveKit wherever its SDK is missing.
        assert_eq!(
            status("sip").state,
            if cfg!(feature = "pc") {
                crate::core::connectors::ConnectorState::Soon
            } else {
                crate::core::connectors::ConnectorState::Off
            }
        );
        assert_eq!(manifest("sip").setup, None);
        assert_eq!(status("news"), ConnectorStatus::configured(Some("лент: 2".into())));
        assert_eq!(status("weather"), ConnectorStatus::configured(Some("город по часовому поясу".into())));
        let home = manifest("home-assistant");
        assert!(!home.cloud, "a Home Assistant at home keeps the house's data at home");
        assert_eq!(home.capabilities, Some(vec![CapabilityName::SmartHome]));
        assert!(home.env.is_empty(), "set up in the panel: no variables to name");
        assert_eq!(manifest("piper-registry").setup, None);
    }

    #[tokio::test]
    async fn keeps_the_built_ins_order_whenever_they_join_and_lists_the_mcp_servers_last() {
        struct Servers;
        impl McpServers for Servers {
            fn states(&self) -> Vec<McpServerState> {
                let server = |id: &str, state, detail: Option<&str>| McpServerState {
                    id: id.into(),
                    title: id.into(),
                    cloud: false,
                    state,
                    tools: 3,
                    detail: detail.map(str::to_string),
                };
                vec![
                    server("files", McpState::Ready, None),
                    server("home", McpState::Login, None),
                    server("broken", McpState::Error, Some("exit 1")),
                ]
            }
        }
        let service = service(&[("ASR_PROVIDER", "none")]);
        let mode = Arc::new(Mode(Mutex::new(PanelMode::Gateway)));
        service.register(builtin::llm(Arc::new(Dialog))).unwrap();
        service.register(builtin::yandex(mode.clone(), Arc::new(Sessions(1, true)))).unwrap();
        service.register(builtin::tts(&config(&[]), Arc::new(Voice), mode.clone())).unwrap();
        service.set_mcp(Arc::new(Servers));
        let views = service.views().await;
        assert_eq!(&ids(&views)[..4], ["yandex", "llm", "asr", "tts"]);
        assert_eq!(&ids(&views)[11..], ["mcp:files", "mcp:home", "mcp:broken"]);
        assert_eq!(views[0].status, ConnectorStatus::ready("сессия открыта"));
        assert_eq!(views[1].status, ConnectorStatus::configured(Some("anthropic claude-haiku-5-5".into())));
        assert!(views[1].manifest.cloud);
        assert_eq!(views[3].status, ConnectorStatus::unset("свой голосовой сервер не задан"));
        assert_eq!(views[11].status, ConnectorStatus::ready("инструментов включено: 3"));
        assert_eq!(views[12].status, ConnectorStatus::error("нужно войти"));
        assert_eq!(views[13].status, ConnectorStatus::error("exit 1"));

        *mode.0.lock().unwrap() = PanelMode::Own;
        assert_eq!(service.views().await[0].status, ConnectorStatus::off("режим «Свой»: без Яндекса"));
        *mode.0.lock().unwrap() = PanelMode::Relay;
        let relay = service.views().await[0].status.clone();
        assert_eq!(relay.state, ConnectorState::Ready);
        assert_eq!(relay.detail.as_deref(), Some("колонка говорит с Яндексом сама"));
    }

    #[tokio::test]
    async fn registers_every_built_in_in_the_node_order_and_sees_no_session_until_the_station_link_gives_them() {
        let mode = Arc::new(Mode(Mutex::new(PanelMode::Gateway)));
        let links = ConnectorLinks { modes: mode.clone(), dialog: Arc::new(Dialog), voice: Arc::new(Voice) };
        let config = config(&[("ASR_PROVIDER", "none")]);
        let service = ConnectorsService::with_links(&config, panel(&config), links);
        let registered: Vec<String> =
            service.registry().list().iter().map(|connector| connector.manifest().id).collect();
        assert_eq!(registered, BUILT_IN_ORDER);
        let yandex = |views: &[ConnectorView]| views[0].status.clone();
        assert_eq!(
            yandex(&service.views().await),
            ConnectorStatus::configured(Some("сессия откроется с колонкой".into()))
        );
        service.set_station_sessions(Arc::new(Sessions(1, true)));
        assert_eq!(yandex(&service.views().await), ConnectorStatus::ready("сессия открыта"));
        *mode.0.lock().unwrap() = PanelMode::Relay;
        assert_eq!(yandex(&service.views().await).state, ConnectorState::Ready, "a station talks to Yandex itself");
    }

    #[tokio::test]
    async fn probes_a_local_model_server_at_its_health() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Router::new().route("/health", get(|| async { "ok" }));
        tokio::spawn(async move { axum::serve(listener, server).await });
        assert!(probe(&format!("http://127.0.0.1:{port}/v1")).await);
        assert!(probe(&format!("http://127.0.0.1:{port}/")).await);
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_port = closed.local_addr().unwrap().port();
        drop(closed);
        assert!(!probe(&format!("http://127.0.0.1:{closed_port}/v1")).await);
    }

    #[test]
    fn names_a_host_as_url_host_did() {
        assert_eq!(host_of("http://192.168.1.5:8123/api"), "192.168.1.5:8123");
        assert_eq!(host_of("https://api.open-meteo.com/v1/forecast"), "api.open-meteo.com");
        assert_eq!(host_of("https://example.com:443/x"), "example.com");
        assert_eq!(host_of("not a url"), "not a url");
    }
}
