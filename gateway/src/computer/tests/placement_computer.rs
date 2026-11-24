//! The computer's part of whole_tests/placement_routes.rs: what only a computer has, tested with the whole gateway built.

use axum::http::StatusCode;
use serde_json::{json, Value};

use crate::app::App;
use crate::config::{Config, Env};
use crate::placement::routes::*;
use crate::whole_tests::placement_routes::*;

/// The link offered to a computer centre is this computer's host with its link's port, never its gateway's port too.
#[test]
fn the_link_offer_takes_the_host_without_the_gateways_port() {
    assert_eq!(pc::link_host("192.168.1.104:7780"), "192.168.1.104");
    assert_eq!(pc::link_host("192.168.1.104"), "192.168.1.104");
}

/// The first run's probe reads a real /health answer, envelope and all: a station's gateway is one, a computer's
/// is not; a gateway from before the envelope (bare) still is.
#[tokio::test]
async fn the_probe_recognises_a_station_gateway_by_its_real_health_answer() {
    let (status, health) = send(&station(&[]), "GET", "/health", &[], Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{health}");
    assert_eq!(pc::recognise(&health), (true, Some(crate::VERSION)), "{health}");
    assert_eq!(pc::recognise(&health["message"]), (true, Some(crate::VERSION)), "bare");

    let computer = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
    let (_, health) = send(&computer, "GET", "/health", &[], Value::Null).await;
    assert_eq!(pc::recognise(&health), (false, Some(crate::VERSION)), "{health}");
    assert_eq!(pc::recognise(&json!({ "code": 200, "message": { "status": "ok" } })), (false, None), "a stranger");
}

/// Stages 2 and 3 of docs/plans/home-setup-flow.md, end to end over a real socket: the computer asks a station
/// centre in (no password of its panel), the code the centre shows its admin is typed on the computer, and the
/// computer is linked to the station as `pair-station` linked it — the pair's key never typed by anybody.
#[tokio::test]
async fn a_computer_joins_the_station_centre_by_the_code_and_is_linked() {
    let centre = station(&[]);
    let admin = centre.access.auth.setup("admin-password-1", "Мама", "test").await.unwrap().unwrap().token;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(crate::app::serve(centre.clone(), listener, std::future::pending()));

    let computer = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
    let owner = computer.access.auth.setup("admin-password-1", "Я", "test").await.unwrap().unwrap().token;
    let owner = format!("Bearer {owner}");
    let mine = [("authorization", owner.as_str())];
    let body = json!({ "address": "127.0.0.1", "port": port });
    let (status, asked) = send(&computer, "POST", "/api/v1/placement/pair-code", &mine, body).await;
    assert_eq!(status, StatusCode::CREATED, "{asked}");
    let id = asked["message"]["id"].as_str().unwrap().to_string();
    assert!(asked["message"].get("token").is_none(), "the token stays in the computer's gateway: {asked}");

    // The code as the centre's admin sees it (and the station says it, in the session mode).
    let code = centre.joins.list(crate::utils::time::now_ms())[0].code.clone().unwrap();
    let wrong = if code == "000000" { "111111" } else { "000000" };
    let uri = format!("/api/v1/placement/pair-code/{id}");
    let (status, refused) = send(&computer, "POST", &uri, &mine, json!({ "code": wrong })).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert!(refused["message"].as_str().unwrap_or_default().contains('2'), "two tries left: {refused}");

    let (status, joined) = send(&computer, "POST", &uri, &mine, json!({ "code": code })).await;
    assert_eq!(status, StatusCode::CREATED, "{joined}");
    assert_eq!(joined["message"]["joined"], true);
    let linked = computer.pc.station_gateway.at_address("127.0.0.1").expect("the station linked");
    assert_eq!(linked.record().map(|record| (record.port, record.managed)), Some((port, false)));
    assert!(admin.len() > 10);
}

#[cfg(feature = "yandex-station")]
#[tokio::test]
async fn a_station_with_its_own_duowave_becomes_the_centres_ear_by_its_own_code() {
    let serve = |app: std::sync::Arc<App>| async move {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(crate::app::serve(app, listener, std::future::pending()));
        port
    };
    let centre = station(&[]);
    let centre_port = serve(centre.clone()).await;
    let bedroom = station(&[]);
    let bedroom_port = serve(bedroom.clone()).await;

    let computer = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
    // This computer is the centre's Turbo already (its pair key, as pair-code gives it). Its people are the
    // home's from then on (the centre's copy): the owner signs in after the link.
    let key = centre.placement.pair_key().unwrap();
    computer.pc.station_gateway.link("127.0.0.1", centre_port, &key, None).await.unwrap();
    let owner = match computer.access.auth.setup("admin-password-1", "Я", "test").await.unwrap() {
        Some(signed) => signed.token,
        None => panic!("the computer kept people of its own after the link"),
    };
    let owner = format!("Bearer {owner}");
    let mine = [("authorization", owner.as_str())];

    let body = json!({ "address": "127.0.0.1", "port": bedroom_port, "kind": "ear" });
    let (status, asked) = send(&computer, "POST", "/api/v1/placement/pair-code", &mine, body).await;
    assert_eq!(status, StatusCode::CREATED, "{asked}");
    let id = asked["message"]["id"].as_str().unwrap().to_string();
    let request = bedroom.joins.list(crate::utils::time::now_ms())[0].clone();
    assert_eq!(request.kind, crate::home::join::JoinKind::Ear, "the bedroom says its own code");

    let uri = format!("/api/v1/placement/pair-code/{id}");
    let (status, joined) = send(&computer, "POST", &uri, &mine, json!({ "code": request.code.unwrap() })).await;
    assert_eq!(status, StatusCode::CREATED, "{joined}");
    assert_eq!((joined["message"]["joined"].as_bool(), joined["message"]["ear"].as_bool()), (Some(true), Some(true)));
    let ear = bedroom.settings.get(&crate::home::ear::HOME_EAR, None).expect("the bedroom is an ear now");
    assert_eq!((ear.address.as_str(), ear.port), ("127.0.0.1", centre_port));

    // (The computer's records are by address — here both stations are 127.0.0.1, one record: that the ear's record
    // stays the ear's is checked by placement::watch::tests and on pretend stations, docs/plans/pretend-stations.md.)

    // The centre lets in the voice of its ear's station: the ear's socket carries the centre's pair key. The
    // centre's own station is there already, so a stranger would wait for the admin.
    assert!(centre.stations.admit("h:centres-own", "127.0.0.1").ok);
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-uprx-device-id", axum::http::HeaderValue::from_static("BEDROOMSTATION"));
    assert!(
        crate::gateway::uniproxy::ear_bridge::dial(
            &bedroom.settings,
            &bedroom.secrets,
            &bedroom.config.gateway.uniproxy_path,
            &headers
        )
        .await
        .is_some(),
        "the centre took the ear's socket"
    );
    let approved = centre
        .stations
        .list()
        .into_iter()
        .filter(|station| station.view.status == crate::stations::StationStatus::Approved)
        .count();
    assert_eq!(approved, 2, "the ear's station approved by the join itself");
    let stranger = format!("ws://127.0.0.1:{centre_port}/uni.ws");
    let mut request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(stranger).unwrap();
    request.headers_mut().insert("x-uprx-device-id", axum::http::HeaderValue::from_static("STRANGER"));
    assert!(tokio_tungstenite::connect_async(request).await.is_err(), "a station without the key waits");
}
