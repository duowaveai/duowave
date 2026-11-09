//! The pair end to end (docs/plans/station-placement.md): two real gateways on free ports of 127.0.0.1 — the
//! computer's and the station's (a computer build playing it: DUOWAVE_ROLE=station) — and a pretend recognition server
//! behind the computer. The station leans on the computer with a key of the computer's (`home.models`); the computer
//! watches the station's gateway with the pair's key and keeps a copy of its settings; while they live on the
//! computer, the station's own panel does not change them. No station, no network beyond 127.0.0.1.

mod support;

use std::sync::Arc;

use axum::routing::{get, post};
use duowave::core::availability::{HomeServer, Placement};
use duowave::settings::definitions::{SettingsHome, StationPlacement};
use reqwest::StatusCode;
use serde_json::{json, Value};
use support::{bearer, message, send, Gateway};
use tokio_util::sync::CancellationToken;

const PEER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// The computer's own recognition server: whatever it hears is «включи свет».
async fn recognition() -> String {
    let router = axum::Router::new()
        .route("/v1/audio/transcriptions", post(|| async { axum::Json(json!({"text": "включи свет"})) }))
        .route("/v1/models", get(|| async { axum::Json(json!({"data": [{"id": "gigaam-v3"}]})) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}/v1")
}

async fn admin(gateway: &Gateway, password: &str) -> (reqwest::header::HeaderName, String) {
    let setup = message(gateway.post("/api/v1/auth/setup").json(&json!({"password": password})), StatusCode::CREATED);
    bearer(setup.await["token"].as_str().expect("a token"))
}

fn feature<'a>(view: &'a Value, id: &str) -> &'a Value {
    view["features"].as_array().unwrap().iter().find(|feature| feature["id"] == id).unwrap()
}

#[tokio::test]
async fn the_station_leans_on_the_computer_and_the_computer_keeps_a_copy_of_its_settings() {
    let asr = recognition().await;
    let pc = Gateway::start(&[("ASR_PROVIDER", "openai"), ("ASR_BASE_URL", asr.as_str())]).await;
    let (name, pc_admin) = admin(&pc, "pair-computer-password").await;
    // A key for the station's home server: only the right to the computer's models.
    let created = message(
        pc.post("/api/v1/api-keys")
            .header(name.clone(), pc_admin.clone())
            .json(&json!({"name": "Колонка: домашний сервер", "grants": ["home.models"]})),
        StatusCode::CREATED,
    )
    .await;
    let key = created["secret"].as_str().unwrap().to_string();

    let station = Gateway::start(&[
        ("DUOWAVE_ROLE", "station"),
        ("HOME_SERVER_URL", pc.base.as_str()),
        ("HOME_SERVER_KEY", key.as_str()),
        ("PEER_KEY", PEER),
        ("UNIPROXY_MODE", "gateway"),
    ])
    .await;
    station.app.home_server.check().await;
    assert_eq!(station.app.home_server.state(), HomeServer::Up);
    let (_, station_admin) = admin(&station, "pair-station-password").await;
    let view =
        message(station.get("/api/v1/availability").header(name.clone(), station_admin.clone()), StatusCode::OK).await;
    assert_eq!(view["placement"], "stationServer");
    assert_eq!(
        (feature(&view, "mode.own")["state"].as_str(), feature(&view, "mode.own")["runs"].as_str()),
        (Some("ready"), Some("homeServer"))
    );
    assert_eq!(feature(&view, "asr.own")["runs"], "homeServer");
    assert_eq!(feature(&view, "calls")["state"], "off");

    // Recognition through the computer, its own server behind it.
    let heard =
        station.app.home_server.recognizer().unwrap().recognize(&[0i16; 1600], 16000, &CancellationToken::new()).await;
    assert_eq!(heard.unwrap(), "включи свет");
    // Without the right, the computer's models are closed.
    let (status, _) = send(pc.get("/home/v1/status").header(name.clone(), pc_admin.clone())).await;
    assert_eq!(status, StatusCode::OK, "an admin holds every right");
    let (status, _) = send(pc.get("/home/v1/status")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The computer watches the station's gateway with the pair's key and keeps a copy of its settings.
    pc.app.secrets.set("placement:peer", Some(PEER)).unwrap();
    let record = StationPlacement {
        placement: Placement::StationServer,
        address: "127.0.0.1".into(),
        port: station.port,
        device: None,
        settings_home: SettingsHome::Station,
        auto_update: false,
        managed: true,
        home_key_id: Some(created["key"]["id"].as_str().unwrap().into()),
        moved_at: None,
    };
    let watch = pc.app.station_gateway.watch("at:127.0.0.1");
    watch.set_record(Some(&record), None).unwrap();
    watch.poll().await;
    let seen = watch.view();
    assert!(seen.live.reachable && seen.compatible, "{:?}", seen.live.error);
    assert_eq!(seen.backups.len(), 1, "the station's settings, a copy on the computer");
    let placement = message(pc.get("/api/v1/placement").header(name.clone(), pc_admin.clone()), StatusCode::OK).await;
    assert_eq!(placement["station"]["live"]["reachable"], true);
    assert_eq!(placement["station"]["live"]["health"]["placement"]["homeServer"]["state"], "up");

    // Another pair's key opens nothing.
    let (status, _) =
        send(station.get("/api/v1/transfer").header(name.clone(), "Bearer not-the-pairs-key-at-all-0000")).await;
    assert!(status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN, "{status}");

    // The settings live on the computer from now: the station's own panel does not change them; the computer may.
    message(
        pc.post("/api/v1/placement/settings-home").header(name.clone(), pc_admin.clone()).json(&json!({"home": "pc"})),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(station.app.placement.state().settings_home, SettingsHome::Pc);
    let (status, body) = send(
        station
            .put("/api/v1/station/routes")
            .header(name.clone(), station_admin.clone())
            .json(&json!({"routes": {"weather": "yandex"}})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["message"].as_str().unwrap().contains("пульте компьютера"));
    let _ = Arc::strong_count(&pc.app);
    station.stop().await;
    pc.stop().await;
}

#[tokio::test]
async fn the_station_takes_an_update_only_from_its_computer_and_only_signed() {
    let station = Gateway::start(&[("DUOWAVE_ROLE", "station"), ("PEER_KEY", PEER)]).await;
    let body =
        json!({"version": "9.9.9", "archive": "bm90IGFuIGFyY2hpdmU=", "signature": "untrusted comment: x\nRUQ=\n"});
    let (status, _) = send(station.post("/api/v1/updates/upload").json(&body)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "not from the computer of the pair");
    let (status, answer) =
        send(station.post("/api/v1/updates/upload").header("authorization", format!("Bearer {PEER}")).json(&body))
            .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(answer["message"].as_str().unwrap().contains("подпис"), "{answer}");
    station.stop().await;
}
