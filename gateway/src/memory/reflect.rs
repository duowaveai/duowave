//! The reflection (docs/plans/memory.md, «Рефлексия»): the sleep's first part, off by default. Once a night the
//! memory thinks over the talk no night has thought over yet: the model gets the conversations, the household's
//! records and names, and answers with a list of actions, not prose. Every action is checked — its records exist,
//! nothing is deleted, the part it belongs to is on, no more than 50 a night — then they all go in one transaction,
//! each into the memory's log of changes with the night's id: the report «За ночь», «Отменить» and «Откатить ночь»
//! come from there.
//!
//! Step 1 of the plan: «Порядок» — repeats glued, what a newer record contradicts closed, other names linked,
//! «завтра» turned into a date, the past put into history, importance recounted — and «Итог дня»: a short summary for
//! the house and a diary entry, in their own voice, for each character who talked that day. Insights, lessons and «на
//! завтра» come with step 2, the days folding into weeks and months later too.
//!
//! The model is the memory's own, apart from «Мозг» (setting «Модель памяти»): a model at home by default (the night
//! is in no hurry, and the talk stays in the house), the «Мозг»'s smart one, or any model of its providers, at home or
//! in the cloud. Without one the reflection waits.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use futures::future::BoxFuture;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::service::{Answering, MemoryService};
use super::store::{self, Entity, Episode, Fact, NewChange, Note};
use super::text::{self, Lang};
use crate::config::ModelProfile;
use crate::core::events::Actor;
use crate::core::identity::principal::Principal;
use crate::dialog::brain::BrainService;
use crate::dialog::llm::{LlmEvent, LlmMessage, LlmRequest};
use crate::dialog::{BrainSource, ModelSlot};
use crate::platform::http::ApiError;
use crate::settings::definitions::{
    MemoryModel, MEMORY_MODEL, MEMORY_REFLECTION, MEMORY_REFLECTION_DAY, MEMORY_REFLECTION_ORDER,
};
use crate::settings::SettingsService;
use crate::sleep::{Night, PartReport, PartState, SleepPart};

/// No more than this many «Порядок» actions a night.
pub const MAX_ACTIONS: usize = 50;
/// Conversations in one question to the model, and their words; the rest go in the next pass.
const EPISODES_PER_PASS: i64 = 40;
const EPISODE_CHARS: usize = 16_000;
/// Passes in one go; what is left waits for the next go or night.
const MAX_PASSES: usize = 5;
/// Records the model sees: the pinned, then the most important and newest.
const FACTS_SHOWN: usize = 120;
/// One side of an exchange as the model sees it.
const SAID_CHARS: usize = 400;
const SUMMARY_CHARS: usize = 1200;
const DIARY_CHARS: usize = 1000;
const FACT_CHARS: usize = 600;
const WHY_CHARS: usize = 200;
const ALIAS_CHARS: usize = 60;
/// Who writes the night's changes in the log.
const ACTOR_KIND: &str = "system";
const ACTOR_ID: &str = "reflection";

// ───────── The model ─────────

/// Where the memory's model comes from: «Мозг»'s providers.
pub trait NightModel: Send + Sync {
    /// A model at home: its questions do not leave the house. None — none is set up.
    fn home(&self) -> Option<ModelSlot>;
    /// The «Мозг»'s smart model, wherever it is.
    fn brain(&self) -> Option<ModelSlot>;
    /// One model of a provider; None — no such provider.
    fn pick(&self, provider: &str, model: &str) -> Option<ModelSlot>;
    /// Every model «Мозг»'s providers have, those at home first.
    fn options(&self) -> Vec<ModelOption>;
}

impl NightModel for BrainService {
    /// The «Мозг»'s own model when it is at home, else the first model of a provider at home (the lists hold only
    /// models that talk: «Мозг» leaves out embeddings, voices and the like).
    fn home(&self) -> Option<ModelSlot> {
        [ModelProfile::Smart, ModelProfile::Fast]
            .into_iter()
            .filter_map(|profile| BrainSource::slot(self, profile))
            .find(|slot| !slot.cloud)
            .or_else(|| {
                NightModel::options(self)
                    .into_iter()
                    .filter(|option| !option.cloud)
                    .find_map(|option| BrainSource::slot_of(self, &option.provider, &option.model))
            })
            .filter(|slot| !slot.cloud)
    }

    fn brain(&self) -> Option<ModelSlot> {
        BrainSource::slot(self, ModelProfile::Smart)
    }

    fn pick(&self, provider: &str, model: &str) -> Option<ModelSlot> {
        BrainSource::slot_of(self, provider, model)
    }

    fn options(&self) -> Vec<ModelOption> {
        let view = self.view();
        let chosen: Vec<_> = [&view.choice.fast, &view.choice.smart].into_iter().flatten().collect();
        let mut options: Vec<ModelOption> = Vec::new();
        for provider in &view.providers {
            // The models «Мозг» thinks with on it come first, even when its list was never asked for.
            let mut models: Vec<String> =
                chosen.iter().filter(|pick| pick.provider == provider.id).map(|pick| pick.model.clone()).collect();
            models.extend(provider.models.iter().cloned());
            let mut seen = HashSet::new();
            for model in models.into_iter().filter(|model| !model.is_empty() && seen.insert(model.clone())) {
                options.push(ModelOption {
                    provider: provider.id.clone(),
                    title: provider.title.clone(),
                    model,
                    cloud: provider.cloud,
                });
            }
        }
        options.sort_by_key(|option| option.cloud);
        options
    }
}

/// A model of «Мозг»'s providers, for the choice of the memory's model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOption {
    pub provider: String,
    /// The provider for people: «OpenAI», «Ollama».
    pub title: String,
    pub model: String,
    pub cloud: bool,
}

/// A model as the panel names it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub title: String,
    pub model: String,
    pub cloud: bool,
}

impl ModelInfo {
    fn of(slot: &ModelSlot) -> ModelInfo {
        ModelInfo { title: slot.title.clone(), model: slot.model.clone(), cloud: slot.cloud }
    }
}

/// The reflection's switches and the memory's model as the panel shows them.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReflectionView {
    pub enabled: bool,
    pub order: bool,
    pub day: bool,
    /// The memory's model as chosen.
    pub model: MemoryModel,
    /// The model that thinks now; None — the memory waits, and `waiting` says why.
    pub using: Option<ModelInfo>,
    pub waiting: Option<String>,
    /// What «Своя, дома» and «Как у «Мозга»» are now.
    pub home: Option<ModelInfo>,
    pub brain: Option<ModelInfo>,
    /// Every model of «Мозг»'s providers, those at home first.
    pub options: Vec<ModelOption>,
    /// Conversations no night has thought over yet.
    pub pending: i64,
}

/// The reflection's switches and the memory's model from the panel; absent is unchanged.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReflectionChange {
    pub enabled: Option<bool>,
    pub order: Option<bool>,
    pub day: Option<bool>,
    pub model: Option<MemoryModel>,
}

pub struct Reflection {
    memory: Arc<MemoryService>,
    settings: Arc<SettingsService>,
    model: Arc<dyn NightModel>,
}

impl Reflection {
    pub fn new(memory: Arc<MemoryService>, settings: Arc<SettingsService>, model: Arc<dyn NightModel>) -> Reflection {
        Reflection { memory, settings, model }
    }

    pub fn enabled(&self) -> bool {
        self.settings.get(&MEMORY_REFLECTION, None).unwrap_or(false)
    }

    fn order(&self) -> bool {
        self.settings.get(&MEMORY_REFLECTION_ORDER, None).unwrap_or(true)
    }

    fn day(&self) -> bool {
        self.settings.get(&MEMORY_REFLECTION_DAY, None).unwrap_or(true)
    }

    fn model_choice(&self) -> MemoryModel {
        self.settings.get(&MEMORY_MODEL, None).unwrap_or_default()
    }

    fn pending(&self) -> i64 {
        self.memory.db().with(store::unreflected_count).unwrap_or(0)
    }

    pub fn view(&self) -> ReflectionView {
        let picked = self.pick();
        ReflectionView {
            enabled: self.enabled(),
            order: self.order(),
            day: self.day(),
            model: self.model_choice(),
            using: picked.as_ref().ok().map(ModelInfo::of),
            waiting: picked.err(),
            home: self.model.home().as_ref().map(ModelInfo::of),
            brain: self.model.brain().as_ref().map(ModelInfo::of),
            options: self.model.options(),
            pending: self.pending(),
        }
    }

    pub fn change(&self, change: ReflectionChange, actor: Option<Actor>) -> Result<ReflectionView, ApiError> {
        if let Some(model) = change.model {
            if let MemoryModel::Pick { provider, model } = &model {
                if provider.trim().is_empty() || model.trim().is_empty() || self.model.pick(provider, model).is_none() {
                    return Err(ApiError::bad_request("Такой модели нет в «Мозге»."));
                }
            }
            self.settings.set(&MEMORY_MODEL, &model, actor.clone())?;
        }
        if let Some(on) = change.order {
            self.settings.set(&MEMORY_REFLECTION_ORDER, &on, actor.clone())?;
        }
        if let Some(on) = change.day {
            self.settings.set(&MEMORY_REFLECTION_DAY, &on, actor.clone())?;
        }
        if let Some(on) = change.enabled {
            self.settings.set(&MEMORY_REFLECTION, &on, actor)?;
        }
        Ok(self.view())
    }

    /// The memory's model now, or why there is none.
    fn pick(&self) -> Result<ModelSlot, String> {
        match self.model_choice() {
            MemoryModel::Brain => {
                self.model.brain().ok_or_else(|| "Ждёт модель: в «Мозге» ничего не выбрано.".to_string())
            }
            MemoryModel::Pick { provider, model } => self.model.pick(&provider, &model).ok_or_else(|| {
                format!("Ждёт модель: «{model}» больше нет в «Мозге». Выберите модель памяти в её настройках.")
            }),
            MemoryModel::Home => self.model.home().ok_or_else(|| {
                "Ждёт свою модель: дома её нет. Выберите модель памяти, домашнюю или облачную, в настройках памяти."
                    .to_string()
            }),
        }
    }

    // ───────── A night ─────────

    async fn reflect(&self, night: &Night) -> PartReport {
        let slot = match self.pick() {
            Ok(slot) => slot,
            Err(why) => return PartReport::new(self, PartState::Waiting, why),
        };
        let (order, day) = (self.order(), self.day());
        let mut tally = Tally::default();
        let mut budget = MAX_ACTIONS;
        let mut broke = None;
        for pass in 0..MAX_PASSES {
            if night.cancel.is_cancelled() {
                break;
            }
            let input = match self.gather(night.zone, order && pass == 0, day) {
                Ok(Some(input)) => input,
                Ok(None) => break,
                Err(error) => {
                    broke = Some(format!("Не прочитала память: {error}"));
                    break;
                }
            };
            let (system, user) = prompt(&input);
            let answer = match ask(&slot, system, user, night.cancel.clone()).await {
                Ok(answer) => answer,
                Err(error) => {
                    broke = Some(format!("{} не ответила: {}", slot.title, short(&error.to_string(), 160)));
                    break;
                }
            };
            let Some(plan) = parse_plan(&answer) else {
                broke = Some(format!("{} ответила не списком действий; попробую ещё раз.", slot.title));
                break;
            };
            let checked = check(&plan, &input, &mut budget);
            match self.apply(&night.run_id, &input, &checked) {
                Ok(applied) => tally.add(&input, applied, checked.skipped),
                Err(error) => {
                    broke = Some(format!("Не записала: {error}"));
                    break;
                }
            }
        }
        if tally.passes > 0 {
            self.memory.changed("reflect", "run", &night.run_id, None);
        }
        let mut report = PartReport::new(self, PartState::Done, tally.summary(&slot));
        report.lines = tally.lines(|id| self.memory.character(id).title);
        if let Some(why) = broke {
            report.state = PartState::Failed;
            report.summary = if tally.passes > 0 { format!("{} {why}", report.summary) } else { why };
        }
        report
    }

    /// What one pass thinks over: the conversations no night has thought over (within the pass's words), the
    /// household's names and records, the notes the days already have. None — nothing left.
    fn gather(&self, zone: Tz, order: bool, day: bool) -> rusqlite::Result<Option<Input>> {
        let gathered = self.memory.db().with(|c| {
            let mut episodes = store::unreflected_episodes(c, EPISODES_PER_PASS)?;
            let mut spent = 0;
            let mut keep = 0;
            for episode in &episodes {
                let cost = clip(&episode.question, SAID_CHARS).len() + clip(&episode.answer, SAID_CHARS).len();
                if keep > 0 && spent + cost > EPISODE_CHARS {
                    break;
                }
                spent += cost;
                keep += 1;
            }
            episodes.truncate(keep);
            if episodes.is_empty() {
                return Ok(None);
            }
            let mut facts = store::visible_facts(c, &super::model::Viewer::default(), true)?;
            facts.sort_by(|a, b| {
                b.pinned.cmp(&a.pinned).then(b.importance.cmp(&a.importance)).then(b.updated_at.cmp(&a.updated_at))
            });
            facts.truncate(FACTS_SHOWN);
            let entities = store::entities(c)?;
            Ok(Some((episodes, facts, entities)))
        })?;
        let Some((episodes, facts, entities)) = gathered else { return Ok(None) };
        let mut days: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut speakers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (index, episode) in episodes.iter().enumerate() {
            let day = local(&episode.at, zone).0;
            days.entry(day.clone()).or_default().push(index);
            if let Some(character) = &episode.character_id {
                speakers.entry(day).or_default().insert(character.clone());
            }
        }
        let characters: BTreeMap<String, Answering> = speakers
            .values()
            .flatten()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|id| (id.clone(), self.memory.character(id)))
            .collect();
        let notes = self.memory.db().with(|c| {
            let mut notes = Vec::new();
            for day in days.keys() {
                notes.extend(store::note_of(c, "day", day, None)?);
                for character in speakers.get(day).into_iter().flatten() {
                    notes.extend(store::note_of(c, "diary", day, Some(character))?);
                }
            }
            Ok(notes)
        })?;
        let said: String = episodes.iter().map(|e| format!("{} {} ", e.question, e.answer)).collect();
        Ok(Some(Input {
            lang: match text::lang_of(&said) {
                Lang::En => Lang::En,
                _ => Lang::Ru,
            },
            today: Utc::now().with_timezone(&zone).format("%Y-%m-%d").to_string(),
            zone,
            episodes,
            days,
            speakers,
            characters,
            facts,
            entities,
            notes,
            order,
            day,
        }))
    }

    /// The checked actions in one transaction, each into the log with the night's id; the pass's conversations are
    /// thought over.
    fn apply(&self, run_id: &str, input: &Input, checked: &Checked) -> rusqlite::Result<Applied> {
        self.memory.db().transaction(|c| {
            let mut applied = Applied::default();
            let log = |c: &rusqlite::Connection,
                       action: &str,
                       target: &str,
                       target_id: &str,
                       summary: &str,
                       before: Option<String>,
                       after: Option<String>,
                       why: &str|
             -> rusqlite::Result<String> {
                store::log_change(
                    c,
                    NewChange {
                        actor_kind: ACTOR_KIND,
                        actor_id: Some(ACTOR_ID),
                        action,
                        target,
                        target_id,
                        summary,
                        before,
                        after,
                        reason: Some(why).filter(|why| !why.is_empty()),
                        run_id: Some(run_id),
                    },
                )
            };
            for act in &checked.acts {
                match act {
                    Act::Merge { keep, drop, text, why } => {
                        let kept = &input.facts[*keep];
                        for gone in drop {
                            let fact = &input.facts[*gone];
                            if store::close_fact(c, &fact.id, "повтор", Some(&kept.id))? == 0 {
                                continue;
                            }
                            let after = json!({ "keep": kept.id, "keepText": text.as_deref().unwrap_or(&kept.text) });
                            log(c, "merge", "fact", &fact.id, &fact.text, to_json(fact), Some(after.to_string()), why)?;
                            applied.merged += 1;
                        }
                        if let Some(text) = text {
                            store::edit_fact(c, &kept.id, text)?;
                            let after = store::fact(c, &kept.id)?;
                            log(
                                c,
                                "edit",
                                "fact",
                                &kept.id,
                                text,
                                to_json(kept),
                                after.as_ref().and_then(to_json),
                                why,
                            )?;
                        }
                    }
                    Act::Close { fact, by, why } => {
                        let fact = &input.facts[*fact];
                        let by = by.map(|by| input.facts[by].id.as_str());
                        if store::close_fact(c, &fact.id, "больше не так", by)? > 0 {
                            let after = json!({ "by": by });
                            log(c, "close", "fact", &fact.id, &fact.text, to_json(fact), Some(after.to_string()), why)?;
                            applied.closed += 1;
                        }
                    }
                    Act::Past { fact, why } => {
                        let fact = &input.facts[*fact];
                        if store::close_fact(c, &fact.id, "прошло", None)? > 0 {
                            log(c, "past", "fact", &fact.id, &fact.text, to_json(fact), None, why)?;
                            applied.past += 1;
                        }
                    }
                    Act::Date { fact, text, why } => {
                        let fact = &input.facts[*fact];
                        store::edit_fact(c, &fact.id, text)?;
                        let after = store::fact(c, &fact.id)?;
                        log(c, "date", "fact", &fact.id, text, to_json(fact), after.as_ref().and_then(to_json), why)?;
                        applied.dated += 1;
                    }
                    Act::Alias { entity, alias, why } => {
                        let entity = &input.entities[*entity];
                        if store::add_alias(c, &entity.id, alias)? > 0 {
                            let after = json!({ "alias": alias, "name": entity.name });
                            log(c, "alias", "entity", &entity.id, alias, None, Some(after.to_string()), why)?;
                            applied.aliases += 1;
                        }
                    }
                    Act::Importance { fact, importance, why } => {
                        let fact = &input.facts[*fact];
                        store::set_importance(c, &fact.id, *importance)?;
                        let after = json!({ "importance": importance });
                        log(
                            c,
                            "importance",
                            "fact",
                            &fact.id,
                            &fact.text,
                            to_json(fact),
                            Some(after.to_string()),
                            why,
                        )?;
                        applied.importance += 1;
                    }
                }
            }
            for note in &checked.notes {
                let character = note.character.as_deref();
                let kind = if character.is_some() { "diary" } else { "day" };
                match store::note_of(c, kind, &note.day, character)? {
                    Some(old) => {
                        store::update_note(c, &old.id, &note.text, Some(run_id))?;
                        let after = store::note(c, &old.id)?;
                        let before = serde_json::to_string(&old).ok();
                        log(c, "edit", "note", &old.id, &note.text, before, after.as_ref().and_then(to_json), "")?;
                    }
                    None => {
                        let new =
                            store::insert_note(c, kind, &note.day, character, &note.text, "reflection", Some(run_id))?;
                        log(c, "add", "note", &new.id, &note.text, None, to_json(&new), "")?;
                    }
                }
                match character {
                    Some(character) => {
                        applied.diaries.insert(character.to_string());
                    }
                    None => {
                        applied.days.insert(note.day.clone());
                    }
                }
            }
            let ids: Vec<String> = input.episodes.iter().map(|episode| episode.id.clone()).collect();
            store::mark_reflected(c, &ids, run_id)?;
            applied.thought = ids.len();
            Ok(applied)
        })
    }

    /// «За ночь»: the night's changes and notes, the conversations it thought over.
    fn night(&self, run_id: &str) -> Value {
        let read = self.memory.db().with(|c| {
            let changes = store::changes_of_run(c, run_id)?;
            let notes = store::notes_of_run(c, run_id)?;
            let thought = store::reflected_count(c, run_id)?;
            Ok((changes, notes, thought))
        });
        let Ok((changes, notes, thought)) = read else { return json!({}) };
        let notes: Vec<Value> = notes
            .into_iter()
            .map(|note| {
                let title = note.character_id.as_deref().map(|id| self.memory.character(id).title);
                json!({
                    "id": note.id,
                    "kind": note.kind,
                    "day": note.day,
                    "characterId": note.character_id,
                    "characterTitle": title,
                    "text": note.text,
                })
            })
            .collect();
        json!({ "changes": changes, "notes": notes, "thought": thought })
    }
}

impl SleepPart for Reflection {
    fn id(&self) -> &'static str {
        "reflection"
    }

    fn title(&self) -> &'static str {
        "Рефлексия"
    }

    fn enabled(&self) -> bool {
        Reflection::enabled(self)
    }

    fn work(&self) -> Result<(), String> {
        if !self.memory.enabled() {
            return Err("Память выключена.".into());
        }
        if !self.order() && !self.day() {
            return Err("Обе её части выключены: «Порядок» и «Итог дня».".into());
        }
        if self.pending() == 0 {
            return Err("Новых разговоров нет: обдумывать нечего.".into());
        }
        Ok(())
    }

    fn ready(&self) -> Result<(), String> {
        self.pick().map(|_| ())
    }

    fn run<'a>(&'a self, night: &'a Night) -> BoxFuture<'a, PartReport> {
        Box::pin(self.reflect(night))
    }

    fn traces(&self, run_id: &str) -> usize {
        // In doubt it left something: a night is never tidied away on a failed read.
        let count =
            self.memory.db().with(|c| Ok(store::count_changes_of_run(c, run_id)? + store::reflected_count(c, run_id)?));
        count.map(|count| count.max(0) as usize).unwrap_or(1)
    }

    fn details(&self, run_id: &str) -> Value {
        self.night(run_id)
    }

    fn roll_back(&self, run_id: &str, actor: &Principal) -> Result<usize, String> {
        self.memory.roll_back_run(run_id, actor).map_err(|error| error.to_string())
    }
}

// ───────── One pass ─────────

/// What one pass gives the model.
struct Input {
    lang: Lang,
    today: String,
    zone: Tz,
    episodes: Vec<Episode>,
    /// Day → its conversations (indices into `episodes`).
    days: BTreeMap<String, Vec<usize>>,
    /// Day → the characters who talked.
    speakers: BTreeMap<String, BTreeSet<String>>,
    characters: BTreeMap<String, Answering>,
    /// F1… in this order.
    facts: Vec<Fact>,
    /// E1… in this order.
    entities: Vec<Entity>,
    /// The summaries and diaries these days already have.
    notes: Vec<Note>,
    /// Whether this pass orders the records («Порядок» is on, and it is the night's first pass).
    order: bool,
    day: bool,
}

/// A checked action, by the records' places in the input.
#[derive(Debug, Clone, PartialEq)]
enum Act {
    Merge { keep: usize, drop: Vec<usize>, text: Option<String>, why: String },
    Close { fact: usize, by: Option<usize>, why: String },
    Past { fact: usize, why: String },
    Date { fact: usize, text: String, why: String },
    Alias { entity: usize, alias: String, why: String },
    Importance { fact: usize, importance: i64, why: String },
}

/// A day's summary (no character) or a character's diary entry.
#[derive(Debug, Clone, PartialEq)]
struct NoteText {
    day: String,
    character: Option<String>,
    text: String,
}

#[derive(Debug, Default)]
struct Checked {
    acts: Vec<Act>,
    notes: Vec<NoteText>,
    /// The model's actions that did not pass.
    skipped: usize,
}

#[derive(Debug, Default)]
struct Applied {
    thought: usize,
    merged: usize,
    closed: usize,
    past: usize,
    dated: usize,
    aliases: usize,
    importance: usize,
    days: BTreeSet<String>,
    diaries: BTreeSet<String>,
}

/// The night's sum over its passes.
#[derive(Debug, Default)]
struct Tally {
    passes: usize,
    first: Option<String>,
    last: Option<String>,
    applied: Applied,
    skipped: usize,
}

impl Tally {
    fn add(&mut self, input: &Input, applied: Applied, skipped: usize) {
        self.passes += 1;
        if self.first.is_none() {
            self.first = input.days.keys().next().cloned();
        }
        self.last = input.days.keys().next_back().cloned().or(self.last.take());
        let sum = &mut self.applied;
        sum.thought += applied.thought;
        sum.merged += applied.merged;
        sum.closed += applied.closed;
        sum.past += applied.past;
        sum.dated += applied.dated;
        sum.aliases += applied.aliases;
        sum.importance += applied.importance;
        sum.days.extend(applied.days);
        sum.diaries.extend(applied.diaries);
        self.skipped += skipped;
    }

    fn summary(&self, slot: &ModelSlot) -> String {
        let a = &self.applied;
        if self.passes == 0 {
            return "Обдумывать было нечего.".into();
        }
        let mut parts = Vec::new();
        let changed = a.merged + a.closed + a.past + a.dated + a.aliases + a.importance;
        if changed > 0 {
            parts.push(format!("{changed} {}", plural(changed, "перемена", "перемены", "перемен")));
        }
        if !a.days.is_empty() {
            parts.push(format!(
                "итог дня за {}",
                a.days.iter().map(|day| short_day(day)).collect::<Vec<_>>().join(", ")
            ));
        }
        let what = if parts.is_empty() {
            "ничего менять не понадобилось".to_string()
        } else {
            parts.join(", ")
        };
        let cloud = if slot.cloud { ", в облаке" } else { "" };
        format!(
            "Обдумала {} {}: {what}. Модель — {} ({}{cloud}).",
            a.thought,
            plural(a.thought, "разговор", "разговора", "разговоров"),
            slot.title,
            slot.model
        )
    }

    fn lines(&self, title: impl Fn(&str) -> String) -> Vec<String> {
        let a = &self.applied;
        let mut lines = Vec::new();
        if let (Some(first), Some(last)) = (&self.first, &self.last) {
            let span =
                if first == last { short_day(first) } else { format!("{}–{}", short_day(first), short_day(last)) };
            lines.push(format!("Разговоры за {span}: {}.", a.thought));
        }
        for (count, what) in [
            (a.merged, "Склеено повторов"),
            (a.closed, "Закрыто устаревших"),
            (a.past, "Убрано в историю"),
            (a.dated, "Даты вместо «завтра»"),
            (a.aliases, "Новые имена"),
            (a.importance, "Важность пересчитана"),
        ] {
            if count > 0 {
                lines.push(format!("{what}: {count}."));
            }
        }
        if !a.diaries.is_empty() {
            lines.push(format!("Дневник: {}.", a.diaries.iter().map(|id| title(id)).collect::<Vec<_>>().join(", ")));
        }
        if self.skipped > 0 {
            lines.push(format!("Не прошли проверку: {} (не те записи, повторы, лишнее).", self.skipped));
        }
        lines
    }
}

/// The question to the model: the instructions, then the house and the talk.
fn prompt(input: &Input) -> (String, String) {
    let language = if input.lang == Lang::En { "English" } else { "Russian" };
    let order = if input.order {
        "\"order\" — tidying the records. Leave it empty when nothing needs it; never invent facts.\n\
         - {\"do\":\"merge\",\"keep\":\"F1\",\"drop\":[\"F7\"],\"text\":\"optional: F1 in words that keep what F7 adds\",\"why\":\"…\"} — the same thing written more than once.\n\
         - {\"do\":\"close\",\"fact\":\"F2\",\"by\":\"F9\",\"why\":\"…\"} — a newer record (by) says it is no longer so: moved, changed jobs, sold the car.\n\
         - {\"do\":\"past\",\"fact\":\"F3\",\"why\":\"…\"} — about a day or an event that is over (a test on the 13th when today is the 14th).\n\
         - {\"do\":\"date\",\"fact\":\"F4\",\"text\":\"…\",\"why\":\"…\"} — it says «завтра», «в пятницу», «на следующей неделе»: write it with the date, counting from the day it was written.\n\
         - {\"do\":\"alias\",\"entity\":\"E2\",\"alias\":\"…\",\"why\":\"…\"} — the talk calls someone by another name («Машенька» for Маша).\n\
         - {\"do\":\"importance\",\"fact\":\"F5\",\"importance\":2,\"why\":\"…\"} — 0 small talk … 3 always worth knowing (allergies, family, names). Change only what is clearly off.\n"
    } else {
        "\"order\" — leave it empty this time.\n"
    };
    let days = if input.day {
        "\"days\" — one item for each day of the conversations:\n\
         {\"day\":\"YYYY-MM-DD\",\"summary\":\"…\",\"diaries\":[{\"character\":\"<id>\",\"text\":\"…\"}]}\n\
         - summary: 2–5 short sentences for the household — what happened, what was asked, decided, planned. No greetings, no «пользователь». When the day already has a summary, write the whole day anew and keep what it said.\n\
         - diaries: an entry for each character who talked that day, first person, in their own voice — what they noticed, what they want to remember. 2–4 sentences. When they already have an entry for the day, write it anew and keep what it said.\n"
    } else {
        "\"days\" — leave it empty this time.\n"
    };
    let system = format!(
        "You are the memory of a home voice assistant thinking the day over at night. You get the household's names \
         and records and the conversations no night has thought over yet. Answer with one JSON object and nothing \
         else — no prose, no code fences:\n{{\"order\":[…],\"days\":[…]}}\n\n{order}\n{days}\nRules:\n\
         - Use only the ids given (F…, E…, the characters' ids). Nothing is ever deleted: merge and close keep the history.\n\
         - No guesses about health, money, intimate relations, politics or faith.\n\
         - Write every text in {language}; \"why\" is a few words for the owner."
    );
    let mut user = format!("Today: {} ({}).\n\nNames (E):\n", input.today, input.zone.name());
    for (index, entity) in input.entities.iter().enumerate() {
        user.push_str(&format!("E{} {} [{}]", index + 1, entity.name, entity.kind.as_str()));
        if !entity.aliases.is_empty() {
            user.push_str(&format!(" — also: {}", entity.aliases.join(", ")));
        }
        user.push('\n');
    }
    user.push_str("\nRecords (F) — about · text · importance · written:\n");
    let place = |id: &str| input.entities.iter().position(|e| e.id == id).map(|i| format!("E{}", i + 1));
    for (index, fact) in input.facts.iter().enumerate() {
        let about = match (place(&fact.subject_id), fact.object_id.as_deref().and_then(place)) {
            (Some(subject), Some(object)) => format!("{subject}→{object}"),
            (Some(subject), None) => subject,
            _ => "?".into(),
        };
        let written = local(&fact.created_at, input.zone).0;
        let pinned = if fact.pinned { " · pinned" } else { "" };
        user.push_str(&format!(
            "F{} [{about}] {} · {} · {}{pinned}\n",
            index + 1,
            fact.text,
            fact.importance,
            short_day(&written)
        ));
    }
    if input.facts.is_empty() {
        user.push_str("(none)\n");
    }
    user.push_str("\nConversations:\n");
    for (day, indices) in &input.days {
        user.push_str(&format!("\n## {day}\n"));
        let talked: Vec<String> = input
            .speakers
            .get(day)
            .into_iter()
            .flatten()
            .map(|id| {
                let who = input.characters.get(id);
                let title = who.map(|who| who.title.as_str()).unwrap_or(id.as_str());
                let gender = if who.is_some_and(|who| !who.feminine) { "he" } else { "she" };
                format!("{id} ({title}, {gender})")
            })
            .collect();
        if !talked.is_empty() {
            user.push_str(&format!("Characters: {}\n", talked.join(", ")));
        }
        for note in input.notes.iter().filter(|note| &note.day == day) {
            match &note.character_id {
                None => user.push_str(&format!("Summary so far: {}\n", note.text)),
                Some(id) => user.push_str(&format!("{id}'s diary so far: {}\n", note.text)),
            }
        }
        for &index in indices {
            let episode = &input.episodes[index];
            let time = local(&episode.at, input.zone).1;
            let who = episode.character_id.as_deref().unwrap_or("assistant");
            user.push_str(&format!(
                "[{time} · {who}] Q: {}\nA: {}\n",
                clip(&episode.question, SAID_CHARS),
                clip(&episode.answer, SAID_CHARS)
            ));
        }
    }
    (system, user)
}

/// What the model answered, as JSON: the object in it, fences or words around it aside.
fn parse_plan(answer: &str) -> Option<Value> {
    let start = answer.find('{')?;
    let end = answer.rfind('}')?;
    let plan: Value = serde_json::from_str(answer.get(start..=end)?).ok()?;
    plan.is_object().then_some(plan)
}

/// The model's actions that pass: their records exist and are current, a record is closed or rewritten once, a pinned
/// one is left as the person pinned it, the night's budget holds, and the part each belongs to is on.
fn check(plan: &Value, input: &Input, budget: &mut usize) -> Checked {
    let mut checked = Checked::default();
    let fact = |value: Option<&Value>| -> Option<usize> {
        let index = value?.as_str()?.trim().strip_prefix('F')?.parse::<usize>().ok()?.checked_sub(1)?;
        (index < input.facts.len()).then_some(index)
    };
    let entity = |value: Option<&Value>| -> Option<usize> {
        let index = value?.as_str()?.trim().strip_prefix('E')?.parse::<usize>().ok()?.checked_sub(1)?;
        (index < input.entities.len()).then_some(index)
    };
    let said = |value: Option<&Value>, most: usize| -> Option<String> {
        let text = value?.as_str()?.trim();
        (!text.is_empty() && text.chars().count() <= most).then(|| text.to_string())
    };
    let mut closed: HashSet<usize> = HashSet::new();
    let mut rewritten: HashSet<usize> = HashSet::new();
    let mut scored: HashSet<usize> = HashSet::new();
    let mut named: HashSet<String> = HashSet::new();
    let free = |index: usize, closed: &HashSet<usize>| !closed.contains(&index) && !input.facts[index].pinned;
    for item in plan.get("order").and_then(Value::as_array).into_iter().flatten() {
        if !input.order || *budget == 0 {
            checked.skipped += 1;
            continue;
        }
        let why = item.get("why").and_then(Value::as_str).map(|why| clip(why.trim(), WHY_CHARS)).unwrap_or_default();
        let act = match item.get("do").and_then(Value::as_str).unwrap_or_default() {
            "merge" => (|| {
                let keep = fact(item.get("keep"))?;
                let drop: Vec<usize> =
                    item.get("drop")?.as_array()?.iter().map(|id| fact(Some(id))).collect::<Option<_>>()?;
                let unique: HashSet<usize> = drop.iter().copied().collect();
                if drop.is_empty() || drop.len() > 5 || unique.len() != drop.len() || unique.contains(&keep) {
                    return None;
                }
                if closed.contains(&keep) || !drop.iter().all(|&gone| free(gone, &closed)) {
                    return None;
                }
                let text = match item.get("text").filter(|text| !text.is_null()) {
                    Some(text) => Some(said(Some(text), FACT_CHARS)?).filter(|text| *text != input.facts[keep].text),
                    None => None,
                };
                if text.is_some() && (input.facts[keep].pinned || rewritten.contains(&keep)) {
                    return None;
                }
                Some(Act::Merge { keep, drop, text, why: why.clone() })
            })(),
            "close" => (|| {
                let target = fact(item.get("fact"))?;
                let by = match item.get("by").filter(|by| !by.is_null()) {
                    Some(by) => Some(fact(Some(by))?),
                    None => None,
                };
                if by == Some(target) || !free(target, &closed) || by.is_some_and(|by| closed.contains(&by)) {
                    return None;
                }
                Some(Act::Close { fact: target, by, why: why.clone() })
            })(),
            "past" => (|| {
                let target = fact(item.get("fact"))?;
                free(target, &closed).then(|| Act::Past { fact: target, why: why.clone() })
            })(),
            "date" => (|| {
                let target = fact(item.get("fact"))?;
                let text = said(item.get("text"), FACT_CHARS)?;
                if !free(target, &closed) || rewritten.contains(&target) || text == input.facts[target].text {
                    return None;
                }
                Some(Act::Date { fact: target, text, why: why.clone() })
            })(),
            "alias" => (|| {
                let target = entity(item.get("entity"))?;
                let alias = said(item.get("alias"), ALIAS_CHARS)?;
                let norm = text::stems(&alias).join(" ");
                let known = &input.entities[target];
                let taken = std::iter::once(&known.name)
                    .chain(known.aliases.iter())
                    .any(|name| text::stems(name).join(" ") == norm);
                if norm.is_empty() || taken || !named.insert(format!("{target}:{norm}")) {
                    return None;
                }
                Some(Act::Alias { entity: target, alias, why: why.clone() })
            })(),
            "importance" => (|| {
                let target = fact(item.get("fact"))?;
                let importance = item.get("importance")?.as_i64()?;
                if !(0..=3).contains(&importance)
                    || importance == input.facts[target].importance
                    || !free(target, &closed)
                    || scored.contains(&target)
                {
                    return None;
                }
                Some(Act::Importance { fact: target, importance, why: why.clone() })
            })(),
            _ => None,
        };
        let Some(act) = act else {
            checked.skipped += 1;
            continue;
        };
        match &act {
            Act::Merge { keep, drop, text, .. } => {
                closed.extend(drop.iter().copied());
                if text.is_some() {
                    rewritten.insert(*keep);
                }
            }
            Act::Close { fact, .. } | Act::Past { fact, .. } => {
                closed.insert(*fact);
            }
            Act::Date { fact, .. } => {
                rewritten.insert(*fact);
            }
            Act::Importance { fact, .. } => {
                scored.insert(*fact);
            }
            Act::Alias { .. } => {}
        }
        *budget -= 1;
        checked.acts.push(act);
    }
    let mut written: HashSet<(String, Option<String>)> = HashSet::new();
    for item in plan.get("days").and_then(Value::as_array).into_iter().flatten() {
        let Some(day) =
            item.get("day").and_then(Value::as_str).map(str::trim).filter(|day| input.days.contains_key(*day))
        else {
            checked.skipped += 1;
            continue;
        };
        if !input.day {
            checked.skipped += 1;
            continue;
        }
        if let Some(summary) = item.get("summary").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
            if written.insert((day.to_string(), None)) {
                checked.notes.push(NoteText {
                    day: day.to_string(),
                    character: None,
                    text: clip(summary, SUMMARY_CHARS),
                });
            }
        }
        for entry in item.get("diaries").and_then(Value::as_array).into_iter().flatten() {
            let character = entry.get("character").and_then(Value::as_str).map(str::trim).unwrap_or_default();
            let text = entry.get("text").and_then(Value::as_str).map(str::trim).unwrap_or_default();
            let talked = input.speakers.get(day).is_some_and(|speakers| speakers.contains(character));
            if !talked || text.is_empty() || !written.insert((day.to_string(), Some(character.to_string()))) {
                checked.skipped += 1;
                continue;
            }
            checked.notes.push(NoteText {
                day: day.to_string(),
                character: Some(character.to_string()),
                text: clip(text, DIARY_CHARS),
            });
        }
    }
    checked
}

/// Asks the model once and takes its whole answer.
async fn ask(slot: &ModelSlot, system: String, user: String, cancel: CancellationToken) -> anyhow::Result<String> {
    let request = LlmRequest {
        system,
        messages: vec![LlmMessage::User(user)],
        model: Some(slot.model.clone()),
        ..LlmRequest::default()
    };
    let mut stream = slot.client.stream(request, cancel);
    let (mut text, mut whole) = (String::new(), None);
    while let Some(event) = stream.next().await {
        match event? {
            LlmEvent::Text(piece) => text.push_str(&piece),
            LlmEvent::Turn(turn) => whole = Some(turn.content),
            LlmEvent::Tool(_) => {}
        }
    }
    Ok(whole.filter(|whole| !whole.trim().is_empty()).unwrap_or(text))
}

// ───────── Small helpers ─────────

/// A time as (day, HH:MM) on the station's clock.
fn local(at: &str, zone: Tz) -> (String, String) {
    match DateTime::parse_from_rfc3339(at) {
        Ok(at) => {
            let at = at.with_timezone(&zone);
            (at.format("%Y-%m-%d").to_string(), at.format("%H:%M").to_string())
        }
        Err(_) => (at.get(..10).unwrap_or(at).to_string(), String::new()),
    }
}

/// «2026-11-12» → «12.11».
fn short_day(day: &str) -> String {
    match (day.get(8..10), day.get(5..7)) {
        (Some(d), Some(m)) => format!("{d}.{m}"),
        _ => day.to_string(),
    }
}

/// At most `most` characters, with «…» when cut.
fn clip(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(most.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

fn short(text: &str, most: usize) -> String {
    clip(text.lines().next().unwrap_or_default(), most)
}

fn to_json<T: Serialize>(value: &T) -> Option<String> {
    serde_json::to_string(value).ok()
}

fn plural(n: usize, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    match (n % 10, n % 100) {
        (1, rest) if rest != 11 => one,
        (2..=4, rest) if !(12..=14).contains(&rest) => few,
        _ => many,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures::stream;

    use super::*;
    use crate::config::{Config, DialogProvider, Env};
    use crate::core::events::EventBus;
    use crate::core::identity::principal::user_principal;
    use crate::core::identity::roles::Role;
    use crate::dialog::llm::{AssistantTurn, LlmClient, LlmStream};
    use crate::dialog::PersonaService;
    use crate::memory::model::EntityKind;
    use crate::storage::Db;

    /// A model that answers what `answer` makes of the question, and keeps the questions.
    struct Scripted {
        answer: Box<dyn Fn(&str) -> String + Send + Sync>,
        asked: Mutex<Vec<String>>,
    }

    impl LlmClient for Scripted {
        fn description(&self) -> String {
            "scripted".into()
        }
        fn tools(&self) -> bool {
            false
        }
        fn stream(&self, request: LlmRequest, _cancel: CancellationToken) -> LlmStream {
            let question = match request.messages.last() {
                Some(LlmMessage::User(text)) => text.clone(),
                _ => String::new(),
            };
            self.asked.lock().unwrap().push(question.clone());
            let text = (self.answer)(&question);
            let turn = AssistantTurn { content: text.clone(), ..AssistantTurn::default() };
            stream::iter([Ok(LlmEvent::Text(text)), Ok(LlmEvent::Turn(turn))]).boxed()
        }
    }

    struct Models {
        home: Option<ModelSlot>,
        brain: Option<ModelSlot>,
    }

    impl NightModel for Models {
        fn home(&self) -> Option<ModelSlot> {
            self.home.clone()
        }
        fn brain(&self) -> Option<ModelSlot> {
            self.brain.clone()
        }
        fn pick(&self, provider: &str, model: &str) -> Option<ModelSlot> {
            [&self.home, &self.brain]
                .into_iter()
                .flatten()
                .find(|slot| slot.provider == provider && slot.model == model)
                .cloned()
        }
        fn options(&self) -> Vec<ModelOption> {
            [&self.home, &self.brain]
                .into_iter()
                .flatten()
                .map(|slot| ModelOption {
                    provider: slot.provider.clone(),
                    title: slot.title.clone(),
                    model: slot.model.clone(),
                    cloud: slot.cloud,
                })
                .collect()
        }
    }

    fn slot(client: Arc<Scripted>, cloud: bool) -> ModelSlot {
        ModelSlot {
            client,
            model: "test-model".into(),
            provider: "test".into(),
            kind: DialogProvider::OpenAi,
            title: "Проба".into(),
            cloud,
            backup: None,
        }
    }

    fn scripted(answer: impl Fn(&str) -> String + Send + Sync + 'static) -> Arc<Scripted> {
        Arc::new(Scripted { answer: Box::new(answer), asked: Mutex::new(Vec::new()) })
    }

    fn memory() -> (Arc<MemoryService>, Arc<SettingsService>) {
        let config = Config::for_tests(&Env::default()).dialog;
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let personas = Arc::new(PersonaService::new(&config, db.clone(), settings.clone(), events.clone()).unwrap());
        (Arc::new(MemoryService::new(db, settings.clone(), events, personas)), settings)
    }

    fn admin() -> Principal {
        user_principal("u1", "Админ", Role::Admin, None)
    }

    fn night(run_id: &str) -> Night {
        Night { run_id: run_id.into(), zone: "Europe/Moscow".parse().unwrap(), cancel: CancellationToken::new() }
    }

    /// The id the question gives a record (F…) or a name (E…) whose line holds `text`.
    fn id(question: &str, mark: char, text: &str) -> String {
        question
            .lines()
            .find(|line| line.starts_with(mark) && line.contains(text))
            .and_then(|line| line.split(' ').next())
            .unwrap_or_else(|| panic!("no {mark} line with {text} in:\n{question}"))
            .to_string()
    }

    /// The first day of the conversations in the question.
    fn day_of(question: &str) -> String {
        question.lines().find_map(|line| line.strip_prefix("## ")).unwrap_or_default().to_string()
    }

    fn fact(memory: &MemoryService, id: &str) -> Fact {
        memory.db().with(|c| store::fact(c, id)).unwrap().unwrap()
    }

    #[tokio::test]
    async fn orders_the_records_writes_the_day_and_rolls_the_night_back() {
        let (memory, settings) = memory();
        let admin = admin();
        let masha = memory.add_entity(EntityKind::Person, "Маша", &admin).unwrap();
        let petya = memory.add_entity(EntityKind::Person, "Петя", &admin).unwrap();
        let add = |about: &str, said: &str, pinned: bool| memory.add_fact(about, said, pinned, &admin).unwrap().fact.id;
        let allergy = add(&masha.id, "У Маши аллергия на орехи", false);
        let again = add(&masha.id, "Маша не ест орехи: аллергия", false);
        let tomorrow = add(&petya.id, "У Пети завтра контрольная", false);
        let gone = add(&petya.id, "У Пети контрольная 10.11", false);
        let cafe = add(&masha.id, "Маша работает в кафе", false);
        let library = add(&masha.id, "Маша перешла в библиотеку", false);
        let sister = add(&masha.id, "Маша — сестра Пети", true);
        memory.episode("kitchen", "Как там Машенька?", "Машенька теперь в библиотеке, ей там нравится.");
        memory.episode("kitchen", "А у Пети что?", "Завтра контрольная.");

        let model = scripted(|q| {
            json!({
                "order": [
                    { "do": "merge", "keep": id(q, 'F', "аллергия на орехи"), "drop": [id(q, 'F', "не ест орехи")], "why": "одно и то же" },
                    { "do": "close", "fact": id(q, 'F', "работает в кафе"), "by": id(q, 'F', "перешла в библиотеку"), "why": "перешла" },
                    { "do": "past", "fact": id(q, 'F', "контрольная 10.11"), "why": "прошла" },
                    { "do": "date", "fact": id(q, 'F', "завтра контрольная"), "text": "У Пети контрольная 13.11", "why": "дата" },
                    { "do": "alias", "entity": id(q, 'E', "Маша ["), "alias": "Машенька", "why": "так её зовут" },
                    { "do": "importance", "fact": id(q, 'F', "аллергия на орехи"), "importance": 3, "why": "здоровье" },
                    { "do": "close", "fact": id(q, 'F', "сестра Пети"), "why": "закреплённое не трогать" },
                    { "do": "close", "fact": "F99", "why": "такой нет" },
                    { "do": "delete", "fact": id(q, 'F', "аллергия на орехи") }
                ],
                "days": [{
                    "day": day_of(q),
                    "summary": "Говорили о Маше: она перешла в библиотеку. У Пети контрольная.",
                    "diaries": [
                        { "character": "alice", "text": "Рада за Машу. Надо спросить Петю, как прошла контрольная." },
                        { "character": "arthur", "text": "Меня тут не было." }
                    ]
                }]
            })
            .to_string()
        });
        let reflection = Reflection::new(
            memory.clone(),
            settings.clone(),
            Arc::new(Models { home: Some(slot(model.clone(), false)), brain: None }),
        );
        assert_eq!(SleepPart::work(&reflection), Ok(()));
        assert!(!SleepPart::enabled(&reflection), "off until the owner turns it on");

        let report = reflection.run(&night("r1")).await;
        assert_eq!(report.state, PartState::Done, "{report:?}");
        assert!(report.summary.starts_with("Обдумала 2 разговора: 6 перемен, итог дня за"), "{}", report.summary);
        assert!(report.lines.contains(&"Склеено повторов: 1.".to_string()), "{:?}", report.lines);
        assert!(report.lines.contains(&"Дневник: Алиса.".to_string()), "{:?}", report.lines);
        assert!(report.lines.iter().any(|line| line.starts_with("Не прошли проверку: 4")), "{:?}", report.lines);
        let question = model.asked.lock().unwrap()[0].clone();
        assert!(question.contains("Q: Как там Машенька?"), "{question}");

        assert_eq!(fact(&memory, &again).replaced_by.as_deref(), Some(allergy.as_str()));
        assert_eq!(fact(&memory, &cafe).replaced_by.as_deref(), Some(library.as_str()));
        assert_eq!(fact(&memory, &gone).closed_reason.as_deref(), Some("прошло"));
        assert_eq!(fact(&memory, &tomorrow).text, "У Пети контрольная 13.11");
        assert_eq!(fact(&memory, &allergy).importance, 3);
        assert!(fact(&memory, &sister).valid_to.is_none(), "a pinned record stays as the person pinned it");
        let aliases = || memory.db().with(|c| store::entity(c, &masha.id)).unwrap().unwrap().aliases;
        assert_eq!(aliases(), vec!["Машенька".to_string()]);
        assert_eq!(reflection.pending(), 0);

        let details = reflection.details("r1");
        assert_eq!(details["changes"].as_array().unwrap().len(), 8);
        assert_eq!(details["thought"], 2);
        let notes = details["notes"].as_array().unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[1]["characterTitle"], "Алиса");
        let woke: Vec<String> = memory.wake_up(&memory.viewer()).into_iter().map(|item| item.text).collect();
        assert!(
            woke.iter().any(|line| line.starts_with("Итог дня ") && line.contains("перешла в библиотеку")),
            "{woke:?}"
        );
        assert!(woke.iter().any(|line| line.starts_with("Мой дневник ")), "{woke:?}");

        // «Отменить» one change, then the whole night.
        let alias = details["changes"].as_array().unwrap().iter().find(|c| c["action"] == "alias").unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        memory.undo(&alias, &admin).unwrap();
        assert!(aliases().is_empty());
        assert_eq!(SleepPart::roll_back(&reflection, "r1", &admin), Ok(7));
        assert!(fact(&memory, &again).valid_to.is_none());
        assert!(fact(&memory, &cafe).valid_to.is_none());
        assert!(fact(&memory, &gone).valid_to.is_none());
        assert_eq!(fact(&memory, &tomorrow).text, "У Пети завтра контрольная");
        assert_eq!(fact(&memory, &allergy).importance, 0);
        assert_eq!(reflection.details("r1")["notes"].as_array().unwrap().len(), 0);
        assert_eq!(reflection.pending(), 2, "the talk waits for the next night");
        assert!(memory.wake_up(&memory.viewer()).iter().all(|item| !item.text.starts_with("Итог дня")));
    }

    #[tokio::test]
    async fn waits_without_a_model_at_home_and_keeps_to_its_parts_and_the_budget() {
        let (memory, settings) = memory();
        let admin = admin();
        let masha = memory.add_entity(EntityKind::Person, "Маша", &admin).unwrap();
        for n in 0..55 {
            memory.add_fact(&masha.id, &format!("У Маши вещь номер {n}"), false, &admin).unwrap();
        }
        memory.episode("kitchen", "Привет", "Привет!");
        let model = scripted(|q| {
            let order: Vec<Value> =
                (1..=55).map(|n| json!({ "do": "importance", "fact": format!("F{n}"), "importance": 1 })).collect();
            json!({ "order": order, "days": [{ "day": day_of(q), "summary": "Поздоровались." }] }).to_string()
        });
        let cloud = slot(model.clone(), true);
        let reflection =
            Reflection::new(memory.clone(), settings.clone(), Arc::new(Models { home: None, brain: Some(cloud) }));

        let report = reflection.run(&night("r1")).await;
        assert_eq!(report.state, PartState::Waiting);
        assert!(report.summary.starts_with("Ждёт свою модель"), "{}", report.summary);

        let view = reflection.view();
        assert_eq!((view.model.clone(), view.using.clone()), (MemoryModel::Home, None));
        assert!(view.waiting.unwrap().starts_with("Ждёт свою модель"));
        assert_eq!(view.options.len(), 1, "the brain's model is there to pick");

        // The cloud only once the owner chose it: one model of «Мозг» by name; «Итог дня» off — the days are left out.
        let pick = MemoryModel::Pick { provider: "test".into(), model: "test-model".into() };
        let view = reflection
            .change(ReflectionChange { model: Some(pick.clone()), day: Some(false), ..Default::default() }, None)
            .unwrap();
        assert_eq!(
            (view.model.clone(), view.day, view.home.is_none(), view.waiting.clone()),
            (pick, false, true, None)
        );
        assert!(view.using.unwrap().cloud);
        assert!(view.brain.unwrap().cloud);
        let report = reflection.run(&night("r2")).await;
        assert_eq!(report.state, PartState::Done, "{report:?}");
        assert!(report.summary.contains("в облаке"), "{}", report.summary);
        assert!(report.lines.contains(&format!("Важность пересчитана: {MAX_ACTIONS}.")), "{:?}", report.lines);
        assert!(report.lines.iter().any(|line| line.starts_with("Не прошли проверку: 6")), "{:?}", report.lines);
        assert!(reflection.details("r2")["notes"].as_array().unwrap().is_empty());
        assert!(model.asked.lock().unwrap()[0].contains("F55 "));
        let moon = MemoryModel::Pick { provider: "moon".into(), model: "x".into() };
        assert!(reflection.change(ReflectionChange { model: Some(moon), ..Default::default() }, None).is_err());
        let view = reflection
            .change(ReflectionChange { model: Some(MemoryModel::Brain), ..Default::default() }, None)
            .unwrap();
        assert_eq!(
            (view.model, view.using.map(|using| using.model)),
            (MemoryModel::Brain, Some("test-model".to_string()))
        );
    }

    #[test]
    fn reads_the_plan_out_of_fences_and_words() {
        assert_eq!(parse_plan("```json\n{\"order\":[]}\n```"), Some(json!({ "order": [] })));
        assert_eq!(parse_plan("Вот: {\"days\":[]} — готово"), Some(json!({ "days": [] })));
        assert_eq!(parse_plan("ничего"), None);
        assert_eq!(parse_plan("[1, 2]"), None);
        assert_eq!(clip("абвгд", 3), "аб…");
        assert_eq!(short_day("2026-11-12"), "12.11");
        assert_eq!(plural(21, "разговор", "разговора", "разговоров"), "разговор");
        assert_eq!(plural(12, "разговор", "разговора", "разговоров"), "разговоров");
    }
}
