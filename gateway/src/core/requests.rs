//! A request is one thing asked of the household's voice — spoken after the wake word, typed, or said by the gateway
//! itself — from the first sound to the last word of the answer. Its steps say what happened, how long each took and
//! who did it: our models at home, the brain in the cloud, Yandex. The panel draws the trip as it goes and keeps
//! the history (src/requests); scenarios will start from finished requests and chain through `parent`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// What started it: the wake word, a typed text, our own speech nobody asked for (a reminder, the panel's «Сказать»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestKind {
    Voice,
    Text,
    Say,
}

impl RequestKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RequestKind::Voice => "voice",
            RequestKind::Text => "text",
            RequestKind::Say => "say",
        }
    }
}

/// The trip's steps, in their usual order: the ear listens and recognises, the brain thinks and looks things up,
/// the voice speaks; and what the station said of what our player started for it — playing, stopped, silent
/// (docs/plans/plugins.md, stage 5а): a note, not a phase the request goes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestPhase {
    Listen,
    Recognize,
    Think,
    Tool,
    Speak,
    Play,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestStatus {
    Active,
    Answered,
    Silent,
    Cancelled,
    Failed,
}

impl RequestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RequestStatus::Active => "active",
            RequestStatus::Answered => "answered",
            RequestStatus::Silent => "silent",
            RequestStatus::Cancelled => "cancelled",
            RequestStatus::Failed => "failed",
        }
    }
}

/// Who did a step: at home (our models and skills), the cloud (a model outside the house), Yandex, a helper agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    Own,
    Cloud,
    Yandex,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestStep {
    pub phase: RequestPhase,
    /// Milliseconds from the request's start.
    pub at: u64,
    /// How long it took; absent while it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Place>,
    /// The model, the voice or the capability: «gigaam-v3», «claude-haiku-5-5», «Погода».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub what: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestView {
    pub id: String,
    /// The station's tag, as its record names it.
    pub device: String,
    /// The station connection it came over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub kind: RequestKind,
    /// The panel's mode when it started: relay, gateway, own.
    pub mode: String,
    pub status: RequestStatus,
    /// The step it is in, while it goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<RequestPhase>,
    pub started_at: String,
    /// From the start to the end, once it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    /// What was asked: heard, typed, or said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The recognition so far, while it listens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial: Option<String>,
    /// What was answered, as far as it is said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    /// Who answered: our skill (own), the model, Yandex, nobody yet (unsupported).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
    #[serde(default)]
    pub steps: Vec<RequestStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// A chain: the scenario or the request this one follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// What a session gives to start following a request; the service adds the id, the mode and the time.
#[derive(Debug, Clone)]
pub struct RequestStart {
    pub device: String,
    pub session: Option<String>,
    pub kind: RequestKind,
    pub text: Option<String>,
    pub parent: Option<String>,
}

/// Where sessions start requests (the service, with the mode filled in).
pub trait RequestStarter: Send + Sync {
    fn begin(&self, start: RequestStart) -> Arc<RequestHandle>;
}

/// Long texts are cut: the journal keeps what was said, not a novel the model wrote.
const MAX_TEXT: usize = 2000;

fn clip(text: &str) -> String {
    if text.chars().count() > MAX_TEXT {
        let mut cut: String = text.chars().take(MAX_TEXT - 1).collect();
        cut.push('…');
        cut
    } else {
        text.to_string()
    }
}

/// The words of a request are what people said: whoever may not read the journal sees the trip without them.
pub fn without_words(view: &RequestView) -> RequestView {
    RequestView { text: None, partial: None, answer: None, error: None, ..view.clone() }
}

pub trait RequestHooks: Send + Sync {
    /// Something changed: the service publishes it.
    fn changed(&self, handle: &RequestHandle);
    /// The request ended: once.
    fn ended(&self, handle: &RequestHandle);
}

/// Whoever answers a request claims it by a token of its own (`new_owner`).
pub type Owner = u64;

static OWNERS: AtomicU64 = AtomicU64::new(1);

/// A token no other claimant has.
pub fn new_owner() -> Owner {
    OWNERS.fetch_add(1, Ordering::Relaxed)
}

/// Milliseconds since some origin; tests give their own.
pub type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;

pub fn monotonic_clock() -> Clock {
    let origin = Instant::now();
    Arc::new(move || origin.elapsed().as_secs_f64() * 1000.0)
}

struct State {
    view: RequestView,
    owner: Option<Owner>,
}

/// One request as it goes. A new step ends the one before; the end closes the last. Whoever answers claims the
/// request (an answer, Yandex's relay): when the answer hands over to another, only the newer one may end it.
/// Everything after the end is ignored.
pub struct RequestHandle {
    state: Mutex<State>,
    hooks: Arc<dyn RequestHooks>,
    clock: Clock,
    origin: f64,
}

impl RequestHandle {
    pub fn new(view: RequestView, hooks: Arc<dyn RequestHooks>, clock: Clock) -> RequestHandle {
        let origin = clock();
        RequestHandle { state: Mutex::new(State { view, owner: None }), hooks, clock, origin }
    }

    pub fn id(&self) -> String {
        self.state.lock().unwrap().view.id.clone()
    }

    pub fn active(&self) -> bool {
        self.state.lock().unwrap().view.status == RequestStatus::Active
    }

    pub fn phase(&self) -> Option<RequestPhase> {
        self.state.lock().unwrap().view.phase
    }

    pub fn started_at(&self) -> String {
        self.state.lock().unwrap().view.started_at.clone()
    }

    pub fn snapshot(&self) -> RequestView {
        self.state.lock().unwrap().view.clone()
    }

    /// The recognition so far.
    pub fn partial(&self, text: &str) {
        self.change(|state, _| {
            if text.is_empty() || state.view.partial.as_deref() == Some(text) {
                return false;
            }
            state.view.partial = Some(clip(text));
            true
        });
    }

    /// A step begins and the one before ends; the same step again only says more about who does it.
    pub fn step(&self, phase: RequestPhase, by: Option<Place>, what: Option<&str>) {
        self.change(|state, elapsed| {
            let open = state.view.steps.last_mut().filter(|step| step.ms.is_none());
            match open {
                Some(open) if open.phase == phase => {
                    if by.is_some() {
                        open.by = by;
                    }
                    if let Some(what) = what.filter(|w| !w.is_empty()) {
                        open.what = Some(what.to_string());
                    }
                }
                _ => {
                    close_step(&mut state.view, elapsed);
                    state.view.steps.push(RequestStep {
                        phase,
                        at: elapsed,
                        ms: None,
                        by,
                        what: what.filter(|w| !w.is_empty()).map(str::to_string),
                    });
                }
            }
            state.view.phase = Some(phase);
            true
        });
    }

    /// A note of something that happened, without changing the step it is in: what the station said of the player
    /// («колонка начала играть»). It goes before the step still open, which stays the last.
    pub fn note(&self, phase: RequestPhase, by: Option<Place>, what: &str) {
        self.change(|state, elapsed| {
            note_step(&mut state.view, phase, by, what, elapsed);
            true
        });
    }

    /// The phrase was recognized before the request began (the panel's microphone, src/talk): the ear's step, done,
    /// with the time it took there.
    pub fn heard_before(&self, by: Place, what: Option<&str>, ms: u64) {
        self.change(|state, _| {
            let step = RequestStep {
                phase: RequestPhase::Recognize,
                at: 0,
                ms: Some(ms),
                by: Some(by),
                what: what.filter(|what| !what.is_empty()).map(|what| what.chars().take(200).collect()),
            };
            state.view.steps.insert(0, step);
            true
        });
    }

    /// What was asked: the recognition ends, the request waits for its executor.
    pub fn heard(&self, text: &str) {
        self.change(|state, elapsed| {
            close_step(&mut state.view, elapsed);
            state.view.phase = None;
            state.view.partial = None;
            state.view.text = Some(clip(text));
            true
        });
    }

    /// Where the router sent it: the capability and who serves it.
    pub fn routed(&self, capability: Option<&str>, executor: &str) {
        self.change(|state, _| {
            if let Some(capability) = capability {
                state.view.capability = Some(capability.to_string());
            }
            state.view.executor = Some(executor.to_string());
            true
        });
    }

    /// The answer's text as far as it is said.
    pub fn answered(&self, text: &str) {
        let trimmed = text.trim();
        self.change(|state, _| {
            if trimmed.is_empty() || state.view.answer.as_deref() == Some(trimmed) {
                return false;
            }
            state.view.answer = Some(clip(trimmed));
            true
        });
    }

    /// Whoever answers now; only it may end the request from here on.
    pub fn claim(&self, owner: Owner) {
        let mut state = self.state.lock().unwrap();
        if state.view.status == RequestStatus::Active {
            state.owner = Some(owner);
        }
    }

    /// The end: by its last claimant, or by anyone when nobody claimed it or no owner is named (a disconnect).
    pub fn finish(&self, status: RequestStatus, error: Option<&str>, owner: Option<Owner>) {
        let ended = {
            let mut state = self.state.lock().unwrap();
            if state.view.status != RequestStatus::Active || status == RequestStatus::Active {
                return;
            }
            if let (Some(owner), Some(claimant)) = (owner, state.owner) {
                if owner != claimant {
                    return;
                }
            }
            let elapsed = self.elapsed();
            close_step(&mut state.view, elapsed);
            state.view.status = status;
            state.view.phase = None;
            state.view.partial = None;
            state.view.ms = Some(elapsed);
            if let Some(error) = error {
                state.view.error = Some(error.chars().take(300).collect());
            }
            true
        };
        if ended {
            self.hooks.changed(self);
            self.hooks.ended(self);
        }
    }

    fn change(&self, apply: impl FnOnce(&mut State, u64) -> bool) {
        let changed = {
            let mut state = self.state.lock().unwrap();
            if state.view.status != RequestStatus::Active {
                return;
            }
            let elapsed = self.elapsed();
            apply(&mut state, elapsed)
        };
        if changed {
            self.hooks.changed(self);
        }
    }

    fn elapsed(&self) -> u64 {
        ((self.clock)() - self.origin).round().max(0.0) as u64
    }
}

/// A finished step at `elapsed`, before the step still open.
pub fn note_step(view: &mut RequestView, phase: RequestPhase, by: Option<Place>, what: &str, elapsed: u64) {
    let step = RequestStep { phase, at: elapsed, ms: Some(0), by, what: Some(what.chars().take(200).collect()) };
    let open = view.steps.last().is_some_and(|step| step.ms.is_none());
    let at = if open { view.steps.len() - 1 } else { view.steps.len() };
    view.steps.insert(at, step);
}

fn close_step(view: &mut RequestView, elapsed: u64) {
    if let Some(open) = view.steps.last_mut().filter(|step| step.ms.is_none()) {
        open.ms = Some(elapsed.saturating_sub(open.at));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct Hooks {
        changes: Mutex<Vec<RequestView>>,
        ended: AtomicUsize,
    }

    impl RequestHooks for Hooks {
        fn changed(&self, handle: &RequestHandle) {
            self.changes.lock().unwrap().push(handle.snapshot());
        }
        fn ended(&self, _handle: &RequestHandle) {
            self.ended.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct Trip {
        handle: RequestHandle,
        hooks: Arc<Hooks>,
        now: Arc<Mutex<f64>>,
    }

    impl Trip {
        fn at(&self, ms: f64) {
            *self.now.lock().unwrap() = ms;
        }
    }

    fn trip(text: Option<&str>) -> Trip {
        let now = Arc::new(Mutex::new(0.0));
        let clock_now = now.clone();
        let hooks = Arc::new(Hooks::default());
        let view = RequestView {
            id: "r1".into(),
            device: "kitchen".into(),
            session: None,
            kind: RequestKind::Voice,
            mode: "gateway".into(),
            status: RequestStatus::Active,
            phase: None,
            started_at: "2026-11-05T12:00:00.000Z".into(),
            ms: None,
            text: text.map(str::to_string),
            partial: None,
            answer: None,
            capability: None,
            executor: None,
            steps: vec![],
            error: None,
            parent: None,
        };
        let handle = RequestHandle::new(view, hooks.clone(), Arc::new(move || *clock_now.lock().unwrap()));
        Trip { handle, hooks, now }
    }

    fn step(phase: RequestPhase, at: u64, ms: Option<u64>, by: Option<Place>, what: Option<&str>) -> RequestStep {
        RequestStep { phase, at, ms, by, what: what.map(str::to_string) }
    }

    #[test]
    fn follows_the_trip_step_by_step() {
        let t = trip(None);
        t.handle.step(RequestPhase::Listen, Some(Place::Own), Some("gigaam-v3"));
        t.at(900.0);
        t.handle.partial("какая пого");
        t.handle.step(RequestPhase::Recognize, Some(Place::Own), Some("gigaam-v3"));
        t.at(1100.0);
        t.handle.heard("Какая погода?");
        t.handle.routed(Some("weather"), "own");
        t.handle.step(RequestPhase::Think, Some(Place::Own), Some("Погода"));
        t.at(1500.0);
        t.handle.step(RequestPhase::Speak, Some(Place::Own), Some("baya"));
        t.handle.answered("Плюс десять, ясно.");
        t.at(2700.0);
        t.handle.finish(RequestStatus::Answered, None, None);

        let view = t.handle.snapshot();
        assert_eq!(view.status, RequestStatus::Answered);
        assert_eq!(view.ms, Some(2700));
        assert_eq!(view.text.as_deref(), Some("Какая погода?"));
        assert_eq!(view.answer.as_deref(), Some("Плюс десять, ясно."));
        assert_eq!(view.capability.as_deref(), Some("weather"));
        assert_eq!(view.executor.as_deref(), Some("own"));
        assert_eq!(view.partial, None);
        assert_eq!(view.phase, None);
        assert_eq!(
            view.steps,
            vec![
                step(RequestPhase::Listen, 0, Some(900), Some(Place::Own), Some("gigaam-v3")),
                step(RequestPhase::Recognize, 900, Some(200), Some(Place::Own), Some("gigaam-v3")),
                step(RequestPhase::Think, 1100, Some(400), Some(Place::Own), Some("Погода")),
                step(RequestPhase::Speak, 1500, Some(1200), Some(Place::Own), Some("baya")),
            ]
        );
        assert_eq!(t.hooks.ended.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_same_step_again_says_more_about_who_does_it() {
        let t = trip(None);
        t.handle.step(RequestPhase::Recognize, Some(Place::Own), None);
        t.handle.step(RequestPhase::Recognize, None, Some("gigaam-v3"));
        assert_eq!(
            t.handle.snapshot().steps,
            vec![step(RequestPhase::Recognize, 0, None, Some(Place::Own), Some("gigaam-v3"))]
        );
        assert_eq!(t.handle.phase(), Some(RequestPhase::Recognize));
    }

    #[test]
    fn only_the_last_claimant_ends_it() {
        let t = trip(None);
        let (answer, relay) = (new_owner(), new_owner());
        t.handle.claim(answer);
        t.handle.claim(relay);
        t.handle.finish(RequestStatus::Cancelled, Some("handed to Yandex"), Some(answer));
        assert!(t.handle.active());
        t.handle.finish(RequestStatus::Answered, None, Some(relay));
        assert_eq!(t.handle.snapshot().status, RequestStatus::Answered);
    }

    #[test]
    fn ends_without_an_owner_named_whoever_claimed_it() {
        let t = trip(None);
        t.handle.claim(new_owner());
        t.handle.finish(RequestStatus::Cancelled, Some("disconnected"), None);
        let view = t.handle.snapshot();
        assert_eq!(view.status, RequestStatus::Cancelled);
        assert_eq!(view.error.as_deref(), Some("disconnected"));
    }

    #[test]
    fn changes_nothing_after_the_end_and_ends_once() {
        let t = trip(None);
        t.handle.finish(RequestStatus::Silent, None, None);
        let count = t.hooks.changes.lock().unwrap().len();
        t.handle.step(RequestPhase::Speak, None, None);
        t.handle.answered("поздно");
        t.handle.partial("поздно");
        t.handle.finish(RequestStatus::Failed, Some("late"), None);
        assert_eq!(t.hooks.changes.lock().unwrap().len(), count);
        assert_eq!(t.handle.snapshot().status, RequestStatus::Silent);
        assert_eq!(t.hooks.ended.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cuts_a_novel_to_the_journal_size() {
        let t = trip(None);
        t.handle.answered(&"слово ".repeat(1000));
        assert_eq!(t.handle.snapshot().answer.unwrap().chars().count(), 2000);
    }

    #[test]
    fn the_trip_without_the_words() {
        let t = trip(Some("Позвони маме"));
        t.handle.step(RequestPhase::Think, Some(Place::Cloud), Some("claude-haiku-5-5"));
        t.handle.answered("Звоню маме.");
        t.handle.finish(RequestStatus::Failed, Some("no SIP"), None);
        let bare = without_words(&t.handle.snapshot());
        assert_eq!(bare.text, None);
        assert_eq!(bare.answer, None);
        assert_eq!(bare.error, None);
        assert_eq!(
            bare.steps,
            vec![step(RequestPhase::Think, 0, Some(0), Some(Place::Cloud), Some("claude-haiku-5-5"))]
        );
        assert_eq!(t.handle.snapshot().text.as_deref(), Some("Позвони маме"));
    }
}
