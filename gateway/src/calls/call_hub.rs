//! Our own calls, without Yandex: endpoints register, a call rings the callee until it answers (45 s), then the
//! audio of each side goes to the other in 20 ms frames of 16 kHz PCM. Two parties per call. Events (`ringing`,
//! `connected`, `ended`) let the gateway announce a call on the station and take "ответь".

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::skills::media::sources::match_score;
use crate::utils::time::{iso, now_ms};

/// 20 ms of 16 kHz mono: the hub's frame.
pub const FRAME_SAMPLES: usize = 320;
/// How long a call rings before it ends unanswered.
pub const RING_MS: u64 = 45_000;
/// An ended call stays in the snapshot this long, then it is forgotten.
const KEEP_ENDED_MS: u64 = 10 * 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EndpointKind {
    Station,
    Browser,
    Sip,
    Telegram,
    Sim,
    Livekit,
}

/// A party as events and lists name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Party {
    pub id: String,
    pub name: String,
    pub kind: EndpointKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallEventType {
    Incoming,
    Ringing,
    Connected,
    Ended,
}

/// What a party hears of its call: `{type, callId, peer, reason?}` on the call page's socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallEvent {
    #[serde(rename = "type")]
    pub kind: CallEventType,
    pub call_id: String,
    pub peer: Party,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A party of our calls: the station's client, a browser, a SIP phone through Asterisk, the simulator, a LiveKit
/// participant. The hub calls `deliver` and `notify` from its own operations (outside its lock): they must not
/// block, and anything they start in the hub goes through a task of their own.
pub trait CallEndpoint: Send + Sync {
    /// Unique: `station:<device>`, `browser:<n>`, `sip:<channel>`.
    fn id(&self) -> &str;
    fn kind(&self) -> EndpointKind;
    /// How people call it: «мама», «кухня».
    fn name(&self) -> &str;
    /// Audio from the other party: 16 kHz mono PCM, 20 ms frames.
    fn deliver(&self, frame: &[i16]);
    fn notify(&self, event: &CallEvent);
}

pub fn party_of(endpoint: &dyn CallEndpoint) -> Party {
    Party { id: endpoint.id().to_string(), name: endpoint.name().to_string(), kind: endpoint.kind() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallState {
    Ringing,
    Active,
    Ended,
}

/// Samples each side sent while the call was on: what the log tells at its end.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sent {
    pub from: u64,
    pub to: u64,
}

/// A call as it is at the moment it is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub id: String,
    pub from: Party,
    pub to: Party,
    pub state: CallState,
    /// Milliseconds since the epoch.
    pub started_at: u64,
    pub answered_at: Option<u64>,
    /// The party that hung up, when one did.
    pub ended_by: Option<String>,
    pub sent: Sent,
}

/// A call in `/calls/status`: the parties by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallSnapshot {
    pub id: String,
    pub from: String,
    pub to: String,
    pub state: CallState,
    pub started_at: String,
}

/// What the hub tells its listeners, after the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubEvent {
    /// A party came or went.
    Endpoints,
    Ringing(Call),
    Connected(Call),
    Ended {
        call: Call,
        reason: String,
    },
}

/// Why the hub refused: the same words the call page shows after «Ошибка:».
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CallError(pub String);

struct Record {
    call: Call,
    from: Arc<dyn CallEndpoint>,
    to: Arc<dyn CallEndpoint>,
    ring: Option<tokio::task::AbortHandle>,
    ended_at: Option<u64>,
}

impl Record {
    fn has(&self, id: &str) -> bool {
        self.call.from.id == id || self.call.to.id == id
    }
}

#[derive(Default)]
struct State {
    /// In the order they came, as the TS hub's Map kept them: lists and ties go by it.
    endpoints: Vec<Arc<dyn CallEndpoint>>,
    calls: Vec<Record>,
    /// The part of a party's audio short of a frame.
    buffers: HashMap<String, Vec<i16>>,
    next: u64,
}

type Listener = Arc<dyn Fn(&HubEvent) + Send + Sync>;

pub struct CallHub {
    state: Mutex<State>,
    listeners: Mutex<Vec<Listener>>,
    /// How long a call rings unanswered: RING_MS.
    ring: Duration,
    me: Weak<CallHub>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn same(a: &Arc<dyn CallEndpoint>, b: &Arc<dyn CallEndpoint>) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))
}

fn base36(mut n: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

fn event(kind: CallEventType, call_id: &str, peer: &Party, reason: Option<&str>) -> CallEvent {
    CallEvent { kind, call_id: call_id.to_string(), peer: peer.clone(), reason: reason.map(str::to_string) }
}

impl CallHub {
    pub fn new() -> Arc<CallHub> {
        CallHub::with_ring(Duration::from_millis(RING_MS))
    }

    /// A hub whose calls ring this long: tests.
    pub fn with_ring(ring: Duration) -> Arc<CallHub> {
        Arc::new_cyclic(|me| CallHub { state: Mutex::default(), listeners: Mutex::default(), ring, me: me.clone() })
    }

    /// Runs `listener` after every change, at once, outside the hub's lock and in the order of the changes (the TS
    /// hub was an EventEmitter): it may call the hub; long work goes to a task of its own.
    pub fn on(&self, listener: impl Fn(&HubEvent) + Send + Sync + 'static) {
        lock(&self.listeners).push(Arc::new(listener));
    }

    fn emit(&self, event: HubEvent) {
        let listeners = lock(&self.listeners).clone();
        for listener in listeners {
            listener(&event);
        }
    }

    pub fn register(&self, endpoint: Arc<dyn CallEndpoint>) {
        let previous = self.endpoint(endpoint.id());
        let replaced = previous.as_ref().is_some_and(|previous| !same(previous, &endpoint));
        if replaced {
            self.unregister(endpoint.id(), "replaced");
        }
        {
            let mut state = lock(&self.state);
            if !state.endpoints.iter().any(|known| same(known, &endpoint)) {
                state.endpoints.retain(|known| known.id() != endpoint.id());
                state.endpoints.push(endpoint);
            }
        }
        self.emit(HubEvent::Endpoints);
    }

    /// The party leaves: its calls end with `reason`.
    pub fn unregister(&self, id: &str, reason: &str) {
        let calls: Vec<String> = lock(&self.state)
            .calls
            .iter()
            .filter(|record| record.call.state != CallState::Ended && record.has(id))
            .map(|record| record.call.id.clone())
            .collect();
        for call in calls {
            self.end(&call, reason);
        }
        {
            let mut state = lock(&self.state);
            state.endpoints.retain(|known| known.id() != id);
            state.buffers.remove(id);
        }
        self.emit(HubEvent::Endpoints);
    }

    pub fn endpoint(&self, id: &str) -> Option<Arc<dyn CallEndpoint>> {
        lock(&self.state).endpoints.iter().find(|endpoint| endpoint.id() == id).cloned()
    }

    pub fn list(&self) -> Vec<Party> {
        lock(&self.state).endpoints.iter().map(|endpoint| party_of(endpoint.as_ref())).collect()
    }

    /// An endpoint by id, or by the name people say («маме» finds «Мама»), not `except`.
    pub fn find(&self, name_or_id: &str, except: Option<&str>) -> Option<Arc<dyn CallEndpoint>> {
        let endpoints = lock(&self.state).endpoints.clone();
        if let Some(exact) = endpoints.iter().find(|endpoint| endpoint.id() == name_or_id) {
            return Some(exact.clone());
        }
        let mut scored: Vec<(f64, Arc<dyn CallEndpoint>)> = endpoints
            .into_iter()
            .filter(|endpoint| Some(endpoint.id()) != except)
            .map(|endpoint| (match_score(name_or_id, endpoint.name()), endpoint))
            .filter(|(score, _)| *score >= 0.5)
            .collect();
        // Stable, as the TS sort: a tie goes to the party that came first.
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal));
        scored.into_iter().next().map(|(_, endpoint)| endpoint)
    }

    /// The call an endpoint takes part in, ringing or going on.
    pub fn call_of(&self, endpoint_id: &str) -> Option<Call> {
        lock(&self.state)
            .calls
            .iter()
            .find(|record| record.call.state != CallState::Ended && record.has(endpoint_id))
            .map(|record| record.call.clone())
    }

    /// A call by its id, as it is now (ended ones for a while): whether it still rings.
    pub fn get(&self, call_id: &str) -> Option<Call> {
        lock(&self.state).calls.iter().find(|record| record.call.id == call_id).map(|record| record.call.clone())
    }

    pub fn snapshot(&self) -> Vec<CallSnapshot> {
        let mut state = lock(&self.state);
        forget_old(&mut state);
        state
            .calls
            .iter()
            .map(|record| CallSnapshot {
                id: record.call.id.clone(),
                from: record.call.from.name.clone(),
                to: record.call.to.name.clone(),
                state: record.call.state,
                started_at: iso_ms(record.call.started_at),
            })
            .collect()
    }

    pub fn call(&self, from_id: &str, to_id: &str) -> Result<Call, CallError> {
        let (call, from, to) = {
            let mut state = lock(&self.state);
            forget_old(&mut state);
            let find = |id: &str| state.endpoints.iter().find(|endpoint| endpoint.id() == id).cloned();
            let (Some(from), Some(to)) = (find(from_id), find(to_id)) else {
                let missing = if find(from_id).is_none() { from_id } else { to_id };
                return Err(CallError(format!("no such party: {missing}")));
            };
            let busy =
                |id: &str| state.calls.iter().any(|record| record.call.state != CallState::Ended && record.has(id));
            if busy(from_id) || busy(to_id) {
                let name = if busy(to_id) { to.name() } else { from.name() };
                return Err(CallError(format!("{name} is busy")));
            }
            let now = now_ms();
            let id = format!("c{}{}", base36(now), base36(state.next));
            state.next += 1;
            let call = Call {
                id,
                from: party_of(from.as_ref()),
                to: party_of(to.as_ref()),
                state: CallState::Ringing,
                started_at: now,
                answered_at: None,
                ended_by: None,
                sent: Sent::default(),
            };
            let ring = self.ring_timer(&call.id);
            state.calls.push(Record { call: call.clone(), from: from.clone(), to: to.clone(), ring, ended_at: None });
            (call, from, to)
        };
        to.notify(&event(CallEventType::Incoming, &call.id, &call.from, None));
        from.notify(&event(CallEventType::Ringing, &call.id, &call.to, None));
        self.emit(HubEvent::Ringing(call.clone()));
        Ok(call)
    }

    /// The call ends unanswered after 45 s. Without a runtime (a plain test) it rings until answered.
    fn ring_timer(&self, call_id: &str) -> Option<tokio::task::AbortHandle> {
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        let hub = self.me.clone();
        let call_id = call_id.to_string();
        let ring = self.ring;
        let task = runtime.spawn(async move {
            tokio::time::sleep(ring).await;
            if let Some(hub) = hub.upgrade() {
                hub.ring_expired(&call_id);
            }
        });
        Some(task.abort_handle())
    }

    /// The ring timer's end: a call that still rings ends unanswered. An abort cannot stop a timer that has already
    /// woken, so an answer at the 45th second can find it running; the TS cleared its timer before the state
    /// changed, on one thread.
    fn ring_expired(&self, call_id: &str) {
        self.end_where(call_id, "no answer", |state| state == CallState::Ringing);
    }

    pub fn accept(&self, call_id: &str, by_id: &str) -> Result<Call, CallError> {
        let (call, from, to) = {
            let mut state = lock(&self.state);
            let Some(record) = state
                .calls
                .iter_mut()
                .find(|record| record.call.id == call_id && record.call.state == CallState::Ringing)
            else {
                return Err(CallError("no such call ringing".into()));
            };
            if record.call.to.id != by_id {
                return Err(CallError("only the callee answers".into()));
            }
            if let Some(ring) = record.ring.take() {
                ring.abort();
            }
            record.call.state = CallState::Active;
            record.call.answered_at = Some(now_ms());
            (record.call.clone(), record.from.clone(), record.to.clone())
        };
        from.notify(&event(CallEventType::Connected, &call.id, &call.to, None));
        to.notify(&event(CallEventType::Connected, &call.id, &call.from, None));
        self.emit(HubEvent::Connected(call.clone()));
        Ok(call)
    }

    /// A party hangs up ("hung up"); an ended or unknown call is nothing to do.
    pub fn hangup(&self, call_id: &str, by_id: &str) -> Result<(), CallError> {
        {
            let mut state = lock(&self.state);
            let Some(record) = state.calls.iter_mut().find(|record| record.call.id == call_id) else { return Ok(()) };
            if record.call.state == CallState::Ended {
                return Ok(());
            }
            if !record.has(by_id) {
                return Err(CallError("not a party of this call".into()));
            }
            record.call.ended_by = Some(by_id.to_string());
        }
        self.end(call_id, "hung up");
        Ok(())
    }

    /// Audio from a party: sliced into 20 ms frames and sent to the other side of its active call.
    pub fn audio(&self, from_id: &str, pcm: &[i16]) {
        let (peer, frames) = {
            let mut state = lock(&self.state);
            let state = &mut *state;
            let Some(record) =
                state.calls.iter_mut().find(|record| record.call.state != CallState::Ended && record.has(from_id))
            else {
                return;
            };
            if record.call.state != CallState::Active {
                return;
            }
            let caller = record.call.from.id == from_id;
            if caller {
                record.call.sent.from += pcm.len() as u64;
            } else {
                record.call.sent.to += pcm.len() as u64;
            }
            let peer = if caller { record.to.clone() } else { record.from.clone() };
            let pending = state.buffers.entry(from_id.to_string()).or_default();
            pending.extend_from_slice(pcm);
            let whole = pending.len() / FRAME_SAMPLES * FRAME_SAMPLES;
            let frames: Vec<Vec<i16>> = pending[..whole].chunks(FRAME_SAMPLES).map(<[i16]>::to_vec).collect();
            pending.drain(..whole);
            (peer, frames)
        };
        for frame in frames {
            peer.deliver(&frame);
        }
    }

    fn end(&self, call_id: &str, reason: &str) {
        self.end_where(call_id, reason, |_| true);
    }

    /// Ends the call unless it has ended, when `may_end` takes its state as read under the lock.
    fn end_where(&self, call_id: &str, reason: &str, may_end: impl Fn(CallState) -> bool) {
        let (call, from, to) = {
            let mut state = lock(&self.state);
            let Some(record) = state.calls.iter_mut().find(|record| record.call.id == call_id) else { return };
            if record.call.state == CallState::Ended || !may_end(record.call.state) {
                return;
            }
            if let Some(ring) = record.ring.take() {
                ring.abort();
            }
            record.call.state = CallState::Ended;
            record.ended_at = Some(now_ms());
            (record.call.clone(), record.from.clone(), record.to.clone())
        };
        from.notify(&event(CallEventType::Ended, &call.id, &call.to, Some(reason)));
        to.notify(&event(CallEventType::Ended, &call.id, &call.from, Some(reason)));
        self.emit(HubEvent::Ended { call, reason: reason.to_string() });
    }
}

/// Ended calls are kept a while for the snapshot, then forgotten.
fn forget_old(state: &mut State) {
    let now = now_ms();
    state.calls.retain(|record| record.ended_at.is_none_or(|ended| now.saturating_sub(ended) < KEEP_ENDED_MS));
}

fn iso_ms(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64).map(iso).unwrap_or_default()
}

/// A message field as JavaScript's String() made it: what the TS gateway passed on from a client's JSON (a missing
/// `callId` was "undefined", and so was the error it led to).
pub fn js_string(value: Option<&serde_json::Value>) -> String {
    use serde_json::Value;
    match value {
        None => "undefined".into(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) => "null".into(),
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                item => js_string(Some(item)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(other) => other.to_string(),
    }
}

/// PCM16LE bytes as samples; an odd last byte is dropped.
pub fn pcm_from_bytes(bytes: &[u8]) -> Vec<i16> {
    bytes.as_chunks::<2>().0.iter().map(|pair| i16::from_le_bytes(*pair)).collect()
}

pub fn pcm_to_bytes(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A party that keeps what it hears.
    pub struct TestParty {
        pub id: String,
        pub name: String,
        pub kind: EndpointKind,
        pub events: Mutex<Vec<CallEvent>>,
        pub frames: Mutex<Vec<Vec<i16>>>,
    }

    impl TestParty {
        pub fn new(id: &str, name: &str, kind: EndpointKind) -> Arc<TestParty> {
            Arc::new(TestParty {
                id: id.into(),
                name: name.into(),
                kind,
                events: Mutex::default(),
                frames: Mutex::default(),
            })
        }

        pub fn last(&self) -> CallEvent {
            lock(&self.events).last().cloned().expect("an event")
        }

        pub fn frames(&self) -> Vec<Vec<i16>> {
            lock(&self.frames).clone()
        }
    }

    impl CallEndpoint for TestParty {
        fn id(&self) -> &str {
            &self.id
        }
        fn kind(&self) -> EndpointKind {
            self.kind
        }
        fn name(&self) -> &str {
            &self.name
        }
        fn deliver(&self, frame: &[i16]) {
            lock(&self.frames).push(frame.to_vec());
        }
        fn notify(&self, event: &CallEvent) {
            lock(&self.events).push(event.clone());
        }
    }

    #[tokio::test]
    async fn rings_the_callee_connects_on_its_answer_and_carries_audio_both_ways_in_20_ms_frames() {
        let hub = CallHub::new();
        let mom = TestParty::new("browser:1", "Мама", EndpointKind::Browser);
        let station = TestParty::new("station:default", "Колонка", EndpointKind::Station);
        hub.register(mom.clone());
        hub.register(station.clone());
        let call = hub.call(&mom.id, &station.id).unwrap();
        assert_eq!(station.last().kind, CallEventType::Incoming);
        assert_eq!(station.last().peer.name, "Мама");
        assert_eq!(mom.last().kind, CallEventType::Ringing);
        // No audio before the answer.
        hub.audio(&mom.id, &[0; 320]);
        assert!(station.frames().is_empty());
        assert_eq!(hub.accept(&call.id, &mom.id).unwrap_err().to_string(), "only the callee answers");

        hub.accept(&call.id, &station.id).unwrap();
        assert_eq!(mom.last().kind, CallEventType::Connected);
        hub.audio(&mom.id, &[7; 500]);
        hub.audio(&mom.id, &[7; 200]);
        assert_eq!(station.frames().iter().map(Vec::len).collect::<Vec<_>>(), vec![320, 320]);
        hub.audio(&station.id, &[3; 320]);
        assert_eq!(mom.frames()[0][0], 3);

        hub.hangup(&call.id, &mom.id).unwrap();
        assert_eq!(station.last().kind, CallEventType::Ended);
        assert_eq!(station.last().reason.as_deref(), Some("hung up"));
        assert!(hub.call_of(&station.id).is_none());
    }

    #[tokio::test]
    async fn finds_a_party_by_the_name_people_say_and_refuses_a_busy_one() {
        let hub = CallHub::new();
        let mom = TestParty::new("browser:1", "Мама", EndpointKind::Browser);
        let dad = TestParty::new("browser:2", "Папа", EndpointKind::Browser);
        let station = TestParty::new("station:default", "Колонка", EndpointKind::Station);
        for party in [&mom, &dad, &station] {
            hub.register(party.clone());
        }
        assert_eq!(hub.find("маме", None).map(|found| found.id().to_string()).as_deref(), Some("browser:1"));
        assert!(hub.find("бабушке", None).is_none());
        hub.call(&mom.id, &station.id).unwrap();
        assert!(hub.call(&dad.id, &station.id).unwrap_err().to_string().contains("busy"));
    }

    #[tokio::test]
    async fn ends_the_calls_of_a_party_that_leaves() {
        let hub = CallHub::new();
        let mom = TestParty::new("browser:1", "Мама", EndpointKind::Browser);
        let station = TestParty::new("station:default", "Колонка", EndpointKind::Station);
        hub.register(mom.clone());
        hub.register(station.clone());
        hub.call(&mom.id, &station.id).unwrap();
        hub.unregister(&mom.id, "left");
        assert_eq!(station.last().kind, CallEventType::Ended);
        assert_eq!(station.last().reason.as_deref(), Some("left"));
    }

    #[tokio::test]
    async fn ends_a_call_nobody_answers_and_tells_the_listeners() {
        let hub = CallHub::with_ring(Duration::from_millis(300));
        let heard = Arc::new(Mutex::new(Vec::new()));
        let sink = heard.clone();
        hub.on(move |event| {
            if let HubEvent::Ended { reason, .. } = event {
                lock(&sink).push(reason.clone());
            }
        });
        let mom = TestParty::new("browser:1", "Мама", EndpointKind::Browser);
        let station = TestParty::new("station:default", "Колонка", EndpointKind::Station);
        hub.register(mom.clone());
        hub.register(station.clone());
        let call = hub.call(&mom.id, &station.id).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(hub.get(&call.id).unwrap().state, CallState::Ringing);
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(hub.get(&call.id).unwrap().state, CallState::Ended);
        assert_eq!(mom.last().reason.as_deref(), Some("no answer"));
        assert_eq!(*lock(&heard), vec!["no answer".to_string()]);
        let snapshot = hub.snapshot();
        assert_eq!((snapshot[0].from.as_str(), snapshot[0].state), ("Мама", CallState::Ended));
        assert!(snapshot[0].started_at.ends_with('Z'));
    }

    #[tokio::test]
    async fn a_ring_timer_that_woke_as_the_call_was_answered_leaves_the_call_on() {
        let hub = CallHub::new();
        let mom = TestParty::new("browser:1", "Мама", EndpointKind::Browser);
        let station = TestParty::new("station:default", "Колонка", EndpointKind::Station);
        hub.register(mom.clone());
        hub.register(station.clone());
        let call = hub.call(&mom.id, &station.id).unwrap();
        hub.accept(&call.id, &station.id).unwrap();
        // The timer's task past its sleep: the answer's abort came too late to stop it.
        hub.ring_expired(&call.id);
        assert_eq!(hub.get(&call.id).unwrap().state, CallState::Active);
        assert_eq!(mom.last().kind, CallEventType::Connected);

        hub.hangup(&call.id, &mom.id).unwrap();
        let ringing = hub.call(&mom.id, &station.id).unwrap();
        hub.ring_expired(&ringing.id);
        assert_eq!(hub.get(&ringing.id).unwrap().state, CallState::Ended);
        assert_eq!(station.last().reason.as_deref(), Some("no answer"));
    }

    #[test]
    fn serializes_events_as_the_call_page_reads_them() {
        let peer = Party { id: "browser:1".into(), name: "Мама".into(), kind: EndpointKind::Browser };
        let ringing = event(CallEventType::Ringing, "c1", &peer, None);
        assert_eq!(
            serde_json::to_value(&ringing).unwrap(),
            serde_json::json!({"type": "ringing", "callId": "c1", "peer": {"id": "browser:1", "name": "Мама", "kind": "browser"}})
        );
        let ended = event(CallEventType::Ended, "c1", &peer, Some("hung up"));
        assert_eq!(serde_json::to_value(&ended).unwrap()["reason"], "hung up");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(36), "10");
        assert_eq!(pcm_from_bytes(&pcm_to_bytes(&[1, -2, 300])), vec![1, -2, 300]);
        assert_eq!(pcm_from_bytes(&[1, 0, 9]), vec![1]);
        // A party by the name people say: the skills' match (src/skills/media/sources).
        assert_eq!(match_score("Алиса, позвони МАМЕ, пожалуйста!", "позвони маме"), 1.0);
        assert_eq!(match_score("маме", "Мама"), 1.0);
        assert_eq!(match_score("в", "Мама"), 0.0);
    }
}
