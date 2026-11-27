//! A scripted Thinking: the audio side's stand-in for the other side in its tests (the contract's rule 9). A test
//! says what to answer each phrase with — events and pauses —, sends notices when it likes, and reads back the
//! phrases and the news the audio side sent.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::mpsc;
use futures::StreamExt;

use super::{Event, Events, News, Notices, Outcome, Outside, Phrase, Thinking, Told, What};

/// One step of a scripted answer.
#[derive(Debug, Clone)]
pub enum Step {
    Event(What),
    /// The answer goes on after this long (the test's clock).
    Pause(Duration),
}

impl Step {
    pub fn say(text: &str) -> Step {
        Step::Event(What::Say { text: text.to_string() })
    }

    pub fn end(outcome: Outcome) -> Step {
        Step::Event(What::End { outcome, reason: None, said: String::new(), ms: 0, error: None })
    }

    pub fn ask(question: &str) -> Step {
        Step::Event(What::Ask { question: question.to_string(), text: None, expects: super::Expects::Free })
    }
}

type Answer = dyn Fn(&Phrase) -> Vec<Step> + Send + Sync;

/// The scripted Thinking.
pub struct Script {
    answer: Box<Answer>,
    /// The agent its events speak as.
    pub speaker: Option<String>,
    phrases: Mutex<Vec<Phrase>>,
    news: Mutex<Vec<News>>,
    stopped: Arc<Mutex<HashSet<String>>>,
    listeners: Mutex<Vec<mpsc::UnboundedSender<Outside>>>,
}

impl Script {
    /// Every phrase is answered by `answer`'s steps.
    pub fn answering(answer: impl Fn(&Phrase) -> Vec<Step> + Send + Sync + 'static) -> Script {
        Script {
            answer: Box::new(answer),
            speaker: None,
            phrases: Mutex::new(Vec::new()),
            news: Mutex::new(Vec::new()),
            stopped: Arc::new(Mutex::new(HashSet::new())),
            listeners: Mutex::new(Vec::new()),
        }
    }

    /// Every phrase is answered with these words, then `end`.
    pub fn saying(text: &'static str) -> Script {
        Script::answering(move |_| vec![Step::say(text), Step::end(Outcome::Answered)])
    }

    /// The phrases the audio side sent, in order.
    pub fn phrases(&self) -> Vec<Phrase> {
        self.phrases.lock().unwrap().clone()
    }

    /// The news the audio side sent, in order.
    pub fn news(&self) -> Vec<News> {
        self.news.lock().unwrap().clone()
    }

    /// Sends a message outside a turn to every listener.
    pub fn notice(&self, outside: Outside) {
        self.listeners.lock().unwrap().retain(|listener| listener.unbounded_send(outside.clone()).is_ok());
    }
}

impl Thinking for Script {
    fn phrase(&self, phrase: Phrase) -> Events {
        self.phrases.lock().unwrap().push(phrase.clone());
        let steps = (self.answer)(&phrase);
        let (turn, speaker, stopped) = (phrase.turn.clone(), self.speaker.clone(), self.stopped.clone());
        futures::stream::unfold(steps.into_iter(), move |mut steps| {
            let (turn, speaker, stopped) = (turn.clone(), speaker.clone(), stopped.clone());
            async move {
                loop {
                    if stopped.lock().unwrap().contains(&turn) {
                        return None;
                    }
                    match steps.next()? {
                        Step::Pause(pause) => tokio::time::sleep(pause).await,
                        Step::Event(what) => return Some((Event::new(&turn, speaker.as_deref(), what), steps)),
                    }
                }
            }
        })
        .boxed()
    }

    fn tell(&self, news: News) {
        if let Told::Stop { turn, .. } = &news.what {
            self.stopped.lock().unwrap().insert(turn.clone());
        }
        self.news.lock().unwrap().push(news);
    }

    fn notices(&self) -> Notices {
        let (sender, receiver) = mpsc::unbounded();
        self.listeners.lock().unwrap().push(sender);
        receiver.boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::door::{Budget, Context, Kind, Opened, Place, StopWhy, To, Unasked, Who, V};

    fn phrase(turn: &str, text: &str) -> Phrase {
        Phrase {
            v: V,
            turn: turn.into(),
            text: text.into(),
            lang: None,
            alts: Vec::new(),
            who: Who::default(),
            to: To::default(),
            place: Place { device: "computer".into(), room: None, kind: Kind::Computer, screen: true, speaker: true },
            hands: Vec::new(),
            opened: Opened::Button,
            context: Context::default(),
            budget: Budget { voice: true, first_word_ms: Some(1500), limit_ms: None },
            heard: None,
            at: None,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_script_answers_in_order_and_a_stop_ends_its_answer() {
        let script = Script::answering(|phrase| {
            vec![
                Step::say(&format!("Вы сказали: {}", phrase.text)),
                Step::Pause(Duration::from_secs(5)),
                Step::say("И ещё."),
            ]
        });
        let mut events = script.phrase(phrase("t-1", "привет"));
        assert_eq!(events.next().await.unwrap().what, What::Say { text: "Вы сказали: привет".into() });
        script.tell(News::new(Told::Stop { turn: "t-1".into(), why: StopWhy::Word, heard: None }));
        assert!(events.next().await.is_none(), "the rest of a stopped answer is not said");
        assert_eq!(script.phrases().len(), 1);
        assert_eq!(script.news().len(), 1);
    }

    #[tokio::test]
    async fn every_listener_hears_a_notice() {
        let script = Script::saying("Да.");
        let (mut one, mut two) = (script.notices(), script.notices());
        script.notice(Outside::new(Unasked::Withdraw { notice: "n-1".into() }));
        assert_eq!(one.next().await.unwrap().what, Unasked::Withdraw { notice: "n-1".into() });
        assert_eq!(two.next().await.unwrap().what, Unasked::Withdraw { notice: "n-1".into() });
    }
}
