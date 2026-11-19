//! The closed window (docs/plans/audio-interface-2.md, «К2»): a phrase the microphone heard by itself — after our
//! question or a work's line, without the button or the wake word — is taken only as one of four things:
//!
//! 1. the answer to the layer's question on the device — a work's or MagicPlace's (the pack, then the reader with
//!    the answers' acts only);
//! 2. a word that closes the window, the whole phrase («нет», «хватит», «спасибо»; words.rs): the question is
//!    declined, nothing is said;
//! 3. the answer to the brain's own question: the brain's, as before;
//! 4. about the work in focus: «и ещё…», «как там», «подробнее» (the rules), or the reader with the acts of a work
//!    going on — never a new thing, the door or conversation.
//!
//! Anything else is not taken: no request of the journal, nothing in the history, no model — the panel hears
//! `dropped`. Only a phrase after the button or the wake word may begin a new work, remember, or act outside the home.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::asked::Kind;
use super::reader::Act;
use super::rules::{Heard, TalkRules};
use super::works::{Focus, Outcome};
use super::{Activator, Talk, TalkEvent, Turn};
use crate::app::App;
use crate::config::ModelProfile;
use crate::core::identity::Principal;
use crate::core::requests::{Place, RequestPhase};
use crate::dialog::DialogContext;
use crate::utils::time::now_ms;

/// What the reader may read a phrase into after the layer's question: its answer, or the work's own words.
const ANSWER_ACTS: &[Act] = &[Act::Answer, Act::Add, Act::Status, Act::Details, Act::Show, Act::Desk, Act::Stop];
/// What the reader may read a phrase into about the work in focus.
const WORK_ACTS: &[Act] = &[Act::Add, Act::Status, Act::Details, Act::Stop];

/// What the window does with a phrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sift {
    /// A word that closes the window: the question is declined.
    Close,
    /// The layer's rules took it.
    Rules(Heard),
    /// The reader may take it, with these acts only.
    Read(&'static [Act]),
    /// The brain asked: the phrase is its answer.
    Brain,
    /// Not taken.
    Drop,
}

/// The rules' words that stay the window's about the work in focus.
fn of_the_work(heard: &Heard) -> bool {
    matches!(heard, Heard::Add { .. } | Heard::Status { .. } | Heard::Details)
}

/// What the window does with a phrase heard without the button on a device.
pub fn sift(app: &App, text: &str, device: &str) -> Sift {
    let layer = &app.talk;
    let rules = TalkRules::new(layer);
    let normalized = crate::skills::text::normalize(text);
    let reads = layer.reader().is_some_and(|reader| reader.ready());
    // «Нет, не надо» to «Что-то ещё добавить?» closes the window silently: nothing to add, the work goes on (20.11).
    if layer.asked.open(device).is_some_and(|question| question.kind == Kind::Extra)
        && super::ready::extra_answer(super::pack::of(layer.lang(device)), &normalized)
            == Some(super::ready::Answer::No)
    {
        return Sift::Close;
    }
    // The answer to the device's question first — a work's, else MagicPlace's («Поставить «Ленту»?»): «нет» to it is
    // its answer; to «Что-то ещё добавить?» it closes the window.
    if let Some(heard) = rules.answer_on(&normalized, device) {
        return Sift::Rules(heard);
    }
    if super::words::household().closes(text) {
        return Sift::Close;
    }
    if layer.asked.open(device).is_some() {
        if let Some(heard) = rules.hear(&normalized, device).filter(of_the_work) {
            return Sift::Rules(heard);
        }
        return if reads { Sift::Read(ANSWER_ACTS) } else { Sift::Drop };
    }
    if layer.asked.open_any(device).is_some_and(|question| question.kind == Kind::Talk) {
        return Sift::Brain;
    }
    if layer.works.focus(device, now_ms()) == Focus::None {
        return Sift::Drop;
    }
    if let Some(heard) = rules.hear(&normalized, device).filter(of_the_work) {
        return Sift::Rules(heard);
    }
    if reads {
        Sift::Read(WORK_ACTS)
    } else {
        Sift::Drop
    }
}

/// A phrase of the window: done here, or None — not the window's (said after the button), or the brain's answer.
pub fn begin(app: &Arc<App>, principal: &Principal, turn: &Turn) -> Option<Talk> {
    if turn.origin.activator != Activator::Answer {
        return None;
    }
    let device = turn.origin.device.clone();
    match sift(app, &turn.text, &device) {
        Sift::Brain => None,
        Sift::Rules(heard) => Some(super::by_heard(app, principal, turn, TalkRules::new(&app.talk), heard)),
        Sift::Close => {
            if let Some(question) = app.talk.asked.take(&device) {
                app.talk.ended_asking(&question.work, Outcome::Declined);
            }
            tracing::info!(target: "talk", "the window on {device} was closed by a word");
            Some(nothing(false))
        }
        Sift::Drop => {
            tracing::info!(target: "talk", "the window on {device} did not take a phrase");
            Some(nothing(true))
        }
        Sift::Read(acts) => Some(read(app, principal, turn, acts)),
    }
}

/// An answer with nothing in it: the window closed by a word, or a phrase not taken.
fn nothing(dropped: bool) -> Talk {
    let (events, receiver) = mpsc::unbounded_channel();
    let _ = events.send(TalkEvent::Done {
        said: String::new(),
        by: "home",
        ms: 0,
        work: None,
        asks: false,
        remind: None,
        dropped,
    });
    Talk { events: receiver, cancel: CancellationToken::new() }
}

/// The reader reads the phrase with the window's acts only; taken — the layer does and says it, and only then the
/// journal has it. Not taken — `dropped`.
fn read(app: &Arc<App>, principal: &Principal, turn: &Turn, acts: &'static [Act]) -> Talk {
    let (events, receiver) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let (app, turn, answering) = (app.clone(), turn.clone(), cancel.clone());
    let key = format!("panel:{}", principal.id);
    let origin = turn.origin.clone().by(Some(principal.name.clone()));
    tokio::spawn(async move {
        let started = Instant::now();
        let since = now_ms();
        let context = DialogContext {
            profile: Some(ModelProfile::Fast),
            channel: crate::dialog::Channel::Computer,
            max_tokens: Some(crate::dialog::SPEECH_MAX_TOKENS),
            ..DialogContext::default()
        };
        let reader = app.talk.reader();
        let read = match reader {
            Some(reader) => tokio::select! {
                _ = answering.cancelled() => return,
                read = reader.read_only(&key, origin.clone(), &turn.text, context, answering.clone(), Some(acts)) => read,
            },
            None => None,
        };
        let Some(read) = read else {
            tracing::info!(target: "talk", "the window on {} did not take a phrase the reader read", turn.origin.device);
            let _ = events.send(TalkEvent::Done {
                said: String::new(),
                by: "home",
                ms: 0,
                work: None,
                asks: false,
                remind: None,
                dropped: true,
            });
            return;
        };
        let trip = super::journal(&app, &turn, &origin, "plugins", "talk:window");
        trip.step(RequestPhase::Tool, Some(Place::Own), Some("talk:read"));
        let _ = events.send(TalkEvent::Start { request: trip.id(), by: "home" });
        super::spoken(&app, &trip, turn.voice, &read.said);
        let _ = events.send(TalkEvent::Phrase { text: read.said.clone() });
        let work = app.talk.touched_since(&turn.origin.device, since);
        let ms = started.elapsed().as_millis() as u64;
        let (asks, remind) = super::closing(&app, &turn.origin.device, &read.said, turn.voice, false);
        let _ = events.send(TalkEvent::Done { said: read.said, by: "home", ms, work, asks, remind, dropped: false });
    });
    Talk { events: receiver, cancel }
}
