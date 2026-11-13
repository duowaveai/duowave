//! «Связи» — the bus and the rights for everything that lives in the system (docs/plans/bus.md). Everything has an
//! address: apps, the gateway's services, folders and files, devices, people. Letters go between them — call a
//! handle, tell news, ask in words — and each pair has one step of the ladder (`level.rs`). Every letter carries the
//! trail of who started its chain (`trail.rs`); the rights of the whole chain never exceed the root's. One rule
//! decides (`policy.rs`); what it lets through a provider carries out (`provider.rs`); a person's «да» is asked like a
//! phone's (`consent.rs`); every letter leaves a line in the journal, never its content (`journal.rs`).
//!
//! The bus knows no participant itself: providers register for kinds of addresses (`providers/`), so a new device,
//! a new kind of thing or a new right is a provider or a handle — the core and its rule stay as they are.

pub mod address;
pub mod cards;
pub mod consent;
pub mod danger;
pub mod journal;
pub mod level;
pub mod links;
pub mod policy;
pub mod provider;
pub mod providers;
pub mod routes;
pub mod trail;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use serde::Serialize;
use serde_json::Value;

pub use address::{Address, Pattern};
pub use level::Level;
pub use provider::{BusError, Handle, Letter, News, Participant, Provider, Words};
pub use trail::{Root, Trail};

use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::core::identity::{can, PrincipalKind};
use crate::platform::http::ApiError;
use crate::settings::SettingsService;
use crate::storage::Db;
use consent::{Consent, Question};
use danger::{danger_of, Danger};
use journal::{Entry, Journal};
use links::{Link, LinkSource, Links};
use policy::{verdict, AskKind, Facts, Reason, Verdict};
use trail::Authority;

/// Calls one app may make to another in a day when no person started the chain: two apps answering each other stop
/// here (crew's `pair_wake_budget`).
pub const PAIR_BUDGET: u32 = 30;

/// The kinds of address that speak for a person: bounded by their own rights, not by links.
const PEOPLE: &[&str] = &[address::PERSON, address::HOME, address::AGENT, address::GATEWAY];

pub struct BusDeps {
    pub db: Arc<Db>,
    pub settings: Arc<SettingsService>,
    pub events: EventBus,
    /// Journal lines kept.
    pub keep_lines: u32,
}

/// What the bus decided for one letter.
pub struct Decision {
    pub provider: Arc<dyn Provider>,
    pub participant: Participant,
    /// The address the letter goes to: the participant itself or something in it (a file in a folder).
    pub target: Address,
    /// What the caller asked for towards it (its manifest): the step a «да» gives.
    pub declared: Level,
    pub handle: Handle,
    pub danger: Option<Danger>,
    pub verdict: Verdict,
    /// The «да» for one dangerous action it would use.
    pub confirmed: bool,
}

/// A participant as one caller sees it: only what it may see, and of its handles what it may do or ask for.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Visible {
    #[serde(flatten)]
    pub participant: Participant,
    pub handles: Vec<VisibleHandle>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisibleHandle {
    #[serde(flatten)]
    pub handle: Handle,
    /// Goes through now.
    pub allowed: bool,
    /// Would ask a person first.
    pub asks: bool,
}

pub struct Bus {
    providers: RwLock<BTreeMap<String, Arc<dyn Provider>>>,
    /// The folders' provider, for the gateway's own work on them (the start's folders, the hourly sweep).
    files: std::sync::OnceLock<Arc<providers::files::Files>>,
    pub links: Links,
    pub consent: Consent,
    pub journal: Journal,
    events: EventBus,
    /// Calls between two apps today when no person started the chain: (from, to) → (day, count).
    pairs: Mutex<HashMap<(String, String), (i64, u32)>>,
    letters: AtomicU64,
}

fn day_now() -> i64 {
    (crate::utils::time::now_ms() / 86_400_000) as i64
}

impl Bus {
    pub fn new(deps: BusDeps) -> Arc<Bus> {
        Arc::new(Bus {
            providers: RwLock::new(BTreeMap::new()),
            files: std::sync::OnceLock::new(),
            links: Links::new(deps.settings),
            consent: Consent::new(),
            journal: Journal::new(deps.db, deps.keep_lines),
            events: deps.events,
            pairs: Mutex::new(HashMap::new()),
            letters: AtomicU64::new(1),
        })
    }

    /// Follows the gateway: a change of `bus.*` past the bus (an import, the settings' API) is read again, and an
    /// app removed leaves no links, mode or questions behind.
    pub fn start(self: &Arc<Self>) {
        let bus = Arc::downgrade(self);
        self.events.on("bus.follow", move |event| {
            let Some(bus) = bus.upgrade() else { return };
            match event.as_ref() {
                GatewayEvent::SettingsChanged { key, .. } if key.starts_with("bus.") => bus.links.forget(),
                GatewayEvent::PackagesChanged { package_id, change, .. } if change == "removed" => {
                    bus.forget(&Address::app(package_id), None);
                }
                _ => {}
            }
        });
    }

    /// A provider answers for its kind from now on (the one before it for that kind goes).
    pub fn register(&self, provider: Arc<dyn Provider>) {
        let kind = provider.kind().to_string();
        self.providers.write().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(kind, provider);
    }

    /// The folders' provider, kept typed for the gateway's own work on them.
    pub fn set_files(&self, files: Arc<providers::files::Files>) {
        let _ = self.files.set(files);
    }

    pub fn files(&self) -> Option<Arc<providers::files::Files>> {
        self.files.get().cloned()
    }

    /// Whether `from` may use `to`'s handle now, for a gate outside the letters (the keyboard's channel): a question
    /// asked of a person when the rule says so; the reason in words when not.
    pub fn may(&self, trail: &Trail, from: &Address, to: &str, handle: &str) -> Result<(), String> {
        let to = Address::parse(to)?;
        let decision = self.decide(trail, from, &to, handle).map_err(|error| error.to_string())?;
        match decision.verdict {
            Verdict::Allow => Ok(()),
            Verdict::Ask(kind) => {
                let question = self.question(kind, trail, from, &decision);
                let why = format!("«{}» хочет {} — ждёт вашего «да»", question.from_title, question.what);
                let (id, new) = self.consent.ask(question);
                if new {
                    self.events.emit(GatewayEvent::BusAsked { id, from: from.to_string(), to: to.to_string() });
                }
                Err(why)
            }
            Verdict::Deny(Reason::NotFound) => Err(format!("нет такого: {to}")),
            Verdict::Deny(_) => Err(format!("нет связи с «{}»", decision.participant.title)),
        }
    }

    pub fn provider(&self, kind: &str) -> Option<Arc<dyn Provider>> {
        self.providers.read().unwrap_or_else(|poisoned| poisoned.into_inner()).get(kind).cloned()
    }

    fn providers(&self) -> Vec<Arc<dyn Provider>> {
        self.providers.read().unwrap_or_else(|poisoned| poisoned.into_inner()).values().cloned().collect()
    }

    /// The title of a participant, for people; its address when no provider knows it.
    pub fn title_of(&self, address: &Address) -> String {
        if let Some(principal_like) = PEOPLE.iter().find(|kind| **kind == address.kind()) {
            return match *principal_like {
                address::HOME => "Дом".into(),
                address::GATEWAY => "Шлюз".into(),
                _ => address.to_string(),
            };
        }
        self.provider(address.kind())
            .and_then(|provider| provider.participant(address))
            .map(|participant| participant.title)
            .unwrap_or_else(|| address.to_string())
    }

    /// What the bus decides for a letter now, without sending it.
    pub fn decide(&self, trail: &Trail, from: &Address, to: &Address, name: &str) -> Result<Decision, BusError> {
        let provider = self.provider(to.kind()).ok_or_else(|| BusError::NotFound(to.to_string()))?;
        let participant = provider.participant(to).ok_or_else(|| BusError::NotFound(to.to_string()))?;
        let handle = provider.handle(to, name).ok_or_else(|| BusError::NotFound(format!("{to}: {name}")))?;
        let caller = self.provider(from.kind());
        let caller_is_person = PEOPLE.contains(&from.kind());
        let declared = caller.as_ref().map(|caller| caller.declared(from, to)).unwrap_or(Level::None);
        let own = provider.owns(from, to);
        let root_may = match (&handle.needs, trail.root.principal()) {
            (Some(right), Some(principal)) => principal.kind == PrincipalKind::System || can(Some(principal), right),
            _ => true,
        };
        let (danger, always) =
            danger_of(handle.danger, &self.links.dangers(), participant.address.as_str(), &handle.name, from.as_str());
        let confirmed = danger.is_some() && self.consent.has_once(from, to, &handle.name);
        let facts = Facts {
            need: handle.level,
            authority: trail.authority(),
            caller_is_person,
            link: self.reach(from, to),
            settled: self.links.link(from, to).is_some_and(|link| link.settled()),
            declared,
            from_mode: if caller_is_person { links::Mode::Rules } else { self.links.mode(from) },
            to_mode: self.links.mode(&participant.address),
            own,
            private: participant.private || handle.private,
            root_may,
            danger,
            always,
            confirmed,
        };
        Ok(Decision {
            verdict: verdict(&facts),
            provider,
            participant,
            target: to.clone(),
            declared,
            handle,
            danger,
            confirmed,
        })
    }

    /// How far `from` reaches `to` by links: the narrowest link the owner made for the pair, or else what the
    /// caller's provider says it has already (an app's older rights).
    pub fn reach(&self, from: &Address, to: &Address) -> Level {
        match self.links.link(from, to) {
            Some(link) => link.level,
            None => self.provider(from.kind()).map(|caller| caller.implicit(from, to)).unwrap_or(Level::None),
        }
    }

    /// Counts a call between two apps that no person started (crew's pair budget).
    fn spend_pair(&self, trail: &Trail, from: &Address, to: &Address) -> Result<(), BusError> {
        if trail.authority() != Authority::App || from.kind() != address::APP || to.kind() != address::APP || from == to
        {
            return Ok(());
        }
        let mut pairs = self.pairs.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let today = day_now();
        pairs.retain(|_, (day, _)| *day == today);
        let entry = pairs.entry((from.to_string(), to.to_string())).or_insert((today, 0));
        entry.1 += 1;
        if entry.1 > PAIR_BUDGET {
            return Err(BusError::Fuse(format!(
                "«{}» сегодня уже {PAIR_BUDGET} раз звало «{}» без человека: хватит до завтра",
                self.title_of(from),
                self.title_of(to)
            )));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn journal(
        &self,
        trail: &Trail,
        kind: &str,
        from: &Address,
        to: &Address,
        handle: &str,
        outcome: &str,
        ms: u64,
        note: Option<&str>,
    ) {
        self.journal.write(Entry {
            trail: trail.id,
            root: &trail.root.label(),
            kind,
            from: from.as_str(),
            to: to.as_str(),
            handle,
            outcome,
            ms,
            note,
        });
    }

    /// The question a person is asked, with words for people.
    /// The phone's question asks for what the app said it would do (its manifest's step, never less than the handle
    /// needs) towards the participant the person is told of — a folder with all in it, when the letter goes to a file.
    fn question(&self, kind: AskKind, trail: &Trail, from: &Address, decision: &Decision) -> Question {
        let to = &decision.participant.address;
        let level = match kind {
            AskKind::Remember => decision.declared.max(decision.handle.level),
            AskKind::Once => decision.handle.level,
        };
        let mut question = Question::new(kind, from.clone(), to.clone(), &decision.handle.name, level);
        if &decision.target != to {
            question.grant = Pattern::below(to);
        }
        question.danger = decision.danger;
        question.from_title = self.title_of(from);
        question.to_title = decision.participant.title.clone();
        let doing = decision.handle.title.clone().unwrap_or_else(|| decision.handle.name.clone());
        question.what = match (kind, decision.danger) {
            (AskKind::Once, Some(danger)) => format!("«{doing}» у «{}» — {}", question.to_title, danger.title()),
            _ => format!("{} «{}»", level.verb(to.kind()), question.to_title),
        };
        question.root = trail.root.title();
        question
    }

    /// Sends a letter: decides, asks a person when it must, carries it out through its provider within the trail,
    /// and writes the journal's line.
    pub async fn call(
        &self,
        trail: Trail,
        from: Address,
        to: Address,
        name: &str,
        input: Value,
        files: Vec<Address>,
    ) -> Result<Value, BusError> {
        if let Err(error) = trail.spend() {
            self.journal(&trail, "call", &from, &to, name, "fuse", 0, Some(&error.to_string()));
            return Err(error.into());
        }
        let decision = match self.decide(&trail, &from, &to, name) {
            Ok(decision) => decision,
            Err(error) => {
                self.journal(&trail, "call", &from, &to, name, "notFound", 0, None);
                return Err(error);
            }
        };
        match decision.verdict {
            Verdict::Allow => {}
            Verdict::Ask(kind) => {
                let question = self.question(kind, &trail, &from, &decision);
                let why = format!("«{}» хочет {}", question.from_title, question.what);
                let (id, new) = self.consent.ask(question);
                if new {
                    self.events.emit(GatewayEvent::BusAsked {
                        id: id.clone(),
                        from: from.to_string(),
                        to: to.to_string(),
                    });
                }
                self.journal(&trail, "call", &from, &to, name, "asked", 0, Some(&why));
                return Err(BusError::NeedsYes { id, why });
            }
            Verdict::Deny(reason) => {
                self.journal(
                    &trail,
                    "call",
                    &from,
                    &to,
                    name,
                    if reason == Reason::NotFound { "notFound" } else { "denied" },
                    0,
                    None,
                );
                return Err(match reason {
                    Reason::NotFound => BusError::NotFound(to.to_string()),
                    Reason::NotAllowed => BusError::Denied(format!(
                        "нет связи: «{}» не может {} «{}»",
                        self.title_of(&from),
                        decision.handle.level.verb(to.kind()),
                        decision.participant.title
                    )),
                    Reason::RootCannot => {
                        BusError::Denied(format!("тому, кто начал ({}), это нельзя", trail.root.title()))
                    }
                });
            }
        }
        if let Err(error) = self.spend_pair(&trail, &from, &to) {
            self.journal(&trail, "call", &from, &to, name, "fuse", 0, Some(&error.to_string()));
            return Err(error);
        }
        if decision.confirmed {
            self.consent.take_once(&from, &to, &decision.handle.name);
        }
        let letter = Letter {
            id: self.letters.fetch_add(1, Ordering::Relaxed),
            from: from.clone(),
            to: to.clone(),
            handle: decision.handle.name.clone(),
            input,
            files,
            trail: trail.clone(),
        };
        let began = Instant::now();
        let result = trail.clone().scope(decision.provider.call(letter)).await;
        let ms = began.elapsed().as_millis() as u64;
        match &result {
            Ok(_) => self.journal(&trail, "call", &from, &to, name, "ok", ms, None),
            Err(error) => self.journal(&trail, "call", &from, &to, name, "failed", ms, Some(&error.to_string())),
        }
        result
    }

    /// Tells news: every listener the rule lets hear it gets it in the background, and the panel hears its name.
    pub fn tell(self: &Arc<Self>, trail: Trail, from: Address, event: &str, payload: Value) {
        self.events.emit(GatewayEvent::BusNews { from: from.to_string(), event: event.to_string() });
        let news = News { from: from.clone(), event: event.to_string(), payload, trail: trail.clone() };
        let mut told = 0;
        for provider in self.providers() {
            for listener in provider.listeners(&from, event) {
                match self.may_listen(&trail, &listener, &from) {
                    Verdict::Allow => {
                        told += 1;
                        let (provider, news, trail, bus, listener, from) = (
                            provider.clone(),
                            news.clone(),
                            trail.clone(),
                            self.clone(),
                            listener.clone(),
                            from.clone(),
                        );
                        tokio::spawn(async move {
                            let began = Instant::now();
                            let result = trail.clone().scope(provider.deliver(&listener, news)).await;
                            let (outcome, note) = match &result {
                                Ok(()) => ("ok", None),
                                Err(error) => ("failed", Some(error.to_string())),
                            };
                            bus.journal(
                                &trail,
                                "deliver",
                                &from,
                                &listener,
                                "news",
                                outcome,
                                began.elapsed().as_millis() as u64,
                                note.as_deref(),
                            );
                        });
                    }
                    Verdict::Ask(_) => {
                        let mut question =
                            Question::new(AskKind::Remember, listener.clone(), from.clone(), "news", Level::Listen);
                        question.from_title = self.title_of(&listener);
                        question.to_title = self.title_of(&from);
                        question.what = format!("слышать новости «{}»", question.to_title);
                        question.root = trail.root.title();
                        let (id, new) = self.consent.ask(question);
                        if new {
                            self.events.emit(GatewayEvent::BusAsked {
                                id,
                                from: listener.to_string(),
                                to: from.to_string(),
                            });
                        }
                    }
                    Verdict::Deny(_) => {}
                }
            }
        }
        self.journal(&trail, "tell", &from, &from, event, "ok", 0, Some(&format!("слушателей: {told}")));
    }

    /// Whether `listener` may hear `from`'s news.
    fn may_listen(&self, trail: &Trail, listener: &Address, from: &Address) -> Verdict {
        let caller = self.provider(listener.kind());
        let facts = Facts {
            need: Level::Listen,
            authority: trail.authority(),
            caller_is_person: PEOPLE.contains(&listener.kind()),
            link: self.reach(listener, from),
            settled: self.links.link(listener, from).is_some_and(|link| link.settled()),
            declared: caller.map(|caller| caller.declared(listener, from)).unwrap_or(Level::None),
            from_mode: self.links.mode(listener),
            to_mode: self.links.mode(from),
            own: listener == from,
            private: false,
            root_may: true,
            danger: None,
            always: false,
            confirmed: false,
        };
        verdict(&facts)
    }

    /// What one caller sees: the participants it may see, each with what it may do or ask for.
    pub fn list(&self, trail: &Trail, from: &Address) -> Vec<Visible> {
        let mut seen = Vec::new();
        for provider in self.providers() {
            for participant in provider.participants() {
                let address = participant.address.clone();
                let handles: Vec<VisibleHandle> = provider
                    .handles(&address)
                    .into_iter()
                    .filter_map(|handle| {
                        let decision = self.decide(trail, from, &address, &handle.name).ok()?;
                        match decision.verdict {
                            Verdict::Allow => Some(VisibleHandle { handle, allowed: true, asks: false }),
                            Verdict::Ask(_) => Some(VisibleHandle { handle, allowed: false, asks: true }),
                            Verdict::Deny(Reason::NotAllowed) => {
                                Some(VisibleHandle { handle, allowed: false, asks: false })
                            }
                            Verdict::Deny(_) => None,
                        }
                    })
                    .collect();
                if !handles.is_empty() && &address != from {
                    seen.push(Visible { participant, handles });
                }
            }
        }
        seen
    }

    /// A person's answer to a question: «да» to the phone's kind becomes a link; to a dangerous action, one use.
    pub fn answer(&self, id: &str, yes: bool, actor: Option<Actor>) -> Result<Option<Question>, ApiError> {
        let Some(question) = self.consent.answer(id, yes) else { return Ok(None) };
        if question.kind == AskKind::Remember {
            // What it has now, by links or its older rights: a «да» never lowers it, a «нет» keeps it as it is.
            let had = self.reach(&question.from, &question.to);
            if had < question.level {
                let (level, source) =
                    if yes { (question.level, LinkSource::Answer) } else { (had, LinkSource::Refused) };
                let mut link = Link::new(Pattern::exact(&question.from), question.grant.clone(), level, source);
                link.by = actor.as_ref().map(|actor| actor.name.clone());
                self.links.grant(link, actor.clone())?;
            }
        }
        let trail = Trail::system();
        self.journal(
            &trail,
            "answer",
            &question.from,
            &question.to,
            &question.handle,
            if yes { "yes" } else { "no" },
            0,
            None,
        );
        self.events.emit(GatewayEvent::BusChanged {
            change: if yes { "yes".into() } else { "no".into() },
            target: Some(question.from.to_string()),
            actor,
        });
        Ok(Some(question))
    }

    /// Tells the panel the links, modes or words changed.
    pub fn changed(&self, change: &str, target: Option<&Address>, actor: Option<Actor>) {
        self.events.emit(GatewayEvent::BusChanged {
            change: change.to_string(),
            target: target.map(ToString::to_string),
            actor,
        });
    }

    /// Everything of a participant gone: its links, mode, questions.
    pub fn forget(&self, address: &Address, actor: Option<Actor>) {
        if let Err(error) = self.links.forget_participant(address, actor) {
            tracing::warn!(target: "bus", "{address}: its links were not removed: {}", error.message);
        }
        self.consent.forget(address);
    }
}

#[cfg(test)]
pub(crate) mod tests;

/// A plugin's `host.bus.*` and `host.files.*` as letters (Host API 2): `bus.call`, `bus.ask`, `bus.tell`, `bus.list` and
/// the folders' `folders.*` — a path without `files/` is inside the app's own folder `files/app/<id>`. In the trail of
/// the call it runs in, else the app's own.
pub fn letter(
    bus: Arc<Bus>,
    caller: &str,
    name: &str,
    input: Value,
    trail: Option<Trail>,
) -> futures::future::BoxFuture<'static, Result<Value, String>> {
    let (caller, name) = (caller.to_string(), name.to_string());
    Box::pin(async move {
        let me = Address::app(&caller);
        let trail = trail.unwrap_or_else(|| Trail::app(&caller));
        let field = |key: &str| input.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        // A folder's address: `files/…` as it is, anything else inside the app's own folder.
        let at = |key: &str| -> Result<Address, String> {
            let given = field(key);
            if given.trim_matches('/').is_empty() {
                Address::parse(&format!("files/app/{caller}"))
            } else if given.starts_with("files/") {
                Address::parse(&given)
            } else {
                Address::parse(&format!("files/app/{caller}/{}", given.trim_matches('/')))
            }
        };
        let text = |error: BusError| error.to_string();
        match name.as_str() {
            "bus.call" => {
                let to = Address::parse(&field("to"))?;
                let files = input
                    .get("files")
                    .and_then(Value::as_array)
                    .map(|files| files.iter().filter_map(Value::as_str).map(Address::parse).collect())
                    .transpose()?
                    .unwrap_or_default();
                let handle = field("handle");
                let letter_input = input.get("input").cloned().unwrap_or(Value::Object(serde_json::Map::new()));
                bus.call(trail, me, to, &handle, letter_input, files).await.map_err(text)
            }
            "bus.ask" => {
                let to = Address::parse(&field("to"))?;
                bus.call(trail, me, to, "ask", serde_json::json!({ "text": field("text") }), vec![]).await.map_err(text)
            }
            "bus.tell" => {
                let event = field("event");
                let apps = bus.provider(address::APP).ok_or_else(|| "приложений на шине нет".to_string())?;
                if apps.handle(&me, &event).is_none() {
                    return Err(format!("новость «{event}» не объявлена в contributes.bus.emits"));
                }
                let payload = input.get("payload").cloned().unwrap_or(Value::Null);
                bus.tell(trail, me, &event, payload);
                Ok(Value::Bool(true))
            }
            "bus.list" => Ok(serde_json::json!(bus.list(&trail, &me))),
            folder if folder.starts_with("folders.") => {
                let handle = folder.trim_start_matches("folders.");
                let mut carried = input.clone();
                if handle == "move" {
                    carried = serde_json::json!({ "to": at("to")?.to_string() });
                }
                bus.call(trail, me, at("at")?, handle, carried, vec![]).await.map_err(text)
            }
            other => Err(format!("нет функции {other}")),
        }
    })
}
