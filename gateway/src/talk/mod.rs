//! The talk without a station (docs/plans/mic-mode.md): a phrase — said after the panel's «Сказать» or typed — goes
//! to the brain with the household's character, its memory and the tools of a conversation, and comes back in
//! phrases the computer says aloud. The same brain and the same history as the panel's check chat
//! (`panel:<person>`): one talk, by voice and in text. Every phrase is a request of the journal with the device
//! `computer` and its steps — the ear, the brain, the tools, the voice — so the panel draws its trip as a station's.
//!
//! «Как говорить с DuoWave» and the lock on «Своё» are input.rs; the routes of all of it routes.rs. The audio
//! interface's first stage builds on this layer: the records of works, what the brain knows now, the works' tools
//! (docs/plans/audio-interface.md, «Стык с режимом «Микрофон»»).

pub mod input;
pub mod origin;
pub mod pack;
pub mod routes;
mod tools;

use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub use origin::{Activator, Origin, Via, COMPUTER};
pub use routes::routes;

use crate::app::App;
use crate::config::ModelProfile;
use crate::core::identity::Principal;
use crate::core::requests::{Place, RequestKind, RequestPhase, RequestStart, RequestStatus};
use crate::dialog::DialogContext;
use crate::platform::http::ApiError;

/// A phrase at most this long: a question, not a document.
pub const MAX_TEXT: usize = 1000;

pub const NO_MODEL: &str = "Модель не выбрана: добавьте провайдера в «Персонаж» → «Мозг».";

/// Where the phrase was recognized before it came here (the panel's `POST /api/v1/listen`), for the journal's step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heard {
    pub by: Place,
    pub ms: Option<u64>,
    /// The recognition model, when the panel knows it.
    pub what: Option<String>,
}

/// One phrase of a talk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub text: String,
    pub origin: Origin,
    pub heard: Option<Heard>,
    /// The answer will be said aloud by the computer: the journal gets the voice's step.
    pub voice: bool,
    /// Start the talk over.
    pub fresh: bool,
}

/// One event of a talk's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TalkEvent {
    /// The request of the journal, and where the brain thinks: `home` or `cloud`.
    Start {
        request: String,
        by: &'static str,
    },
    /// A phrase ready to be said.
    Phrase {
        text: String,
    },
    /// A tool works: its name for people.
    Tool {
        name: String,
    },
    Done {
        said: String,
        by: &'static str,
        ms: u64,
    },
    Error {
        message: String,
    },
}

/// The brain of a talk: where its question goes and which model answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Brain {
    pub by: Place,
    pub what: String,
}

impl Brain {
    /// The fast profile's, as the station's conversation names it.
    fn of(app: &App) -> Brain {
        let brain = app.dialog.brain();
        let slot = brain.profiles.fast.as_ref();
        let cloud = slot.map_or(brain.cloud, |slot| slot.cloud);
        let what = slot.map(|slot| slot.model.clone()).unwrap_or_else(|| brain.models.fast.clone());
        Brain { by: if cloud { Place::Cloud } else { Place::Own }, what }
    }

    fn place(&self) -> &'static str {
        if self.by == Place::Cloud {
            "cloud"
        } else {
            "home"
        }
    }
}

/// An answer on its way: its events as they come. Dropping `cancel`'s guard — the panel closed the stream — stops it.
pub struct Talk {
    pub events: mpsc::UnboundedReceiver<TalkEvent>,
    pub cancel: CancellationToken,
}

/// A phrase goes to the brain; the answer comes as events. Refused in words when no model is chosen.
pub fn begin(app: &Arc<App>, principal: &Principal, turn: Turn) -> Result<Talk, ApiError> {
    if !app.dialog.enabled() {
        return Err(ApiError::conflict(NO_MODEL));
    }
    let key = format!("panel:{}", principal.id);
    if turn.fresh {
        app.dialog.forget(&key);
    }
    let brain = Brain::of(app);
    let kind = if turn.origin.via == Via::Voice { RequestKind::Voice } else { RequestKind::Text };
    // «Своё» whatever the mode: a talk without a station never goes to Yandex.
    let trip = app.requests.begin_in(
        RequestStart {
            device: turn.origin.device.clone(),
            session: None,
            kind,
            text: Some(turn.text.clone()),
            parent: None,
        },
        "own",
    );
    if let Some(heard) = &turn.heard {
        trip.heard_before(heard.by, heard.what.as_deref(), heard.ms.unwrap_or(0));
    }
    trip.routed(Some("chat"), "model");
    trip.step(RequestPhase::Think, Some(brain.by), Some(&brain.what));
    let started = Instant::now();
    let (events, receiver) = mpsc::unbounded_channel();
    let _ = events.send(TalkEvent::Start { request: trip.id(), by: brain.place() });
    let cancel = CancellationToken::new();
    let toolbox = tools::toolbox(tools::Deps {
        app: app.clone(),
        principal: principal.clone(),
        text: turn.text.clone(),
        trip: trip.clone(),
        events: events.clone(),
        brain: brain.clone(),
    });
    // The computer's clock: no station says its time zone.
    let context = DialogContext {
        profile: Some(ModelProfile::Fast),
        channel: crate::dialog::Channel::Computer,
        ..DialogContext::default()
    };
    let mut phrases = app.dialog.answer(&key, &turn.text, context, cancel.clone(), toolbox);
    let (app, answering) = (app.clone(), cancel.clone());
    tokio::spawn(async move {
        let mut said: Vec<String> = Vec::new();
        loop {
            let next = tokio::select! {
                _ = answering.cancelled() => {
                    trip.finish(RequestStatus::Cancelled, None, None);
                    return;
                }
                next = phrases.next() => next,
            };
            match next {
                Some(Ok(text)) => {
                    if said.is_empty() && turn.voice {
                        let voice = app.voice.own_choice().map(|choice| choice.voice);
                        trip.step(RequestPhase::Speak, Some(Place::Own), voice.as_deref());
                    }
                    said.push(text.clone());
                    trip.answered(&said.join(" "));
                    let _ = events.send(TalkEvent::Phrase { text });
                }
                Some(Err(error)) => {
                    if answering.is_cancelled() {
                        trip.finish(RequestStatus::Cancelled, None, None);
                        return;
                    }
                    let message: String = app.secrets.redact(&format!("{error:#}")).chars().take(300).collect();
                    trip.finish(RequestStatus::Failed, Some(&message), None);
                    let _ = events.send(TalkEvent::Error { message });
                    return;
                }
                None => {
                    let status = if said.is_empty() { RequestStatus::Silent } else { RequestStatus::Answered };
                    trip.finish(status, None, None);
                    let ms = started.elapsed().as_millis() as u64;
                    let _ = events.send(TalkEvent::Done { said: said.join(" "), by: brain.place(), ms });
                    return;
                }
            }
        }
    });
    Ok(Talk { events: receiver, cancel })
}
