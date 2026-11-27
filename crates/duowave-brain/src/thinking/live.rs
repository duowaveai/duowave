//! Live voice (docs/plans/thinking.md, §12; the owner, 26.11: live voice will come into use more and more, and our
//! system must be its amplifier, not a second mind): `LiveThinking`, one more Thinking behind the door, with the same
//! agent's persona, memory and card.
//!
//! - **The card is the session's instructions**: the agent's prompt, the memory's wake-up, «how to ask for more» and
//!   the catalog; its tools are the card's.
//! - **The rounds are the model's tool calls**: `open`, `recall`, the card's tools, a work's start, the place's hands
//!   — run as the pipeline runs them (the card's toolbox); a provider that can add tools to a running session takes
//!   an opened group as tools, one that cannot gets them as text (the delivery `use`).
//! - **Hands, works and notices** go through the door as `hands`, `work` and `notice`, as the pipeline sends them.
//! - **The transcript** of each exchange goes to the history of the place, so the pipeline goes on with it.
//! - **The fallback is the pipeline**: no network, a limit, a broken session — the phrase goes to ours, with the same
//!   history.
//!
//! The provider stands behind `LiveProvider`; the audio side's pipe and the contract's `live` message are not built
//! yet, so the provider here takes the person's words as text. A scripted provider tests it.

use std::sync::{Arc, Mutex};

use futures::stream::BoxStream;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::dialog::llm::{LlmTool, ToolCall};
use crate::door::{self, Event, Events, News, Notices, Outcome, Phrase, What};

use super::OurThinking;

/// What a live model says or does.
#[derive(Debug, Clone, PartialEq)]
pub enum LiveEvent {
    Text(String),
    Call(ToolCall),
    /// The model's reply is over.
    Done,
}

/// A running live session.
pub trait LiveSession: Send + Sync {
    /// The person's words into the session.
    fn send(&self, text: &str);
    /// A tool's result for the model.
    fn result(&self, call: &str, content: &str);
    /// Tools added to the running session; false — this provider cannot (the opened tools then come as text).
    fn add_tools(&self, tools: &[LlmTool]) -> bool;
    /// The model's events of its reply; an error ends the session.
    fn events(&self) -> BoxStream<'static, anyhow::Result<LiveEvent>>;
}

/// A live model's provider.
pub trait LiveProvider: Send + Sync {
    fn open(&self, instructions: &str, tools: &[LlmTool]) -> anyhow::Result<Arc<dyn LiveSession>>;
}

/// The live model behind the door, the pipeline behind it.
pub struct LiveThinking {
    ours: Arc<OurThinking>,
    provider: Arc<dyn LiveProvider>,
    /// The session running, shared with the turns: one that broke is let go there.
    session: Arc<Mutex<Option<Arc<dyn LiveSession>>>>,
}

impl LiveThinking {
    pub fn new(ours: Arc<OurThinking>, provider: Arc<dyn LiveProvider>) -> LiveThinking {
        LiveThinking { ours, provider, session: Arc::new(Mutex::new(None)) }
    }

    fn session(&self) -> Option<Arc<dyn LiveSession>> {
        self.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    fn set(&self, session: Option<Arc<dyn LiveSession>>) {
        *self.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = session;
    }
}

impl door::Thinking for LiveThinking {
    fn phrase(&self, phrase: Phrase) -> Events {
        let made = self.ours.live_card(&phrase);
        let session = match self.session() {
            Some(session) => Some(session),
            None => match self.provider.open(&made.instructions, &made.tools) {
                Ok(session) => {
                    self.set(Some(session.clone()));
                    Some(session)
                }
                Err(error) => {
                    tracing::info!(target: "thinking", "the live session did not open ({error}): the pipeline answers");
                    None
                }
            },
        };
        let Some(session) = session else { return door::Thinking::phrase(&*self.ours, phrase) };
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        let (ours, turn, broken) = (self.ours.clone(), phrase.turn.clone(), self.session.clone());
        tokio::spawn(async move {
            let send = |what: What| sender.unbounded_send(Event::new(&turn, made.speaker.as_deref(), what)).is_ok();
            session.send(&phrase.text);
            let mut said: Vec<String> = Vec::new();
            let mut events = session.events();
            loop {
                match events.next().await {
                    Some(Ok(LiveEvent::Text(text))) => {
                        said.push(text.clone());
                        send(What::Say { text });
                    }
                    Some(Ok(LiveEvent::Call(call))) => {
                        let outcome = (made.run)(call.clone(), CancellationToken::new()).await;
                        let outcome = outcome
                            .unwrap_or_else(|error| crate::dialog::ToolOutcome::error(format!("error: {error}")));
                        // An opened group's tools go into the running session, or as text where it cannot.
                        if !outcome.opens.is_empty() && !session.add_tools(&outcome.opens) {
                            let described: Vec<String> = outcome
                                .opens
                                .iter()
                                .map(|tool| format!("{}: {}", tool.name, tool.description))
                                .collect();
                            session.result(&call.id, &format!("{}\n{}", outcome.content, described.join("\n")));
                        } else {
                            session.result(&call.id, &outcome.content);
                        }
                        // The hands and works the call sent go through the door as the pipeline's do.
                        for what in made.drain() {
                            send(what);
                        }
                    }
                    Some(Ok(LiveEvent::Done)) | None => {
                        let text = said.join(" ");
                        ours.remember_live(&phrase, &text);
                        send(What::End { outcome: Outcome::Answered, reason: None, said: text, ms: 0, error: None });
                        return;
                    }
                    Some(Err(error)) => {
                        // The session broke: the pipeline goes on with the same phrase and history.
                        tracing::info!(target: "thinking", "the live session broke ({error}): the pipeline answers");
                        *broken.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
                        let mut rest = door::Thinking::phrase(&*ours, phrase.clone());
                        while let Some(event) = rest.next().await {
                            if sender.unbounded_send(event).is_err() {
                                return;
                            }
                        }
                        return;
                    }
                }
            }
        });
        receiver.boxed()
    }

    fn tell(&self, news: News) {
        door::Thinking::tell(&*self.ours, news);
    }

    fn notices(&self) -> Notices {
        door::Thinking::notices(&*self.ours)
    }
}

/// The card of a live session: its instructions, its tools, how a call runs, and the door's events the calls sent.
pub struct LiveCard {
    pub instructions: String,
    pub tools: Vec<LlmTool>,
    pub run: crate::dialog::ToolRun,
    pub speaker: Option<String>,
    /// The card's own `Out`: the hands and works its calls send come here.
    events: Mutex<Option<futures::channel::mpsc::UnboundedReceiver<Event>>>,
}

impl LiveCard {
    pub fn new(
        instructions: String,
        tools: Vec<LlmTool>,
        run: crate::dialog::ToolRun,
        speaker: Option<String>,
        events: futures::channel::mpsc::UnboundedReceiver<Event>,
    ) -> LiveCard {
        LiveCard { instructions, tools, run, speaker, events: Mutex::new(Some(events)) }
    }

    /// The door's events the calls sent so far: hands, works, waits.
    fn drain(&self) -> Vec<What> {
        let mut events = self.events.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(receiver) = events.as_mut() else { return Vec::new() };
        let mut found = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if matches!(event.what, What::Hands { .. } | What::Work { .. } | What::Wait { .. }) {
                found.push(event.what);
            }
        }
        found
    }
}
