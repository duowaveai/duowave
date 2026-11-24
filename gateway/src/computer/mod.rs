//! The computer's floor in the gateway (crates/duowave-pc; docs/plans/modular-gateway.md, step 9): what only a computer
//! has, wired in one place — its services made in their turns of `App::build`, started and stopped with the gateway,
//! their routes, their parts for the handlers, the link's port beside the HTTP one, the pretend stations, the
//! computer's own commands. The station's build has none.rs in its place: the same names, nothing behind them.

use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;

pub use duowave_pc::{
    calls, capabilities, characters, helpers, home, home_server, mcp, models, placement, platform, settings, speech,
    stations, voice_lab, works,
};

use crate::access::AccessService;
use crate::app::App;
use crate::config::Config;
use crate::connectors::settings::ConnectorSettings;
use crate::core::events::EventBus;
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

pub mod links;
pub mod tools;

#[cfg(test)]
mod tests;

/// What the computer's floor says at the program's start, before the configuration is read (`duowave_pc::install`).
pub fn install() {
    duowave_pc::install();
}

/// The computer's services in the gateway.
pub struct Computer {
    /// Who this computer helps over the link («Помогаю», helpers/clients.rs).
    pub clients: Arc<helpers::clients::Clients>,
    /// The helper's side of the link (helpers/link_serve.rs): its TLS port, the pairings, the homes' links.
    pub link: Arc<helpers::link_serve::LinkServer>,
    /// Recognition on this computer itself (speech/asr/local), when it hears by itself (ASR_PROVIDER=local).
    pub local_asr: Option<Arc<speech::asr::local::supervisor::LocalAsr>>,
    /// The own voice's server the desktop app sets up and keeps (speech/tts/managed.rs).
    pub managed_tts: Option<Arc<speech::tts::managed::ManagedTts>>,
    /// The built-in helper (helpers/builtin.rs): this computer's heavy part behind the helpers' contract —
    /// `/home/v1` answers through it, and a computer that is not in the station role takes its own parts through it.
    pub builtin: Arc<helpers::builtin::BuiltinHelper>,
    /// The panel's wizard that connects a station over SSH.
    pub station_connect: Arc<stations::connect::station_connect::StationConnectService>,
    /// The owner's own voices (voice_lab).
    pub voice_lab: Arc<voice_lab::VoiceLabService>,
    /// The computer's watches over the stations' gateways (one per station with a placement of its own), and the
    /// moves between the computer and a station.
    pub station_gateway: Arc<placement::watch::StationGateways>,
    pub moves: Arc<placement::moves::MovesService>,
    /// What the placement asks of this computer (`placement::computer::PlacementComputer`).
    pub placement: Arc<placement::computer::Computer>,
    /// The model shop: the catalog, the voices on disk, the local recognition and voice servers.
    pub models: Arc<models::ModelsService>,
    /// Our own calls (calls): the hub, the station clients, the bridges, the Telegram bot.
    pub calls: Arc<calls::CallsService>,
    /// The works in folders, the helper's heavy part (works): work folders, agents, work copies, journals of runs.
    pub works: Option<Arc<works::FolderWorks>>,
    /// What the home asks of this computer (`home::computer::ComputerHome`): its stations, its shop and voice lab,
    /// its link.
    pub home: Arc<home::computer::Computer>,
}

/// What each step of `App::build` takes of the gateway's base.
pub struct Base<'a> {
    pub config: &'a Arc<Config>,
    pub db: &'a Arc<Db>,
    pub settings: &'a Arc<SettingsService>,
    pub secrets: &'a Arc<SecretsService>,
    pub events: &'a EventBus,
}

/// The computer's services as `App::build` makes them, in their turns.
#[derive(Default)]
pub struct Building {
    clients: Option<Arc<helpers::clients::Clients>>,
    link: Option<Arc<helpers::link_serve::LinkServer>>,
    local_asr: Option<Arc<speech::asr::local::supervisor::LocalAsr>>,
    managed_tts: Option<Arc<speech::tts::managed::ManagedTts>>,
    builtin: Option<Arc<helpers::builtin::BuiltinHelper>>,
    station_connect: Option<Arc<stations::connect::station_connect::StationConnectService>>,
    voice_lab: Option<Arc<voice_lab::VoiceLabService>>,
    station_gateway: Option<Arc<placement::watch::StationGateways>>,
    moves: Option<Arc<placement::moves::MovesService>>,
    placement: Option<Arc<placement::computer::Computer>>,
    models: Option<Arc<models::ModelsService>>,
    calls: Option<Arc<calls::CallsService>>,
    works: Option<Arc<works::FolderWorks>>,
}

/// A step's part that an earlier step made.
const MADE: &str = "made by an earlier step of App::build";

impl Building {
    pub fn new() -> Building {
        Building::default()
    }

    /// This computer as a helper (stage 7): who it helps, and its side of the link — its certificate made at the first
    /// start of a computer that helps (never in the station role).
    pub fn link(&mut self, base: &Base<'_>) {
        let station = crate::core::build::Profile::for_role(base.config.placement.role.as_deref()).station;
        let clients =
            Arc::new(helpers::clients::Clients::new(base.settings.clone(), base.secrets.clone(), base.events.clone()));
        let link = Arc::new(helpers::link_serve::LinkServer::new(
            base.config.clone(),
            base.secrets.clone(),
            base.settings.clone(),
            base.events.clone(),
            clients.clone(),
            !station,
        ));
        self.clients = Some(clients);
        self.link = Some(link);
    }

    /// Recognition on this computer itself, the own voice's server, the built-in helper — this computer's heavy part.
    pub fn heavy(
        &mut self,
        base: &Base<'_>,
        profile: crate::core::build::Profile,
        identity: &Arc<Identity>,
        voice: &Arc<VoiceService>,
        helpers: &Arc<crate::helpers::Helpers>,
        brain: &Arc<BrainService>,
    ) {
        let config = base.config;
        // Recognition on this computer itself: its engine in a worker of this program, the model fetched once
        // (speech/asr/local). Started with the gateway's start; the models' shop says which model.
        let local_asr = match &config.speech.asr.local {
            Some(local) if config.speech.asr.enabled && !profile.station => {
                use speech::asr::local::supervisor::{LocalAsr, LocalAsrOptions};
                let mut options = LocalAsrOptions::new(
                    local.dir.clone(),
                    local.mirror.clone(),
                    config.speech.asr.language.clone(),
                    local.gpu,
                );
                // It hears the household's language (unless ASR_LANGUAGE says one), read from the setting itself:
                // the models' shop looks again when it changes (models).
                let household = base.settings.clone();
                options.household = Arc::new(move || crate::locale::household(&household));
                match LocalAsr::new(options) {
                    Ok(asr) => Some(asr),
                    Err(error) => {
                        tracing::error!("local recognition cannot start: {error:#}");
                        None
                    }
                }
            }
            _ => None,
        };
        // The own voice's server, the app's own: set up when a Silero or Piper voice is chosen, kept while one speaks.
        let managed_tts = match &config.models.tts_managed {
            Some(managed) if !profile.station => {
                use speech::tts::managed::{ManagedTts, ManagedTtsOptions};
                let options = ManagedTtsOptions {
                    dir: managed.dir.clone(),
                    // Beside the program: the app's resources (its working folder), the repository's folder elsewhere.
                    server_dir: std::path::absolute("tts-server").unwrap_or_else(|_| "tts-server".into()),
                    models_dir: config.models.tts_dir.clone(),
                    piper_root: managed.piper_root.clone(),
                    port: managed.port,
                };
                Some(ManagedTts::new(options, base.events.clone()))
            }
            _ => None,
        };
        // The built-in helper (helpers/builtin.rs): this computer's heavy part behind the helpers' contract, made in
        // both roles — `/home/v1` answers through it —; a computer that is not in the station role takes its own
        // hearing, voice and model through it, first among its helpers.
        let builtin = helpers::builtin::BuiltinHelper::new(helpers::builtin::BuiltinParts {
            config: config.clone(),
            db: base.db.clone(),
            settings: base.settings.clone(),
            secrets: base.secrets.clone(),
            events: base.events.clone(),
            identity: identity.clone(),
            local_asr: local_asr.clone(),
            voice: voice.clone(),
            local: helpers.local(),
        });
        if !profile.station {
            helpers.set_own(builtin.clone());
            // «Мозг» sees this computer's model servers now: a main model on one of them gets its default backup.
            brain.keep_backup();
        }
        self.local_asr = local_asr;
        self.managed_tts = managed_tts;
        self.builtin = Some(builtin);
    }

    /// The station connect over SSH, the voice lab, the computer's watch over its stations' gateways and the moves
    /// between the two; what the placement asks of this computer.
    pub fn stations(
        &mut self,
        base: &Base<'_>,
        access: &Arc<AccessService>,
        stations: &Arc<StationsService>,
        updates: &Arc<UpdatesService>,
        placement: &Arc<crate::placement::PlacementService>,
    ) {
        use placement::moves::{MovesDeps, MovesService};
        use stations::connect::station_connect::{ShellConfig, StationConnectService, StationPasswords};
        use stations::connect::station_shell::RusshOpener;
        let Base { config, db, settings, secrets, events } = *base;
        let ssh = || ShellConfig {
            port: config.gateway.station_ssh.port,
            user: config.gateway.station_ssh.user.clone(),
            password: config.gateway.station_ssh.password.clone(),
            passwords: StationPasswords::new(secrets.clone()),
        };
        let station_connect = Arc::new(StationConnectService::new(
            settings.clone(),
            stations.clone(),
            events.clone(),
            Arc::new(RusshOpener),
            ssh(),
            config.gateway.uniproxy_path.clone(),
            // The port the station dials: the published one in a container, else the gateway's own.
            config.app.public_port.unwrap_or(config.app.port),
            Default::default(),
        ));
        let voice_lab = voice_lab::VoiceLabService::new(config, events.clone());
        // The station's gateway as this computer sees it, and the moves between the two (placement).
        let station_gateway = placement::watch::StationGateways::new(
            config.clone(),
            db.clone(),
            settings.clone(),
            secrets.clone(),
            events.clone(),
        );
        // What «Обновить колонку» does depends on this computer's own updates (placement/watch.rs).
        station_gateway.set_updates(updates.clone());
        let moves = MovesService::new(MovesDeps {
            config: config.clone(),
            db: db.clone(),
            settings: settings.clone(),
            secrets: secrets.clone(),
            events: events.clone(),
            access: access.clone(),
            opener: Arc::new(RusshOpener),
            ssh: ssh(),
            station: station_gateway.clone(),
            stations: stations.clone(),
            restarter: crate::placement::service::restart_soon(),
            timing: Default::default(),
            payload: placement::moves::this_computers_build(),
        });
        // What the placement asks of this computer: its stations, the move going now, the addresses a station dials.
        let computer = Arc::new(placement::computer::Computer::new(
            station_gateway.clone(),
            moves.clone(),
            station_connect.clone(),
        ));
        placement.set_computer(computer.clone());
        self.station_connect = Some(station_connect);
        self.voice_lab = Some(voice_lab);
        self.station_gateway = Some(station_gateway);
        self.moves = Some(moves);
        self.placement = Some(computer);
    }

    /// The voice, the model shop and the own voices (models.module.ts, voice-lab.module.ts): the shop chooses through
    /// the voice, an own voice's route comes from the voice lab, the voice lab asks which voice is chosen. The stored
    /// choice speaks from now on, resolved through the shop's catalog in the database; the shop's start resolves it
    /// again once it has read the registry.
    pub fn models(&mut self, base: &Base<'_>, voice: &Arc<VoiceService>) {
        let voice_lab = self.voice_lab.as_ref().expect(MADE);
        voice_lab.link_speech(Arc::new(links::voice::LabSpeech(voice.clone())));
        let driven = models::ModelsLinks {
            voice: Some(Arc::new(links::voice::ShopVoice(voice.clone()))),
            own_voices: Some(Arc::new(links::voice::LabVoices(voice_lab.clone()))),
            local_asr: self.local_asr.clone(),
            managed_tts: self.managed_tts.clone(),
        };
        let models = Arc::new(models::ModelsService::new(
            base.config,
            base.db.clone(),
            base.events.clone(),
            base.settings.clone(),
            driven,
        ));
        voice.restore_from_settings(base.settings, Some(models.as_ref()));
        self.models = Some(models);
    }

    /// Our calls (calls.module.ts): the stations say which address is an approved station's; the codec is speech's;
    /// the bot and the bridges are the panel's and follow its changes. What the skills take of them.
    pub fn calls(
        &mut self,
        base: &Base<'_>,
        stations: &Arc<StationsService>,
        connector_settings: &Arc<ConnectorSettings>,
    ) -> CallsLinks {
        let calls = calls::CallsService::new(
            &base.config.calls,
            base.events.clone(),
            stations.clone(),
            Arc::new(calls::SpeechOpus),
            Arc::new(links::calls::CallsSettings(connector_settings.clone())),
        );
        links::calls::follow_settings(&calls, connector_settings);
        let skills = links::calls::calls_links(&calls);
        self.calls = Some(calls);
        skills
    }

    /// The calls' events told to the calls skill.
    pub fn skills(&self, skills: &SkillsService) {
        links::calls::report_calls(self.calls.as_ref().expect(MADE), skills);
    }

    /// ElevenLabs by the owner's key among the voices of the apps' lines.
    pub fn voices(&self, voices: &crate::capabilities::voices::Voices, secrets: &Arc<SecretsService>) {
        voices.register(Arc::new(capabilities::voices::ElevenLabs::new(secrets)));
    }

    /// A work in a work folder (docs/plans/router-and-works.md, 10): the executor's helper — the works' module.
    pub fn works(
        &mut self,
        base: &Base<'_>,
        folder_executor: &Arc<crate::talk::folder::FolderExecutor>,
        plugins: &Arc<crate::plugins::PluginHost>,
    ) {
        let config = base.config;
        let dir = config.plugins.dev_dir.with_file_name("works");
        let mut guarded: Vec<std::path::PathBuf> = vec![
            config.plugins.packages_dir.clone(),
            config.plugins.dev_dir.clone(),
            config.plugins.catalog_dir.clone(),
            dir.clone(),
        ];
        guarded.extend(config.data_dir.clone());
        guarded.extend(config.skills.state_dir.clone());
        let works = works::FolderWorks::new(
            base.db.clone(),
            dir,
            guarded,
            base.settings.clone(),
            base.secrets.clone(),
            base.events.clone(),
        );
        folder_executor.attach(works.clone());
        // The agents' cards from MagicPlace: only those installed from the catalog (works/agents.rs).
        let host = Arc::downgrade(plugins);
        works.agents.set_cards(Arc::new(move || {
            let Some(host) = host.upgrade() else { return Vec::new() };
            host.agent_cards()
                .into_iter()
                .map(|(id, title, description, card)| works::agents::Card { id, title, description, card })
                .collect()
        }));
        self.works = Some(works);
    }

    /// The characters' voices on this computer (characters/voices.rs): the voice lab's, the shop's.
    pub fn characters(&self, characters: &Arc<crate::characters::CharactersService>, settings: &Arc<SettingsService>) {
        let voice_lab = self.voice_lab.clone().expect(MADE);
        characters.link_voices(characters::voices::CharacterVoices::new(
            voice_lab.clone(),
            self.models.clone().expect(MADE),
            Arc::new(links::voice::LabVoices(voice_lab)),
            settings.clone(),
        ));
    }

    /// The emergency words' way back on a computer's gateway (gateway/uniproxy/rescue.rs): over SSH, as «Вернуть к
    /// Яндексу» does.
    #[cfg(feature = "yandex-station")]
    pub fn rescue(&self) -> Option<Arc<dyn crate::gateway::uniproxy::rescue::StationRescue>> {
        let connect = self.station_connect.clone().expect(MADE);
        Some(Arc::new(stations::connect::station_connect::OverSsh(connect)))
    }

    /// The station watch's alerts through the calls' Telegram bot.
    pub fn telegram(&self) -> Option<Arc<dyn TelegramSender>> {
        let calls = self.calls.as_ref().expect(MADE);
        Some(Arc::new(links::calls::TelegramAlerts(calls.telegram.clone())))
    }

    /// What the computer's stations' gateways take of the station watch, the settings and the skills.
    pub fn station_watch(
        &self,
        station_watch: &Arc<StationWatch>,
        settings: &Arc<SettingsService>,
        skills: &SkillsService,
    ) {
        let station_gateway = self.station_gateway.as_ref().expect(MADE);
        // This computer's own addresses, for the station's home server: told to it when they change.
        let connect = self.station_connect.clone().expect(MADE);
        station_gateway.set_home_addresses(Arc::new(move || {
            connect.home_addresses(
                &stations::connect::station_connect::host_interfaces(),
                stations::connect::station_connect::in_container(),
            )
        }));
        // The station's gateway and the safety net tell the household the way the station watch does.
        let watch = station_watch.clone();
        station_gateway.set_teller(Arc::new(move |text: String| {
            let watch = watch.clone();
            tokio::spawn(async move { watch.tell(&text).await });
        }));
        // While every station's gateway runs on it, none is missing at this computer's /uni.ws. Weak: the watches hold
        // the station watch already (its teller), and an engine started again leaves neither behind. Nor is one awaited
        // while the household talks by this computer's microphone (docs/plans/mic-mode.md): «колонка не подключена…
        // вернуть к Яндексу» every five minutes to someone with no station is noise (live 14.11).
        let gateways = Arc::downgrade(station_gateway);
        let talk_settings = Arc::downgrade(settings);
        station_watch.set_elsewhere(Arc::new(move || {
            talk_settings.upgrade().is_some_and(|settings| crate::talk::input::on_computer(&settings))
                || gateways.upgrade().is_some_and(|gateways| gateways.none_here())
        }));
        // The bot's messages reach the stations this computer helps: they are not at its /uni.ws (live 11.11).
        skills.calls.attach_relay(placement::messages::StationMessages::new(station_gateway));
    }

    /// The calls' Telegram bot among the publishers: where a file can go.
    pub fn publishers(&self, publishers: &crate::capabilities::publish::Publishers) {
        let calls = self.calls.as_ref().expect(MADE);
        publishers.register(Arc::new(calls::publish::Telegram { bot: calls.telegram.clone() }));
    }

    /// The computer's services, made.
    pub fn done(self, events: &EventBus) -> Computer {
        let station_gateway = self.station_gateway.expect(MADE);
        let models = self.models.expect(MADE);
        let voice_lab = self.voice_lab.expect(MADE);
        let link = self.link.expect(MADE);
        let clients = self.clients.expect(MADE);
        let home = Arc::new(home::computer::Computer {
            station_gateway: station_gateway.clone(),
            models: models.clone(),
            voice_lab: voice_lab.clone(),
            link: link.clone(),
            clients: clients.clone(),
            events: events.clone(),
        });
        Computer {
            clients,
            link,
            local_asr: self.local_asr,
            managed_tts: self.managed_tts,
            builtin: self.builtin.expect(MADE),
            station_connect: self.station_connect.expect(MADE),
            voice_lab,
            station_gateway,
            moves: self.moves.expect(MADE),
            placement: self.placement.expect(MADE),
            models,
            calls: self.calls.expect(MADE),
            works: self.works,
            home,
        }
    }
}

impl Computer {
    /// What the home asks of this computer (`home::Parts::computer`).
    pub fn home(&self) -> Option<Arc<dyn home::computer::ComputerHome>> {
        Some(self.home.clone())
    }

    /// The calls a connector's change would end, as its sheet sees them (`connectors::controller::Parts::calls`).
    pub fn calls_sheet(&self) -> Option<Arc<dyn crate::connectors::controller::CallsSheet>> {
        Some(self.calls.clone())
    }

    /// Whether this computer's own Telegram bot runs (`helpers::Parts::own_bot`).
    pub fn own_bot(&self) -> crate::helpers::OwnBot {
        let calls = self.calls.clone();
        Arc::new(move || calls.telegram.enabled())
    }

    /// «Поиск дома» (docs/plans/home-setup-flow.md, stage 6): this computer or server says what it is on the home
    /// network — a centre, a Turbo, an ear — so another computer finds its home on any port. Never by a pretend
    /// gateway: the home network does not hear of it (docs/plans/pretend-stations.md).
    pub fn announce(&self, app: &App) {
        if app.config.placement.pretend.is_none() {
            home::announce::start(Provides::provide(app), self.home.clone());
        }
    }

    /// Before the helpers start: whether others can reach this computer on Windows (the network's category, the
    /// firewall's rules), to the log; the built-in helper follows what it declares (the helpers' tick looks at its
    /// model servers).
    pub fn before_helpers(&self) {
        platform::windows_net::start();
        self.builtin.start();
    }

    /// After the helpers start: a Turbo at home that moved to another address is found again by its announcement
    /// (stage 6); the stations' gateways watched, the live channel of each relayed to this gateway's panels.
    pub fn after_helpers(&self, app: &App) {
        helpers::search_moved(&app.helpers);
        self.station_gateway.start();
        placement::relay::StationRelay::start(self.station_gateway.clone(), &app.live, &app.events);
    }

    /// The works in folders: old works, journals and copies cleaned.
    pub fn clean(&self) {
        if let Some(works) = &self.works {
            works.start_cleaning();
        }
    }

    /// The voice lab, the calls, the shop; recognition on this computer (the model the shop chose is fetched and
    /// loaded in the background) and the app's own voice server (kept running while a Silero or Piper voice speaks —
    /// the shop said which).
    pub async fn start(&self) -> anyhow::Result<()> {
        self.voice_lab.start();
        self.calls.start();
        self.models.start().await?;
        if let Some(local) = &self.local_asr {
            local.start();
        }
        if let Some(tts) = &self.managed_tts {
            tts.start();
        }
        Ok(())
    }

    /// Goodbye to the home network's caches: this gateway is not there to be found any more.
    pub async fn goodbye(&self) {
        home::announce::stop().await;
    }

    /// Who this computer helped, and when it last saw them.
    pub fn persist(&self) {
        self.clients.persist_seen();
    }

    /// The agents in work folders end their turns: their works are interrupted, «продолжай» goes on.
    pub fn stop_works(&self) {
        if let Some(works) = &self.works {
            works.stop_all();
        }
    }

    /// Recognition's worker and the own voice's server end with the gateway: the desktop app restarts the gateway
    /// inside its process, and an old gateway's processes must not stay behind with their models.
    pub async fn shutdown(&self) {
        if let Some(local) = &self.local_asr {
            local.shutdown().await;
        }
        if let Some(tts) = &self.managed_tts {
            tts.shutdown().await;
        }
    }

    /// The gateway stops: the calls' station clients and sockets end; the homes' links hear that this helper stops
    /// (1001), this gateway's own links close (1000).
    pub fn before_shutdown(&self) {
        self.calls.stop();
        self.link.close_all();
    }

    /// The port the link listens on (a computer that helps), once bound.
    pub fn link_port(&self) -> Option<u16> {
        self.link.port()
    }

    /// The port the gateway listens on: imprint.py hears the words of a take through this gateway when recognition is
    /// inside it.
    pub fn listening(&self, port: u16) {
        self.voice_lab.set_local_port(port);
    }
}

/// What the computer takes of the gateway once it is built: the built-in helper's late parts, the link's door, the
/// home this computer is the Turbo of for /health's `placement.centre`. Each holds the gateway weakly.
pub fn attach(app: &Arc<App>) {
    let pc = &app.pc;
    // The built-in helper's late parts: the bot, the apps, their history, the catalog (stage 6).
    pc.builtin.attach(Arc::new(links::helpers::Late(Arc::downgrade(app))));
    // The link's door (stage 7): who this home is, the built-in helper and the voice — a pairing's answer, the status a
    // home sees —, and `/home/v1` for a link's requests, made for each link.
    let weak = Arc::downgrade(app);
    pc.link.attach(helpers::link_serve::LinkParts {
        identity: app.identity.clone(),
        builtin: pc.builtin.clone(),
        voice: app.voice.clone(),
        home_v1: Arc::new(move || weak.upgrade().map(|app| home_server::serve::router().with_state(app))),
    });
    // The home this computer is the Turbo of over the link, for /health's `placement.centre`.
    let weak = Arc::downgrade(app);
    pc.placement.set_centre(Arc::new(move || {
        weak.upgrade().and_then(|app| home::devices::link_centre(&app.pc.home, &Provides::provide(&app)))
    }));
}

/// The computer's routes: our calls, Windows' reach and the report, the works in folders (folders, agents, runs and
/// their journals), the helper's pass to the station's gateway.
pub fn routes(routes: Router<Arc<App>>) -> Router<Arc<App>> {
    routes
        .merge(calls::router())
        .merge(platform::windows_net::routes())
        .merge(platform::report::routes())
        .merge(works::routes::routes())
        .merge(placement::proxy::routes())
}

/// Outermost: the link's door for a proxy on this computer (a plain upgrade to /home/v1/link) — the proxy passes the
/// site's name, which the Host check does not know.
pub fn outermost(routes: Router, app: &Arc<App>) -> Router {
    routes.layer(axum::middleware::from_fn_with_state(app.pc.link.clone(), helpers::link_serve::link_socket))
}

/// The link's port beside the HTTP one (helpers/link_serve.rs), bound with it.
pub struct LinkPort(Option<helpers::link_serve::LinkListener>);

pub async fn bind_link(app: &Arc<App>, listener: &TcpListener) -> LinkPort {
    helpers::link_serve::raise_open_files();
    LinkPort(helpers::link_serve::bind(&app.pc.link, &app.config, listener).await)
}

/// The HTTP server, and the link's port beside it until `stop`.
pub async fn serve_with(
    app: &Arc<App>,
    link: LinkPort,
    main: impl std::future::IntoFuture<Output = std::io::Result<()>>,
    stop: tokio_util::sync::CancellationToken,
) -> std::io::Result<()> {
    match link.0 {
        Some(port) => {
            let link = helpers::link_serve::serve(app.pc.link.clone(), port, stop.cancelled_owned());
            let (served, linked) = tokio::join!(main.into_future(), link);
            if let Err(error) = linked {
                tracing::warn!("link: the port stopped: {error}");
            }
            served
        }
        None => main.await,
    }
}

/// A pretend station or the Turbo beside them (docs/plans/pretend-stations.md): its search and its connections.
pub fn pretend(config: &Config) -> anyhow::Result<Option<crate::config::PretendConfig>> {
    let pretend = config.placement.pretend.clone();
    if let Some(pretend) = &pretend {
        stations::pretend::install(pretend);
    }
    Ok(pretend)
}

/// The pretend parts beside the gateway, once it started.
pub async fn pretend_started(app: &Arc<App>, pretend: Option<crate::config::PretendConfig>) {
    if let Some(pretend) = &pretend {
        links::stations::pretend_entry().await;
        stations::pretend::start(&app.identity, pretend).await;
    }
}

// ── The computer's parts for the handlers (`Uses<…>`, src/links/http.rs) ──

macro_rules! provides {
    ($($field:ident: $ty:ty,)*) => {$(
        impl Provides<$ty> for App {
            fn provide(&self) -> $ty {
                self.pc.$field.clone()
            }
        }
    )*};
}

provides! {
    voice_lab: Arc<voice_lab::VoiceLabService>,
    builtin: Arc<helpers::builtin::BuiltinHelper>,
    models: Arc<models::ModelsService>,
    local_asr: Option<Arc<speech::asr::local::supervisor::LocalAsr>>,
    station_connect: Arc<stations::connect::station_connect::StationConnectService>,
    calls: Arc<calls::CallsService>,
    works: Option<Arc<works::FolderWorks>>,
    station_gateway: Arc<placement::watch::StationGateways>,
    moves: Arc<placement::moves::MovesService>,
    clients: Arc<helpers::clients::Clients>,
    link: Arc<helpers::link_serve::LinkServer>,
}

impl Provides<home_server::serve::Door> for App {
    fn provide(&self) -> home_server::serve::Door {
        home_server::serve::Door { clients: self.pc.clients.clone(), voice: self.voice.clone() }
    }
}

/// This computer's own Telegram bot for the station's «передай …» (calls).
impl Provides<home_server::serve::OwnBot> for App {
    fn provide(&self) -> home_server::serve::OwnBot {
        home_server::serve::OwnBot(Arc::new(links::calls::SkillsTelegram(self.pc.calls.telegram.clone())))
    }
}

/// The work folders an agent's removal frees (works).
impl Provides<Arc<dyn crate::plugins::routes::AgentSeats>> for App {
    fn provide(&self) -> Arc<dyn crate::plugins::routes::AgentSeats> {
        Arc::new(works::folders::Seats(self.settings.clone()))
    }
}
