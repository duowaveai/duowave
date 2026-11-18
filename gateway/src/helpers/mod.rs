//! Helpers (docs/plans/home-and-helpers.md, block 1): the muscles a gateway leans on — the computer at home now, one's
//! own server and the DuoWave cloud later — for the heavy work and the big storage. A helper has no memory, people,
//! characters or settings of its own: it gives abilities (hearing, voice, a model, …) by one contract, the same for
//! every helper.
//!
//! The household keeps them in one ordered list (the setting `helpers`, which travels with the settings package, and
//! each helper's key `helper:<id>:key`), and each ability goes to the first helper on the list that is switched on,
//! answers, declares it, is ready and is not kept from it — the hearing goes down the list within one request. The
//! regulator «Что может уходить из дома» (the setting `away`) is one rule per helper: a helper outside the home that it
//! keeps out gives nothing and is never looked at; a helper at home is never kept out. It also governs this gateway's
//! own cloud roads (the .env's cloud hearing, a cloud voice). A gateway knows itself in a list by the id its contract
//! names (`helperId`), and the same helper reached by two entries answers once.
//!
//! - contract.rs — the abilities, the contract's versions, and a helper's `/home/v1/status` read as one.
//! - remote.rs — a helper of the list as this gateway reaches it: the look at its status every 15 s, its bot's
//!   contacts, its clients for hearing and voice.
//! - door.rs — the pair's door: the entry `home-computer` as the install's .env and the computer's routes
//!   (`/api/v1/home-server*`) set it.
//! - models.rs — the helpers with a model, as «Мозг» reads them.
//! - builtin.rs (the computer's build) — the built-in helper: the computer's heavy part behind the same contract,
//!   which `/home/v1` answers through and a computer that is not in the station role takes its own parts from.
//! - now.rs — «Где сейчас»: who gives each ability now, and why not the first.
//! - registry.rs — what works where (core/availability.rs) as the helpers give it.
//! - routes.rs — `/api/v1/helpers*`.
//! - link.rs — the link a gateway opens to its helper: pairing, the pinned TLS, frames, the bridge (stage 7).
//! - link_serve.rs, clients.rs — the helper's side of the link and «Помогаю» (pc).

#[cfg(feature = "pc")]
pub mod clients;
pub mod contract;
pub mod door;
pub mod link;
#[cfg(feature = "pc")]
pub mod link_serve;
pub mod models;
pub mod now;
pub mod registry;
pub mod remote;
pub mod routes;

// The computer's build only: its built-in helper.
#[cfg(feature = "pc")]
pub mod builtin;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use self::contract::{Abilities, Ability, AbilityState, Contract, HelperIdentity, HelperStatus};
use self::link::{LinkNews, LinkState, LinkTiming, Paired};
use self::remote::RemoteHelper;
use crate::config::Config;
use crate::core::availability::HomeServer;
use crate::core::build::Profile;
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::core::identity::home::{clean_name, Identity};
use crate::gateway::mode::ModeService;
use crate::operations::networks::is_local_url;
use crate::platform::http::ApiError;
use crate::secrets::{SecretMask, SecretsService};
use crate::settings::definitions::{HelperPin, PanelMode, AWAY as AWAY_SETTING, HELPERS, HELPER_PINS};
use crate::settings::SettingsService;
use crate::speech::asr::speech_recognizer::SpeechRecognizer;
use crate::speech::tts::own_speech::OwnSpeech;

/// Where a helper is, as a person says it: «дома», «мой сервер», «Облако DuoWave».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HelperKind {
    Home,
    Own,
    Cloud,
}

impl HelperKind {
    /// How far its road goes by its kind alone: home — at home; one's own server and DuoWave's cloud — mine.
    pub fn reach(self) -> Reach {
        match self {
            HelperKind::Home => Reach::Home,
            HelperKind::Own | HelperKind::Cloud => Reach::Mine,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            HelperKind::Home => "home",
            HelperKind::Own => "own",
            HelperKind::Cloud => "cloud",
        }
    }

    pub fn parse(text: &str) -> Option<HelperKind> {
        [HelperKind::Home, HelperKind::Own, HelperKind::Cloud].into_iter().find(|kind| kind.as_str() == text)
    }
}

/// How far a road goes, for the regulator: home (this gateway, helpers at a home address), mine (one's own server, the
/// DuoWave cloud), foreign (clouds by keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reach {
    Home,
    Mine,
    Foreign,
}

/// «Что может уходить из дома»: «Только дома» · «Дом и моё облако» · «Любые облака» (the default).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Away {
    Home,
    Mine,
    #[default]
    Any,
}

impl Away {
    /// Whether a road of this reach may be taken: «Только дома» — home only; «Дом и моё облако» — home and mine; «Любые
    /// облака» — all.
    pub fn lets(self, reach: Reach) -> bool {
        match self {
            Away::Home => reach == Reach::Home,
            Away::Mine => reach != Reach::Foreign,
            Away::Any => true,
        }
    }

    pub fn parse(text: &str) -> Option<Away> {
        serde_json::from_value(Value::String(text.to_string())).ok()
    }
}

/// How far an entry's road goes: the larger of its kind's reach and its address's (a home address — home, any other —
/// mine). An outside address is never «дома», whatever kind was given.
pub fn reach_of(entry: &HelperSetting) -> Reach {
    let address = if is_local_url(Some(&entry.url)) { Reach::Home } else { Reach::Mine };
    entry.kind.reach().max(address)
}

/// One entry of the setting `helpers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperSetting {
    /// `[a-z0-9-]{1,40}`, never a route word (`RESERVED`); `home-computer` for the pair's door (legacy.rs).
    pub id: String,
    /// What people see: 1–60 characters.
    pub name: String,
    /// `http(s)://host[:port][/path]`, no trailing slash, no query.
    pub url: String,
    pub kind: HelperKind,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// The abilities not taken from it («Не брать у него…»); names this gateway does not know are passed by.
    #[serde(default, skip_serializing_if = "Vec::is_empty", deserialize_with = "abilities_lenient")]
    pub without: Vec<Ability>,
    /// The id its contract named at its last look (`contract.helper.id`): a gateway knows itself in a list by it, and
    /// the same helper reached twice by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper_id: Option<String>,
}

fn yes() -> bool {
    true
}

/// A list of ability names; the ones this gateway does not know (a newer gateway's) are passed by.
pub(crate) fn abilities_lenient<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Ability>, D::Error> {
    let names: Vec<Value> = Vec::deserialize(deserializer)?;
    Ok(sorted(names.iter().filter_map(Value::as_str).filter_map(Ability::parse)))
}

/// Abilities sorted in their declaration order, each once.
fn sorted(abilities: impl IntoIterator<Item = Ability>) -> Vec<Ability> {
    abilities.into_iter().collect::<Abilities>().iter().collect()
}

/// The pair's door: the entry the old «Домашний сервер» became (legacy.rs), each gateway's own (it never travels).
pub const HOME_COMPUTER: &str = "home-computer";

/// Whether a home computer's name is still the default one, in any language it was written in: «Компьютер дома» is
/// the source, its English is what a gateway without the household's language wrote (item 48).
fn default_name(name: &str) -> bool {
    let name = name.trim();
    name == "Компьютер дома" || name == "Home computer" || name == crate::tr!("Компьютер дома")
}
/// Words of `/api/v1/helpers/<word>` beside `{id}`: never an id. `self` is the built-in helper's row (stage 6).
pub const RESERVED: [&str; 5] = ["order", "away", "key", "clients", "self"];
/// The built-in helper's id (builtin.rs): its row in the list as the panel sees it, its model servers in «Мозг» and
/// its roads in «Где сейчас».
pub const SELF: &str = "self";
/// How often each helper is looked at.
pub const CHECK_EVERY: Duration = Duration::from_secs(15);
/// Looks in a row without an answer before a helper counts as away (~30 s).
pub const DOWN_AFTER: u32 = 2;
/// How long the helpers answer before «Свой» comes back by itself.
pub const RETURN_AFTER: Duration = Duration::from_secs(60);
/// The longest name of a helper.
pub const NAME_MAX: usize = 60;
/// Why «Свой» gave way to «Вместе», or came back (static; their English is in services.json).
pub const AWAY: &str = "Турбо не на связи";
pub const BACK: &str = "Турбо снова на связи";
pub const NO_HEARING: &str = "нет Турбо со слухом";
pub const NO_VOICE: &str = "нет Турбо с голосом";
pub const GIVEN: &str = "Турбо снова даёт слух и голос";
pub const HEARING_NOT_READY: &str = "слух Турбо ещё не готов";
pub const VOICE_NOT_READY: &str = "голос Турбо ещё не готов";
pub const SHUT: &str = "регулятор не пускает Турбо";
/// Reasons of a switch out of «Свой» for a part not given (and the old ones a station may still have stored): «Свой»
/// comes back saying `GIVEN`.
const FOR_A_PART: [&str; 6] =
    [NO_HEARING, NO_VOICE, HEARING_NOT_READY, VOICE_NOT_READY, "слух компьютера выключен", "голос компьютера выключен"];

/// The secret a helper's key lives under.
pub fn key_secret(id: &str) -> String {
    format!("helper:{id}:key")
}

/// An entry's id: `[a-z0-9-]{1,40}`, not a route word.
pub fn valid_id(id: &str) -> bool {
    (1..=40).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !RESERVED.contains(&id)
}

/// The setting `helpers` as read: the items that do not read, whose id is no id, or whose id came before, are passed
/// by (a hand edit, a newer gateway's shape).
pub fn parse_helpers(value: &Value) -> Option<Vec<HelperSetting>> {
    let mut list: Vec<HelperSetting> = Vec::new();
    for item in value.as_array()? {
        let Ok(entry) = serde_json::from_value::<HelperSetting>(item.clone()) else { continue };
        if valid_id(&entry.id) && !list.iter().any(|known| known.id == entry.id) {
            list.push(entry);
        }
    }
    Some(list)
}

/// A helper as the panel shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HelperState {
    /// Not looked at yet, or no key to look with.
    Unknown,
    Up,
    Down,
    /// Switched off.
    Off,
    /// This gateway itself.
    #[serde(rename = "self")]
    Itself,
    /// The same helper as an earlier entry, which is on the line.
    Twin,
}

/// Who gives an ability: this gateway's own parts (the built-in helper, stage 6), or a helper of the list.
#[derive(Clone)]
pub enum Giver {
    Itself,
    Helper(Arc<RemoteHelper>),
}

/// This gateway's own parts seen as a helper — the built-in helper of a computer that is not in the station role
/// (stage 6). Taken without the network.
pub trait OwnParts: Send + Sync {
    /// What it gives other gateways (ready flags as they see them).
    fn contract(&self) -> Contract;
    /// Whether this gateway takes the ability from itself now: the part is there (a loading engine counts).
    fn gives_here(&self, ability: Ability) -> bool;
    fn recognizer(&self) -> Option<Arc<dyn SpeechRecognizer>>;
    fn speech(&self) -> Option<Arc<OwnSpeech>>;
    /// This computer's own model servers among «Мозг»'s rows, each with whether it answered the last look.
    fn models(&self) -> Vec<OwnModel>;
    /// An entry's address is this gateway itself (its port on loopback or one of its home addresses).
    fn is_me(&self, url: &str) -> bool;
    /// The server of a part it gives here — its recognition, its voice — was found closed at the last look: still
    /// tried first here, but «Где сейчас» names the road that answers and says why.
    fn silent(&self, _ability: Ability) -> bool {
        false
    }
    /// Its own parts looked at, on the helpers' tick (never its own /home/v1).
    fn check(&self) -> BoxFuture<'_, ()>;
}

/// A model server of this computer among «Мозг»'s rows (stage 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnModel {
    pub provider: String,
    pub base_url: String,
    /// It answered the last look; None — not looked at yet.
    pub up: Option<bool>,
}

/// Where this gateway's in-process clients reach a linked helper (stage 7): the port it really listens on and the
/// bridge's per-start token. Made by `Helpers::new`; never stored, never sent.
pub struct Local {
    port: AtomicU16,
    token: String,
}

impl Local {
    pub fn port(&self) -> u16 {
        self.port.load(Ordering::Relaxed)
    }

    pub fn token(&self) -> &str {
        &self.token
    }
}

/// A new helper, as the panel adds it (`POST /api/v1/helpers`).
#[derive(Debug, Clone, Default)]
pub struct AddInput {
    pub url: String,
    pub key: String,
    pub name: Option<String>,
    pub kind: Option<HelperKind>,
    pub without: Vec<Ability>,
    /// «Подключить заново» (stage 7): the entry a new pairing replaces.
    pub replace: Option<String>,
}

/// A change of an entry (`PUT /api/v1/helpers/{id}`): what is not given stays.
#[derive(Debug, Clone, Default)]
pub struct Change {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub without: Option<Vec<Ability>>,
}

/// The pair's door, in the old names (`/api/v1/home-server*`, legacy.rs).
#[derive(Debug, Clone, Default)]
pub struct DoorInput {
    pub url: Option<String>,
    pub key: Option<String>,
    pub enabled: Option<bool>,
    /// "asr", "tts", "llm", "messages".
    pub without: Option<Vec<String>>,
}

/// The list, the regulator and the linked helpers' pins as last read: no choice of road reads the settings table.
#[derive(Debug, Default)]
struct Snapshot {
    list: Vec<HelperSetting>,
    away: Away,
    pins: BTreeMap<String, HelperPin>,
}

/// The helpers together, for the registry and «Свой»: undecided until the first decided look.
#[derive(Debug, Default)]
struct Aggregate {
    state: Option<HomeServer>,
    up_since: Option<Instant>,
}

/// The helpers: the list and the choice of road, the looks, the regulator, «Свой» on the station.
pub struct Helpers {
    config: Arc<Config>,
    settings: Arc<SettingsService>,
    secrets: Arc<SecretsService>,
    events: EventBus,
    profile: Profile,
    identity: Arc<Identity>,
    snapshot: RwLock<Arc<Snapshot>>,
    /// Each entry as this gateway reaches it, by its id: made again when its address or key changes.
    remotes: Mutex<BTreeMap<String, Arc<RemoteHelper>>>,
    /// Every read-modify-write of the list or the regulator, and the snapshot that follows them.
    writing: Mutex<()>,
    modes: OnceLock<Arc<ModeService>>,
    /// The station's cloud hearing and voice as its panel sets them (src/speech/cloud_speech.rs).
    cloud: OnceLock<Arc<crate::speech::cloud_speech::CloudSpeech>>,
    own: OnceLock<Arc<dyn OwnParts>>,
    local: Arc<Local>,
    stop: CancellationToken,
    aggregate: Mutex<Aggregate>,
    /// This gateway's own id, once known.
    me: OnceLock<String>,
    /// The listener that follows the settings: ended by `stop`.
    followers: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// The list, the regulator or the pins did not read at the last try: read again at the next tick.
    unread: AtomicBool,
    /// Itself, for the links' news.
    this: OnceLock<Weak<Helpers>>,
    /// The links' times (tests scale them).
    link_timing: Mutex<LinkTiming>,
    /// The looks have started, and the server listens on its port (the bridge's address): links run once both hold.
    started: AtomicBool,
    listening: AtomicBool,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Helpers {
    /// The list and the regulator read once; their changes from elsewhere (the settings' route, a test) followed.
    pub fn new(
        config: Arc<Config>,
        settings: Arc<SettingsService>,
        secrets: Arc<SecretsService>,
        events: EventBus,
        profile: Profile,
        identity: Arc<Identity>,
    ) -> Arc<Helpers> {
        let local = Arc::new(Local { port: AtomicU16::new(config.app.port), token: crate::utils::ids::random_hex(32) });
        let helpers = Arc::new(Helpers {
            // Until the list and the regulator are read: no helper, and nothing leaves the home.
            snapshot: RwLock::new(Arc::new(Snapshot { list: Vec::new(), away: Away::Home, pins: BTreeMap::new() })),
            config,
            settings,
            secrets,
            events,
            profile,
            identity,
            remotes: Mutex::new(BTreeMap::new()),
            writing: Mutex::new(()),
            modes: OnceLock::new(),
            cloud: OnceLock::new(),
            own: OnceLock::new(),
            local,
            stop: CancellationToken::new(),
            aggregate: Mutex::new(Aggregate::default()),
            me: OnceLock::new(),
            followers: Mutex::new(Vec::new()),
            unread: AtomicBool::new(false),
            this: OnceLock::new(),
            link_timing: Mutex::new(LinkTiming::default()),
            started: AtomicBool::new(false),
            listening: AtomicBool::new(false),
        });
        let _ = helpers.this.set(Arc::downgrade(&helpers));
        helpers.reread();
        helpers.follow();
        helpers
    }

    /// The gateway's own switch out of «Свой» and back (the station), as the helpers go and come.
    /// The station's cloud hearing and voice: «Свой» stands on them while no helper gives them
    /// (docs/plans/speaker-alone.md).
    pub fn set_cloud(&self, cloud: Arc<crate::speech::cloud_speech::CloudSpeech>) {
        let _ = self.cloud.set(cloud);
    }

    /// The cloud hears for the station: the panel's (src/speech/cloud_speech.rs), else the .env's — while the
    /// regulator lets the clouds out.
    fn cloud_hears(&self) -> bool {
        let set = match self.cloud.get() {
            Some(cloud) => cloud.hears(),
            None => {
                let asr = &self.config.speech.asr;
                asr.enabled && !is_local_url(Some(&asr.base_url))
            }
        };
        set && self.lets(Reach::Foreign)
    }

    /// The cloud speaks for the station: the panel's, else the .env's — while the regulator lets the clouds out.
    fn cloud_speaks(&self) -> bool {
        let set = match self.cloud.get() {
            Some(cloud) => cloud.speaks(),
            None => self.config.gateway.tts.own.as_ref().is_some_and(|own| !is_local_url(Some(&own.base_url))),
        };
        set && self.lets(Reach::Foreign)
    }

    pub fn set_modes(&self, modes: Arc<ModeService>) {
        let _ = self.modes.set(modes);
    }

    /// This gateway's own parts as a helper (stage 6: a computer build that is not a station).
    pub fn set_own(&self, own: Arc<dyn OwnParts>) {
        let _ = self.own.set(own);
    }

    /// This gateway's own hearing, when it takes it from itself (stage 6).
    pub fn own_recognizer(&self) -> Option<Arc<dyn SpeechRecognizer>> {
        self.own.get().and_then(|own| own.recognizer())
    }

    /// The port this gateway really listens on (`app::serve`; tests listen on port 0). The links start from now on: a
    /// linked helper's clients ask this gateway's own bridge, which answers only once the server listens.
    pub fn set_local_port(&self, port: u16) {
        self.local.port.store(port, Ordering::Relaxed);
        self.listening.store(true, Ordering::SeqCst);
        self.links();
    }

    pub fn local_port(&self) -> u16 {
        self.local.port()
    }

    pub fn local(&self) -> Arc<Local> {
        self.local.clone()
    }

    /// Follows `settings.changed` of the list and the regulator: a writer other than this one. Only where a runtime
    /// runs (a build in a sync test follows nothing).
    fn follow(self: &Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let me = Arc::downgrade(self);
        let follower = self.events.on("helpers", move |event| {
            let GatewayEvent::SettingsChanged { key, user_id: None, .. } = &*event else { return };
            if key != HELPERS.key && key != AWAY_SETTING.key && key != HELPER_PINS.key {
                return;
            }
            if let Some(helpers) = me.upgrade() {
                if let Some(before) = helpers.reread() {
                    helpers.prune();
                    helpers.forget_crossed(&before);
                    helpers.links();
                    helpers.settle();
                }
            }
        });
        locked(&self.followers).push(follower);
    }

    /// The snapshot read again from the settings; the one it replaced, when it changed. A part that does not read
    /// keeps its last read — before the first: no helper, «Только дома», no pin — and is read again at the next tick:
    /// a read error never empties the list nor opens the regulator. (A linked entry whose pin did not read stays linked
    /// by its key: never reached plainly, its `dwl1_` key goes nowhere — nor into «Мозг» —, remote.rs.)
    fn reread(&self) -> Option<Arc<Snapshot>> {
        let _writing = locked(&self.writing);
        let snap = self.snap();
        let (list, away, pins) =
            (self.settings.peek(&HELPERS), self.settings.peek(&AWAY_SETTING), self.settings.peek(&HELPER_PINS));
        if let Some(error) = list.as_ref().err().or(away.as_ref().err()).or(pins.as_ref().err()) {
            tracing::warn!(
                "helpers: the list, the regulator or the pins not read, the last read stays: {}",
                error.message
            );
        }
        self.unread.store(list.is_err() || away.is_err() || pins.is_err(), Ordering::Relaxed);
        let list = list.map(Option::unwrap_or_default).unwrap_or_else(|_| snap.list.clone());
        let away = away.map(Option::unwrap_or_default).unwrap_or(snap.away);
        let pins = pins.map(Option::unwrap_or_default).unwrap_or_else(|_| snap.pins.clone());
        if snap.list == list && snap.away == away && snap.pins == pins {
            return None;
        }
        *self.snapshot.write().unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Arc::new(Snapshot { list, away, pins });
        Some(snap)
    }

    fn snap(&self) -> Arc<Snapshot> {
        self.snapshot.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// The looks: right away, then every `CHECK_EVERY`, until `stop`. The tick holds the helpers only while it looks,
    /// so a gateway started again in its process (the desktop app) leaves no tick behind.
    pub fn start(self: &Arc<Self>) {
        self.started.store(true, Ordering::SeqCst);
        self.links();
        let me: Weak<Helpers> = Arc::downgrade(self);
        let stop = self.stop.clone();
        tokio::spawn(async move {
            loop {
                let Some(helpers) = me.upgrade() else { break };
                tokio::select! {
                    _ = helpers.check_all() => {}
                    _ = stop.cancelled() => break,
                }
                if let Some(own) = helpers.own.get().cloned() {
                    own.check().await;
                }
                drop(helpers);
                tokio::select! {
                    _ = tokio::time::sleep(CHECK_EVERY) => {}
                    _ = stop.cancelled() => break,
                }
            }
        });
    }

    /// The tick ends, and the listener with it (`App::shutdown`); the links too.
    pub fn stop(&self) {
        self.stop.cancel();
        for follower in locked(&self.followers).drain(..) {
            follower.abort();
        }
        self.stop_links();
    }

    /// Every link closed (1000), none started again: the gateway stops.
    pub fn stop_links(&self) {
        self.started.store(false, Ordering::SeqCst);
        for remote in locked(&self.remotes).values() {
            remote.halt_link(false);
        }
    }

    /// For tests: the links' times, for the links started from now on.
    pub fn set_link_timing(&self, timing: LinkTiming) {
        *locked(&self.link_timing) = timing;
    }

    pub fn link_timing(&self) -> LinkTiming {
        locked(&self.link_timing).clone()
    }

    /// The pin of a linked entry.
    pub fn pin(&self, id: &str) -> Option<HelperPin> {
        self.snap().pins.get(id).cloned()
    }

    /// The link of a linked entry, while it runs: the bridge's way to it.
    pub fn link_of(&self, id: &str) -> Option<Arc<link::Link>> {
        locked(&self.remotes).get(id).and_then(|remote| remote.link())
    }

    /// An entry that leaves its link's client behind — removed, or renewed with a new key —: its link says `bye` (the
    /// old key proves that client, so its helper forgets it) and ends; a link not up says nothing, and its client stays
    /// in the helper's «Помогаю» until removed there.
    fn say_bye(&self, id: &str) {
        let old = locked(&self.remotes).remove(id);
        if let Some(old) = old {
            old.halt_link(true);
        }
    }

    /// Each linked entry's link runs only while the entry may give at all — switched on, keyed, pinned, let out by
    /// the regulator, not this gateway —: started when it may, stopped (1000) as soon as it may not. A twin's link runs
    /// on: while the earlier entry stands for the helper the twin gives nothing, and freed it says at once how it is —
    /// its watch is its link's, never stale. Nothing dials before the looks start and the server listens on its port.
    fn links(&self) {
        let snap = self.snap();
        let started = self.started.load(Ordering::SeqCst) && self.listening.load(Ordering::SeqCst);
        for entry in &snap.list {
            let Some(remote) = self.remote(entry).filter(|remote| remote.linked()) else { continue };
            let runs = started && entry.enabled && snap.away.lets(reach_of(entry)) && !self.is_self(entry);
            if !runs {
                remote.halt_link(false);
                continue;
            }
            let me = self.this.get().cloned().unwrap_or_default();
            let news: self::link::NewsSink = Arc::new(move |id: &str, news: LinkNews| {
                if let Some(helpers) = me.upgrade() {
                    helpers.on_link(id, news);
                }
            });
            remote.run_link(self.link_timing(), news);
        }
    }

    /// What a link told: the entry's state and status at once, the events, «Свой»; a helper on the line with a bot has
    /// its contacts read through the bridge, its bot found or gone told as a look's is (`bot_news`). News of an entry
    /// the regulator keeps out now — told as the regulator closed — is forgotten with the watch, and nothing follows it
    /// (as a look's, `look_at`).
    fn on_link(&self, id: &str, news: LinkNews) {
        let Some(remote) = locked(&self.remotes).get(id).cloned() else { return };
        let before = remote.status().map(|status| declared_of(&status.contract));
        let changed = remote.linked_seen(&news);
        if self.kept_out(id) {
            remote.reset();
            return;
        }
        let after = remote.status().map(|status| declared_of(&status.contract));
        self.after_look(&remote, changed, before != after && after.is_some());
        if changed == Some(true) && remote.declares(Ability::Messages) {
            let (remote, me) = (remote.clone(), self.this.get().cloned().unwrap_or_default());
            tokio::spawn(async move {
                let found = remote.read_contacts().await;
                if let Some(helpers) = me.upgrade().filter(|_| found) {
                    helpers.bot_news(&remote);
                }
            });
        }
        self.settle();
    }

    /// A linked helper's bot found or gone, read through the bridge (`RemoteHelper::read_contacts`): news for the
    /// registry, as a look's is (`after_look`) — what it gives changed —, unless the regulator keeps it out now.
    fn bot_news(&self, remote: &RemoteHelper) {
        if !self.kept_out(remote.id()) {
            self.after_look(remote, None, true);
        }
    }

    /// One look at every helper that may be looked at — switched on, with a key, not this gateway, not the same as an
    /// earlier one on the line, let out by the regulator (a blocked helper's look would leave the home all the same) —
    /// at once; one found away frees an entry that reaches the same helper another way, looked at in the same tick —
    /// a twin keeps no watch (`forget_twins`), so that look decides at once. A linked helper is not looked at: its
    /// link says how it is, and its bot's contacts are read through the bridge (found or gone, news: `bot_news`).
    /// Then «Свой» as they are, with none looked at too: a station started in «Свой» whose helpers the regulator keeps
    /// out all (an imported «Только дома») leaves it at its first tick. A list that did not read is read first.
    pub async fn check_all(&self) {
        if self.unread.load(Ordering::Relaxed) {
            if let Some(before) = self.reread() {
                self.prune();
                self.forget_crossed(&before);
                self.links();
            }
        }
        let mut looked: Vec<String> = Vec::new();
        loop {
            let snap = self.snap();
            self.forget_twins(&snap, &looked);
            let (linked, next): (Vec<Arc<RemoteHelper>>, Vec<Arc<RemoteHelper>>) = snap
                .list
                .iter()
                .filter(|entry| !looked.contains(&entry.id) && self.eligible(entry, &snap, None))
                .filter_map(|entry| self.remote(entry))
                .partition(|remote| remote.linked());
            if next.is_empty() && linked.is_empty() {
                break;
            }
            looked.extend(next.iter().chain(&linked).map(|remote| remote.id().to_string()));
            let contacts: Vec<&Arc<RemoteHelper>> =
                linked.iter().filter(|remote| remote.up() && remote.declares(Ability::Messages)).collect();
            let reads = futures::future::join_all(contacts.iter().map(|remote| remote.read_contacts()));
            let looks = futures::future::join_all(next.iter().map(|remote| self.look_at(remote)));
            let (_, found) = futures::future::join(looks, reads).await;
            for (remote, found) in contacts.into_iter().zip(found) {
                if found {
                    self.bot_news(remote);
                }
            }
        }
        self.settle();
    }

    /// One look now at an entry, a switched-off one too; one the regulator keeps out says so and is not looked at.
    pub async fn check(&self, id: &str) -> Result<(), ApiError> {
        let snap = self.snap();
        let entry = snap.list.iter().find(|entry| entry.id == id).ok_or_else(no_such_helper)?;
        if !snap.away.lets(reach_of(entry)) {
            return Err(ApiError::conflict(crate::tr!(
                "Регулятор «Что может уходить из дома» не пускает этого Турбо."
            )));
        }
        if self.is_self(entry) || self.twin_of(entry, &snap).is_some() {
            return Ok(());
        }
        let Some(remote) = self.remote(entry) else { return Ok(()) };
        // A linked helper: its link tries now, the pauses' count kept.
        if remote.linked() {
            if let Some(link) = remote.link() {
                link.wake();
            }
            return Ok(());
        }
        self.look_at(&remote).await;
        self.settle();
        Ok(())
    }

    /// A look and what follows it: the state, the contacts of a helper with a bot, the events; this gateway itself
    /// recognised. A look the regulator closed on while it was out is forgotten with the watch, and nothing follows
    /// it — no event, no contacts read: checked after `seen`, so a `set_away` on either side of the check leaves no
    /// watch behind (it forgets the watches after it has changed the snapshot).
    async fn look_at(&self, remote: &Arc<RemoteHelper>) {
        let before = remote.status().map(|status| declared_of(&status.contract));
        let outcome = remote.look().await;
        if let Ok(status) = &outcome {
            let named = status.contract.helper.id.clone();
            if !named.is_empty() && self.me().as_deref() == Some(named.as_str()) {
                remote.mark_itself();
            }
            if !named.is_empty() {
                self.note_helper_id(remote.id(), remote.url(), &named, &status.contract.helper.name);
            }
        }
        let changed = remote.seen(&outcome);
        if self.kept_out(remote.id()) {
            remote.reset();
            return;
        }
        let mut bot = false;
        if remote.up() && remote.declares(Ability::Messages) && !remote.itself() {
            bot = remote.read_contacts().await;
        }
        let after = remote.status().map(|status| declared_of(&status.contract));
        self.after_look(remote, changed, (before != after && after.is_some()) || bot);
    }

    /// The events of a look: up or down when the state changed — and the old ones for the pair's door —, a new
    /// contract when what it declares changed, or its bot was found or gone (what it gives: the registry reads again).
    fn after_look(&self, remote: &RemoteHelper, changed: Option<bool>, contract: bool) {
        let (id, url) = (remote.id().to_string(), remote.url().to_string());
        match changed {
            Some(true) => {
                tracing::info!("helpers: «{id}» at {url} is up");
                self.events.emit(GatewayEvent::HelperUp { id: id.clone(), url: url.clone() });
                if id == HOME_COMPUTER {
                    self.events.emit(GatewayEvent::HomeServerUp { url: url.clone() });
                }
            }
            Some(false) => {
                let reason = remote.error().unwrap_or_else(|| AWAY.to_string());
                tracing::warn!("helpers: «{id}» at {url} is away: {reason}");
                self.events.emit(GatewayEvent::HelperDown { id: id.clone(), url: url.clone(), reason: reason.clone() });
                if id == HOME_COMPUTER {
                    self.events.emit(GatewayEvent::HomeServerDown { url: url.clone(), reason });
                }
            }
            None => {}
        }
        if contract {
            self.events.emit(GatewayEvent::HelpersChanged { id: Some(id), change: "contract".into(), actor: None });
        }
    }

    /// The id a look found at an entry, kept in it — one write, only when it changes, only while its address is the
    /// one looked at. The home's computer still under its default name («Компьютер дома», which a station whose
    /// household language was not set yet wrote as «Home computer») takes the name the computer gives itself
    /// (clean-path check 18.11, item 48: «Турбо · Home computer» in a Russian panel, «Мак» in the workshop).
    fn note_helper_id(&self, id: &str, url: &str, named: &str, own_name: &str) {
        let own_name = own_name.trim();
        let renamed = |entry: &HelperSetting| {
            entry.id == HOME_COMPUTER && !own_name.is_empty() && default_name(&entry.name) && entry.name != own_name
        };
        let noted = |entry: &HelperSetting| {
            entry.id == id && entry.url == url && entry.helper_id.as_deref() == Some(named) && !renamed(entry)
        };
        if self.snap().list.iter().any(noted) {
            return;
        }
        let written = self.write(None, |list| {
            let Some(entry) = list.iter_mut().find(|entry| entry.id == id && entry.url == url) else { return Ok(None) };
            let rename = renamed(entry);
            if entry.helper_id.as_deref() == Some(named) && !rename {
                return Ok(None);
            }
            entry.helper_id = Some(named.to_string());
            if rename {
                entry.name = own_name.to_string();
            }
            Ok(Some(()))
        });
        if let Err(error) = written {
            tracing::warn!("helpers: the id of «{id}» not kept: {}", error.message);
        }
    }

    pub fn list(&self) -> Vec<HelperSetting> {
        self.snap().list.clone()
    }

    pub fn entry(&self, id: &str) -> Option<HelperSetting> {
        self.snap().list.iter().find(|entry| entry.id == id).cloned()
    }

    pub fn away(&self) -> Away {
        self.snap().away
    }

    /// Whether the regulator lets a road of this reach out now.
    pub fn lets(&self, reach: Reach) -> bool {
        self.away().lets(reach)
    }

    /// This gateway's own id (core/identity/home.rs).
    pub fn me(&self) -> Option<String> {
        if let Some(me) = self.me.get() {
            return Some(me.clone());
        }
        let me = self.identity.gateway()?.id;
        Some(self.me.get_or_init(|| me).clone())
    }

    /// An entry that is this gateway itself: its id is ours, its look found us, or its address is ours.
    pub fn is_self(&self, entry: &HelperSetting) -> bool {
        (entry.helper_id.is_some() && entry.helper_id == self.me())
            || locked(&self.remotes).get(&entry.id).is_some_and(|remote| remote.itself() && remote.url() == entry.url)
            || self.own.get().is_some_and(|own| own.is_me(&entry.url))
    }

    /// An entry whose helper an earlier entry reaches now (the same `helperId`): passed by.
    pub fn is_twin(&self, entry: &HelperSetting) -> bool {
        self.twin_of(entry, &self.snap()).is_some()
    }

    /// The earlier entry that reaches the same helper (the same `helperId`) and stands for it now: it may give and is
    /// on the line. While it cannot — switched off, keyless, kept out by the regulator, away or not read yet — the
    /// same helper is reached by this one, after a look of its own (a twin keeps no watch: `forget_twins`).
    fn twin_of<'a>(&self, entry: &HelperSetting, snap: &'a Snapshot) -> Option<&'a HelperSetting> {
        let named = entry.helper_id.as_deref()?;
        snap.list.iter().take_while(|earlier| earlier.id != entry.id).find(|earlier| {
            earlier.helper_id.as_deref() == Some(named)
                && self.open(earlier, snap)
                && self.remote(earlier).is_some_and(|remote| remote.up())
        })
    }

    /// An entry's key, for the shell's old `configured()`.
    pub(crate) fn key(&self, id: &str) -> Option<String> {
        self.secrets.get(&key_secret(id))
    }

    /// What the panel may know of an entry's key.
    pub fn key_mask(&self, id: &str) -> SecretMask {
        self.secrets.mask(&key_secret(id))
    }

    /// The entry as this gateway reaches it — over its link when it has a pin; None without a key. Made again when
    /// its address, key or pin changes (its old link stopped).
    pub fn remote(&self, entry: &HelperSetting) -> Option<Arc<RemoteHelper>> {
        let key = self.secrets.get(&key_secret(&entry.id))?;
        let pin = self.snap().pins.get(&entry.id).cloned();
        let mut remotes = locked(&self.remotes);
        if let Some(remote) = remotes.get(&entry.id).filter(|remote| remote.same_as(entry, &key, pin.as_ref())) {
            return Some(remote.clone());
        }
        if let Some(old) = remotes.remove(&entry.id) {
            old.halt_link(false);
        }
        let remote =
            Arc::new(RemoteHelper::reached(entry, key, pin, &self.config, self.identity.clone(), self.local.clone()));
        remotes.insert(entry.id.clone(), remote.clone());
        Some(remote)
    }

    /// The runtime of entries gone from the list let go, their links stopped.
    fn prune(&self) {
        let snap = self.snap();
        locked(&self.remotes).retain(|id, remote| {
            let kept = snap.list.iter().any(|entry| entry.id == *id);
            if !kept {
                remote.halt_link(false);
            }
            kept
        });
    }

    /// The regulator keeps the entry out now: it is not looked at and keeps no watch.
    fn kept_out(&self, id: &str) -> bool {
        let snap = self.snap();
        snap.list.iter().find(|entry| entry.id == id).is_some_and(|entry| !snap.away.lets(reach_of(entry)))
    }

    /// After a change of the regulator or the list (`before` — the snapshot it replaced): the watch forgotten of every
    /// entry the regulator keeps out now, and of every one it let out only now. One kept out is not looked at, so what
    /// it last showed is not now — the pair's old view and the shell say «down» meanwhile —; one let out again gives
    /// after a new look, never on one from before — the add's, or a look the regulator closed on while it was out.
    fn forget_crossed(&self, before: &Snapshot) {
        let snap = self.snap();
        let let_out_before = |id: &str| {
            before.list.iter().find(|entry| entry.id == id).is_some_and(|entry| before.away.lets(reach_of(entry)))
        };
        let remotes = locked(&self.remotes);
        for entry in snap.list.iter().filter(|entry| !snap.away.lets(reach_of(entry)) || !let_out_before(&entry.id)) {
            if let Some(remote) = remotes.get(&entry.id) {
                remote.reset();
            }
        }
    }

    /// The watch of every twin not looked at in this tick (`looked`) forgotten: a twin is not looked at, so what it
    /// last showed is not now. Freed — its earlier entry switched off, kept out, keyless, removed or away — it gives
    /// only after a look of its own, which decides at once: in the same tick when the earlier entry went away in it,
    /// else at the next. A linked twin keeps its watch: its link runs on and tells it as it changes (`links`).
    fn forget_twins(&self, snap: &Snapshot, looked: &[String]) {
        let twins: Vec<&str> = snap
            .list
            .iter()
            .filter(|entry| !looked.contains(&entry.id) && self.twin_of(entry, snap).is_some())
            .map(|entry| entry.id.as_str())
            .collect();
        let remotes = locked(&self.remotes);
        for id in twins {
            if let Some(remote) = remotes.get(id).filter(|remote| !remote.linked()) {
                remote.reset();
            }
        }
    }

    /// The pair's door on the line, for its old views: its own watch up, or the same computer reached by an earlier
    /// entry that is (a twin keeps no watch of its own).
    pub(crate) fn door_up(&self, entry: &HelperSetting) -> bool {
        self.is_twin(entry) || self.remote(entry).is_some_and(|remote| remote.up())
    }

    /// An entry may give at all: switched on, with a key, let out by the regulator, not this gateway.
    fn open(&self, entry: &HelperSetting, snap: &Snapshot) -> bool {
        entry.enabled
            && self.secrets.has(&key_secret(&entry.id))
            && snap.away.lets(reach_of(entry))
            && !self.is_self(entry)
    }

    /// An entry may give: open, not switched off for the ability, and not the same as an earlier one on the line.
    fn eligible(&self, entry: &HelperSetting, snap: &Snapshot, ability: Option<Ability>) -> bool {
        self.open(entry, snap)
            && ability.is_none_or(|ability| !entry.without.contains(&ability))
            && self.twin_of(entry, snap).is_none()
    }

    /// For a sticky ability (the history): the first switched-on entry that declares it — regulator, liveness and
    /// `without` aside —, when it may give now; never the next one.
    fn sticky(&self, ability: Ability, snap: &Snapshot) -> Option<Arc<RemoteHelper>> {
        let first = snap.list.iter().find(|entry| {
            entry.enabled
                && !self.is_self(entry)
                && self.twin_of(entry, snap).is_none()
                && self.remote(entry).is_some_and(|remote| remote.declares(ability))
        })?;
        self.eligible(first, snap, Some(ability)).then(|| self.remote(first)).flatten()
    }

    /// Who gives the ability now, in order: this gateway's own parts first (stage 6), then the helpers that are on the
    /// line, have declared it and are ready.
    pub fn pick_all(&self, ability: Ability) -> Vec<Giver> {
        let snap = self.snap();
        let mut givers = Vec::new();
        if self.own.get().is_some_and(|own| own.gives_here(ability)) {
            givers.push(Giver::Itself);
        }
        if ability.sticky() {
            givers.extend(
                self.sticky(ability, &snap).filter(|remote| remote.up() && remote.gives(ability)).map(Giver::Helper),
            );
            return givers;
        }
        for entry in snap.list.iter().filter(|entry| self.eligible(entry, &snap, Some(ability))) {
            if let Some(remote) = self.remote(entry).filter(|remote| remote.up() && remote.gives(ability)) {
                givers.push(Giver::Helper(remote));
            }
        }
        givers
    }

    /// The first who gives the ability now.
    pub fn pick(&self, ability: Ability) -> Option<Giver> {
        self.pick_all(ability).into_iter().next()
    }

    /// The first who would give it, on the line or not: the first eligible helper that declares it, or that is not
    /// read yet.
    pub fn offer(&self, ability: Ability) -> Option<Giver> {
        if self.own.get().is_some_and(|own| own.gives_here(ability)) {
            return Some(Giver::Itself);
        }
        if ability.sticky() {
            return self.sticky(ability, &self.snap()).map(Giver::Helper);
        }
        self.offering(ability).into_iter().next().map(Giver::Helper)
    }

    /// Every helper that would give the ability, on the line or not, in order: eligible, and declaring it or not read
    /// yet (one not read only may give it).
    pub fn offering(&self, ability: Ability) -> Vec<Arc<RemoteHelper>> {
        let snap = self.snap();
        snap.list
            .iter()
            .filter(|entry| self.eligible(entry, &snap, Some(ability)))
            .filter_map(|entry| self.remote(entry))
            .filter(|remote| remote.declares(ability))
            .collect()
    }

    /// The helper that gives it now; None when this gateway gives it itself, or nobody.
    pub fn pick_remote(&self, ability: Ability) -> Option<Arc<RemoteHelper>> {
        match self.pick(ability)? {
            Giver::Helper(remote) => Some(remote),
            Giver::Itself => None,
        }
    }

    /// The helpers together, for the registry: none to lean on, one of them on the line, or all away.
    pub fn state(&self) -> HomeServer {
        let snap = self.snap();
        let mut eligible = snap.list.iter().filter(|entry| self.eligible(entry, &snap, None)).peekable();
        if eligible.peek().is_none() {
            return HomeServer::None;
        }
        if eligible.any(|entry| self.remote(entry).is_some_and(|remote| remote.up())) {
            HomeServer::Up
        } else {
            HomeServer::Down
        }
    }

    /// Whether the helpers together are decided: none to lean on, one on the line, or every one with its first look.
    fn decided(&self, state: HomeServer) -> bool {
        if state != HomeServer::Down {
            return true;
        }
        let snap = self.snap();
        snap.list
            .iter()
            .filter(|entry| self.eligible(entry, &snap, None))
            .all(|entry| self.remote(entry).is_some_and(|remote| remote.state() != HelperState::Unknown))
    }

    /// Helpers switched on and keyed exist, and the regulator keeps them all out.
    fn shut(&self) -> bool {
        let snap = self.snap();
        let mut keyed =
            snap.list.iter().filter(|entry| entry.enabled && self.secrets.has(&key_secret(&entry.id))).peekable();
        keyed.peek().is_some() && keyed.all(|entry| !snap.away.lets(reach_of(entry)))
    }

    /// What «Свой» on the station would lack now: hearing and voice, from a helper or a cloud the regulator lets out —
    /// there is no Yandex in «Свой». None on a computer (it hears and speaks itself), and while there is no helper to
    /// lean on (then «Свой» has gone already).
    pub fn svoi_lacks(&self) -> Option<&'static str> {
        if !self.profile.station || self.state() == HomeServer::None {
            return None;
        }
        let (cloud_hearing, cloud_voice) = (self.cloud_hears(), self.cloud_speaks());
        let lacks = |ability: Ability, cloud: bool, not_ready: &'static str, none: &'static str| {
            if cloud || self.pick(ability).is_some() {
                return None;
            }
            Some(match self.offer(ability) {
                Some(Giver::Helper(remote)) if remote.up() => not_ready,
                Some(_) => AWAY,
                None => none,
            })
        };
        lacks(Ability::Hearing, cloud_hearing, HEARING_NOT_READY, NO_HEARING)
            .or_else(|| lacks(Ability::Voice, cloud_voice, VOICE_NOT_READY, NO_VOICE))
    }

    /// The aggregate after a look or a change, and «Свой» on the station as it goes: away — «Свой» gives way, saying
    /// why; on the line — it stays only with hearing and voice, and comes back after a minute when the switch out of it
    /// was the gateway's own. Only a decided aggregate counts: a quick refusal of one helper while another is still
    /// looked at decides nothing. No lock is held while the mode switches (it asks the gate, which asks here).
    fn settle(&self) {
        if !self.profile.station {
            return;
        }
        let state = self.state();
        if !self.decided(state) {
            return;
        }
        let changed = {
            let mut aggregate = locked(&self.aggregate);
            let changed = aggregate.state != Some(state);
            if changed {
                aggregate.state = Some(state);
                aggregate.up_since = (state == HomeServer::Up).then(Instant::now);
            }
            changed
        };
        // The cloud hears and speaks for the station (docs/plans/speaker-alone.md): «Свой» stays on it.
        let cloud = self.cloud_hears() && self.cloud_speaks();
        match state {
            HomeServer::Down if changed && !cloud => self.leave_svoi(AWAY),
            HomeServer::None if changed && !cloud => self.leave_svoi(if self.shut() { SHUT } else { AWAY }),
            HomeServer::Up => match self.svoi_lacks() {
                Some(reason) => self.leave_svoi(reason),
                None => self.maybe_return(),
            },
            _ => {}
        }
    }

    /// «Свой» gives way to «Вместе», kept as the gateway's own switch, with why.
    fn leave_svoi(&self, reason: &str) {
        let Some(modes) = self.modes.get() else { return };
        match modes.leave_by_itself(PanelMode::Own, PanelMode::Gateway, reason) {
            Ok(true) => tracing::warn!("mode: «Свой» → «Шлюз» by itself: {reason}"),
            Ok(false) => {}
            Err(error) => tracing::warn!("mode: «Свой» not left: {}", error.message),
        }
    }

    /// A minute of the helpers on the line: «Свой» back, if the switch out of it was the gateway's own — never while it
    /// would lack hearing or voice there.
    fn maybe_return(&self) {
        let Some(modes) = self.modes.get() else { return };
        let long_enough = locked(&self.aggregate).up_since.is_some_and(|since| since.elapsed() >= RETURN_AFTER);
        if !long_enough || self.svoi_lacks().is_some() {
            return;
        }
        let for_a_part = modes.state().auto.is_some_and(|auto| FOR_A_PART.contains(&auto.reason.as_str()));
        let reason = if for_a_part { GIVEN } else { BACK };
        match modes.return_by_itself(reason) {
            Ok(true) => tracing::info!("mode: «Свой» is back by itself: {reason}"),
            Ok(false) => {}
            Err(error) => tracing::warn!("mode: «Свой» not back: {}", error.message),
        }
    }

    /// For tests: the helpers have been on the line this long longer.
    #[cfg(test)]
    pub(crate) fn age_up(&self, by: Duration) {
        let mut aggregate = locked(&self.aggregate);
        aggregate.up_since = aggregate.up_since.and_then(|since| since.checked_sub(by));
    }

    /// The plugins', the history's and the catalog's way to a helper (`HomeLink`): the address and the key of the one
    /// that gives the ability now; none while this gateway gives it itself, or nobody does.
    pub fn link_for(self: &Arc<Self>, ability: Ability) -> Arc<dyn crate::plugins::controller::HomeLink> {
        Arc::new(AbilityLink { helpers: Arc::downgrade(self), ability })
    }

    /// One read-modify-write of the list under `writing`, from the list as stored: a list that does not read is never
    /// written over. The change says None when there is nothing to write, and an error writes nothing. The snapshot
    /// follows at once.
    pub(crate) fn write<T>(
        &self,
        actor: Option<Actor>,
        change: impl FnOnce(&mut Vec<HelperSetting>) -> Result<Option<T>, ApiError>,
    ) -> Result<Option<T>, ApiError> {
        let _writing = locked(&self.writing);
        let mut list = self.settings.peek(&HELPERS)?.unwrap_or_default();
        let Some(done) = change(&mut list)? else { return Ok(None) };
        self.settings.set(&HELPERS, &list, actor)?;
        let snap = self.snap();
        *self.snapshot.write().unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Arc::new(Snapshot { list, away: snap.away, pins: snap.pins.clone() });
        Ok(Some(done))
    }

    /// The list and the pins read now. Whatever writes both asks first, before anything it cannot take back — a
    /// pairing's key spent at its helper, a `bye`, an entry gone —: neither is ever written over when it does not read
    /// (`write`, `set_pin`), so such a change would stop half done.
    fn writable(settings: &SettingsService) -> Result<(), ApiError> {
        if settings.peek(&HELPERS).is_ok() && settings.peek(&HELPER_PINS).is_ok() {
            return Ok(());
        }
        Err(ApiError::unavailable(crate::tr!("Список Турбо сейчас не читается — попробуйте ещё раз через минуту.")))
    }

    /// A linked entry's pin, or none: one read-modify-write of the pins as stored — pins that do not read are never
    /// written over, as the list's (`write`). The snapshot follows at once.
    fn set_pin(&self, id: &str, pin: Option<HelperPin>) -> Result<(), ApiError> {
        let _writing = locked(&self.writing);
        let mut pins = self.settings.peek(&HELPER_PINS)?.unwrap_or_default();
        let changed = match pin {
            Some(pin) => pins.insert(id.to_string(), pin.clone()) != Some(pin),
            None => pins.remove(id).is_some(),
        };
        if changed {
            self.settings.set(&HELPER_PINS, &pins, None)?;
        }
        let snap = self.snap();
        if snap.pins != pins {
            *self.snapshot.write().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Arc::new(Snapshot { list: snap.list.clone(), away: snap.away, pins });
        }
        Ok(())
    }

    /// After a change: the runtime pruned, the links as they may run now, the pair's door mirrored into its old
    /// setting, the events, the aggregate.
    fn changed(&self, id: Option<&str>, change: &str, actor: Option<Actor>) {
        self.prune();
        self.links();
        self.events.emit(GatewayEvent::HelpersChanged {
            id: id.map(str::to_string),
            change: change.to_string(),
            actor: actor.clone(),
        });
        if id == Some(HOME_COMPUTER) {
            let configured = self.entry(HOME_COMPUTER).is_some_and(|entry| entry.enabled)
                && self.secrets.has(&key_secret(HOME_COMPUTER));
            self.events.emit(GatewayEvent::HomeServerChanged { configured, actor });
        }
        self.settle();
    }

    /// The look at a helper not in the list yet: its address, its key, what it gives — checked before anything is
    /// stored. The draft entry and the status its look read.
    async fn look_first(&self, input: &AddInput) -> Result<(HelperSetting, String, HelperStatus), ApiError> {
        let url = helper_url(&input.url)?;
        let home = is_local_url(Some(&url));
        if url.starts_with("http://") && !home {
            return Err(ApiError::bad_request(crate::tr!(
                "По http — только в домашней сети: снаружи Турбо подключается ключом подключения."
            )));
        }
        let key = input.key.trim().to_string();
        if key.is_empty() {
            return Err(ApiError::bad_request(crate::tr!("Нужен ключ подключения Турбо.")));
        }
        let kind = match input.kind {
            Some(HelperKind::Home) if !home => {
                return Err(ApiError::bad_request(crate::tr!("Турбо по адресу вне дома не может быть «дома».")))
            }
            Some(kind) => kind,
            None if home => HelperKind::Home,
            None => HelperKind::Own,
        };
        let name = input.name.as_deref().map(helper_name).transpose()?;
        let draft = HelperSetting {
            id: String::new(),
            name: name.unwrap_or_default(),
            url,
            kind,
            enabled: true,
            without: sorted(input.without.iter().copied()),
            helper_id: None,
        };
        let remote = RemoteHelper::new(&draft, key.clone(), &self.config, self.identity.clone(), self.local.clone());
        let status = remote.look().await.map_err(ApiError::bad_gateway)?;
        let named = status.contract.helper.id.clone();
        if !named.is_empty() && self.me().as_deref() == Some(named.as_str()) {
            return Err(ApiError::conflict(crate::tr!("Это сам этот центр: себя в Турбо не добавляют.")));
        }
        let mut draft = draft;
        draft.helper_id = (!named.is_empty()).then_some(named);
        if draft.name.is_empty() {
            // Read cleaned and short already (contract.rs).
            let told = status.contract.helper.name.clone();
            draft.name = if told.is_empty() { crate::tr!("Турбо") } else { told };
        }
        self.listed_already(&draft, &self.snap().list, input.replace.as_deref())?;
        Ok((draft, key, status))
    }

    /// The same address or the same helper is in the list already — the entry «Подключить заново» renews aside, which
    /// must be there.
    fn listed_already(
        &self,
        draft: &HelperSetting,
        list: &[HelperSetting],
        replace: Option<&str>,
    ) -> Result<(), ApiError> {
        if let Some(replace) = replace {
            if !list.iter().any(|entry| entry.id == replace) {
                return Err(no_such_helper());
            }
        }
        let same = list
            .iter()
            .filter(|entry| Some(entry.id.as_str()) != replace)
            .find(|entry| entry.url == draft.url || (draft.helper_id.is_some() && entry.helper_id == draft.helper_id));
        match same {
            Some(entry) => Err(ApiError::conflict(crate::tr!("Это Турбо уже в списке: «{name}».", name = entry.name))),
            None => Ok(()),
        }
    }

    /// What a pairing would do to the list (stage 7): which entry it renews — the one `replace` names, or the one of
    /// the same helper whose pin is the very certificate or name this pairing saw —; none for a new entry. The pair's
    /// door `home-computer` is never one.
    pub fn matched(&self, replace: Option<&str>, paired: &Paired) -> Result<Option<String>, ApiError> {
        let snap = self.snap();
        if let Some(replace) = replace {
            let entry = snap.list.iter().find(|entry| entry.id == replace).ok_or_else(no_such_helper)?;
            if entry.id == HOME_COMPUTER {
                return Err(listed(&entry.name));
            }
            return Ok(Some(entry.id.clone()));
        }
        Ok(snap
            .list
            .iter()
            .filter(|entry| entry.id != HOME_COMPUTER)
            .find(|entry| {
                entry.helper_id.as_deref() == Some(paired.helper.id.as_str())
                    && snap.pins.get(&entry.id).is_some_and(|pin| same_pin(pin, &paired.pin))
            })
            .map(|entry| entry.id.clone()))
    }

    /// What a pairing checks before it spends anything (`link::pair`, before `ok`): the helper is not this gateway
    /// itself, nor the pair's door `home-computer` (D27), nor another entry's helper — unless `replace` names that
    /// entry or the pin is the one stored for it —, and the list and the pins read now (`writable`): the key is never
    /// spent for an entry that could not be stored.
    pub fn pair_check(
        &self,
        replace: Option<String>,
    ) -> impl Fn(&HelperIdentity, &HelperPin) -> Result<(), ApiError> + Send + Sync + 'static {
        let snap = self.snap();
        let me = self.me();
        let settings = self.settings.clone();
        move |helper: &HelperIdentity, pin: &HelperPin| {
            if helper.id.is_empty() {
                return Err(ApiError::bad_gateway(crate::tr!("По этому адресу не Турбо DuoWave.")));
            }
            if me.as_deref() == Some(helper.id.as_str()) {
                return Err(ApiError::conflict(crate::tr!("Это сам этот центр: себя в Турбо не добавляют.")));
            }
            if let Some(replace) = replace.as_deref() {
                let entry = snap.list.iter().find(|entry| entry.id == replace).ok_or_else(no_such_helper)?;
                if entry.id == HOME_COMPUTER {
                    return Err(listed(&entry.name));
                }
            }
            for entry in snap.list.iter().filter(|entry| entry.helper_id.as_deref() == Some(helper.id.as_str())) {
                let same = snap.pins.get(&entry.id).is_some_and(|stored| same_pin(stored, pin));
                if entry.id == HOME_COMPUTER || (replace.as_deref() != Some(entry.id.as_str()) && !same) {
                    return Err(listed(&entry.name));
                }
            }
            Helpers::writable(&settings)
        }
    }

    /// A helper paired over the link, stored: a re-pairing (`matched`) renews that entry's address, key and pin and
    /// keeps its name and «Не брать у него…»; else a new entry, last in the list, whose helper id is fixed. Its id and
    /// the entry it matched. Its link starts, and its first outcome is awaited a moment.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_linked(
        &self,
        url: &str,
        name: Option<String>,
        kind: Option<HelperKind>,
        without: Vec<Ability>,
        replace: Option<String>,
        paired: Paired,
        actor: Option<Actor>,
    ) -> Result<(String, Option<String>), ApiError> {
        let key = paired.key.clone().ok_or_else(|| ApiError::internal("helpers: a pairing without its key"))?;
        let home = is_local_url(Some(url));
        let kind = match kind {
            Some(HelperKind::Home) if !home => {
                return Err(ApiError::bad_request(crate::tr!("Турбо по адресу вне дома не может быть «дома».")))
            }
            Some(kind) => kind,
            None if home => HelperKind::Home,
            None => HelperKind::Own,
        };
        let name = name.as_deref().map(helper_name).transpose()?;
        let matched = self.matched(replace.as_deref(), &paired)?;
        // Read just now by the pairing's check; asked again before the old link's `bye`.
        Helpers::writable(&self.settings)?;
        let id = match &matched {
            Some(id) => {
                // The client its old key was: let go at its helper over the old link.
                self.say_bye(id);
                // Its key first, then its pin, then the entry: never an entry with a pin and no key.
                self.secrets.set(&key_secret(id), Some(&key))?;
                self.set_pin(id, Some(paired.pin.clone()))?;
                self.write(actor.clone(), |list| {
                    let entry = list.iter_mut().find(|entry| entry.id == *id).ok_or_else(no_such_helper)?;
                    entry.url = url.to_string();
                    entry.helper_id = Some(paired.helper.id.clone());
                    if entry.kind == HelperKind::Home && !home {
                        entry.kind = HelperKind::Own;
                    }
                    Ok(Some(()))
                })?;
                tracing::info!("helpers: «{id}» paired again at {url}");
                self.changed(Some(id), "updated", actor);
                id.clone()
            }
            None => {
                let told = clean_name(&paired.helper.name).chars().take(NAME_MAX).collect::<String>();
                let mut draft = HelperSetting {
                    id: String::new(),
                    name: name.unwrap_or(if told.is_empty() { crate::tr!("Турбо") } else { told }),
                    url: url.to_string(),
                    kind,
                    enabled: true,
                    without: sorted(without.iter().copied()),
                    helper_id: Some(paired.helper.id.clone()),
                };
                let id = loop {
                    let id = format!("{}-{}", kind.as_str(), crate::utils::ids::random_hex(4));
                    if self.entry(&id).is_none() {
                        break id;
                    }
                };
                draft.id = id.clone();
                self.secrets.set(&key_secret(&id), Some(&key))?;
                self.set_pin(&id, Some(paired.pin.clone()))?;
                self.write(actor.clone(), |list| {
                    list.push(draft.clone());
                    Ok(Some(()))
                })?;
                tracing::info!("helpers: «{id}» paired at {url}");
                self.changed(Some(&id), "added", actor);
                id
            }
        };
        // What it gives, as the pairing read it; its link comes up in a moment.
        if let Some(remote) = self.entry(&id).and_then(|entry| self.remote(&entry)) {
            remote.linked_seen(&LinkNews::Status(paired.status.clone()));
            if let Some(link) = remote.link() {
                link.settled(Duration::from_secs(5)).await;
            }
        }
        Ok((id, matched))
    }

    /// A pairing's probe as the panel shows it: the helper as it would be listed.
    pub fn paired_view(&self, url: &str, name: Option<&str>, kind: Option<HelperKind>, paired: &Paired) -> HelperView {
        let home = is_local_url(Some(url));
        let kind = kind.unwrap_or(if home { HelperKind::Home } else { HelperKind::Own });
        let told = clean_name(&paired.helper.name).chars().take(NAME_MAX).collect::<String>();
        let draft = HelperSetting {
            id: String::new(),
            name: name.map(clean_name).filter(|name| !name.is_empty()).unwrap_or(told),
            url: url.to_string(),
            kind,
            enabled: true,
            without: Vec::new(),
            helper_id: Some(paired.helper.id.clone()),
        };
        let mut view = self.view_of(&draft, &Snapshot::default(), None);
        view.state = HelperState::Up;
        view.allowed = self.lets(reach_of(&draft));
        view.link = Some(LinkView::of(&paired.pin, Some(LinkState::Up { since: crate::utils::time::now_iso() })));
        if let Some(status) = HelperStatus::read(&paired.status) {
            fill_status(&mut view, &status);
        }
        view
    }

    /// «До сохранения шлюз проверяет…»: the look and the checks of `add`, nothing stored — the helper as it would show.
    pub async fn probe(&self, input: &AddInput) -> Result<HelperView, ApiError> {
        let (draft, _, status) = self.look_first(input).await?;
        let mut view = self.view_of(&draft, &Snapshot::default(), None);
        view.state = HelperState::Up;
        view.allowed = self.lets(reach_of(&draft));
        fill_status(&mut view, &status);
        Ok(view)
    }

    /// A new helper, checked first (its address, its key, what it gives); last in the list. Its id. With `replace`
    /// («Подключить заново») that entry is renewed instead — its address, key and helper; its name, its kind at home and
    /// «Не брать у него…» kept, a link it had let go.
    pub async fn add(&self, input: AddInput, actor: Option<Actor>) -> Result<String, ApiError> {
        let (mut draft, key, status) = self.look_first(&input).await?;
        if let Some(id) = input.replace.clone() {
            // Its pin goes too: nothing changes unless both read.
            Helpers::writable(&self.settings)?;
            self.write(actor.clone(), |list| {
                self.listed_already(&draft, list, Some(&id))?;
                let entry = list.iter_mut().find(|entry| entry.id == id).ok_or_else(no_such_helper)?;
                self.secrets.set(&key_secret(&id), Some(&key))?;
                entry.url = draft.url.clone();
                entry.helper_id = draft.helper_id.clone();
                if entry.kind == HelperKind::Home && reach_of(&draft) != Reach::Home {
                    entry.kind = draft.kind;
                }
                Ok(Some(()))
            })?;
            // A link it had: the client of its old key let go at that helper.
            self.say_bye(&id);
            self.set_pin(&id, None)?;
            if let Some(remote) = self.entry(&id).and_then(|entry| self.remote(&entry)) {
                remote.seen(&Ok(status));
            }
            tracing::info!("helpers: «{id}» renewed at {}", draft.url);
            self.changed(Some(&id), "updated", actor);
            return Ok(id);
        }
        let id = self
            .write(actor.clone(), |list| {
                self.listed_already(&draft, list, None)?;
                let id = loop {
                    let id = format!("{}-{}", draft.kind.as_str(), crate::utils::ids::random_hex(4));
                    if !list.iter().any(|entry| entry.id == id) {
                        break id;
                    }
                };
                draft.id = id.clone();
                // Its key first: an entry is never stored without it.
                self.secrets.set(&key_secret(&id), Some(&key))?;
                list.push(draft.clone());
                Ok(Some(id))
            })?
            .ok_or_else(|| ApiError::internal("helpers: nothing added"))?;
        // It answered just now: on the line already, with what it read. One the regulator keeps out keeps what it
        // declares, not the watch: let out, it gives after a look.
        if let Some(remote) = self.remote(&draft) {
            remote.seen(&Ok(status));
            if self.kept_out(&id) {
                remote.reset();
            }
        }
        tracing::info!("helpers: «{id}» added at {}", draft.url);
        self.changed(Some(&id), "added", actor);
        Ok(id)
    }

    /// The name, the switch, what is not taken from it.
    pub fn update(&self, id: &str, change: Change, actor: Option<Actor>) -> Result<(), ApiError> {
        let name = change.name.as_deref().map(helper_name).transpose()?;
        let mut switched = false;
        let changed = self.write(actor.clone(), |list| {
            let entry = list.iter_mut().find(|entry| entry.id == id).ok_or_else(no_such_helper)?;
            let before = entry.clone();
            if let Some(name) = &name {
                entry.name = name.clone();
            }
            if let Some(on) = change.enabled {
                switched = entry.enabled != on;
                entry.enabled = on;
            }
            if let Some(without) = &change.without {
                entry.without = sorted(without.iter().copied());
            }
            Ok((*entry != before).then_some(()))
        })?;
        if changed.is_none() {
            return Ok(());
        }
        if switched {
            if let Some(remote) = locked(&self.remotes).get(id) {
                remote.reset();
            }
        }
        self.changed(Some(id), "updated", actor);
        Ok(())
    }

    /// The order of the list: every stored id once.
    pub fn order(&self, ids: &[String], actor: Option<Actor>) -> Result<(), ApiError> {
        let wrong = || ApiError::bad_request(crate::tr!("ids: каждое Турбо списка по одному разу."));
        self.write(actor.clone(), |list| {
            let mut asked: Vec<&String> = ids.iter().collect();
            asked.sort();
            asked.dedup();
            let mut stored: Vec<&String> = list.iter().map(|entry| &entry.id).collect();
            stored.sort();
            if asked.len() != ids.len() || asked != stored {
                return Err(wrong());
            }
            let mut ordered = Vec::with_capacity(list.len());
            for id in ids {
                if let Some(at) = list.iter().position(|entry| entry.id == *id) {
                    ordered.push(list.remove(at));
                }
            }
            *list = ordered;
            Ok(Some(()))
        })?;
        self.changed(None, "order", actor);
        Ok(())
    }

    /// The entry and its key; for the pair's door also the old setting and its key (the mirror). A linked helper
    /// hears `bye` first (it forgets this home), and its pin goes — nothing goes unless the list and the pins read.
    pub fn remove(&self, id: &str, actor: Option<Actor>) -> Result<(), ApiError> {
        Helpers::writable(&self.settings)?;
        self.write(actor.clone(), |list| {
            let at = list.iter().position(|entry| entry.id == id).ok_or_else(no_such_helper)?;
            list.remove(at);
            Ok(Some(()))
        })?;
        self.say_bye(id);
        self.set_pin(id, None)?;
        self.secrets.delete(&key_secret(id))?;
        tracing::info!("helpers: «{id}» removed");
        self.changed(Some(id), "removed", actor);
        Ok(())
    }

    /// The regulator.
    pub fn set_away(&self, away: Away, actor: Option<Actor>) -> Result<(), ApiError> {
        let before = {
            let _writing = locked(&self.writing);
            self.settings.set(&AWAY_SETTING, &away, actor.clone())?;
            let before = self.snap();
            let (list, pins) = (before.list.clone(), before.pins.clone());
            *self.snapshot.write().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Arc::new(Snapshot { list, away, pins });
            before
        };
        self.forget_crossed(&before);
        tracing::info!("helpers: the regulator is at «{}»", serde_json::to_value(away).unwrap_or_default());
        self.events.emit(GatewayEvent::HelpersChanged { id: None, change: "away".into(), actor });
        // A link to a helper the regulator keeps out closes; one it lets out again opens.
        self.links();
        self.settle();
        Ok(())
    }

    /// One helper as the panel shows it; `twin` names the earlier entry it is the same as.
    fn view_of(&self, entry: &HelperSetting, snap: &Snapshot, twin: Option<&HelperSetting>) -> HelperView {
        let remote = (!entry.id.is_empty()).then(|| self.remote(entry)).flatten();
        let allowed = snap.away.lets(reach_of(entry));
        let mut view = HelperView {
            id: entry.id.clone(),
            name: entry.name.clone(),
            kind: entry.kind,
            builtin: false,
            url: Some(entry.url.clone()),
            enabled: entry.enabled,
            without: entry.without.clone(),
            key: (!entry.id.is_empty()).then(|| self.secrets.mask(&key_secret(&entry.id))),
            state: HelperState::Unknown,
            since: None,
            error: None,
            allowed,
            contract: None,
            helper: None,
            version: None,
            abilities: Vec::new(),
            link: snap.pins.get(&entry.id).map(|pin| LinkView::of(pin, None)),
        };
        let Some(remote) = remote else {
            if !entry.id.is_empty() {
                view.error = Some(crate::tr!("нет ключа Турбо"));
            }
            return view;
        };
        if let Some(status) = remote.status() {
            fill_status(&mut view, &status);
        }
        if let Some(pin) = remote.pin() {
            view.link = Some(LinkView::of(pin, remote.link_state()));
        }
        if let Some(earlier) = twin {
            view.state = HelperState::Twin;
            view.error = Some(crate::tr!("то же Турбо, что «{name}»", name = earlier.name));
        } else if self.is_self(entry) {
            view.state = HelperState::Itself;
        } else if !entry.enabled {
            view.state = HelperState::Off;
        } else if allowed {
            view.state = remote.state();
            view.since = remote.since();
            view.error = remote.error().map(|error| crate::core::tx_dyn(&error));
        }
        view
    }

    /// Every entry as the panel shows it, in the list's order — on a computer that takes its own parts from itself,
    /// after its built-in helper.
    pub fn views(&self) -> Vec<HelperView> {
        let snap = self.snap();
        let own = self.own.get().map(|own| own_view(own.as_ref()));
        own.into_iter()
            .chain(snap.list.iter().map(|entry| self.view_of(entry, &snap, self.twin_of(entry, &snap))))
            .collect()
    }

    /// `/health` `placement.helpers`: the regulator and each helper in brief — what it declares and is not kept from,
    /// and whether the regulator lets it out (`allowed`, judged as the gateway judges it: by its kind and its address).
    pub fn health(&self) -> Value {
        let list: Vec<Value> = self
            .views()
            .into_iter()
            .map(|view| {
                let abilities: Abilities = view
                    .abilities
                    .iter()
                    .map(|ability| ability.id)
                    .filter(|ability| !view.without.contains(ability))
                    .collect();
                let mut brief = json!({
                    "id": view.id, "name": view.name, "kind": view.kind, "enabled": view.enabled,
                    "state": view.state, "allowed": view.allowed, "abilities": abilities,
                });
                if view.builtin {
                    brief["builtin"] = Value::Bool(true);
                }
                if let Some(since) = view.since {
                    brief["since"] = Value::String(since);
                }
                brief
            })
            .collect();
        json!({ "away": self.away(), "list": list })
    }

    /// `/health` `placement.homeServer`, the old shape a 0.11 computer reads: the pair's door, while its key is stored.
    pub fn pair_view(&self) -> Option<Value> {
        let entry = self.entry(HOME_COMPUTER)?;
        if !self.secrets.has(&key_secret(HOME_COMPUTER)) {
            return None;
        }
        let remote = self.remote(&entry);
        let state = if !entry.enabled {
            HomeServer::None
        } else if self.door_up(&entry) {
            HomeServer::Up
        } else {
            HomeServer::Down
        };
        Some(json!({
            "url": entry.url,
            "enabled": entry.enabled,
            "state": state,
            "since": remote.as_ref().and_then(|remote| remote.since()),
            "error": remote.as_ref().and_then(|remote| remote.error()).map(|error| crate::core::tx_dyn(&error)),
            "without": door::to_legacy(&entry.without),
        }))
    }
}

/// What a contract declares and how ready, for «what it declares changed».
fn declared_of(contract: &Contract) -> (String, Vec<(Ability, bool)>) {
    (contract.helper.id.clone(), contract.abilities.iter().map(|state| (state.id, state.ready)).collect())
}

fn no_such_helper() -> ApiError {
    ApiError::not_found(crate::tr!("Нет такого Турбо."))
}

fn listed(name: &str) -> ApiError {
    ApiError::conflict(crate::tr!("Это Турбо уже в списке: «{name}».", name = name))
}

/// A helper's address as stored: http or https, a host, no query or fragment, no trailing `/`.
fn helper_url(text: &str) -> Result<String, ApiError> {
    let bad = || ApiError::bad_request(crate::tr!("Адрес Турбо: http://адрес:порт или https://…"));
    let parsed = url::Url::parse(text.trim()).map_err(|_| bad())?;
    let host = parsed.host_str().filter(|host| !host.is_empty()).ok_or_else(bad)?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(bad());
    }
    let port = parsed.port().map(|port| format!(":{port}")).unwrap_or_default();
    Ok(format!("{}://{host}{port}{}", parsed.scheme(), parsed.path().trim_end_matches('/')))
}

/// A helper's name as a person gave it: cleaned, 1–60 characters.
fn helper_name(text: &str) -> Result<String, ApiError> {
    let name = clean_name(text);
    if name.is_empty() || name.chars().count() > NAME_MAX {
        return Err(ApiError::bad_request(crate::tr!("Имя Турбо: от 1 до 60 знаков.")));
    }
    Ok(name)
}

/// The built-in helper's row (stage 6): this computer itself, on the line, always first — no address, no key, and
/// nothing to move, rename, switch off or remove.
fn own_view(own: &dyn OwnParts) -> HelperView {
    let contract = own.contract();
    HelperView {
        id: SELF.to_string(),
        name: crate::tr!("Этот компьютер"),
        kind: HelperKind::Home,
        builtin: true,
        url: None,
        enabled: true,
        without: Vec::new(),
        key: None,
        state: HelperState::Up,
        since: None,
        error: None,
        allowed: true,
        contract: Some(contract.version),
        helper: (!contract.helper.id.is_empty()).then_some(contract.helper),
        version: None,
        abilities: contract.abilities,
        link: None,
    }
}

/// What a helper's last status said, into its view.
fn fill_status(view: &mut HelperView, status: &HelperStatus) {
    view.contract = Some(status.contract.version);
    view.helper = (!status.contract.helper.id.is_empty()).then(|| status.contract.helper.clone());
    view.version = status.program.clone();
    view.abilities = status.contract.abilities.clone();
}

/// A helper as the panel shows it (`GET /api/v1/helpers`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperView {
    pub id: String,
    pub name: String,
    pub kind: HelperKind,
    /// This computer itself (stage 6).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub builtin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub enabled: bool,
    pub without: Vec<Ability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<SecretMask>,
    pub state: HelperState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The regulator lets it out.
    pub allowed: bool,
    /// The contract's version it answered with: 0 or 1; absent — not read yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub helper: Option<HelperIdentity>,
    /// Its program's version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// What it declares, as last read.
    pub abilities: Vec<AbilityState>,
    /// A linked helper's link (stage 7).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<LinkView>,
}

/// A linked helper's link as the panel shows it: its state, since when, why away, and what was pinned.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LinkView {
    /// `connecting`, `up` or `down`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `sha256:<64 hex>` or `public:<host>`.
    pub pin: String,
}

impl LinkView {
    pub fn of(pin: &HelperPin, state: Option<LinkState>) -> LinkView {
        let pin = match (&pin.sha256, &pin.public) {
            (Some(hash), _) => format!("sha256:{hash}"),
            (None, Some(host)) => format!("public:{host}"),
            (None, None) => String::new(),
        };
        let (state, since, reason) = match state {
            None | Some(LinkState::Connecting) => ("connecting", None, None),
            Some(LinkState::Up { since }) => ("up", Some(since), None),
            Some(LinkState::Down { since, reason }) => ("down", Some(since), Some(crate::core::tx_dyn(&reason))),
        };
        LinkView { state, since, reason, pin }
    }
}

/// Two pins of the same certificate or name.
pub fn same_pin(a: &HelperPin, b: &HelperPin) -> bool {
    a.sha256 == b.sha256 && a.public == b.public
}

/// The way of one ability to a helper, for the plugins, the history and the catalog.
struct AbilityLink {
    helpers: Weak<Helpers>,
    ability: Ability,
}

impl crate::plugins::controller::HomeLink for AbilityLink {
    fn up(&self) -> Option<(String, String)> {
        let remote = self.helpers.upgrade()?.pick_remote(self.ability)?;
        Some((remote.base(), remote.credential()))
    }

    fn offered(&self) -> bool {
        self.helpers.upgrade().is_some_and(|helpers| matches!(helpers.offer(self.ability), Some(Giver::Helper(_))))
    }
}

/// `/health`: the old `homeServer` of the pair's door, and the helpers.
impl crate::placement::service::HomeServerView for Helpers {
    fn view(&self) -> Option<Value> {
        self.pair_view()
    }

    fn helpers(&self) -> Option<Value> {
        Some(self.health())
    }
}

/// Our voice through the helpers (src/speech/voice_service.rs): the first that gives it; the regulator for this
/// gateway's own cloud voice; a station leans on them for its voice, a computer never.
impl crate::speech::voice_service::HomeVoice for Helpers {
    fn speech(&self) -> Option<Arc<OwnSpeech>> {
        match self.pick(Ability::Voice)? {
            Giver::Helper(remote) => Some(remote.speech()),
            Giver::Itself => None,
        }
    }

    fn allows(&self, reach: Reach) -> bool {
        self.lets(reach)
    }

    fn leans(&self) -> bool {
        self.profile.station
    }
}

/// The unit tests of the helpers, and the stand-ins other modules' tests share: a helper's `/home/v1`, a station.
#[cfg(test)]
pub(crate) mod tests;
