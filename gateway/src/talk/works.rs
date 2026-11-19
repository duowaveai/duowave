//! The records of works (docs/plans/audio-interface.md, «Запись работы»): one per work, from what was asked to what
//! came of it — the whole wish, what was added on the way, how it ended, the question it waits on, where it was asked.
//! The thread of a conversation is the work itself: nothing to open, nothing to close. For an app the draft's card
//! and the apps' history go on from it; a new version of a finished thing is a new record with the same `target`.
//!
//! What this file keeps for the works to come (docs/plans/audio-interface.md, «Задел на несколько дел»):
//!
//! 1. a work is found only by its id — no list here assumes a device has one work;
//! 2. the thing in focus is computed (`focus`), never kept in a record;
//! 3. the record is flat and grows by fields: a project and a parent work are two more fields of a later migration;
//! 4. an executor starts a work through the same layer a tool does, with an origin of its own;
//! 5. the numbers are here, in one place.
//!
//! The table is `works` (migration 0012). A work that was going on when the gateway stopped becomes `interrupted` at
//! the next start: nothing restarts by itself.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::origin::{Origin, Via};
use crate::storage::Db;
use crate::utils::time::now_ms;

/// Works going on at once.
pub const AT_ONCE: usize = 2;
/// A new work by voice at most this often from one device.
pub const NEW_EVERY_MS: u64 = 2 * 60_000;
/// New works by voice a day (additions and edits do not count).
pub const NEW_PER_DAY: usize = 10;
/// A wish like a thing of this long ago is a question: edit it, or make a new one?
pub const SIMILAR_MS: u64 = 30 * 60_000;
/// The thing talked about this long ago is still in focus.
pub const FOCUS_MS: u64 = 30 * 60_000;
/// Ended works are kept this long.
const KEPT_MS: u64 = 30 * 24 * 60 * 60_000;
/// What a record keeps of a wish and of a note.
const BRIEF: usize = 4000;
const NOTE: usize = 1000;
const TITLE: usize = 80;

/// What a work is about: an app the workshop makes.
pub const APP: &str = "app";
/// A background tool's call (docs/plans/long-tools.md, point 2): its own count, no day's limit — the service counts
/// the money itself.
pub const TOOL: &str = "tool";
/// An app's long task begun in a conversation (docs/plans/long-tools.md, point 4): limited by the app's own budget.
pub const TASK: &str = "task";
/// Background tools' works at once; the next one waits in the queue (the owner's default 2).
pub const TOOLS_AT_ONCE: usize = 3;
/// A work in a work folder (docs/plans/router-and-works.md, 10): an agent in its copy of the folder, for hours.
pub const FOLDER: &str = "folder";
/// Works in folders at once on the helper; the next one waits in the queue (decision 16).
pub const FOLDERS_AT_ONCE: usize = 1;

/// Which works count together against the limits: the things made (apps), the background tools, the apps' tasks, the
/// works in folders — those have their own count, and their new ones by voice count with the apps' for the day.
fn pool(kind: &str) -> u8 {
    match kind {
        TOOL => 1,
        TASK => 2,
        FOLDER => 3,
        _ => 0,
    }
}

/// Where a work is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkState {
    /// Waits for its turn: the limits held it (two at once, a new one by voice once in two minutes); it starts by
    /// itself (docs/plans/audio-interface.md, «Доводка после проверки руками 14.11», 5).
    Queued,
    Working,
    Ready,
    Unfinished,
    Failed,
    Stopped,
    Interrupted,
    Done,
}

impl WorkState {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkState::Queued => "queued",
            WorkState::Working => "working",
            WorkState::Ready => "ready",
            WorkState::Unfinished => "unfinished",
            WorkState::Failed => "failed",
            WorkState::Stopped => "stopped",
            WorkState::Interrupted => "interrupted",
            WorkState::Done => "done",
        }
    }

    pub fn parse(text: &str) -> Option<WorkState> {
        [
            WorkState::Queued,
            WorkState::Working,
            WorkState::Ready,
            WorkState::Unfinished,
            WorkState::Failed,
            WorkState::Stopped,
            WorkState::Interrupted,
            WorkState::Done,
        ]
        .into_iter()
        .find(|state| state.as_str() == text)
    }

    /// The executor is at it now.
    pub fn going(self) -> bool {
        self == WorkState::Working
    }

    /// It waits for its turn.
    pub fn queued(self) -> bool {
        self == WorkState::Queued
    }
}

/// What a thing becomes: a widget of the desk, a skill, a game, a theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Face {
    Widget,
    Skill,
    Game,
    Theme,
}

impl Face {
    pub fn parse(text: &str) -> Option<Face> {
        match text {
            "widget" => Some(Face::Widget),
            "skill" | "plugin" => Some(Face::Skill),
            "game" | "quiz" => Some(Face::Game),
            "theme" => Some(Face::Theme),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Face::Widget => "widget",
            Face::Skill => "skill",
            Face::Game => "game",
            Face::Theme => "theme",
        }
    }

    /// Its place in a pack's `Faces`.
    pub fn index(self) -> usize {
        match self {
            Face::Widget => 0,
            Face::Skill => 1,
            Face::Game => 2,
            Face::Theme => 3,
        }
    }
}

/// Something said to a work on the way: «и ещё кнопку…».
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub text: String,
    pub at: u64,
    pub origin: Origin,
    /// When the executor took it in (the agent read it between its steps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_at: Option<u64>,
}

/// How a person decided about a thing that is ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum End {
    Desk,
    Fitting,
    Parked,
    Declined,
    /// The thing is gone: its app was removed and its draft deleted, by whoever (TalkLayer::recheck, by its id).
    Removed,
}

/// What came of a work: its line, and what became of the thing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkResult {
    pub line: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<End>,
    /// A tool's or a task's result retold by the model in a sentence or two (signed «пересказ» in the panel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The result as it came, for the panel (up to 64 KB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

/// The question a work waits on: its words, what it asks, and until when an answer is taken without the wake word.
/// Silence leaves it in the record: the thing waits on the desk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub text: String,
    pub until: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<super::asked::Kind>,
    /// When it was last asked aloud on a device (docs/plans/audio-interface-2.md, «К3»).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_at: Option<u64>,
    /// How its asking ended: none — not asked yet, or still awaited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    /// It was asked once more after it ended silent: never a second time.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reminded: bool,
}

impl Question {
    /// A question of a finished work, not asked aloud yet.
    pub fn new(text: String, until: u64, kind: super::asked::Kind) -> Question {
        Question { text, until, kind: Some(kind), asked_at: None, outcome: None, reminded: false }
    }

    /// The question still waits for the person: nobody answered it nor said «not now».
    pub fn waits(&self) -> bool {
        self.outcome != Some(Outcome::Declined)
    }
}

/// How the asking of a question ended (docs/plans/audio-interface-2.md, «К3»): answered — the answer goes where it
/// was meant; declined — «нет», «потом», «хватит»: what was named for it beforehand, nothing more; silent — nobody
/// answered in its time: nothing is done, it waits, and is asked once more after the device's next talk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Answered,
    Declined,
    Silent,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Answered => "answered",
            Outcome::Declined => "declined",
            Outcome::Silent => "silent",
        }
    }
}

/// The model's spending on a work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub rounds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Work {
    pub id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub face: Option<Face>,
    pub executor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub title: String,
    pub state: WorkState,
    pub brief: String,
    /// A new version of a thing there is already (an edit, «доделай»), not a new thing.
    pub again: bool,
    pub notes: Vec<Note>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<WorkResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<Question>,
    pub origin: Origin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub said_at: Option<u64>,
    pub tokens: Tokens,
    pub started_at: u64,
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
}

impl Work {
    pub fn device(&self) -> &str {
        &self.origin.device
    }

    /// The executor is at it now.
    pub fn going(&self) -> bool {
        self.state.going()
    }

    /// Its thing is gone (deleted from the desk and the drafts).
    pub fn removed(&self) -> bool {
        self.result.as_ref().is_some_and(|result| result.end == Some(End::Removed))
    }

    /// Ready or not finished, and nobody said what becomes of it yet.
    pub fn awaiting(&self) -> bool {
        matches!(self.state, WorkState::Ready | WorkState::Unfinished)
            && self.result.as_ref().is_none_or(|result| result.end.is_none())
    }

    /// A new work by voice, as the day's limit counts them.
    fn by_voice(&self) -> bool {
        self.origin.via == Via::Voice
    }
}

/// A new work, before it has an id.
#[derive(Debug, Clone)]
pub struct NewWork {
    pub kind: &'static str,
    pub face: Option<Face>,
    pub executor: String,
    pub target: Option<String>,
    pub title: String,
    pub brief: String,
    pub origin: Origin,
    /// A new version of a thing: additions and edits do not count against the day's limit.
    pub again: bool,
}

/// The thing in focus of a device: the one in work, or the one talked about in the last half hour. An answer of one
/// lookup, never kept: its size does not matter.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum Focus {
    None,
    One(Work),
    /// More than one fits: the person is asked which.
    Many(Vec<Work>),
}

/// Why a new work is not started now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// `AT_ONCE` works go on already.
    Busy,
    /// A new work by voice came from this device less than `NEW_EVERY_MS` ago: in so many ms.
    TooSoon(u64),
    /// `NEW_PER_DAY` new works by voice today.
    Enough,
}

/// The works, as the table has them, all in memory: there are few.
pub struct Works {
    db: Option<Arc<Db>>,
    list: Mutex<Vec<Work>>,
    next: AtomicU64,
    /// Started at once: a test raises it (the fifth rule).
    at_once: AtomicU64,
}

fn short(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut kept: String = text.chars().take(max - 1).collect();
    kept.push('…');
    kept
}

impl Works {
    /// The works the table keeps: what was going on when the gateway stopped is interrupted now, ended works older
    /// than a month are gone. Without a database (a check) only memory.
    pub fn open(db: Option<Arc<Db>>) -> Works {
        let now = now_ms();
        let mut list = db.as_deref().map(load).unwrap_or_default();
        list.retain(|work| work.ended_at.is_none_or(|ended| now.saturating_sub(ended) < KEPT_MS));
        let mut cut = Vec::new();
        // A work going on, or waiting in the queue, does not outlive the gateway: interrupted, as a person sees it.
        for work in list.iter_mut().filter(|work| work.going() || work.state.queued()) {
            work.state = WorkState::Interrupted;
            work.ended_at = Some(now);
            work.updated_at = now;
            cut.push(work.clone());
        }
        if let Some(db) = &db {
            let _ = db.with(|c| {
                c.execute(
                    "DELETE FROM works WHERE ended_at IS NOT NULL AND ended_at < ?1",
                    params![(now.saturating_sub(KEPT_MS)) as i64],
                )
            });
            for work in &cut {
                save(db, work);
            }
        }
        if !cut.is_empty() {
            tracing::info!(target: "talk", "{} work(s) were going on when the gateway stopped: interrupted", cut.len());
        }
        let next = list.iter().filter_map(|work| work.id.parse::<u64>().ok()).max().unwrap_or(0) + 1;
        Works { db, list: Mutex::new(list), next: AtomicU64::new(next), at_once: AtomicU64::new(AT_ONCE as u64) }
    }

    fn list(&self) -> MutexGuard<'_, Vec<Work>> {
        self.list.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// How many works may go on at once.
    pub fn at_once(&self) -> usize {
        self.at_once.load(Ordering::Relaxed) as usize
    }

    /// Raises (or lowers) how many go on at once: the fifth rule's test runs five.
    pub fn set_at_once(&self, at_once: usize) {
        self.at_once.store(at_once as u64, Ordering::Relaxed);
    }

    /// Whether a new work may start now from this origin: how many go on, and for a new one by voice how often and how
    /// many today.
    pub fn may_start(&self, origin: &Origin, again: bool, now: u64) -> Result<(), Refused> {
        self.may_start_kind(APP, origin, again, now)
    }

    /// The same for a work of a kind: the works of its pool only — a background tool never holds an app back.
    pub fn may_start_kind(&self, kind: &str, origin: &Origin, again: bool, now: u64) -> Result<(), Refused> {
        let pool = pool(kind);
        let list: Vec<Work> = self.list().iter().filter(|work| self::pool(&work.kind) == pool).cloned().collect();
        match pool {
            2 => return Ok(()),
            1 if list.iter().filter(|work| work.going()).count() >= TOOLS_AT_ONCE => return Err(Refused::Busy),
            1 => return Ok(()),
            _ => {}
        }
        let at_once = if pool == 3 { FOLDERS_AT_ONCE } else { self.at_once() };
        if list.iter().filter(|work| work.going()).count() >= at_once {
            return Err(Refused::Busy);
        }
        if again || origin.via != Via::Voice {
            return Ok(());
        }
        // The day's new works by voice: the apps' and the folders' together (decision 16).
        let list: Vec<Work> =
            self.list().iter().filter(|work| matches!(self::pool(&work.kind), 0 | 3)).cloned().collect();
        let day_ago = now.saturating_sub(24 * 60 * 60_000);
        // What waits in the queue has not started: it holds nothing back, itself least of all.
        let new_by_voice: Vec<&Work> = list
            .iter()
            .filter(|work| work.by_voice() && !work.again && !work.state.queued() && work.started_at > day_ago)
            .collect();
        if new_by_voice.len() >= NEW_PER_DAY {
            return Err(Refused::Enough);
        }
        let last = new_by_voice.iter().filter(|work| work.device() == origin.device).map(|work| work.started_at).max();
        if let Some(last) = last.filter(|last| now.saturating_sub(*last) < NEW_EVERY_MS) {
            return Err(Refused::TooSoon(NEW_EVERY_MS - now.saturating_sub(last)));
        }
        Ok(())
    }

    /// A new record, going on from now.
    pub fn create(&self, new: NewWork) -> Work {
        let now = now_ms();
        let id = self.next.fetch_add(1, Ordering::Relaxed).to_string();
        let work = Work {
            id,
            kind: new.kind.to_string(),
            face: new.face,
            executor: new.executor,
            target: new.target,
            title: short(&new.title, TITLE),
            state: WorkState::Working,
            brief: short(&new.brief, BRIEF),
            again: new.again,
            notes: Vec::new(),
            result: None,
            question: None,
            origin: new.origin,
            said_at: None,
            tokens: Tokens::default(),
            started_at: now,
            updated_at: now,
            ended_at: None,
        };
        self.list().push(work.clone());
        self.keep(&work);
        work
    }

    fn keep(&self, work: &Work) {
        if let Some(db) = &self.db {
            save(db, work);
        }
    }

    pub fn get(&self, id: &str) -> Option<Work> {
        let id = id.trim().trim_start_matches('[').trim_end_matches(']');
        self.list().iter().find(|work| work.id == id).cloned()
    }

    /// Changes a record and keeps it; None when there is no such work.
    pub fn update(&self, id: &str, change: impl FnOnce(&mut Work)) -> Option<Work> {
        let work = {
            let mut list = self.list();
            let work = list.iter_mut().find(|work| work.id == id)?;
            change(work);
            work.updated_at = now_ms();
            if !work.going() && work.ended_at.is_none() {
                work.ended_at = Some(work.updated_at);
            }
            if work.going() {
                work.ended_at = None;
            }
            work.clone()
        };
        self.keep(&work);
        Some(work)
    }

    /// The works of a device (all without one), of a state (any without one), the newest first.
    pub fn all(&self, device: Option<&str>, state: Option<WorkState>) -> Vec<Work> {
        let mut works: Vec<Work> = self
            .list()
            .iter()
            .filter(|work| device.is_none_or(|device| work.device() == device))
            .filter(|work| state.is_none_or(|state| work.state == state))
            .cloned()
            .collect();
        works.sort_by_key(|work| std::cmp::Reverse((work.started_at, work.id.parse::<u64>().unwrap_or(0))));
        works
    }

    /// The newest work of an executor's thing.
    pub fn of_target(&self, executor: &str, target: &str) -> Option<Work> {
        self.all(None, None)
            .into_iter()
            .find(|work| work.executor == executor && work.target.as_deref() == Some(target))
    }

    /// All the works of an executor's thing, the newest first.
    pub fn of_thing(&self, executor: &str, target: &str) -> Vec<Work> {
        self.all(None, None)
            .into_iter()
            .filter(|work| work.executor == executor && work.target.as_deref() == Some(target))
            .collect()
    }

    /// The newest work going on an executor's thing — not the newest work of it, if going: a newer record that ended
    /// would hide an older one still going, and the run's end would never reach it (live 19.11).
    pub fn going_on(&self, executor: &str, target: &str) -> Option<Work> {
        self.of_thing(executor, target).into_iter().find(Work::going)
    }

    /// How many go on now.
    pub fn going(&self) -> usize {
        self.list().iter().filter(|work| work.going()).count()
    }

    /// The thing in focus of a device: the one in work, or the one talked about in the last half hour — this one
    /// function answers it for the rules and the tools (the second rule).
    pub fn focus(&self, device: &str, now: u64) -> Focus {
        let works = self.all(Some(device), None);
        let going: Vec<Work> = works.iter().filter(|work| work.going()).cloned().collect();
        match going.len() {
            1 => return Focus::One(going[0].clone()),
            n if n > 1 => return Focus::Many(going),
            _ => {}
        }
        // Of the recent ones, the one talked about last; one thing in several records counts once. A thing that is gone
        // is in nobody's focus.
        let mut recent: Vec<Work> = works
            .into_iter()
            .filter(|work| now.saturating_sub(work.updated_at) < FOCUS_MS && !work.removed())
            .collect();
        recent.sort_by_key(|work| std::cmp::Reverse(work.updated_at));
        match recent.into_iter().next() {
            Some(work) => Focus::One(work),
            None => Focus::None,
        }
    }
}

/// A record's note: its text kept short.
pub fn note_of(text: &str, origin: Origin, at: u64) -> Note {
    Note { text: short(text, NOTE), at, origin, read_at: None }
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

fn save(db: &Db, work: &Work) {
    let saved = db.with(|c| {
        c.execute(
            "INSERT INTO works (id, kind, face, executor, target, title, state, brief, notes, result, question, origin,
                 device, said_at, tokens_in, tokens_out, tokens_cache_read, tokens_cache_write, rounds, started_at,
                 updated_at, ended_at, again)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
                 ?23)
             ON CONFLICT (id) DO UPDATE SET face = excluded.face, target = excluded.target, title = excluded.title,
                 state = excluded.state, brief = excluded.brief, notes = excluded.notes, result = excluded.result,
                 question = excluded.question, said_at = excluded.said_at, tokens_in = excluded.tokens_in,
                 tokens_out = excluded.tokens_out, tokens_cache_read = excluded.tokens_cache_read,
                 tokens_cache_write = excluded.tokens_cache_write, rounds = excluded.rounds,
                 updated_at = excluded.updated_at, ended_at = excluded.ended_at",
            params![
                work.id,
                work.kind,
                work.face.map(Face::as_str),
                work.executor,
                work.target,
                work.title,
                work.state.as_str(),
                work.brief,
                json(&work.notes),
                work.result.as_ref().map(json),
                work.question.as_ref().map(json),
                json(&work.origin),
                work.origin.device,
                work.said_at.map(|at| at as i64),
                work.tokens.input as i64,
                work.tokens.output as i64,
                work.tokens.cache_read as i64,
                work.tokens.cache_write as i64,
                work.tokens.rounds as i64,
                work.started_at as i64,
                work.updated_at as i64,
                work.ended_at.map(|at| at as i64),
                work.again,
            ],
        )
        .map(|_| ())
    });
    if let Err(error) = saved {
        tracing::warn!(target: "talk", "the work {} is not kept: {error}", work.id);
    }
}

fn load(db: &Db) -> Vec<Work> {
    let read = db.with(|c| {
        let mut statement = c.prepare(
            "SELECT id, kind, face, executor, target, title, state, brief, notes, result, question, origin, said_at,
                 tokens_in, tokens_out, tokens_cache_read, tokens_cache_write, rounds, started_at, updated_at, ended_at, again
             FROM works ORDER BY started_at, id",
        )?;
        let rows = statement.query_map([], |row| {
            let number = |index: usize| row.get::<_, i64>(index).map(|value| value.max(0) as u64);
            let maybe = |index: usize| row.get::<_, Option<i64>>(index).map(|value| value.map(|value| value.max(0) as u64));
            let text = |index: usize| row.get::<_, Option<String>>(index);
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                text(2)?,
                row.get::<_, String>(3)?,
                text(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                (text(9)?, text(10)?, row.get::<_, String>(11)?),
                maybe(12)?,
                Tokens { input: number(13)?, output: number(14)?, cache_read: number(15)?, cache_write: number(16)?, rounds: number(17)? },
                (number(18)?, number(19)?, maybe(20)?, row.get::<_, bool>(21)?),
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    });
    let rows = match read {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(target: "talk", "the works are not read: {error}");
            return Vec::new();
        }
    };
    rows.into_iter()
        .filter_map(
            |(
                id,
                kind,
                face,
                executor,
                target,
                title,
                state,
                brief,
                notes,
                (result, question, origin),
                said_at,
                tokens,
                (started_at, updated_at, ended_at, again),
            )| {
                Some(Work {
                    id,
                    kind,
                    face: face.as_deref().and_then(Face::parse),
                    executor,
                    target,
                    title,
                    state: WorkState::parse(&state)?,
                    brief,
                    again,
                    notes: serde_json::from_str(&notes).unwrap_or_default(),
                    result: result.and_then(|text| serde_json::from_str(&text).ok()),
                    question: question.and_then(|text| serde_json::from_str(&text).ok()),
                    origin: serde_json::from_str(&origin).ok()?,
                    said_at,
                    tokens,
                    started_at,
                    updated_at,
                    ended_at,
                })
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::talk::origin::Activator;

    fn new(device: &str, title: &str, via: Via) -> NewWork {
        let origin = match via {
            Via::Voice => Origin::station(device, true),
            Via::Text => Origin::computer(Activator::Text),
        };
        NewWork {
            kind: APP,
            face: Some(Face::Widget),
            executor: "workshop".into(),
            target: None,
            title: title.into(),
            brief: format!("{title}: всё желание"),
            origin,
            again: false,
        }
    }

    #[test]
    fn keeps_a_work_from_the_wish_to_its_end_and_a_restart_interrupts_what_went_on() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(Some(dir.path())).unwrap());
        let works = Works::open(Some(db.clone()));
        let spanish = works.create(new("kitchen", "Испанский: слова дня", Via::Voice));
        let done = works.create(new("kitchen", "Полив", Via::Voice));
        assert_eq!((spanish.id.as_str(), done.id.as_str()), ("1", "2"));
        works.update(&spanish.id, |work| {
            work.target = Some("golos-vidzhet-1".into());
            work.notes.push(note_of("и ещё кнопку произношения", Origin::station("kitchen", true), 5));
        });
        works.update(&done.id, |work| {
            work.state = WorkState::Done;
            work.result =
                Some(WorkResult {
                    line: "Полив: готово.".into(), end: Some(End::Desk), ..WorkResult::default()
                });
        });
        assert_eq!(works.going(), 1);
        assert!(works.get("2").unwrap().ended_at.is_some(), "an ended work knows when");
        assert_eq!(works.get("[1]").unwrap().notes.len(), 1, "a label finds it too");
        drop(works);

        // The gateway restarted: what went on is interrupted; nothing restarts; new ids go on after the old ones.
        let works = Works::open(Some(db));
        let spanish = works.get("1").unwrap();
        assert_eq!(spanish.state, WorkState::Interrupted);
        assert!(spanish.ended_at.is_some());
        assert_eq!(
            (spanish.target.as_deref(), spanish.notes[0].text.as_str()),
            (Some("golos-vidzhet-1"), "и ещё кнопку произношения")
        );
        assert_eq!(works.get("2").unwrap().result.unwrap().end, Some(End::Desk));
        assert_eq!(works.create(new("hall", "Часы", Via::Voice)).id, "3");
        assert_eq!(works.of_target("workshop", "golos-vidzhet-1").unwrap().id, "1");
        assert_eq!(works.all(Some("hall"), None).len(), 1);
        assert_eq!(works.all(None, Some(WorkState::Interrupted)).len(), 1);
    }

    #[test]
    fn the_focus_is_the_work_going_on_or_the_one_talked_about_lately_and_two_are_a_question() {
        let works = Works::open(None);
        assert_eq!(works.focus("kitchen", now_ms()), Focus::None);
        let first = works.create(new("kitchen", "Испанский", Via::Voice));
        assert!(matches!(works.focus("kitchen", now_ms()), Focus::One(work) if work.id == first.id));
        assert_eq!(works.focus("hall", now_ms()), Focus::None, "another device has its own focus");
        works.set_at_once(5);
        let second = works.create(new("kitchen", "Учитель", Via::Voice));
        let Focus::Many(both) = works.focus("kitchen", now_ms()) else { panic!("two go on") };
        assert_eq!(both.len(), 2);
        works.update(&first.id, |work| work.state = WorkState::Ready);
        assert!(matches!(works.focus("kitchen", now_ms()), Focus::One(work) if work.id == second.id));
        works.update(&second.id, |work| work.state = WorkState::Ready);
        assert!(
            matches!(works.focus("kitchen", now_ms()), Focus::One(work) if work.id == second.id),
            "the newest change"
        );
        assert_eq!(
            works.focus("kitchen", now_ms() + FOCUS_MS + 1),
            Focus::None,
            "half an hour later nothing is in focus"
        );
    }

    #[test]
    fn two_go_on_at_once_one_by_voice_every_two_minutes_and_ten_a_day() {
        let works = Works::open(None);
        let kitchen = Origin::station("kitchen", true);
        let now = now_ms();
        assert_eq!(works.may_start(&kitchen, false, now), Ok(()));
        let first = works.create(new("kitchen", "Раз", Via::Voice));
        assert!(
            matches!(works.may_start(&kitchen, false, now), Err(Refused::TooSoon(ms)) if ms > 0 && ms <= NEW_EVERY_MS)
        );
        assert_eq!(works.may_start(&kitchen, true, now), Ok(()), "an edit of a thing is not a new one");
        assert_eq!(works.may_start(&Origin::station("hall", true), false, now), Ok(()), "another device");
        assert_eq!(works.may_start(&kitchen, false, now + NEW_EVERY_MS + 1000), Ok(()));
        works.create(new("hall", "Два", Via::Voice));
        assert_eq!(works.may_start(&Origin::computer(Activator::Text), false, now), Err(Refused::Busy), "two go on");
        works.update(&first.id, |work| work.state = WorkState::Ready);
        assert_eq!(works.may_start(&Origin::computer(Activator::Text), false, now), Ok(()), "typed is not by voice");
        for n in 0..8 {
            let work = works.create(new(&format!("room-{n}"), "Ещё", Via::Voice));
            works.update(&work.id, |work| work.state = WorkState::Done);
        }
        assert_eq!(
            works.may_start(&Origin::station("attic", true), false, now + NEW_EVERY_MS + 1000),
            Err(Refused::Enough)
        );
    }
}

/// The first rule of «Задел на несколько дел»: a work is found only by its id — with the limit raised, five works go
/// on at once, and each gets its own words added on the way.
#[cfg(test)]
mod many {
    use std::sync::{Arc, Mutex};

    use futures::future::BoxFuture;

    use super::*;
    use crate::core::events::EventBus;
    use crate::talk::executor::{Closed, Doing, Executor, Reporter, Started};
    use crate::talk::layer::TalkLayer;
    use crate::talk::origin::Activator;
    use crate::talk::workshop::WORKSHOP;

    /// An executor that keeps what it was given.
    #[derive(Default)]
    struct Keeper {
        added: Mutex<Vec<(String, String)>>,
    }

    impl Executor for Keeper {
        fn name(&self) -> &'static str {
            WORKSHOP
        }
        fn listen(&self, _reports: Reporter) {}
        fn start(&self, work: &Work) -> Result<Started, String> {
            Ok(Started { target: format!("thing-{}", work.id), title: work.title.clone() })
        }
        fn add(&self, work: &Work, text: &str) -> bool {
            self.added.lock().unwrap().push((work.target.clone().unwrap_or_default(), text.to_string()));
            true
        }
        fn thing(&self, _name: &str) -> Option<Started> {
            None
        }
        fn open<'a>(&'a self, target: &'a str, _copy: bool) -> BoxFuture<'a, Result<Started, Closed>> {
            Box::pin(async move { Ok(Started { target: target.to_string(), title: target.to_string() }) })
        }
        fn status(&self, _work: &Work) -> Option<Doing> {
            Some(Doing::Writing)
        }
        fn details(&self, _work: &Work) -> Option<String> {
            None
        }
        fn facts<'a>(&'a self, _work: &'a Work) -> BoxFuture<'a, crate::talk::executor::Facts> {
            Box::pin(async { crate::talk::executor::Facts::default() })
        }
        fn put<'a>(
            &'a self,
            _work: &'a Work,
            _end: End,
            _by: crate::talk::executor::ByVoice,
        ) -> BoxFuture<'a, Result<crate::talk::executor::Put, String>> {
            Box::pin(async { Ok(crate::talk::executor::Put::Parked) })
        }
        fn stop(&self, _work: &Work) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn five_works_go_on_at_once_and_each_gets_its_own_notes() {
        let layer = TalkLayer::new(None, EventBus::new());
        let keeper = Arc::new(Keeper::default());
        layer.add_executor(keeper.clone());
        layer.works().set_at_once(5);
        let typed = || Origin::computer(Activator::Text);
        let titles = ["Часы", "Погода в Лиссабоне", "Курс доллара", "Слова дня", "Полив цветов"];
        for title in titles {
            let said = layer.start(typed(), &format!("сделай {title}"), None, Some(title), true).unwrap();
            assert!(said.starts_with("Делаю: "), "{said}");
        }
        assert_eq!(layer.works().going(), 5);
        let sixth = layer.start(typed(), "ещё одно", None, Some("Ещё"), true).unwrap();
        assert_eq!(sixth, "Сейчас делается другое — начну следом: ещё.", "the sixth waits in the queue");
        assert_eq!(layer.works().all(None, Some(WorkState::Queued)).len(), 1);
        for (n, title) in titles.iter().enumerate() {
            let label = format!("р{}", n + 1);
            let said = layer.add(typed(), &format!("и ещё {n}"), Some(&label), false).await.unwrap();
            assert_eq!(said, format!("Добавлю: и ещё {n}."), "{title}");
        }
        for work in layer.works().all(None, None).into_iter().filter(|work| !work.state.queued()) {
            assert_eq!(work.notes.len(), 1, "{}", work.title);
            let n = work.id.parse::<usize>().unwrap() - 1;
            assert_eq!(work.notes[0].text, format!("и ещё {n}"));
        }
        let added = keeper.added.lock().unwrap().clone();
        let expected: Vec<(String, String)> =
            (0..5).map(|n| (format!("thing-{}", n + 1), format!("и ещё {n}"))).collect();
        assert_eq!(added, expected);
        // Two going on and none named: the person is asked which.
        let asked = layer.add(typed(), "покрупнее", None, false).await.unwrap_err();
        assert!(matches!(asked, crate::talk::work_tools::Refusal::Which(ref things) if things.len() == 5), "{asked:?}");
    }

    /// 20.11: «Делаю: … Что-то ещё добавить?» is the device's question. «Нет, не надо» answers it — the work goes on;
    /// a lone «да» asks what; «и ещё…» goes into the work; «как там» stays a question about it; only «останови» stops.
    #[tokio::test]
    async fn the_answer_no_to_anything_else_lets_the_work_go_on_and_only_a_stop_stops_it() {
        use crate::talk::asked::Kind;
        use crate::talk::origin::COMPUTER;
        use crate::talk::rules::{Heard, TalkRules};

        let layer = TalkLayer::new(None, EventBus::new());
        let keeper = Arc::new(Keeper::default());
        layer.add_executor(keeper.clone());
        let rules = TalkRules::new(&layer);
        let typed = || Origin::computer(Activator::Answer);
        let said = layer.start(typed(), "сделай часы", None, Some("Часы"), true).unwrap();
        assert!(said.ends_with("Что-то ещё добавить?"), "{said}");
        let id = layer.works().all(None, None)[0].id.clone();
        assert_eq!(layer.asked.open(COMPUTER).map(|question| question.kind), Some(Kind::Extra));
        let answer = |text: &str| rules.answer_on(&crate::skills::text::normalize(text), COMPUTER);

        for no in ["Нет, не надо", "Нет, не надо ничего больше добавлять", "Ничего", "Так оставь"]
        {
            layer.asked.ask(COMPUTER, &id, Kind::Extra, false);
            let heard = answer(no).expect(no);
            assert_eq!(rules.act(heard, typed()).await.as_deref(), Some("Хорошо, продолжаю."), "{no}");
            assert!(layer.works().get(&id).is_some_and(|work| work.going()), "{no}: the work goes on");
        }
        assert!(keeper.added.lock().unwrap().is_empty(), "nothing was added");

        layer.asked.ask(COMPUTER, &id, Kind::Extra, false);
        assert_eq!(rules.act(answer("Да").unwrap(), typed()).await.as_deref(), Some("Что добавить?"));
        assert_eq!(layer.asked.open(COMPUTER).map(|question| question.kind), Some(Kind::Extra), "asked again");
        let heard = answer("И ещё секундную стрелку").expect("an addition");
        assert_eq!(rules.act(heard, typed()).await.as_deref(), Some("Добавлю: секундную стрелку."));
        assert_eq!(keeper.added.lock().unwrap().len(), 1);
        // Other words — a wish, talk in the room — are not the answer: the reader tells them apart.
        layer.asked.ask(COMPUTER, &id, Kind::Extra, false);
        assert_eq!(answer("А что на ужин?"), None);
        assert_eq!(answer("Да, сделай фон синим"), None);

        layer.asked.ask(COMPUTER, &id, Kind::Extra, false);
        assert_eq!(answer("Как там?"), None, "a question about the work is not the answer");
        assert!(!matches!(answer("Останови"), Some(Heard::Answer { answer: crate::talk::ready::Answer::No, .. })));
        let said = rules.act(answer("Останови").unwrap(), typed()).await.unwrap_or_default();
        // The executor was asked to stop; its report ends the record.
        assert!(said.starts_with("Остановлено"), "{said}");
    }
}
