//! The talk without a station (docs/plans/mic-mode.md): a phrase — said after the panel's «Сказать» or typed — goes
//! to the brain with the household's character, its memory and the tools of a conversation, and comes back in
//! phrases the computer says aloud. The same brain and the same history as the panel's check chat
//! (`panel:<person>`): one talk, by voice and in text. Every phrase is a request of the journal with the device
//! `computer` and its steps — the ear, the brain, the tools, the voice — so the panel draws its trip as a station's.
//!
//! «Как говорить с DuoWave» and the lock on «Своё» are input.rs; the routes of all of it routes.rs. The audio
//! interface's first stage builds on this layer (docs/plans/audio-interface.md, «Слой разговора»): `TalkLayer`
//! (layer.rs) keeps the records of works (works.rs) its executors report to (executor.rs; the workshop —
//! workshop.rs) and tells the brain what goes on (now.rs).

pub mod asked;
pub mod door;
pub mod executor;
pub mod folder;
pub mod input;
pub mod layer;
pub mod now;
pub mod origin;
pub mod pack;
pub mod reader;
pub mod ready;
pub mod routes;
pub mod rules;
pub mod store;
pub mod task_work;
pub mod thought;
pub mod tool_work;
mod tools;
pub mod window;
pub mod words;
pub mod work_tools;
pub mod works;
pub mod workshop;

use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub use layer::TalkLayer;
pub use origin::{Activator, Origin, Via, Why, COMPUTER};
pub use routes::routes;

use crate::config::ModelProfile;
use crate::core::connectors::ToolContribution;
use crate::core::identity::Principal;
use crate::core::requests::{Place, RequestHandle, RequestKind, RequestPhase, RequestStart, RequestStatus};
use crate::dialog::{DialogContext, DialogService};
use crate::mcp::McpService;
use crate::platform::http::ApiError;
use crate::secrets::SecretsService;
use crate::skills::SkillsService;
use crate::speech::VoiceService;

/// A phrase at most this long: a question, not a document.
pub const MAX_TEXT: usize = 1000;

/// What a talk works with: the layer, the brain, the voice, the secrets (kept out of errors), the skills and their
/// routes, MCP and the pool of models — and, from the floors above (gateway/layers.json), the journal of requests and
/// the connectors' tools, as functions the gateway gives. The gateway makes it (src/links/http.rs); a talk holds it
/// while its answer runs.
#[derive(Clone)]
pub struct Parts {
    pub talk: Arc<TalkLayer>,
    pub dialog: Arc<DialogService>,
    pub voice: Arc<VoiceService>,
    pub secrets: Arc<SecretsService>,
    pub skills: Arc<SkillsService>,
    pub mcp: Arc<McpService>,
    pub router: Arc<crate::router::Router>,
    /// A talk's request in the journal, answered by our side (src/requests).
    pub journal: Journal,
    /// The connectors' tools a person may use (src/connectors).
    pub connector_tools: ConnectorTools,
}

crate::route_needs!(
    Parts,
    Arc<TalkLayer>,
    Arc<crate::router::Router>,
    Arc<crate::dialog::PersonaService>,
    Arc<crate::settings::SettingsService>,
    Arc<crate::placement::PlacementService>,
    Arc<crate::gateway::mode::ModeService>,
);

/// A request's start → its handle in the journal, answered by our side.
pub type Journal = Arc<dyn Fn(RequestStart) -> Arc<RequestHandle> + Send + Sync>;

/// A person → the connectors' tools they may use.
pub type ConnectorTools = Arc<dyn Fn(&Principal) -> Vec<ToolContribution> + Send + Sync>;

pub const NO_MODEL: &str = "Модель не выбрана: добавьте провайдера в «Персонаж» → «Мышление».";

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
        /// The work the phrase began or added to, when it did (docs/plans/audio-interface.md, «Пульт»).
        #[serde(skip_serializing_if = "Option::is_none")]
        work: Option<String>,
        /// The answer asks: the device's question is open, the panel listens for the answer
        /// (docs/plans/audio-interface-2.md, «К3»).
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        asks: bool,
        /// A work's question nobody answered, asked once more after this answer: the panel says it and listens.
        #[serde(skip_serializing_if = "Option::is_none")]
        remind: Option<String>,
        /// Said in the closed window and not taken (window.rs): nobody answers it, nothing kept it.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        dropped: bool,
    },
    Error {
        message: String,
    },
}

/// Whether an answer ends with a question: the microphone listens after it.
pub fn ends_with_question(text: &str) -> bool {
    text.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '»' | '"' | ')' | ']' | '”' | '\''))
        .ends_with(['?', '？'])
}

/// The end of a spoken answer on a device: whether it asks, and the reminder of a work's question nobody answered — once,
/// and never on top of a new question. `brain` — the brain answered: its question is the device's now.
fn closing(parts: &Parts, device: &str, said: &str, voice: bool, brain: bool) -> (bool, Option<String>) {
    let asks = ends_with_question(said);
    if !voice {
        return (asks, None);
    }
    if asks {
        // A background tool's «Запускать?» asked in this answer stays the device's question: «да» runs it.
        let run = parts.talk.asked.open(device).is_some_and(|question| question.kind == asked::Kind::Run);
        if brain && !run {
            parts.talk.asked.ask(device, "", asked::Kind::Talk, false);
        }
        return (true, None);
    }
    (false, parts.talk.remind(device))
}

/// The brain of a talk: where its question goes and which model answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Brain {
    pub by: Place,
    pub what: String,
}

impl Brain {
    /// The fast profile's, as the station's conversation names it.
    fn of(parts: &Parts) -> Brain {
        let brain = parts.dialog.brain();
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

/// A phrase goes to the talk layer's rules first (docs/plans/audio-interface.md, «Порядок разбора»): the answer to
/// the computer's question, the open door, the works' words — what they take never reaches the model. The rest goes
/// to the brain; the answer comes as events. Refused in words when no model is chosen.
pub fn begin(parts: &Parts, principal: &Principal, turn: Turn) -> Result<Talk, ApiError> {
    // A phrase the microphone heard by itself goes through the closed window first: what it does not take, nothing
    // keeps (docs/plans/audio-interface-2.md, «К2»).
    if let Some(talk) = window::begin(parts, principal, &turn) {
        return Ok(talk);
    }
    // The brain's question is answered now — or the person went on with the button: it is not awaited any more.
    let device = &turn.origin.device;
    if parts.talk.asked.open_any(device).is_some_and(|question| question.kind == asked::Kind::Talk) {
        parts.talk.asked.take(device);
    }
    if let Some(talk) = by_rules(parts, principal, &turn) {
        return Ok(talk);
    }
    if !parts.dialog.enabled() {
        return Err(ApiError::conflict(NO_MODEL));
    }
    let key = format!("panel:{}", principal.id);
    if turn.fresh {
        parts.dialog.forget(&key);
    }
    let brain = Brain::of(parts);
    let kind = if turn.origin.via == Via::Voice { RequestKind::Voice } else { RequestKind::Text };
    // «Своё» whatever the mode: a talk without a station never goes to Yandex.
    let trip = (parts.journal)(RequestStart {
        device: turn.origin.device.clone(),
        session: None,
        kind,
        text: Some(turn.text.clone()),
        parent: None,
    });
    if let Some(heard) = &turn.heard {
        trip.heard_before(heard.by, heard.what.as_deref(), heard.ms.unwrap_or(0));
    }
    trip.set_origin(turn.origin.clone().by(Some(principal.name.clone())));
    trip.routed(Some("chat"), "model");
    trip.step(RequestPhase::Think, Some(brain.by), Some(&brain.what));
    let started = Instant::now();
    let since = crate::utils::time::now_ms();
    let (events, receiver) = mpsc::unbounded_channel();
    let _ = events.send(TalkEvent::Start { request: trip.id(), by: brain.place() });
    let cancel = CancellationToken::new();
    let toolbox = tools::toolbox(tools::Deps {
        parts: parts.clone(),
        principal: principal.clone(),
        text: turn.text.clone(),
        origin: turn.origin.clone().by(Some(principal.name.clone())),
        trip: trip.clone(),
        events: events.clone(),
        brain: brain.clone(),
    });
    // The computer's clock: no station says its time zone.
    let context = DialogContext {
        spoken: true,
        profile: Some(ModelProfile::Fast),
        channel: crate::dialog::Channel::Computer,
        // A spoken answer: a few sentences (docs/plans/audio-interface.md, decision 14).
        max_tokens: Some(crate::dialog::SPEECH_MAX_TOKENS),
        ..DialogContext::default()
    };
    // A phrase about the works is read first: the layer does and says it (reader.rs).
    let reader = parts.talk.reader().filter(|reader| reader.wants(&turn.origin.device, &turn.text));
    let origin = turn.origin.clone().by(Some(principal.name.clone()));
    let (parts, answering) = (parts.clone(), cancel.clone());
    tokio::spawn(async move {
        if let Some(reader) = reader {
            let read = tokio::select! {
                _ = answering.cancelled() => {
                    trip.finish(RequestStatus::Cancelled, None, None);
                    return;
                }
                read = reader.read(&key, origin, &turn.text, context.clone(), answering.clone()) => read,
            };
            if let Some(read) = read {
                trip.step(RequestPhase::Tool, Some(Place::Own), Some("talk:read"));
                if turn.voice {
                    let voice = parts.voice.own_choice().map(|choice| choice.voice);
                    trip.step(RequestPhase::Speak, Some(Place::Own), voice.as_deref());
                }
                trip.answered(&read.said);
                trip.finish(RequestStatus::Answered, None, None);
                let _ = events.send(TalkEvent::Phrase { text: read.said.clone() });
                let ms = started.elapsed().as_millis() as u64;
                let work = parts.talk.touched_since(&turn.origin.device, since);
                let (asks, remind) = closing(&parts, &turn.origin.device, &read.said, turn.voice, false);
                let _ = events.send(TalkEvent::Done {
                    said: read.said,
                    by: brain.place(),
                    ms,
                    work,
                    asks,
                    remind,
                    dropped: false,
                });
                return;
            }
        }
        let mut phrases = parts.dialog.answer(&key, &turn.text, context, answering.clone(), toolbox);
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
                        let voice = parts.voice.own_choice().map(|choice| choice.voice);
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
                    let message: String = parts.secrets.redact(&format!("{error:#}")).chars().take(300).collect();
                    trip.finish(RequestStatus::Failed, Some(&message), None);
                    let _ = events.send(TalkEvent::Error { message });
                    return;
                }
                None => {
                    let status = if said.is_empty() { RequestStatus::Silent } else { RequestStatus::Answered };
                    trip.finish(status, None, None);
                    let ms = started.elapsed().as_millis() as u64;
                    let work = parts.talk.touched_since(&turn.origin.device, since);
                    let said = said.join(" ");
                    // A work's line («Делаю: … Что-то ещё добавить?») is the layer's question, not the brain's.
                    let (asks, remind) = closing(&parts, &turn.origin.device, &said, turn.voice, work.is_none());
                    let _ = events.send(TalkEvent::Done {
                        said,
                        by: brain.place(),
                        ms,
                        work,
                        asks,
                        remind,
                        dropped: false,
                    });
                    return;
                }
            }
        }
    });
    Ok(Talk { events: receiver, cancel })
}

/// The phrase as the talk layer's rules take it, without the model: the answer to the computer's question (a phrase
/// said after it, `answer`), the open door, the works' words. None — the brain's.
fn by_rules(parts: &Parts, principal: &Principal, turn: &Turn) -> Option<Talk> {
    let rules = rules::TalkRules::new(&parts.talk);
    let device = turn.origin.device.clone();
    let normalized = crate::skills::text::normalize(&turn.text);
    let answer = (turn.origin.activator == Activator::Answer).then(|| rules.answer_on(&normalized, &device)).flatten();
    let heard =
        answer.or_else(|| rules.through_door(&normalized, &device)).or_else(|| rules.hear(&normalized, &device))?;
    Some(by_heard(parts, principal, turn, rules, heard))
}

/// What the layer's rules heard in a phrase, done and said: a request of the journal, the work's line in the history.
pub(crate) fn by_heard(
    parts: &Parts,
    principal: &Principal,
    turn: &Turn,
    rules: Arc<rules::TalkRules>,
    heard: rules::Heard,
) -> Talk {
    let device = turn.origin.device.clone();
    let origin = turn.origin.clone().by(Some(principal.name.clone()));
    let trip = journal(parts, turn, &origin, "plugins", "talk:works");
    let (events, receiver) = mpsc::unbounded_channel();
    let _ = events.send(TalkEvent::Start { request: trip.id(), by: "home" });
    let cancel = CancellationToken::new();
    let (parts, started, since, voice) = (parts.clone(), Instant::now(), crate::utils::time::now_ms(), turn.voice);
    let (key, text) = (format!("panel:{}", principal.id), turn.text.clone());
    tokio::spawn(async move {
        let said = rules.act(heard, origin).await.unwrap_or_default();
        if said.is_empty() {
            trip.finish(RequestStatus::Silent, None, None);
        } else {
            // The conversation goes on with the brain: it knows what the layer did — the work's line, not the phrase.
            let kept = parts.talk.fact_since(&device, since);
            let remembered = kept.as_deref().unwrap_or(&said);
            parts.dialog.remember(&key, &text, remembered, crate::dialog::AnsweredBy::Skill, None);
            spoken(&parts, &trip, voice, &said);
            let _ = events.send(TalkEvent::Phrase { text: said.clone() });
        }
        let work = parts.talk.touched_since(&device, since);
        let ms = started.elapsed().as_millis() as u64;
        let (asks, remind) =
            if said.is_empty() { (false, None) } else { closing(&parts, &device, &said, voice, false) };
        let _ = events.send(TalkEvent::Done { said, by: "home", ms, work, asks, remind, dropped: false });
    });
    Talk { events: receiver, cancel }
}

/// A request of the journal for a phrase the layer takes: where it was heard, where it came from, which rule.
pub(crate) fn journal(
    parts: &Parts,
    turn: &Turn,
    origin: &Origin,
    route: &str,
    rule: &str,
) -> Arc<crate::core::requests::RequestHandle> {
    let kind = if turn.origin.via == Via::Voice { RequestKind::Voice } else { RequestKind::Text };
    let trip = (parts.journal)(RequestStart {
        device: turn.origin.device.clone(),
        session: None,
        kind,
        text: Some(turn.text.clone()),
        parent: None,
    });
    if let Some(heard) = &turn.heard {
        trip.heard_before(heard.by, heard.what.as_deref(), heard.ms.unwrap_or(0));
    }
    trip.set_origin(origin.clone());
    trip.routed(Some(route), "own");
    trip.step(RequestPhase::Think, Some(Place::Own), Some(rule));
    trip
}

/// The layer's phrase is said: the voice's step when the computer says it, the answer in the journal.
pub(crate) fn spoken(parts: &Parts, trip: &crate::core::requests::RequestHandle, voice: bool, said: &str) {
    if voice {
        let voice = parts.voice.own_choice().map(|choice| choice.voice);
        trip.step(RequestPhase::Speak, Some(Place::Own), voice.as_deref());
    }
    trip.answered(said);
    trip.finish(RequestStatus::Answered, None, None);
}
