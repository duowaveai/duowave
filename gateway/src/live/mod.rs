//! The panel's live feed from the gateway's bus: requests as they go (the dashboard's trip), stations' presence, and
//! the names of changes to re-read. Each listener gets what its rights allow; a listener that cannot be written to
//! is dropped. `/api/v1/live` serves it as Server-Sent Events.

use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures::Stream;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::app::App;
use crate::core::events::{EventBus, GatewayEvent};
use crate::core::identity::permissions::{JOURNAL_VIEW, STATION_VIEW};
use crate::core::identity::{can, Principal};
use crate::core::requests::{without_words, RequestView};
use crate::core::stations::presence::StationPresence;
use crate::platform::http::{ApiError, Caller};
use crate::requests::RequestsService;
use crate::utils::time::now_iso;

/// Events the panel re-reads on. Names only: whatever they carry, the panel reads through its own API and rights.
pub const RELAYED: &[&str] = &[
    "settings.changed",
    "mode.changed",
    "routes.changed",
    "voice.changed",
    "persona.changed",
    "model.installed",
    "model.removed",
    "model.server.restarted",
    "voice.engine.installed",
    "voice.own.created",
    "voice.own.renamed",
    "voice.own.deleted",
    "voice.job.finished",
    "brain.changed",
    "mcp.changed",
    "packages.changed",
    "packages.catalog",
    "plugins.resources",
    "workshop.changed",
    "connector.changed",
    "station.updated",
    "station.pending",
    "station.synced",
    "availability.changed",
    "homeServer.up",
    "homeServer.down",
    "homeServer.changed",
    "placement.moved",
    "placement.changed",
    "gateway.state",
    "safety.fired",
    "stationGateway.updated",
    // A new version: the panel puts its dot on «Дом» and the line on the home screen.
    "update.available",
];

/// Finished requests in the hello: the dashboard's «last asked».
const HELLO_RECENT: usize = 10;
const PING: Duration = Duration::from_secs(20);
/// A stream lives this long; the panel opens it again, signed anew (a revoked key or a changed role takes effect).
const LIFETIME: Duration = Duration::from_secs(15 * 60);
/// Messages a panel may fall behind by before its stream is closed (it opens it again with a fresh hello).
const QUEUE: usize = 256;

/// What the panel hears: the state at once, then each change.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LiveMessage {
    #[serde(rename_all = "camelCase")]
    Hello {
        /// The gateway's clock: the panel times a request going now against it, not its own.
        server_time: String,
        presence: Vec<StationPresence>,
        active: Vec<RequestView>,
        recent: Vec<RequestView>,
    },
    Request {
        request: Box<RequestView>,
    },
    Presence {
        presence: StationPresence,
    },
    /// Something the panel shows changed elsewhere (another phone, a script, the gateway itself): it reads it again.
    Changed {
        name: String,
    },
}

impl LiveMessage {
    /// The SSE event's name: hello, request, presence, changed.
    pub fn kind(&self) -> &'static str {
        match self {
            LiveMessage::Hello { .. } => "hello",
            LiveMessage::Request { .. } => "request",
            LiveMessage::Presence { .. } => "presence",
            LiveMessage::Changed { .. } => "changed",
        }
    }
}

/// The stations' presence (src/stations/presence, ported apart): the hello's list. Until it is set the hello has
/// none, and each station's presence arrives with its next change.
pub trait PresenceSource: Send + Sync {
    fn list(&self) -> Vec<StationPresence>;
}

/// Where a listener's messages go: a panel's stream, or a test.
pub trait LiveSink: Send + Sync {
    /// False when it cannot be written to: the hub drops it.
    fn send(&self, message: &LiveMessage) -> bool;
    /// The gateway stops: the stream ends.
    fn end(&self);
}

struct Listener {
    id: u64,
    /// Whether it may read what people said (journal.view): without it, requests come without their words.
    words: bool,
    sink: Box<dyn LiveSink>,
}

pub struct LiveHub {
    me: Weak<LiveHub>,
    requests: Arc<RequestsService>,
    presence: OnceLock<Arc<dyn PresenceSource>>,
    listeners: Mutex<Vec<Listener>>,
    next: AtomicU64,
}

/// A listener's place in the hub: dropping it leaves.
pub struct Leave {
    hub: Weak<LiveHub>,
    id: u64,
}

impl Drop for Leave {
    fn drop(&mut self) {
        if let Some(hub) = self.hub.upgrade() {
            hub.lock().retain(|listener| listener.id != self.id);
        }
    }
}

impl LiveHub {
    pub fn new(requests: Arc<RequestsService>) -> Arc<LiveHub> {
        Arc::new_cyclic(|me| LiveHub {
            me: me.clone(),
            requests,
            presence: OnceLock::new(),
            listeners: Mutex::new(Vec::new()),
            next: AtomicU64::new(1),
        })
    }

    /// The stations' presence for the hello, once the stations module is there.
    pub fn set_presence(&self, source: Arc<dyn PresenceSource>) {
        if self.presence.set(source).is_err() {
            tracing::warn!("live: the presence source is set already");
        }
    }

    /// Follows the bus from now on (inside a tokio runtime).
    pub fn start(self: &Arc<Self>, events: &EventBus) {
        let hub = Arc::downgrade(self);
        events.on("live", move |event| {
            if let Some(hub) = hub.upgrade() {
                hub.publish(&event);
            }
        });
    }

    /// Open streams end with the gateway: the HTTP server would wait for them.
    pub fn shutdown(&self) {
        let listeners = std::mem::take(&mut *self.lock());
        for listener in &listeners {
            listener.sink.end();
        }
    }

    pub fn size(&self) -> usize {
        self.lock().len()
    }

    /// A panel joins: the state now, then every change it may see.
    pub fn join(&self, principal: &Principal, sink: Box<dyn LiveSink>) -> Leave {
        let words = can(Some(principal), JOURNAL_VIEW);
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut listeners = self.lock();
        let visible = |view: RequestView| if words { view } else { without_words(&view) };
        let hello = LiveMessage::Hello {
            server_time: now_iso(),
            presence: self.presence.get().map(|source| source.list()).unwrap_or_default(),
            active: self.requests.active_views().into_iter().map(visible).collect(),
            recent: self.requests.recent_views(HELLO_RECENT).into_iter().map(visible).collect(),
        };
        if sink.send(&hello) {
            listeners.push(Listener { id, words, sink });
        } else {
            tracing::debug!("a panel's stream dropped before its hello");
        }
        Leave { hub: self.me.clone(), id }
    }

    fn publish(&self, event: &GatewayEvent) {
        let mut listeners = self.lock();
        if listeners.is_empty() {
            return;
        }
        let deliver = |listeners: &mut Vec<Listener>, message: &dyn Fn(&Listener) -> LiveMessage| {
            listeners.retain(|listener| {
                let kept = listener.sink.send(&message(listener));
                if !kept {
                    tracing::debug!("a panel's stream dropped");
                }
                kept
            });
        };
        match event {
            GatewayEvent::RequestUpdated { request } => {
                let bare = without_words(request);
                deliver(&mut listeners, &|listener| LiveMessage::Request {
                    request: Box::new(if listener.words { request.clone() } else { bare.clone() }),
                });
            }
            GatewayEvent::StationPresenceChanged { presence } => {
                deliver(&mut listeners, &|_| LiveMessage::Presence { presence: presence.clone() });
            }
            other if RELAYED.contains(&other.name()) => {
                let name = other.name().to_string();
                deliver(&mut listeners, &|_| LiveMessage::Changed { name: name.clone() });
            }
            _ => {}
        }
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Listener>> {
        self.listeners.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A panel's stream: a queue the hub fills and the response drains. A full queue is a panel that fell behind.
struct ChannelSink(mpsc::Sender<LiveMessage>);

impl LiveSink for ChannelSink {
    fn send(&self, message: &LiveMessage) -> bool {
        self.0.try_send(message.clone()).is_ok()
    }

    fn end(&self) {
        // The hub drops the sink right after: the queue closes and the stream ends once it is drained.
    }
}

/// One SSE frame: `id`, `event`, `data` (the message as JSON).
pub fn frame(id: u64, message: &LiveMessage) -> String {
    let data = serde_json::to_string(message).unwrap_or_else(|_| "{}".into());
    format!("id: {id}\nevent: {}\ndata: {data}\n\n", message.kind())
}

struct Stream_ {
    queue: mpsc::Receiver<LiveMessage>,
    _leave: Leave,
    ping: tokio::time::Interval,
    end: std::pin::Pin<Box<tokio::time::Sleep>>,
    id: u64,
}

/// The stream's bytes: each message as it comes, a comment every 20 s through proxies, the end after 15 minutes or
/// when the hub lets it go.
fn events_body(queue: mpsc::Receiver<LiveMessage>, leave: Leave) -> impl Stream<Item = Result<Bytes, Infallible>> {
    let start = tokio::time::Instant::now();
    let state = Stream_ {
        queue,
        _leave: leave,
        ping: tokio::time::interval_at(start + PING, PING),
        end: Box::pin(tokio::time::sleep_until(start + LIFETIME)),
        id: 0,
    };
    futures::stream::unfold(Some(state), |state| async move {
        let mut state = state?;
        tokio::select! {
            message = state.queue.recv() => {
                let message = message?;
                state.id += 1;
                Some((Ok(Bytes::from(frame(state.id, &message))), Some(state)))
            }
            _ = state.ping.tick() => Some((Ok(Bytes::from_static(b": ping\n\n")), Some(state))),
            _ = &mut state.end => None,
        }
    })
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/live", get(stream))
}

/// The panel's live channel: Server-Sent Events over a plain GET, signed like any API call — the panel reads it with
/// fetch and its bearer token (EventSource cannot send one). A hello with the state, then each change; a comment
/// every 20 s keeps it open through proxies; after 15 minutes it ends and the panel opens it again.
async fn stream(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
    let principal = caller.permitted(STATION_VIEW)?;
    let (sender, queue) = mpsc::channel(QUEUE);
    let leave = app.live.join(principal, Box::new(ChannelSink(sender)));
    let mut response = Response::new(Body::from_stream(events_body(queue, leave)));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, no-transform"));
    headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UniproxyMode;
    use crate::core::identity::principal::PrincipalKind;
    use crate::core::requests::{Place, RequestKind, RequestPhase, RequestStart, RequestStatus};
    use crate::core::stations::presence::{PresenceState, StationChannel};
    use crate::requests::StoredMode;
    use crate::settings::SettingsService;
    use crate::storage::Db;

    async fn tick() {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    fn person(grants: &[&str]) -> Principal {
        Principal {
            kind: PrincipalKind::User,
            id: grants.join(","),
            name: "test".into(),
            role: None,
            grants: grants.iter().map(|grant| grant.to_string()).collect(),
            denies: Vec::new(),
        }
    }

    fn presence(state: PresenceState) -> StationPresence {
        StationPresence {
            device: "kitchen".into(),
            state,
            channels: if state == PresenceState::Voice { vec![StationChannel::Voice] } else { vec![] },
            last_seen: None,
            since: "2026-11-05T12:00:00.000Z".into(),
        }
    }

    struct Stations;

    impl PresenceSource for Stations {
        fn list(&self) -> Vec<StationPresence> {
            vec![presence(PresenceState::Voice)]
        }
    }

    #[derive(Default)]
    struct Recorder {
        messages: Mutex<Vec<LiveMessage>>,
        ended: AtomicU64,
        /// Fails every send after this many.
        fails_after: Option<usize>,
    }

    impl LiveSink for Arc<Recorder> {
        fn send(&self, message: &LiveMessage) -> bool {
            let mut messages = self.messages.lock().unwrap();
            if self.fails_after.is_some_and(|limit| messages.len() >= limit) {
                return false;
            }
            messages.push(message.clone());
            true
        }

        fn end(&self) {
            self.ended.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl Recorder {
        fn messages(&self) -> Vec<LiveMessage> {
            self.messages.lock().unwrap().clone()
        }
    }

    /// The hub on a bus of its own, over a requests service with one request thinking in the cloud.
    fn hub() -> (Arc<LiveHub>, EventBus, RequestView) {
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let modes = Arc::new(StoredMode::new(settings.clone(), UniproxyMode::Gateway));
        let requests = RequestsService::new(db, EventBus::new(), settings, modes);
        let trip = requests.begin_in(
            RequestStart {
                device: "kitchen".into(),
                session: None,
                kind: RequestKind::Voice,
                text: Some("Позвони маме".into()),
                parent: None,
            },
            "gateway",
        );
        trip.step(RequestPhase::Think, Some(Place::Cloud), Some("claude-haiku-5-5"));
        let events = EventBus::new();
        let live = LiveHub::new(requests);
        live.set_presence(Arc::new(Stations));
        live.start(&events);
        (live, events, trip.snapshot())
    }

    #[tokio::test]
    async fn gives_the_words_of_a_request_only_to_whoever_may_read_the_journal_the_trip_to_all_who_may_see_the_station()
    {
        let (live, events, request) = hub();
        let reader = Arc::new(Recorder::default());
        let viewer = Arc::new(Recorder::default());
        let _reader = live.join(&person(&["station.view", "journal.view"]), Box::new(reader.clone()));
        let _viewer = live.join(&person(&["station.view"]), Box::new(viewer.clone()));

        let LiveMessage::Hello { presence: stations, active, .. } = &reader.messages()[0] else { panic!("hello") };
        assert_eq!((stations[0].device.as_str(), stations[0].state), ("kitchen", PresenceState::Voice));
        assert_eq!(active[0].text.as_deref(), Some("Позвони маме"));
        let LiveMessage::Hello { active, .. } = &viewer.messages()[0] else { panic!("hello") };
        assert_eq!(active[0].text, None);
        assert_eq!(active[0].steps, request.steps);

        let answered = RequestView { answer: Some("Звоню маме.".into()), ..request.clone() };
        events.emit(GatewayEvent::RequestUpdated { request: answered.clone() });
        events.emit(GatewayEvent::ModeChanged { from: "gateway".into(), to: "relay".into(), actor: None, auto: None });
        events.emit(GatewayEvent::AuthLogin { user_id: "u1".into(), name: "Мама".into(), source: "panel".into() });
        tick().await;
        assert_eq!(
            reader.messages()[1..],
            [
                LiveMessage::Request { request: Box::new(answered) },
                LiveMessage::Changed { name: "mode.changed".into() }
            ]
        );
        let LiveMessage::Request { request: bare } = &viewer.messages()[1] else { panic!("a request") };
        assert_eq!((bare.id.as_str(), bare.phase), (request.id.as_str(), Some(RequestPhase::Think)));
        assert_eq!(bare.answer, None);
        assert_eq!(bare.status, RequestStatus::Active);
    }

    #[tokio::test]
    async fn drops_a_panel_whose_stream_cannot_be_written_and_ends_every_stream_at_shutdown() {
        let (live, events, _) = hub();
        let steady = Arc::new(Recorder::default());
        let broken = Arc::new(Recorder { fails_after: Some(1), ..Recorder::default() });
        let _steady = live.join(&person(&["station.view"]), Box::new(steady.clone()));
        let _broken = live.join(&person(&["station.view"]), Box::new(broken.clone()));
        assert_eq!(live.size(), 2);
        events.emit(GatewayEvent::StationPresenceChanged { presence: presence(PresenceState::Offline) });
        tick().await;
        assert_eq!(live.size(), 1);
        assert_eq!(steady.messages()[1], LiveMessage::Presence { presence: presence(PresenceState::Offline) });
        live.shutdown();
        assert_eq!(steady.ended.load(Ordering::SeqCst), 1);
        assert_eq!(live.size(), 0);
    }

    #[tokio::test]
    async fn leaves_when_the_stream_goes() {
        let (live, _, _) = hub();
        let leave = live.join(&person(&["station.view"]), Box::new(Arc::new(Recorder::default())));
        assert_eq!(live.size(), 1);
        drop(leave);
        assert_eq!(live.size(), 0);
    }

    #[test]
    fn writes_each_message_as_an_event_of_its_type() {
        let message = LiveMessage::Changed { name: "mode.changed".into() };
        assert_eq!(
            frame(7, &message),
            "id: 7\nevent: changed\ndata: {\"type\":\"changed\",\"name\":\"mode.changed\"}\n\n"
        );
        let hello = LiveMessage::Hello { server_time: "t".into(), presence: vec![], active: vec![], recent: vec![] };
        assert_eq!(
            serde_json::to_value(&hello).unwrap(),
            serde_json::json!({"type": "hello", "serverTime": "t", "presence": [], "active": [], "recent": []})
        );
    }
}
