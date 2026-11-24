//! The station's live channel through the computer (src/placement/relay.rs), end to end: the computer's build only
//! (gateway/Cargo.toml, `required-features`); the panel's channel itself is tests/live.rs.

mod support;

use duowave::core::requests::{RequestKind, RequestStart, RequestStarter, RequestStatus};
use reqwest::StatusCode;
use serde_json::json;
use support::live::{setup, Live, Socket};
use support::{bearer, message, until, Gateway};

/// The station's live channel through the computer (src/placement/relay.rs): two real gateways, the station's a
/// computer build playing it. The computer keeps one channel to it and tells its panels' sockets what it says — the
/// words of its requests only to who may read the journal here; the stream of an older panel hears none of it.
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
    let watch = pc.app.pc.station_gateway.watch("h:kitchen");
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
