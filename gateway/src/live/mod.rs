//! The panel's live feed from the gateway's bus: requests as they go (the dashboard's trip), stations' presence, and
//! the names of changes to re-read. Each listener gets what its rights allow; a listener that cannot be written to
//! is dropped. `/api/v1/live` serves it as Server-Sent Events (older panels, plugins), `/api/v1/live/ws` as a
//! WebSocket (ws.rs) — with, for a panel at home, the channels of the stations whose gateway runs on them, as this
//! gateway relays them (placement/relay.rs).

pub mod ws;

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
use serde_json::Value;
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
    // A plugin's data changed (its kv): its widgets read theirs again.
    "packages.data",
    "packages.catalog",
    "plugins.resources",
    "workshop.changed",
    "connector.changed",
    "station.updated",
    "station.pending",
    "station.synced",
    "station.state",
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
pub(crate) const HELLO_RECENT: usize = 10;
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
    /// `subject` — whose change it is, when it is one package's (`packages.*`: the package's id): a plugin's widget
    /// reads its data again only on its own. `change` — what happened to the package (`packages.changed`): its code
    /// loaded or unloaded changes nothing a widget shows.
    Changed {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        subject: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        change: Option<String>,
    },
    /// A message of the live channel of a station whose gateway runs on it, as the station said it
    /// (placement/relay.rs): a hello, a request, a presence, a change. Without one — that channel is lost until its
    /// next hello.
    Station {
        station: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<Value>,
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
            LiveMessage::Station { .. } => "station",
        }
    }

    /// A change by its name alone.
    pub fn changed(name: &str) -> LiveMessage {
        LiveMessage::Changed { name: name.to_string(), subject: None, change: None }
    }
}

/// Whose a change is, and what it was, when it is one package's: its widgets read theirs again only on their own.
fn subject_of(event: &GatewayEvent) -> (Option<String>, Option<String>) {
    match event {
        GatewayEvent::PackagesChanged { package_id, change, .. } => (Some(package_id.clone()), Some(change.clone())),
        GatewayEvent::PackageDataChanged { package } => (Some(package.clone()), None),
        _ => (None, None),
    }
}

/// A station's message as one may see it who may not read the journal: its requests without what was said
/// (`without_words`, on the message as the station sent it).
pub fn station_without_words(message: &Value) -> Value {
    let mut message = message.clone();
    let strip = |request: &mut Value| {
        if let Some(fields) = request.as_object_mut() {
            for field in ["text", "partial", "answer", "error"] {
                fields.remove(field);
            }
        }
    };
    match message.get("type").and_then(Value::as_str) {
        Some("request") => {
            if let Some(request) = message.get_mut("request") {
                strip(request);
            }
        }
        Some("hello") => {
            for list in ["active", "recent"] {
                if let Some(Value::Array(requests)) = message.get_mut(list) {
                    requests.iter_mut().for_each(strip);
                }
            }
        }
        _ => {}
    }
    message
}

/// The stations' presence (src/stations/presence, ported apart): the hello's list. Until it is set the hello has
/// none, and each station's presence arrives with its next change.
pub trait PresenceSource: Send + Sync {
    fn list(&self) -> Vec<StationPresence>;
}

/// The stations' channels this gateway relays (placement/relay.rs): each one's state as a hello — what a panel that
/// joins now hears of it right after its own hello.
pub trait StationSource: Send + Sync {
    fn snapshots(&self) -> Vec<(String, Value)>;
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
    /// Whether it hears the stations' channels: a panel of the live socket, at home.
    stations: bool,
    sink: Box<dyn LiveSink>,
}

pub struct LiveHub {
    me: Weak<LiveHub>,
    requests: Arc<RequestsService>,
    presence: OnceLock<Arc<dyn PresenceSource>>,
    stations: OnceLock<Arc<dyn StationSource>>,
    listeners: Mutex<Vec<Listener>>,
    next: AtomicU64,
    /// The live socket's tickets (ws.rs).
    pub tickets: ws::Tickets,
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
            stations: OnceLock::new(),
            listeners: Mutex::new(Vec::new()),
            next: AtomicU64::new(1),
            tickets: ws::Tickets::default(),
        })
    }

    /// The stations' presence for the hello, once the stations module is there.
    pub fn set_presence(&self, source: Arc<dyn PresenceSource>) {
        if self.presence.set(source).is_err() {
            tracing::warn!("live: the presence source is set already");
        }
    }

    /// The stations' channels this gateway relays: a panel that joins hears their state after its hello.
    pub fn set_stations(&self, source: Arc<dyn StationSource>) {
        if self.stations.set(source).is_err() {
            tracing::warn!("live: the stations' relay is set already");
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
        self.join_as(principal, false, sink)
    }

    /// The same, and the stations' channels this gateway relays: each one's state right after the hello, then its
    /// messages. A panel of the live socket at home (ws.rs).
    pub fn join_with_stations(&self, principal: &Principal, sink: Box<dyn LiveSink>) -> Leave {
        self.join_as(principal, true, sink)
    }

    fn join_as(&self, principal: &Principal, stations: bool, sink: Box<dyn LiveSink>) -> Leave {
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
        let mut joined = sink.send(&hello);
        if stations && joined {
            for (station, state) in self.stations.get().map(|source| source.snapshots()).unwrap_or_default() {
                let message = if words { state } else { station_without_words(&state) };
                joined = joined && sink.send(&LiveMessage::Station { station, message: Some(message) });
            }
        }
        if joined {
            listeners.push(Listener { id, words, stations, sink });
        } else {
            tracing::debug!("a panel's stream dropped before its hello");
        }
        Leave { hub: self.me.clone(), id }
    }

    /// A station's message, relayed (placement/relay.rs), to the panels that hear the stations — the words of its
    /// requests only to those who may read the journal. `None`: the station's channel is lost.
    pub fn relay(&self, station: &str, message: Option<&Value>) {
        let mut listeners = self.lock();
        if !listeners.iter().any(|listener| listener.stations) {
            return;
        }
        let full = LiveMessage::Station { station: station.to_string(), message: message.cloned() };
        let bare = LiveMessage::Station { station: station.to_string(), message: message.map(station_without_words) };
        listeners.retain(|listener| {
            if !listener.stations {
                return true;
            }
            let kept = listener.sink.send(if listener.words { &full } else { &bare });
            if !kept {
                tracing::debug!("a panel's stream dropped");
            }
            kept
        });
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
                let (subject, change) = subject_of(other);
                let message = LiveMessage::Changed { name: other.name().to_string(), subject, change };
                deliver(&mut listeners, &|_| message.clone());
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
    Router::new()
        .route("/api/v1/live", get(stream))
        .route("/api/v1/live/ticket", axum::routing::post(ws::ticket))
        .route("/api/v1/live/ws", get(ws::socket))
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
            [LiveMessage::Request { request: Box::new(answered) }, LiveMessage::changed("mode.changed")]
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
        let message = LiveMessage::changed("mode.changed");
        assert_eq!(
            frame(7, &message),
            "id: 7\nevent: changed\ndata: {\"type\":\"changed\",\"name\":\"mode.changed\"}\n\n"
        );
        let hello = LiveMessage::Hello { server_time: "t".into(), presence: vec![], active: vec![], recent: vec![] };
        assert_eq!(
            serde_json::to_value(&hello).unwrap(),
            serde_json::json!({"type": "hello", "serverTime": "t", "presence": [], "active": [], "recent": []})
        );
        let lost = LiveMessage::Station { station: "kitchen".into(), message: None };
        assert_eq!(serde_json::to_value(&lost).unwrap(), serde_json::json!({"type": "station", "station": "kitchen"}));
    }

    #[tokio::test]
    async fn names_the_package_a_change_is_of_and_what_happened_to_it() {
        let (live, events, _) = hub();
        let panel = Arc::new(Recorder::default());
        let _panel = live.join(&person(&["station.view"]), Box::new(panel.clone()));
        events.emit(GatewayEvent::PackageDataChanged { package: "shopping".into() });
        events.emit(GatewayEvent::PackagesChanged {
            change: "unloaded".into(),
            package_id: "weather".into(),
            version: Some("1.0.0".into()),
            error: None,
            actor: None,
        });
        tick().await;
        let changes: Vec<Value> =
            panel.messages()[1..].iter().map(|message| serde_json::to_value(message).unwrap()).collect();
        assert_eq!(
            changes,
            [
                serde_json::json!({"type": "changed", "name": "packages.data", "subject": "shopping"}),
                serde_json::json!({"type": "changed", "name": "packages.changed", "subject": "weather", "change": "unloaded"}),
            ]
        );
    }

    /// A relay's stations, as it would keep them.
    struct Relayed(Vec<(String, Value)>);

    impl StationSource for Relayed {
        fn snapshots(&self) -> Vec<(String, Value)> {
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn tells_the_stations_channels_only_to_the_sockets_at_home_and_their_words_only_to_the_journals_readers() {
        let (live, _, _) = hub();
        let asked =
            serde_json::json!({"id": "r1", "status": "answered", "text": "Добавь молоко", "answer": "Добавила."});
        let hello =
            serde_json::json!({"type": "hello", "serverTime": "t", "presence": [], "active": [], "recent": [asked]});
        live.set_stations(Arc::new(Relayed(vec![("h:kitchen".into(), hello)])));
        let reader = Arc::new(Recorder::default());
        let viewer = Arc::new(Recorder::default());
        let old = Arc::new(Recorder::default());
        let _reader = live.join_with_stations(&person(&["station.view", "journal.view"]), Box::new(reader.clone()));
        let _viewer = live.join_with_stations(&person(&["station.view"]), Box::new(viewer.clone()));
        let _old = live.join(&person(&["station.view", "journal.view"]), Box::new(old.clone()));

        // The station's state right after the hello: its words to the journal's reader only.
        let LiveMessage::Station { station, message: Some(state) } = &reader.messages()[1] else { panic!("a station") };
        assert_eq!((station.as_str(), state["recent"][0]["text"].as_str()), ("h:kitchen", Some("Добавь молоко")));
        let LiveMessage::Station { message: Some(state), .. } = &viewer.messages()[1] else { panic!("a station") };
        assert_eq!(state["recent"][0]["id"], "r1");
        assert_eq!((state["recent"][0].get("text"), state["recent"][0].get("answer")), (None, None));
        assert_eq!(old.messages().len(), 1, "the stream of an older panel: its own hello only");

        let request =
            serde_json::json!({"type": "request", "request": {"id": "r2", "status": "active", "text": "Который час?"}});
        live.relay("h:kitchen", Some(&request));
        live.relay("h:kitchen", None);
        assert_eq!(reader.messages()[2], LiveMessage::Station { station: "h:kitchen".into(), message: Some(request) });
        let LiveMessage::Station { message: Some(bare), .. } = &viewer.messages()[2] else { panic!("a station") };
        assert_eq!((bare["request"]["id"].as_str(), bare["request"].get("text")), (Some("r2"), None));
        assert_eq!(viewer.messages()[3], LiveMessage::Station { station: "h:kitchen".into(), message: None }, "lost");
        assert_eq!(old.messages().len(), 1);
    }
}
