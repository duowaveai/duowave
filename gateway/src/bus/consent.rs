//! A person's «да» (docs/plans/bus.md, decisions 2, 9, 10): the questions the bus asks when an app first reaches for
//! what its manifest asked for, or does something dangerous no person started. A question waits a day; the same one
//! asked again while it waits is the same question. «Да» to the first kind is remembered as a link by the bus; «да»
//! to the second buys one action, used within ten minutes.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::address::{Address, Pattern};
use super::danger::Danger;
use super::level::Level;
use super::policy::AskKind;
use crate::utils::time::now_iso;

/// How long a question waits for its answer.
const QUESTION_TTL: Duration = Duration::from_secs(24 * 3600);
/// How long a «да» to one dangerous action waits to be used.
const ONCE_TTL: Duration = Duration::from_secs(10 * 60);
/// More questions than this at once: the oldest go.
const MAX_QUESTIONS: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub id: String,
    pub kind: AskKind,
    pub from: Address,
    pub to: Address,
    pub handle: String,
    /// The step it asks for: what the app asked for in its manifest, so one «да» covers what it said it would do.
    pub level: Level,
    /// What the answer is remembered for: the participant (`app/notes`), or a folder and all in it
    /// (`files/home/Документы/*`) — what the person was told.
    pub grant: Pattern,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub danger: Option<Danger>,
    /// For people: «Утреннее шоу», «Календарь».
    pub from_title: String,
    pub to_title: String,
    /// What it wants in people's words: «звать «Календарь»».
    pub what: String,
    /// Who started the chain, for people.
    pub root: String,
    pub at: String,
    #[serde(skip)]
    born: Option<Instant>,
}

impl Question {
    pub fn new(kind: AskKind, from: Address, to: Address, handle: &str, level: Level) -> Question {
        let grant = Pattern::exact(&to);
        Question {
            id: crate::utils::ids::uuid(),
            kind,
            from,
            to,
            grant,
            handle: handle.to_string(),
            level,
            danger: None,
            from_title: String::new(),
            to_title: String::new(),
            what: String::new(),
            root: String::new(),
            at: now_iso(),
            born: Some(Instant::now()),
        }
    }

    /// The same question: for the phone's kind the same caller and the same grant, whatever handle it reached
    /// for; for one dangerous action the same handle too.
    fn same(&self, other: &Question) -> bool {
        if self.kind != other.kind || self.from != other.from {
            return false;
        }
        match self.kind {
            AskKind::Remember => self.grant == other.grant,
            AskKind::Once => self.to == other.to && self.handle == other.handle,
        }
    }

    fn fresh(&self, ttl: Duration) -> bool {
        self.born.is_none_or(|born| born.elapsed() < ttl)
    }
}

/// A «да» to one dangerous action, waiting to be used.
#[derive(Debug, Clone)]
struct Once {
    from: Address,
    to: Address,
    handle: String,
    at: Instant,
}

#[derive(Default)]
pub struct Consent {
    questions: Mutex<Vec<Question>>,
    once: Mutex<Vec<Once>>,
}

impl Consent {
    pub fn new() -> Consent {
        Consent::default()
    }

    /// Asks, or finds the same question still waiting: its id, and whether it is new.
    pub fn ask(&self, question: Question) -> (String, bool) {
        let mut questions = self.questions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        questions.retain(|old| old.fresh(QUESTION_TTL));
        if let Some(old) = questions.iter().find(|old| old.same(&question)) {
            return (old.id.clone(), false);
        }
        let id = question.id.clone();
        questions.push(question);
        let excess = questions.len().saturating_sub(MAX_QUESTIONS);
        questions.drain(..excess);
        (id, true)
    }

    /// The questions waiting, the oldest first.
    pub fn waiting(&self) -> Vec<Question> {
        let mut questions = self.questions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        questions.retain(|old| old.fresh(QUESTION_TTL));
        questions.clone()
    }

    /// The answer: the question it answers, gone from the waiting ones. A «да» to a dangerous action waits to be
    /// used once.
    pub fn answer(&self, id: &str, yes: bool) -> Option<Question> {
        let question = {
            let mut questions = self.questions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let index = questions.iter().position(|question| question.id == id)?;
            questions.remove(index)
        };
        if yes && question.kind == AskKind::Once {
            let mut once = self.once.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            once.retain(|old| old.at.elapsed() < ONCE_TTL);
            once.push(Once {
                from: question.from.clone(),
                to: question.to.clone(),
                handle: question.handle.clone(),
                at: Instant::now(),
            });
        }
        Some(question)
    }

    /// Uses up a «да» to this one action, if one waits.
    pub fn take_once(&self, from: &Address, to: &Address, handle: &str) -> bool {
        let mut once = self.once.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        once.retain(|old| old.at.elapsed() < ONCE_TTL);
        match once.iter().position(|old| &old.from == from && &old.to == to && old.handle == handle) {
            Some(index) => {
                once.remove(index);
                true
            }
            None => false,
        }
    }

    /// Whether a «да» to this one action waits, without using it.
    pub fn has_once(&self, from: &Address, to: &Address, handle: &str) -> bool {
        let once = self.once.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        once.iter().any(|old| old.at.elapsed() < ONCE_TTL && &old.from == from && &old.to == to && old.handle == handle)
    }

    /// Drops the questions of a participant gone (an app removed).
    pub fn forget(&self, address: &Address) {
        let mut questions = self.questions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        questions.retain(|question| &question.from != address && &question.to != address);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(text: &str) -> Address {
        Address::parse(text).unwrap()
    }

    #[test]
    fn the_same_question_waits_once_and_an_answer_takes_it_away() {
        let consent = Consent::new();
        let question = || Question::new(AskKind::Remember, address("app/a"), address("app/b"), "list", Level::Call);
        let (id, new) = consent.ask(question());
        assert!(new);
        let (again, new) = consent.ask(question());
        assert_eq!((again.as_str(), new), (id.as_str(), false));
        // Another handle of the same participant: the same question — one «да» covers what the app asked for.
        let other = Question::new(AskKind::Remember, address("app/a"), address("app/b"), "add", Level::Call);
        assert_eq!(consent.ask(other).0, id);
        assert_eq!(consent.waiting().len(), 1);
        let answered = consent.answer(&id, true).unwrap();
        assert_eq!(answered.handle, "list");
        assert!(consent.waiting().is_empty());
        assert!(consent.answer(&id, true).is_none());
        // A remembered «да» is the bus's link, not a waiting one-off.
        assert!(!consent.take_once(&address("app/a"), &address("app/b"), "list"));
    }

    #[test]
    fn a_yes_to_a_dangerous_action_buys_one_action() {
        let consent = Consent::new();
        let (id, _) =
            consent.ask(Question::new(AskKind::Once, address("app/a"), address("app/lock"), "open", Level::Call));
        consent.answer(&id, true).unwrap();
        assert!(consent.has_once(&address("app/a"), &address("app/lock"), "open"));
        assert!(consent.take_once(&address("app/a"), &address("app/lock"), "open"));
        assert!(!consent.take_once(&address("app/a"), &address("app/lock"), "open"));
        let (no, _) =
            consent.ask(Question::new(AskKind::Once, address("app/a"), address("app/lock"), "open", Level::Call));
        consent.answer(&no, false).unwrap();
        assert!(!consent.has_once(&address("app/a"), &address("app/lock"), "open"));
    }
}
