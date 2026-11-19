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
use crate::core::identity::home::{DeviceKind, Identity};
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
    /// Who this gateway is (src/core/identity/home.rs): the home it is in, its own id and name.
    pub identity: Arc<Identity>,
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
    /// The router (src/router): the journal of deals — every answer of a model with its seat, tokens and dollars —
    /// and the models' prices.
    pub router: Arc<crate::router::Router>,
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
    /// «Выход в интернет» (src/egress): who goes to the internet directly and who through the person's own server.
    pub egress: Arc<crate::egress::Egress>,
    /// The apps' long tasks (src/tasks): what an app started and reports on, for the panel to follow and cancel.
    pub tasks: Arc<crate::tasks::Tasks>,
    /// The plugins' schedules (plugins/schedule.rs): rhythms of their manifests and tasks of their code.
    pub schedules: Arc<crate::plugins::schedule::Scheduler>,
    /// The plugins' pictures (plugins/files.rs).
    pub files: Arc<crate::plugins::files::PluginFiles>,
    /// «Смотри и выбирай»: an app's video with its fork on a page of this gateway (plugins/watch.rs).
    pub watch: Arc<crate::plugins::watch::WatchPages>,
    /// Publishers (capabilities/publish.rs): where a file can go — the gateway's own and the platforms' upload pages.
    pub publishers: Arc<crate::capabilities::publish::Publishers>,
    /// Voices' providers (capabilities/voices.rs): the computer's and the cloud's behind `system/voice`.
    pub voices: Arc<crate::capabilities::voices::Voices>,
    /// A MIDI keyboard for apps (src/midi): the keyboards plugged into this computer, heard while an app listens.
    pub midi: Arc<crate::midi::MidiService>,
    /// «Связи» (src/bus, docs/plans/bus.md): the bus between everything with an address — apps, the gateway's
    /// services, folders, devices — with its one rule, its questions «да» and its journal.
    pub bus: Arc<crate::bus::Bus>,
    /// The computer's screen (src/screen): windows of temporary content — a photo, a video, a sound — the DuoWave app
    /// opens over all the others; `device/screen` on the bus, the brain's `screen_*` tools.
    pub screen: Arc<crate::screen::Screen>,
    /// The packages' catalog and the installer (src/plugins/catalog.rs): one feed for every kind.
    pub catalog: Arc<PackageCatalog>,
    /// Streams the gateway relays for the station: the fallback for one it keeps silent on (gateway/media/stream.rs).
    pub streams: Arc<crate::gateway::media::stream::StreamRelays>,
    /// The workshop (src/plugins/workshop): drafts, the sandbox, the agent, the install.
    pub workshop: Arc<crate::plugins::workshop::Workshop>,
    /// The talk layer (src/talk): the records of works, what the brain knows now.
    pub talk: Arc<crate::talk::TalkLayer>,
    /// The works in folders, the helper's heavy part (src/works): work folders, agents, work copies, journals of runs.
    /// A computer's part; the talk layer reaches it through its port (talk/folder.rs).
    #[cfg(feature = "pc")]
    pub works: Option<Arc<crate::works::FolderWorks>>,
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
    /// The built-in helper (src/helpers/builtin.rs): this computer's heavy part behind the helpers' contract —
    /// `/home/v1` answers through it, and a computer that is not in the station role takes its own parts through it.
    #[cfg(feature = "pc")]
    pub builtin: Arc<crate::helpers::builtin::BuiltinHelper>,
    /// Our own calls (src/calls): the hub, the station clients, the bridges, the Telegram bot.
    #[cfg(feature = "pc")]
    pub calls: Arc<crate::calls::CallsService>,
    /// Where this gateway runs, and its state record (src/placement).
    pub placement: Arc<PlacementService>,
    /// What works where, now (src/placement/availability.rs): the panels and the mode switch ask it.
    pub availability: Arc<AvailabilityService>,
    /// The helpers (src/helpers): the list, the choice of road, the regulator.
    pub helpers: Arc<crate::helpers::Helpers>,
    /// The station's cloud hearing and voice as the panel sets them (src/speech/cloud_speech.rs).
    pub cloud_speech: Arc<crate::speech::cloud_speech::CloudSpeech>,
    /// The station's home server (src/home_server): the helpers' entry `home-computer`, in the old shape.
    pub home_server: Arc<crate::home_server::HomeServerService>,
    /// Who this computer helps over the link («Помогаю», src/helpers/clients.rs).
    #[cfg(feature = "pc")]
    pub clients: Arc<crate::helpers::clients::Clients>,
    /// The helper's side of the link (src/helpers/link_serve.rs): its TLS port, the pairings, the homes' links.
    #[cfg(feature = "pc")]
    pub link: Arc<crate::helpers::link_serve::LinkServer>,
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
        // «Выход в интернет» first: its routes stand and its entry for the apps' programs runs before any app starts.
        self.egress.start().await;
        // Background work the stations need: the records' sync listener, the presence tracker, the offline watch,
        // the journal retention.
        self.stations.start();
        self.presence.start();
        // The devices of the home: the panel's top line hears when they move.
        crate::home::watch(&self.events);
        // The home's character and its voice: the Турбо that speaks hears it (stage 2).
        crate::home::character::watch(self);
        // The character's voice up on the Турбо: the station switches to the computer's voice by itself (item 70).
        crate::models::home_voice::watch_follow(self);
        crate::home::telegram::watch(self);
        crate::home::copy::watch(self);
        // The apps' faces here, their code on the Турбо (stage 5б).
        crate::home::apps::watch(self);
        // A second station as the centre's ear: its copy of the home (stage 4).
        crate::home::ear::start(self);
        // Where the gateway runs: the safety net's news on the station, the watch over the station's gateway here.
        self.placement.start();
        // The settings revision counted anew after a change: the home's copy follows within seconds.
        self.placement.watch_revision();
        // The built-in helper follows what it declares; the helpers' tick looks at its model servers.
        #[cfg(feature = "pc")]
        self.builtin.start();
        self.helpers.start();
        #[cfg(feature = "pc")]
        self.station_gateway.start();
        // The live channel of each station whose gateway runs on it, relayed to this gateway's panels.
        #[cfg(feature = "pc")]
        crate::placement::relay::StationRelay::start(self.station_gateway.clone(), &self.live, &self.events);
        self.station_watch.start();
        self.retention.start();
        // The old deals and their days cleaned: now and daily.
        self.router.start();
        // The works in folders: old works, journals and copies cleaned.
        #[cfg(feature = "pc")]
        if let Some(works) = &self.works {
            works.start_cleaning();
        }
        self.updates.spawn_checks();
        // On the station: what an update left, and a trial of a new version from memory (src/updates/trial.rs).
        self.updates.start_on_station();
        // The reminders' scheduler and the routes' listener.
        self.skills.start();
        // The plugins' idle unloading and the events they follow; their schedules' tick.
        self.plugins.start();
        // The apps' long tasks: a cancel nobody answered, an app that fell silent, results kept long enough.
        self.tasks.watch();
        self.schedules.start();
        // An app came into force: its first data rhythm runs now, not at the next tick (plugins/schedule.rs, `soon`).
        let schedules = Arc::downgrade(&self.schedules);
        self.events.on("plugins.schedules", move |event| {
            if let crate::core::events::GatewayEvent::PackagesChanged { change, .. } = event.as_ref() {
                if crate::plugins::schedule::Scheduler::brings_into_force(change) {
                    if let Some(schedules) = schedules.upgrade() {
                        schedules.soon();
                    }
                }
            }
        });
        // The keyboards: the system's news of them first (a Mac), then the list and the keeper of idle ports.
        self.midi.start();
        // The bus follows the settings and the apps removed; the folders' `tmp/` and bin are cleaned every hour.
        self.bus.start();
        if self.config.data_dir.is_some() {
            if let Some(files) = self.bus.files() {
                files.places().prepare();
                if let Some(folders) = files.folders() {
                    folders.apply();
                }
                if let Some(drives) = files.drives() {
                    drives.start();
                }
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
        // The talk layer's constant phrases in our own voice, kept ahead: now and whenever the voice changes.
        self.warm_phrases();
        {
            let me = Arc::downgrade(self);
            self.events.on("talk.phrases", move |event| {
                if let (Some(app), crate::core::events::GatewayEvent::VoiceChanged { .. }) =
                    (me.upgrade(), event.as_ref())
                {
                    app.warm_phrases();
                }
            });
        }
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

    /// The language pack's constant phrases synthesized ahead in our own voice, in the background (only while our
    /// voice speaks: Alice's needs none).
    fn warm_phrases(&self) {
        let Some(own) = self.voice.own_speech().cloned() else { return };
        if self.voice.for_computer().is_none() {
            return;
        }
        let prompt = self.personas.system_prompt_for(None, crate::dialog::Channel::Station).text;
        let pack = crate::talk::pack::of(crate::memory::text::lang_of(&prompt));
        let texts: Vec<String> = pack.work.say.kept_ahead.iter().map(|text| text.to_string()).collect();
        tokio::spawn(async move { own.warm(&texts, &tokio_util::sync::CancellationToken::new()).await });
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
            // A move's characters and own sources first (placement/carry.rs, item 43): the package is applied after.
            crate::placement::transfer::take_carried(
                dir,
                &config.plugins.packages_dir,
                &config.plugins.catalog_dir,
                &db,
            );
            crate::placement::transfer::apply_incoming(dir, &db, &secrets, station, Some(&pictures));
        }
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        // The household's language for the gateway's own words (src/locale/text.rs).
        crate::locale::text::remember_household(crate::locale::household(&settings));
        // Who this gateway is: its home and itself, made when missing — after the other gateway's package, which may
        // bring the home, and before the audit log listens (nobody made them).
        let identity = Arc::new(Identity::new(
            settings.clone(),
            secrets.clone(),
            db.clone(),
            DeviceKind::of(crate::core::build::Profile::for_role(config.placement.role.as_deref())),
            events.clone(),
        ));
        if let Err(error) = identity.ensure() {
            tracing::error!("identity: the home and this gateway's id are not made: {error}");
        }
        // This computer as a helper (stage 7): who it helps, and its side of the link — its certificate made at the
        // first start of a computer that helps (never in the station role).
        #[cfg(feature = "pc")]
        let (clients, link) = {
            let station = crate::core::build::Profile::for_role(config.placement.role.as_deref()).station;
            let clients =
                Arc::new(crate::helpers::clients::Clients::new(settings.clone(), secrets.clone(), events.clone()));
            let link = Arc::new(crate::helpers::link_serve::LinkServer::new(
                config.clone(),
                secrets.clone(),
                settings.clone(),
                events.clone(),
                clients.clone(),
                !station,
            ));
            (clients, link)
        };
        // The old «Домашний сервер» into the helpers' list (and back, for a rollback): before anything reads the
        // placement.
        crate::helpers::door::at_start(&config, &settings, &secrets);
        // «Выход в интернет» (src/egress): its routes are read by every HTTP client; decided before anything goes out.
        let egress =
            crate::egress::Egress::new(config.clone(), db.clone(), settings.clone(), secrets.clone(), events.clone());
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
        // Ready phrases in the household's own voice, kept on disk (src/speech/tts/phrase_cache.rs).
        if let (Some(dir), Some(own)) = (config.data_dir.as_ref(), voice.own_speech()) {
            own.set_cache(Arc::new(crate::speech::tts::phrase_cache::PhraseCache::new(dir.join("phrases"))));
        }
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
        // The station's cloud hearing and voice as the panel sets them (src/speech/cloud_speech.rs): read on every use.
        let cloud_speech = Arc::new(crate::speech::cloud_speech::CloudSpeech::new(
            settings.clone(),
            secrets.clone(),
            config.speech.asr.clone(),
            config.gateway.tts.own.clone(),
            profile.station,
        ));
        if profile.station {
            availability.set_cloud(cloud_speech.clone());
            voice.set_cloud(cloud_speech.clone());
        }
        // The helpers: their state feeds what works where and /health; a station's voice goes through them first.
        let helpers = crate::helpers::Helpers::new(
            config.clone(),
            settings.clone(),
            secrets.clone(),
            events.clone(),
            profile,
            identity.clone(),
        );
        availability.set_helpers(helpers.clone());
        placement.set_home_server(helpers.clone());
        // Our voice through the helpers on every gateway: a computer's own first (its built-in helper), the regulator
        // over a cloud one; only a station leans on them for its «Голос» window (`HomeVoice::leans`).
        voice.set_home(helpers.clone());
        // «Свой» on the station stands on the cloud's hearing and voice while no helper gives them.
        if profile.station {
            helpers.set_cloud(cloud_speech.clone());
        }
        // The old «Домашний сервер», now the helpers' entry `home-computer`, for the code and the routes that ask it.
        let home_server = crate::home_server::HomeServerService::over(helpers.clone());
        let mode_service = Arc::new(ModeService::new(
            config.gateway.mode,
            settings.clone(),
            events.clone(),
            voice.clone(),
            availability.clone(),
        ));
        // The computer's microphone holds «Своё» (src/talk/input.rs), whatever mode was stored.
        crate::talk::input::hold_at_start(&mode_service, &settings, profile.station);
        let modes: Arc<dyn ModeSource> = mode_service.clone();
        availability.set_modes(modes.clone());
        // The gateway's own switch out of «Свой» and back, as its helpers go and come.
        helpers.set_modes(mode_service.clone());
        // The station by itself in «Свой» on the cloud's hearing and voice: one gone (off in its panel, the key
        // removed) — «Свой» gives way to «Шлюз» rather than stay deaf or mute (docs/plans/speaker-alone.md).
        if profile.station && tokio::runtime::Handle::try_current().is_ok() {
            let (modes, registry) = (mode_service.clone(), availability.clone());
            events.on("cloud-svoi", move |event| {
                let crate::core::events::GatewayEvent::AvailabilityChanged { features } = &*event else { return };
                // With helpers their watch leaves and brings back «Свой» itself (src/helpers).
                let alone = registry.context().placement == crate::core::availability::Placement::Station;
                let own = modes.current() == crate::settings::definitions::PanelMode::Own;
                if !alone || !own || !features.iter().any(|id| id == "mode.own") {
                    return;
                }
                let Some(state) = registry.get("mode.own").filter(|state| !state.on()) else { return };
                match modes.leave_by_itself(
                    crate::settings::definitions::PanelMode::Own,
                    crate::settings::definitions::PanelMode::Gateway,
                    &state.reason,
                ) {
                    Ok(true) => tracing::warn!("mode: «Свой» → «Шлюз» by itself: {}", state.reason),
                    Ok(false) => {}
                    Err(error) => tracing::warn!("mode: «Свой» not left: {}", error.message),
                }
            });
        }
        availability.start(&events);
        let requests = RequestsService::new(db.clone(), events.clone(), settings.clone(), modes.clone());
        requests.start();
        let live = LiveHub::new(requests.clone());
        live.start(&events);
        let health = Arc::new(HealthStatusService::new(modes.clone()));
        health.set_placement(placement.clone());
        health.set_gate(availability.clone());
        health.set_identity(identity.clone());
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
        // «Мозг» knows the helpers (docs/plans/home-and-helpers.md, stage 4): each helper with a model has its row, a
        // chosen model whose helper is away gives way at once, and the gateway sets a backup by itself where it keeps
        // its own settings.
        brain.set_helpers(helpers.clone());
        {
            let placement = placement.clone();
            brain.set_owner(Arc::new(move || placement.owns_settings()));
        }
        // A model whose VPN does not answer now («Выход в интернет», published since `Egress::new`) stands in for
        // nobody: not for an away helper's model, not as the gateway's own backup.
        brain.set_refusals(Arc::new(|provider| crate::egress::refused_row(&format!("provider:{provider}"))));
        if let Err(error) = brain.keep_helper_models() {
            tracing::warn!("brain: the helpers' models are not kept: {}", error.message);
        }
        brain.keep_backup();
        {
            use crate::core::events::GatewayEvent;
            // The rows follow the helpers; a row a choice kept goes once no choice answers with it; a helper up, or one
            // away (the next helper is wanted then), gets the list asked that it could not give before.
            let models = Arc::downgrade(&brain);
            events.on("helper-models", move |event| {
                let follows = match &*event {
                    GatewayEvent::HelpersChanged { .. } | GatewayEvent::HelperUp { .. } => true,
                    GatewayEvent::HelperDown { .. } => true,
                    GatewayEvent::BrainChanged { change, .. } => change == "choice",
                    _ => false,
                };
                if let Some(brain) = models.upgrade().filter(|_| follows) {
                    if let Err(error) = brain.keep_helper_models() {
                        tracing::warn!("brain: the helpers' models are not kept: {}", error.message);
                    }
                }
            });
            let backup = Arc::downgrade(&brain);
            events.on("brain-backup", move |event| {
                let looks_again = match &*event {
                    GatewayEvent::BrainChanged { change, .. } => change != "backup.auto",
                    GatewayEvent::HelpersChanged { .. } => true,
                    GatewayEvent::SettingsChanged { key, .. } => key == crate::settings::definitions::AWAY.key,
                    _ => false,
                };
                if let Some(brain) = backup.upgrade().filter(|_| looks_again) {
                    brain.keep_backup();
                }
            });
        }
        // A sign-in's return comes to the address a browser on this computer reaches the gateway at.
        let oauth = Arc::new(OAuthService::new(config.app.public_port.unwrap_or(config.app.port), secrets.clone()));
        let source: Arc<dyn BrainSource> = brain.clone();
        let dialog = Arc::new(DialogService::new(config.dialog.clone(), personas.clone(), Some(source)));
        dialog.attach_memory(memory.clone());
        // The journal of deals and the prices (src/router): what each answer of a model cost; the model of .env is
        // priced by its kind and address.
        let router = crate::router::Router::new(
            db.clone(),
            events.clone(),
            crate::router::pool::EnvModel { kind: config.dialog.provider, base_url: config.dialog.base_url.clone() },
        );
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
                let mut options = LocalAsrOptions::new(
                    local.dir.clone(),
                    local.mirror.clone(),
                    config.speech.asr.language.clone(),
                    local.gpu,
                );
                // It hears the household's language (unless ASR_LANGUAGE says one), read from the setting itself:
                // the models' shop looks again when it changes (src/models).
                let household = settings.clone();
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
        // The built-in helper (src/helpers/builtin.rs): this computer's heavy part behind the helpers' contract, made in
        // both roles — `/home/v1` answers through it —; a computer that is not in the station role takes its own
        // hearing, voice and model through it, first among its helpers.
        #[cfg(feature = "pc")]
        let builtin = crate::helpers::builtin::BuiltinHelper::new(crate::helpers::builtin::BuiltinParts {
            config: config.clone(),
            db: db.clone(),
            settings: settings.clone(),
            secrets: secrets.clone(),
            events: events.clone(),
            identity: identity.clone(),
            local_asr: local_asr.clone(),
            voice: voice.clone(),
            local: helpers.local(),
        });
        #[cfg(feature = "pc")]
        if !profile.station {
            helpers.set_own(builtin.clone());
            // «Мозг» sees this computer's model servers now: a main model on one of them gets its default backup.
            brain.keep_backup();
        }
        // Recognition goes the helpers' way (src/home_server/chain.rs): this computer's own parts first, then the helpers
        // of the list, then the cloud while the regulator lets it out — else Yandex.
        // On the station the cloud is the panel's (src/speech/cloud_speech.rs), read on every request; elsewhere the .env's.
        let chain = if profile.station {
            crate::home_server::chain::station_chain(cloud_speech.clone(), helpers.clone())
        } else {
            crate::home_server::chain::hearing_chain(&config.speech.asr, helpers.clone())
        };
        let speech = SpeechService::with_recognizer(config.speech.clone(), Some(Arc::new(chain)));
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
                helpers.clone(),
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
        plugin_services.set_model(&dialog);
        // The voices' providers (capabilities/voices.rs): the computer's, ElevenLabs by the owner's key — the clouds' only
        // while the regulator «Что может уходить из дома» lets them out.
        let voices = {
            let helpers = helpers.clone();
            Arc::new(crate::capabilities::voices::Voices::new(Arc::new(move |reach| helpers.lets(reach))))
        };
        voices.register(Arc::new(crate::capabilities::voices::Computer::new(&voice)));
        #[cfg(feature = "pc")]
        voices.register(Arc::new(crate::capabilities::voices::ElevenLabs::new(&secrets)));
        plugin_services.set_voices(&voices);
        plugin_services.set_memory(&memory);
        plugin_services.set_speech(&speech);
        plugin_services.set_brain(&brain);
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
                placement: Arc::new(move || availability.placement()),
                runtime: Arc::new(crate::plugins::quickjs::QuickJs),
                memory: Arc::new(crate::plugins::budget::MachineMemory),
                gauge,
                bundled: if config.plugins.bundled { crate::plugins::bundled::packages() } else { Vec::new() },
                player: Some(skills.player.clone()),
                watch: Some(skills.player_watch.clone()),
                home: Some(helpers.link_for(crate::helpers::contract::Ability::Apps)),
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
        egress.set_plugins(&plugins);
        plugin_services.set_egress(&egress);
        // The apps' long tasks (src/tasks): what ran when the gateway stopped is marked interrupted here. The plugin
        // host tells them when an app's program is gone and when an app is removed.
        let tasks = crate::tasks::Tasks::new(db.clone(), events.clone());
        plugins.set_tasks(tasks.clone());
        plugin_services.set_tasks(&tasks);
        {
            let dialog = dialog.clone();
            plugins.set_cloud_model(Arc::new(move || dialog.in_cloud()));
        }
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
            // The home folder is a person's own folder in the app (HOME_FILES: `~/DuoWave`), the data's elsewhere.
            let places = crate::bus::providers::files::Places::new(
                &config.data_dir.clone().unwrap_or_else(|| std::path::PathBuf::from("data")),
                &config.skills.content_dir,
                placement.role() == crate::placement::service::Role::Station,
            )
            .with_home(config.files.home.clone());
            let host = Arc::downgrade(&plugins);
            let folders_places = places.clone();
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
            // The household's word on its folders and what is attached (docs/plans/shared-folders.md); drives are
            // watched by a computer's gateway with FOLDERS on.
            let folders =
                crate::bus::providers::files::rules::Folders::new(db.clone(), folders_places, Some(events.clone()));
            files.set_folders(folders.clone());
            // What leaves the house: the station's voice (ours, or Yandex's) and the model's place, as they are now.
            {
                let (voice, dialog) = (voice.clone(), dialog.clone());
                files.set_leaving(
                    Arc::new(move || voice.whose(true) == crate::speech::voice_service::Whose::Own),
                    Arc::new(move || dialog.in_cloud()),
                );
            }
            if config.files.attach && placement.role() != crate::placement::service::Role::Station {
                files.set_drives(crate::bus::providers::files::drives::Drives::new(folders, Some(events.clone())));
            }
            bus.set_files(files.clone());
            bus.register(files);
        }
        // The plugins' schedules are kept by the gateway that keeps the station's session: the station's own, or the
        // computer the station talks to — not a computer that only helps a station (they would run twice).
        let schedules = {
            let station_build = placement.role() == crate::placement::service::Role::Station;
            let availability = availability.clone();
            let leads = move || station_build || availability.placement() == crate::core::availability::Placement::Pc;
            Arc::new(crate::plugins::schedule::Scheduler::new(
                db.clone(),
                Arc::downgrade(&plugins),
                Arc::new(leads),
                config.plugins.schedules,
            ))
        };
        plugin_services.set_schedules(&schedules);
        // A receiver on the station shows itself to phones by the station's name in DuoWave (DUOWAVE_STATION_NAME).
        if placement.role() == crate::placement::service::Role::Station {
            let placement = Arc::downgrade(&placement);
            plugins.set_station_name(Arc::new(move || {
                placement.upgrade().and_then(|placement| placement.station_self().1)
            }));
        }
        // The plugins' pictures, in DATA_DIR/plugin-files; the station's limits where the gateway runs there.
        let files = {
            let root = crate::plugins::files::root_of(&config.data_dir);
            // An app's program writes its own folder itself (DUOWAVE_APP_FILES): what its window plays.
            plugins.set_app_files_root(root.clone());
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
        // The computer's screen: its windows show only links the gateway made (Files' `media`).
        let screen = crate::screen::Screen::new(Some(events.clone()));
        {
            let bus = Arc::downgrade(&bus);
            screen.set_known(Arc::new(move |token| {
                bus.upgrade().and_then(|bus| bus.files()).is_some_and(|files| files.played(token).is_some())
            }));
        }
        // Devices on the bus: the MIDI keyboard first, the screen; a new device is one more `add`.
        {
            let devices = Arc::new(crate::bus::providers::devices::Devices::new());
            devices.add(Arc::new(crate::bus::providers::devices::MidiKeyboard::new(midi.clone())));
            devices.add(Arc::new(crate::screen::device::ScreenDevice::new(screen.clone())));
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
                Arc::new(move || !availability.placement().on_station() || on_station_flag),
            )
        };
        skills.player_watch.set_relay(Arc::new(crate::gateway::media::stream::Relay(streams.clone())));
        // The workshop: drafts in DATA_DIR/packages-dev, installed into the packages through the plugin host; by voice
        // too («давай сделаем навык»), through the plugins' skill and a rule of the router.
        let workshop = crate::plugins::workshop::Workshop::new(config.plugins.dev_dir.clone(), plugins.clone());
        // A teacher for a MIDI keyboard is made only where a keyboard is heard (docs/plans/midi.md).
        workshop.set_midi(midi.available());
        workshop.set_folders(placement.role() != crate::placement::service::Role::Station);
        // The agent's steps reach the panel as `workshop.changed` (the plaque «Делаю виджет», the workshop's log).
        workshop.attach_events(events.clone());
        // A trial left running before a restart is still taken back on its time; a run the restart cut is interrupted.
        workshop.resume_trials();
        workshop.after_restart();
        // The talk layer (docs/plans/audio-interface.md): the works the workshop does, what the brain knows now.
        let talk = crate::talk::TalkLayer::new(Some(db.clone()), events.clone());
        let workshop_executor = crate::talk::workshop::WorkshopExecutor::new(workshop.clone());
        {
            let live = live.clone();
            workshop_executor.set_panels(Arc::new(move || live.size() > 0));
        }
        {
            // Which rights only the panel approves: a handle the bus marks dangerous under one of them.
            let bus = Arc::downgrade(&bus);
            workshop_executor.set_dangers(Arc::new(move |rights: &[String]| {
                bus.upgrade().is_some_and(|bus| bus.any_dangerous(rights))
            }));
        }
        talk.add_executor(workshop_executor);
        // MCP tools marked «В фоне» run as works (docs/plans/long-tools.md): the executor joins the layer and takes
        // the servers' background calls.
        crate::talk::tool_work::ToolExecutor::new(&talk, &mcp, Some(&dialog));
        // An app's long task begun in a conversation is a work too: «как там…?», «стоп», its «Готово» said there.
        crate::talk::task_work::TaskExecutor::new(&talk, &tasks, &events, Some(&dialog));
        // A work in a work folder (docs/plans/router-and-works.md, 10): the executor in both builds, its helper — the
        // works' module — only on a computer.
        let folder_executor = crate::talk::folder::FolderExecutor::new(&talk);
        #[cfg(feature = "pc")]
        let works = {
            let dir = config.plugins.dev_dir.with_file_name("works");
            let mut guarded: Vec<std::path::PathBuf> = vec![
                config.plugins.packages_dir.clone(),
                config.plugins.dev_dir.clone(),
                config.plugins.catalog_dir.clone(),
                dir.clone(),
            ];
            guarded.extend(config.data_dir.clone());
            guarded.extend(config.skills.state_dir.clone());
            let works = crate::works::FolderWorks::new(
                db.clone(),
                dir,
                guarded,
                settings.clone(),
                secrets.clone(),
                events.clone(),
            );
            folder_executor.attach(works.clone());
            // The agents' cards from MagicPlace: only those installed from the catalog (src/works/agents.rs).
            let host = Arc::downgrade(&plugins);
            works.agents.set_cards(Arc::new(move || {
                let Some(host) = host.upgrade() else { return Vec::new() };
                host.agent_cards()
                    .into_iter()
                    .map(|(id, title, description, card)| crate::works::agents::Card { id, title, description, card })
                    .collect()
            }));
            Some(works)
        };
        {
            // «…: готово.» of a work synthesized ahead, while it is made: its title is known from the start.
            let voice = voice.clone();
            talk.set_warm(Arc::new(move |texts: Vec<String>| {
                let (Some(own), true) = (voice.own_speech().cloned(), voice.for_computer().is_some()) else { return };
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move { own.warm(&texts, &tokio_util::sync::CancellationToken::new()).await });
                }
            }));
        }
        {
            // The language of a device's character: its pack's words for the works' phrases and the block.
            let personas = personas.clone();
            talk.set_langs(Arc::new(move |device: &str| {
                let channel = if device == crate::talk::COMPUTER {
                    crate::dialog::Channel::Computer
                } else {
                    crate::dialog::Channel::Station
                };
                crate::memory::text::lang_of(&personas.system_prompt_for(None, channel).text)
            }));
        }
        {
            let host = Arc::downgrade(&plugins);
            talk.set_desk(Arc::new(move || {
                let Some(host) = host.upgrade() else { return Vec::new() };
                let here = host.placement_now();
                // Apps and plugins only: a character, a theme, a voice is not something to ask for its data.
                host.visible_all()
                    .iter()
                    .filter(|package| {
                        matches!(
                            package.manifest.kind,
                            crate::plugins::manifest::PackageKind::App | crate::plugins::manifest::PackageKind::Plugin
                        )
                    })
                    .filter(|package| host.is_active(&package.manifest.id))
                    .map(|package| crate::talk::layer::DeskApp {
                        id: package.manifest.id.clone(),
                        title: package.manifest.title.clone(),
                        turbo: package.manifest.face_at(here),
                    })
                    .collect()
            }));
            let layer = Arc::downgrade(&talk);
            dialog.attach_now(Arc::new(move |device: &str, lang| {
                layer.upgrade().map(|layer| layer.now(device, lang)).unwrap_or_default()
            }));
            // A phrase about the works is read by the brain's model before the talk (src/talk/reader.rs).
            talk.set_reader(crate::talk::reader::Reader::new(&talk, &dialog));
            // A device's conversation is remembered while its work goes on, and half an hour after.
            let dialog_hold = Arc::downgrade(&dialog);
            talk.set_hold(Arc::new(move |device: &str, until: u64| {
                if let Some(dialog) = dialog_hold.upgrade() {
                    dialog.hold(device, until);
                }
            }));
            // What the model spent, round by round: the workshop's agent on its work, a conversation on the device's
            // request in the journal (docs/plans/audio-interface.md, «Счёт токенов»). Each answer once it is over: a
            // deal of the router, with the request or the work it belongs to (src/router).
            let (layer, journal, deals) = (Arc::downgrade(&talk), Arc::downgrade(&requests), Arc::downgrade(&router));
            let usage: crate::router::UsageSink = Arc::new(move |heard: crate::router::Usage<'_>| {
                let key = match heard {
                    crate::router::Usage::Round { key, .. } => key,
                    crate::router::Usage::Answer(spent) => spent.key.as_str(),
                };
                let draft = key.strip_prefix("workshop:make:");
                let device = if key.starts_with("panel:") {
                    Some(crate::talk::COMPUTER)
                } else {
                    (!key.is_empty() && !key.contains(':')).then_some(key)
                };
                // The work a round or an answer belongs to: the workshop's agent on its draft, a background work's
                // round (talk/tool_work.rs, task_work.rs) by `work:<executor>:<target>`.
                let work = || match draft {
                    Some(draft) => {
                        layer.upgrade().and_then(|layer| layer.work_of(crate::talk::workshop::WORKSHOP, draft))
                    }
                    None => key
                        .strip_prefix("work:")
                        .and_then(|rest| rest.split_once(':'))
                        .and_then(|(executor, target)| layer.upgrade()?.work_of(executor, target)),
                };
                match heard {
                    crate::router::Usage::Round { provider, model, seat, usage, .. } => {
                        match (draft, device) {
                            (Some(draft), _) => {
                                if let Some(layer) = layer.upgrade() {
                                    layer.spent(crate::talk::workshop::WORKSHOP, draft, usage);
                                }
                            }
                            (None, Some(device)) => {
                                if let Some(journal) = journal.upgrade() {
                                    journal.spent(device, model, usage);
                                }
                            }
                            (None, None) => {}
                        }
                        // Who answers a work going on, before its deal is written (the page «Работы»).
                        if let (Some(work), Some(router)) = (work(), deals.upgrade()) {
                            router.heard(&work, provider, model, seat);
                        }
                    }
                    crate::router::Usage::Answer(spent) => {
                        let Some(deals) = deals.upgrade() else { return };
                        let journal = journal.upgrade();
                        let links = crate::router::Links {
                            request: device.and_then(|device| journal.as_ref()?.going(device)),
                            work: work(),
                            part: None,
                        };
                        // The journal's request shows its deal: seat, model, dollars.
                        if let (Some(deal), Some(journal)) = (deals.close(spent, links), journal) {
                            if let Some(request) = &deal.request {
                                journal.dealt(request, &deal);
                            }
                        }
                    }
                }
            });
            dialog.attach_usage(usage.clone());
            reflection.attach_usage(usage);
            // Each run of an agent in a work folder is one deal, by `work:folder:<id>` (talk/folder.rs).
            let (layer, deals) = (Arc::downgrade(&talk), Arc::downgrade(&router));
            let work_of = move |key: &str| {
                let (executor, target) = key.strip_prefix("work:")?.split_once(':')?;
                layer.upgrade()?.work_of(executor, target)
            };
            let work_of = Arc::new(work_of);
            let (closing, hearing) = (work_of.clone(), work_of);
            let heard_by = deals.clone();
            folder_executor.attach_deals(crate::talk::folder::FolderDeals {
                close: Arc::new(move |spent: &crate::router::Spent| {
                    let links = crate::router::Links { request: None, work: closing(&spent.key), part: None };
                    deals.upgrade()?.close(spent, links).map(|deal| deal.id)
                }),
                heard: Arc::new(move |key: &str, provider: &str, model: &str| {
                    if let (Some(work), Some(router)) = (hearing(key), heard_by.upgrade()) {
                        router.heard(&work, provider, model, crate::router::Seat::Worker);
                    }
                }),
            });
        }
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
            history.set_home(helpers.link_for(crate::helpers::contract::Ability::History));
            plugins.set_history(history.clone());
            workshop.set_history(history.clone());
        }
        // The agent works with «Мозг»; the talk layer says «Готово» where the work was asked and hears the answer.
        workshop.set_dialog(dialog.clone());
        // The talk layer's rules (docs/plans/audio-interface.md): the works and the workshop's door, first among the
        // extra rules.
        let talk_rules = crate::talk::rules::TalkRules::new(&talk);
        skills.router.add_rules_first(talk_rules.clone());
        plugin_skill.set_talk(talk_rules);
        // The memory's voice commands («запомни», «забудь», «что ты помнишь»): ours, at once, without the model.
        skills.register(Arc::new(crate::skills::own::memory_skill::MemorySkill::new(memory.clone())));
        skills.router.add_rules(Arc::new(crate::skills::own::memory_skill::MemoryRules));
        // «Верни погоду как было вчера»: the history by voice, beside the plugins' phrases; «верни как было» also undoes
        // a new version a device's voice put on the desk lately (src/talk/ready.rs).
        if let Some(history) = &history {
            let voice_history = crate::plugins::history::voice::VoiceHistory::new(history.clone());
            {
                let layer = Arc::downgrade(&talk);
                voice_history.set_recent_edit(Arc::new(move |device: &str| {
                    layer.upgrade().and_then(|layer| layer.recent_edit(device))
                }));
            }
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
        // What an app plays by itself (AirPlay): the player's voice commands reach it through the plugin host.
        skills.app_playback.set_control(Arc::new(links::app_player::AppPlayerLink(Arc::downgrade(&plugins))));
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
                placement: Arc::new(move || availability.placement()),
                home: Some(helpers.link_for(crate::helpers::contract::Ability::Catalog)),
                apps: Some(helpers.link_for(crate::helpers::contract::Ability::Apps)),
            })
        };
        // MagicPlace by voice (src/talk/store.rs, docs/plans/magicplace.md): it installs from this catalog under the
        // switch of the desk by voice, and the panel opens the store when one listens.
        {
            let live = live.clone();
            talk.set_store(Arc::downgrade(&catalog), settings.clone(), Arc::new(move || live.size() > 0));
        }
        let characters = CharactersService::new(db.clone(), events.clone(), personas.clone(), plugins.clone());
        memory.attach_characters(characters.clone());
        // The chosen character's catchphrases, from the feed its package came from (src/characters/catchphrases.rs):
        // said now and then by the dialog, word for word.
        {
            let (catalog, personas) = (Arc::downgrade(&catalog), personas.clone());
            dialog.attach_catchphrases(Arc::new(move || {
                let chosen = personas.chosen()?.preset_id?;
                let (package, character) = chosen.split_once(':')?;
                catalog.upgrade()?.catchphrases(package, character)
            }));
        }
        #[cfg(feature = "pc")]
        characters.link_voices(crate::characters::voices::CharacterVoices::new(
            voice_lab.clone(),
            models.clone(),
            Arc::new(links::voice::LabVoices(voice_lab.clone())),
            settings.clone(),
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
        // The works' tools for the brain: begin a thing from the whole wish, how it goes, the desk (src/talk).
        if let Err(error) = connectors.register(Arc::new(crate::talk::work_tools::TalkConnector::new(&talk))) {
            tracing::error!("{error}");
        }
        // The screen's windows for the model: which are open, keep, close, pause (src/screen/tools.rs).
        if let Err(error) = connectors.register(Arc::new(crate::screen::tools::ScreenConnector::new(&screen))) {
            tracing::error!("{error}");
        }
        // The VPN's switch for the brain: `vpn_on`, `vpn_off`, `vpn_status`, while the app «VPN» is installed
        // (src/egress/tools.rs).
        if let Err(error) = connectors.register(Arc::new(crate::egress::tools::VpnConnector::new(&egress))) {
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
            talk: Some(talk.clone()),
            characters: Some(characters.clone()),
        });
        // An app on the station that hears a word of its own wakes the station through its session (src/activator).
        plugin_services.set_activator(crate::activator::Activator::new(
            placement.role() == crate::placement::service::Role::Station,
            station_sessions.wake_station(),
        ));
        // The plugins' turn timer says «время вышло» on the device's live station; their schedules speak there too.
        plugin_skill.set_turns(station_sessions.turns());
        // «Готово» of a work said on the station that asked; once more after its next talk when it was cut.
        talk.set_turns(station_sessions.turns());
        talk.follow_requests();
        // A thing removed — an app off the desk, a draft deleted — reaches its works by its id; what went while the
        // gateway was off is checked now.
        talk.follow_removals();
        talk.reconcile();
        // A drive plugged in: the station asks about it when it is free, by the app «Папки» (its function
        // `driveFound`); the tile on the desk asks in any case (docs/plans/shared-folders.md, the owner's decision 3).
        if let Some(drives) = bus.files().and_then(|files| files.drives()) {
            let turns = station_sessions.turns();
            let host = Arc::downgrade(&plugins);
            let watched = Arc::downgrade(&drives);
            drives.set_on_offer(Arc::new(move |offer: &crate::bus::providers::files::drives::Offer| {
                let (turns, host, watched, offer) = (turns.clone(), host.clone(), watched.clone(), offer.clone());
                tokio::spawn(async move {
                    let app = crate::bus::providers::files::FOLDERS_APP;
                    if !host.upgrade().is_some_and(|host| host.is_active(app)) || turns.busy("*") {
                        return;
                    }
                    use crate::skills::capabilities::{CapabilityName, Intent};
                    let mut intent = Intent::new(CapabilityName::Plugins, "drive");
                    intent.params.insert("plugin".into(), serde_json::json!(app));
                    intent.params.insert("handler".into(), serde_json::json!("driveFound"));
                    intent.params.insert("slots".into(), serde_json::json!({ "name": offer.name, "offer": offer.id }));
                    let id = offer.id.clone();
                    let still = Arc::new(move || watched.upgrade().is_some_and(|drives| drives.offer(&id).is_some()))
                        as Arc<dyn Fn() -> bool + Send + Sync>;
                    turns.ask("*", intent, format!("флешка · {}", offer.name), still).await;
                });
            }));
        }
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
            // Nor is one awaited while the household talks by this computer's microphone (docs/plans/mic-mode.md): «колонка
            // не подключена… вернуть к Яндексу» every five minutes to someone with no station is noise (live 14.11).
            let gateways = Arc::downgrade(&station_gateway);
            let talk_settings = Arc::downgrade(&settings);
            station_watch.set_elsewhere(Arc::new(move || {
                talk_settings.upgrade().is_some_and(|settings| crate::talk::input::on_computer(&settings))
                    || gateways.upgrade().is_some_and(|gateways| gateways.none_here())
            }));
            // The bot's messages reach the stations this computer helps: they are not at its /uni.ws (live 11.11).
            skills.calls.attach_relay(crate::placement::messages::StationMessages::new(&station_gateway));
        }
        {
            let watch = station_watch.clone();
            events.on("safety-alert", move |event| {
                if let crate::core::events::GatewayEvent::SafetyFired { what, .. } = &*event {
                    let text = match what.as_str() {
                        "previous-version" => {
                            "Страховка: новая версия DuoWave на колонке не поднялась — вернулась прежняя."
                        }
                        _ => "Страховка: DuoWave на колонке не поднимался — колонка вернулась к прежнему адресу.",
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
            settings: settings.clone(),
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

        // An update on the station (src/updates/trial.rs): whether the station is quiet now, its apps switched off for a
        // trial's memory and back. A trial that switched them off holds them from its start.
        updates.set_station_hooks(Arc::new(StationUpdateHooks {
            sessions: Arc::downgrade(&station_sessions),
            apps: skills.app_playback.clone(),
            plugins: Arc::downgrade(&plugins),
        }));
        if updates.holds_apps() {
            plugins.hold_apps(true);
        }

        // ── The links end here. ──

        let watch = Arc::new(crate::plugins::watch::WatchPages::new(
            config.data_dir.clone().unwrap_or_else(|| std::path::PathBuf::from("data")).join("watch"),
        ));
        // Publishers (capabilities/publish.rs): the gateway's own first, then the platforms' upload pages.
        let publishers = Arc::new(crate::capabilities::publish::Publishers::new());
        publishers.register(Arc::new(crate::capabilities::publish::Watch { pages: watch.clone() }));
        #[cfg(feature = "pc")]
        publishers.register(Arc::new(crate::capabilities::publish::Telegram { bot: calls.telegram.clone() }));
        publishers.register_upload_pages();
        plugin_services.set_publishers(&publishers);

        let app = Arc::new(App {
            config,
            db,
            events,
            secrets,
            settings,
            identity,
            egress,
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
            router,
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
            talk,
            #[cfg(feature = "pc")]
            works,
            history,
            plugins,
            tasks,
            schedules,
            files,
            watch,
            publishers,
            voices,
            midi,
            bus,
            screen,
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
            builtin,
            #[cfg(feature = "pc")]
            calls,
            placement,
            availability,
            helpers,
            cloud_speech,
            home_server,
            #[cfg(feature = "pc")]
            clients,
            #[cfg(feature = "pc")]
            link,
            #[cfg(feature = "pc")]
            station_gateway,
            #[cfg(feature = "pc")]
            moves,
            modes: mode_service,
            sessions: station_sessions,
            started: Instant::now(),
            started_at: now_iso(),
        });
        // The built-in helper's late parts: the bot, the apps, their history, the catalog (stage 6).
        #[cfg(feature = "pc")]
        app.builtin.attach(Arc::downgrade(&app));
        // What needs the whole app joins here (stage 6: the built-in helper; stage 7: the link).
        #[cfg(feature = "pc")]
        app.link.attach(Arc::downgrade(&app));
        Ok(app)
    }

    /// Stops what runs beside the HTTP server: MCP connections and their programs, the skills' tasks.
    pub async fn shutdown(&self) {
        // The helpers' looks end: an engine started again in its process leaves no tick behind; the links close.
        self.helpers.stop();
        // Who this computer helped, and when it last saw them.
        #[cfg(feature = "pc")]
        self.clients.persist_seen();
        self.mcp.shutdown().await;
        self.skills.shutdown();
        // Before the apps stop: what they ran was cut short by the gateway, not by them.
        self.tasks.shutdown();
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
        self.router.stop();
        // The agents in work folders end their turns: their works are interrupted, «продолжай» goes on.
        #[cfg(feature = "pc")]
        if let Some(works) = &self.works {
            works.stop_all();
        }
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
        // The homes' links hear that this helper stops (1001), this gateway's own links close (1000).
        #[cfg(feature = "pc")]
        self.link.close_all();
        self.helpers.stop_links();
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
        .merge(crate::speech::cloud_speech::routes())
        .merge(crate::health_status::routes())
        .merge(crate::panel::routes())
        .merge(crate::connectors::routes())
        .merge(crate::updates::router())
        .merge(crate::gateway::media::router())
        .merge(crate::gateway::station_http::router())
        // The station link: the station's API (/api/v1/station, /debug). Its voice socket comes before the routes.
        .merge(crate::gateway::debug::router())
        .merge(crate::dialog::routes())
        .merge(crate::router::routes::routes())
        .merge(crate::memory::routes())
        .merge(crate::sleep::routes())
        .merge(crate::models::router())
        .merge(crate::mcp::routes())
        .merge(crate::oauth::routes())
        .merge(crate::plugins::routes())
        .merge(crate::bus::routes::routes())
        .merge(crate::bus::providers::files::routes::routes())
        .merge(crate::screen::routes::routes())
        .merge(crate::midi::routes::routes())
        .merge(crate::skills::own::later_skill::routes())
        .merge(crate::plugins::workshop::routes::routes())
        .merge(crate::plugins::history::routes::routes())
        .merge(crate::speech::listen::routes())
        .merge(crate::talk::routes())
        .merge(crate::locale::routes())
        .merge(crate::egress::routes::routes())
        .merge(crate::apps::host_http::routes())
        .merge(crate::plugins::watch::router())
        .merge(crate::capabilities::routes::routes())
        .merge(crate::tasks::routes::routes())
        .merge(crate::characters::routes())
        .merge(crate::stations::router())
        .merge(crate::placement::router())
        .merge(crate::home_server::router())
        .merge(crate::helpers::routes::router())
        .merge(crate::home::routes())
        // The bridge of this gateway's own clients to its linked helpers (src/helpers/link.rs).
        .merge(crate::helpers::link::via_routes())
        .merge(crate::voice_lab::router());
    // Our calls are a PC's part: the station's build has no /calls.
    #[cfg(feature = "pc")]
    let routes = routes.merge(crate::calls::router());
    // The works in folders: folders, agents, runs and their journals (src/works) — a computer's part.
    #[cfg(feature = "pc")]
    let routes = routes.merge(crate::works::routes::routes());
    // The helper's pass to the station's gateway (src/placement/proxy.rs): a PC's part too.
    #[cfg(feature = "pc")]
    let routes = routes.merge(crate::placement::proxy::routes());
    let routes = routes
        .fallback(not_found)
        // After every route: a path that exists with another method is Express's 404 too, not 405.
        .method_not_allowed_fallback(not_found)
        .with_state(app.clone())
        // An ear of the home's centre (src/home/ear.rs, stage 4): the home is edited at the centre, not here.
        .layer(axum::middleware::from_fn_with_state(app.clone(), crate::home::ear::guard))
        .layer(axum::middleware::from_fn(error_bodies))
        // The language of the request (`Accept-Language`): the views of the apps answer in it (src/locale).
        .layer(axum::middleware::from_fn(crate::locale::request_language))
        .layer(axum::middleware::from_fn_with_state(names, security))
        // The station's voice socket (an upgrade to UNIPROXY_PATH), before the Host check: Node's 'upgrade' listener
        // never went through Express's middleware. The upgrade has its own checks.
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            crate::gateway::uniproxy::uniproxy_server::station_socket,
        ));
    // The link's door for a proxy on this computer (a plain upgrade to /home/v1/link), outermost: the proxy passes the
    // site's name, which the Host check does not know.
    #[cfg(feature = "pc")]
    let routes = routes.layer(axum::middleware::from_fn_with_state(app, crate::helpers::link_serve::link_socket));
    routes
}

/// Serves the router on a listener until `shutdown` resolves, then stops the services that run beside it. A computer
/// that helps serves the link's own TLS port beside it, on the same address (src/helpers/link_serve.rs): both end
/// together, and both ports are free when this returns.
pub async fn serve(
    app: Arc<App>,
    listener: TcpListener,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let stopping = app.clone();
    // Apps reach the Host API at the address the server really listens on.
    if let Ok(address) = listener.local_addr() {
        app.plugins.set_gateway_url(format!("http://127.0.0.1:{}", address.port()));
        app.helpers.set_local_port(address.port());
        // imprint.py hears the words of a take through this gateway when recognition is inside it.
        #[cfg(feature = "pc")]
        app.voice_lab.set_local_port(address.port());
    }
    #[cfg(feature = "pc")]
    crate::helpers::link_serve::raise_open_files();
    #[cfg(feature = "pc")]
    let link = crate::helpers::link_serve::bind(&app, &listener).await;
    let link_stop = tokio_util::sync::CancellationToken::new();
    let stop_link = link_stop.clone();
    let main = axum::serve(listener, router(app.clone()).into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async move {
            shutdown.await;
            // What would hold the graceful close ends now: the panel's live streams, the station clients' polls and
            // streams, the calls' sockets, the links.
            stopping.live.shutdown();
            stopping.before_shutdown();
            stop_link.cancel();
        });
    #[cfg(feature = "pc")]
    let served = match link {
        Some(link) => {
            let link = crate::helpers::link_serve::serve(app.clone(), link, link_stop.cancelled_owned());
            let (served, linked) = tokio::join!(std::future::IntoFuture::into_future(main), link);
            if let Err(error) = linked {
                tracing::warn!("link: the port stopped: {error}");
            }
            served
        }
        None => main.await,
    };
    #[cfg(not(feature = "pc"))]
    let served = {
        drop(link_stop);
        main.await
    };
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

/// What an update on the station asks of the rest of the gateway (src/updates/trial.rs).
struct StationUpdateHooks {
    sessions: std::sync::Weak<SessionRegistry>,
    apps: Arc<crate::skills::media::app_playback::AppPlayback>,
    plugins: std::sync::Weak<PluginHost>,
}

impl crate::updates::StationHooks for StationUpdateHooks {
    fn quiet(&self) -> bool {
        self.sessions.upgrade().is_none_or(|sessions| sessions.quiet()) && !self.apps.any_playing()
    }

    fn pause_apps(&self) -> futures::future::BoxFuture<'static, Vec<String>> {
        let plugins = self.plugins.upgrade();
        Box::pin(async move {
            match plugins {
                Some(plugins) => plugins.pause_apps().await,
                None => Vec::new(),
            }
        })
    }

    fn resume_apps(&self) {
        if let Some(plugins) = self.plugins.upgrade() {
            plugins.hold_apps(false);
        }
    }
}
