//! The question a device was just asked by the layer (docs/plans/audio-interface.md, «Честное «Готово» и вопрос в
//! конце»): «На стол или примерить?», «Продолжить?», «Применить?», «Удалить черновик?». It is the device's own, in
//! the layer — not the station's one `continuation`: the station listens after the question because its answer asked
//! it to (`listen`). A phrase without the wake word belongs to the question for a short while; the pack reads it, and
//! what it cannot read is not the question's — the phrase goes on as any other. Its words also stay in the work's
//! record: silence leaves the question there, the thing waits on the desk.
//!
//! The brain's own question is the device's question too (docs/plans/audio-interface-2.md, «К3»): `Kind::Talk` — an
//! answer of the brain that ended with a question; the phrase said to it is the brain's. The layer's rules and reader
//! do not read it (`open`), the closed window does (`open_any`). A question nobody answered in its time ends silent:
//! `on_silence` hears it — the layer marks the work's question and reminds of it once.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// A phrase without the wake word belongs to the question this long after it was asked: the station listens some
/// five seconds after the line, and the line takes a few to say.
pub const ANSWER_FOR: Duration = Duration::from_secs(20);
/// On the computer the panel itself closes its window six seconds after it finished saying the question — and it may
/// say a long answer first: the gateway only lets go of a question forgotten.
pub const COMPUTER_ANSWER_FOR: Duration = Duration::from_secs(60);

/// How long a device's question waits for its answer.
pub fn answer_for(device: &str) -> Duration {
    if device == super::origin::COMPUTER {
        COMPUTER_ANSWER_FOR
    } else {
        ANSWER_FOR
    }
}

/// Who hears that a device's question ended without an answer: the device and the question.
pub type OnSilence = Arc<dyn Fn(&str, &Question) + Send + Sync>;

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
    /// The brain's own question: its answer ended with «?». The phrase said to it goes to the brain.
    Talk,
    /// «Запуск платный: … Запускать?»: a background tool's paid run waits for «да» (docs/plans/long-tools.md). The
    /// question's `work` is the run's key, not a work: there is no work until the answer.
    Run,
    /// «Рассказать подробнее?»: a background work's retelling was cut.
    More,
    /// «Лимит … кончился до …: подождать или остановить?»: a work in a folder waits for a plan's limit; silence waits
    /// too (docs/plans/router-and-works.md, 9).
    Wait,
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
    open: Arc<Mutex<HashMap<String, Question>>>,
    silence: Arc<OnceLock<OnSilence>>,
}

impl Asked {
    fn questions(&self) -> std::sync::MutexGuard<'_, HashMap<String, Question>> {
        self.open.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Who hears a question that ended without an answer.
    pub fn on_silence(&self, hear: OnSilence) {
        let _ = self.silence.set(hear);
    }

    /// The question waits its time: still the same and unanswered then — it ended silent.
    fn expire_later(&self, device: &str, asked: Instant) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
        let (open, silence, device) = (self.open.clone(), self.silence.clone(), device.to_string());
        handle.spawn(async move {
            tokio::time::sleep(answer_for(&device)).await;
            let ended = {
                let mut questions = open.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                match questions.get(&device) {
                    Some(question) if question.asked == asked => questions.remove(&device),
                    _ => None,
                }
            };
            if let (Some(question), Some(hear)) = (ended, silence.get()) {
                hear(&device, &question);
            }
        });
    }

    fn put(&self, device: &str, question: Question) {
        let asked = question.asked;
        self.questions().insert(device.to_string(), question);
        self.expire_later(device, asked);
    }

    /// The device was asked now: its earlier question, if any, is not awaited any more.
    pub fn ask(&self, device: &str, work: &str, kind: Kind, again: bool) {
        self.put(device, Question { work: work.to_string(), kind, again, pending: None, asked: Instant::now() });
    }

    /// The device was asked now about a wish or a change that waits for the answer.
    pub fn ask_with(&self, device: &str, work: &str, kind: Kind, pending: Pending) {
        self.put(
            device,
            Question { work: work.to_string(), kind, again: false, pending: Some(pending), asked: Instant::now() },
        );
    }

    /// The answer came: the question, with what waited for it, is not awaited any more.
    pub fn take(&self, device: &str) -> Option<Question> {
        self.questions().remove(device)
    }

    /// Takes the device's question only when it is about this work: an answer given by a button on the page
    /// «Работы» leaves the device's question about another thing open.
    pub fn take_of(&self, device: &str, work: &str) -> Option<Question> {
        let mut questions = self.questions();
        match questions.get(device) {
            Some(question) if question.work == work => questions.remove(device),
            _ => None,
        }
    }

    /// The device's question while its answer is awaited, whoever asked it — the layer or the brain.
    pub fn open_any(&self, device: &str) -> Option<Question> {
        let mut questions = self.questions();
        match questions.get(device) {
            Some(question) if question.asked.elapsed() < answer_for(device) => Some(question.clone()),
            Some(_) => {
                questions.remove(device);
                None
            }
            None => None,
        }
    }

    /// The layer's question of the device while its answer is awaited: the brain's own is the brain's to read.
    pub fn open(&self, device: &str) -> Option<Question> {
        self.open_any(device).filter(|question| question.kind != Kind::Talk)
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

    #[test]
    fn the_brains_question_is_the_devices_but_not_the_layers_to_read() {
        let asked = Asked::default();
        asked.ask(crate::talk::COMPUTER, "", Kind::Talk, false);
        assert_eq!(asked.open(crate::talk::COMPUTER), None, "the layer's rules do not read the brain's question");
        assert_eq!(asked.open_any(crate::talk::COMPUTER).map(|question| question.kind), Some(Kind::Talk));
        asked.set_age(crate::talk::COMPUTER, ANSWER_FOR);
        assert!(asked.open_any(crate::talk::COMPUTER).is_some(), "the computer's question waits longer");
        asked.set_age(crate::talk::COMPUTER, COMPUTER_ANSWER_FOR);
        assert_eq!(asked.open_any(crate::talk::COMPUTER), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_question_nobody_answered_in_its_time_ends_silent_and_one_answered_does_not() {
        let asked = Asked::default();
        let silent: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        let heard = silent.clone();
        asked.on_silence(Arc::new(move |device: &str, question: &Question| {
            heard.lock().unwrap().push((device.to_string(), question.work.clone()));
        }));
        asked.ask("kitchen", "3", Kind::Ready, false);
        asked.ask("hall", "4", Kind::Ready, false);
        asked.take("hall");
        tokio::time::sleep(ANSWER_FOR + Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        assert_eq!(*silent.lock().unwrap(), [("kitchen".to_string(), "3".to_string())]);
        assert_eq!(asked.open("kitchen"), None);
    }
}
