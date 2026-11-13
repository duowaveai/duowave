//! The memory as the rest of the gateway uses it: its state (on, in passing, the pause), the wake-up for the prompt,
//! remembering, forgetting and recalling for the model's tools and the voice commands, the episodes of the day, and
//! what the panel's «Память» window reads and changes. Every change goes into the memory's own log (for
//! «Отменить») and out as `memory.changed`; a person's change also lands in the audit log, without the words.

use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Duration, NaiveTime, TimeZone, Utc};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use super::commands::PauseSpan;
use super::context::{self, PromptItem, PromptLayer, Writing, WAKE_UP_ITEMS, WAKE_UP_TOKENS};
use super::model::{Audience, Channel, EntityKind, FactKind, Origin, Source, SpeakerKind, Viewer, Visibility};
use super::store::{self, Change, Counts, Entity, Fact, NewChange, NewEpisode, NewFact, Note};
use super::text::{self, Lang};
use crate::characters::{CharactersService, Gender};
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::core::identity::principal::{Principal, PrincipalKind};
use crate::dialog::system_prompt::ALICE;
use crate::dialog::PersonaService;
use crate::platform::http::ApiError;
use crate::settings::definitions::{MEMORY, MEMORY_IN_PASSING, MEMORY_PAUSE};
use crate::settings::SettingsService;
use crate::storage::Db;
use crate::utils::time::iso;

/// The words of a conversation are kept this long; what was learned from them stays.
const EPISODE_DAYS: i64 = 7;
/// What the memory may take on a station (texts and their rows); the panel shows it against this.
pub const LIMIT_BYTES: i64 = 8 * 1024 * 1024;
/// A fact is one short sentence.
const MAX_FACT_CHARS: usize = 600;
/// Two facts this alike (both ways) are the same fact said twice.
const SAME_FACT: f32 = 0.85;
/// How much of «забудь …» must be found in a fact to close it.
const FORGET_MATCH: f32 = 0.5;

/// Why the memory did not write.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MemoryError {
    #[error("Память выключена.")]
    Off,
    #[error("Память на паузе: ничего нового не запоминаю.")]
    Paused,
    #[error("Нечего запоминать.")]
    Empty,
    #[error("Слишком длинно: факт — одно короткое предложение.")]
    TooLong,
    #[error("Память заполнена: сотрите лишнее в пульте.")]
    Full,
    #[error("Такой записи нет.")]
    NotFound,
    #[error("Эту перемену отменить нельзя.")]
    NotUndoable,
    #[error("{0}")]
    Bad(String),
    #[error("память: {0}")]
    Db(String),
}

impl From<rusqlite::Error> for MemoryError {
    fn from(error: rusqlite::Error) -> MemoryError {
        MemoryError::Db(error.to_string())
    }
}

impl From<MemoryError> for ApiError {
    fn from(error: MemoryError) -> ApiError {
        match error {
            MemoryError::NotFound => ApiError::not_found(error.to_string()),
            MemoryError::Db(_) => ApiError::internal(error.to_string()),
            MemoryError::Off | MemoryError::Paused | MemoryError::Full | MemoryError::NotUndoable => {
                ApiError::conflict(error.to_string())
            }
            _ => ApiError::bad_request(error.to_string()),
        }
    }
}

/// The memory's switches as the panel shows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryState {
    pub enabled: bool,
    pub in_passing: bool,
    pub paused: bool,
    /// Until when, when the pause has an end.
    pub paused_until: Option<String>,
}

/// Who answers now: whose eyes the wake-up is, and how the replies are worded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Answering {
    pub id: String,
    pub title: String,
    pub feminine: bool,
}

/// What to remember: the model's tool and the panel fill it, a voice command gives only the words.
#[derive(Debug, Clone, Default)]
pub struct Remember {
    /// The fact as one short sentence: «У Маши аллергия на орехи».
    pub fact: String,
    /// Who or what it is about; none — whoever the words name, or the home itself.
    pub about: Option<String>,
    pub about_kind: Option<EntityKind>,
    /// «аллергия», «сестра», «работает в».
    pub relation: Option<String>,
    /// Another one it links to: Петя in «Маша — сестра Пети».
    pub other: Option<String>,
    pub other_kind: Option<EntityKind>,
    /// An old fact this one replaces: it is closed today.
    pub replaces: Option<String>,
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Remembered {
    /// Written now (with the fact it closed, if any; boxed: the two variants stay one size).
    Added { fact: Fact, closed: Option<Box<Fact>> },
    /// The same was known already.
    Known(Fact),
}

impl Remembered {
    pub fn fact(&self) -> &Fact {
        match self {
            Remembered::Added { fact, .. } | Remembered::Known(fact) => fact,
        }
    }
}

/// A fact with the names it points at, for the panel and the tools.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FactView {
    #[serde(flatten)]
    pub fact: Fact,
    pub subject_name: String,
    /// What the subject is: «Дом» itself reads as just «Дом».
    pub subject_kind: Option<EntityKind>,
    pub object_name: Option<String>,
    /// home | personal: the panel colours it.
    pub layer: PromptLayer,
}

/// An entity in the «Дом» list.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityItem {
    #[serde(flatten)]
    pub entity: Entity,
    /// Current facts about it.
    pub facts: usize,
    /// A line under the name: its newest fact.
    pub note: Option<String>,
}

/// A link of the «кто с кем» picture: a fact that joins two entities.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub fact_id: String,
    pub relation: Option<String>,
    pub other_id: String,
    pub other_name: String,
    pub other_kind: EntityKind,
    pub closed: bool,
    pub text: String,
}

/// An entity's card: what is true now, what was, and who it is linked with.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityCard {
    pub entity: Entity,
    pub current: Vec<FactView>,
    pub closed: Vec<FactView>,
    pub links: Vec<Link>,
}

/// «Новое»: a recent record with the line of the log that undoes it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentItem {
    pub change_id: Option<String>,
    #[serde(flatten)]
    pub view: FactView,
}

/// «Глазами персонажа»: exactly what goes into their prompt every time.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Eyes {
    pub character: Answering,
    pub items: Vec<PromptItem>,
    pub tokens: usize,
    pub max_tokens: usize,
    pub max_items: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub state: MemoryState,
    pub counts: Counts,
    pub bytes: i64,
    pub limit_bytes: i64,
    pub recent: Vec<RecentItem>,
    pub eyes: Eyes,
}

pub struct MemoryService {
    db: Arc<Db>,
    settings: Arc<SettingsService>,
    events: EventBus,
    personas: Arc<PersonaService>,
    characters: OnceLock<Arc<CharactersService>>,
}

impl MemoryService {
    pub fn new(
        db: Arc<Db>,
        settings: Arc<SettingsService>,
        events: EventBus,
        personas: Arc<PersonaService>,
    ) -> MemoryService {
        MemoryService { db, settings, events, personas, characters: OnceLock::new() }
    }

    /// The packages' characters, built later than the memory: their titles and genders.
    pub fn attach_characters(&self, characters: Arc<CharactersService>) {
        let _ = self.characters.set(characters);
    }

    /// The memory's tables, for the reflection next door.
    pub(super) fn db(&self) -> &Arc<Db> {
        &self.db
    }

    // ───────── State ─────────

    pub fn enabled(&self) -> bool {
        self.settings.get(&MEMORY, None).unwrap_or(true)
    }

    pub fn in_passing(&self) -> bool {
        self.settings.get(&MEMORY_IN_PASSING, None).unwrap_or(true)
    }

    /// The pause now: none, until a time, or until turned back on (`Some(None)`).
    fn pause_now(&self, now: DateTime<Utc>) -> Option<Option<DateTime<Utc>>> {
        let stored = self.settings.get(&MEMORY_PAUSE, None).unwrap_or_default();
        match stored.as_str() {
            "" => None,
            "manual" => Some(None),
            at => match DateTime::parse_from_rfc3339(at) {
                Ok(until) if until.with_timezone(&Utc) > now => Some(Some(until.with_timezone(&Utc))),
                _ => None,
            },
        }
    }

    pub fn state(&self) -> MemoryState {
        let pause = self.pause_now(Utc::now());
        MemoryState {
            enabled: self.enabled(),
            in_passing: self.in_passing(),
            paused: pause.is_some(),
            paused_until: pause.flatten().map(iso),
        }
    }

    /// Whether anything new may be written now: on and not paused.
    pub fn writing(&self) -> Result<(), MemoryError> {
        if !self.enabled() {
            return Err(MemoryError::Off);
        }
        if self.pause_now(Utc::now()).is_some() {
            return Err(MemoryError::Paused);
        }
        Ok(())
    }

    pub fn set_enabled(&self, on: bool, actor: Option<Actor>) -> Result<MemoryState, ApiError> {
        self.settings.set(&MEMORY, &on, actor)?;
        Ok(self.state())
    }

    pub fn set_in_passing(&self, on: bool, actor: Option<Actor>) -> Result<MemoryState, ApiError> {
        self.settings.set(&MEMORY_IN_PASSING, &on, actor)?;
        Ok(self.state())
    }

    /// «Не запоминай ничего до утра»: returns the state and, for a pause till a time, that time on the station's
    /// clock.
    pub fn pause(
        &self,
        span: PauseSpan,
        timezone: Option<&str>,
        actor: Option<Actor>,
    ) -> Result<(MemoryState, Option<NaiveTime>), ApiError> {
        let now = Utc::now();
        let (value, local) = match span {
            PauseSpan::Hour => (iso(now + Duration::hours(1)), None),
            PauseSpan::Morning => {
                let (at, local) = morning_after(now, timezone);
                (iso(at), Some(local))
            }
            PauseSpan::UntilResumed => ("manual".to_string(), None),
        };
        self.settings.set(&MEMORY_PAUSE, &value, actor)?;
        Ok((self.state(), local))
    }

    /// The end of the pause on the station's clock (its time zone, else this machine's); none without an end.
    pub fn paused_until_on_clock(&self, timezone: Option<&str>) -> Option<NaiveTime> {
        let until = self.pause_now(Utc::now()).flatten()?;
        Some(match timezone.and_then(|name| name.parse::<chrono_tz::Tz>().ok()) {
            Some(zone) => until.with_timezone(&zone).time(),
            None => until.with_timezone(&chrono::Local).time(),
        })
    }

    pub fn resume(&self, actor: Option<Actor>) -> Result<MemoryState, ApiError> {
        self.settings.set(&MEMORY_PAUSE, &String::new(), actor)?;
        Ok(self.state())
    }

    // ───────── Who answers ─────────

    pub fn answering(&self) -> Answering {
        let id = self.personas.chosen().and_then(|persona| persona.preset_id).unwrap_or_else(|| ALICE.to_string());
        let character = self.characters.get().and_then(|characters| characters.character(&id));
        match character {
            Some(character) => {
                Answering { feminine: !matches!(character.gender, Some(Gender::Male)), title: character.title, id }
            }
            None => Answering { id, title: "Алиса".into(), feminine: true },
        }
    }

    /// A character by id: its title and gender, for the reflection's diaries and «За ночь».
    pub fn character(&self, id: &str) -> Answering {
        match self.characters.get().and_then(|characters| characters.character(id)) {
            Some(character) => Answering {
                feminine: !matches!(character.gender, Some(Gender::Male)),
                title: character.title,
                id: id.to_string(),
            },
            None if id == ALICE => Answering { id: id.to_string(), title: "Алиса".into(), feminine: true },
            None => Answering { id: id.to_string(), title: id.to_string(), feminine: true },
        }
    }

    /// What the answering character may see when nobody in particular is known: the voice of the house.
    pub fn viewer(&self) -> Viewer {
        Viewer { person: None, character: Some(self.answering().id), world: None }
    }

    /// What a signed-in person may see: their own records too, once the memory knows which entity they are.
    pub fn viewer_for(&self, principal: &Principal) -> Viewer {
        let person = match principal.kind {
            PrincipalKind::User => self
                .db
                .with(store::entities)
                .ok()
                .and_then(|list| list.into_iter().find(|e| e.account_id.as_deref() == Some(principal.id.as_str())))
                .map(|e| e.id),
            _ => None,
        };
        Viewer { person, character: Some(self.answering().id), world: None }
    }

    // ───────── The prompt ─────────

    /// The wake-up: the most important records the viewer may see, capped.
    pub fn wake_up(&self, viewer: &Viewer) -> Vec<PromptItem> {
        if !self.enabled() {
            return Vec::new();
        }
        let facts = self.db.with(|c| store::visible_facts(c, viewer, true)).unwrap_or_default();
        let items = facts
            .into_iter()
            .filter(|fact| matches!(fact.kind, FactKind::Fact | FactKind::Opinion | FactKind::Todo))
            .map(|fact| PromptItem {
                layer: layer_of(&fact),
                updated_at: DateTime::parse_from_rfc3339(&fact.updated_at)
                    .map(|at| at.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                id: fact.id,
                text: fact.text,
                pinned: fact.pinned,
                importance: fact.importance,
                mentions: fact.mentions,
            })
            .collect::<Vec<_>>();
        let mut items = items;
        items.extend(self.notes_for(viewer));
        context::pick_wake_up(items, WAKE_UP_ITEMS, WAKE_UP_TOKENS)
    }

    /// What the reflection wrote lately: the house's last day summary, the character's last diary entry — for a
    /// few days, cut short.
    fn notes_for(&self, viewer: &Viewer) -> Vec<PromptItem> {
        let since = (Utc::now() - Duration::days(NOTE_DAYS)).format("%Y-%m-%d").to_string();
        let found = self.db.with(|c| {
            let day = store::latest_note(c, "day", None, &since)?;
            let diary = match viewer.character.as_deref() {
                Some(character) => store::latest_note(c, "diary", Some(character), &since)?,
                None => None,
            };
            Ok([day, diary])
        });
        found
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .map(|note| PromptItem {
                text: note_line(&note),
                layer: if note.character_id.is_some() { PromptLayer::Personal } else { PromptLayer::Home },
                updated_at: DateTime::parse_from_rfc3339(&note.updated_at)
                    .map(|at| at.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                id: note.id,
                pinned: false,
                importance: 2,
                mentions: 0,
            })
            .collect()
    }

    /// The memory's block for a character's prompt, in the prompt's language: the wake-up and how to use the memory
    /// tools offered in this answer (`offered`: their names). Empty when the memory is off and nothing is offered.
    pub fn prompt(&self, lang: Lang, offered: &[&str]) -> String {
        if !self.enabled() {
            return String::new();
        }
        let items = self.wake_up(&self.viewer());
        let ids: Vec<String> = items.iter().map(|item| item.id.clone()).collect();
        if !ids.is_empty() {
            let _ = self.db.with(|c| store::prompted(c, &ids));
        }
        let remember = offered.contains(&"remember");
        let writing =
            Writing { remember, in_passing: remember && self.in_passing(), recall: offered.contains(&"recall") };
        context::render(lang, &items, writing)
    }

    // ───────── Writing ─────────

    /// Writes a fact: finds or adds who it is about, skips one known already, closes the one it replaces.
    pub fn remember(
        &self,
        input: Remember,
        origin: Origin,
        source: Source,
        actor: Option<&Principal>,
    ) -> Result<Remembered, MemoryError> {
        self.writing()?;
        let said = input.fact.trim().trim_end_matches('.').trim().to_string();
        if said.is_empty() {
            return Err(MemoryError::Empty);
        }
        if said.chars().count() > MAX_FACT_CHARS {
            return Err(MemoryError::TooLong);
        }
        if self.db.with(store::approx_bytes)? > LIMIT_BYTES {
            return Err(MemoryError::Full);
        }
        let viewer = Viewer::default();
        let home_name = if text::lang_of(&said) == Lang::En { "Home" } else { "Дом" };
        let said = match source {
            Source::Voice => polish(&said, &self.db.with(store::entities)?),
            _ => said,
        };
        let result = self.db.transaction(|c| {
            let subject = match input.about.as_deref().map(str::trim).filter(|about| !about.is_empty()) {
                Some(about) if is_home(about) => store::household(c, home_name)?,
                Some(about) => match store::find_entity(c, about)? {
                    Some(found) => found.id,
                    None => {
                        store::insert_entity(c, input.about_kind.unwrap_or(EntityKind::Person), &title_case(about))?.id
                    }
                },
                None => {
                    let named = store::mentioned_entities(c, &said)?;
                    match named.as_slice() {
                        [only] => only.id.clone(),
                        _ => store::household(c, home_name)?,
                    }
                }
            };
            let object = match input.other.as_deref().map(str::trim).filter(|other| !other.is_empty()) {
                Some(other) => Some(match store::find_entity(c, other)? {
                    Some(found) => found.id,
                    None => {
                        store::insert_entity(c, input.other_kind.unwrap_or(EntityKind::Person), &title_case(other))?.id
                    }
                }),
                None => None,
            };
            let current = store::facts_of(c, &viewer, &subject)?;
            if let Some(known) =
                current.iter().filter(|fact| fact.valid_to.is_none() && fact.subject_id == subject).find(|fact| {
                    text::overlap(&said, &fact.text) >= SAME_FACT && text::overlap(&fact.text, &said) >= SAME_FACT
                })
            {
                return Ok(Remembered::Known(known.clone()));
            }
            let fact = store::insert_fact(
                c,
                &NewFact {
                    subject_id: subject.clone(),
                    relation: input.relation.clone().map(|r| r.trim().to_string()).filter(|r| !r.is_empty()),
                    object_id: object,
                    value: None,
                    text: said.clone(),
                    kind: FactKind::Fact,
                    viewpoint: None,
                    valid_from: None,
                    confidence: 1.0,
                    importance: 0,
                    pinned: input.pinned,
                    source,
                    episode_id: None,
                    origin: origin.clone(),
                    visibility: Visibility::Household,
                    visibility_owner: None,
                    owner_id: None,
                },
            )?;
            let (kind, id) = actor_columns(actor, &origin);
            store::log_change(
                c,
                NewChange {
                    actor_kind: &kind,
                    actor_id: id.as_deref(),
                    action: "add",
                    target: "fact",
                    target_id: &fact.id,
                    summary: &fact.text,
                    before: None,
                    after: serde_json::to_string(&fact).ok(),
                    reason: None,
                    run_id: None,
                },
            )?;
            let closed = match input.replaces.as_deref().map(str::trim).filter(|old| !old.is_empty()) {
                Some(old) => {
                    let replaced = current
                        .iter()
                        .filter(|f| f.valid_to.is_none() && f.id != fact.id)
                        .map(|f| (text::overlap(old, &f.text), f))
                        .filter(|(score, _)| *score >= FORGET_MATCH)
                        .max_by(|a, b| a.0.total_cmp(&b.0))
                        .map(|(_, f)| f.clone());
                    if let Some(old) = &replaced {
                        store::close_fact(c, &old.id, "заменён новым", Some(&fact.id))?;
                        store::log_change(
                            c,
                            NewChange {
                                actor_kind: &kind,
                                actor_id: id.as_deref(),
                                action: "close",
                                target: "fact",
                                target_id: &old.id,
                                summary: &old.text,
                                before: serde_json::to_string(old).ok(),
                                after: None,
                                reason: Some("заменён новым"),
                                run_id: None,
                            },
                        )?;
                    }
                    replaced
                }
                None => None,
            };
            Ok(Remembered::Added { fact, closed: closed.map(Box::new) })
        })?;
        if let Remembered::Added { fact, closed } = &result {
            self.changed("add", "fact", &fact.id, actor);
            if let Some(closed) = closed {
                self.changed("close", "fact", &closed.id, actor);
            }
        }
        Ok(result)
    }

    /// «Забудь …»: closes the current fact the words fit best. Forgetting is allowed during a pause: it writes
    /// nothing new.
    pub fn forget(&self, what: &str, viewer: &Viewer, actor: Option<&Principal>) -> Result<Option<Fact>, MemoryError> {
        if !self.enabled() {
            return Err(MemoryError::Off);
        }
        let closed = self.db.transaction(|c| {
            let entities = store::entities(c)?;
            let best = store::visible_facts(c, viewer, true)?
                .into_iter()
                .map(|fact| {
                    let names = names_of(&entities, &fact.subject_id);
                    let score = text::overlap(what, &format!("{} {names}", fact.text));
                    (score, fact)
                })
                .filter(|(score, _)| *score >= FORGET_MATCH)
                .max_by(|a, b| a.0.total_cmp(&b.0).then(a.1.created_at.cmp(&b.1.created_at)))
                .map(|(_, fact)| fact);
            let Some(fact) = best else { return Ok(None) };
            store::close_fact(c, &fact.id, "забыть просили", None)?;
            let (kind, id) = actor_columns(actor, &Origin::system());
            store::log_change(
                c,
                NewChange {
                    actor_kind: &kind,
                    actor_id: id.as_deref(),
                    action: "close",
                    target: "fact",
                    target_id: &fact.id,
                    summary: &fact.text,
                    before: serde_json::to_string(&fact).ok(),
                    after: None,
                    reason: Some("забыть просили"),
                    run_id: None,
                },
            )?;
            Ok(Some(fact))
        })?;
        if let Some(fact) = &closed {
            self.changed("close", "fact", &fact.id, actor);
        }
        Ok(closed)
    }

    /// What the memory knows that fits the words: the named entity's facts first, then the best word matches.
    pub fn recall(&self, query: &str, viewer: &Viewer, limit: usize) -> Result<Vec<FactView>, MemoryError> {
        if !self.enabled() {
            return Ok(Vec::new());
        }
        let found = self.db.with(|c| {
            let entities = store::entities(c)?;
            let facts = store::visible_facts(c, viewer, true)?;
            let named: Vec<String> = match store::find_entity(c, query)? {
                Some(entity) => vec![entity.id],
                None => store::mentioned_entities(c, query)?.into_iter().map(|e| e.id).collect(),
            };
            let mut scored: Vec<(f32, Fact)> = facts
                .into_iter()
                .map(|fact| {
                    let about = named.iter().any(|id| *id == fact.subject_id || fact.object_id.as_ref() == Some(id));
                    let names = names_of(&entities, &fact.subject_id);
                    let words = text::overlap(query, &format!("{} {names}", fact.text));
                    (if about { 1.0 + words } else { words }, fact)
                })
                .filter(|(score, _)| *score > 0.0)
                .collect();
            scored.sort_by(|a, b| {
                b.0.total_cmp(&a.0).then(b.1.pinned.cmp(&a.1.pinned)).then(b.1.created_at.cmp(&a.1.created_at))
            });
            let picked: Vec<Fact> = scored.into_iter().take(limit).map(|(_, fact)| fact).collect();
            let ids: Vec<String> = picked.iter().map(|fact| fact.id.clone()).collect();
            store::mention(c, &ids)?;
            Ok(picked.into_iter().map(|fact| view_of(&entities, fact)).collect())
        })?;
        Ok(found)
    }

    /// «Что ты помнишь про Машу?»: the entity's current facts, or the best matches when no entity has that name.
    pub fn about(&self, name: &str, viewer: &Viewer, limit: usize) -> Result<Vec<FactView>, MemoryError> {
        self.recall(name, viewer, limit)
    }

    // ───────── Episodes ─────────

    /// One exchange of a conversation, word for word for 7 days: a station's (its device tag is the key) or a
    /// person's in the panel (`panel:<user>`). The workshop's talk is not the household's. Nothing while the memory
    /// is off or paused.
    pub fn episode(&self, key: &str, question: &str, answer: &str) {
        // The workshop's agent and apps asking each other in words (src/links/bus_words.rs) are no episodes of a life.
        if key.starts_with("workshop:")
            || key.starts_with("bus:")
            || question.trim().is_empty()
            || answer.trim().is_empty()
        {
            return;
        }
        if self.writing().is_err() {
            return;
        }
        let character = self.answering().id;
        let (station, origin) = match key.strip_prefix("panel:") {
            Some(user) => (None, Origin::panel_user(user)),
            None => (Some(key.to_string()), Origin::station_voice(Some(key), Some(&character))),
        };
        let episode = NewEpisode {
            station,
            character_id: Some(character),
            question: question.trim().to_string(),
            answer: answer.trim().to_string(),
            origin,
        };
        let before = iso(Utc::now() - Duration::days(EPISODE_DAYS));
        let written = self.db.transaction(|c| {
            store::prune_episodes(c, &before)?;
            if store::approx_bytes(c)? > LIMIT_BYTES {
                return Ok(false);
            }
            store::insert_episode(c, &episode)?;
            Ok(true)
        });
        match written {
            Ok(true) => {}
            Ok(false) => tracing::warn!(target: "memory", "memory is full: the episode is not kept"),
            Err(error) => tracing::warn!(target: "memory", "episode not kept: {error}"),
        }
    }

    // ───────── The panel ─────────

    pub fn overview(&self, viewer: &Viewer) -> Result<Overview, MemoryError> {
        let (counts, bytes, recent) = self.db.with(|c| {
            let counts = store::counts(c, viewer)?;
            let bytes = store::approx_bytes(c)?;
            let entities = store::entities(c)?;
            let changes = store::recent_changes(c, 40)?;
            let mut recent = Vec::new();
            for fact in store::visible_facts(c, viewer, false)?.into_iter().take(8) {
                let change_id = changes
                    .iter()
                    .find(|change| change.target_id == fact.id && change.action == "add" && change.undone_at.is_none())
                    .map(|change| change.id.clone());
                recent.push(RecentItem { change_id, view: view_of(&entities, fact) });
            }
            Ok((counts, bytes, recent))
        })?;
        let items = self.wake_up(viewer);
        let eyes = Eyes {
            character: self.answering(),
            tokens: context::wake_up_tokens(&items),
            items,
            max_tokens: WAKE_UP_TOKENS,
            max_items: WAKE_UP_ITEMS,
        };
        Ok(Overview { state: self.state(), counts, bytes, limit_bytes: LIMIT_BYTES, recent, eyes })
    }

    /// The «Дом» list: everyone and everything the household graph knows, with their fact counts.
    pub fn entity_list(&self, viewer: &Viewer) -> Result<Vec<EntityItem>, MemoryError> {
        Ok(self.db.with(|c| {
            let facts = store::visible_facts(c, viewer, true)?;
            let list = store::entities(c)?
                .into_iter()
                .map(|entity| {
                    // About it either way: «Маша — сестра Пети» is about Петя too.
                    let about: Vec<&Fact> = facts
                        .iter()
                        .filter(|f| f.subject_id == entity.id || f.object_id.as_deref() == Some(entity.id.as_str()))
                        .collect();
                    EntityItem {
                        note: about
                            .iter()
                            .find(|f| f.subject_id == entity.id)
                            .or(about.first())
                            .map(|f| f.text.clone()),
                        facts: about.len(),
                        entity,
                    }
                })
                .collect();
            Ok(list)
        })?)
    }

    pub fn entity_card(&self, id: &str, viewer: &Viewer) -> Result<EntityCard, MemoryError> {
        self.db
            .with(|c| {
                let entities = store::entities(c)?;
                let Some(entity) = entities.iter().find(|e| e.id == id).cloned() else { return Ok(None) };
                let facts = store::facts_of(c, viewer, id)?;
                let mut links = Vec::new();
                for fact in &facts {
                    let other =
                        if fact.subject_id == id { fact.object_id.as_deref() } else { Some(fact.subject_id.as_str()) };
                    if let Some(other) = other.and_then(|other| entities.iter().find(|e| e.id == other)) {
                        links.push(Link {
                            fact_id: fact.id.clone(),
                            relation: fact.relation.clone(),
                            other_id: other.id.clone(),
                            other_name: other.name.clone(),
                            other_kind: other.kind,
                            closed: fact.valid_to.is_some(),
                            text: fact.text.clone(),
                        });
                    }
                }
                let (current, closed): (Vec<Fact>, Vec<Fact>) = facts.into_iter().partition(|f| f.valid_to.is_none());
                Ok(Some(EntityCard {
                    entity,
                    current: current.into_iter().map(|f| view_of(&entities, f)).collect(),
                    closed: closed.into_iter().map(|f| view_of(&entities, f)).collect(),
                    links,
                }))
            })?
            .ok_or(MemoryError::NotFound)
    }

    pub fn add_entity(&self, kind: EntityKind, name: &str, actor: &Principal) -> Result<Entity, MemoryError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(MemoryError::Bad("Нужно имя.".into()));
        }
        if kind == EntityKind::Household {
            return Err(MemoryError::Bad("Дом в памяти один.".into()));
        }
        let entity = self.db.transaction(|c| {
            let entity = store::insert_entity(c, kind, name)?;
            self.log_person(
                c,
                actor,
                "add",
                "entity",
                &entity.id,
                &entity.name,
                None,
                serde_json::to_string(&entity).ok(),
            )?;
            Ok(entity)
        })?;
        self.changed("add", "entity", &entity.id, Some(actor));
        Ok(entity)
    }

    /// Renames, and sets the other names to exactly `aliases` when given.
    pub fn update_entity(
        &self,
        id: &str,
        name: Option<&str>,
        aliases: Option<&[String]>,
        actor: &Principal,
    ) -> Result<Entity, MemoryError> {
        let entity = self.db.transaction(|c| {
            let Some(before) = store::entity(c, id)? else { return Ok(None) };
            if let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) {
                store::rename_entity(c, id, name)?;
            }
            if let Some(aliases) = aliases {
                for old in &before.aliases {
                    store::remove_alias(c, id, old)?;
                }
                for alias in aliases.iter().map(|a| a.trim()).filter(|a| !a.is_empty()) {
                    store::add_alias(c, id, alias)?;
                }
            }
            let after = store::entity(c, id)?;
            self.log_person(
                c,
                actor,
                "edit",
                "entity",
                id,
                &before.name,
                serde_json::to_string(&before).ok(),
                after.as_ref().and_then(|a| serde_json::to_string(a).ok()),
            )?;
            Ok(after)
        })?;
        let entity = entity.ok_or(MemoryError::NotFound)?;
        self.changed("edit", "entity", id, Some(actor));
        Ok(entity)
    }

    /// Two entries for one: everything moves to `into`, the other's name becomes another name.
    pub fn merge_entities(&self, from: &str, into: &str, actor: &Principal) -> Result<Entity, MemoryError> {
        if from == into {
            return Err(MemoryError::Bad("Это одна и та же запись.".into()));
        }
        let entity = self.db.transaction(|c| {
            let (Some(old), Some(_)) = (store::entity(c, from)?, store::entity(c, into)?) else { return Ok(None) };
            if old.kind == EntityKind::Household {
                return Ok(None);
            }
            store::merge_entities(c, from, into)?;
            self.log_person(c, actor, "merge", "entity", into, &old.name, serde_json::to_string(&old).ok(), None)?;
            store::entity(c, into)
        })?;
        let entity = entity.ok_or(MemoryError::NotFound)?;
        self.changed("merge", "entity", into, Some(actor));
        Ok(entity)
    }

    /// Erases an entity and every fact about it: for good.
    pub fn delete_entity(&self, id: &str, actor: &Principal) -> Result<(), MemoryError> {
        let gone = self.db.transaction(|c| {
            let Some(entity) = store::entity(c, id)? else { return Ok(false) };
            if entity.kind == EntityKind::Household {
                return Ok(false);
            }
            for fact in store::facts_of(c, &Viewer::default(), id)?.iter().filter(|f| f.subject_id == id) {
                store::erase_fact(c, &fact.id)?;
            }
            store::scrub_changes(c, id)?;
            store::delete_entity(c, id)?;
            self.log_person(c, actor, "erase", "entity", id, "", None, None)?;
            Ok(true)
        })?;
        if !gone {
            return Err(MemoryError::NotFound);
        }
        self.changed("erase", "entity", id, Some(actor));
        Ok(())
    }

    /// A fact added by hand in the panel: written even during a pause (a person in the panel decides), not when
    /// the memory is off.
    pub fn add_fact(
        &self,
        subject_id: &str,
        said: &str,
        pinned: bool,
        actor: &Principal,
    ) -> Result<FactView, MemoryError> {
        if !self.enabled() {
            return Err(MemoryError::Off);
        }
        let said = said.trim();
        if said.is_empty() {
            return Err(MemoryError::Empty);
        }
        if said.chars().count() > MAX_FACT_CHARS {
            return Err(MemoryError::TooLong);
        }
        let origin = Origin::panel_user(&actor.id);
        let view = self.db.transaction(|c| {
            if store::entity(c, subject_id)?.is_none() {
                return Ok(None);
            }
            let fact = store::insert_fact(
                c,
                &NewFact {
                    subject_id: subject_id.to_string(),
                    relation: None,
                    object_id: None,
                    value: None,
                    text: said.to_string(),
                    kind: FactKind::Fact,
                    viewpoint: None,
                    valid_from: None,
                    confidence: 1.0,
                    importance: 0,
                    pinned,
                    source: Source::Panel,
                    episode_id: None,
                    origin: origin.clone(),
                    visibility: Visibility::Household,
                    visibility_owner: None,
                    owner_id: None,
                },
            )?;
            self.log_person(c, actor, "add", "fact", &fact.id, &fact.text, None, serde_json::to_string(&fact).ok())?;
            let entities = store::entities(c)?;
            Ok(Some(view_of(&entities, fact)))
        })?;
        let view = view.ok_or(MemoryError::NotFound)?;
        self.changed("add", "fact", &view.fact.id, Some(actor));
        Ok(view)
    }

    /// «Исправить» and «Закрепить»: new words, or always in the prompt.
    pub fn update_fact(
        &self,
        id: &str,
        said: Option<&str>,
        pinned: Option<bool>,
        actor: &Principal,
    ) -> Result<FactView, MemoryError> {
        let view = self.db.transaction(|c| {
            let Some(before) = store::fact(c, id)? else { return Ok(None) };
            if let Some(said) = said.map(str::trim) {
                if said.is_empty() {
                    return Err(rusqlite::Error::InvalidParameterName("empty".into()));
                }
                store::edit_fact(c, id, said)?;
            }
            if let Some(pinned) = pinned {
                store::pin_fact(c, id, pinned)?;
            }
            let after = store::fact(c, id)?.expect("the fact is there");
            let action = match (said, pinned) {
                (None, Some(true)) => "pin",
                (None, Some(false)) => "unpin",
                _ => "edit",
            };
            self.log_person(
                c,
                actor,
                action,
                "fact",
                id,
                &after.text,
                serde_json::to_string(&before).ok(),
                serde_json::to_string(&after).ok(),
            )?;
            let entities = store::entities(c)?;
            Ok(Some((action, view_of(&entities, after))))
        });
        let (action, view) = match view {
            Ok(Some(found)) => found,
            Ok(None) => return Err(MemoryError::NotFound),
            Err(rusqlite::Error::InvalidParameterName(_)) => return Err(MemoryError::Empty),
            Err(error) => return Err(error.into()),
        };
        self.changed(action, "fact", id, Some(actor));
        Ok(view)
    }

    /// «Больше не так»: the fact goes into the history with today's date.
    pub fn close_fact(&self, id: &str, reason: Option<&str>, actor: &Principal) -> Result<(), MemoryError> {
        let reason = reason.map(str::trim).filter(|r| !r.is_empty()).unwrap_or("больше не так");
        let done = self.db.transaction(|c| {
            let Some(fact) = store::fact(c, id)? else { return Ok(false) };
            if store::close_fact(c, id, reason, None)? == 0 {
                return Ok(true);
            }
            self.log_person(c, actor, "close", "fact", id, &fact.text, serde_json::to_string(&fact).ok(), None)?;
            Ok(true)
        })?;
        if !done {
            return Err(MemoryError::NotFound);
        }
        self.changed("close", "fact", id, Some(actor));
        Ok(())
    }

    pub fn reopen_fact(&self, id: &str, actor: &Principal) -> Result<(), MemoryError> {
        let done = self.db.transaction(|c| {
            let Some(fact) = store::fact(c, id)? else { return Ok(false) };
            store::reopen_fact(c, id)?;
            self.log_person(c, actor, "reopen", "fact", id, &fact.text, None, None)?;
            Ok(true)
        })?;
        if !done {
            return Err(MemoryError::NotFound);
        }
        self.changed("reopen", "fact", id, Some(actor));
        Ok(())
    }

    /// «Стереть совсем»: the fact and its words in the log are gone; nothing to undo.
    pub fn erase_fact(&self, id: &str, actor: &Principal) -> Result<(), MemoryError> {
        let done = self.db.transaction(|c| {
            if store::fact(c, id)?.is_none() {
                return Ok(false);
            }
            store::erase_fact(c, id)?;
            self.log_person(c, actor, "erase", "fact", id, "", None, None)?;
            Ok(true)
        })?;
        if !done {
            return Err(MemoryError::NotFound);
        }
        self.changed("erase", "fact", id, Some(actor));
        Ok(())
    }

    /// «Отменить» a line of the log: a new record goes, a closed or glued one is open again, an edit goes back, another
    /// name is gone, a night's note goes or gets its words back.
    pub fn undo(&self, change_id: &str, actor: &Principal) -> Result<(), MemoryError> {
        let target = self.db.transaction(|c| {
            let Some(change) = store::change(c, change_id)? else { return Ok(Err(MemoryError::NotFound)) };
            if change.undone_at.is_some() {
                return Ok(Err(MemoryError::NotUndoable));
            }
            let Some(summary) = undo_change(c, &change)? else { return Ok(Err(MemoryError::NotUndoable)) };
            store::mark_undone(c, change_id)?;
            self.log_person(c, actor, "undo", &change.target, &change.target_id, &summary, None, None)?;
            Ok(Ok((change.target, change.target_id)))
        })??;
        self.changed("undo", &target.0, &target.1, Some(actor));
        Ok(())
    }

    /// «Откатить ночь»: every change of the night undone, the newest first, and what it thought over waits for the
    /// next night. How many changes went back.
    pub fn roll_back_run(&self, run_id: &str, actor: &Principal) -> Result<usize, MemoryError> {
        let undone = self.db.transaction(|c| {
            let changes = store::changes_of_run(c, run_id)?;
            let mut undone = 0;
            for change in changes.iter().rev().filter(|change| change.undone_at.is_none()) {
                if undo_change(c, change)?.is_some() {
                    store::mark_undone(c, &change.id)?;
                    undone += 1;
                }
            }
            store::unmark_reflected(c, run_id)?;
            self.log_person(c, actor, "rollback", "run", run_id, &format!("перемен: {undone}"), None, None)?;
            Ok(undone)
        })?;
        self.changed("rollback", "run", run_id, Some(actor));
        Ok(undone)
    }

    pub fn changes(&self, limit: i64) -> Result<Vec<Change>, MemoryError> {
        Ok(self.db.with(|c| store::recent_changes(c, limit))?)
    }

    // ───────── Helpers ─────────

    #[allow(clippy::too_many_arguments)]
    fn log_person(
        &self,
        c: &rusqlite::Connection,
        actor: &Principal,
        action: &str,
        target: &str,
        target_id: &str,
        summary: &str,
        before: Option<String>,
        after: Option<String>,
    ) -> rusqlite::Result<String> {
        store::log_change(
            c,
            NewChange {
                actor_kind: actor.kind.as_str(),
                actor_id: Some(&actor.id),
                action,
                target,
                target_id,
                summary,
                before,
                after,
                reason: None,
                run_id: None,
            },
        )
    }

    /// Tells the panel and the audit log; a person (signed in, a key) is the actor, the household's voice, the model
    /// and the night are not.
    pub(super) fn changed(&self, action: &str, target: &str, target_id: &str, actor: Option<&Principal>) {
        let actor = actor.filter(|p| matches!(p.kind, PrincipalKind::User | PrincipalKind::ApiKey)).map(Actor::from);
        self.events.emit(GatewayEvent::MemoryChanged {
            action: action.to_string(),
            target: target.to_string(),
            target_id: target_id.to_string(),
            actor,
        });
    }
}

/// How long the reflection's notes stay in the wake-up.
const NOTE_DAYS: i64 = 3;
/// How much of a note goes into the prompt.
const NOTE_CHARS: usize = 500;

/// A note as a line of the wake-up: «Итог дня 12.11: …», «Мой дневник 12.11: …».
fn note_line(note: &Note) -> String {
    let day = match (note.day.get(8..10), note.day.get(5..7)) {
        (Some(d), Some(m)) => format!("{d}.{m}"),
        _ => note.day.clone(),
    };
    let head = match (note.lang.as_str(), note.character_id.is_some()) {
        ("en", false) => "Day summary",
        ("en", true) => "My diary",
        (_, false) => "Итог дня",
        (_, true) => "Мой дневник",
    };
    let mut text: String = note.text.chars().take(NOTE_CHARS).collect();
    if note.text.chars().count() > NOTE_CHARS {
        text.push('…');
    }
    format!("{head} {day}: {text}")
}

/// Undoes one line of the log; what the undo's own line says, None — this one cannot be undone.
fn undo_change(c: &Connection, change: &Change) -> rusqlite::Result<Option<String>> {
    let json = |text: &Option<String>| text.as_deref().and_then(|text| serde_json::from_str::<Value>(text).ok());
    let before = json(&change.before);
    match change.target.as_str() {
        "fact" => {
            let Some(fact) = store::fact(c, &change.target_id)? else { return Ok(None) };
            match change.action.as_str() {
                "add" => {
                    store::erase_fact(c, &fact.id)?;
                    Ok(Some(String::new()))
                }
                "close" | "merge" | "past" => {
                    store::reopen_fact(c, &fact.id)?;
                    Ok(Some(fact.text))
                }
                "edit" | "pin" | "unpin" | "date" => {
                    let Some(before) = before else { return Ok(None) };
                    if let Some(said) = before.get("text").and_then(Value::as_str) {
                        store::edit_fact(c, &fact.id, said)?;
                    }
                    if let Some(pinned) = before.get("pinned").and_then(Value::as_bool) {
                        store::pin_fact(c, &fact.id, pinned)?;
                    }
                    Ok(Some(fact.text))
                }
                "importance" => {
                    let Some(importance) = before.as_ref().and_then(|b| b.get("importance")).and_then(Value::as_i64)
                    else {
                        return Ok(None);
                    };
                    store::set_importance(c, &fact.id, importance)?;
                    Ok(Some(fact.text))
                }
                _ => Ok(None),
            }
        }
        "entity" if change.action == "alias" => {
            let alias = json(&change.after)
                .and_then(|after| after.get("alias").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_else(|| change.summary.clone());
            store::remove_alias(c, &change.target_id, &alias)?;
            Ok(Some(alias))
        }
        "note" => match change.action.as_str() {
            "add" => {
                store::delete_note(c, &change.target_id)?;
                Ok(Some(String::new()))
            }
            "edit" => {
                let Some(before) = before else { return Ok(None) };
                let Some(said) = before.get("text").and_then(Value::as_str) else { return Ok(None) };
                let run = before.get("runId").and_then(Value::as_str);
                store::update_note(c, &change.target_id, said, run)?;
                Ok(Some(said.to_string()))
            }
            _ => Ok(None),
        },
        _ => Ok(None),
    }
}

/// A fact said by voice as people write it: a capital first letter, and the names the memory knows with theirs
/// («у маши аллергия» → «У Маши аллергия»).
fn polish(said: &str, entities: &[Entity]) -> String {
    let names: Vec<String> = entities
        .iter()
        .filter(|entity| entity.kind != EntityKind::Household)
        .flat_map(|entity| std::iter::once(entity.name.clone()).chain(entity.aliases.iter().cloned()))
        .map(|name| text::normalize(&name))
        .filter(|name| !name.is_empty() && !name.contains(' '))
        .map(|name| text::stem(&name))
        .collect();
    let words: Vec<String> = said
        .split(' ')
        .map(|word| {
            let core = text::normalize(word);
            if !core.is_empty() && !core.contains(' ') && names.contains(&text::stem(&core)) {
                capital(word)
            } else {
                word.to_string()
            }
        })
        .collect();
    capital(&words.join(" "))
}

/// The first letter in capitals, past any punctuation before it.
fn capital(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut done = false;
    for c in word.chars() {
        if !done && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            done = true;
        } else {
            out.push(c);
        }
    }
    out
}

/// «Дом», «дома», «home»: the household itself.
fn is_home(about: &str) -> bool {
    matches!(text::stems(about).as_slice(), [one] if one == "дом" || one == "home" || one == "household")
}

/// «маша» from a voice becomes «Маша».
fn title_case(name: &str) -> String {
    name.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) if first.is_lowercase() => first.to_uppercase().chain(chars).collect(),
                Some(first) => std::iter::once(first).chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn layer_of(fact: &Fact) -> PromptLayer {
    if fact.visibility == Visibility::Character {
        PromptLayer::Personal
    } else {
        PromptLayer::Home
    }
}

fn names_of(entities: &[Entity], id: &str) -> String {
    entities
        .iter()
        .find(|e| e.id == id)
        .map(|e| std::iter::once(e.name.clone()).chain(e.aliases.iter().cloned()).collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

fn view_of(entities: &[Entity], fact: Fact) -> FactView {
    let name = |id: &str| entities.iter().find(|e| e.id == id).map(|e| e.name.clone());
    FactView {
        subject_name: name(&fact.subject_id).unwrap_or_default(),
        subject_kind: entities.iter().find(|e| e.id == fact.subject_id).map(|e| e.kind),
        object_name: fact.object_id.as_deref().and_then(name),
        layer: layer_of(&fact),
        fact,
    }
}

/// The log's actor for a write: the principal when known, otherwise who said it.
fn actor_columns(actor: Option<&Principal>, origin: &Origin) -> (String, Option<String>) {
    match actor {
        Some(principal) => (principal.kind.as_str().to_string(), Some(principal.id.clone())),
        None => (
            match origin.said_by_kind {
                SpeakerKind::Contact => "household".to_string(),
                kind => kind.as_str().to_string(),
            },
            origin.said_by_id.clone(),
        ),
    }
}

/// The next 7:00 on the station's clock (its time zone, else this machine's): as a moment, and as the clock shows it.
fn morning_after(now: DateTime<Utc>, timezone: Option<&str>) -> (DateTime<Utc>, NaiveTime) {
    let seven = NaiveTime::from_hms_opt(7, 0, 0).expect("7:00 is a time");
    fn next<Z: TimeZone>(now: DateTime<Utc>, zone: &Z, seven: NaiveTime) -> DateTime<Utc> {
        let local = now.with_timezone(zone);
        let mut day = local.date_naive();
        if local.time() >= seven {
            day = day.succ_opt().unwrap_or(day);
        }
        zone.from_local_datetime(&day.and_time(seven))
            .earliest()
            .map(|at| at.with_timezone(&Utc))
            .unwrap_or(now + Duration::hours(12))
    }
    let at = match timezone.and_then(|name| name.parse::<chrono_tz::Tz>().ok()) {
        Some(zone) => next(now, &zone, seven),
        None => next(now, &chrono::Local, seven),
    };
    (at, seven)
}

/// Where the memory tools run: who asked through which door.
pub fn origin_of(principal: &Principal, station: Option<&str>, character: &str) -> Origin {
    match (principal.kind, station) {
        (PrincipalKind::User, None) => Origin::panel_user(&principal.id),
        (PrincipalKind::User, Some(station)) => Origin {
            said_by_kind: SpeakerKind::User,
            said_by_id: Some(principal.id.clone()),
            channel: Channel::Station,
            station: Some(station.to_string()),
            audience: Audience::Room,
            heard_by: Some(character.to_string()),
        },
        (PrincipalKind::ApiKey, _) => Origin {
            said_by_kind: SpeakerKind::ApiKey,
            said_by_id: Some(principal.id.clone()),
            channel: Channel::Api,
            station: station.map(str::to_string),
            audience: Audience::Speaker,
            heard_by: Some(character.to_string()),
        },
        (PrincipalKind::System, _) => Origin::system(),
        (PrincipalKind::Household, _) => Origin::station_voice(station, Some(character)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::core::identity::principal::{household_principal, user_principal};
    use crate::core::identity::roles::Role;

    fn service() -> MemoryService {
        let config = Config::for_tests(&Env::default()).dialog;
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let personas = Arc::new(PersonaService::new(&config, db.clone(), settings.clone(), events.clone()).unwrap());
        MemoryService::new(db, settings, events, personas)
    }

    fn voice() -> Origin {
        Origin::station_voice(Some("kitchen"), Some("alice"))
    }

    fn said(fact: &str) -> Remember {
        Remember { fact: fact.into(), ..Remember::default() }
    }

    #[tokio::test]
    async fn remembers_a_voice_fact_about_the_named_person_or_the_home() {
        let memory = service();
        let admin = user_principal("u1", "Админ", Role::Admin, None);
        let masha = memory.add_entity(EntityKind::Person, "Маша", &admin).unwrap();
        let added = memory.remember(said("У Маши аллергия на орехи"), voice(), Source::Voice, None).unwrap();
        assert_eq!(added.fact().subject_id, masha.id, "«у Маши» is about Маша");
        assert_eq!(added.fact().origin.channel, Channel::Station);
        let home = memory.remember(said("По пятницам заказываем пиццу"), voice(), Source::Voice, None).unwrap();
        let household = memory.db.with(|c| store::household(c, "Дом")).unwrap();
        assert_eq!(home.fact().subject_id, household);
        // Said twice: known, not written again.
        let again = memory.remember(said("у маши аллергия на орехи."), voice(), Source::Voice, None).unwrap();
        assert!(matches!(again, Remembered::Known(_)));
        assert_eq!(memory.overview(&Viewer::default()).unwrap().counts.home, 2);
    }

    #[tokio::test]
    async fn the_model_names_people_and_replaces_old_facts() {
        let memory = service();
        let household = household_principal(Role::Member, None);
        let first = Remember {
            fact: "Петя ходит на плавание по средам".into(),
            about: Some("Петя".into()),
            relation: Some("кружок".into()),
            ..Remember::default()
        };
        memory.remember(first, voice(), Source::Model, Some(&household)).unwrap();
        let second = Remember {
            fact: "Петя ходит на шахматы по средам".into(),
            about: Some("петя".into()),
            relation: Some("кружок".into()),
            replaces: Some("плавание по средам".into()),
            ..Remember::default()
        };
        let Remembered::Added { closed, fact } =
            memory.remember(second, voice(), Source::Model, Some(&household)).unwrap()
        else {
            panic!("a new fact")
        };
        assert_eq!(closed.map(|c| c.text), Some("Петя ходит на плавание по средам".to_string()));
        let card = memory.entity_card(&fact.subject_id, &Viewer::default()).unwrap();
        assert_eq!(card.entity.name, "Петя", "named once, in title case");
        assert_eq!(card.current.len(), 1);
        assert_eq!(card.closed.len(), 1);
        assert_eq!(card.closed[0].fact.replaced_by.as_deref(), Some(fact.id.as_str()));
    }

    #[tokio::test]
    async fn forgets_recalls_and_undoes() {
        let memory = service();
        let admin = user_principal("u1", "Админ", Role::Admin, None);
        memory.add_entity(EntityKind::Person, "Маша", &admin).unwrap();
        memory.remember(said("У Маши аллергия на орехи"), voice(), Source::Voice, None).unwrap();
        memory.remember(said("Маша работает в библиотеке"), voice(), Source::Voice, None).unwrap();
        let found = memory.about("Машу", &Viewer::default(), 5).unwrap();
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|f| f.subject_name == "Маша"));
        let forgotten = memory.forget("про орехи", &Viewer::default(), None).unwrap().unwrap();
        assert_eq!(forgotten.text, "У Маши аллергия на орехи");
        assert_eq!(memory.recall("орехи", &Viewer::default(), 5).unwrap().len(), 0);
        assert!(memory.forget("про погоду на Марсе", &Viewer::default(), None).unwrap().is_none());

        // «Отменить» the closing: the fact is back.
        let close = memory.changes(10).unwrap().into_iter().find(|c| c.action == "close").unwrap();
        memory.undo(&close.id, &admin).unwrap();
        assert_eq!(memory.recall("орехи", &Viewer::default(), 5).unwrap().len(), 1);
        assert_eq!(memory.undo(&close.id, &admin), Err(MemoryError::NotUndoable), "undone once");

        // «Отменить» an addition: it is gone, its words too.
        let add = memory
            .changes(20)
            .unwrap()
            .into_iter()
            .find(|c| c.action == "add" && c.summary.contains("библиотеке"))
            .unwrap();
        memory.undo(&add.id, &admin).unwrap();
        assert!(memory.recall("библиотеке", &Viewer::default(), 5).unwrap().is_empty());
        assert!(memory.changes(50).unwrap().iter().all(|c| !c.summary.contains("библиотеке")));
    }

    #[tokio::test]
    async fn off_and_paused_write_nothing_but_known_facts_stay_in_use() {
        let memory = service();
        memory.remember(said("По будням будильник в 7"), voice(), Source::Voice, None).unwrap();
        memory.pause(PauseSpan::Hour, None, None).unwrap();
        assert!(memory.state().paused);
        assert_eq!(memory.remember(said("Новый факт"), voice(), Source::Voice, None), Err(MemoryError::Paused));
        memory.episode("kitchen", "Как дела?", "Хорошо.");
        assert_eq!(memory.overview(&Viewer::default()).unwrap().counts.episodes, 0, "a pause keeps no episodes");
        assert_eq!(memory.wake_up(&memory.viewer()).len(), 1, "what is known is still used");
        memory.resume(None).unwrap();
        memory.episode("kitchen", "Как дела?", "Хорошо.");
        memory.episode("workshop:voice:kitchen", "Сделай навык", "Делаю.");
        assert_eq!(memory.overview(&Viewer::default()).unwrap().counts.episodes, 1);

        memory.set_enabled(false, None).unwrap();
        assert_eq!(memory.remember(said("Ещё факт"), voice(), Source::Voice, None), Err(MemoryError::Off));
        assert!(memory.wake_up(&memory.viewer()).is_empty());
        assert_eq!(memory.prompt(Lang::Ru, &["remember", "recall"]), "");
    }

    #[tokio::test]
    async fn prompts_the_wake_up_with_the_offered_tools() {
        let memory = service();
        let admin = user_principal("u1", "Админ", Role::Admin, None);
        let fact = memory.remember(said("У Маши аллергия на орехи"), voice(), Source::Voice, None).unwrap();
        memory.update_fact(&fact.fact().id, None, Some(true), &admin).unwrap();
        let block = memory.prompt(Lang::Ru, &["remember", "forget", "recall"]);
        assert!(block.contains("- У Маши аллергия на орехи.\n"));
        assert!(block.contains("даже без просьбы"));
        memory.set_in_passing(false, None).unwrap();
        assert!(memory.prompt(Lang::Ru, &["remember", "recall"]).contains("только когда человек просит"));
        let eyes = memory.overview(&Viewer::default()).unwrap().eyes;
        assert_eq!(eyes.items.len(), 1);
        assert_eq!(eyes.character.id, "alice");
        assert!(eyes.character.feminine);
    }

    #[test]
    fn finds_the_next_morning_on_the_stations_clock() {
        let now = Utc.with_ymd_and_hms(2026, 11, 12, 20, 30, 0).unwrap();
        let (at, local) = morning_after(now, Some("Europe/Moscow"));
        assert_eq!(local, NaiveTime::from_hms_opt(7, 0, 0).unwrap());
        // 23:30 in Moscow → 7:00 the next day there, 04:00 UTC.
        assert_eq!(at, Utc.with_ymd_and_hms(2026, 11, 13, 4, 0, 0).unwrap());
        let early = Utc.with_ymd_and_hms(2026, 11, 12, 2, 0, 0).unwrap();
        assert_eq!(morning_after(early, Some("Europe/Moscow")).0, Utc.with_ymd_and_hms(2026, 11, 12, 4, 0, 0).unwrap());
    }

    #[test]
    fn writes_a_voice_fact_as_people_write_it() {
        let masha = Entity {
            id: "m".into(),
            kind: EntityKind::Person,
            name: "Маша".into(),
            aliases: vec!["Мария".into()],
            account_id: None,
            contact: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert_eq!(polish("у маши аллергия на орехи", std::slice::from_ref(&masha)), "У Маши аллергия на орехи");
        assert_eq!(polish("марии подарить книгу", &[masha]), "Марии подарить книгу");
        assert_eq!(polish("купить молоко", &[]), "Купить молоко");
    }

    #[test]
    fn knows_the_home_and_title_cases_names() {
        assert!(is_home("дом"));
        assert!(is_home("Дома"));
        assert!(!is_home("домработница"));
        assert_eq!(title_case("маша"), "Маша");
        assert_eq!(title_case("анна петровна"), "Анна Петровна");
    }
}
