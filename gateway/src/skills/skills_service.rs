//! Our implementations of Alice's capabilities and the router that picks, per request, ours, Yandex's or "not
//! yet". One instance for the whole gateway: per-device state lives in the skills, keyed by device.
//!
//! The `.ts` files named here are the Node gateway's, removed from the tree: git tag `node-gateway-last`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use serde::Serialize;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::callbacks::OUR_CALLBACK_PREFIX;
use super::capabilities::{CapabilityName, Intent};
use super::js;
use super::lookups::news_skill::{FeedSource, NewsFeed, NewsSkill};
use super::lookups::weather_skill::{PlaceSource, WeatherSkill};
use super::media::favorites::Favorites;
use super::media::media_skills::{MediaControl, MusicSkill, RadioSkill, TaleWriter, TalesSkill};
use super::media::playback::{PlaybackWatch, RETRY_CALLBACK};
use super::media::player::{MediaPlayer, PlayerEvent, MEDIA_CALLBACK, MEDIA_PLAY_ID, STACK_GET_NEXT};
use super::media::sources::{FolderLibrary, MediaSources, StaticRadioCatalog};
use super::own::alarms_skill::AlarmsSkill;
use super::own::bluetooth_skill::BluetoothSkill;
use super::own::calls_skill::{CallsAccess, CallsSkill, HubAccess, MessengerAccess, NoCalls};
use super::own::clock_skill::ClockSkill;
use super::own::do_not_disturb_skill::DoNotDisturbSkill;
use super::own::games_skill::{GameHost, GamesSkill, ModelOn};
use super::own::led_skill::LedSkill;
use super::own::playback_skill::{PlaybackSkill, PlayerControl};
use super::own::reminders_skill::{RemindersSkill, Speaker};
use super::own::timers_skill::{TimerView, TimersSkill};
use super::own::volume_skill::VolumeSkill;
use super::player_memory::{ContentSource, PlayerMemory};
use super::router::{RouteInput, Router};
use super::routes::{serialize_routes, RouteTable};
use super::skill::{Outcome, OwnSkill, PhraseStream};
use super::skill_stats::SkillStats;
use super::smart_home::home_assistant::HomeAssistantProvider;
use super::smart_home::provider::SmartHomeProvider;
use super::smart_home::smart_home_skill::{ProviderSource, SmartHomeSkill};
use super::tools;
use crate::config::{HomeAssistantConfig, SkillsConfig};
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::dialog::llm::ToolCall;
use crate::gateway::protocol::protocol_types::{DeviceState, PlayerState};
use crate::platform::http::ApiError;
use crate::settings::definitions::ROUTES;
use crate::settings::SettingsService;
use crate::storage::{Db, StateStore};
use crate::utils::time::now_ms;

/// What the skills need of the dialog (src/dialog/dialog.service.ts): whether a model answers, a tale told, a game
/// hosted.
pub trait DialogAccess: Send + Sync {
    /// Whether a model answers now: the panel may switch it off and on without a restart.
    fn enabled(&self) -> bool;
    /// Instructions told phrase by phrase by the smart model (a tale); nothing without a model.
    fn narrate(&self, instructions: String, cancel: CancellationToken) -> PhraseStream;
    /// A conversation in a role (a game host): its own prompt and its own history under `key`; `fresh` forgets the
    /// history first.
    fn roleplay(
        &self,
        key: String,
        system: String,
        text: String,
        cancel: CancellationToken,
        fresh: bool,
    ) -> PhraseStream;
}

/// The calls module as the skills reach it; `CallsLinks::none()` where there is none (the station's build).
#[derive(Clone)]
pub struct CallsLinks {
    pub hub: Arc<dyn HubAccess>,
    pub calls: Arc<dyn CallsAccess>,
    pub messenger: Arc<dyn MessengerAccess>,
}

impl CallsLinks {
    pub fn none() -> CallsLinks {
        let none = Arc::new(NoCalls);
        CallsLinks { hub: none.clone(), calls: none.clone(), messenger: none }
    }
}

/// What the panel sets up for the skills (src/connectors, through src/links): read on every request, so a change
/// applies at once.
pub trait SkillConnectors: Send + Sync {
    /// The news feeds switched on.
    fn news_feeds(&self) -> Vec<NewsFeed>;
    /// The weather's usual place; None: the city of the station's time zone.
    fn weather_place(&self) -> Option<String>;
    /// Home Assistant, when it is set up and on.
    fn home_assistant(&self) -> Option<HomeAssistantConfig>;
}

/// Nothing set up: no feeds, no smart home, the weather by the time zone.
pub struct NoConnectors;

impl SkillConnectors for NoConnectors {
    fn news_feeds(&self) -> Vec<NewsFeed> {
        Vec::new()
    }

    fn weather_place(&self) -> Option<String> {
        None
    }

    fn home_assistant(&self) -> Option<HomeAssistantConfig> {
        None
    }
}

/// What the service is built from.
pub struct SkillsDeps {
    pub config: SkillsConfig,
    /// TTS_MARKUP: sounds in a game's speech.
    pub tts_markup: bool,
    pub db: Arc<Db>,
    pub settings: Arc<SettingsService>,
    pub events: EventBus,
    pub dialog: Arc<dyn DialogAccess>,
    pub calls: CallsLinks,
    /// The news feeds, the weather's place and Home Assistant, as the panel sets them up.
    pub connectors: Arc<dyn SkillConnectors>,
}

/// Takes a callback of one of our commands (our player's on_started, …): the device, the callback's name and its
/// payload; its outcome, if any, is the response.
pub type CallbackHandler =
    Arc<dyn Fn(String, String, Map<String, Value>) -> BoxFuture<'static, Option<Outcome>> + Send + Sync>;

/// The routes as the gateway uses them now, and whether nobody has changed the default yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutesSetting {
    pub routes: BTreeMap<CapabilityName, String>,
    pub is_default: bool,
}

/// The routes after a replacement; problems mean nothing was applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoutesUpdate {
    pub routes: BTreeMap<CapabilityName, String>,
    pub problems: Vec<String>,
}

/// The station's player has nothing to stop: idle, stopped or absent. Bluetooth from a phone does not show here; a
/// state we do not know counts as playing.
fn quiet(state: &DeviceState) -> bool {
    state.player.as_ref().is_none_or(|player| matches!(player.state, PlayerState::Idle | PlayerState::Stopped))
}

/// What the model is asked for a tale: told aloud, a few minutes long.
fn tale_request(topic: Option<&str>) -> String {
    match topic {
        Some(topic) if !topic.is_empty() => {
            format!("Расскажи детскую сказку про {topic}. Минуты на три чтения вслух, с добрым концом.")
        }
        _ => "Расскажи короткую добрую детскую сказку, минуты на три чтения вслух.".into(),
    }
}

fn with_param(intent: &Intent, key: &str, value: Value) -> Intent {
    let mut changed = intent.clone();
    changed.params.insert(key.into(), value);
    changed
}

pub struct SkillsService {
    pub config: SkillsConfig,
    pub router: Arc<Router>,
    pub stats: Arc<SkillStats>,
    pub playback: Arc<PlaybackSkill>,
    pub timers: Arc<TimersSkill>,
    pub alarms: Arc<AlarmsSkill>,
    pub player: Arc<MediaPlayer>,
    /// What the station says of what our player plays: the request's journal, the plugins, the gateway's way for a
    /// silent stream (media/playback.rs).
    pub player_watch: Arc<PlaybackWatch>,
    /// Where the radio, the music and the tales come from: ours, and the plugins' (src/plugins).
    pub media_sources: Arc<MediaSources>,
    /// Next, previous, continue and likes of what our player plays.
    media: Arc<MediaControl>,
    pub reminders: Arc<RemindersSkill>,
    /// Whether the station's player played before a break: its request to continue after reconnecting goes on only
    /// then.
    pub player_memory: Arc<PlayerMemory>,
    pub led: Arc<LedSkill>,
    pub calls: Arc<CallsSkill>,
    pub weather: Arc<WeatherSkill>,
    settings: Arc<SettingsService>,
    events: EventBus,
    /// Whether the stored routes are still the ones the env seeded.
    routes_default: AtomicBool,
    callbacks: RwLock<Vec<(String, CallbackHandler)>>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// How things are said without a request, once the station link gives it: the reminders, the calls, the
    /// plugins.
    speaker: RwLock<Option<Arc<dyn Speaker>>>,
    /// The station's state as its last request carried it — its player, its timers — with when (unix ms): the
    /// panel's widgets between requests (docs/plans/desk.md, stage 2).
    device_states: Mutex<HashMap<String, (DeviceState, u64)>>,
    /// The station's own radio we stopped from the panel (player_pause and clear_queue), per device, and when: its
    /// queue is gone with the station, so the panel's «продолжить» is answered in words until something plays there.
    radio_stopped: Mutex<HashMap<String, u64>>,
}

/// The panel's «продолжить» for the station's own radio after we stopped it: its player_continue brings nothing back
/// once the queue is cleared (live 03.11, `yandex_content`) — the words that do.
const STATION_RADIO_STOPPED: &str = "Радио колонки так не вернуть — скажите ей «Алиса, включи радио».";

/// What plays on a station, for the panel's widget: our player's item, else what the station said it plays.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NowPlaying {
    /// `own` — our player, `station` — the station's own (Yandex's), `none`.
    pub source: &'static str,
    /// `playing`, `paused`, `stopped`, `idle`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    /// FmRadio, Track, FairyTale…
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Where in the item it is now, ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_ms: Option<u64>,
    /// Unix ms when it started playing (a radio's «играет 44 минуты»).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
}

impl SkillsService {
    /// Every skill, the router and its interceptors; the routes seeded and loaded from the database.
    pub fn new(deps: SkillsDeps) -> Arc<SkillsService> {
        let SkillsDeps { config, tts_markup, db, settings, events, dialog, calls, connectors } = deps;
        let content = std::path::absolute(&config.content_dir).unwrap_or_else(|_| config.content_dir.clone());
        let player = Arc::new(MediaPlayer::new());
        let player_watch = PlaybackWatch::new(player.clone());
        // Every source of media, ours first; the plugins' join as they are switched on (src/plugins).
        let media_sources = MediaSources::new(vec![
            Arc::new(StaticRadioCatalog::new(content.join("radio.json"))),
            Arc::new(FolderLibrary::new(content.clone(), "music", "Track")),
            Arc::new(FolderLibrary::new(content.clone(), "tales", "FairyTale")),
        ]);
        // The model may come and go with the panel's choice: the skills ask whether it answers on every request.
        let model_on: ModelOn = {
            let dialog = dialog.clone();
            Arc::new(move || dialog.enabled())
        };
        let writer: TaleWriter = {
            let dialog = dialog.clone();
            Arc::new(move |topic: Option<String>, cancel| dialog.narrate(tale_request(topic.as_deref()), cancel))
        };
        let favorites = Arc::new(Favorites::new(StateStore::open(db.clone(), "favorites")));
        let mut skills: Vec<Arc<dyn OwnSkill>> = vec![
            Arc::new(RadioSkill::new(media_sources.clone(), player.clone())),
            Arc::new(MusicSkill::new(media_sources.clone(), player.clone(), Some(favorites.clone()))),
            Arc::new(TalesSkill::new(
                media_sources.clone(),
                player.clone(),
                content.join("tales").join("generated"),
                Some(writer),
                Some(model_on.clone()),
            )),
        ];
        let media = Arc::new(MediaControl::new(player.clone(), Some(favorites)));
        let playback = Arc::new(PlaybackSkill::new());
        playback.attach_player(media.clone() as Arc<dyn PlayerControl>);

        // Quality: lookups, the smart home, games, reminders, the light.
        let reminders = Arc::new(RemindersSkill::new(StateStore::open(db.clone(), "reminders")));
        let led = Arc::new(LedSkill::new(content.join("animations")));
        // The panel's feeds, place and Home Assistant, read on every request.
        let place: PlaceSource = {
            let connectors = connectors.clone();
            Arc::new(move || connectors.weather_place())
        };
        let weather = Arc::new(WeatherSkill::new(config.weather.clone(), None).with_place(place));
        let feeds: FeedSource = {
            let connectors = connectors.clone();
            Arc::new(move || connectors.news_feeds())
        };
        let home: ProviderSource = Arc::new(move || {
            connectors.home_assistant().map(|home| {
                Arc::new(HomeAssistantProvider::new(&home.url, &home.token, None, None)) as Arc<dyn SmartHomeProvider>
            })
        });
        let host: GameHost = {
            let dialog = dialog.clone();
            Arc::new(move |key, system, text, cancel, fresh| dialog.roleplay(key, system, text, cancel, fresh))
        };
        skills.push(weather.clone());
        skills.push(Arc::new(NewsSkill::live(feeds, None)));
        skills.push(Arc::new(SmartHomeSkill::live(home)));
        skills.push(Arc::new(GamesSkill::new(Some(host), tts_markup, Some(model_on))));
        skills.push(reminders.clone());
        skills.push(led.clone());
        let calls = CallsSkill::new(calls.hub, calls.calls, calls.messenger);
        skills.push(calls.clone());
        let player_memory = Arc::new(PlayerMemory::new(StateStore::open(db.clone(), "player")));
        let timers = Arc::new(TimersSkill::new(StateStore::open(db.clone(), "timers")));
        let alarms = Arc::new(AlarmsSkill::new(StateStore::open(db, "alarms")));
        skills.push(Arc::new(VolumeSkill));
        skills.push(playback.clone());
        skills.push(Arc::new(BluetoothSkill));
        skills.push(Arc::new(ClockSkill));
        skills.push(timers.clone());
        skills.push(alarms.clone());
        skills.push(Arc::new(DoNotDisturbSkill));
        let router = Arc::new(Router::new(RouteTable::parse(&config.route_default, &config.routes), skills));

        let service = Arc::new(SkillsService {
            config,
            router,
            stats: Arc::new(SkillStats::new()),
            playback,
            timers,
            alarms,
            player,
            player_watch,
            media_sources,
            media,
            reminders,
            player_memory,
            led,
            calls,
            weather,
            settings,
            events,
            routes_default: AtomicBool::new(true),
            callbacks: RwLock::new(Vec::new()),
            tasks: Mutex::new(Vec::new()),
            speaker: RwLock::new(None),
            device_states: Mutex::new(HashMap::new()),
            radio_stopped: Mutex::new(HashMap::new()),
        });
        service.intercept();
        let (player, watch) = (service.player.clone(), service.player_watch.clone());
        service.on_callback(
            "media_",
            Arc::new(move |device, name, payload| {
                // Our retry of an item the station kept silent on: the item anew, through the gateway's way.
                if name == format!("{MEDIA_CALLBACK}{RETRY_CALLBACK}") {
                    let play_id = js::string_or_empty(payload.get(MEDIA_PLAY_ID));
                    let again = watch.retried(&device, &play_id);
                    return Box::pin(async move { again.map(Outcome::directives) });
                }
                let event = name.strip_prefix(&format!("{MEDIA_CALLBACK}on_")).and_then(PlayerEvent::parse);
                let play_id = js::string_or_empty(payload.get("play_id"));
                // The watch hears of the item the callback is about: after the player took a start of its queued
                // item, before an end moves it on to the next.
                if event.is_some_and(|event| event != PlayerEvent::Started) {
                    watch.on_event(&device, event.unwrap_or(PlayerEvent::Stopped), &play_id);
                }
                let next = event.and_then(|event| player.on_event(&device, event, &play_id, now_ms()));
                if event == Some(PlayerEvent::Started) {
                    watch.on_event(&device, PlayerEvent::Started, &play_id);
                }
                Box::pin(async move { next.map(Outcome::directives) })
            }),
        );
        service.init_routes();
        service
    }

    fn intercept(&self) {
        let (timers, alarms, calls) = (self.timers.clone(), self.alarms.clone(), self.calls.clone());
        self.router.intercept(Box::new(move |intent, input| while_ringing(&timers, &alarms, &calls, intent, input)));
        let memory = self.player_memory.clone();
        self.router.intercept(Box::new(move |intent, input| yandex_content(&memory, intent, input)));
        let media = self.media.clone();
        self.router.intercept(Box::new(move |intent, input| our_content(&media, intent, input)));
    }

    /// Routes live in the database: the panel's changes survive restarts. A fresh gateway seeds them from
    /// ROUTES/ROUTE_DEFAULT — after that the database wins and the env only names the default for resets.
    fn init_routes(&self) {
        if let Err(error) = self.settings.seed(&ROUTES, &self.router.routes().describe_by_name()) {
            tracing::warn!(target: "skills", "routes not seeded: {}", error.message);
        }
        self.load_routes();
        let routes = self.router.routes();
        for problem in &routes.problems {
            tracing::warn!(target: "skills", "{problem}");
        }
        let chat: Vec<&str> = routes.route(CapabilityName::Chat).iter().map(|step| step.as_str()).collect();
        let exceptions: Vec<String> = routes
            .describe()
            .into_iter()
            .filter(|(_, route)| *route != chat.join(","))
            .map(|(name, route)| format!("{name}={route}"))
            .collect();
        tracing::info!(
            target: "skills",
            "routes{}: {}{}",
            if self.routes_default.load(Ordering::SeqCst) { " (default)" } else { "" },
            self.config.route_default,
            if exceptions.is_empty() { String::new() } else { format!("; {}", exceptions.join(", ")) }
        );
    }

    /// The reminders' scheduler and the routes' listener: whoever writes the routes (the panel, the mode's one-time
    /// give-back, a future scenario), they apply at once. Inside the runtime.
    pub fn start(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        let listener = self.events.on("skills.routes", move |event| {
            if let GatewayEvent::SettingsChanged { key, user_id: None, .. } = event.as_ref() {
                if key == ROUTES.key {
                    if let Some(me) = me.upgrade() {
                        me.load_routes();
                    }
                }
            }
        });
        let reminders = self.reminders.start();
        self.tasks.lock().unwrap().extend([listener, reminders]);
    }

    /// The tasks stop: at shutdown.
    pub fn shutdown(&self) {
        self.reminders.stop();
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
    }

    /// The stored routes into the router; a stored map with problems is reported and left out.
    fn load_routes(&self) {
        let Some(stored) = self.settings.entry(&ROUTES) else { return };
        self.routes_default.store(stored.is_default, Ordering::SeqCst);
        let table = RouteTable::parse(&self.config.route_default, &serialize_routes(&stored.value));
        if !table.problems.is_empty() {
            for problem in &table.problems {
                tracing::warn!(target: "skills", "stored routes: {problem}");
            }
        } else if !stored.is_default {
            self.router.replace_routes(table);
        }
    }

    /// The routes as the gateway uses them now, and whether nobody has changed the default yet.
    pub fn routes_setting(&self) -> RoutesSetting {
        let stored = self.settings.entry(&ROUTES);
        RoutesSetting {
            routes: self.router.routes().describe(),
            is_default: stored.map(|stored| stored.is_default).unwrap_or(true),
        }
    }

    /// Rewrites the routes the gateway uses from now on and keeps them in the database — no restart. The map is
    /// whole (what is not in it goes back to ROUTE_DEFAULT); problems mean nothing was applied. A panel's value that
    /// is not a string is written as JavaScript would (`js::string_of`) before it comes here.
    pub fn replace_routes(
        &self,
        routes: &BTreeMap<String, String>,
        actor: Option<Actor>,
    ) -> Result<RoutesUpdate, ApiError> {
        let table = RouteTable::parse(&self.config.route_default, &serialize_routes(routes));
        if !table.problems.is_empty() {
            return Ok(RoutesUpdate { routes: self.router.routes().describe(), problems: table.problems });
        }
        let described = table.describe();
        let by_name = table.describe_by_name();
        self.router.replace_routes(table);
        self.routes_default.store(false, Ordering::SeqCst);
        self.settings.set(&ROUTES, &by_name, actor.clone())?;
        self.events.emit(GatewayEvent::RoutesChanged { routes: by_name, actor });
        tracing::info!(target: "skills", "routes: replaced from the panel, {} capabilities", described.len());
        Ok(RoutesUpdate { routes: described, problems: Vec::new() })
    }

    /// The station's state its request carried: kept for the panel; a new player item or other timers tell the
    /// panel's widgets to read again.
    pub fn note_device_state(&self, device: &str, state: Option<&DeviceState>, now: u64) {
        let Some(state) = state else { return };
        // Something plays there again: the radio we stopped is not what its «продолжить» is about any more.
        if state.player.as_ref().is_some_and(|player| player.state == PlayerState::Playing) {
            self.radio_stopped.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(device);
        }
        let changed = {
            let mut states = self.device_states.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let before = states.insert(device.to_string(), (state.clone(), now));
            let key = |state: &DeviceState| {
                (
                    state.player.as_ref().map(|player| (player.state, player.id.clone(), player.title.clone())),
                    state.timers.iter().map(|timer| (timer.id.clone(), timer.paused)).collect::<Vec<_>>(),
                )
            };
            before.is_none_or(|(old, _)| key(&old) != key(state))
        };
        if changed {
            self.events.emit(GatewayEvent::StationStateChanged { device: device.to_string(), what: "station".into() });
        }
    }

    /// The station's state its last request carried, and when.
    pub fn device_state(&self, device: &str) -> Option<(DeviceState, u64)> {
        self.device_states.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(device).cloned()
    }

    /// The station's last state of any device, when the panel names none: the newest.
    fn latest_state(&self) -> Option<(String, DeviceState, u64)> {
        let states = self.device_states.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        states.iter().max_by_key(|(_, (_, at))| *at).map(|(device, (state, at))| (device.clone(), state.clone(), *at))
    }

    /// The device the panel means: the one it names, else the one heard from last.
    pub fn device_or_latest(&self, device: Option<&str>) -> Option<String> {
        device.filter(|device| !device.is_empty()).map(str::to_string).or_else(|| self.latest_state().map(|(device, _, _)| device))
    }

    /// The device the panel's widgets mean: as `device_or_latest`, else the station on the line now (`live`), else
    /// one with timers of ours. The states the requests carried live in memory: after a restart they are gone until
    /// the station's next request, the timers kept on disk are not.
    pub fn device_for_panel(
        &self,
        device: Option<&str>,
        live: impl FnOnce() -> Option<String>,
        now: u64,
    ) -> Option<String> {
        self.device_or_latest(device).or_else(live).or_else(|| self.timers.devices(now).into_iter().next())
    }

    /// Timers of a device for the panel: ours with their names and the station's own.
    pub fn timer_views(&self, device: &str, now: u64) -> Vec<TimerView> {
        let state = self.device_state(device);
        self.timers.views(device, state.as_ref().map(|(state, at)| (state, *at)), now)
    }

    /// What plays on a device: our player's item while it plays or stands paused by us, else the station's word.
    pub fn now_playing(&self, device: &str, now: u64) -> NowPlaying {
        use super::media::player::PlayState;
        let station = self.device_state(device);
        // When Yandex's player was last heard of (its callbacks, kept in the player memory).
        let yandex_since = self
            .player_memory
            .get(device)
            .filter(|record| record.source == Some(ContentSource::Yandex))
            .and_then(|record| DateTime::parse_from_rfc3339(&record.at).ok())
            .map(|at| at.timestamp_millis().max(0) as u64);
        if let Some((item, player)) = self.player.current(device) {
            let state = match player.state {
                PlayState::Starting | PlayState::Playing => "playing",
                PlayState::Stopped if player.stop_asked_at.is_some() => "paused",
                PlayState::Stopped | PlayState::Finished | PlayState::Failed => "stopped",
            };
            // The station's later word wins over a stopped item of ours: Yandex plays something else now. So does
            // Yandex's player heard after ours stopped or paused — «продолжи» goes to Yandex then, not to ours.
            let stale = state == "stopped" && station.as_ref().is_some_and(|(_, at)| *at > player.at);
            let superseded = state != "playing" && yandex_since.is_some_and(|since| since > player.at);
            if !stale && !superseded {
                return NowPlaying {
                    source: "own",
                    state,
                    title: Some(item.title.clone()),
                    subtitle: item.subtitle.clone(),
                    image_url: item.image_url.clone(),
                    kind: Some(item.r#type.clone()),
                    duration_ms: item.duration_ms,
                    // Paused, it stands where it was paused, not where this play began.
                    position_ms: Some(player.position_ms(now)),
                    since: player.started_at,
                };
            }
        }
        let Some((state, at)) = station else {
            return NowPlaying { source: "none", state: "idle", title: None, subtitle: None, image_url: None, kind: None, duration_ms: None, position_ms: None, since: None };
        };
        let Some(player) = state.player else {
            return NowPlaying { source: "none", state: "idle", title: None, subtitle: None, image_url: None, kind: None, duration_ms: None, position_ms: None, since: None };
        };
        use crate::gateway::protocol::protocol_types::PlayerState as StationPlayer;
        let playing = player.state == StationPlayer::Playing;
        let offset = player.offset_ms.unwrap_or(0).max(0) as u64;
        NowPlaying {
            source: if player.title.is_some() { "station" } else { "none" },
            state: match player.state {
                StationPlayer::Playing => "playing",
                StationPlayer::Stopped => "stopped",
                StationPlayer::Idle | StationPlayer::Unknown => "idle",
            },
            title: player.title,
            subtitle: None,
            image_url: None,
            kind: player.r#type,
            duration_ms: None,
            position_ms: Some(if playing { offset + now.saturating_sub(at) } else { offset }),
            since: None,
        }
    }

    /// The station's player commanded from the panel: pause, resume, next, previous, stop, like — as the voice
    /// commands do it, through the router's interceptors for what plays. What Yandex played is Yandex's to control
    /// while it is up: the voice command goes to it, the panel cannot send it there, so it is refused with the words
    /// to say (our commands do nothing to Yandex's queue, and our old radio must not start in place of its music).
    /// What our player plays is ours: next and previous of its own items, «продолжить» where it stood. The
    /// directives to push to the station.
    pub async fn player_command(
        &self,
        device: &str,
        action: &str,
        yandex_available: bool,
    ) -> Result<Vec<crate::gateway::protocol::protocol_types::Directive>, ApiError> {
        // Live radio has no pause: the station's player_pause alone left it playing (the owner, 10.11), while next and
        // previous worked. It is stopped as Yandex stops it — player_pause and clear_queue, ours marked stopping — and
        // «продолжить» starts ours again. The panel sends «stop» for it; a «pause» is taken the same.
        let now = now_ms();
        let playing = self.now_playing(device, now);
        let radio = playing.kind.as_deref() == Some("FmRadio");
        let action = if action == "pause" && radio { "stop" } else { action };
        let mut intent = Intent::new(CapabilityName::Playback, action);
        if action == "like" || action == "dislike" {
            intent.params.insert("ours".into(), Value::Bool(true));
        }
        // The interceptors of what plays, as the router has them after `while_ringing` — that one is the voice's: a
        // «стоп» said while a timer rings is for the timer, a button of the player is for the player. No station state
        // here (between requests it is old): our player's own count stands in for its word.
        let input = RouteInput { device_tag: device.to_string(), yandex_available, now, ..RouteInput::default() };
        let intent = yandex_content(&self.player_memory, &intent, &input)
            .or_else(|| our_content(&self.media, &intent, &input))
            .unwrap_or(intent);
        // The station's own radio we stopped: its queue went with the station, its player_continue brings nothing
        // back — and Yandex's «продолжи» starts music instead. A pause the station made itself goes on as it was.
        if action == "resume" && !js::is_true(&intent.params, "ours") && self.stopped_radio(device) {
            return Err(ApiError::conflict(STATION_RADIO_STOPPED));
        }
        if yandex_available && js::is_true(&intent.params, "external") {
            return Err(ApiError::conflict(yandex_plays(action)));
        }
        let context = super::skill::ExecutionContext::new(device, Utc::now());
        let outcome =
            self.playback.execute(&intent, &context).await.map_err(|error| ApiError::bad_request(error.to_string()))?;
        if action == "stop" && radio && playing.source == "station" {
            self.radio_stopped.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(device.to_string(), now);
        }
        Ok(outcome.directives)
    }

    /// Whether the station's own radio on the device was stopped by us and nothing played there since: neither the
    /// station (`note_device_state`) nor our player.
    fn stopped_radio(&self, device: &str) -> bool {
        let mut stopped = self.radio_stopped.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(&at) = stopped.get(device) else { return false };
        let ours_since =
            self.player.current(device).is_some_and(|(_, player)| player.started_at.unwrap_or(player.at) > at);
        if ours_since {
            stopped.remove(device);
        }
        !ours_since
    }

    /// Whether a timer or an alarm of the station rings now.
    pub fn ringing_now(&self, device: &str, state: Option<&DeviceState>, now: u64) -> bool {
        !self.timers.ringing(device, state, now).is_empty() || self.alarms.ringing(device, state, at(now)).is_some()
    }

    /// A station connected: what its first requests will need is looked up now (the weather's city).
    pub fn prepare(&self, timezone: Option<&str>) {
        self.weather.prepare(timezone);
    }

    /// How our reminders and calls are said: the session registry knows the live station of a device.
    pub fn attach_speaker(&self, speaker: Arc<dyn Speaker>) {
        *self.speaker.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(speaker.clone());
        self.reminders.attach_speaker(speaker.clone());
        self.calls.attach_speaker(speaker);
    }

    /// The speaker the station link gave, when it stands.
    pub fn speaker(&self) -> Option<Arc<dyn Speaker>> {
        self.speaker.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    pub fn register(&self, skill: Arc<dyn OwnSkill>) {
        self.router.register(skill);
    }

    pub fn skill(&self, capability: CapabilityName) -> Option<Arc<dyn OwnSkill>> {
        self.router.skill(capability)
    }

    /// Our callbacks by name prefix: `media_` for the player (`vgw_media_…`).
    pub fn on_callback(&self, prefix: &str, handler: CallbackHandler) {
        let prefix = format!("{OUR_CALLBACK_PREFIX}{prefix}");
        let mut callbacks = self.callbacks.write().unwrap();
        match callbacks.iter_mut().find(|(known, _)| *known == prefix) {
            Some(entry) => entry.1 = handler,
            None => callbacks.push((prefix, handler)),
        }
    }

    /// Our handler of a callback, if it is ours and someone takes it. The request for the next track has Yandex's
    /// name: ours is the one with our play id.
    pub fn callback_handler(&self, name: &str, payload: Option<&Map<String, Value>>) -> Option<CallbackHandler> {
        if name == STACK_GET_NEXT
            && payload.and_then(|payload| payload.get(MEDIA_PLAY_ID)).is_some_and(Value::is_string)
        {
            let player = self.player.clone();
            return Some(Arc::new(move |device, _name, body| {
                let next = player.next(&device, &js::string_or_empty(body.get(MEDIA_PLAY_ID)));
                Box::pin(async move { next.map(Outcome::directives) })
            }));
        }
        if !name.starts_with(OUR_CALLBACK_PREFIX) {
            return None;
        }
        let callbacks = self.callbacks.read().unwrap();
        callbacks.iter().find(|(prefix, _)| name.starts_with(prefix.as_str())).map(|(_, handler)| handler.clone())
    }

    /// The intent of a model's tool call, or why the call cannot be one (said back to the model).
    pub fn intent_of(&self, call: &ToolCall) -> Result<Intent, String> {
        tools::intent_of(call)
    }

    /// Implemented capabilities and whether they are set up: for /health.
    pub fn implemented(&self) -> BTreeMap<CapabilityName, bool> {
        self.router.skills().into_iter().map(|skill| (skill.capability(), skill.available())).collect()
    }

    /// Where the station fetches our /media: MEDIA_BASE_URL, or the host it reached us at for /uni.ws.
    pub fn media_base_url(&self, host: Option<&str>) -> Option<String> {
        match &self.config.media_base_url {
            Some(base) => Some(format!("{base}/media")),
            None => host.map(|host| format!("http://{host}/media")),
        }
    }

    /// The content folder, for /media.
    pub fn content_dir(&self) -> &Path {
        &self.config.content_dir
    }
}

fn at(ms: u64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms as i64).unwrap_or_default()
}

/// "Стоп" while a timer or an alarm rings stops it, not the player; so does «выключи будильник» while one rings.
/// The station says which alarm rings; timers it lists that are not ours (Yandex set them) go to Yandex, which knows
/// whether they ring; so do their status and cancel. Live (02.11): «стоп» to a ringing alarm was our silent stop, the
/// alarm went on.
fn while_ringing(
    timers: &TimersSkill,
    alarms: &AlarmsSkill,
    calls: &CallsSkill,
    intent: &Intent,
    input: &RouteInput,
) -> Option<Intent> {
    let device = input.device_tag.as_str();
    let state = input.device_state.as_ref();
    let theirs: Vec<String> = state
        .map(|state| state.timer_ids.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|id| !timers.knows(device, id))
        .collect();
    let alarm = alarms.ringing(device, state, at(input.now));
    let stop_alarm = |alarm: &str| {
        let mut stop = Intent::new(CapabilityName::Alarms, "stop");
        stop.params.insert("alarmId".into(), alarm.into());
        stop
    };
    if let Some(alarm) = &alarm {
        if intent.capability == CapabilityName::Alarms
            && intent.action == "cancel"
            && !intent.params.contains_key("hours")
        {
            return Some(stop_alarm(alarm));
        }
    }
    if intent.capability == CapabilityName::Playback && intent.action == "stop" {
        if let Some(alarm) = &alarm {
            return Some(stop_alarm(alarm));
        }
        let ours = timers.ringing(device, state, input.now);
        let stop_timers = |ids: Vec<String>, external: bool| {
            let mut stop = Intent::new(CapabilityName::Timers, "stop");
            stop.params.insert("timerIds".into(), ids.into());
            if external {
                stop.params.insert("external".into(), true.into());
            }
            stop
        };
        if !ours.is_empty() {
            return Some(stop_timers(ours, false));
        }
        if !theirs.is_empty() {
            return Some(stop_timers(theirs, true));
        }
        // The player is not playing: the stop interrupts speech, which the station has cut already.
        if state.is_some_and(quiet) {
            return Some(Intent::new(CapabilityName::Playback, "silence"));
        }
    }
    // Calls are ours only when our hub or our contacts have them; the rest is Yandex's own calling.
    if intent.capability == CapabilityName::Calls {
        return Some(with_param(intent, "ours", calls.ours(intent, device).into()));
    }
    if intent.capability == CapabilityName::Timers
        && ["status", "cancel", "pause", "resume"].contains(&intent.action.as_str())
        && !theirs.is_empty()
        && timers.active(device, input.now).is_empty()
    {
        return Some(with_param(intent, "external", true.into()));
    }
    None
}

/// Controlling what Yandex played (its FM radio, its music) is Yandex's: it holds the queue and the context. Live:
/// after our «стоп» (pause and clear the queue) «продолжи», «дальше», «назад» of ours did nothing; sent to Yandex,
/// «продолжи» started music instead of the FM radio, as Yandex only heard the station stop. So its stop goes to
/// Yandex too, while it plays; a stop with nothing playing stays our silent one.
fn yandex_content(memory: &PlayerMemory, intent: &Intent, input: &RouteInput) -> Option<Intent> {
    if intent.capability != CapabilityName::Playback {
        return None;
    }
    let player = memory.get(&input.device_tag).filter(|player| player.source == Some(ContentSource::Yandex))?;
    // A like is Yandex's too: its music is liked in the account (live 03.11 «мне нравится» under its radio went to
    // the model as a remark, and nothing was liked).
    let control = ["resume", "next", "previous", "replay", "like", "dislike"].contains(&intent.action.as_str());
    let stop = ["stop", "pause"].contains(&intent.action.as_str()) && player.playing;
    (control || stop).then(|| with_param(intent, "external", true.into()))
}

/// Why the panel's button does nothing to what Yandex plays: the words that do it, said to the station.
fn yandex_plays(action: &str) -> String {
    let words = match action {
        "pause" => "пауза",
        "stop" => "стоп",
        "resume" => "продолжи",
        "next" => "дальше",
        "previous" => "назад",
        "replay" => "сначала",
        "like" => "мне нравится",
        "dislike" => "не нравится",
        _ => return "Это играет Яндекс: им управляет он — скажите колонке голосом.".into(),
    };
    format!("Это играет Яндекс: им управляет он — скажите колонке «Алиса, {words}».")
}

/// Controlling what our player plays is ours, whatever the route says of Yandex: only we hold the queue and know the
/// track. Live 03.11 (music ours, the player's route Yandex's): «следующий трек» under our music went to Yandex,
/// which started a song of its own.
fn our_content(media: &MediaControl, intent: &Intent, input: &RouteInput) -> Option<Intent> {
    if intent.capability != CapabilityName::Playback
        || !["resume", "next", "previous", "replay", "like", "dislike"].contains(&intent.action.as_str())
    {
        return None;
    }
    media.owns(&input.device_tag).then(|| with_param(intent, "ours", true.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::gateway::protocol::protocol_types::{PlayerView, StationAlarm};
    use crate::skills::router::{Decision, ModelState, Resolution};
    use crate::skills::skill::ExecutionContext;
    use futures::StreamExt;
    use serde_json::json;

    struct Model;

    impl DialogAccess for Model {
        fn enabled(&self) -> bool {
            true
        }
        fn narrate(&self, instructions: String, _cancel: CancellationToken) -> PhraseStream {
            futures::stream::iter([Ok(instructions)]).boxed()
        }
        fn roleplay(&self, _key: String, _system: String, text: String, _: CancellationToken, _: bool) -> PhraseStream {
            futures::stream::iter([Ok(format!("ход: {text}"))]).boxed()
        }
    }

    struct Built {
        service: Arc<SkillsService>,
        db: Arc<Db>,
        events: EventBus,
        _content: tempfile::TempDir,
    }

    fn build_on(db: Arc<Db>, env: &[(&str, &str)]) -> Built {
        build_with(db, env, Arc::new(NoConnectors))
    }

    /// Home Assistant set up in the panel.
    struct WithHome;

    impl SkillConnectors for WithHome {
        fn news_feeds(&self) -> Vec<NewsFeed> {
            Vec::new()
        }

        fn weather_place(&self) -> Option<String> {
            None
        }

        fn home_assistant(&self) -> Option<HomeAssistantConfig> {
            Some(HomeAssistantConfig { url: "http://ha.local".into(), token: "t".into() })
        }
    }

    fn build_with(db: Arc<Db>, env: &[(&str, &str)], connectors: Arc<dyn SkillConnectors>) -> Built {
        let content = tempfile::tempdir().unwrap();
        let mut pairs: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        pairs.push(("CONTENT_DIR".into(), content.path().display().to_string()));
        let config = Config::for_tests(&Env::from_pairs(pairs));
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let service = SkillsService::new(SkillsDeps {
            config: config.skills,
            tts_markup: false,
            db: db.clone(),
            settings,
            events: events.clone(),
            dialog: Arc::new(Model),
            calls: CallsLinks::none(),
            connectors,
        });
        Built { service, db, events, _content: content }
    }

    fn build(env: &[(&str, &str)]) -> Built {
        build_on(Arc::new(Db::memory()), env)
    }

    fn input(text: &str) -> RouteInput {
        RouteInput {
            text: text.into(),
            device_tag: "kitchen".into(),
            yandex_available: true,
            model: ModelState { enabled: true, tools: true },
            now: now_ms(),
            ..RouteInput::default()
        }
    }

    fn intent_of(decision: Decision) -> Intent {
        match decision {
            Decision::Intent { intent, .. } => intent,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn seeds_the_routes_once_and_keeps_the_panels_across_a_restart() {
        let built = build(&[("ROUTES", "radio=yandex")]);
        let setting = built.service.routes_setting();
        assert!(setting.is_default);
        assert_eq!(setting.routes[&CapabilityName::Radio], "yandex");
        let mut routes = BTreeMap::new();
        routes.insert("timers".to_string(), "own".to_string());
        routes.insert("calls".to_string(), "off".to_string());
        let mut changes = built.events.subscribe();
        let update = built.service.replace_routes(&routes, None).unwrap();
        assert!(update.problems.is_empty());
        assert_eq!(update.routes[&CapabilityName::Calls], "off");
        // What is not in the map goes back to ROUTE_DEFAULT.
        assert_eq!(update.routes[&CapabilityName::Radio], "own,yandex");
        let mut names = Vec::new();
        while let Ok(event) = changes.try_recv() {
            names.push(event.name());
        }
        assert_eq!(names, ["settings.changed", "routes.changed"]);

        let restarted = build_on(built.db.clone(), &[("ROUTES", "radio=yandex")]);
        let setting = restarted.service.routes_setting();
        assert!(!setting.is_default);
        assert_eq!(setting.routes[&CapabilityName::Calls], "off");
        assert_eq!(setting.routes[&CapabilityName::Radio], "own,yandex");

        let mut broken = BTreeMap::new();
        broken.insert("radio".to_string(), "everywhere".to_string());
        let refused = restarted.service.replace_routes(&broken, None).unwrap();
        assert_eq!(refused.problems, ["ROUTES radio: \"everywhere\" is not own, yandex or off"]);
        assert_eq!(refused.routes[&CapabilityName::Calls], "off");
    }

    #[tokio::test]
    async fn applies_routes_written_by_anyone_at_once() {
        let built = build(&[]);
        built.service.start();
        let settings = SettingsService::new(built.db.clone(), built.events.clone());
        let mut routes = built.service.router.routes().describe_by_name();
        routes.insert("music".into(), "yandex".into());
        settings.set(&ROUTES, &routes, None).unwrap();
        for _ in 0..100 {
            if built.service.router.routes().describe()[&CapabilityName::Music] == "yandex" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(built.service.router.routes().describe()[&CapabilityName::Music], "yandex");
        built.service.shutdown();
    }

    #[tokio::test]
    async fn stops_what_rings_not_the_player() {
        let built = build(&[]);
        let skills = &built.service;
        let mut context = ExecutionContext::new("kitchen", Utc::now() - chrono::Duration::seconds(70));
        let set = Intent {
            params: json!({"seconds": 60}).as_object().cloned().unwrap(),
            ..Intent::new(CapabilityName::Timers, "set")
        };
        skills.timers.execute(&set, &context).await.unwrap();
        // Our timer is due: «стоп» stops it.
        let stop = intent_of(skills.router.decide(&input("стоп")));
        assert_eq!((stop.capability, stop.action.as_str()), (CapabilityName::Timers, "stop"));
        assert!(skills.ringing_now("kitchen", None, now_ms()));
        // A timer the station lists that is not ours: Yandex's to stop.
        let theirs = RouteInput {
            device_tag: "hall".into(),
            device_state: Some(DeviceState { timer_ids: vec!["yandex-1".into()], ..DeviceState::default() }),
            ..input("стоп")
        };
        let stop = intent_of(skills.router.decide(&theirs));
        assert_eq!(serde_json::to_value(&stop.params).unwrap(), json!({"timerIds": ["yandex-1"], "external": true}));
        let status = intent_of(skills.router.decide(&RouteInput { text: "сколько осталось".into(), ..theirs.clone() }));
        assert_eq!(status.params.get("external"), Some(&Value::Bool(true)));
        // An alarm the station says rings: «выключи будильник» stops it.
        let alarm = StationAlarm {
            id: "a1".into(),
            hours: 7,
            minutes: 0,
            seconds: 0,
            date: None,
            timezone: None,
            offset_seconds: None,
            edit_timestamp: None,
            ringing: true,
            rung: false,
        };
        let ringing = RouteInput {
            device_tag: "hall".into(),
            device_state: Some(DeviceState { alarms: Some(vec![alarm]), ..DeviceState::default() }),
            ..input("выключи будильник")
        };
        let stop = intent_of(skills.router.decide(&ringing));
        assert_eq!((stop.action.as_str(), stop.params.get("alarmId")), ("stop", Some(&json!("a1"))));
        // Nothing rings and nothing plays: a silent stop of the speech.
        context.device_tag = "office".into();
        let idle = RouteInput {
            device_tag: "office".into(),
            device_state: Some(DeviceState {
                player: Some(PlayerView {
                    state: PlayerState::Idle,
                    id: None,
                    title: None,
                    r#type: None,
                    offset_ms: None,
                }),
                ..DeviceState::default()
            }),
            ..input("стоп")
        };
        assert_eq!(intent_of(skills.router.decide(&idle)).action, "silence");
        // Calls without our hub are Yandex's.
        let call = intent_of(skills.router.decide(&input("позвони маме")));
        assert_eq!(call.params.get("ours"), Some(&Value::Bool(false)));
    }

    /// Live 06.11 in «Свой»: «стоп» after Yandex's radio, the player's route Yandex's only, answered «Этого я
    /// пока не умею». Without Yandex the stop is ours: the commands Yandex would send.
    #[tokio::test]
    async fn stops_the_player_itself_without_yandex() {
        let built = build(&[("ROUTES", "playback=yandex")]);
        let skills = &built.service;
        skills.player_memory.note_callback("kitchen", "music_thin_client_on_started", Utc::now());
        let playing = |text: &str, yandex_available: bool| RouteInput {
            device_state: Some(DeviceState {
                player: Some(PlayerView {
                    state: PlayerState::Playing,
                    id: None,
                    title: None,
                    r#type: None,
                    offset_ms: None,
                }),
                ..DeviceState::default()
            }),
            yandex_available,
            ..input(text)
        };
        for text in ["стоп", "пауза"] {
            match skills.router.decide(&playing(text, false)) {
                Decision::Intent { intent, resolution: Resolution::Own(skill), .. } => {
                    let outcome = skill.execute(&intent, &ExecutionContext::new("kitchen", Utc::now())).await.unwrap();
                    assert_eq!(outcome.directives[0].name, "player_pause", "{text}");
                    assert!(outcome.speech.is_none(), "{text}: no «не умею»");
                }
                other => panic!("{text}: {other:?}"),
            }
        }
        // With Yandex up the route stands: Yandex holds the queue of what it played.
        assert!(matches!(
            skills.router.decide(&playing("стоп", true)),
            Decision::Intent { resolution: Resolution::Yandex, .. }
        ));
    }

    #[tokio::test]
    async fn gives_the_control_of_yandexs_music_to_yandex_and_of_ours_to_us() {
        let built = build(&[]);
        let skills = &built.service;
        skills.player_memory.note_callback("kitchen", "music_thin_client_on_started", Utc::now());
        let next = intent_of(skills.router.decide(&input("дальше")));
        assert_eq!(next.params.get("external"), Some(&Value::Bool(true)));
        let directives = skills.player.play(
            "office",
            vec![crate::skills::media::media_types::MediaItem {
                id: "music/a.mp3".into(),
                title: "a".into(),
                url: "http://gw/media/music/a.mp3".into(),
                format: "MP3".into(),
                codec: None,
                r#type: "Track".into(),
                subtitle: None,
                image_url: None,
                duration_ms: None,
            }],
            0,
            0,
        );
        assert_eq!(directives[0].name, "audio_play");
        let ours = intent_of(skills.router.decide(&RouteInput { device_tag: "office".into(), ..input("дальше") }));
        assert_eq!(ours.params.get("ours"), Some(&Value::Bool(true)));
        let resolution = skills.router.resolve(&ours, true, None);
        assert_eq!(resolution.kind(), "own");
    }

    fn media(id: &str, kind: &str, duration_ms: Option<u64>) -> crate::skills::media::media_types::MediaItem {
        crate::skills::media::media_types::MediaItem {
            id: id.into(),
            title: id.into(),
            url: format!("http://gw/media/{id}"),
            format: "MP3".into(),
            codec: None,
            r#type: kind.into(),
            subtitle: None,
            image_url: None,
            duration_ms,
        }
    }

    /// Review G4: the panel's buttons go through the interceptors the voice commands do. Yandex's music after our old
    /// radio is Yandex's to control: our commands do nothing to its queue and «▶» must not start our radio in its
    /// place — the panel is told the words instead, and its widget does not show our radio.
    #[tokio::test]
    async fn the_panels_player_leaves_what_yandex_plays_to_yandex() {
        let built = build(&[]);
        let skills = &built.service;
        let now = now_ms();
        skills.player.play("kitchen", vec![media("studio21", "FmRadio", None)], 0, 0);
        let play = skills.player.current("kitchen").unwrap().1.play_id;
        skills.player.on_event("kitchen", PlayerEvent::Started, &play, now - 120_000);
        skills.player.on_event("kitchen", PlayerEvent::Stopped, &play, now - 60_000);
        skills.player_memory.note_callback("kitchen", "music_thin_client_on_started", Utc::now());
        assert_eq!(skills.now_playing("kitchen", now).source, "none", "not our old radio");
        for (action, words) in
            [("resume", "«Алиса, продолжи»"), ("next", "«Алиса, дальше»"), ("pause", "«Алиса, пауза»")]
        {
            let refused = skills.player_command("kitchen", action, true).await.unwrap_err();
            assert_eq!(refused.status.as_u16(), 409, "{action}");
            assert!(refused.message.contains(words), "{action}: {}", refused.message);
        }
        // Without Yandex the voice command is ours, and so is the panel's.
        let paused = skills.player_command("kitchen", "pause", false).await.unwrap();
        assert!(paused.iter().any(|directive| directive.name == "player_pause"));
    }

    /// Review G5: our track paused from the panel stands where it was paused, and goes on from there — the panel's
    /// «продолжить» has no word from the station, our count stands in for it.
    #[tokio::test]
    async fn the_panels_player_pauses_our_track_and_goes_on_where_it_stood() {
        let built = build(&[]);
        let skills = &built.service;
        let now = now_ms();
        skills.player.play("kitchen", vec![media("music/a.mp3", "Track", Some(240_000))], 0, 0);
        let play = skills.player.current("kitchen").unwrap().1.play_id;
        skills.player_memory.note_callback("kitchen", "vgw_media_on_started", Utc::now());
        skills.player.on_event("kitchen", PlayerEvent::Started, &play, now - 130_000);
        let paused = skills.player_command("kitchen", "pause", true).await.unwrap();
        assert_eq!(paused[0].name, "player_pause");
        skills.player.on_event("kitchen", PlayerEvent::Stopped, &play, now);
        let shown = skills.now_playing("kitchen", now + 600_000);
        assert_eq!((shown.source, shown.state, shown.position_ms), ("own", "paused", Some(130_000)));
        let resumed = skills.player_command("kitchen", "resume", true).await.unwrap();
        assert_eq!(resumed[0].name, "audio_play");
        assert_eq!(resumed[0].payload.as_ref().unwrap()["stream"]["offset_ms"], 130_000);
    }

    /// The owner, 10.11: live radio played on through the panel's pause, while «дальше» and «назад» worked. A pause
    /// of live radio stops it as Yandex does — player_pause and clear_queue, ours marked stopping — and «продолжить»
    /// brings ours back: it plays the station again. The station's own radio lost its station with the queue, and its
    /// player_continue brings nothing back (live 03.11, review owner-4): «продолжить» says the words that do, until
    /// something plays there again; a pause the station made itself goes on with player_continue.
    #[tokio::test]
    async fn the_panels_pause_stops_live_radio_ours_comes_back_the_stations_own_is_asked_for_in_words() {
        let built = build(&[]);
        let skills = &built.service;
        let now = now_ms();
        let names = |directives: &[crate::gateway::protocol::protocol_types::Directive]| {
            directives.iter().map(|directive| directive.name.clone()).collect::<Vec<_>>()
        };

        // Ours.
        skills.player.play("kitchen", vec![media("radio/hit", "FmRadio", None)], 0, 0);
        let play = skills.player.current("kitchen").unwrap().1.play_id;
        skills.player_memory.note_callback("kitchen", "vgw_media_on_started", Utc::now());
        skills.player.on_event("kitchen", PlayerEvent::Started, &play, now - 60_000);
        let paused = skills.player_command("kitchen", "pause", true).await.unwrap();
        assert_eq!(names(&paused), ["player_pause", "clear_queue"]);
        assert!(skills.player.current("kitchen").unwrap().1.stop_asked_at.is_some(), "ours: its stop is ours");
        skills.player.on_event("kitchen", PlayerEvent::Stopped, &play, now);
        assert_eq!(skills.now_playing("kitchen", now + 1000).state, "paused");
        let resumed = skills.player_command("kitchen", "resume", true).await.unwrap();
        assert_eq!(resumed[0].name, "audio_play");
        assert_eq!(resumed[0].payload.as_ref().unwrap()["stream"]["id"], "radio/hit");

        // The station's own, as its state says.
        let radio = DeviceState {
            player: Some(PlayerView {
                state: PlayerState::Playing,
                id: Some("europa".into()),
                title: Some("Европа Плюс".into()),
                r#type: Some("FmRadio".into()),
                offset_ms: None,
            }),
            ..DeviceState::default()
        };
        skills.note_device_state("hall", Some(&radio), now);
        let paused = skills.player_command("hall", "pause", false).await.unwrap();
        assert_eq!(names(&paused), ["player_pause", "clear_queue"]);
        let stopped = DeviceState {
            player: Some(PlayerView { state: PlayerState::Stopped, ..radio.player.clone().unwrap() }),
            ..DeviceState::default()
        };
        skills.note_device_state("hall", Some(&stopped), now + 1000);
        for yandex in [false, true] {
            let refused = skills.player_command("hall", "resume", yandex).await.unwrap_err();
            assert_eq!(refused.status, axum::http::StatusCode::CONFLICT);
            assert_eq!(refused.message, STATION_RADIO_STOPPED, "Yandex up: {yandex}");
        }
        // It plays again (asked of the station), then the station pauses it itself: its «продолжить» goes on.
        skills.note_device_state("hall", Some(&radio), now + 60_000);
        skills.note_device_state("hall", Some(&stopped), now + 120_000);
        let resumed = skills.player_command("hall", "resume", false).await.unwrap();
        assert_eq!(names(&resumed), ["player_continue"]);

        // A track still pauses.
        let track = DeviceState {
            player: Some(PlayerView { r#type: Some("Track".into()), ..radio.player.clone().unwrap() }),
            ..DeviceState::default()
        };
        skills.note_device_state("office", Some(&track), now);
        assert_eq!(names(&skills.player_command("office", "pause", false).await.unwrap()), ["player_pause"]);
    }

    /// Review G8: after a restart the states the requests carried are gone; the widgets still find the station — the
    /// one on the line, else the one with timers of ours on disk.
    #[tokio::test]
    async fn the_widgets_find_the_station_after_a_restart_before_its_next_request() {
        let built = build(&[]);
        let skills = &built.service;
        let set = Intent {
            params: json!({"seconds": 600}).as_object().cloned().unwrap(),
            ..Intent::new(CapabilityName::Timers, "set")
        };
        skills.timers.execute(&set, &ExecutionContext::new("kitchen", Utc::now())).await.unwrap();
        let restarted = build_on(built.db.clone(), &[]);
        let skills = &restarted.service;
        let now = now_ms();
        assert_eq!(skills.device_for_panel(None, || None, now).as_deref(), Some("kitchen"), "its timers on disk");
        assert_eq!(skills.timer_views("kitchen", now).len(), 1);
        assert_eq!(skills.device_for_panel(None, || Some("hall".into()), now).as_deref(), Some("hall"), "on the line");
        skills.note_device_state("office", Some(&DeviceState::default()), now);
        assert_eq!(skills.device_for_panel(None, || Some("hall".into()), now).as_deref(), Some("office"), "heard last");
        assert_eq!(skills.device_for_panel(Some("hall"), || None, now).as_deref(), Some("hall"), "named");
    }

    #[tokio::test]
    async fn takes_our_callbacks_and_the_stations_request_for_the_next_track() {
        let built = build(&[]);
        let skills = &built.service;
        let item = |id: &str| crate::skills::media::media_types::MediaItem {
            id: id.into(),
            title: id.into(),
            url: format!("http://gw/media/{id}.mp3"),
            format: "MP3".into(),
            codec: None,
            r#type: "Track".into(),
            subtitle: None,
            image_url: None,
            duration_ms: None,
        };
        let directives = skills.player.play("kitchen", vec![item("a"), item("b")], 0, 0);
        let play_id = directives[1].payload.as_ref().unwrap()[MEDIA_PLAY_ID].as_str().unwrap().to_string();
        let started = skills.callback_handler("vgw_media_on_started", None).unwrap();
        let mut payload = Map::new();
        payload.insert("play_id".into(), play_id.clone().into());
        assert!(started("kitchen".into(), "vgw_media_on_started".into(), payload.clone()).await.is_none());
        let mut ask = Map::new();
        ask.insert(MEDIA_PLAY_ID.into(), play_id.clone().into());
        let next = skills.callback_handler(STACK_GET_NEXT, Some(&ask)).unwrap();
        let outcome = next("kitchen".into(), STACK_GET_NEXT.into(), ask.clone()).await.unwrap();
        assert_eq!(outcome.directives[0].payload.as_ref().unwrap()["stream"]["id"], "b");
        assert!(skills.callback_handler(STACK_GET_NEXT, None).is_none());
        assert!(skills.callback_handler("music_thin_client_on_started", None).is_none());
        assert!(skills.callback_handler("vgw_other", None).is_none());
    }

    #[tokio::test]
    async fn tells_what_is_implemented_and_makes_intents_of_tool_calls() {
        let built = build_with(Arc::new(Db::memory()), &[], Arc::new(WithHome));
        let skills = &built.service;
        let implemented = skills.implemented();
        assert_eq!(implemented.len(), 17);
        // Home Assistant is set up in the panel; no news feeds and no radio list in an empty content folder.
        assert!(implemented[&CapabilityName::SmartHome]);
        assert!(!implemented[&CapabilityName::News]);
        assert!(!implemented[&CapabilityName::Radio]);
        assert!(implemented[&CapabilityName::Chat]);
        let call = ToolCall {
            id: "1".into(),
            name: "set_timer".into(),
            input: json!({"seconds": 90}).as_object().cloned().unwrap(),
            error: None,
        };
        assert_eq!(skills.intent_of(&call).unwrap().params.get("seconds"), Some(&json!(90)));
        // A tale on the model: its instructions, told as phrases.
        let tale = Intent {
            params: json!({"topic": "ежика"}).as_object().cloned().unwrap(),
            ..Intent::new(CapabilityName::Tales, "tell")
        };
        let skill = skills.skill(CapabilityName::Tales).unwrap();
        let outcome = skill.execute(&tale, &ExecutionContext::new("kitchen", Utc::now())).await.unwrap();
        let phrases: Vec<String> = outcome.phrases.unwrap().map(|phrase| phrase.unwrap()).collect().await;
        assert_eq!(phrases, ["Расскажи детскую сказку про ежика. Минуты на три чтения вслух, с добрым концом."]);
        assert_eq!(skills.media_base_url(Some("192.168.1.5:3000")).as_deref(), Some("http://192.168.1.5:3000/media"));
    }
}
