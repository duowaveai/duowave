//! The question a device was just asked by the layer (docs/plans/audio-interface.md, «Честное «Готово» и вопрос в
//! конце»): «На стол или примерить?», «Продолжить?», «Применить?», «Удалить черновик?». It is the device's own, in
//! the layer — not the station's one `continuation`: the station listens after the question because its answer asked
//! it to (`listen`). A phrase without the wake word belongs to the question for a short while; the pack reads it, and
//! what it cannot read is not the question's — the phrase goes on as any other. Its words also stay in the work's
//! record: silence leaves the question there, the thing waits on the desk.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// A phrase without the wake word belongs to the question this long after it was asked: the station listens some
/// five seconds after the line, and the line takes a few to say.
pub const ANSWER_FOR: Duration = Duration::from_secs(20);

/// What the question was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// «На стол или примерить?»
    Ready,
    /// «Продолжить?»
    Unfinished,
    /// «Применить?»: a new version of a thing on the desk asks for more rights.
    Apply,
    /// «Удалить черновик?»
    Delete,
    /// «Поправить «Испанский» — или сделать новую?»: a wish like a thing of the last half hour.
    Similar,
    /// «…встроенное приложение, его не правят. Сделать свою копию?»
    Copy,
    /// «Какую: «Часы» или «Часы с будильником»?»: the action waits for the thing.
    Which,
    /// ««Испанский» больше нет — его убрали. Сделать заново?»: the thing's wish waits.
    Remake,
}

/// What was asked for before the question, done once it is answered: the wish of «Поправить … или новую?», the change
/// of «Сделать свою копию?».
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pending {
    pub text: String,
    pub title: Option<String>,
    pub face: Option<super::works::Face>,
    pub thing: Option<String>,
    /// «Какую?»: the action that waits (`add`, `status`, `show`, `desk`, `details`, `stop`) and the works to choose
    /// from, in the order they were named.
    pub act: Option<String>,
    pub choices: Vec<String>,
    /// The question as it was said, for the reader.
    pub question: Option<String>,
}

/// One device's question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// The work it is about; empty for an app no work names (a built-in one's copy).
    pub work: String,
    pub kind: Kind,
    /// «Подробнее» says the details and asks again once.
    pub again: bool,
    /// What waits for the answer to be done.
    pub pending: Option<Pending>,
    asked: Instant,
}

/// The questions of every device.
#[derive(Default)]
pub struct Asked {
    open: Mutex<HashMap<String, Question>>,
}

impl Asked {
    fn questions(&self) -> std::sync::MutexGuard<'_, HashMap<String, Question>> {
        self.open.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The device was asked now: its earlier question, if any, is not awaited any more.
    pub fn ask(&self, device: &str, work: &str, kind: Kind, again: bool) {
        let question = Question { work: work.to_string(), kind, again, pending: None, asked: Instant::now() };
        self.questions().insert(device.to_string(), question);
    }

    /// The device was asked now about a wish or a change that waits for the answer.
    pub fn ask_with(&self, device: &str, work: &str, kind: Kind, pending: Pending) {
        let question =
            Question { work: work.to_string(), kind, again: false, pending: Some(pending), asked: Instant::now() };
        self.questions().insert(device.to_string(), question);
    }

    /// The answer came: the question, with what waited for it, is not awaited any more.
    pub fn take(&self, device: &str) -> Option<Question> {
        self.questions().remove(device)
    }

    /// The device's question while its answer is awaited.
    pub fn open(&self, device: &str) -> Option<Question> {
        let mut questions = self.questions();
        match questions.get(device) {
            Some(question) if question.asked.elapsed() < ANSWER_FOR => Some(question.clone()),
            Some(_) => {
                questions.remove(device);
                None
            }
            None => None,
        }
    }

    /// The answer came: the question is not awaited any more.
    pub fn answered(&self, device: &str) {
        self.questions().remove(device);
    }

    /// For a test: the device's question was asked this long ago.
    #[cfg(test)]
    pub fn set_age(&self, device: &str, age: Duration) {
        if let Some(question) = self.questions().get_mut(device) {
            question.asked = Instant::now() - age;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_has_one_question_for_a_short_while() {
        let asked = Asked::default();
        assert_eq!(asked.open("kitchen"), None);
        asked.ask("kitchen", "3", Kind::Ready, false);
        assert_eq!(
            asked.open("kitchen").map(|question| (question.work, question.kind)),
            Some(("3".into(), Kind::Ready))
        );
        assert_eq!(asked.open("hall"), None, "another device was not asked");
        asked.ask("kitchen", "3", Kind::Delete, false);
        assert_eq!(asked.open("kitchen").map(|question| question.kind), Some(Kind::Delete), "the newer question");
        asked.set_age("kitchen", ANSWER_FOR);
        assert_eq!(asked.open("kitchen"), None, "too late: the phrase is not its");
        asked.ask("kitchen", "4", Kind::Unfinished, false);
        asked.answered("kitchen");
        assert_eq!(asked.open("kitchen"), None);
    }
}
