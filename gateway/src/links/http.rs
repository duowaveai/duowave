//! The gateway as the HTTP layer and the modules' routes see it (src/platform/http/state.rs): the gate a `Caller` is
//! checked against, and each thing a module's routes say they use. A module whose routes stop taking the whole gateway
//! adds what they use here, a line each (step В0 of docs/plans/modular-gateway.md).
use std::any::Any;
use std::sync::Arc;

use crate::access::users_repository::UsersRepository;
use crate::access::AccessService;
use crate::app::App;
use crate::config::Config;
use crate::core::identity::Principal;
use crate::platform::http::{ApiError, Gate, Provides};
use crate::secrets::SecretsService;
use crate::settings::SettingsService;

impl Gate for App {
    fn home_networks(&self) -> &[String] {
        &self.config.gateway.debug_api_allow
    }

    fn panel_remote(&self) -> bool {
        self.config.gateway.panel_remote
    }

    fn peer_key(&self) -> Option<&str> {
        self.placement.peer_key()
    }

    fn knows_person(&self, id: &str) -> bool {
        UsersRepository::new(self.db.clone()).by_id(id).ok().flatten().is_some()
    }

    fn authenticate(&self, authorization: Option<&str>) -> Result<Option<Principal>, ApiError> {
        self.access.try_authenticate(authorization)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Provides<crate::helpers::home_voice::HomeVoice> for App {
    fn provide(&self) -> crate::helpers::home_voice::HomeVoice {
        crate::helpers::home_voice::HomeVoice {
            voice: self.voice.clone(),
            helpers: self.helpers.clone(),
            settings: self.settings.clone(),
            package_of: {
                let characters = self.characters.clone();
                Arc::new(move |id| characters.character(id).map(|character| character.package))
            },
        }
    }
}

impl Provides<crate::talk::Parts> for App {
    fn provide(&self) -> crate::talk::Parts {
        let requests = self.requests.clone();
        let connectors = self.connectors.clone();
        crate::talk::Parts {
            talk: self.talk.clone(),
            dialog: self.dialog.clone(),
            voice: self.voice.clone(),
            secrets: self.secrets.clone(),
            skills: self.skills.clone(),
            mcp: self.mcp.clone(),
            router: self.router.clone(),
            journal: Arc::new(move |start| requests.begin_in(start, "own")),
            connector_tools: Arc::new(move |principal| connectors.tools(principal)),
        }
    }
}

impl Provides<crate::connectors::controller::Parts> for App {
    fn provide(&self) -> crate::connectors::controller::Parts {
        crate::connectors::controller::Parts {
            connector_settings: self.connector_settings.clone(),
            config: self.config.clone(),
            calls: self.pc.calls_sheet(),
        }
    }
}

impl Provides<crate::home::Parts> for App {
    fn provide(&self) -> crate::home::Parts {
        crate::home::Parts {
            config: self.config.clone(),
            db: self.db.clone(),
            events: self.events.clone(),
            secrets: self.secrets.clone(),
            settings: self.settings.clone(),
            identity: self.identity.clone(),
            helpers: self.helpers.clone(),
            plugins: self.plugins.clone(),
            catalog: self.catalog.clone(),
            characters: self.characters.clone(),
            stations: self.stations.clone(),
            presence: self.presence.clone(),
            placement: self.placement.clone(),
            connector_settings: self.connector_settings.clone(),
            joins: self.joins.clone(),
            #[cfg(feature = "yandex-station")]
            sessions: self.sessions.clone(),
            voice: self.voice.clone(),
            modes: self.modes.clone(),
            health: self.health.clone(),
            brain: self.brain.clone(),
            computer: self.pc.home(),
        }
    }
}

impl Provides<crate::midi::routes::Parts> for App {
    fn provide(&self) -> crate::midi::routes::Parts {
        crate::midi::routes::Parts {
            midi: self.midi.clone(),
            house: self.house.clone(),
            config: self.config.clone(),
            plugins: self.plugins.clone(),
            // A computer's stations, for their windows on the keyboard here; the station's build has none.
            stations: self
                .placement
                .computer()
                .is_some()
                .then(|| self.placement.clone() as Arc<dyn crate::midi::routes::PairedStations>),
        }
    }
}

/// The home's parts for a watch, while the gateway lives: the watch holds the gateway weakly, as it always did — the
/// bus it listens on is the gateway's, and a strong hold would keep the gateway from ever ending.
pub fn home_parts(app: &Arc<App>) -> impl Fn() -> Option<crate::home::Parts> + Clone + Send + Sync + 'static {
    let weak = Arc::downgrade(app);
    move || weak.upgrade().map(|app| app.provide())
}

impl Provides<crate::helpers::Parts> for App {
    fn provide(&self) -> crate::helpers::Parts {
        crate::helpers::Parts {
            helpers: self.helpers.clone(),
            identity: self.identity.clone(),
            secrets: self.secrets.clone(),
            brain: self.brain.clone(),
            config: self.config.clone(),
            modes: self.modes.clone(),
            placement: self.placement.clone(),
            voice: self.voice.clone(),
            own_bot: self.pc.own_bot(),
        }
    }
}

/// Each field of the gateway a module's routes may use, a line each: `field: Type,` gives `Uses<Type>` to a handler.
macro_rules! provides {
    ($($(#[$attr:meta])* $field:ident: $ty:ty,)*) => {$(
        $(#[$attr])*
        impl Provides<$ty> for App {
            fn provide(&self) -> $ty {
                self.$field.clone()
            }
        }
    )*};
}

/// The home folder's media engine for the station's `/media/v/<key>` (src/gateway/media): the folders' provider keeps
/// it, the bus keeps that provider.
impl Provides<crate::gateway::media::FolderMedia> for App {
    fn provide(&self) -> crate::gateway::media::FolderMedia {
        use crate::bus::BusFolders;
        crate::gateway::media::FolderMedia(self.bus.files().map(|files| files.media_shared()))
    }
}

/// Whether this gateway is the station's own now, for the talk's voice input (src/talk/routes.rs).
impl Provides<crate::talk::OnStation> for App {
    fn provide(&self) -> crate::talk::OnStation {
        crate::talk::OnStation(self.placement.role() == crate::placement::service::Role::Station)
    }
}

/// The placement's pair key for the station's socket (src/gateway/uniproxy/uniproxy_server.rs).
#[cfg(feature = "yandex-station")]
impl Provides<crate::gateway::uniproxy::uniproxy_server::PairKey> for App {
    fn provide(&self) -> crate::gateway::uniproxy::uniproxy_server::PairKey {
        crate::gateway::uniproxy::uniproxy_server::PairKey(self.placement.peer_key().map(str::to_string))
    }
}

/// The apps' sound and the packages' assets for the media (src/gateway/media).
impl Provides<Arc<dyn crate::gateway::media::AppMedia>> for App {
    fn provide(&self) -> Arc<dyn crate::gateway::media::AppMedia> {
        self.plugins.clone()
    }
}

// A computer's own parts — the shop, the voice lab, the calls, the link, the stations' watch, the works' seats — are
// given in src/computer; the station's build gives what stands in for them there (computer/none.rs).

/// A Турбо's files by their key for the media (src/gateway/media).
impl Provides<Arc<dyn crate::gateway::media::TurboMedia>> for App {
    fn provide(&self) -> Arc<dyn crate::gateway::media::TurboMedia> {
        self.house.clone()
    }
}

provides! {
    config: Arc<Config>,
    access: Arc<AccessService>,
    settings: Arc<SettingsService>,
    secrets: Arc<SecretsService>,
    cloud_speech: Arc<crate::speech::cloud_speech::CloudSpeech>,
    speech: Arc<crate::speech::SpeechService>,
    brain: Arc<crate::dialog::brain::BrainService>,
    oauth: Arc<crate::oauth::OAuthService>,
    personas: Arc<crate::dialog::PersonaService>,
    router: Arc<crate::router::Router>,
    talk: Arc<crate::talk::TalkLayer>,
    placement: Arc<crate::placement::PlacementService>,
    modes: Arc<crate::ear::mode::ModeService>,
    memory: Arc<crate::memory::MemoryService>,
    reflection: Arc<crate::memory::reflect::Reflection>,
    characters: Arc<crate::characters::CharactersService>,
    skills: Arc<crate::skills::SkillsService>,
    tasks: Arc<crate::tasks::Tasks>,
    sleep: Arc<crate::sleep::SleepService>,
    live: Arc<crate::live::LiveHub>,
    requests: Arc<crate::requests::RequestsService>,
    stations: Arc<crate::stations::StationsService>,
    #[cfg(feature = "yandex-station")]
    sessions: Arc<crate::gateway::uniproxy::SessionRegistry>,
    bus: Arc<crate::bus::Bus>,
    streams: Arc<crate::gateway::media::stream::StreamRelays>,
    plugins: Arc<crate::plugins::PluginHost>,
    history: Option<Arc<crate::plugins::history::History>>,
    workshop: Arc<crate::plugins::workshop::Workshop>,
    dialog: Arc<crate::dialog::DialogService>,
    catalog: Arc<crate::plugins::PackageCatalog>,
    files: Arc<crate::plugins::files::PluginFiles>,
    schedules: Arc<crate::plugins::schedule::Scheduler>,
    watch: Arc<crate::plugins::watch::WatchPages>,
    publishers: Arc<crate::capabilities::publish::Publishers>,
    voices: Arc<crate::capabilities::voices::Voices>,
    connectors: Arc<crate::connectors::ConnectorsService>,
    mcp: Arc<crate::mcp::McpService>,
    screen: Arc<crate::screen::Screen>,
    midi: Arc<crate::midi::MidiService>,
    egress: Arc<crate::egress::Egress>,
    health: Arc<crate::health_status::HealthStatusService>,
    updates: Arc<crate::updates::UpdatesService>,
    db: Arc<crate::storage::Db>,
    availability: Arc<crate::placement::AvailabilityService>,
    events: crate::core::events::EventBus,
    identity: Arc<crate::core::identity::home::Identity>,
    home_server: Arc<crate::home_server::HomeServerService>,
    connector_settings: Arc<crate::connectors::settings::ConnectorSettings>,
    voice: Arc<crate::speech::VoiceService>,
    helpers: Arc<crate::helpers::Helpers>,
    joins: Arc<crate::home::join::Joins>,
    house: Arc<crate::home::turbo_devices::HouseDevices>,
}
