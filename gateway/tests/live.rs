//! The panel's live channel end to end, as the panel reads it: a signed GET, Server-Sent Events as they come; the
//! journal of the same requests; the streams' end when the gateway stops. (The Node test drove requests through a
//! station; here the RequestStarter the sessions will use starts them.)

mod support;

use duowave::core::requests::{Place, RequestKind, RequestPhase, RequestStart, RequestStarter, RequestStatus};
use reqwest::StatusCode;
use serde_json::json;
use support::live::{setup, Live, Socket};
use support::{bearer, message, until, Gateway};

/// The panel's live socket (src/live/ws.rs): a ticket first, then the socket with it; what it says, as it comes.
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

/// A stream relayed to the station (`/media/stream`) is endless: the gateway's stop ends it and does not wait for it.
/// On the station on 20.11.2026 the stop waited for one for good, and the process never ended.
#[tokio::test]
async fn a_stream_relayed_to_the_station_ends_with_the_gateway_and_does_not_hold_its_stop() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // An endless radio: a little sound every 50 ms.
    let radio = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = radio.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = radio.accept().await {
            tokio::spawn(async move {
                let mut request = [0u8; 1024];
                let _ = socket.read(&mut request).await;
                let head = b"HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nConnection: close\r\n\r\n";
                if socket.write_all(head).await.is_err() {
                    return;
                }
                while socket.write_all(&[0u8; 512]).await.is_ok() {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            });
        }
    });
    let gateway = Gateway::start(&[]).await;
    let token = gateway.app.streams.register(&format!("http://{address}/live.mp3"));
    let mut response = gateway.get(&format!("/media/stream/{token}")).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.chunk().await.unwrap().is_some(), "it plays");
    let listening = tokio::spawn(async move { while let Ok(Some(_)) = response.chunk().await {} });
    let began = std::time::Instant::now();
    tokio::time::timeout(std::time::Duration::from_secs(3), gateway.stop()).await.expect("the stop does not wait");
    assert!(began.elapsed() < duowave::app::DRAIN_LIMIT, "the stream was cut, not waited out: {:?}", began.elapsed());
    tokio::time::timeout(std::time::Duration::from_secs(2), listening).await.expect("the stream ended").unwrap();
}
