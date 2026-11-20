//! A helper of the list as this gateway reaches it (docs/plans/home-and-helpers.md, block 1): the look at its
//! `/home/v1/status` every 15 s — read as its contract —, its state (the first look decides at once, then two misses in
//! a row are away), its Telegram bot's contacts for «передай …», and its clients for hearing and voice. Every request
//! names this gateway and its home to the helper — for the helper's «Помогаю», never for rights — and follows no
//! redirect (`client_without_redirects`): `/home/v1` never sends anywhere else.
//!
//! A **linked** helper (an entry with a pin or a link key, stage 7) is reached over its link instead (link.rs): its state
//! and status are the link's, and its clients ask this gateway's own bridge `/via/<entry>/home/v1/…` with the per-start
//! token — the link key never leaves the link.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::contract::{Ability, HelperIdentity, HelperStatus};
use super::link::{Link, LinkNews, LinkSetup, LinkState, LinkTiming};
use super::{HelperKind, HelperSetting, HelperState, Local, Reach, AWAY, DOWN_AFTER};
use crate::config::{AsrConfig, Config, OwnSpeechConfig};
use crate::core::identity::home::{clean_name, Identity};
use crate::settings::definitions::HelperPin;
use crate::speech::asr::openai_transcriber::OpenAiTranscriber;
use crate::speech::asr::speech_recognizer::SpeechRecognizer;
use crate::speech::tts::own_speech::OwnSpeech;
use crate::utils::time::now_iso;

/// How long a look waits for the status.
const LOOK_WAIT: Duration = Duration::from_secs(5);
/// How long the contacts' read waits.
const CONTACTS_WAIT: Duration = Duration::from_secs(3);
/// The longest name this gateway sends of itself.
const NAME_SENT: usize = 60;

/// The helper's Telegram bot as this gateway last saw it (`/home/v1/messenger/contacts`). Kept while it is away:
/// «передай маме…» is still ours then, and says the helper is away.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Messenger {
    /// Not read yet, or the helper has no such route (an older one).
    #[default]
    Unknown,
    /// The helper has no bot set up.
    NoBot,
    /// The names of its contacts, as its bot has them: no chats.
    Contacts(Vec<String>),
}

/// The state of the looks.
#[derive(Debug, Clone, Default)]
struct Watch {
    /// Its last state: None — not looked at yet.
    up: Option<bool>,
    since: Option<String>,
    failures: u32,
    up_since: Option<Instant>,
    error: Option<String>,
    /// The last cause told to the log: a look that fails the same way every 30 s is told once.
    told: Option<String>,
}

/// This gateway and its home as they name themselves to a helper: the headers of every request, the pairing (stage 7).
#[derive(Clone, Debug, PartialEq)]
pub struct Me {
    pub gateway: HelperIdentity,
    pub home: HelperIdentity,
}

impl Me {
    /// A gateway and its home for tests.
    #[cfg(test)]
    pub(crate) fn default_for_tests() -> Me {
        Me {
            gateway: HelperIdentity { id: "gateway-1".into(), name: "Колонка".into() },
            home: HelperIdentity { id: "home-1".into(), name: "Дом".into() },
        }
    }

    pub fn of(identity: &Identity) -> Me {
        let home =
            HelperIdentity { id: identity.home().map(|home| home.id).unwrap_or_default(), name: identity.home_name() };
        Me { gateway: identity.as_helper(), home }
    }
}

/// One helper of the list, reached at its address with its key.
pub struct RemoteHelper {
    id: String,
    url: String,
    key: String,
    kind: HelperKind,
    reach: Reach,
    /// This gateway's recognition settings, for the helper's hearing: the language, the prompt, the time, the wake words.
    asr: AsrConfig,
    tts_timeout_ms: u64,
    watch: Mutex<Watch>,
    status: Mutex<Option<HelperStatus>>,
    messenger: Mutex<Messenger>,
    /// The abilities of a version-0 helper whose routes answered 404: forgotten at its every read.
    missing: Mutex<Vec<Ability>>,
    /// The look found this gateway itself.
    itself: AtomicBool,
    /// Its hearing and voice clients, and the address they were made for: made again when it changes (a linked
    /// helper's bridge is on this gateway's own port, known once the server listens).
    clients: Mutex<Option<(String, Clients)>>,
    /// Read at each request: a rename shows at once.
    identity: Arc<Identity>,
    /// The local bridge's port and token for a linked helper (stage 7); a plain entry does not use it.
    local: Arc<Local>,
    /// A linked entry's pin: its helper is reached over the link only.
    pin: Option<HelperPin>,
    /// The helper's id a linked entry was paired with.
    helper_id: Option<String>,
    /// The link while it runs; its last state stays after it stopped.
    link: Mutex<Option<Arc<Link>>>,
    last_link: Mutex<Option<LinkState>>,
}

/// A helper's hearing and voice, as this gateway's clients ask them.
type Clients = (Arc<dyn SpeechRecognizer>, Arc<OwnSpeech>);

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl RemoteHelper {
    pub fn new(
        entry: &HelperSetting,
        key: String,
        config: &Config,
        identity: Arc<Identity>,
        local: Arc<Local>,
    ) -> Self {
        RemoteHelper::reached(entry, key, None, config, identity, local)
    }

    /// An entry as this gateway reaches it: over its link when it has a pin.
    pub fn reached(
        entry: &HelperSetting,
        key: String,
        pin: Option<HelperPin>,
        config: &Config,
        identity: Arc<Identity>,
        local: Arc<Local>,
    ) -> Self {
        RemoteHelper {
            id: entry.id.clone(),
            url: entry.url.clone(),
            key,
            kind: entry.kind,
            reach: super::reach_of(entry),
            asr: config.speech.asr.clone(),
            tts_timeout_ms: config.gateway.tts.own.as_ref().map(|own| own.timeout_ms).unwrap_or(15_000),
            watch: Mutex::new(Watch::default()),
            status: Mutex::new(None),
            messenger: Mutex::new(Messenger::Unknown),
            missing: Mutex::new(Vec::new()),
            itself: AtomicBool::new(false),
            clients: Mutex::new(None),
            identity,
            local,
            pin,
            helper_id: entry.helper_id.clone(),
            link: Mutex::new(None),
            last_link: Mutex::new(None),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn kind(&self) -> HelperKind {
        self.kind
    }

    pub fn reach(&self) -> Reach {
        self.reach
    }

    /// Where its ability clients and the apps' calls go: the entry's address, or — linked — this gateway's own bridge
    /// to its link.
    pub fn base(&self) -> String {
        if self.linked() {
            return super::link::via_base(self.local.port(), &self.id);
        }
        self.url.clone()
    }

    /// The bearer its in-process clients send: the entry's key, or — linked — the bridge's per-start token.
    pub fn credential(&self) -> String {
        if self.linked() {
            return self.local.token().to_string();
        }
        self.key.clone()
    }

    /// Reached over the link: it has a pin, or a link key — whose pin did not read (pins that do not read keep their
    /// last read, none before the first) or is gone. Such an entry is still asked only through the bridge, which has
    /// no link for it, so its `dwl1_` key is never sent another way, nor kept in a «Мозг» row.
    pub fn linked(&self) -> bool {
        self.pin.is_some() || super::link::is_link_key(&self.key)
    }

    pub fn pin(&self) -> Option<&HelperPin> {
        self.pin.as_ref()
    }

    /// Made for this address, key and pin. (A linked entry's helper id changes only with a new pairing, which brings a
    /// new key; a plain entry's is learned by its looks and changes nothing here.)
    pub(crate) fn same_as(&self, entry: &HelperSetting, key: &str, pin: Option<&HelperPin>) -> bool {
        let same_pin = match (&self.pin, pin) {
            (Some(mine), Some(theirs)) => mine.sha256 == theirs.sha256 && mine.public == theirs.public,
            (None, None) => true,
            _ => false,
        };
        self.url == entry.url && self.key == key && same_pin
    }

    /// The link while it runs.
    pub fn link(&self) -> Option<Arc<Link>> {
        locked(&self.link).clone().filter(|link| !link.stopped())
    }

    /// The link started, unless it runs already. Never for an entry that cannot be linked: no https address, no pin,
    /// no link key — the key then goes nowhere, and the entry says why.
    pub(crate) fn run_link(&self, timing: LinkTiming, news: super::link::NewsSink) {
        let mut link = locked(&self.link);
        if link.as_ref().is_some_and(|link| !link.stopped()) {
            return;
        }
        let linkable = self.url.starts_with("https://") && super::link::is_link_key(&self.key);
        let Some(pin) = self.pin.clone().filter(|_| linkable) else {
            drop(link);
            self.linked_seen(&LinkNews::Down(super::link::REFUSED_WORDS.to_string()));
            return;
        };
        let identity = self.identity.clone();
        let setup = LinkSetup {
            entry: self.id.clone(),
            url: self.url.clone(),
            pin,
            key: self.key.clone(),
            helper_id: self.helper_id.clone().unwrap_or_default(),
            me: Arc::new(move || Me::of(&identity)),
        };
        *link = Some(Link::start(setup, timing, news));
    }

    /// The link stopped (1000) — switched off, kept out, replaced; `bye` when the helper is removed.
    pub(crate) fn halt_link(&self, bye: bool) {
        if let Some(link) = locked(&self.link).take() {
            *locked(&self.last_link) = Some(link.state());
            if bye {
                link.bye();
            } else {
                link.stop();
            }
        }
    }

    /// The link's state now, or its last one.
    pub fn link_state(&self) -> Option<LinkState> {
        match locked(&self.link).as_ref() {
            Some(link) => Some(link.state()),
            None => locked(&self.last_link).clone(),
        }
    }

    /// What the link told: its status kept, its state at once (its grace is spent already). Some(up) when the state
    /// changed.
    pub(crate) fn linked_seen(&self, news: &LinkNews) -> Option<bool> {
        let up = match news {
            LinkNews::Up(status) | LinkNews::Status(status) => {
                if let Some(status) = HelperStatus::read(status) {
                    *locked(&self.status) = Some(status);
                }
                true
            }
            LinkNews::Down(_) => false,
        };
        if matches!(news, LinkNews::Status(_)) {
            return None;
        }
        let mut watch = locked(&self.watch);
        watch.failures = 0;
        watch.error = match news {
            LinkNews::Down(reason) => Some(reason.clone()),
            _ => None,
        };
        let changed = watch.up != Some(up);
        if changed {
            watch.up = Some(up);
            watch.since = Some(now_iso());
            watch.up_since = up.then(Instant::now);
        }
        changed.then_some(up)
    }

    /// A GET of the helper with the key and this gateway's names.
    async fn get(&self, path: &str, wait: Duration) -> reqwest::Result<reqwest::Response> {
        let me = Me::of(&self.identity);
        let name: String = clean_name(&me.gateway.name).chars().take(NAME_SENT).collect();
        let mut request = crate::utils::http::client_without_redirects()
            .get(format!("{}{path}", self.base()))
            .bearer_auth(self.credential())
            .timeout(wait);
        for (header, value) in
            [("x-duowave-gateway", me.gateway.id), ("x-duowave-gateway-name", name), ("x-duowave-home", me.home.id)]
        {
            if let Ok(value) = reqwest::header::HeaderValue::from_bytes(value.as_bytes()) {
                if !value.is_empty() {
                    request = request.header(header, value);
                }
            }
        }
        request.send().await
    }

    /// One look: its status, read as its contract; a miss says why in words. A linked helper's is its link's: a link
    /// key is never sent by a plain look.
    pub async fn look(&self) -> Result<HelperStatus, String> {
        if self.linked() {
            let refused = || super::link::REFUSED_WORDS.to_string();
            let link = self.link().ok_or_else(refused)?;
            return match link.state() {
                LinkState::Up { .. } => link.status().as_ref().and_then(HelperStatus::read).ok_or_else(refused),
                LinkState::Down { reason, .. } => Err(reason),
                LinkState::Connecting => Err(AWAY.to_string()),
            };
        }
        let not_a_helper = || crate::tr!("По этому адресу не Турбо DuoWave.");
        match self.get("/home/v1/status", LOOK_WAIT).await {
            Ok(response) if response.status().is_success() => match response.json::<Value>().await {
                Ok(body) => HelperStatus::read(&body).ok_or_else(not_a_helper),
                Err(_) => Err(not_a_helper()),
            },
            Ok(response) if matches!(response.status().as_u16(), 401 | 403) => {
                Err(crate::tr!("Турбо не принимает ключ: сделайте новый на нём в «Доступ» → «Ключи»"))
            }
            Ok(response) => Err(crate::tr!("Турбо ответило {status}", status = response.status().as_u16())),
            Err(error) if error.is_timeout() => {
                self.tell_cause(&format!("no answer in {} s", LOOK_WAIT.as_secs()));
                Err(crate::tr!("Турбо не ответило вовремя"))
            }
            Err(error) => {
                // The real cause — refused, no route, reset — for the log only: the panel keeps its words (Windows
                // test 21.11: «Турбо не на связи», and gateway.log said nothing of why).
                self.tell_cause(&cause_of(&error));
                Err(AWAY.to_string())
            }
        }
    }

    /// The cause of a failed look to the log, once per new cause, with the address dialed.
    fn tell_cause(&self, cause: &str) {
        let mut watch = locked(&self.watch);
        if watch.told.as_deref() == Some(cause) {
            return;
        }
        watch.told = Some(cause.to_string());
        drop(watch);
        tracing::warn!("helpers: {} ({}) does not answer at {}: {cause}", self.id, self.kind_word(), self.base());
    }

    fn kind_word(&self) -> &'static str {
        match self.kind {
            HelperKind::Home => "a computer at home",
            HelperKind::Own => "an own server",
            HelperKind::Cloud => "the cloud",
        }
    }

    /// A look's outcome: the status it read kept, the state — the first look decides at once, then `DOWN_AFTER` misses
    /// in a row are away. Some(up) when the state changed.
    pub fn seen(&self, outcome: &Result<HelperStatus, String>) -> Option<bool> {
        if let Ok(status) = outcome {
            let mut status = status.clone();
            if status.contract.version == 0 {
                for ability in locked(&self.missing).iter() {
                    status.contract.forget(*ability);
                }
            }
            *locked(&self.status) = Some(status);
        }
        let mut watch = locked(&self.watch);
        match outcome {
            Ok(_) => {
                if watch.told.take().is_some() {
                    tracing::info!("helpers: {} answers again", self.id);
                }
                watch.failures = 0;
                watch.error = None;
                let changed = watch.up != Some(true);
                if changed {
                    watch.up = Some(true);
                    watch.since = Some(now_iso());
                    watch.up_since = Some(Instant::now());
                }
                changed.then_some(true)
            }
            Err(error) => {
                watch.failures += 1;
                watch.error = Some(error.clone());
                let away = watch.up.is_none() || watch.failures >= DOWN_AFTER;
                let changed = away && watch.up != Some(false);
                if changed {
                    watch.up = Some(false);
                    watch.since = Some(now_iso());
                    watch.up_since = None;
                }
                changed.then_some(false)
            }
        }
    }

    /// The names of its bot's contacts, or that it has none; a version-0 helper without the route no longer declares
    /// messages. A miss keeps what was seen. Whether its bot was found or gone — read for the first time, set up, taken
    /// away —, not whether its contacts' names changed. A true is news for the registry: whoever reads the contacts
    /// tells it on as a `helpers.changed` «contract» (`Helpers::after_look`).
    pub async fn read_contacts(&self) -> bool {
        let seen = match self.get("/home/v1/messenger/contacts", CONTACTS_WAIT).await {
            Ok(response) if response.status().is_success() => match response.json::<Value>().await {
                Ok(body) => Messenger::Contacts(
                    body.get("contacts")
                        .and_then(Value::as_array)
                        .map(|names| names.iter().filter_map(Value::as_str).map(str::to_string).collect())
                        .unwrap_or_default(),
                ),
                Err(_) => return false,
            },
            Ok(response) if response.status().as_u16() == 503 => Messenger::NoBot,
            // A helper of an older version: no messages through it.
            Ok(response) if response.status().as_u16() == 404 => {
                let mut status = locked(&self.status);
                if let Some(status) = status.as_mut().filter(|status| status.contract.version == 0) {
                    status.contract.forget(Ability::Messages);
                    locked(&self.missing).push(Ability::Messages);
                }
                Messenger::Unknown
            }
            Ok(response) => {
                tracing::debug!("helpers: the contacts of «{}»: HTTP {}", self.id, response.status().as_u16());
                return false;
            }
            Err(error) => {
                tracing::debug!("helpers: the contacts of «{}»: {error}", self.id);
                return false;
            }
        };
        let mut messenger = locked(&self.messenger);
        let changed = std::mem::discriminant(&*messenger) != std::mem::discriminant(&seen);
        *messenger = seen;
        changed
    }

    pub fn state(&self) -> HelperState {
        match locked(&self.watch).up {
            None => HelperState::Unknown,
            Some(true) => HelperState::Up,
            Some(false) => HelperState::Down,
        }
    }

    pub fn up(&self) -> bool {
        locked(&self.watch).up == Some(true)
    }

    pub fn since(&self) -> Option<String> {
        locked(&self.watch).since.clone()
    }

    pub fn error(&self) -> Option<String> {
        locked(&self.watch).error.clone()
    }

    /// Looked at anew: its state forgotten — switched off and on again, kept out by the regulator or let out again, a
    /// twin not looked at. What it declares stays.
    pub fn reset(&self) {
        *locked(&self.watch) = Watch::default();
    }

    pub fn status(&self) -> Option<HelperStatus> {
        locked(&self.status).clone()
    }

    /// It declares the ability, or is not read yet (it may).
    pub fn declares(&self, ability: Ability) -> bool {
        locked(&self.status).as_ref().is_none_or(|status| status.contract.declares(ability))
    }

    /// It gives the ability as last read: declared and ready; messages — its bot's contacts read.
    pub fn gives(&self, ability: Ability) -> bool {
        let status = locked(&self.status);
        let Some(status) = status.as_ref() else { return false };
        match ability {
            Ability::Messages => {
                status.contract.declares(ability) && matches!(*locked(&self.messenger), Messenger::Contacts(_))
            }
            _ => status.contract.ready(ability),
        }
    }

    /// Its hearing and voice clients for its address as it is now: kept while the address stays.
    fn clients(&self) -> Clients {
        let base = self.base();
        let mut clients = locked(&self.clients);
        if let Some((_, made)) = clients.as_ref().filter(|(made_for, _)| *made_for == base) {
            return made.clone();
        }
        let http = crate::utils::http::client_without_redirects();
        let config = AsrConfig {
            enabled: true,
            base_url: format!("{base}/home/v1/asr"),
            api_key: Some(self.credential()),
            model: "home".into(),
            language: self.asr.language.clone(),
            prompt: self.asr.prompt.clone(),
            timeout_ms: self.asr.timeout_ms,
            wake_words: self.asr.wake_words.clone(),
            local: None,
        };
        let recognizer: Arc<dyn SpeechRecognizer> = Arc::new(OpenAiTranscriber::with_client(config, http.clone()));
        let config = OwnSpeechConfig {
            base_url: format!("{base}/home/v1/tts"),
            api_key: Some(self.credential()),
            model: "home".into(),
            voice: "home".into(),
            // The helper answers with Ogg/Opus, ready for the station.
            format: "opus".into(),
            timeout_ms: self.tts_timeout_ms,
        };
        let made: Clients = (recognizer, Arc::new(OwnSpeech::with_client(config, http)));
        *clients = Some((base, made.clone()));
        made
    }

    /// Its hearing, at `/home/v1/asr`.
    pub fn recognizer(&self) -> Arc<dyn SpeechRecognizer> {
        self.clients().0
    }

    /// Its voice — its own choice, its voice lab's too — at `/home/v1/tts`.
    pub fn speech(&self) -> Arc<OwnSpeech> {
        self.clients().1
    }

    pub fn messenger(&self) -> Messenger {
        locked(&self.messenger).clone()
    }

    /// It says its Telegram bot is not set up: its contacts answered so, or its contract of version 1 declares messages
    /// not ready (version 0 declares them so always — its contacts tell). Not while they are only unread.
    pub fn no_bot(&self) -> bool {
        let answered = *locked(&self.messenger) == Messenger::NoBot;
        answered
            || locked(&self.status).as_ref().is_some_and(|status| {
                status.contract.version >= 1
                    && status.contract.declares(Ability::Messages)
                    && !status.contract.ready(Ability::Messages)
            })
    }

    /// For tests: what its bot was seen with.
    #[cfg(test)]
    pub(crate) fn set_messenger(&self, messenger: Messenger) {
        *locked(&self.messenger) = messenger;
    }

    /// For tests: this link as the entry's own, one that never dials (`Link::idle`): its news are told by hand.
    #[cfg(test)]
    pub(crate) fn hold_link(&self, link: Arc<Link>) {
        *locked(&self.link) = Some(link);
    }

    /// The look found this gateway itself: never looked at again while its address stays.
    pub fn mark_itself(&self) {
        self.itself.store(true, Ordering::Relaxed);
    }

    pub fn itself(&self) -> bool {
        self.itself.load(Ordering::Relaxed)
    }
}

/// A request's failure in the system's words: «connection refused», «no route to host», «timed out» — what reqwest
/// keeps in its chain of sources, joined.
pub fn cause_of(error: &reqwest::Error) -> String {
    let mut words = vec![error.to_string()];
    let mut source = std::error::Error::source(error);
    while let Some(inner) = source {
        let text = inner.to_string();
        if !words.iter().any(|word| word.contains(&text)) {
            words.push(text);
        }
        source = inner.source();
    }
    words.join(": ")
}
