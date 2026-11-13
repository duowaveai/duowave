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
use crate::oauth::OAuthService;
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
use crate::storage::{Db, StateStore};
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
    /// Sign-ins to outside services instead of their keys (src/oauth): the ones begun and their returns.
    pub oauth: Arc<OAuthService>,
    /// Answers with the model: phrases for speech, the history per device.
    pub dialog: Arc<DialogService>,
    /// The characters' memory (src/memory): the household graph, the wake-up in the prompt, the «Память» window.
    pub memory: Arc<crate::memory::MemoryService>,
    /// The memory's reflection, the sleep's first part: it thinks the day over at night (src/memory/reflect.rs).
    pub reflection: Arc<crate::memory::reflect::Reflection>,
    /// The night's work in parts (src/sleep): its hour, «Запустить сон сейчас», «За ночь», «Откатить ночь».
    pub sleep: Arc<crate::sleep::SleepService>,
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
    /// The plugins' schedules (plugins/schedule.rs): rhythms of their manifests and tasks of their code.
    pub schedules: Arc<crate::plugins::schedule::Scheduler>,
    /// The plugins' pictures (plugins/files.rs).
    pub files: Arc<crate::plugins::files::PluginFiles>,
    /// A MIDI keyboard for apps (src/midi): the keyboards plugged into this computer, heard while an app listens.
    pub midi: Arc<crate::midi::MidiService>,
    /// «Связи» (src/bus, docs/plans/bus.md): the bus between everything with an address — apps, the gateway's
    /// services, folders, devices — with its one rule, its questions «да» and its journal.
    pub bus: Arc<crate::bus::Bus>,
    /// The packages' catalog and the installer (src/plugins/catalog.rs): one feed for every kind.
    pub catalog: Arc<PackageCatalog>,
    /// Streams the gateway relays for the station: the fallback for one it keeps silent on (gateway/media/stream.rs).
    pub streams: Arc<crate::gateway::media::stream::StreamRelays>,
    /// The workshop (src/plugins/workshop): drafts, the sandbox, the agent, the install.
    pub workshop: Arc<crate::plugins::workshop::Workshop>,
    /// The apps' history (src/plugins/history): every step and version kept by hash; None when its folder cannot be
    /// made — the gateway goes on without it.
    pub history: Option<Arc<crate::plugins::history::History>>,
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
    /// Recognition on this computer itself (src/speech/asr/local), when it hears by itself (ASR_PROVIDER=local).
    #[cfg(feature = "pc")]
    pub local_asr: Option<Arc<crate::speech::asr::local::supervisor::LocalAsr>>,
    /// The own voice's server the desktop app sets up and keeps (src/speech/tts/managed.rs).
    #[cfg(feature = "pc")]
    pub managed_tts: Option<Arc<crate::speech::tts::managed::ManagedTts>>,
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
        // The live channel of each station whose gateway runs on it, relayed to this gateway's panels.
        #[cfg(feature = "pc")]
        crate::placement::relay::StationRelay::start(self.station_gateway.clone(), &self.live, &self.events);
        self.station_watch.start();
        self.retention.start();
        self.updates.spawn_checks();
        // The reminders' scheduler and the routes' listener.
        self.skills.start();
        // The plugins' idle unloading and the events they follow; their schedules' tick.
        self.plugins.start();
        self.schedules.start();
        // The keyboards: the system's news of them first (a Mac), then the list and the keeper of idle ports.
        self.midi.start();
        // The bus follows the settings and the apps removed; the folders' `tmp/` and bin are cleaned every hour.
        self.bus.start();
        if self.config.data_dir.is_some() {
            if let Some(files) = self.bus.files() {
                files.places().prepare();
                let places = files.places().clone();
                tokio::spawn(async move {
                    let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
                    loop {
                        tick.tick().await;
                        let places = places.clone();
                        let removed = tokio::task::spawn_blocking(move || places.sweep()).await.unwrap_or(0);
                        if removed > 0 {
                            tracing::info!(target: "bus", "files: {removed} old file(s) cleaned from tmp/ and the bin");
                        }
                    }
                });
            }
        }
        // The apps' history: what is on disk and not in it yet, recorded; its cleaning once a day and the station's
        // sending to the computer.
        if let Some(history) = &self.history {
            history.start();
        }
        // The night's work: at its hour, when a part that is on has work.
        self.sleep.start();
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
        // Recognition on this computer: the model the shop chose is fetched and loaded in the background.
        #[cfg(feature = "pc")]
        if let Some(local) = &self.local_asr {
            local.start();
        }
        // The app's own voice server: kept running while a Silero or Piper voice speaks (the shop said which above).
        #[cfg(feature = "pc")]
        if let Some(tts) = &self.managed_tts {
            tts.start();
        }
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
            let pictures = crate::plugins::files::root_of(&config.data_dir);
            crate::placement::transfer::apply_incoming(dir, &db, &secrets, station, Some(&pictures));
        }
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        // «Связи» (src/bus): made before what registers on it; each kind of participant joins as it stands.
        let bus = crate::bus::Bus::new(crate::bus::BusDeps {
            db: db.clone(),
            settings: settings.clone(),
            events: events.clone(),
            keep_lines: if cfg!(feature = "station") {
                crate::bus::journal::STATION_LINES
            } else {
                crate::bus::journal::COMPUTER_LINES
            },
        });
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
        let memory =
            Arc::new(crate::memory::MemoryService::new(db.clone(), settings.clone(), events.clone(), personas.clone()));
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
        // A sign-in's return comes to the address a browser on this computer reaches the gateway at.
        let oauth = Arc::new(OAuthService::new(config.app.public_port.unwrap_or(config.app.port), secrets.clone()));
        let source: Arc<dyn BrainSource> = brain.clone();
        let dialog = Arc::new(DialogService::new(config.dialog.clone(), personas.clone(), Some(source)));
        dialog.attach_memory(memory.clone());
        // The sleep and its first part, the reflection: off until the owner turns it on.
        let reflection =
            Arc::new(crate::memory::reflect::Reflection::new(memory.clone(), settings.clone(), brain.clone()));
        let sleep = Arc::new(crate::sleep::SleepService::new(db.clone(), settings.clone(), events.clone()));
        sleep.register(reflection.clone());
        // Recognition on this computer itself: its engine in a worker of this program, the model fetched once
        // (src/speech/asr/local). Started with the gateway's start; the models' shop says which model.
        #[cfg(feature = "pc")]
        let local_asr = match &config.speech.asr.local {
            Some(local) if config.speech.asr.enabled && !profile.station => {
                use crate::speech::asr::local::supervisor::{LocalAsr, LocalAsrOptions};
                let options = LocalAsrOptions::new(
                    local.dir.clone(),
                    local.mirror.clone(),
                    config.speech.asr.language.clone(),
                    local.gpu,
                );
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
        #[cfg(feature = "pc")]
        let managed_tts = match &config.models.tts_managed {
            Some(managed) if !profile.station => {
                use crate::speech::tts::managed::{ManagedTts, ManagedTtsOptions};
                let options = ManagedTtsOptions {
                    dir: managed.dir.clone(),
                    // Beside the program: the app's resources (its working folder), the repository's folder elsewhere.
                    server_dir: std::path::absolute("tts-server").unwrap_or_else(|_| "tts-server".into()),
                    models_dir: config.models.tts_dir.clone(),
                    piper_root: managed.piper_root.clone(),
                    port: managed.port,
                };
                Some(ManagedTts::new(options, events.clone()))
            }
            _ => None,
        };
        // On the station recognition goes the computer's way first, then the cloud's (src/home_server/chain.rs).
        let speech = if profile.station {
            let chain = crate::home_server::chain::station_recognizer(&config.speech.asr, home_server.clone());
            SpeechService::with_recognizer(config.speech.clone(), Some(Arc::new(chain)))
        } else {
            #[cfg(feature = "pc")]
            let speech = match &local_asr {
                Some(asr) => {
                    let local = crate::speech::asr::local::recognizer::LocalRecognizer::new(asr.clone());
                    SpeechService::with_recognizer(config.speech.clone(), Some(Arc::new(local)))
                }
                None => SpeechService::new(config.speech.clone()),
            };
            #[cfg(not(feature = "pc"))]
            let speech = SpeechService::new(config.speech.clone());
            speech
        };
        // The gains the stations' wake words picked outlive a restart: a request without one starts from them.
        let speech = Arc::new(speech.keeping_wake_word_gains(StateStore::open(db.clone(), "wake_word_gain")));
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
                local_asr: local_asr.clone(),
                managed_tts: managed_tts.clone(),
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
        // and the calls' events told to the calls skill. Without a bot here, «передай …» goes through the computer's
        // while it helps the station (src/home_server/messenger.rs).
        #[cfg(feature = "pc")]
        let calls_links = links::calls::calls_links(&calls);
        #[cfg(not(feature = "pc"))]
        let calls_links = crate::skills::CallsLinks::none();
        let calls_links = crate::skills::CallsLinks {
            messenger: crate::home_server::messenger::HomeMessenger::new(
                calls_links.messenger.clone(),
                home_server.clone(),
            ),
            ..calls_links
        };
        let skills = SkillsService::new(SkillsDeps {
            config: config.skills.clone(),
            tts_markup: config.gateway.tts.markup,
            db: db.clone(),
            settings: settings.clone(),
            events: events.clone(),
            dialog: Arc::new(links::dialog::SkillsDialog(dialog.clone())),
            calls: calls_links,
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
                runtime: Arc::new(crate::plugins::quickjs::QuickJs),
                memory: Arc::new(crate::plugins::budget::MachineMemory),
                gauge,
                bundled: if config.plugins.bundled { crate::plugins::bundled::packages() } else { Vec::new() },
                player: Some(skills.player.clone()),
                watch: Some(skills.player_watch.clone()),
                home: Some(home_server.clone() as Arc<dyn crate::plugins::controller::HomeLink>),
                gateway_url: format!("http://127.0.0.1:{}", config.app.port),
                services: Some(plugin_services.clone() as Arc<dyn crate::plugins::host_api::HostServices>),
                secrets: secrets.clone(),
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
        // Apps and the gateway's services on the bus; the services reach the bus for the plugins' `host.bus`.
        {
            let apps = Arc::new(crate::bus::providers::apps::Apps::new());
            apps.set_host(Arc::downgrade(&plugins));
            // An app without its own `ask` answers in words through the household's model with its tools.
            apps.set_words(Arc::new(links::bus_words::BrainWords::new(&dialog, &plugins, &bus)));
            bus.register(apps);
            // What an app reaches outside a letter (its speech by schedule, the model's answer in its words, its page
            // hearing the keyboard) is the bus's word too: in the trail it runs in, else its own.
            let gate_bus = Arc::downgrade(&bus);
            plugins.set_gate(Arc::new(move |app: &str, to: &str, handle: &str| {
                let bus = gate_bus.upgrade().ok_or_else(|| "шина остановлена".to_string())?;
                let trail = crate::bus::trail::current().unwrap_or_else(|| crate::bus::Trail::app(app));
                bus.may(&trail, &crate::bus::Address::app(app), to, handle)
            }));
            let system = Arc::new(crate::bus::providers::system::System::new());
            let services: Arc<dyn crate::plugins::host_api::HostServices> = plugin_services.clone();
            system.set_services(Arc::downgrade(&services));
            bus.register(system);
            plugin_services.set_bus(&bus);
            // Folders: DATA_DIR/files and the household's music and tales; an app's own folder within its budget.
            let places = crate::bus::providers::files::Places::new(
                &config.data_dir.clone().unwrap_or_else(|| std::path::PathBuf::from("data")),
                &config.skills.content_dir,
                placement.role() == crate::placement::service::Role::Station,
            );
            let host = Arc::downgrade(&plugins);
            let files = Arc::new(crate::bus::providers::files::Files::new(
                places,
                Arc::new(move |id: &str| {
                    host.upgrade()
                        .and_then(|host| host.package(id))
                        .map(|package| package.manifest.budget.files_mb)
                        .unwrap_or(crate::bus::providers::files::APP_FOLDER_MB)
                }),
            ));
            files.set_bus(Arc::downgrade(&bus));
            bus.set_files(files.clone());
            bus.register(files);
        }
        // The plugins' schedules are kept by the gateway that keeps the station's session: the station's own, or the
        // computer the station talks to — not a computer that only helps a station (they would run twice).
        let schedules = {
            let station_build = placement.role() == crate::placement::service::Role::Station;
            let availability = availability.clone();
            let leads =
                move || station_build || availability.context().placement == crate::core::availability::Placement::Pc;
            Arc::new(crate::plugins::schedule::Scheduler::new(
                db.clone(),
                Arc::downgrade(&plugins),
                Arc::new(leads),
                config.plugins.schedules,
            ))
        };
        plugin_services.set_schedules(&schedules);
        // The plugins' pictures, in DATA_DIR/plugin-files; the station's limits where the gateway runs there.
        let files = {
            let root = crate::plugins::files::root_of(&config.data_dir);
            let plugins = Arc::downgrade(&plugins);
            Arc::new(crate::plugins::files::PluginFiles::new(
                root,
                Arc::new(move || plugins.upgrade().is_some_and(|plugins| plugins.on_station())),
            ))
        };
        plugin_services.set_files(&files);
        // A MIDI keyboard for apps (docs/plans/midi.md): the computer's build with `midi`; an app's page hears it while
        // the app is on here with the right `midi`, and its journal says when the keyboard opened and closed.
        let midi = {
            let off = (!config.midi.enabled).then(|| "MIDI выключен (MIDI=0).".to_string());
            let midi = crate::midi::MidiService::new(crate::midi::ports::system(), off, config.midi.remote);
            midi.set_events(events.clone());
            availability.set_midi(midi.available());
            // Which app's page may hear the keyboard is the bus's to say (`listen:device/midi`, the owner's «да»).
            let host = Arc::downgrade(&plugins);
            midi.set_gate(Arc::new(move |package: &str| {
                let host = host.upgrade().ok_or_else(|| "приложения остановлены".to_string())?;
                host.may_hear_midi(package)
            }));
            let host = Arc::downgrade(&plugins);
            midi.set_journal(Arc::new(move |package: &str, line: &str| {
                if let Some(host) = host.upgrade() {
                    host.note_line(package, "info", line);
                }
            }));
            midi
        };
        // Devices on the bus: the MIDI keyboard first; a new device is one more `add`.
        {
            let devices = Arc::new(crate::bus::providers::devices::Devices::new());
            devices.add(Arc::new(crate::bus::providers::devices::MidiKeyboard::new(midi.clone())));
            bus.register(devices);
        }

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
        // A teacher for a MIDI keyboard is made only where a keyboard is heard (docs/plans/midi.md).
        workshop.set_midi(midi.available());
        // The agent's steps reach the panel as `workshop.changed` (the plaque «Делаю виджет», the workshop's log).
        workshop.attach_events(events.clone());
        // A trial left running before a restart is still taken back on its time.
        workshop.resume_trials();
        // The apps' history (src/plugins/history), in DATA_DIR/package-history: the workshop's steps, the versions
        // the workshop, the catalog and copies put in, the plugins' data before each change.
        let history = {
            let host = Arc::downgrade(&plugins);
            crate::plugins::history::History::open(
                config.plugins.history_dir(),
                config.plugins.dev_dir.clone(),
                Some(settings.clone()),
                Arc::new(move || host.upgrade().is_some_and(|host| host.on_station())),
            )
        };
        if let Some(history) = &history {
            history.set_host(&plugins);
            history.set_events(events.clone());
            history.set_data(crate::plugins::history::data::DataParts {
                db: db.clone(),
                schedules: Some(schedules.clone()),
                files: Some(files.clone()),
            });
            history.set_home(home_server.clone() as Arc<dyn crate::plugins::controller::HomeLink>);
            plugins.set_history(history.clone());
            workshop.set_history(history.clone());
        }
        // One road for «сделай…» (docs/plans/voice-creation.md): the station's request goes the panel's way, the
        // agent works in the background, the station that asked says «Готово» and listens for the answer.
        let voice_workshop = crate::plugins::workshop::voice::VoiceWorkshop::new(workshop.clone(), dialog.clone());
        skills.router.add_rules(voice_workshop.clone());
        // The memory's voice commands («запомни», «забудь», «что ты помнишь»): ours, at once, without the model.
        skills.register(Arc::new(crate::skills::own::memory_skill::MemorySkill::new(memory.clone())));
        skills.router.add_rules(Arc::new(crate::skills::own::memory_skill::MemoryRules));
        plugin_skill.set_voice(voice_workshop.clone());
        {
            let live = live.clone();
            voice_workshop.set_panels(Arc::new(move || live.size() > 0));
        }
        // «Верни погоду как было вчера»: the history by voice, beside the plugins' phrases.
        if let Some(history) = &history {
            let voice_history = crate::plugins::history::voice::VoiceHistory::new(history.clone());
            skills.router.add_rules(voice_history.clone());
            plugin_skill.set_history_voice(voice_history);
        }
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
            events: events.clone(),
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
        memory.attach_characters(characters.clone());
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
        // The memory's tools for the model: `remember`, `forget`, `recall` (src/memory/tools.rs).
        if let Err(error) = connectors.register(Arc::new(crate::memory::tools::MemoryConnector::new(&memory))) {
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
        // The plugins' turn timer says «время вышло» on the device's live station; their schedules speak there too.
        plugin_skill.set_turns(station_sessions.turns());
        voice_workshop.set_turns(station_sessions.turns());
        schedules.set_station(station_sessions.schedule_station());
        // The nights keep to the station's clock.
        let clock = station_sessions.schedule_station();
        sleep.set_zone(Arc::new(move || crate::plugins::schedule::ScheduleStation::timezone(&*clock)));
        // The station watch alerts through the calls' Telegram bot on a PC; the station's build has no bot.
        let sessions: Arc<dyn ActiveSessions> = station_sessions.clone();
        #[cfg(feature = "pc")]
        let telegram: Option<Arc<dyn TelegramSender>> =
            Some(Arc::new(links::calls::TelegramAlerts(calls.telegram.clone())));
        #[cfg(not(feature = "pc"))]
        let telegram: Option<Arc<dyn TelegramSender>> = None;
        let station_watch = Arc::new(StationWatch::new(sessions, telegram, config.operations.clone(), events.clone()));
        // The plugins' messages to the owner go the station watch's ways: the chat «Оповещения», the webhook, the Mac.
        schedules.set_notice(Arc::new(links::plugins::OwnerAlerts(station_watch.clone())));
        // So does a delayed command that came too late to run.
        {
            let watch = station_watch.clone();
            skills.later.set_notice(Arc::new(move |text: String| {
                let watch = watch.clone();
                tokio::spawn(async move { watch.tell(&text).await });
            }));
        }
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
            // While every station's gateway runs on it, none is missing at this computer's /uni.ws. Weak: the watches
            // hold the station watch already (its teller), and an engine started again leaves neither behind.
            let gateways = Arc::downgrade(&station_gateway);
            station_watch
                .set_elsewhere(Arc::new(move || gateways.upgrade().is_some_and(|gateways| gateways.none_here())));
            // The bot's messages reach the stations this computer helps: they are not at its /uni.ws (live 11.11).
            skills.calls.attach_relay(crate::placement::messages::StationMessages::new(&station_gateway));
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
            oauth,
            dialog,
            memory,
            reflection,
            sleep,
            #[cfg(feature = "pc")]
            models,
            speech,
            voice,
            mcp,
            skills,
            workshop,
            history,
            plugins,
            schedules,
            files,
            midi,
            bus,
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
            local_asr,
            #[cfg(feature = "pc")]
            managed_tts,
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
        self.schedules.stop();
        self.sleep.stop();
        // Every page's notes let go: no note sounds on after the gateway.
        self.midi.stop();
        self.characters.shutdown();
        // On the way down keep every reachable station's «last seen», so a restart shows it reconnecting.
        self.presence.stop();
        self.station_watch.stop();
        self.retention.stop();
        // Recognition's worker and the own voice's server end with the gateway: the desktop app restarts the gateway
        // inside its process, and an old gateway's processes must not stay behind with their models.
        #[cfg(feature = "pc")]
        if let Some(local) = &self.local_asr {
            local.shutdown().await;
        }
        #[cfg(feature = "pc")]
        if let Some(tts) = &self.managed_tts {
            tts.shutdown().await;
        }
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
        .merge(crate::memory::routes())
        .merge(crate::sleep::routes())
        .merge(crate::models::router())
        .merge(crate::mcp::routes())
        .merge(crate::oauth::routes())
        .merge(crate::plugins::routes())
        .merge(crate::bus::routes::routes())
        .merge(crate::midi::routes::routes())
        .merge(crate::skills::own::later_skill::routes())
        .merge(crate::plugins::workshop::routes::routes())
        .merge(crate::plugins::history::routes::routes())
        .merge(crate::speech::listen::routes())
        .merge(crate::apps::host_http::routes())
        .merge(crate::characters::routes())
        .merge(crate::stations::router())
        .merge(crate::placement::router())
        .merge(crate::home_server::router())
        .merge(crate::voice_lab::router());
    // Our calls are a PC's part: the station's build has no /calls.
    #[cfg(feature = "pc")]
    let routes = routes.merge(crate::calls::router());
    // The helper's pass to the station's gateway (src/placement/proxy.rs): a PC's part too.
    #[cfg(feature = "pc")]
    let routes = routes.merge(crate::placement::proxy::routes());
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
