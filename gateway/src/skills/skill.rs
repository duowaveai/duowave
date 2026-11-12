//! What a skill is: the context of the request it serves, what it does with it, and the interface every one of our
//! implementations of a capability has.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::capabilities::{CapabilityName, Intent};
use crate::gateway::protocol::protocol_types::{DeviceState, Directive};

/// Speech that comes phrase by phrase (a tale the model tells, a game's move); it may fail on the way.
pub type PhraseStream = BoxStream<'static, anyhow::Result<String>>;

/// Called once with the moment the station stops speaking an answer (ms since the epoch, from the audio it was sent):
/// the turn timer counts from there (plugins/skill.rs).
pub type SpokenHook = Box<dyn FnOnce(u64) + Send>;

/// What an executor knows about the request it serves.
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// The device, as the journal names it: per-device state (timers, the player) is kept under it.
    pub device_tag: String,
    /// The request's text after the wake word.
    pub text: String,
    pub timezone: Option<String>,
    pub device_state: Option<DeviceState>,
    /// Where the station fetches our /media: `http://<the host it connects to>/media`.
    pub media_base_url: Option<String>,
    pub now: DateTime<Utc>,
    /// The request is over (cancelled, a newer one came): lookups and the model stop.
    pub cancel: CancellationToken,
}

impl ExecutionContext {
    /// A context for a device now, nothing else known.
    pub fn new(device_tag: &str, now: DateTime<Utc>) -> ExecutionContext {
        ExecutionContext {
            device_tag: device_tag.to_string(),
            text: String::new(),
            timezone: None,
            device_state: None,
            media_base_url: None,
            now,
            cancel: CancellationToken::new(),
        }
    }
}

/// Follow-ups without the wake word go back to the executor until it lets go (a game).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContinuationRequest {
    pub capability: CapabilityName,
    pub ttl_ms: u64,
}

/// What our implementation does with a request.
#[derive(Default)]
pub struct Outcome {
    /// Speech, said in Alice's voice.
    pub speech: Option<String>,
    /// Speech that comes phrase by phrase (a tale the model tells).
    pub phrases: Option<PhraseStream>,
    /// Station commands in the response.
    pub directives: Vec<Directive>,
    /// The station listens after the answer: the conversation goes on (a game, a question).
    pub listen: bool,
    /// For the model: a lookup's result it puts into words instead of `speech` (weather, smart home state).
    pub data: Option<String>,
    /// Follow-ups without the wake word go back to this executor until it lets go (a game).
    pub continuation: Option<ContinuationRequest>,
    /// Keep the spoken audio in this file (a told tale, to play again).
    pub record_audio: Option<PathBuf>,
    /// For the model's history: what was really done behind a short `speech` («Передала.»), so the model stands
    /// by it (live 02.11: it told the user its «Передала» were wrong and nothing was sent).
    pub done: Option<String>,
    /// The answer's speech has ended on the station: called with when. An answer not spoken to its end (superseded,
    /// cancelled, deferred) never calls it.
    pub on_spoken: Option<SpokenHook>,
}

impl Outcome {
    pub fn speech(text: impl Into<String>) -> Outcome {
        Outcome { speech: Some(text.into()), ..Outcome::default() }
    }

    pub fn directives(directives: Vec<Directive>) -> Outcome {
        Outcome { directives, ..Outcome::default() }
    }

    /// Speech with station commands.
    pub fn said(text: impl Into<String>, directives: Vec<Directive>) -> Outcome {
        Outcome { speech: Some(text.into()), directives, ..Outcome::default() }
    }

    /// Nothing to say and nothing to do (a stop with nothing playing).
    pub fn is_empty(&self) -> bool {
        self.speech.is_none()
            && self.phrases.is_none()
            && self.directives.is_empty()
            && !self.listen
            && self.data.is_none()
            && self.continuation.is_none()
            && self.record_audio.is_none()
            && self.done.is_none()
    }
}

impl std::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Outcome")
            .field("speech", &self.speech)
            .field("phrases", &self.phrases.as_ref().map(|_| "…"))
            .field("directives", &self.directives.iter().map(|d| d.name.as_str()).collect::<Vec<_>>())
            .field("listen", &self.listen)
            .field("data", &self.data)
            .field("continuation", &self.continuation)
            .field("record_audio", &self.record_audio)
            .field("done", &self.done)
            .field("on_spoken", &self.on_spoken.as_ref().map(|_| "…"))
            .finish()
    }
}

/// Our implementation of a capability.
pub trait OwnSkill: Send + Sync {
    fn capability(&self) -> CapabilityName;
    /// Whether the commands this intent needs are confirmed on our station. An unconfirmed one (known only from
    /// the wiki) is used only when its route has no Yandex to fall back to.
    fn verified(&self, intent: &Intent) -> bool;
    /// Whether it can work now: a smart home system configured, a content source with something in it.
    fn available(&self) -> bool {
        true
    }
    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>>;
}

/// An intent with parameters; a `None` value is left out, as an undefined property is.
pub fn intent(capability: CapabilityName, action: &str, params: Vec<(&str, Option<Value>)>) -> Intent {
    let mut map = Map::new();
    for (key, value) in params {
        if let Some(value) = value {
            map.insert(key.to_string(), value);
        }
    }
    Intent { capability, action: action.to_string(), params: map }
}

/// A string parameter, or none.
pub fn text_value(text: Option<&str>) -> Option<Value> {
    text.map(|text| Value::String(text.to_string()))
}
