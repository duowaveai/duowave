//! The computer's floor in the station's build: none. The same names as computer/mod.rs — the gateway calls them the
//! same way in both builds —, nothing behind them: the floors' own modules in place of the computer's merged ones, the
//! steps of `App::build` that make nothing, the answers the station's build gave before (docs/plans/modular-gateway.md,
//! step 9).

use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;

pub use duowave_apps::{capabilities, mcp};
pub use duowave_base::{platform, settings};
pub use duowave_brain::characters;
pub use duowave_ears::stations;
pub use duowave_home::{helpers, home, home_server, placement};
pub use duowave_voice::{models, speech, voice_lab};

use crate::access::AccessService;
use crate::app::App;
use crate::config::Config;
use crate::connectors::settings::ConnectorSettings;
use crate::core::identity::home::Identity;
use crate::dialog::brain::BrainService;
use crate::operations::station_watch::{StationWatch, TelegramSender};
use crate::platform::http::Provides;
use crate::secrets::SecretsService;
use crate::settings::SettingsService;
use crate::skills::{CallsLinks, SkillsService};
use crate::speech::VoiceService;
use crate::stations::StationsService;
use crate::storage::Db;
use crate::updates::UpdatesService;

#[cfg(test)]
#[path = "none_tests/mod.rs"]
mod tests;

/// Nothing to say: the computer's floor is not in this program.
pub fn install() {}

/// The computer's services: none.
pub struct Computer;

/// What each step of `App::build` takes of the gateway's base.
pub struct Base<'a> {
    pub config: &'a Arc<Config>,
    pub db: &'a Arc<Db>,
    pub settings: &'a Arc<SettingsService>,
    pub secrets: &'a Arc<SecretsService>,
    pub events: &'a crate::core::events::EventBus,
}

/// The steps of `App::build` that make the computer's services: here they make none.
#[derive(Default)]
pub struct Building;

impl Building {
    pub fn new() -> Building {
        Building
    }

    pub fn link(&mut self, _base: &Base<'_>) {}

    pub fn heavy(
        &mut self,
        _base: &Base<'_>,
        _profile: crate::core::build::Profile,
        _identity: &Arc<Identity>,
        _voice: &Arc<VoiceService>,
        _helpers: &Arc<crate::helpers::Helpers>,
        _brain: &Arc<BrainService>,
    ) {
    }

    pub fn stations(
        &mut self,
        _base: &Base<'_>,
        _access: &Arc<AccessService>,
        _stations: &Arc<StationsService>,
        _updates: &Arc<UpdatesService>,
        _placement: &Arc<crate::placement::PlacementService>,
    ) {
    }

    /// The station's build has no shop: only the stored provider comes back.
    pub fn models(&mut self, base: &Base<'_>, voice: &Arc<VoiceService>) {
        voice.restore_from_settings(base.settings, None);
    }

    /// No calls: the skills' calls are none.
    pub fn calls(
        &mut self,
        _base: &Base<'_>,
        _stations: &Arc<StationsService>,
        _connector_settings: &Arc<ConnectorSettings>,
    ) -> CallsLinks {
        CallsLinks::none()
    }

    pub fn skills(&self, _skills: &SkillsService) {}

    pub fn voices(&self, _voices: &crate::capabilities::voices::Voices, _secrets: &Arc<SecretsService>) {}

    pub fn works(
        &mut self,
        _base: &Base<'_>,
        _folder_executor: &Arc<crate::talk::folder::FolderExecutor>,
        _plugins: &Arc<crate::plugins::PluginHost>,
    ) {
    }

    pub fn characters(
        &self,
        _characters: &Arc<crate::characters::CharactersService>,
        _settings: &Arc<SettingsService>,
    ) {
    }

    /// The station's own gateway changes its own files (`App::build` says so itself); no way over SSH.
    #[cfg(feature = "yandex-station")]
    pub fn rescue(&self) -> Option<Arc<dyn crate::gateway::uniproxy::rescue::StationRescue>> {
        None
    }

    /// The station's build has no bot to alert through.
    pub fn telegram(&self) -> Option<Arc<dyn TelegramSender>> {
        None
    }

    pub fn station_watch(
        &self,
        _station_watch: &Arc<StationWatch>,
        _settings: &Arc<SettingsService>,
        _skills: &SkillsService,
    ) {
    }

    pub fn publishers(&self, _publishers: &crate::capabilities::publish::Publishers) {}

    pub fn done(self, _events: &crate::core::events::EventBus) -> Computer {
        Computer
    }
}

impl Computer {
    /// The home asks no computer here.
    pub fn home(&self) -> Option<Arc<dyn home::computer::ComputerHome>> {
        None
    }

    /// No calls for a connector's change to end.
    pub fn calls_sheet(&self) -> Option<Arc<dyn crate::connectors::controller::CallsSheet>> {
        None
    }

    /// No bot of its own.
    pub fn own_bot(&self) -> crate::helpers::OwnBot {
        Arc::new(|| false)
    }

    /// Stations do not announce: their RAM and space are tight, and they are found as before.
    pub fn announce(&self, _app: &App) {}

    pub fn before_helpers(&self) {}

    pub fn after_helpers(&self, _app: &App) {}

    pub fn clean(&self) {}

    pub async fn start(&self) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn goodbye(&self) {}

    pub fn persist(&self) {}

    pub fn stop_works(&self) {}

    pub async fn shutdown(&self) {}

    pub fn before_shutdown(&self) {}

    /// No link's port.
    pub fn link_port(&self) -> Option<u16> {
        None
    }

    pub fn listening(&self, _port: u16) {}
}

pub fn attach(_app: &Arc<App>) {}

/// No routes of a computer's: the station's build has no /calls.
pub fn routes(routes: Router<Arc<App>>) -> Router<Arc<App>> {
    routes
}

pub fn outermost(routes: Router, _app: &Arc<App>) -> Router {
    routes
}

/// No link's port: the station helps no home over the link.
pub struct LinkPort;

pub async fn bind_link(_app: &Arc<App>, _listener: &TcpListener) -> LinkPort {
    LinkPort
}

/// The HTTP server alone.
pub async fn serve_with(
    _app: &Arc<App>,
    _link: LinkPort,
    main: impl std::future::IntoFuture<Output = std::io::Result<()>>,
    _stop: tokio_util::sync::CancellationToken,
) -> std::io::Result<()> {
    main.await
}

/// The station's own build plays no pretend station.
pub fn pretend(config: &Config) -> anyhow::Result<Option<crate::config::PretendConfig>> {
    if config.placement.pretend.is_some() {
        anyhow::bail!("DUOWAVE_PRETEND is the computer's build's only");
    }
    Ok(None)
}

pub async fn pretend_started(_app: &Arc<App>, _pretend: Option<crate::config::PretendConfig>) {}

/// The computer's own commands (src/computer/tools): the station's build says it has none.
pub mod tools {
    use crate::tools::Usage;

    pub fn run(name: &str, _args: &[String]) -> Option<anyhow::Result<()>> {
        Some(match name {
            #[cfg(feature = "yandex-station")]
            "pretend" => Err(Usage("duowave pretend: the computer's build's only".into()).into()),
            "asr-check" => Err(Usage("duowave asr-check: not in the station's build".into()).into()),
            "call-sim" | "livekit-token" => {
                Err(Usage(format!("duowave {name}: not in this build — the station's build has no calls")).into())
            }
            _ => return None,
        })
    }
}

/// No work folders for an agent's removal to free.
impl Provides<Arc<dyn crate::plugins::routes::AgentSeats>> for App {
    fn provide(&self) -> Arc<dyn crate::plugins::routes::AgentSeats> {
        Arc::new(crate::plugins::routes::NoSeats)
    }
}
