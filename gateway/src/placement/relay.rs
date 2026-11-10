//! The station's live channel, relayed (docs/plans/desk-after-station.md, А). While a station's gateway runs on it,
//! this computer's gateway keeps one subscription to that station's live channel — with the pair's key, as the
//! helper's pass does (proxy.rs) — and tells its own panels every message of it, marked with the station:
//! `{type: 'station', station, message}` (live/mod.rs). The panels open no stream to the station themselves: a
//! browser gives one address six HTTP/1.1 connections, and two streams in every tab took them all.
//!
//! The subscription asks with the admin's role, and the computer's hub gives the words of the station's requests only
//! to who may read the journal here. It keeps the station's state — its last hello with the requests and presence
//! since — so a panel that joins hears it at once. It starts and stops as the station's record changes
//! (`placement.changed`), opens again at once when the station ends it (every 15 minutes), and after a failure waits
//! 1, 2, 5, 10, 30 s, then a minute — or less, when the watch sees the station again. A station that is off is told
//! in the log once, and once more when it is back.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use reqwest::header::ACCEPT;
use serde_json::Value;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::peer::HELPER_ROLE;
use super::watch::StationGateways;
use crate::core::events::{EventBus, GatewayEvent};
use crate::live::{LiveHub, StationSource, HELLO_RECENT};
use crate::utils::time::now_ms;

/// The station's channel says something at least every 20 s (a ping): silence this long is a dead connection.
const SILENCE: Duration = Duration::from_secs(45);
/// Pauses before the next try after failures in a row: 1, 2, 5, 10, 30 s, then a minute.
const BACKOFF: [Duration; 6] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(60),
];
/// The station did not take the pair's key: the pair may be linked anew — asked that rarely.
const REFUSED_WAIT: Duration = Duration::from_secs(60);
/// The station ended the stream itself (its lifetime): opened again at once.
const AGAIN: Duration = Duration::from_millis(300);
/// An SSE block larger than this is not the station's.
const BLOCK_LIMIT: usize = 1024 * 1024;
/// What the station says, as the panel hears it; anything else (a newer build's) is not passed on.
const KINDS: &[&str] = &["hello", "request", "presence", "changed"];

/// Where the station's gateway is and its pair key: a change of either opens the channel anew.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    base: String,
    key: String,
}

struct Running {
    target: Target,
    task: JoinHandle<()>,
    /// Wakes a channel waiting after a failure: the station's record changed, or the watch saw it.
    wake: Arc<Notify>,
}

/// A station's state as its channel said it: its last hello with every request and presence since, and its clock
/// against ours (ms) — a panel joining later gets the hello with the station's time now.
struct Kept {
    hello: Value,
    offset: i64,
}

/// How a subscription ended.
#[derive(Debug, PartialEq)]
enum Outcome {
    /// The station ended it (its lifetime): opened again at once.
    Ended,
    /// The station did not take the pair's key.
    Refused(String),
    /// Unreachable, broken, silent.
    Failed(String),
}

/// What the log says of a station's channel: its loss once, its return once — a station that is off for the night
/// does not fill the log.
#[derive(Debug, Default)]
struct Quiet {
    told: bool,
}

impl Quiet {
    /// The channel failed: whether to tell it.
    fn failed(&mut self) -> bool {
        !std::mem::replace(&mut self.told, true)
    }

    /// The channel is up: whether to tell that it is back.
    fn up(&mut self) -> bool {
        std::mem::take(&mut self.told)
    }
}

/// The pause after this many failures in a row.
fn pause_after(failures: usize) -> Duration {
    BACKOFF[failures.clamp(1, BACKOFF.len()) - 1]
}

pub struct StationRelay {
    me: Weak<StationRelay>,
    stations: Arc<StationGateways>,
    hub: Weak<LiveHub>,
    running: Mutex<BTreeMap<String, Running>>,
    kept: Mutex<BTreeMap<String, Kept>>,
}

impl StationRelay {
    /// Relays the channel of every station whose gateway runs on it, from now on: the hub's panels hear them, and
    /// the station's record changing starts or stops its channel.
    pub fn start(stations: Arc<StationGateways>, hub: &Arc<LiveHub>, events: &EventBus) -> Arc<StationRelay> {
        let relay = Arc::new_cyclic(|me| StationRelay {
            me: me.clone(),
            stations,
            hub: Arc::downgrade(hub),
            running: Mutex::new(BTreeMap::new()),
            kept: Mutex::new(BTreeMap::new()),
        });
        hub.set_stations(relay.clone());
        let weak = Arc::downgrade(&relay);
        events.on("station-relay", move |event| {
            if let GatewayEvent::PlacementChanged { .. } = &*event {
                if let Some(relay) = weak.upgrade() {
                    relay.reconcile();
                }
            }
        });
        relay.reconcile();
        relay
    }

    /// The channels that should be open, opened; the others closed. A channel waiting after a failure tries now.
    fn reconcile(&self) {
        let Some(me) = self.me.upgrade() else { return };
        let wanted: BTreeMap<String, Target> = self
            .stations
            .all()
            .into_iter()
            .filter_map(|watch| {
                let client = watch.client()?;
                Some((
                    watch.key().to_string(),
                    Target { base: client.base().to_string(), key: client.key().to_string() },
                ))
            })
            .collect();
        let mut gone = Vec::new();
        {
            let mut running = self.lock_running();
            running.retain(|key, run| {
                let keep = wanted.get(key) == Some(&run.target);
                if !keep {
                    run.task.abort();
                    gone.push(key.clone());
                }
                keep
            });
            for (key, target) in wanted {
                if let Some(run) = running.get(&key) {
                    run.wake.notify_one();
                    continue;
                }
                tracing::debug!(target: "placement", "relay: following the station {key}'s live channel");
                let wake = Arc::new(Notify::new());
                let task = tokio::spawn(me.clone().follow(key.clone(), target.clone(), wake.clone()));
                running.insert(key, Running { target, task, wake });
            }
        }
        for key in gone {
            self.lost(&key);
        }
    }

    /// One station's channel, for as long as it is wanted: opened, read, opened again.
    async fn follow(self: Arc<Self>, key: String, target: Target, wake: Arc<Notify>) {
        let mut failures = 0usize;
        let mut quiet = Quiet::default();
        loop {
            let outcome = self.listen(&key, &target, &mut failures, &mut quiet).await;
            let pause = match &outcome {
                Outcome::Ended => {
                    tracing::debug!(target: "placement", "relay: the station {key} ended its live channel");
                    AGAIN
                }
                Outcome::Refused(why) | Outcome::Failed(why) => {
                    self.lost(&key);
                    if quiet.failed() {
                        tracing::info!(target: "placement", "relay: the station {key}'s live channel: {why}; trying again quietly");
                    } else {
                        tracing::debug!(target: "placement", "relay: the station {key}'s live channel: {why}");
                    }
                    if matches!(outcome, Outcome::Refused(_)) {
                        REFUSED_WAIT
                    } else {
                        failures += 1;
                        pause_after(failures)
                    }
                }
            };
            tokio::select! {
                _ = tokio::time::sleep(pause) => {}
                _ = wake.notified() => {}
            }
        }
    }

    /// The station's channel once: what it says, relayed, until it ends, breaks or goes silent.
    async fn listen(&self, key: &str, target: &Target, failures: &mut usize, quiet: &mut Quiet) -> Outcome {
        let request = super::proxy::client()
            .get(format!("{}/api/v1/live", target.base))
            .bearer_auth(&target.key)
            .header(HELPER_ROLE, "admin")
            .header(ACCEPT, "text/event-stream");
        let mut response = match request.send().await {
            Ok(response) => response,
            Err(error) => return Outcome::Failed(describe(&error)),
        };
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Outcome::Refused(format!("шлюз колонки не принял ключ пары ({})", status.as_u16()));
        }
        if !status.is_success() {
            return Outcome::Failed(format!("шлюз колонки ответил {}", status.as_u16()));
        }
        let mut buffer: Vec<u8> = Vec::new();
        loop {
            let chunk = match tokio::time::timeout(SILENCE, response.chunk()).await {
                Err(_) => return Outcome::Failed("канал колонки молчит".into()),
                Ok(Err(error)) => return Outcome::Failed(describe(&error)),
                Ok(Ok(None)) => return Outcome::Ended,
                Ok(Ok(Some(chunk))) => chunk,
            };
            buffer.extend_from_slice(&chunk);
            while let Some(end) = buffer.windows(2).position(|pair| pair == b"\n\n") {
                let block: Vec<u8> = buffer.drain(..end + 2).collect();
                let Some(message) = data_of(&block) else { continue };
                if message.get("type").and_then(Value::as_str) == Some("hello") {
                    *failures = 0;
                    if quiet.up() {
                        tracing::info!(target: "placement", "relay: the station {key}'s live channel is back");
                    }
                }
                self.heard(key, message);
            }
            if buffer.len() > BLOCK_LIMIT {
                return Outcome::Failed("канал колонки прислал непонятное".into());
            }
        }
    }

    /// A station's message: kept into its state, then told to the hub's panels.
    fn heard(&self, key: &str, message: Value) {
        let kind = message.get("type").and_then(Value::as_str).unwrap_or_default();
        if !KINDS.contains(&kind) {
            return;
        }
        {
            let mut kept = self.lock_kept();
            match kind {
                "hello" => {
                    kept.insert(key.to_string(), Kept { offset: offset_of(&message), hello: message.clone() });
                }
                "request" => {
                    if let (Some(state), Some(request)) = (kept.get_mut(key), message.get("request")) {
                        keep_request(&mut state.hello, request);
                    }
                }
                "presence" => {
                    if let (Some(state), Some(presence)) = (kept.get_mut(key), message.get("presence")) {
                        keep_presence(&mut state.hello, presence);
                    }
                }
                _ => {}
            }
        }
        if let Some(hub) = self.hub.upgrade() {
            hub.relay(key, Some(&message));
        }
    }

    /// The station's channel is lost: its state goes, and the panels hear it.
    fn lost(&self, key: &str) {
        let had = self.lock_kept().remove(key).is_some();
        if let (true, Some(hub)) = (had, self.hub.upgrade()) {
            hub.relay(key, None);
        }
    }

    fn lock_running(&self) -> MutexGuard<'_, BTreeMap<String, Running>> {
        self.running.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_kept(&self) -> MutexGuard<'_, BTreeMap<String, Kept>> {
        self.kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl StationSource for StationRelay {
    /// Each station's state as a hello, with the station's time now.
    fn snapshots(&self) -> Vec<(String, Value)> {
        let now = now_ms() as i64;
        self.lock_kept()
            .iter()
            .map(|(key, kept)| {
                let mut hello = kept.hello.clone();
                if let (Some(fields), Some(at)) =
                    (hello.as_object_mut(), chrono::DateTime::from_timestamp_millis(now + kept.offset))
                {
                    fields.insert("serverTime".into(), Value::String(crate::utils::time::iso(at)));
                }
                (key.clone(), hello)
            })
            .collect()
    }
}

impl Drop for StationRelay {
    fn drop(&mut self) {
        for run in self.lock_running().values() {
            run.task.abort();
        }
    }
}

fn describe(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "шлюз колонки не ответил вовремя".into()
    } else if error.is_connect() {
        "шлюз колонки не отвечает".into()
    } else {
        format!("шлюз колонки: {error}")
    }
}

/// An SSE block's message: its `data` lines as JSON. A comment (the ping) has none.
fn data_of(block: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(block).ok()?;
    let data: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|line| line.strip_prefix(' ').unwrap_or(line))
        .collect();
    if data.is_empty() {
        return None;
    }
    serde_json::from_str(&data.join("\n")).ok()
}

/// The station's clock against ours, by its hello's time (ms).
fn offset_of(hello: &Value) -> i64 {
    hello
        .get("serverTime")
        .and_then(Value::as_str)
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .map(|at| at.timestamp_millis() - now_ms() as i64)
        .unwrap_or(0)
}

/// A request as it goes, into the hello: among those going while it goes, then first of the last finished.
fn keep_request(hello: &mut Value, request: &Value) {
    let Some(id) = request.get("id").cloned() else { return };
    for list in ["active", "recent"] {
        if let Some(Value::Array(requests)) = hello.get_mut(list) {
            requests.retain(|entry| entry.get("id") != Some(&id));
        }
    }
    let going = request.get("status").and_then(Value::as_str) == Some("active");
    if let Some(Value::Array(requests)) = hello.get_mut(if going { "active" } else { "recent" }) {
        requests.insert(0, request.clone());
        if !going {
            requests.truncate(HELLO_RECENT);
        }
    }
}

/// A station's presence, into the hello: in place of its previous one.
fn keep_presence(hello: &mut Value, presence: &Value) {
    let device = presence.get("device").cloned();
    if let Some(Value::Array(list)) = hello.get_mut("presence") {
        match list.iter_mut().find(|entry| entry.get("device") == device.as_ref()) {
            Some(entry) => *entry = presence.clone(),
            None => list.push(presence.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::body::{Body, Bytes};
    use axum::http::HeaderMap;
    use axum::routing::get;
    use axum::Router;
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;
    use crate::app::App;
    use crate::config::{Config, Env};
    use crate::core::availability::Placement;
    use crate::core::identity::principal::{Principal, PrincipalKind};
    use crate::live::{LiveMessage, LiveSink};
    use crate::settings::definitions::{SettingsHome, StationPlacement};

    const PAIR: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn keeps_the_stations_state_as_a_hello() {
        let mut hello = json!({
            "type": "hello",
            "serverTime": "2026-11-10T12:00:00.000Z",
            "presence": [{"device": "kitchen", "state": "voice"}],
            "active": [],
            "recent": (0..HELLO_RECENT).map(|n| json!({"id": format!("old{n}"), "status": "answered"})).collect::<Vec<_>>(),
        });
        keep_request(&mut hello, &json!({"id": "r1", "status": "active", "text": "Который час?"}));
        assert_eq!(hello["active"][0]["id"], "r1");
        keep_request(&mut hello, &json!({"id": "r1", "status": "answered", "answer": "Полдень."}));
        assert_eq!(hello["active"], json!([]));
        assert_eq!(
            (hello["recent"][0]["id"].as_str(), hello["recent"][0]["answer"].as_str()),
            (Some("r1"), Some("Полдень."))
        );
        assert_eq!(hello["recent"].as_array().unwrap().len(), HELLO_RECENT, "the oldest finished went");
        keep_presence(&mut hello, &json!({"device": "kitchen", "state": "offline"}));
        keep_presence(&mut hello, &json!({"device": "hall", "state": "alive"}));
        assert_eq!(
            hello["presence"],
            json!([{"device": "kitchen", "state": "offline"}, {"device": "hall", "state": "alive"}])
        );
    }

    #[test]
    fn reads_the_stations_messages_out_of_its_stream_and_tells_the_log_of_it_once() {
        assert_eq!(
            data_of(b"id: 3\nevent: changed\ndata: {\"type\":\"changed\",\"name\":\"mode.changed\"}\n\n").unwrap()
                ["name"],
            "mode.changed"
        );
        assert_eq!(data_of(b": ping\n\n"), None);
        assert_eq!(data_of(b"data: {broken\n\n"), None);
        assert_eq!(
            (0..8).map(pause_after).map(|pause| pause.as_secs()).collect::<Vec<_>>(),
            [1, 1, 2, 5, 10, 30, 60, 60]
        );
        let mut quiet = Quiet::default();
        assert!(!quiet.up(), "up from the start: nothing to say");
        assert!(quiet.failed());
        assert!(!quiet.failed() && !quiet.failed(), "said once");
        assert!(quiet.up(), "back: said once");
        assert!(!quiet.up());
        assert!(quiet.failed());
    }

    #[derive(Default)]
    struct Panel(Mutex<Vec<LiveMessage>>);

    impl LiveSink for Arc<Panel> {
        fn send(&self, message: &LiveMessage) -> bool {
            self.0.lock().unwrap().push(message.clone());
            true
        }

        fn end(&self) {}
    }

    impl Panel {
        fn stations(&self) -> Vec<Option<Value>> {
            let messages = self.0.lock().unwrap();
            messages
                .iter()
                .filter_map(|message| match message {
                    LiveMessage::Station { message, .. } => Some(message.clone()),
                    _ => None,
                })
                .collect()
        }
    }

    /// A pretend station's live channel: each connection is told what `say` sends, and `ends` closes it; how many
    /// came and with which headers is noted.
    struct Pretend {
        port: u16,
        say: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>>,
        came: Arc<AtomicUsize>,
        headers: Arc<Mutex<Option<HeaderMap>>>,
    }

    impl Pretend {
        async fn start() -> Pretend {
            let say: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>> = Arc::default();
            let came = Arc::new(AtomicUsize::new(0));
            let headers: Arc<Mutex<Option<HeaderMap>>> = Arc::default();
            let (said, count, seen) = (say.clone(), came.clone(), headers.clone());
            let live = move |request: axum::extract::Request| {
                let (said, count, seen) = (said.clone(), count.clone(), seen.clone());
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    *seen.lock().unwrap() = Some(request.headers().clone());
                    let (sender, receiver) = mpsc::unbounded_channel::<String>();
                    *said.lock().unwrap() = Some(sender);
                    let stream = futures::stream::unfold(receiver, |mut receiver| async move {
                        let block = receiver.recv().await?;
                        Some((Ok::<_, std::convert::Infallible>(Bytes::from(block)), receiver))
                    });
                    axum::response::Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(Body::from_stream(stream))
                        .unwrap()
                }
            };
            let router = Router::new().route("/api/v1/live", get(live));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            Pretend { port, say, came, headers }
        }

        fn send(&self, message: Value) {
            let block = format!("event: x\ndata: {message}\n\n");
            self.say.lock().unwrap().as_ref().expect("a channel open").send(block).unwrap();
        }

        /// The station ends the channel it has open.
        fn end(&self) {
            self.say.lock().unwrap().take();
        }
    }

    async fn until(mut ok: impl FnMut() -> bool) -> bool {
        for _ in 0..200 {
            if ok() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    fn kitchen(port: u16, placement: Placement) -> StationPlacement {
        StationPlacement {
            placement,
            address: "127.0.0.1".into(),
            port,
            device: Some("h:kitchen".into()),
            settings_home: SettingsHome::Station,
            auto_update: false,
            managed: false,
            home_key_id: None,
            moved_at: None,
        }
    }

    fn admin() -> Principal {
        Principal {
            kind: PrincipalKind::User,
            id: "admin".into(),
            name: "Мама".into(),
            role: None,
            grants: vec!["*".into()],
            denies: Vec::new(),
        }
    }

    #[tokio::test]
    async fn keeps_one_channel_to_a_station_with_its_own_gateway_and_tells_the_panels_what_it_says() {
        let station = Pretend::start().await;
        let state = tempfile::tempdir().unwrap();
        let app = App::build(Config::for_tests(&Env::from_pairs([(
            "STATE_DIR",
            state.path().to_string_lossy().into_owned(),
        )])))
        .unwrap();
        app.secrets.set(&crate::placement::peer::station_secret("h:kitchen"), Some(PAIR)).unwrap();
        let watch = app.station_gateway.watch("h:kitchen");
        watch.set_record(Some(&kitchen(station.port, Placement::Station)), None).unwrap();
        let relay = StationRelay::start(app.station_gateway.clone(), &app.live, &app.events);
        let panel = Arc::new(Panel::default());
        let _panel = app.live.join_with_stations(&admin(), Box::new(panel.clone()));

        // One channel, with the pair's key and the admin's role: the computer's hub gives the words to its readers.
        assert!(until(|| station.say.lock().unwrap().is_some()).await, "the relay came");
        let headers = station.headers.lock().unwrap().clone().unwrap();
        assert_eq!(headers["authorization"], format!("Bearer {PAIR}"));
        assert_eq!(headers[HELPER_ROLE], "admin");
        let hello = json!({"type": "hello", "serverTime": "2026-11-10T12:00:00.000Z", "presence": [], "active": [], "recent": []});
        station.send(hello.clone());
        station.send(json!({"type": "changed", "name": "packages.data", "subject": "shopping"}));
        station.send(json!({"type": "request", "request": {"id": "r1", "status": "active", "text": "Добавь молоко"}}));
        assert!(until(|| panel.stations().len() == 3).await, "{:?}", panel.stations());
        assert_eq!(panel.stations()[0], Some(hello));
        assert_eq!(panel.stations()[1].as_ref().unwrap()["subject"], "shopping");

        // A panel that joins now hears the station's state after its own hello, the station's clock as now.
        let later = Arc::new(Panel::default());
        let _later = app.live.join_with_stations(&admin(), Box::new(later.clone()));
        let snapshot = later.stations()[0].clone().unwrap();
        assert_eq!(
            (snapshot["type"].as_str(), snapshot["active"][0]["text"].as_str()),
            (Some("hello"), Some("Добавь молоко"))
        );
        assert_ne!(snapshot["serverTime"], "2026-11-10T12:00:00.000Z");

        // The station ends its channel (its 15 minutes): opened again at once, and nothing is said to be lost.
        station.end();
        assert!(until(|| station.came.load(Ordering::SeqCst) == 2 && station.say.lock().unwrap().is_some()).await);
        assert!(panel.stations().iter().all(Option::is_some), "not lost: {:?}", panel.stations());

        // The gateway back on the computer: the channel closes, and the panels hear it is lost.
        watch.set_record(Some(&kitchen(station.port, Placement::Pc)), None).unwrap();
        assert!(until(|| panel.stations().last() == Some(&None)).await, "{:?}", panel.stations());
        assert!(relay.lock_running().is_empty());
        assert!(relay.snapshots().is_empty());
    }

    #[tokio::test]
    async fn a_station_that_is_off_is_tried_again_less_and_less_and_its_panels_hear_nothing() {
        let state = tempfile::tempdir().unwrap();
        let app = App::build(Config::for_tests(&Env::from_pairs([(
            "STATE_DIR",
            state.path().to_string_lossy().into_owned(),
        )])))
        .unwrap();
        app.secrets.set(&crate::placement::peer::station_secret("h:kitchen"), Some(PAIR)).unwrap();
        // Nobody listens on port 9 of this computer.
        app.station_gateway.watch("h:kitchen").set_record(Some(&kitchen(9, Placement::StationServer)), None).unwrap();
        let relay = StationRelay::start(app.station_gateway.clone(), &app.live, &app.events);
        let panel = Arc::new(Panel::default());
        let _panel = app.live.join_with_stations(&admin(), Box::new(panel.clone()));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(relay.lock_running().len(), 1, "still wanted");
        assert!(panel.stations().is_empty(), "never up: nothing to say lost");
    }
}
