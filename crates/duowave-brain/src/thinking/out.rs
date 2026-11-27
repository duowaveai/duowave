//! What a turn says through the door (the contract's «Messages»): every event carries the turn and the agent speaking
//! (`as`), in the order it is to be said, the last one `end`. `Out` sends them, and keeps what the turn's line needs:
//! when the first word was ready, what was said, the `wait`s sent.
//!
//! Today's pieces of the talk layer (the closed window, the layer's rules) answer in the talk's events: `forward`
//! turns them into the door's, as `TalkThinking` does (talk/thinking.rs), so both give the same events for the same
//! talk.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures::channel::mpsc;
use tokio_util::sync::CancellationToken;

use crate::door::{Event, Expects, Failure, Outcome, WaitWhy, What};
use crate::talk::{Parts, Talk, TalkEvent, Why};

/// How a turn ended, for its line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub outcome: Outcome,
    pub said: String,
    /// The work the turn began or touched.
    pub work: Option<String>,
    pub error: Option<String>,
}

#[derive(Default)]
struct Kept {
    /// The journal's request of the turn, once `begun` named it.
    request: Option<String>,
    first_word_ms: Option<u64>,
    said: Vec<String>,
    waits: Vec<(WaitWhy, Option<u64>, u64)>,
    ended: bool,
}

/// The door's side of one turn.
#[derive(Clone)]
pub struct Out {
    sender: mpsc::UnboundedSender<Event>,
    turn: String,
    speaker: Option<String>,
    started: Instant,
    kept: Arc<Mutex<Kept>>,
    /// Cancelled when the audio side drops the answer: nobody listens any more.
    gone: CancellationToken,
}

impl Out {
    pub fn new(
        sender: mpsc::UnboundedSender<Event>,
        turn: &str,
        speaker: Option<String>,
        gone: CancellationToken,
    ) -> Out {
        Out {
            sender,
            turn: turn.to_string(),
            speaker,
            started: Instant::now(),
            kept: Arc::new(Mutex::new(Kept::default())),
            gone,
        }
    }

    fn kept(&self) -> std::sync::MutexGuard<'_, Kept> {
        self.kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn speaker(&self) -> Option<&str> {
        self.speaker.as_deref()
    }

    pub fn turn(&self) -> &str {
        &self.turn
    }

    /// Milliseconds since the phrase came.
    pub fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// One event; false when the audio side dropped the answer (the turn is cancelled then).
    pub fn send(&self, what: What) -> bool {
        if self.kept().ended {
            return false;
        }
        if let What::End { .. } = what {
            self.kept().ended = true;
        }
        let event = Event::new(&self.turn, self.speaker.as_deref(), what);
        if self.sender.unbounded_send(event).is_err() {
            self.gone.cancel();
            return false;
        }
        true
    }

    pub fn begun(&self, request: &str, by: &str) {
        self.kept().request = Some(request.to_string());
        self.send(What::Begun { request: request.to_string(), by: by.to_string() });
    }

    /// The journal's request of the turn.
    pub fn request(&self) -> Option<String> {
        self.kept().request.clone()
    }

    pub fn say(&self, text: &str) {
        {
            let mut kept = self.kept();
            if kept.first_word_ms.is_none() {
                kept.first_word_ms = Some(self.started.elapsed().as_millis() as u64);
            }
            kept.said.push(text.to_string());
        }
        self.send(What::Say { text: text.to_string() });
    }

    pub fn wait(&self, why: WaitWhy, expect_ms: Option<u64>, name: Option<String>) {
        let at = self.elapsed_ms();
        self.kept().waits.push((why, expect_ms, at));
        self.send(What::Wait { why, expect_ms, name });
    }

    pub fn ask(&self, question: &str, text: Option<String>, expects: Expects) {
        self.send(What::Ask { question: question.to_string(), text, expects });
    }

    pub fn end(&self, outcome: Outcome, reason: Option<Failure>, error: Option<String>) {
        self.end_said(outcome, reason, self.said(), error);
    }

    /// The end, with what the turn's own words were (a reminder said after them is not among them).
    pub fn end_said(&self, outcome: Outcome, reason: Option<Failure>, said: String, error: Option<String>) {
        let ms = self.elapsed_ms();
        self.send(What::End { outcome, reason, said, ms, error });
    }

    /// Whether anything has been said yet.
    pub fn has_said(&self) -> bool {
        !self.kept().said.is_empty()
    }

    pub fn said(&self) -> String {
        self.kept().said.join(" ")
    }

    pub fn first_word_ms(&self) -> Option<u64> {
        self.kept().first_word_ms
    }

    /// The `wait`s sent: why, the time expected, when.
    pub fn waits(&self) -> Vec<(WaitWhy, Option<u64>, u64)> {
        self.kept().waits.clone()
    }

    pub fn ended(&self) -> bool {
        self.kept().ended
    }

    /// The audio side dropped the answer.
    pub fn gone(&self) -> &CancellationToken {
        &self.gone
    }
}

/// A question's id says why the window opens: the audio side gives it back as the reply's `answer_to`.
pub fn ask_and_end(out: &Out, why: Option<Why>, expects: Expects, said: String) -> Outcome {
    match why {
        Some(why) => {
            out.ask(why.as_str(), None, expects);
            out.end_said(Outcome::Asked, None, said, None);
            Outcome::Asked
        }
        None => {
            out.end_said(Outcome::Answered, None, said, None);
            Outcome::Answered
        }
    }
}

/// The work's title and state, for the door's `work`.
pub fn work_of(parts: &Parts, id: &str) -> What {
    let work = parts.talk.works.get(id);
    What::Work {
        work: id.to_string(),
        title: work.as_ref().map(|work| work.title.clone()),
        state: work.as_ref().map(|work| work.state.as_str().to_string()),
    }
}

/// A talk of today's pieces, said through the door: its events in the door's words, to the end. `cancel` stops it.
pub async fn forward(parts: &Parts, device: &str, mut talk: Talk, out: &Out, cancel: &CancellationToken) -> Ended {
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => {
                talk.cancel.cancel();
                return Ended { outcome: Outcome::Other, said: out.said(), work: None, error: None };
            }
            _ = out.gone().cancelled() => {
                talk.cancel.cancel();
                return Ended { outcome: Outcome::Other, said: out.said(), work: None, error: None };
            }
            next = talk.events.recv() => next,
        };
        let Some(next) = next else {
            out.end(Outcome::Answered, None, None);
            return Ended { outcome: Outcome::Answered, said: out.said(), work: None, error: None };
        };
        match next {
            TalkEvent::Start { request, by } => out.begun(&request, by),
            TalkEvent::Phrase { text } => out.say(&text),
            TalkEvent::Tool { name } => out.wait(WaitWhy::Tool, None, Some(name)),
            TalkEvent::Done { said, work, asks, remind, dropped, .. } => {
                if let Some(id) = &work {
                    out.send(work_of(parts, id));
                }
                if dropped {
                    out.end_said(Outcome::Dropped, None, said.clone(), None);
                    return Ended { outcome: Outcome::Dropped, said, work, error: None };
                }
                let why = if remind.is_some() {
                    Some(Why::Ready)
                } else if asks {
                    Some(Why::Question)
                } else {
                    None
                };
                if let Some(remind) = remind {
                    out.say(&remind);
                }
                let outcome = ask_and_end(out, why, Expects::Free, said.clone());
                return Ended { outcome, said, work, error: None };
            }
            TalkEvent::Error { message } => {
                tracing::info!(target: "thinking", "an answer on {device} broke off: {message}");
                let pack = crate::talk::pack::of(parts.talk.lang(device));
                out.say(pack.failed);
                out.end_said(Outcome::Failed, Some(Failure::Error), String::new(), Some(message.clone()));
                return Ended { outcome: Outcome::Failed, said: String::new(), work: None, error: Some(message) };
            }
        }
    }
}
