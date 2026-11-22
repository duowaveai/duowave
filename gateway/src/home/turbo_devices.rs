//! The home's devices (docs/plans/turbo-devices.md): what is plugged into this gateway and into every Турбо that helps
//! it, as one set of devices of the home. «Комп включился — у шлюза доступ ко всему этому» (owner 20.11): a Турбо that
//! comes on the line brings its MIDI keyboard, its screen, its flash drives with it, the centre sees them on the bus as
//! `device/<kind>`, and an app, the workshop and the brain reach them there by the same links and rules as anything at
//! home.
//!
//! - **One interface, kinds as implementations.** A computer's device is an `impl Device`
//!   (src/bus/providers/devices.rs); a Турбо gives its home what it has in its contract (`devices`, contract 3) and
//!   answers letters at `/home/v1/devices/<kind>/call` (src/home_server/devices.rs). The centre needs nothing per kind.
//! - **A kind is the address, the place a property** (the bus's decision 6): an app asks for `listen:device/midi`, not
//!   «the Mac's keyboard»; its links stay when the keyboard moves to another computer. A thing's id names its place —
//!   `home-computer:in-2`; this gateway's own stay bare.
//! - **Look everywhere, act at one place** (owner 20.11, 1): what only sees or hears goes to every place that has the
//!   kind; what does something goes to one — the one named, the one used last by this ear, the first in the Турбо's
//!   order — and the choice is said aloud at its first time and when it changes (owner 20.11, 2).
//! - **Things at hand only at home** (owner 20.11, 6): a Турбо reached from outside the home gives its folders, not its
//!   keyboard or its screen.
//! - **Remembered**: what the centre saw at each Турбо stays in `house_devices` (migration 0017), so a sleeping Mac's
//!   keyboard is «у Mac, а он спит», not «нет клавиатуры», and the workshop makes a teacher for it anyway (owner 20.11,
//!   5). A sleeping Турбо is woken over the home network on a person's «да» (src/helpers/wake.rs).
//! - **The centre decides the rights**: the person, the app's links, the «да». A Турбо only checks that the home is its
//!   own and at home, and what it gives («Что этот компьютер даёт дому», `turbo_shares`).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use futures::future::BoxFuture;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::bus::provider::{BusError, Handle};
use crate::bus::providers::devices::{Device, Devices, Instance, Nature};
use crate::bus::Level;
use crate::helpers::contract::Ability;
use crate::helpers::{HelperKind, Helpers};
use crate::settings::definitions::TurboShares;
use crate::storage::Db;
use crate::utils::time::now_ms;

/// This gateway's own place.
pub const HERE: &str = "";
/// The kinds the home knows by name, in the order people see them; a newer Турбо's other kinds follow.
pub const KINDS: [&str; 3] = ["midi", "screen", "storage"];
/// A sleeping Турбо has this long to come on the line after it was woken from the panel.
pub const WAKE_WAIT: Duration = Duration::from_secs(20);
/// …and this long by voice: a station's answer cannot wait longer.
pub const VOICE_WAKE_WAIT: Duration = Duration::from_secs(8);
/// While it wakes, its status is asked this often.
const WAKE_LOOK: Duration = Duration::from_secs(2);
/// A letter to a Турбо's device waits this long for its answer.
const CALL_WAIT: Duration = Duration::from_secs(15);

/// One handle of a kind as a Турбо gives it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GivenHandle {
    pub name: String,
    pub level: Level,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// One kind as a Турбо gives it: its contract's `devices.details.kinds[]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GivenKind {
    pub kind: String,
    pub title: String,
    pub nature: Nature,
    #[serde(default)]
    pub handles: Vec<GivenHandle>,
    #[serde(default)]
    pub instances: Vec<Instance>,
}

impl GivenKind {
    /// What this gateway gives of one of its devices.
    pub fn of(device: &dyn Device) -> GivenKind {
        GivenKind {
            kind: device.name().to_string(),
            title: device.title(),
            nature: device.nature(),
            handles: device
                .handles()
                .into_iter()
                .map(|handle| GivenHandle { name: handle.name, level: handle.level, title: handle.title })
                .collect(),
            instances: device.instances(),
        }
    }
}

/// A Турбо as the home's devices read it.
#[derive(Debug, Clone, Default)]
pub struct TurboAt {
    /// Its id in the setting `helpers`.
    pub id: String,
    /// The computer at that entry, as its contract names itself (`contract.helper.id`); '' while not known. An entry
    /// may pass to another computer (several Турбо, 0.18.2): what is remembered of it goes with the computer.
    pub computer: String,
    pub name: String,
    pub cloud: bool,
    /// On the line and giving its devices now.
    pub up: bool,
    /// Reached from home, by its home address — not over the link: a thing at hand is the home's only so.
    pub home: bool,
    /// What it gives now, as its last status said.
    pub kinds: Vec<GivenKind>,
    /// It gives its folders (`files`).
    pub files: bool,
    /// Its hardware addresses for a wake over the home network.
    pub macs: Vec<String>,
}

/// What the home's devices read of the Турбо and ask of them: the helpers in the gateway, a stand-in in the tests.
pub trait TurboSource: Send + Sync {
    /// Every Турбо that may give devices, in the order of the list, on the line or not.
    fn turbos(&self) -> Vec<TurboAt>;
    /// A letter to one of its devices: `POST /home/v1/devices/<kind>/call`.
    fn call(&self, turbo: &str, kind: &str, handle: &str, input: Value) -> BoxFuture<'static, Result<Value, String>>;
    /// Looks at it now: whether it is on the line.
    fn look(&self, turbo: &str) -> BoxFuture<'static, bool>;
    /// Its notes' channel, when it is reached at its home address (the link carries no WebSocket): the address and the
    /// key (src/midi/relay.rs).
    fn stream(&self, _turbo: &str) -> Option<(String, String)> {
        None
    }
    /// A body posted to one of its routes (`/home/v1/devices/midi/read`), its answer back.
    fn post(&self, _turbo: &str, _path: &str, _body: Vec<u8>) -> BoxFuture<'static, Result<Value, String>> {
        Box::pin(async { Err(crate::tr!("Турбо не ответило.")) })
    }
    /// A letter to its home folder (`/home/v1/files/call`): a folder's list, a file read, put, removed.
    fn files(
        &self,
        _turbo: &str,
        _to: &str,
        _handle: &str,
        _input: Value,
    ) -> BoxFuture<'static, Result<Value, BusError>> {
        Box::pin(async { Err(BusError::Failed(crate::tr!("Турбо не ответило."))) })
    }
    /// Where its files are fetched from by the engine's key (`/media/v/<key>`): its home address; none over the link.
    fn media_base(&self, _turbo: &str) -> Option<String> {
        None
    }
}

/// A key of a Турбо's engine as the home names it: the centre's `/media/v/<this>` sends the fetch to the Турбо.
pub const TURBO_KEY: &str = "turbo.";

/// Where one place stands for one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaceState {
    /// It has the kind and can be reached now.
    Ready,
    /// On the line and at home, but nothing of the kind is plugged in now (it was before).
    Unplugged,
    /// Not on the line: asleep, switched off or out of the network.
    Asleep,
    /// On the line from outside the home: a thing at hand is not the home's now.
    Away,
}

/// One place of one kind, as people see it: «Keystation 49 · Mac».
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceOf {
    /// '' — this gateway; a Турбо's id.
    pub place: String,
    pub name: String,
    pub cloud: bool,
    pub state: PlaceState,
    /// The things plugged in there — ids as the home names them (`<place>:<id>`, bare for this gateway); remembered
    /// ones while the place is not ready.
    pub instances: Vec<Instance>,
}

/// One kind of the home with its places.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KindView {
    pub kind: String,
    pub title: String,
    pub nature: Nature,
    pub places: Vec<PlaceOf>,
}

/// Where a kind is in the home: what the workshop says when it makes a thing for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Whereabouts {
    /// At this gateway itself.
    Here,
    /// At a Турбо on the line and at home, plugged in now.
    Turbo(String),
    /// At a Турбо on the line and at home, nothing of it plugged in now.
    Unplugged(String),
    /// At a Турбо that sleeps.
    Asleep(String),
    /// At a Турбо that is not at home now.
    Away(String),
    /// Never seen, though the home has Турбо.
    Never,
    /// The home has no Турбо to plug it into.
    NoTurbo,
}

/// Where a kind's act goes, and what is said of the choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub place: String,
    pub name: String,
    /// Said at the first choice among several places and when the choice changes (owner 20.11, 2).
    pub said: Option<String>,
}

/// A device's id as the home names it: `<place>:<id>`, bare for this gateway's own.
pub fn home_id(place: &str, id: &str) -> String {
    if place == HERE {
        id.to_string()
    } else {
        format!("{place}:{id}")
    }
}

/// A device's id as the home names it, split: (place, the id at that place). A bare id is this gateway's — unless a
/// Турбо's id is known by that prefix.
pub fn split_id<'a>(id: &'a str, places: &[String]) -> (&'a str, &'a str) {
    match id.split_once(':') {
        Some((place, rest)) if places.iter().any(|known| known == place) => (place, rest),
        _ => (HERE, id),
    }
}

/// How a wake is sent: the home network's broadcast (src/helpers/wake.rs), a stand-in in the tests.
pub type Waker = Arc<dyn Fn(Vec<String>) -> BoxFuture<'static, Result<usize, String>> + Send + Sync>;

/// The home's devices.
pub struct HouseDevices {
    /// This gateway's own devices.
    local: Arc<Devices>,
    source: OnceLock<Arc<dyn TurboSource>>,
    db: Option<Arc<Db>>,
    /// What this computer gives as a Турбо: read from its settings.
    shares: OnceLock<Arc<dyn Fn() -> TurboShares + Send + Sync>>,
    /// The name this gateway goes by in the words («Mac»).
    name_here: Mutex<String>,
    /// Wakes in progress: a second «да» does not send again.
    waking: Mutex<BTreeSet<String>>,
    waker: OnceLock<Waker>,
}

impl HouseDevices {
    pub fn new(local: Arc<Devices>, db: Option<Arc<Db>>) -> Arc<HouseDevices> {
        Arc::new(HouseDevices {
            local,
            source: OnceLock::new(),
            db,
            shares: OnceLock::new(),
            name_here: Mutex::new(crate::tr!("этот компьютер")),
            waking: Mutex::new(BTreeSet::new()),
            waker: OnceLock::new(),
        })
    }

    /// How wakes are sent (tests: a stand-in).
    pub fn set_waker(&self, waker: Waker) {
        let _ = self.waker.set(waker);
    }

    /// The Турбо as the gateway knows them.
    pub fn set_source(&self, source: Arc<dyn TurboSource>) {
        let _ = self.source.set(source);
    }

    /// What this computer gives its home as a Турбо.
    pub fn set_shares(&self, shares: Arc<dyn Fn() -> TurboShares + Send + Sync>) {
        let _ = self.shares.set(shares);
    }

    pub fn set_name_here(&self, name: &str) {
        if !name.trim().is_empty() {
            *self.name_here.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = name.trim().to_string();
        }
    }

    fn name_here(&self) -> String {
        self.name_here.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// This gateway's own devices: what it gives its home when it is a Турбо.
    pub fn local(&self) -> &Arc<Devices> {
        &self.local
    }

    /// What this computer gives its home now (its contract's `devices`): its own devices it shares, a sensitive one
    /// only when ticked.
    pub fn given(&self) -> Vec<GivenKind> {
        let shares = self.shares.get().map(|shares| shares()).unwrap_or_default();
        self.local
            .list()
            .iter()
            .filter(|device| shares.gives(device.name(), device.sensitive()))
            .map(|device| GivenKind::of(device.as_ref()))
            .collect()
    }

    fn turbos(&self) -> Vec<TurboAt> {
        self.source.get().map(|source| source.turbos()).unwrap_or_default()
    }

    /// The kinds the home knows: this gateway's, its Турбо's, the remembered ones — the known names first.
    pub fn kinds(&self) -> Vec<String> {
        let mut found: BTreeSet<String> = self.local.list().iter().map(|device| device.name().to_string()).collect();
        for turbo in self.turbos() {
            found.extend(turbo.kinds.iter().map(|kind| kind.kind.clone()));
        }
        found.extend(self.remembered_kinds());
        let mut kinds: Vec<String> =
            KINDS.iter().filter(|kind| found.contains(**kind)).map(|kind| kind.to_string()).collect();
        kinds.extend(found.into_iter().filter(|kind| !KINDS.contains(&kind.as_str())));
        kinds
    }

    /// The places of a kind: this gateway first, then the Турбо in the list's order; each as it stands now.
    pub fn places(&self, kind: &str) -> Vec<PlaceOf> {
        let mut places = Vec::new();
        if let Some(device) = self.local.get(kind) {
            places.push(PlaceOf {
                place: HERE.into(),
                name: self.name_here(),
                cloud: false,
                state: PlaceState::Ready,
                instances: device.instances(),
            });
        }
        for turbo in self.turbos() {
            let given = turbo.kinds.iter().find(|given| given.kind == kind);
            let remembered = self.remembered(&turbo, kind);
            if given.is_none() && remembered.is_empty() {
                continue;
            }
            let nature = given.map(|given| given.nature).unwrap_or_else(|| self.nature_of(kind));
            let plugged: Vec<Instance> = given.map(|given| given.instances.clone()).unwrap_or_default();
            let state = if !turbo.up {
                PlaceState::Asleep
            } else if nature == Nature::Physical && !turbo.home {
                PlaceState::Away
            } else if given.is_none() || (plugged.is_empty() && nature == Nature::Physical) {
                PlaceState::Unplugged
            } else {
                PlaceState::Ready
            };
            let shown = if plugged.is_empty() { remembered } else { plugged };
            places.push(PlaceOf {
                instances: shown
                    .into_iter()
                    .map(|instance| Instance { id: home_id(&turbo.id, &instance.id), title: instance.title })
                    .collect(),
                place: turbo.id,
                name: turbo.name,
                cloud: turbo.cloud,
                state,
            });
        }
        places
    }

    /// Every kind with its places, for the panel («Дом» → «Устройства») and the brain.
    pub fn view(&self) -> Vec<KindView> {
        self.kinds()
            .into_iter()
            .map(|kind| KindView {
                title: self.title_of(&kind),
                nature: self.nature_of(&kind),
                places: self.places(&kind),
                kind,
            })
            .filter(|view| !view.places.is_empty())
            .collect()
    }

    /// Whether the home has anything to tell of: a thing plugged in somewhere, or a Турбо with a kind (asleep too). A
    /// computer whose build has MIDI but no keyboard plugged in has nothing.
    pub fn anything(&self) -> bool {
        self.view()
            .iter()
            .any(|kind| kind.places.iter().any(|place| place.place != HERE || !place.instances.is_empty()))
    }

    /// A kind's name for people: this gateway's device's, else a Турбо's.
    pub fn title_of(&self, kind: &str) -> String {
        if let Some(device) = self.local.get(kind) {
            return device.title();
        }
        self.turbos()
            .iter()
            .flat_map(|turbo| turbo.kinds.iter())
            .find(|given| given.kind == kind)
            .map(|given| given.title.clone())
            .unwrap_or_else(|| match kind {
                "midi" => crate::tr!("MIDI-клавиатура"),
                "screen" => crate::tr!("Экран компьютера"),
                "storage" => crate::tr!("Флешки и диски"),
                other => other.to_string(),
            })
    }

    /// A kind's nature: this gateway's device's, else a Турбо's; a thing at hand when nobody says.
    pub fn nature_of(&self, kind: &str) -> Nature {
        if let Some(device) = self.local.get(kind) {
            return device.nature();
        }
        self.turbos()
            .iter()
            .flat_map(|turbo| turbo.kinds.iter())
            .find(|given| given.kind == kind)
            .map(|given| given.nature)
            .unwrap_or(Nature::Physical)
    }

    /// What a kind can do: this gateway's device's handles and every Турбо's, each name once; a kind known by name
    /// keeps its handles while every place of it sleeps — its letters then say why not.
    pub fn handles_of(&self, kind: &str) -> Vec<Handle> {
        let mut handles: Vec<Handle> = self.local.get(kind).map(|device| device.handles()).unwrap_or_default();
        for known in known_handles(kind) {
            if !handles.iter().any(|had| had.name == known.name) {
                handles.push(known);
            }
        }
        for turbo in self.turbos() {
            for given in turbo.kinds.iter().filter(|given| given.kind == kind) {
                for handle in &given.handles {
                    if !handles.iter().any(|known| known.name == handle.name) {
                        let mut made = Handle::new(&handle.name, handle.level);
                        if let Some(title) = &handle.title {
                            made = made.titled(title);
                        }
                        handles.push(made);
                    }
                }
            }
        }
        handles
    }

    /// Whether the home knows the kind at all: here, at a Турбо, or remembered.
    pub fn knows(&self, kind: &str) -> bool {
        self.local.get(kind).is_some()
            || self.turbos().iter().any(|turbo| turbo.kinds.iter().any(|given| given.kind == kind))
            || self.remembered_kinds().iter().any(|known| known == kind)
    }

    /// Where a kind is in the home: here, at a Турбо that has it now, at one that sleeps or is away, or nowhere.
    pub fn whereabouts(&self, kind: &str) -> Whereabouts {
        let places = self.places(kind);
        if places.iter().any(|place| place.place == HERE) {
            return Whereabouts::Here;
        }
        let first =
            |state: PlaceState| places.iter().find(|place| place.state == state).map(|place| place.name.clone());
        if let Some(name) = first(PlaceState::Ready) {
            return Whereabouts::Turbo(name);
        }
        if let Some(name) = first(PlaceState::Unplugged) {
            return Whereabouts::Unplugged(name);
        }
        if let Some(name) = first(PlaceState::Away) {
            return Whereabouts::Away(name);
        }
        if let Some(name) = first(PlaceState::Asleep) {
            return Whereabouts::Asleep(name);
        }
        match self.turbos().first() {
            Some(turbo) if turbo.up && turbo.home => Whereabouts::Unplugged(turbo.name.clone()),
            Some(_) => Whereabouts::Never,
            None => Whereabouts::NoTurbo,
        }
    }

    /// Whether the home's folders reach a Турбо's (`files`): the first one on the line that gives them.
    pub fn folders_at_turbo(&self) -> Option<String> {
        self.turbos().into_iter().find(|turbo| turbo.up && turbo.files).map(|turbo| turbo.name)
    }

    /// Why nothing of a kind can be reached now, in words: where it is and what to do.
    pub fn why_none(&self, kind: &str) -> String {
        let title = self.title_of(kind);
        match self.whereabouts(kind) {
            Whereabouts::Here | Whereabouts::Turbo(_) => crate::tr!("«{title}» сейчас не отвечает.", title = title),
            Whereabouts::Unplugged(name) => unplugged(&title, &name),
            Whereabouts::Asleep(name) => asleep(&title, &name),
            Whereabouts::Away(name) => away(&title, &name),
            Whereabouts::Never => {
                crate::tr!("«{title}» в доме пока не видно: подключите к компьютеру с DuoWave.", title = title)
            }
            Whereabouts::NoTurbo => crate::tr!(
                "«{title}» подключают к компьютеру с DuoWave, а в доме его нет: подключите компьютер к дому как Турбо.",
                title = title
            ),
        }
    }

    /// The sleeping Турбо that has a kind, when one does and can be woken: (id, name).
    pub fn sleeping_with(&self, kind: &str) -> Option<(String, String)> {
        if self.places(kind).iter().any(|place| matches!(place.state, PlaceState::Ready)) {
            return None;
        }
        self.places(kind)
            .into_iter()
            .find(|place| place.state == PlaceState::Asleep && !place.cloud && !self.macs_of(&place.place).is_empty())
            .map(|place| (place.place, place.name))
    }

    /// A letter to one place of a kind: this gateway's own device, or the Турбо's (its ids without the place).
    pub async fn call_at(&self, kind: &str, place: &str, handle: &str, input: Value) -> Result<Value, BusError> {
        if place == HERE {
            let device = self.local.get(kind).ok_or_else(|| BusError::NotFound(format!("device/{kind}")))?;
            return device.call(handle, input).await;
        }
        let source = self.source.get().cloned().ok_or_else(|| BusError::Failed(self.why_none(kind)))?;
        let answer = source.call(place, kind, handle, input).await.map_err(BusError::Failed)?;
        Ok(with_place(answer, place))
    }

    /// A Турбо's notes' channel for the relay: its home address and key; none over the link.
    pub fn stream_of(&self, place: &str) -> Option<(String, String)> {
        self.source.get()?.stream(place)
    }

    /// The Турбо whose folders are the home's now: on the line and giving its folders — from home or from afar, folders
    /// are data (owner 20.11, 6): (id, name), in the list's order.
    pub fn folder_places(&self) -> Vec<(String, String)> {
        self.turbos().into_iter().filter(|turbo| turbo.files).map(|turbo| (turbo.id, turbo.name)).collect()
    }

    /// A letter to a Турбо's home folder.
    pub async fn files_at(&self, place: &str, to: &str, handle: &str, input: Value) -> Result<Value, BusError> {
        let source = self.source.get().cloned().ok_or_else(|| BusError::Failed(crate::tr!("Турбо в доме нет.")))?;
        source.files(place, to, handle, input).await
    }

    /// Where a Турбо's file is fetched by its engine's key: its home address — the centre sends the fetch there.
    pub fn media_url(&self, place: &str, key: &str) -> Option<String> {
        let base = self.source.get()?.media_base(place)?;
        Some(format!("{}/media/v/{key}", base.trim_end_matches('/')))
    }

    /// A `.mid` read by a Турбо that has MIDI (a centre whose build has none): its tracks.
    pub async fn read_midi(&self, bytes: Vec<u8>) -> Result<Value, String> {
        let source = self.source.get().cloned().ok_or_else(|| self.why_none("midi"))?;
        let place =
            self.ready("midi").into_iter().find(|place| place.place != HERE).ok_or_else(|| self.why_none("midi"))?;
        source.post(&place.place, "/home/v1/devices/midi/read", bytes).await
    }

    /// The places a kind's sight and hearing go to: every one ready now.
    pub fn ready(&self, kind: &str) -> Vec<PlaceOf> {
        self.places(kind).into_iter().filter(|place| place.state == PlaceState::Ready).collect()
    }

    /// Where a kind's act goes for this ear: the only place ready; else the one this ear chose last, else the one
    /// used last, else the first. Said aloud at the first choice among several and when it changes.
    pub fn choose(&self, kind: &str, ear: &str) -> Result<Choice, BusError> {
        let ready = self.ready(kind);
        let Some(first) = ready.first() else { return Err(BusError::Failed(self.why_none(kind))) };
        if ready.len() == 1 {
            let place = first.place.clone();
            self.store_choice(kind, ear, &place, false);
            return Ok(Choice { place, name: first.name.clone(), said: None });
        }
        let before = self.choice_of(kind, ear);
        let remembered = before.as_ref().filter(|(place, _)| ready.iter().any(|ready| ready.place == *place));
        let (chosen, reason) = match remembered {
            Some((place, _)) => (place.clone(), Reason::Chosen),
            None => match self.last_used(kind, &ready) {
                Some(place) => (place, Reason::Used),
                None => (first.place.clone(), Reason::First),
            },
        };
        let said_before = before.as_ref().is_some_and(|(place, said)| *place == chosen && *said);
        let name = ready.iter().find(|place| place.place == chosen).map(|place| place.name.clone()).unwrap_or_default();
        let said = (!said_before).then(|| choice_words(&self.title_of(kind), &ready, &name, reason));
        self.store_choice(kind, ear, &chosen, said.is_some() || said_before);
        Ok(Choice { place: chosen, name, said })
    }

    /// «На ПК»: the person chose a place for a kind at this ear. The place by its id or a part of its name.
    pub fn set_choice(&self, kind: &str, ear: &str, named: &str) -> Result<Choice, BusError> {
        let named = named.trim().to_lowercase();
        let places = self.places(kind);
        let found = places
            .iter()
            .find(|place| place.place.to_lowercase() == named)
            .or_else(|| places.iter().find(|place| !named.is_empty() && place.name.to_lowercase().contains(&named)))
            .ok_or_else(|| {
                BusError::Failed(crate::tr!("Такого места для «{title}» нет.", title = self.title_of(kind)))
            })?;
        self.store_choice(kind, ear, &found.place, true);
        Ok(Choice { place: found.place.clone(), name: found.name.clone(), said: None })
    }

    /// The Турбо's devices seen now, kept: what is plugged into each, its wake addresses. Called on the helpers'
    /// changes and by the keeper.
    pub fn remember(&self) {
        let Some(db) = &self.db else { return };
        let now = now_ms() as i64;
        let turbos = self.turbos();
        let result = db.transaction(|tx| {
            for turbo in &turbos {
                if !turbo.macs.is_empty() {
                    tx.execute(
                        "INSERT INTO turbo_wake (helper, computer, macs, updated_at) VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT (helper) DO UPDATE SET computer = excluded.computer, macs = excluded.macs,
                         updated_at = excluded.updated_at",
                        params![turbo.id, turbo.computer, json!(turbo.macs).to_string(), now],
                    )?;
                }
                if !turbo.up {
                    continue;
                }
                for given in &turbo.kinds {
                    for instance in &given.instances {
                        tx.execute(
                            "INSERT INTO house_devices (helper, computer, kind, device, title, first_seen, last_seen)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                             ON CONFLICT (helper, computer, kind, device) DO UPDATE SET title = excluded.title,
                             last_seen = excluded.last_seen",
                            params![turbo.id, turbo.computer, given.kind, instance.id, instance.title, now],
                        )?;
                    }
                }
            }
            Ok(())
        });
        if let Err(error) = result {
            tracing::warn!(target: "devices", "the home's devices are not kept: {error}");
        }
    }

    /// A kind was used at a place: the house's last use (the choice row of the ear ''), which an ear without a choice
    /// of its own leans to.
    pub fn used(&self, kind: &str, place: &str) {
        let Some(db) = &self.db else { return };
        let computer = self.computer_of(place).unwrap_or_default();
        let _ = db.with(|conn| {
            conn.execute(
                "INSERT INTO device_choice (kind, ear, helper, computer, chosen_at, said_at) VALUES (?1, '', ?2, ?3, ?4, NULL)
                 ON CONFLICT (kind, ear) DO UPDATE SET
                 said_at = CASE WHEN device_choice.helper = excluded.helper AND device_choice.computer = excluded.computer
                           THEN device_choice.said_at ELSE NULL END,
                 helper = excluded.helper, computer = excluded.computer, chosen_at = excluded.chosen_at",
                params![kind, place, computer, now_ms() as i64],
            )
        });
    }

    /// The things of a kind remembered at a Турбо — at the computer there now —, newest seen first.
    fn remembered(&self, turbo: &TurboAt, kind: &str) -> Vec<Instance> {
        let Some(db) = &self.db else { return Vec::new() };
        db.with(|conn| {
            let mut statement = conn.prepare(
                "SELECT device, title FROM house_devices WHERE helper = ?1 AND computer = ?2 AND kind = ?3
                 ORDER BY last_seen DESC LIMIT 20",
            )?;
            let rows = statement.query_map(params![turbo.id, turbo.computer, kind], |row| {
                Ok(Instance { id: row.get(0)?, title: row.get(1)? })
            })?;
            rows.collect()
        })
        .unwrap_or_default()
    }

    /// The kinds remembered at the Турбо of the home now, each at the computer there now: a removed entry's, or the
    /// computer's before at the same entry, are not the home's.
    fn remembered_kinds(&self) -> Vec<String> {
        let Some(db) = &self.db else { return Vec::new() };
        let turbos = self.turbos();
        db.with(|conn| {
            let mut statement =
                conn.prepare("SELECT DISTINCT kind FROM house_devices WHERE helper = ?1 AND computer = ?2")?;
            let mut kinds = BTreeSet::new();
            for turbo in &turbos {
                let rows = statement.query_map(params![turbo.id, turbo.computer], |row| row.get::<_, String>(0))?;
                for kind in rows {
                    kinds.insert(kind?);
                }
            }
            Ok(kinds.into_iter().collect())
        })
        .unwrap_or_default()
    }

    /// The computer at a place: '' for this gateway, the Турбо's as it reads now; None for a place the home has not.
    fn computer_of(&self, place: &str) -> Option<String> {
        if place == HERE {
            return Some(String::new());
        }
        self.turbos().into_iter().find(|turbo| turbo.id == place).map(|turbo| turbo.computer)
    }

    /// Where the house used the kind last, among the places ready now.
    fn last_used(&self, kind: &str, ready: &[PlaceOf]) -> Option<String> {
        self.choice_of(kind, "").map(|(place, _)| place).filter(|place| ready.iter().any(|ready| ready.place == *place))
    }

    /// This ear's choice: (place, said aloud). The ear '' is the house's last use. A place that has another computer
    /// now than when it was chosen is no choice.
    fn choice_of(&self, kind: &str, ear: &str) -> Option<(String, bool)> {
        let db = self.db.as_ref()?;
        let (place, computer, said) = db
            .with(|conn| {
                conn.query_row(
                    "SELECT helper, computer, said_at IS NOT NULL FROM device_choice WHERE kind = ?1 AND ear = ?2",
                    params![kind, ear],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, bool>(2)?)),
                )
                .optional()
            })
            .ok()
            .flatten()?;
        (self.computer_of(&place)? == computer).then_some((place, said))
    }

    fn store_choice(&self, kind: &str, ear: &str, place: &str, said: bool) {
        let Some(db) = &self.db else { return };
        let now = now_ms() as i64;
        let computer = self.computer_of(place).unwrap_or_default();
        let _ = db.with(|conn| {
            conn.execute(
                "INSERT INTO device_choice (kind, ear, helper, computer, chosen_at, said_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (kind, ear) DO UPDATE SET
                 said_at = CASE WHEN device_choice.helper = excluded.helper AND device_choice.computer = excluded.computer
                           AND excluded.said_at IS NULL THEN device_choice.said_at ELSE excluded.said_at END,
                 helper = excluded.helper, computer = excluded.computer, chosen_at = excluded.chosen_at",
                params![kind, ear, place, computer, now, said.then_some(now)],
            )
        });
    }

    /// The wake addresses a Турбо gave, kept: its last status's, else the remembered ones.
    pub fn macs_of(&self, helper: &str) -> Vec<String> {
        let Some(turbo) = self.turbos().into_iter().find(|turbo| turbo.id == helper) else { return Vec::new() };
        if !turbo.macs.is_empty() {
            return turbo.macs;
        }
        let Some(db) = &self.db else { return Vec::new() };
        // Only the computer's at the entry now: another one's addresses would wake the computer before it.
        db.with(|conn| {
            conn.query_row(
                "SELECT macs FROM turbo_wake WHERE helper = ?1 AND computer = ?2",
                params![helper, turbo.computer],
                |row| row.get::<_, String>(0),
            )
            .optional()
        })
        .ok()
        .flatten()
        .and_then(|macs| serde_json::from_str::<Vec<String>>(&macs).ok())
        .unwrap_or_default()
    }

    /// A Турбо's name as people know it.
    pub fn name_of(&self, place: &str) -> Option<String> {
        if place == HERE {
            return Some(self.name_here());
        }
        self.turbos().into_iter().find(|turbo| turbo.id == place).map(|turbo| turbo.name)
    }

    /// The first sleeping Турбо that can be woken: (id, name). A face's code runs at the first Турбо that gives the
    /// apps — at home it is this one.
    pub fn sleeping_turbo(&self) -> Option<(String, String)> {
        self.turbos()
            .into_iter()
            .find(|turbo| !turbo.up && !turbo.cloud && !self.macs_of(&turbo.id).is_empty())
            .map(|turbo| (turbo.id, turbo.name))
    }

    /// Wakes a sleeping Турбо over the home network and waits for it up to `wait` (owner 20.11, 5): true once it is on
    /// the line.
    pub async fn wake(&self, helper: &str, wait: Duration) -> Result<bool, String> {
        let source = self.source.get().cloned().ok_or_else(|| crate::tr!("Турбо в доме нет."))?;
        if self.turbos().iter().any(|turbo| turbo.id == helper && turbo.up) {
            return Ok(true);
        }
        let macs = self.macs_of(helper);
        if macs.is_empty() {
            return Err(crate::tr!("Этот компьютер не сказал, как его будить по сети."));
        }
        {
            let mut waking = self.waking.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if !waking.insert(helper.to_string()) {
                return Err(crate::tr!("Уже бужу его. Подождите немного."));
            }
        }
        let outcome = async {
            match self.waker.get().cloned() {
                Some(waker) => waker(macs.clone()).await?,
                None => crate::helpers::wake::send(&macs).await?,
            };
            let until = tokio::time::Instant::now() + wait;
            while tokio::time::Instant::now() < until {
                tokio::time::sleep(WAKE_LOOK).await;
                if source.look(helper).await {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        .await;
        self.waking.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(helper);
        outcome
    }
}

/// The handles of a kind the home knows by name: kept while every place of it sleeps, so an app's link to it stands and
/// its letters say why not.
fn known_handles(kind: &str) -> Vec<Handle> {
    let named = |name: &str, level: Level| Handle::new(name, level);
    match kind {
        "midi" => vec![
            named("keyboards", Level::See),
            named("notes", Level::Listen),
            named("play", Level::Call),
            named("stop", Level::Call),
        ],
        "screen" => vec![
            named("windows", Level::See),
            named("open", Level::Call),
            named("keep", Level::Call),
            named("close", Level::Call),
            named("pause", Level::Call),
            named("resume", Level::Call),
        ],
        "storage" => vec![named("drives", Level::See)],
        _ => Vec::new(),
    }
}

/// The screen a file is shown on: a Турбо's file (`turbo.<place>.<key>`, files.rs) at that Турбо, with the key it
/// knows; this gateway's own at this gateway. Files of two computers at once are not shown together.
fn screen_place(input: &mut Value, ready: &[PlaceOf]) -> Result<Option<String>, BusError> {
    let Some(items) = input.get_mut("items").and_then(Value::as_array_mut) else { return Ok(None) };
    let mut places = BTreeSet::new();
    for item in items.iter_mut() {
        let Some(Value::String(file)) = item.get("file") else { continue };
        let place = match file.strip_prefix(TURBO_KEY).and_then(|rest| rest.split_once('.')) {
            Some((place, key)) => {
                let (place, key) = (place.to_string(), key.to_string());
                item["file"] = Value::String(key);
                place
            }
            None => HERE.to_string(),
        };
        places.insert(place);
    }
    match places.len() {
        0 => Ok(None),
        1 => {
            let place = places.into_iter().next().unwrap_or_default();
            if ready.iter().any(|ready| ready.place == place) {
                Ok(Some(place))
            } else {
                Err(BusError::Failed(crate::tr!("Экрана у компьютера, где лежит этот файл, сейчас нет.")))
            }
        }
        _ => Err(BusError::Failed(crate::tr!("Файлы с разных компьютеров на одном экране не показать."))),
    }
}

/// Why a place was chosen, for the words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    Chosen,
    Used,
    First,
}

/// «MIDI-клавиатура: у Mac и у ПК. Беру у Mac, потому что там пользовались последним. Если надо, скажите «у ПК».»
fn choice_words(title: &str, ready: &[PlaceOf], chosen: &str, reason: Reason) -> String {
    let names: Vec<String> = ready.iter().map(|place| crate::tr!("у {name}", name = place.name)).collect();
    let other = ready.iter().find(|place| place.name != chosen).map(|place| place.name.clone()).unwrap_or_default();
    let why = match reason {
        Reason::Chosen => crate::tr!("так выбрали в прошлый раз"),
        Reason::Used => crate::tr!("там пользовались последним"),
        Reason::First => crate::tr!("он первый в списке Турбо"),
    };
    crate::tr!(
        "{title}: {places}. Беру у {chosen}, потому что {why}. Если надо, скажите «у {other}».",
        title = title,
        places = names.join(&crate::tr!(" и ")),
        chosen = chosen,
        why = why,
        other = other
    )
}

/// A Турбо's answer with the ids in it named by the home: each object of a list, or the answer itself, gets its
/// `place`, and an `id`/`input`/`output` gets the place before it.
pub fn with_place(answer: Value, place: &str) -> Value {
    fn mark(value: &mut Value, place: &str) {
        if let Value::Object(fields) = value {
            for key in ["id", "input", "output", "playing"] {
                if let Some(Value::String(id)) = fields.get(key) {
                    let named = home_id(place, id);
                    fields.insert(key.into(), Value::String(named));
                }
            }
            fields.insert("place".into(), Value::String(place.to_string()));
        }
    }
    let mut answer = answer;
    match &mut answer {
        Value::Array(items) => items.iter_mut().for_each(|item| mark(item, place)),
        other => mark(other, place),
    }
    answer
}

/// The answers of several places as one: lists joined, else each by its place.
pub fn joined(answers: Vec<(String, Value)>) -> Value {
    if answers.len() == 1 {
        return answers.into_iter().next().map(|(_, value)| value).unwrap_or(Value::Null);
    }
    if answers.iter().all(|(_, value)| value.is_array()) {
        return Value::Array(
            answers.into_iter().flat_map(|(_, value)| value.as_array().cloned().unwrap_or_default()).collect(),
        );
    }
    Value::Array(answers.into_iter().map(|(place, value)| json!({ "place": place, "answer": value })).collect())
}

/// One kind of the home on the bus: `device/<kind>` is every place that has it (src/bus/providers/devices.rs).
pub struct HouseDevice {
    kind: String,
    house: Weak<HouseDevices>,
}

impl HouseDevice {
    pub fn new(kind: &str, house: &Arc<HouseDevices>) -> HouseDevice {
        HouseDevice { kind: kind.to_string(), house: Arc::downgrade(house) }
    }
}

impl Device for HouseDevice {
    fn name(&self) -> &'static str {
        // The kinds are few and live as long as the program (a newer Турбо's other kinds too).
        static NAMES: OnceLock<Mutex<std::collections::HashMap<String, &'static str>>> = OnceLock::new();
        let mut names = NAMES.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        names.entry(self.kind.clone()).or_insert_with(|| Box::leak(self.kind.clone().into_boxed_str()))
    }

    fn title(&self) -> String {
        self.house.upgrade().map(|house| house.title_of(&self.kind)).unwrap_or_else(|| self.kind.clone())
    }

    fn handles(&self) -> Vec<Handle> {
        self.house.upgrade().map(|house| house.handles_of(&self.kind)).unwrap_or_default()
    }

    /// While the home knows it: a sleeping Турбо's keyboard stays an address, its letters answer why not.
    fn available(&self) -> bool {
        self.house.upgrade().is_some_and(|house| house.knows(&self.kind))
    }

    fn nature(&self) -> Nature {
        self.house.upgrade().map(|house| house.nature_of(&self.kind)).unwrap_or(Nature::Physical)
    }

    fn instances(&self) -> Vec<Instance> {
        let Some(house) = self.house.upgrade() else { return Vec::new() };
        house.ready(&self.kind).into_iter().flat_map(|place| place.instances).collect()
    }

    fn call(&self, handle: &str, input: Value) -> BoxFuture<'static, Result<Value, BusError>> {
        let (kind, handle, house) = (self.kind.clone(), handle.to_string(), self.house.clone());
        Box::pin(async move {
            let house = house.upgrade().ok_or_else(|| BusError::Failed("the gateway is stopping".into()))?;
            house.letter(&kind, &handle, input).await
        })
    }
}

impl HouseDevices {
    /// A letter to the home's kind: what sees or hears — to every place ready; what acts — to the place named by its
    /// device's id or `place`, or the one chosen for the ear (`ear`). The notes' channel is the panel's own.
    pub async fn letter(&self, kind: &str, handle: &str, input: Value) -> Result<Value, BusError> {
        let level = self.handles_of(kind).into_iter().find(|known| known.name == handle).map(|known| known.level);
        let Some(level) = level else { return Err(BusError::NotFound(format!("device/{kind}: {handle}"))) };
        if kind == "midi" && handle == "notes" {
            return Ok(json!({ "channel": "/api/v1/midi/ws" }));
        }
        let mut input = input;
        // The device that heard the wish: the brain's tools name it, an app's own letter does not.
        let ear = match input.as_object_mut().and_then(|fields| fields.remove("ear")) {
            Some(Value::String(ear)) => ear,
            _ => String::new(),
        };
        if level <= Level::Listen {
            let ready = self.ready(kind);
            if ready.is_empty() {
                return Err(BusError::Failed(self.why_none(kind)));
            }
            let mut answers = Vec::new();
            let mut failed = None;
            for place in ready {
                match self.call_at(kind, &place.place, handle, input.clone()).await {
                    Ok(answer) => answers.push((place.place, answer)),
                    Err(error) => failed = Some(error),
                }
            }
            return match (answers.is_empty(), failed) {
                (true, Some(error)) => Err(error),
                _ => Ok(joined(answers)),
            };
        }
        let places: Vec<String> = self.places(kind).into_iter().map(|place| place.place).collect();
        let named = input
            .get("device")
            .and_then(Value::as_str)
            .filter(|device| !device.is_empty())
            .map(|device| split_id(device, &places))
            .map(|(place, id)| (place.to_string(), id.to_string()));
        let mut asked_place = input.get("place").and_then(Value::as_str).map(str::to_string);
        // A file shown on a screen goes to the screen of the computer it is at: its key names its place.
        if kind == "screen" && handle == "open" && named.is_none() {
            let shown = screen_place(&mut input, &self.ready(kind))?;
            asked_place = asked_place.or(shown);
        }
        let (place, said) = match (named, asked_place) {
            (Some((place, id)), _) => {
                if let Some(fields) = input.as_object_mut() {
                    fields.insert("device".into(), Value::String(id));
                }
                (place, None)
            }
            (None, Some(place)) if places.contains(&place) => (place, None),
            _ => {
                let choice = self.choose(kind, &ear)?;
                (choice.place, choice.said)
            }
        };
        if let Some(fields) = input.as_object_mut() {
            fields.remove("place");
        }
        if place != HERE && !self.ready(kind).iter().any(|ready| ready.place == place) {
            return Err(BusError::Failed(self.why_at(kind, &place)));
        }
        let answer = self.call_at(kind, &place, handle, input).await?;
        self.used(kind, &place);
        Ok(match (said, answer) {
            (Some(said), Value::Object(mut fields)) => {
                fields.insert("said".into(), Value::String(said));
                Value::Object(fields)
            }
            (Some(said), other) => json!({ "answer": other, "said": said }),
            (None, answer) => answer,
        })
    }

    /// Why one place of a kind cannot be reached now.
    fn why_at(&self, kind: &str, place: &str) -> String {
        let title = self.title_of(kind);
        let Some(found) = self.places(kind).into_iter().find(|known| known.place == place) else {
            return self.why_none(kind);
        };
        let name = found.name;
        match found.state {
            PlaceState::Ready => crate::tr!("«{title}» сейчас не отвечает.", title = title),
            PlaceState::Unplugged => unplugged(&title, &name),
            PlaceState::Asleep => asleep(&title, &name),
            PlaceState::Away => away(&title, &name),
        }
    }
}

/// «У Mac сейчас нет «MIDI-клавиатура»…»: the words name no gender — a keyboard and a screen alike.
fn unplugged(title: &str, name: &str) -> String {
    crate::tr!("У {name} сейчас нет «{title}»: подключите к нему и скажите ещё раз.", title = title, name = name)
}

fn asleep(title: &str, name: &str) -> String {
    crate::tr!("«{title}» — у {name}, а он сейчас спит или выключен.", title = title, name = name)
}

fn away(title: &str, name: &str) -> String {
    crate::tr!("«{title}» — у {name}, а он сейчас не дома: на связи не из домашней сети.", title = title, name = name)
}

/// The helpers as the home's devices read them.
pub struct HelpersSource {
    helpers: Weak<Helpers>,
}

impl HelpersSource {
    pub fn new(helpers: &Arc<Helpers>) -> HelpersSource {
        HelpersSource { helpers: Arc::downgrade(helpers) }
    }
}

/// A Турбо's contract details for its devices: what it gives, whether the home reaches it from home, its wake
/// addresses.
fn read_details(details: &Map<String, Value>) -> (Vec<GivenKind>, bool, Vec<String>) {
    let kinds = details
        .get("kinds")
        .and_then(Value::as_array)
        .map(|kinds| kinds.iter().filter_map(|kind| serde_json::from_value::<GivenKind>(kind.clone()).ok()).collect())
        .unwrap_or_default();
    let home = details.get("home").and_then(Value::as_bool).unwrap_or(false);
    let macs = details
        .get("wake")
        .and_then(|wake| wake.get("macs"))
        .and_then(Value::as_array)
        .map(|macs| macs.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    (kinds, home, macs)
}

impl TurboSource for HelpersSource {
    fn turbos(&self) -> Vec<TurboAt> {
        let Some(helpers) = self.helpers.upgrade() else { return Vec::new() };
        helpers
            .offering(Ability::Devices)
            .into_iter()
            .map(|remote| {
                let status = remote.status();
                let devices = status.as_ref().and_then(|status| status.contract.get(Ability::Devices).cloned());
                let (kinds, home, macs) =
                    devices.as_ref().map(|devices| read_details(&devices.details)).unwrap_or_default();
                let files = status.as_ref().is_some_and(|status| status.contract.ready(Ability::Files));
                let entry = helpers.entry(remote.id());
                let computer = entry
                    .as_ref()
                    .and_then(|entry| entry.helper_id.clone())
                    .or_else(|| status.as_ref().map(|status| status.contract.helper.id.clone()))
                    .unwrap_or_default();
                let name = entry.map(|entry| entry.name).unwrap_or_else(|| remote.id().to_string());
                TurboAt {
                    id: remote.id().to_string(),
                    computer,
                    name,
                    cloud: remote.kind() == HelperKind::Cloud,
                    up: remote.up() && remote.gives(Ability::Devices),
                    home: home && !remote.linked(),
                    kinds,
                    files: files && remote.up(),
                    macs,
                }
            })
            .collect()
    }

    fn call(&self, turbo: &str, kind: &str, handle: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
        let remote =
            self.helpers.upgrade().and_then(|helpers| helpers.entry(turbo).and_then(|entry| helpers.remote(&entry)));
        let (kind, handle) = (kind.to_string(), handle.to_string());
        Box::pin(async move {
            let remote = remote.ok_or_else(|| crate::tr!("Такого Турбо в доме нет."))?;
            let response = crate::utils::http::client_without_redirects()
                .post(format!("{}/home/v1/devices/{kind}/call", remote.base().trim_end_matches('/')))
                .bearer_auth(remote.credential())
                .timeout(CALL_WAIT)
                .json(&json!({ "handle": handle, "input": input }))
                .send()
                .await
                .map_err(|_| crate::tr!("Турбо не ответило."))?;
            let status = response.status();
            let body: Value = response.json().await.unwrap_or(Value::Null);
            if !status.is_success() {
                let words = body.get("message").and_then(Value::as_str).map(str::to_string);
                return Err(
                    words.unwrap_or_else(|| crate::tr!("Турбо отказало (HTTP {status}).", status = status.as_u16()))
                );
            }
            Ok(body.get("message").cloned().unwrap_or(body))
        })
    }

    fn stream(&self, turbo: &str) -> Option<(String, String)> {
        let helpers = self.helpers.upgrade()?;
        let remote = helpers.entry(turbo).and_then(|entry| helpers.remote(&entry))?;
        if remote.linked() || !remote.up() {
            return None;
        }
        let base = remote.url().trim_end_matches('/');
        let socket = if let Some(rest) = base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else {
            format!("ws://{}", base.strip_prefix("http://")?)
        };
        Some((format!("{socket}/home/v1/devices/midi/stream"), remote.credential()))
    }

    fn post(&self, turbo: &str, path: &str, body: Vec<u8>) -> BoxFuture<'static, Result<Value, String>> {
        let remote =
            self.helpers.upgrade().and_then(|helpers| helpers.entry(turbo).and_then(|entry| helpers.remote(&entry)));
        let path = path.to_string();
        Box::pin(async move {
            let remote = remote.ok_or_else(|| crate::tr!("Такого Турбо в доме нет."))?;
            let response = crate::utils::http::client_without_redirects()
                .post(format!("{}{path}", remote.base().trim_end_matches('/')))
                .bearer_auth(remote.credential())
                .timeout(CALL_WAIT)
                .body(body)
                .send()
                .await
                .map_err(|_| crate::tr!("Турбо не ответило."))?;
            let status = response.status();
            let answer: Value = response.json().await.unwrap_or(Value::Null);
            if !status.is_success() {
                let words = answer.get("message").and_then(Value::as_str).map(str::to_string);
                return Err(
                    words.unwrap_or_else(|| crate::tr!("Турбо отказало (HTTP {status}).", status = status.as_u16()))
                );
            }
            Ok(answer.get("message").cloned().unwrap_or(answer))
        })
    }

    fn files(&self, turbo: &str, to: &str, handle: &str, input: Value) -> BoxFuture<'static, Result<Value, BusError>> {
        let remote =
            self.helpers.upgrade().and_then(|helpers| helpers.entry(turbo).and_then(|entry| helpers.remote(&entry)));
        let body = json!({ "to": to, "handle": handle, "input": input });
        Box::pin(async move {
            let remote = remote.ok_or_else(|| BusError::Failed(crate::tr!("Такого Турбо в доме нет.")))?;
            let response = crate::utils::http::client_without_redirects()
                .post(format!("{}/home/v1/files/call", remote.base().trim_end_matches('/')))
                .bearer_auth(remote.credential())
                .timeout(CALL_WAIT)
                .json(&body)
                .send()
                .await
                .map_err(|_| BusError::Failed(crate::tr!("Турбо не ответило.")))?;
            let status = response.status();
            let answer: Value = response.json().await.unwrap_or(Value::Null);
            let words = || {
                answer
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| crate::tr!("Турбо отказало (HTTP {status}).", status = status.as_u16()))
            };
            match status.as_u16() {
                200..=299 => Ok(answer.get("message").cloned().unwrap_or(Value::Null)),
                404 => Err(BusError::NotFound(words())),
                403 => Err(BusError::Denied(words())),
                _ => Err(BusError::Failed(words())),
            }
        })
    }

    fn media_base(&self, turbo: &str) -> Option<String> {
        let helpers = self.helpers.upgrade()?;
        let remote = helpers.entry(turbo).and_then(|entry| helpers.remote(&entry))?;
        (!remote.linked() && remote.up()).then(|| remote.url().trim_end_matches('/').to_string())
    }

    fn look(&self, turbo: &str) -> BoxFuture<'static, bool> {
        let helpers = self.helpers.clone();
        let turbo = turbo.to_string();
        Box::pin(async move {
            let Some(helpers) = helpers.upgrade() else { return false };
            let _ = helpers.check(&turbo).await;
            helpers.entry(&turbo).and_then(|entry| helpers.remote(&entry)).is_some_and(|remote| remote.up())
        })
    }
}

/// The home's kinds on the bus: one `device/<kind>` for each kind the home knows by name, and each kind this gateway
/// has; a newer Турбо's other kinds come on its next start.
pub fn register(devices: &Devices, house: &Arc<HouseDevices>) {
    let mut kinds: Vec<String> = KINDS.iter().map(|kind| kind.to_string()).collect();
    kinds.extend(house.local().list().iter().map(|device| device.name().to_string()));
    kinds.extend(house.kinds());
    let mut seen = BTreeSet::new();
    for kind in kinds.into_iter().filter(|kind| seen.insert(kind.clone())) {
        devices.add(Arc::new(HouseDevice::new(&kind, house)));
    }
}

pub mod files;
pub mod routes;
pub mod storage;
pub mod tools;

#[cfg(test)]
mod tests;
