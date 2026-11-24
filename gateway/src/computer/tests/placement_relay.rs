//! The tests of `placement/relay.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::HeaderMap;
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::app::App;
use crate::config::{Config, Env};
use crate::core::availability::Placement;
use crate::core::identity::principal::{Principal, PrincipalKind};
use crate::live::{LiveMessage, LiveSink, StationSource, HELLO_RECENT};
use crate::placement::peer::HELPER_ROLE;
use crate::placement::relay::*;
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
        data_of(b"id: 3\nevent: changed\ndata: {\"type\":\"changed\",\"name\":\"mode.changed\"}\n\n").unwrap()["name"],
        "mode.changed"
    );
    assert_eq!(data_of(b": ping\n\n"), None);
    assert_eq!(data_of(b"data: {broken\n\n"), None);
    assert_eq!((0..8).map(pause_after).map(|pause| pause.as_secs()).collect::<Vec<_>>(), [1, 1, 2, 5, 10, 30, 60, 60]);
    let mut quiet = Quiet::default();
    assert!(!quiet.up(), "up from the start: nothing to say");
    assert!(quiet.failed());
    assert!(!quiet.failed() && !quiet.failed(), "said once");
    assert!(quiet.up(), "back: said once");
    assert!(!quiet.up());
    assert!(quiet.failed());
}

#[derive(Default, Clone)]
struct Panel(std::sync::Arc<Mutex<Vec<LiveMessage>>>);

impl LiveSink for Panel {
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
    let app =
        App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", state.path().to_string_lossy().into_owned())])))
            .unwrap();
    app.secrets.set(&crate::placement::peer::station_secret("h:kitchen"), Some(PAIR)).unwrap();
    let watch = app.pc.station_gateway.watch("h:kitchen");
    watch.set_record(Some(&kitchen(station.port, Placement::Station)), None).unwrap();
    let relay = StationRelay::start(app.pc.station_gateway.clone(), &app.live, &app.events);
    let panel = Panel::default();
    let _panel = app.live.join_with_stations(&admin(), Box::new(panel.clone()));

    // One channel, with the pair's key and the admin's role: the computer's hub gives the words to its readers.
    assert!(until(|| station.say.lock().unwrap().is_some()).await, "the relay came");
    let headers = station.headers.lock().unwrap().clone().unwrap();
    assert_eq!(headers["authorization"], format!("Bearer {PAIR}"));
    assert_eq!(headers[HELPER_ROLE], "admin");
    let hello =
        json!({"type": "hello", "serverTime": "2026-11-10T12:00:00.000Z", "presence": [], "active": [], "recent": []});
    station.send(hello.clone());
    station.send(json!({"type": "changed", "name": "packages.data", "subject": "shopping"}));
    station.send(json!({"type": "request", "request": {"id": "r1", "status": "active", "text": "Добавь молоко"}}));
    assert!(until(|| panel.stations().len() == 3).await, "{:?}", panel.stations());
    assert_eq!(panel.stations()[0], Some(hello));
    assert_eq!(panel.stations()[1].as_ref().unwrap()["subject"], "shopping");

    // A panel that joins now hears the station's state after its own hello, the station's clock as now.
    let later = Panel::default();
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
    let app =
        App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", state.path().to_string_lossy().into_owned())])))
            .unwrap();
    app.secrets.set(&crate::placement::peer::station_secret("h:kitchen"), Some(PAIR)).unwrap();
    // Nobody listens on port 9 of this computer.
    app.pc.station_gateway.watch("h:kitchen").set_record(Some(&kitchen(9, Placement::StationServer)), None).unwrap();
    let relay = StationRelay::start(app.pc.station_gateway.clone(), &app.live, &app.events);
    let panel = Panel::default();
    let _panel = app.live.join_with_stations(&admin(), Box::new(panel.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(relay.lock_running().len(), 1, "still wanted");
    assert!(panel.stations().is_empty(), "never up: nothing to say lost");
}
