//! The panel's live channel end to end, as the panel reads it: a signed GET, Server-Sent Events as they come; the
//! journal of the same requests; the streams' end when the gateway stops. (The Node test drove requests through a
//! station; here the RequestStarter the sessions will use starts them.)

mod support;

use std::sync::{Arc, Mutex};

use duowave::core::requests::{Place, RequestKind, RequestPhase, RequestStart, RequestStarter, RequestStatus};
use reqwest::StatusCode;
use serde_json::{json, Value};
use support::{bearer, message, until, Gateway};

struct Live {
    messages: Arc<Mutex<Vec<Value>>>,
    ended: Arc<Mutex<bool>>,
    reader: tokio::task::JoinHandle<()>,
}

impl Live {
    async fn open(gateway: &Gateway, token: &str) -> Live {
        let (name, value) = bearer(token);
        let response = gateway.get("/api/v1/live").header(name, value).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let kind = response.headers()["content-type"].to_str().unwrap().to_string();
        assert!(kind.starts_with("text/event-stream"), "{kind}");
        assert_eq!(response.headers()["cache-control"], "no-cache, no-transform");
        let messages = Arc::new(Mutex::new(Vec::new()));
        let ended = Arc::new(Mutex::new(false));
        let (into, done) = (messages.clone(), ended.clone());
        let reader = tokio::spawn(async move {
            let mut response = response;
            let mut buffer = String::new();
            while let Ok(Some(chunk)) = response.chunk().await {
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(end) = buffer.find("\n\n") {
                    let block: String = buffer.drain(..end + 2).collect();
                    let data: Vec<&str> = block.lines().filter_map(|line| line.strip_prefix("data: ")).collect();
                    if !data.is_empty() {
                        into.lock().unwrap().push(serde_json::from_str(&data.join("\n")).unwrap());
                    }
                }
            }
            *done.lock().unwrap() = true;
        });
        Live { messages, ended, reader }
    }

    fn of_type(&self, kind: &str) -> Vec<Value> {
        self.messages.lock().unwrap().iter().filter(|message| message["type"] == kind).cloned().collect()
    }

    fn requests(&self) -> Vec<Value> {
        self.of_type("request").into_iter().map(|message| message["request"].clone()).collect()
    }
}

/// The panel's live socket (src/live/ws.rs): a ticket first, then the socket with it; what it says, as it comes.
struct Socket {
    messages: Arc<Mutex<Vec<Value>>>,
    /// The close frame's code, once it came.
    closed: Arc<Mutex<Option<u16>>>,
    reader: tokio::task::JoinHandle<()>,
}

impl Socket {
    async fn ticket(gateway: &Gateway, token: &str) -> String {
        let (name, value) = bearer(token);
        let ticket = message(gateway.post("/api/v1/live/ticket").header(name, value), StatusCode::CREATED).await;
        assert_eq!(ticket["expiresIn"], 30);
        ticket["ticket"].as_str().unwrap().to_string()
    }

    fn address(gateway: &Gateway, ticket: &str) -> String {
        format!("ws://127.0.0.1:{}/api/v1/live/ws?ticket={ticket}", gateway.port)
    }

    async fn open(gateway: &Gateway, token: &str) -> Socket {
        let ticket = Socket::ticket(gateway, token).await;
        let (client, _) = tokio_tungstenite::connect_async(Socket::address(gateway, &ticket)).await.unwrap();
        let messages = Arc::new(Mutex::new(Vec::new()));
        let closed = Arc::new(Mutex::new(None));
        let (into, end) = (messages.clone(), closed.clone());
        let reader = tokio::spawn(async move {
            use futures::StreamExt;
            use tokio_tungstenite::tungstenite::Message;
            let mut client = client;
            while let Some(Ok(frame)) = client.next().await {
                match frame {
                    Message::Text(text) => into.lock().unwrap().push(serde_json::from_str(text.as_str()).unwrap()),
                    Message::Close(frame) => {
                        *end.lock().unwrap() = Some(frame.map(|frame| u16::from(frame.code)).unwrap_or(1005));
                    }
                    _ => {}
                }
            }
        });
        Socket { messages, closed, reader }
    }

    fn of_type(&self, kind: &str) -> Vec<Value> {
        self.messages.lock().unwrap().iter().filter(|message| message["type"] == kind).cloned().collect()
    }

    /// What a station said, relayed: `None` — its channel lost.
    fn station(&self, key: &str) -> Vec<Option<Value>> {
        self.of_type("station")
            .into_iter()
            .filter(|message| message["station"] == key)
            .map(|message| message.get("message").cloned())
            .collect()
    }
}

async fn setup(gateway: &Gateway) -> String {
    let setup =
        message(gateway.post("/api/v1/auth/setup").json(&json!({"password": "admin-password-1"})), StatusCode::CREATED);
    setup.await["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn follows_requests_live_on_the_panels_channel_the_trip_the_words_the_end_and_keeps_them_in_the_journal() {
    let gateway = Gateway::start(&[("UNIPROXY_MODE", "gateway")]).await;
    let admin = setup(&gateway).await;
    let member = json!({"name": "Аня", "password": "anya-password", "role": "member"});
    let (name, value) = bearer(&admin);
    message(gateway.post("/api/v1/users").header(name, value).json(&member), StatusCode::CREATED).await;
    let anya = message(
        gateway.post("/api/v1/auth/login").json(&json!({"name": "Аня", "password": "anya-password"})),
        StatusCode::OK,
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_string();

    let reader = Live::open(&gateway, &admin).await;
    let viewer = Live::open(&gateway, &anya).await;
    assert!(until(2000, || !reader.of_type("hello").is_empty() && !viewer.of_type("hello").is_empty()).await);
    let hello = reader.of_type("hello").remove(0);
    assert!(hello["presence"].is_array());
    assert!(chrono::DateTime::parse_from_rfc3339(hello["serverTime"].as_str().unwrap()).is_ok());

    let requests: &dyn RequestStarter = gateway.app.requests.as_ref();
    let trip = requests.begin(RequestStart {
        device: "kitchen".into(),
        session: Some("s1".into()),
        kind: RequestKind::Text,
        text: Some("Который час?".into()),
        parent: None,
    });
    tokio::task::yield_now().await;
    trip.routed(Some("chat"), "own");
    trip.step(RequestPhase::Think, Some(Place::Own), Some("Разговор, время, дата"));
    trip.step(RequestPhase::Speak, Some(Place::Yandex), Some("Алиса"));
    trip.answered("12:00.");
    trip.finish(RequestStatus::Answered, None, None);

    let ended = |live: &Live| live.requests().iter().any(|request| request["status"] == "answered");
    assert!(until(2000, || ended(&reader) && ended(&viewer)).await, "{:?}", reader.messages.lock().unwrap());
    let updates = reader.requests();
    assert_eq!((updates[0]["status"].as_str(), updates[0]["mode"].as_str()), (Some("active"), Some("gateway")));
    let done = updates.last().unwrap();
    assert_eq!((done["capability"].as_str(), done["executor"].as_str()), (Some("chat"), Some("own")));
    assert_eq!((done["text"].as_str(), done["answer"].as_str()), (Some("Который час?"), Some("12:00.")));
    let phases: Vec<&str> =
        done["steps"].as_array().unwrap().iter().filter_map(|step| step["phase"].as_str()).collect();
    assert_eq!(phases, ["think", "speak"]);
    assert_eq!(done["steps"][1]["by"], "yandex");
    // Who may not read the journal sees the trip, not the words.
    let bare = viewer.requests().last().unwrap().clone();
    assert_eq!((bare["status"].as_str(), bare.get("text"), bare.get("answer")), (Some("answered"), None, None));
    assert_eq!(bare["steps"], done["steps"]);

    let (name, value) = bearer(&admin);
    let journal = message(gateway.get("/api/v1/requests?limit=10").header(name, value), StatusCode::OK).await;
    assert_eq!(journal["keepDays"], 30);
    let row = journal["requests"].as_array().unwrap().iter().find(|row| row["id"] == done["id"]).unwrap().clone();
    assert_eq!((row["text"].as_str(), row["status"].as_str()), (Some("Который час?"), Some("answered")));
    let (name, value) = bearer(&anya);
    assert_eq!(
        message(gateway.get("/api/v1/requests").header(name, value), StatusCode::FORBIDDEN).await,
        "Нет права: читать журнал запросов: что говорили колонке.",
        "the journal is for journal.view"
    );

    // How long the journal is kept is a setting of the house.
    let (name, value) = bearer(&admin);
    let kept =
        message(gateway.put("/api/v1/requests/keep").header(name, value).json(&json!({"days": 7})), StatusCode::OK);
    assert_eq!(kept.await, json!({"days": 7}));
    let changed = || reader.of_type("changed").iter().any(|message| message["name"] == "settings.changed");
    assert!(until(2000, changed).await, "the panel re-reads what changed elsewhere");
}

#[tokio::test]
async fn ends_the_open_streams_when_the_gateway_stops_and_journals_what_was_going() {
    let gateway = Gateway::start(&[]).await;
    let admin = setup(&gateway).await;
    let live = Live::open(&gateway, &admin).await;
    assert!(until(2000, || !live.of_type("hello").is_empty()).await);
    let requests: &dyn RequestStarter = gateway.app.requests.as_ref();
    let going = requests.begin(RequestStart {
        device: "kitchen".into(),
        session: None,
        kind: RequestKind::Voice,
        text: None,
        parent: None,
    });
    let app = gateway.app.clone();
    let ended = live.ended.clone();
    tokio::time::timeout(std::time::Duration::from_secs(5), gateway.stop()).await.expect("the server stops at once");
    assert!(until(2000, || *ended.lock().unwrap()).await, "the stream ended");
    live.reader.abort();
    assert_eq!(going.snapshot().status, RequestStatus::Cancelled);
    let journal = app.requests.history(&Default::default()).unwrap();
    assert_eq!(journal[0].error.as_deref(), Some("the gateway stopped"));
}

/// How the socket refused: the HTTP status of its upgrade.
async fn refused(address: String) -> u16 {
    match tokio_tungstenite::connect_async(address).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => response.status().as_u16(),
        other => panic!("the socket should be refused: {:?}", other.map(|(_, response)| response.status())),
    }
}

#[tokio::test]
async fn the_live_socket_opens_with_a_ticket_once_says_what_the_stream_says_and_ends_with_the_gateway() {
    let gateway = Gateway::start(&[]).await;
    let admin = setup(&gateway).await;
    // A ticket is a signed call's; the token never goes into the socket's address.
    let (status, _) = support::send(gateway.post("/api/v1/live/ticket")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let ticket = Socket::ticket(&gateway, &admin).await;
    let (client, _) = tokio_tungstenite::connect_async(Socket::address(&gateway, &ticket)).await.unwrap();
    drop(client);
    assert_eq!(refused(Socket::address(&gateway, &ticket)).await, 401, "a ticket is good once");
    assert_eq!(refused(Socket::address(&gateway, "not-a-ticket")).await, 401);
    assert_eq!(refused(format!("ws://127.0.0.1:{}/api/v1/live/ws", gateway.port)).await, 401);
    assert_eq!(
        refused(format!("ws://127.0.0.1:{}/api/v1/live/ws?ticket={admin}", gateway.port)).await,
        401,
        "nor the token"
    );

    // The same messages as the stream: the hello, then each change — a package's with its id.
    let socket = Socket::open(&gateway, &admin).await;
    assert!(until(2000, || !socket.of_type("hello").is_empty()).await);
    let hello = socket.of_type("hello").remove(0);
    assert!(hello["presence"].is_array() && hello["recent"].is_array());
    assert!(chrono::DateTime::parse_from_rfc3339(hello["serverTime"].as_str().unwrap()).is_ok());
    gateway.app.events.emit(duowave::core::events::GatewayEvent::PackageDataChanged { package: "shopping".into() });
    let changed = || socket.of_type("changed").iter().any(|message| message["subject"] == "shopping");
    assert!(until(2000, changed).await, "{:?}", socket.messages.lock().unwrap());
    let requests: &dyn RequestStarter = gateway.app.requests.as_ref();
    let trip = requests.begin(RequestStart {
        device: "kitchen".into(),
        session: None,
        kind: RequestKind::Text,
        text: Some("Который час?".into()),
        parent: None,
    });
    trip.finish(RequestStatus::Answered, None, None);
    let answered = || socket.of_type("request").iter().any(|message| message["request"]["status"] == "answered");
    assert!(until(2000, answered).await);

    // The gateway stops: the socket ends at once, said to open again.
    let closed = socket.closed.clone();
    tokio::time::timeout(std::time::Duration::from_secs(5), gateway.stop()).await.expect("the server stops at once");
    assert!(until(2000, || closed.lock().unwrap().is_some()).await, "the socket closed");
    assert_eq!(*closed.lock().unwrap(), Some(1000));
    socket.reader.abort();
}

/// The station's live channel through the computer (src/placement/relay.rs): two real gateways, the station's a
/// computer build playing it. The computer keeps one channel to it and tells its panels' sockets what it says — the
/// words of its requests only to who may read the journal here; the stream of an older panel hears none of it.
#[cfg(feature = "pc")]
#[tokio::test]
async fn the_computer_relays_the_stations_channel_to_its_panels_sockets() {
    use duowave::core::availability::Placement;
    use duowave::settings::definitions::{SettingsHome, StationPlacement};
    const PEER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    let station =
        Gateway::start(&[("DUOWAVE_ROLE", "station"), ("PEER_KEY", PEER), ("UNIPROXY_MODE", "gateway")]).await;
    let pc = Gateway::start(&[]).await;
    let admin = setup(&pc).await;
    let (name, value) = bearer(&admin);
    let member = json!({"name": "Аня", "password": "anya-password", "role": "member"});
    message(pc.post("/api/v1/users").header(name, value).json(&member), StatusCode::CREATED).await;
    let login = pc.post("/api/v1/auth/login").json(&json!({"name": "Аня", "password": "anya-password"}));
    let anya = message(login, StatusCode::OK).await["token"].as_str().unwrap().to_string();

    pc.app.secrets.set(&duowave::placement::peer::station_secret("h:kitchen"), Some(PEER)).unwrap();
    let record = StationPlacement {
        placement: Placement::Station,
        address: "127.0.0.1".into(),
        port: station.port,
        device: Some("h:kitchen".into()),
        settings_home: SettingsHome::Station,
        auto_update: false,
        managed: false,
        home_key_id: None,
        moved_at: None,
    };
    let watch = pc.app.station_gateway.watch("h:kitchen");
    watch.set_record(Some(&record), None).unwrap();

    let reader = Socket::open(&pc, &admin).await;
    let viewer = Socket::open(&pc, &anya).await;
    let older = Live::open(&pc, &admin).await;
    let station_hello = |socket: &Socket| {
        socket.station("h:kitchen").iter().any(|message| message.as_ref().is_some_and(|m| m["type"] == "hello"))
    };
    assert!(
        until(5000, || station_hello(&reader) && station_hello(&viewer)).await,
        "{:?}",
        reader.messages.lock().unwrap()
    );

    // The station's own bus: a request asked of it, a plugin's data written there.
    let requests: &dyn RequestStarter = station.app.requests.as_ref();
    let trip = requests.begin(RequestStart {
        device: "kitchen".into(),
        session: None,
        kind: RequestKind::Voice,
        text: Some("Добавь молоко".into()),
        parent: None,
    });
    trip.answered("Добавила.");
    trip.finish(RequestStatus::Answered, None, None);
    station.app.events.emit(duowave::core::events::GatewayEvent::PackageDataChanged { package: "shopping".into() });

    let answered = |socket: &Socket| {
        socket
            .station("h:kitchen")
            .into_iter()
            .flatten()
            .find(|m| m["type"] == "request" && m["request"]["status"] == "answered")
    };
    assert!(until(5000, || answered(&reader).is_some() && answered(&viewer).is_some()).await);
    let full = answered(&reader).unwrap();
    assert_eq!(
        (full["request"]["text"].as_str(), full["request"]["answer"].as_str()),
        (Some("Добавь молоко"), Some("Добавила."))
    );
    let bare = answered(&viewer).unwrap();
    assert_eq!(
        (bare["request"].get("text"), bare["request"].get("answer")),
        (None, None),
        "a member: the trip, not the words"
    );
    let data = |socket: &Socket| {
        socket
            .station("h:kitchen")
            .into_iter()
            .flatten()
            .any(|m| m["name"] == "packages.data" && m["subject"] == "shopping")
    };
    assert!(until(5000, || data(&reader)).await);
    assert!(older.of_type("station").is_empty(), "an older panel's stream hears no station");

    // The gateway back on the computer: the channel closes, and the sockets hear it is lost.
    watch.set_record(Some(&StationPlacement { placement: Placement::Pc, ..record }), None).unwrap();
    assert!(until(5000, || reader.station("h:kitchen").last() == Some(&None)).await);
    reader.reader.abort();
    viewer.reader.abort();
    older.reader.abort();
}
