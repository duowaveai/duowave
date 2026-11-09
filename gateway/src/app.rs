//! The gateway assembled: its services built in order (config, database, events, secrets, settings, access, then
//! the features on top), the links between them (src/links) and its HTTP routes. A new module adds its service here
//! (a field and a line in `build`), what it asks of the others in src/links, and its routes in `router`.
//!
//! The `*.module.ts` and `*.ts` named below are the Node gateway's, removed: git tag `node-gateway-last`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use tokio::net::TcpListener;

use crate::access::AccessService;
use crate::characters::CharactersService;
use crate::config::Config;
use crate::connectors::settings::ConnectorSettings;
use crate::connectors::{ConnectorLinks, ConnectorsService};
use crate::core::events::EventBus;
use crate::dialog::brain::BrainService;
use crate::dialog::{BrainSource, DialogService, PersonaService};
use crate::gateway::mode::ModeService;
use crate::gateway::protocol::aapi_codec::AapiCodec;
use crate::gateway::uniproxy::{SessionDeps, SessionRegistry, StationLinkHealth};
use crate::health_status::HealthStatusService;
use crate::links;
use crate::live::LiveHub;
use crate::mcp::McpService;
#[cfg(feature = "pc")]
use crate::models::{ModelsLinks, ModelsService};
use crate::operations::station_watch::{ActiveSessions, TelegramSender};
use crate::operations::{RetentionService, StationWatch};
use crate::placement::{AvailabilityService, PlacementService};
use crate::platform::http::api::{error_bodies, not_found};
use crate::platform::http::security::{security, AllowedHosts};
use crate::plugins::{CatalogDeps, PackageCatalog, PluginHost, PluginsDeps};
use crate::requests::{ModeSource, RequestsService};
use crate::secrets::SecretsService;
use crate::settings::SettingsService;
use crate::skills::{SkillsDeps, SkillsService};
use crate::speech::{SpeechService, VoiceService};
use crate::stations::{PresenceService, StationsService};
use crate::storage::Db;
use crate::updates::UpdatesService;
use crate::utils::time::now_iso;
#[cfg(feature = "pc")]
use crate::voice_lab::VoiceLabService;

pub struct App {
    pub config: Arc<Config>,
    pub db: Arc<Db>,
    pub events: EventBus,
    pub secrets: Arc<SecretsService>,
    pub settings: Arc<SettingsService>,
    pub access: Arc<AccessService>,
    /// The requests going now and the journal; the `RequestStarter` sessions start requests with.
    pub requests: Arc<RequestsService>,
    /// The panel's live channel.
    pub live: Arc<LiveHub>,
    pub health: Arc<HealthStatusService>,
    pub connectors: Arc<ConnectorsService>,
    /// Home Assistant, Telegram, LiveKit, SIP, the weather and the news as the panel sets them up.
    pub connector_settings: Arc<ConnectorSettings>,
    /// The names requests may come by: the listed ones and LiveKit's from the panel.
    pub hosts: Arc<AllowedHosts>,
    pub updates: Arc<UpdatesService>,
    /// Alisa's character: the cards and the prompt read on every question.
    pub personas: Arc<PersonaService>,
    /// The model providers from the panel and the choice of models («Мозг»).
    pub brain: Arc<BrainService>,
    /// Answers with the model: phrases for speech, the history per device.
    pub dialog: Arc<DialogService>,
    /// The model shop (a PC's build): the catalog, the voices on disk, the local recognition and voice servers.
    #[cfg(feature = "pc")]
    pub models: Arc<ModelsService>,
    /// VAD, recognition and a pipeline per voice request.
    pub speech: Arc<SpeechService>,
    /// Whose voice answers, and our own synthesis.
    pub voice: Arc<VoiceService>,
    /// MCP servers: their connections, and the tools the model is offered.
    pub mcp: Arc<McpService>,
    /// Our skills and the router that picks, per request, ours, Yandex's or "not yet".
    pub skills: Arc<SkillsService>,
    /// The plugins (src/plugins): their packages, the admin's choices, the code of those that are on.
    pub plugins: Arc<PluginHost>,
    /// The packages' catalog and the installer (src/plugins/catalog.rs): one feed for every kind.
    pub catalog: Arc<PackageCatalog>,
    /// Streams the gateway relays for the station: the fallback for one it keeps silent on (gateway/media/stream.rs).
    pub streams: Arc<crate::gateway::media::stream::StreamRelays>,
    /// The workshop (src/plugins/workshop): drafts, the sandbox, the agent, the install.
    pub workshop: Arc<crate::plugins::workshop::Workshop>,
    /// The characters (src/characters): the built-in ones and the packages', and their voices.
    pub characters: Arc<CharactersService>,
    pub stations: Arc<StationsService>,
    pub presence: Arc<PresenceService>,
    pub station_watch: Arc<StationWatch>,
    pub retention: Arc<RetentionService>,
    /// The panel's wizard that connects a station over SSH: PC builds only (the station build has no SSH).
    #[cfg(feature = "pc")]
    pub station_connect: Arc<crate::stations::connect::station_connect::StationConnectService>,
    /// The owner's own voices (src/voice_lab): a PC's part.
    #[cfg(feature = "pc")]
    pub voice_lab: Arc<VoiceLabService>,
    /// Our own calls (src/calls): the hub, the station clients, the bridges, the Telegram bot.
    #[cfg(feature = "pc")]
    pub calls: Arc<crate::calls::CallsService>,
    /// Where this gateway runs, and its state record (src/placement).
    pub placement: Arc<PlacementService>,
    /// What works where, now (src/placement/availability.rs): the panels and the mode switch ask it.
    pub availability: Arc<AvailabilityService>,
    /// The station's home server (src/home_server): the computer it leans on, while it is up.
    pub home_server: Arc<crate::home_server::HomeServerService>,
    /// The computer's watches over the stations' gateways (one per station with a placement of its own), and the
    /// moves between the computer and a station.
    #[cfg(feature = "pc")]
    pub station_gateway: Arc<crate::placement::watch::StationGateways>,
    #[cfg(feature = "pc")]
    pub moves: Arc<crate::placement::moves::MovesService>,
    /// The station link (src/gateway/mode, src/gateway/uniproxy): the station's mode, and its live sessions.
    pub modes: Arc<ModeService>,
    pub sessions: Arc<SessionRegistry>,
    /// When this gateway started: uptime, and the stations' «reconnecting».
    pub started: Instant,
    pub started_at: String,
}

impl App {
    /// What runs once the services are built and before the first request, as the Node gateway's module starts
    /// did: the update checks, the model catalog seeded and the stored choices restored. The program and the
    /// desktop app both call it after `build`.
    pub async fn start(self: &Arc<Self>) -> anyhow::Result<()> {
        // Background work the stations need: the records' sync listener, the presence tracker, the offline watch,
        // the journal retention.
        self.stations.start();
        self.presence.start();
        // Where the gateway runs: the safety net's news on the station, the watch over the station's gateway here.
        self.placement.start();
        self.home_server.start();
        #[cfg(feature = "pc")]
        self.station_gateway.start();
        self.station_watch.start();
        self.retention.start();
        self.updates.spawn_checks();
        // The reminders' scheduler and the routes' listener.
        self.skills.start();
        // The plugins' idle unloading and the events they follow.
        self.plugins.start();
        // The packages' characters in the table, their voices in the voice lab; followed as packages come and go.
        self.characters.start();
        // The station link: the skills speak through the live stations; a mode switch reconnects them.
        self.sessions.start();
        #[cfg(feature = "pc")]
        self.voice_lab.start();
        #[cfg(feature = "pc")]
        self.calls.start();
        #[cfg(feature = "pc")]
        self.models.start().await?;
        Ok(())
    }

    /// Every service, in order. The database is migrated here. Runs inside a tokio runtime: services start their
    /// listeners on the bus and their timers here.
    pub fn build(config: Config) -> anyhow::Result<Arc<App>> {
        let config = Arc::new(config);
        let state_dir = config.skills.state_dir.clone();
        let db = Arc::new(Db::open(state_dir.as_deref())?);
        if let Some(file) = db.file() {
            tracing::info!("database: {} ({} migration(s) applied)", file.display(), db.applied_migrations());
        }
        if let Some(backup) = db.backup() {
            tracing::info!("database copied before the migrations: {}", backup.display());
        }
        let events = EventBus::new();
        let secrets = Arc::new(SecretsService::new(db.clone(), state_dir.as_deref())?);
        // Settings the other gateway of the pair sent (src/placement/transfer.rs): applied before anything reads them.
        if let Some(dir) = state_dir.as_deref() {
            let station = crate::core::build::Profile::for_role(config.placement.role.as_deref()).station;
            crate::placement::transfer::apply_incoming(dir, &db, &secrets, station);
        }
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let access = Arc::new(AccessService::new(db.clone(), events.clone(), settings.clone(), &secrets)?);
        access.start();
        // The connectors the panel sets up (src/connectors/settings.rs): .env seeds the ones never stored, once.
        let connector_settings = Arc::new(ConnectorSettings::new(
            &config.connectors,
            settings.clone(),
            secrets.clone(),
            events.clone(),
            config.calls.telegram_inbox.clone(),
        ));
        let hosts = Arc::new(AllowedHosts::new(config.app.allowed_hosts.clone()));
        links::connectors::follow_livekit_hosts(&connector_settings, &hosts);
        // The station's mode (src/gateway/mode), read by the requests, /health and the connectors. Its view says whose
        // voice speaks, so the voice is built here, before them.
        let voice = Arc::new(VoiceService::new(&config.gateway, events.clone()));
        // Where the gateway runs and what works there (src/placement): the mode switch asks the registry.
        let profile = crate::core::build::Profile::for_role(config.placement.role.as_deref());
        let placement = Arc::new(PlacementService::new(
            profile,
            config.clone(),
            db.clone(),
            secrets.clone(),
            settings.clone(),
            events.clone(),
        ));
        let availability = AvailabilityService::new(&config, profile, events.clone());
        availability.set_placement(placement.clone());
        // The station's home server: its state feeds what works where and /health; our voice goes through it first.
        let home_server = crate::home_server::HomeServerService::new(
            config.clone(),
            settings.clone(),
            secrets.clone(),
            events.clone(),
        );
        availability.set_home_server(home_server.clone());
        placement.set_home_server(home_server.clone());
        if profile.station {
            voice.set_home(home_server.clone());
        }
        let mode_service = Arc::new(ModeService::new(
            config.gateway.mode,
            settings.clone(),
            events.clone(),
            voice.clone(),
            availability.clone(),
        ));
        let modes: Arc<dyn ModeSource> = mode_service.clone();
        availability.set_modes(modes.clone());
        // The gateway's own switch out of «Свой» and back, as its home server goes and comes.
        home_server.set_modes(mode_service.clone());
        availability.start(&events);
        let requests = RequestsService::new(db.clone(), events.clone(), settings.clone(), modes.clone());
        requests.start();
        let live = LiveHub::new(requests.clone());
        live.start(&events);
        let health = Arc::new(HealthStatusService::new(modes.clone()));
        health.set_placement(placement.clone());
        health.set_gate(availability.clone());
        let updates = Arc::new(UpdatesService::new(&config, events.clone()));
        if profile.station {
            updates.set_station(placement.managed_by());
        }
        let personas = Arc::new(PersonaService::new(&config.dialog, db.clone(), settings.clone(), events.clone())?);
        let brain = Arc::new(BrainService::new(
            config.dialog.clone(),
            db.clone(),
            settings.clone(),
            secrets.clone(),
            events.clone(),
        )?);
        // On the station the computer's own model is one of «Мозг»'s providers while the home server is set up.
        if profile.station {
            crate::home_server::keep_brain_provider(&home_server, &brain);
            let (home, models) = (home_server.clone(), brain.clone());
            events.on("home-brain", move |event| {
                if let crate::core::events::GatewayEvent::HomeServerChanged { .. } = &*event {
                    crate::home_server::keep_brain_provider(&home, &models);
                }
            });
        }
        let source: Arc<dyn BrainSource> = brain.clone();
        let dialog = Arc::new(DialogService::new(config.dialog.clone(), personas.clone(), Some(source)));
        // On the station recognition goes the computer's way first, then the cloud's (src/home_server/chain.rs).
        let speech = Arc::new(if profile.station {
            let chain = crate::home_server::chain::station_recognizer(&config.speech.asr, home_server.clone());
            SpeechService::with_recognizer(config.speech.clone(), Some(Arc::new(chain)))
        } else {
            SpeechService::new(config.speech.clone())
        });
        speech.start(&events);
        let mcp = McpService::new(
            db.clone(),
            secrets.clone(),
            events.clone(),
            &config,
            links::connectors::home_assistant_url(&connector_settings),
        );
        // The servers that are on connect at the start (a build outside a runtime, a sync test, connects nothing).
        if tokio::runtime::Handle::try_current().is_ok() {
            mcp.start_all();
        }
        let stations = Arc::new(StationsService::new(db.clone(), events.clone(), config.gateway.station_pairing)?);
        let presence = Arc::new(PresenceService::new(stations.clone(), events.clone()));
        #[cfg(feature = "pc")]
        let station_connect = {
            use crate::stations::connect::station_connect::{ShellConfig, StationConnectService};
            use crate::stations::connect::station_shell::RusshOpener;
            let ssh = ShellConfig {
                port: config.gateway.station_ssh.port,
                user: config.gateway.station_ssh.user.clone(),
                password: config.gateway.station_ssh.password.clone(),
            };
            Arc::new(StationConnectService::new(
                settings.clone(),
                stations.clone(),
                events.clone(),
                Arc::new(RusshOpener),
                ssh,
                config.gateway.uniproxy_path.clone(),
                // The port the station dials: the published one in a container, else the gateway's own.
                config.app.public_port.unwrap_or(config.app.port),
                Default::default(),
            ))
        };
        #[cfg(feature = "pc")]
        let voice_lab = VoiceLabService::new(&config, events.clone());
        // The station's gateway as this computer sees it, and the moves between the two (src/placement).
        #[cfg(feature = "pc")]
        let (station_gateway, moves) = {
            use crate::placement::moves::{MovesDeps, MovesService};
            use crate::stations::connect::station_connect::ShellConfig;
            use crate::stations::connect::station_shell::RusshOpener;
            let station_gateway = crate::placement::watch::StationGateways::new(
                config.clone(),
                db.clone(),
                settings.clone(),
                secrets.clone(),
                events.clone(),
            );
            placement.set_station(station_gateway.clone());
            let moves = MovesService::new(MovesDeps {
                config: config.clone(),
                db: db.clone(),
                settings: settings.clone(),
                secrets: secrets.clone(),
                events: events.clone(),
                access: access.clone(),
                opener: Arc::new(RusshOpener),
                ssh: ShellConfig {
                    port: config.gateway.station_ssh.port,
                    user: config.gateway.station_ssh.user.clone(),
                    password: config.gateway.station_ssh.password.clone(),
                },
                station: station_gateway.clone(),
                stations: stations.clone(),
                restarter: crate::placement::service::restart_soon(),
                timing: Default::default(),
                payload: crate::placement::moves::this_computers_build(),
            });
            (station_gateway, moves)
        };

        // ── Links between the modules (src/links): what each asks of the others, as the Nest modules injected one
        // service into another. ──

        // The panel's live channel (live.module.ts) says in its hello where each station is.
        live.set_presence(Arc::new(links::stations::LivePresence(presence.clone())));

        // The voice, the model shop and the own voices (models.module.ts, voice-lab.module.ts): the shop chooses
        // through the voice, an own voice's route comes from the voice lab, the voice lab asks which voice is
        // chosen. The stored choice speaks from now on, resolved through the shop's catalog in the database; the
        // shop's start resolves it again once it has read the registry. The station's build has no shop: only the
        // stored provider comes back.
        #[cfg(feature = "pc")]
        let models = {
            voice_lab.link_speech(Arc::new(links::voice::LabSpeech(voice.clone())));
            let driven = ModelsLinks {
                voice: Some(Arc::new(links::voice::ShopVoice(voice.clone()))),
                own_voices: Some(Arc::new(links::voice::LabVoices(voice_lab.clone()))),
            };
            let models = Arc::new(ModelsService::new(&config, db.clone(), events.clone(), settings.clone(), driven));
            voice.restore_from_settings(&settings, Some(models.as_ref()));
            models
        };
        #[cfg(not(feature = "pc"))]
        voice.restore_from_settings(&settings, None);

        // Our calls (calls.module.ts): the stations say which address is an approved station's; the codec is
        // speech's; the bot and the bridges are the panel's and follow its changes.
        #[cfg(feature = "pc")]
        let calls = {
            let calls = crate::calls::CallsService::new(
                &config.calls,
                events.clone(),
                stations.clone(),
                Arc::new(crate::calls::SpeechOpus),
                Arc::new(links::calls::CallsSettings(connector_settings.clone())),
            );
            links::calls::follow_settings(&calls, &connector_settings);
            calls
        };

        // The skills (skills.module.ts): the dialog for tales and games, the calls on a PC (none on the station),
        // and the calls' events told to the calls skill.
        let skills = SkillsService::new(SkillsDeps {
            config: config.skills.clone(),
            tts_markup: config.gateway.tts.markup,
            db: db.clone(),
            settings: settings.clone(),
            events: events.clone(),
            dialog: Arc::new(links::dialog::SkillsDialog(dialog.clone())),
            #[cfg(feature = "pc")]
            calls: links::calls::calls_links(&calls),
            #[cfg(not(feature = "pc"))]
            calls: crate::skills::CallsLinks::none(),
            connectors: Arc::new(links::connectors::SkillSettings(connector_settings.clone())),
        });
        #[cfg(feature = "pc")]
        links::calls::report_calls(&calls, &skills);

        // The plugins (src/plugins): their phrases go to the router, their intents to the bridge skill, their media
        // sources join the media skills', their tools the connectors' (below). No code loads until it is called.
        // What the plugins reach through `host.call`: the household's content, tools, the smart home (links/plugins.rs).
        let plugin_services = links::plugins::PluginServices::new(
            &skills,
            Arc::new(links::connectors::SkillSettings(connector_settings.clone())),
        );
        let plugins = {
            // The plugins' free memory, kept by the host's watch and read by the availability registry.
            let gauge = Arc::new(crate::plugins::budget::MemoryGauge::default());
            availability.set_memory(gauge.clone());
            let availability = availability.clone();
            let host = PluginHost::new(PluginsDeps {
                config: config.plugins.clone(),
                db: db.clone(),
                settings: settings.clone(),
                events: events.clone(),
                media: skills.media_sources.clone(),
                placement: Arc::new(move || availability.context().placement),
                speaker: {
                    let skills = Arc::downgrade(&skills);
                    Arc::new(move || skills.upgrade().and_then(|skills| skills.speaker()))
                },
                runtime: Arc::new(crate::plugins::quickjs::QuickJs),
                memory: Arc::new(crate::plugins::budget::MachineMemory),
                gauge,
                bundled: if config.plugins.bundled { crate::plugins::bundled::packages() } else { Vec::new() },
                player: Some(skills.player.clone()),
                watch: Some(skills.player_watch.clone()),
                home: Some(home_server.clone() as Arc<dyn crate::plugins::controller::HomeLink>),
                gateway_url: format!("http://127.0.0.1:{}", config.app.port),
                services: Some(plugin_services.clone() as Arc<dyn crate::plugins::host_api::HostServices>),
            });
            skills.router.add_rules(host.clone());
            let skill = Arc::new(crate::plugins::skill::PluginSkill::new(
                Arc::downgrade(&host),
                Arc::downgrade(&skills.router),
                skills.player.clone(),
            ));
            skills.register(skill.clone());
            (host, skill)
        };
        let (plugins, plugin_skill) = plugins;
        plugin_services.set_plugins(&plugins);

        // What the station says of our player (stage 5а): into the request's journal and to the plugin whose item it
        // is; a stream it keeps silent on goes the gateway's way — through /media/stream, AAC transcoded by ffmpeg on a
        // computer and on the station only by the owner's flag (MEDIA_TRANSCODE_ON_STATION).
        let streams = {
            let ffmpeg = crate::gateway::media::stream::find_ffmpeg();
            let on_station_flag = config.skills.transcode_on_station;
            availability.set_transcoding(ffmpeg.is_some(), on_station_flag);
            let availability = availability.clone();
            crate::gateway::media::stream::StreamRelays::new(
                ffmpeg,
                Arc::new(move || !availability.context().placement.on_station() || on_station_flag),
            )
        };
        skills.player_watch.set_relay(Arc::new(crate::gateway::media::stream::Relay(streams.clone())));
        // The workshop: drafts in DATA_DIR/packages-dev, installed into the packages through the plugin host; by voice
        // too («давай сделаем навык»), through the plugins' skill and a rule of the router.
        let workshop = crate::plugins::workshop::Workshop::new(config.plugins.dev_dir.clone(), plugins.clone());
        // A trial left running before a restart is still taken back on its time.
        workshop.resume_trials();
        let voice_workshop =
            crate::plugins::workshop::voice::VoiceWorkshop::new(workshop.clone(), dialog.clone(), events.clone());
        skills.router.add_rules(voice_workshop.clone());
        plugin_skill.set_voice(voice_workshop);
        // Answers said by the gateway's model in a plugin's words (`model`, right llm.chat): a game's host, a tale.
        {
            let dialog = dialog.clone();
            plugin_skill.set_model(Arc::new(move |key: String, system: String, text: String, cancel, fresh| {
                dialog.roleplay(&key, &system, &text, cancel, fresh)
            }));
        }
        skills.player_watch.set_sink(Arc::new(links::playback::PlaybackLinks {
            requests: requests.clone(),
            plugins: Arc::downgrade(&plugins),
        }));

        // The packages' catalog and installer over the plugin host; the characters over the packages and the persona,
        // their voices over the voice lab and the models (a PC's build).
        let catalog = {
            let availability = availability.clone();
            PackageCatalog::new(CatalogDeps {
                config: config.plugins.clone(),
                host: plugins.clone(),
                db: db.clone(),
                settings: settings.clone(),
                events: events.clone(),
                placement: Arc::new(move || availability.context().placement),
                home: Some(home_server.clone() as Arc<dyn crate::plugins::controller::HomeLink>),
            })
        };
        let characters = CharactersService::new(db.clone(), events.clone(), personas.clone(), plugins.clone());
        #[cfg(feature = "pc")]
        characters.link_voices(crate::characters::voices::CharacterVoices::new(
            voice_lab.clone(),
            models.clone(),
            Arc::new(links::voice::LabVoices(voice_lab.clone())),
        ));

        // The connectors (connectors.module.ts): every built-in in the Node order, the model's over the dialog, our
        // voice's over the voice, the MCP servers last, the plugins as one more. The station link gives the Yandex
        // connector its sessions (`connectors.set_station_sessions`); the station's conversations take the tools
        // the connectors give.
        let connectors = Arc::new(ConnectorsService::with_links(
            &config,
            connector_settings.clone(),
            ConnectorLinks {
                modes: modes.clone(),
                dialog: Arc::new(links::connectors::DialogConnector(dialog.clone())),
                voice: Arc::new(links::connectors::VoiceConnector(voice.clone())),
            },
        ));
        connectors.set_mcp(Arc::new(links::connectors::McpConnectors(mcp.clone())));
        if let Err(error) =
            connectors.register(Arc::new(crate::plugins::connector::PluginsConnector::new(Arc::downgrade(&plugins))))
        {
            tracing::error!("{error}");
        }

        // The station link (session-registry.ts): the station's sessions, answered with the speech, the dialog, the
        // skills, the voice and the MCP servers, or relayed to Yandex, as the mode says. The offline watch counts
        // them.
        // The emergency words' way back (src/gateway/uniproxy/rescue.rs): the station's own gateway changes its own
        // files; a computer's goes over SSH, as «Вернуть к Яндексу» does.
        #[cfg(feature = "pc")]
        let over_ssh: Option<Arc<dyn crate::gateway::uniproxy::rescue::StationRescue>> =
            Some(Arc::new(crate::gateway::uniproxy::rescue::OverSsh(station_connect.clone())));
        #[cfg(not(feature = "pc"))]
        let over_ssh: Option<Arc<dyn crate::gateway::uniproxy::rescue::StationRescue>> = None;
        let rescue = if placement.role() == crate::placement::service::Role::Station {
            Some(Arc::new(crate::gateway::uniproxy::rescue::OwnStation)
                as Arc<dyn crate::gateway::uniproxy::rescue::StationRescue>)
        } else {
            over_ssh
        };
        let station_sessions = SessionRegistry::new(SessionDeps {
            codec: Arc::new(AapiCodec::new()),
            config: Arc::new(config.gateway.clone()),
            speech: speech.clone(),
            dialog: dialog.clone(),
            skills: skills.clone(),
            voice: voice.clone(),
            modes: mode_service.clone(),
            household: access.household.clone(),
            events: events.clone(),
            requests: Some(requests.clone()),
            mcp: Some(mcp.clone()),
            tools: Some(connectors.clone()),
            rescue,
        });
        // The station watch alerts through the calls' Telegram bot on a PC; the station's build has no bot.
        let sessions: Arc<dyn ActiveSessions> = station_sessions.clone();
        #[cfg(feature = "pc")]
        let telegram: Option<Arc<dyn TelegramSender>> =
            Some(Arc::new(links::calls::TelegramAlerts(calls.telegram.clone())));
        #[cfg(not(feature = "pc"))]
        let telegram: Option<Arc<dyn TelegramSender>> = None;
        let station_watch = Arc::new(StationWatch::new(sessions, telegram, config.operations.clone(), events.clone()));
        let retention = Arc::new(RetentionService::new(
            config.gateway.recorder.dir.clone(),
            config.operations.retention_days,
            config.operations.journal_cap,
        ));

        // This computer's own addresses, for the station's home server: told to it when they change.
        #[cfg(feature = "pc")]
        {
            let connect = station_connect.clone();
            station_gateway.set_home_addresses(Arc::new(move || {
                connect.home_addresses(
                    &crate::stations::connect::station_connect::host_interfaces(),
                    crate::stations::connect::station_connect::in_container(),
                )
            }));
        }
        // The station's gateway and the safety net tell the household the way the station watch does.
        #[cfg(feature = "pc")]
        {
            let watch = station_watch.clone();
            station_gateway.set_teller(Arc::new(move |text: String| {
                let watch = watch.clone();
                tokio::spawn(async move { watch.tell(&text).await });
            }));
        }
        {
            let watch = station_watch.clone();
            events.on("safety-alert", move |event| {
                if let crate::core::events::GatewayEvent::SafetyFired { what, .. } = &*event {
                    let text = match what.as_str() {
                        "previous-version" => {
                            "Страховка: новая версия шлюза на колонке не поднялась — вернулась прежняя."
                        }
                        _ => "Страховка: шлюз на колонке не поднимался — колонка вернулась к прежнему адресу.",
                    };
                    let watch = watch.clone();
                    tokio::spawn(async move { watch.tell(text).await });
                }
            });
        }

        // The check chat (ask.controller.ts) asks the dialog.
        mcp.attach_dialog(Arc::new(links::dialog::AskBrain(dialog.clone())));

        // /health reads the speech, the dialog, the voice and the skills (health-status.service.ts); the station
        // link adds its own source for the sessions and the station.
        health.add_sources(Arc::new(links::health::ModuleHealth {
            speech: speech.clone(),
            dialog: dialog.clone(),
            voice: voice.clone(),
            skills: skills.clone(),
        }));
        // The station link's part of /health: the sessions open now and the station's presence.
        health.add_sources(Arc::new(StationLinkHealth {
            sessions: station_sessions.clone(),
            watch: station_watch.clone(),
        }));

        // The Yandex connector reads the station link's sessions.
        connectors.set_station_sessions(station_sessions.clone());

        // ── The links end here. ──

        Ok(Arc::new(App {
            config,
            db,
            events,
            secrets,
            settings,
            access,
            requests,
            live,
            health,
            connectors,
            connector_settings,
            hosts,
            updates,
            personas,
            brain,
            dialog,
            #[cfg(feature = "pc")]
            models,
            speech,
            voice,
            mcp,
            skills,
            workshop,
            plugins,
            catalog,
            streams,
            characters,
            stations,
            presence,
            station_watch,
            retention,
            #[cfg(feature = "pc")]
            station_connect,
            #[cfg(feature = "pc")]
            voice_lab,
            #[cfg(feature = "pc")]
            calls,
            placement,
            availability,
            home_server,
            #[cfg(feature = "pc")]
            station_gateway,
            #[cfg(feature = "pc")]
            moves,
            modes: mode_service,
            sessions: station_sessions,
            started: Instant::now(),
            started_at: now_iso(),
        }))
    }

    /// Stops what runs beside the HTTP server: MCP connections and their programs, the skills' tasks.
    pub async fn shutdown(&self) {
        self.mcp.shutdown().await;
        self.skills.shutdown();
        self.plugins.shutdown();
        self.characters.shutdown();
        // On the way down keep every reachable station's «last seen», so a restart shows it reconnecting.
        self.presence.stop();
        self.station_watch.stop();
        self.retention.stop();
    }

    /// The gateway stops: what would hold the server's graceful close ends now (the station clients' polls and
    /// streams, the calls' sockets) and the bridges leave.
    pub fn before_shutdown(&self) {
        #[cfg(feature = "pc")]
        self.calls.stop();
        // The stations' sockets are cut: they reconnect to the next gateway.
        self.sessions.stop();
    }
}

/// Every route of the gateway, behind the Host check and the error envelope; the station's voice socket ahead of them.
pub fn router(app: Arc<App>) -> Router {
    let names = app.hosts.clone();
    let routes = Router::new()
        .merge(crate::access::routes())
        .merge(crate::requests::routes())
        .merge(crate::live::routes())
        .merge(crate::health_status::routes())
        .merge(crate::panel::routes())
        .merge(crate::connectors::routes())
        .merge(crate::updates::router())
        .merge(crate::gateway::media::router())
        .merge(crate::gateway::station_http::router())
        // The station link: the station's API (/api/v1/station, /debug). Its voice socket comes before the routes.
        .merge(crate::gateway::debug::router())
        .merge(crate::dialog::routes())
        .merge(crate::models::router())
        .merge(crate::mcp::routes())
        .merge(crate::plugins::routes())
        .merge(crate::plugins::workshop::routes::routes())
        .merge(crate::apps::host_http::routes())
        .merge(crate::characters::routes())
        .merge(crate::stations::router())
        .merge(crate::placement::router())
        .merge(crate::home_server::router())
        .merge(crate::voice_lab::router());
    // Our calls are a PC's part: the station's build has no /calls.
    #[cfg(feature = "pc")]
    let routes = routes.merge(crate::calls::router());
    routes
        .fallback(not_found)
        // After every route: a path that exists with another method is Express's 404 too, not 405.
        .method_not_allowed_fallback(not_found)
        .with_state(app.clone())
        // On the station, while its settings live on the computer, they are read-only here (src/placement).
        .layer(axum::middleware::from_fn_with_state(app.clone(), crate::placement::routes::guard))
        .layer(axum::middleware::from_fn(error_bodies))
        .layer(axum::middleware::from_fn_with_state(names, security))
        // The station's voice socket (an upgrade to UNIPROXY_PATH), before the Host check: Node's 'upgrade' listener
        // never went through Express's middleware. The upgrade has its own checks.
        .layer(axum::middleware::from_fn_with_state(app, crate::gateway::uniproxy::uniproxy_server::station_socket))
}

/// Serves the router on a listener until `shutdown` resolves, then stops the services that run beside it.
pub async fn serve(
    app: Arc<App>,
    listener: TcpListener,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let stopping = app.clone();
    // Apps reach the Host API at the address the server really listens on.
    if let Ok(address) = listener.local_addr() {
        app.plugins.set_gateway_url(format!("http://127.0.0.1:{}", address.port()));
    }
    let served = axum::serve(listener, router(app.clone()).into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async move {
            shutdown.await;
            // What would hold the graceful close ends now: the panel's live streams, the station clients' polls and
            // streams, the calls' sockets.
            stopping.live.shutdown();
            stopping.before_shutdown();
        })
        .await;
    app.shutdown().await;
    served
}

/// The gateway as the program runs it: build, listen on PORT, serve until Ctrl+C or SIGTERM.
pub async fn run(config: Config) -> anyhow::Result<()> {
    let port = config.app.port;
    let app = App::build(config)?;
    app.start().await?;
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("HTTP server running on port {port}");
    serve(app.clone(), listener, shutdown_signal()).await?;
    // What is still going is journaled as cancelled before the database closes.
    app.requests.shutdown();
    Ok(())
}

/// Ctrl+C, or SIGTERM from a service manager.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutting down");
}
