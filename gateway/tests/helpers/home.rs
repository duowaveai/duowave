//! The home and the gateways' names end to end (docs/plans/home-and-helpers.md, block 1, stage 2): a new gateway makes
//! its home and its own id by itself and keeps them when it starts again; a person names it in its panel; and a station
//! whose settings live on the computer joins the computer's home with the first package while each keeps its own id.
//! No station, no network beyond 127.0.0.1: the station's gateway is a computer build playing it (DUOWAVE_ROLE=station).

use duowave::core::identity::home::{HOME_KEY, NAME_MAX};
use reqwest::StatusCode;
use serde_json::{json, Value};

use crate::support::{bearer, message, send, Gateway};

/// The home's id, this gateway's id and its name, as its /health says them to this computer.
async fn who(gateway: &Gateway) -> (String, String, String) {
    let health = message(gateway.get("/health"), StatusCode::OK).await;
    let text = |pointer: &str| health.pointer(pointer).and_then(Value::as_str).unwrap_or_default().to_string();
    (text("/home/id"), text("/gateway/id"), text("/gateway/name"))
}

fn uuid_v4(text: &str) -> bool {
    uuid::Uuid::parse_str(text).is_ok_and(|id| id.get_version_num() == 4)
}

/// The first admin's token.
async fn admin(gateway: &Gateway) -> String {
    let setup = gateway.post("/api/v1/auth/setup").json(&json!({"password": "home-admin-password"}));
    message(setup, StatusCode::CREATED).await["token"].as_str().expect("a token").to_string()
}

#[tokio::test]
async fn a_new_gateway_creates_its_home_and_its_name_by_itself() {
    let gateway = Gateway::start(&[]).await;
    let (home, me, name) = who(&gateway).await;
    assert!(uuid_v4(&home) && uuid_v4(&me) && home != me, "{home} {me}");
    assert!(name.starts_with("Компьютер"), "{name}");
    let key = gateway.app.secrets.get(HOME_KEY).expect("the home's key");
    let health = send(gateway.get("/health")).await.1;
    assert!(!health.to_string().contains(&key), "the home's key is nobody's to see");

    let token = admin(&gateway).await;
    let (header, as_admin) = bearer(&token);
    let view =
        message(gateway.get("/api/v1/gateway/self").header(header.clone(), as_admin.clone()), StatusCode::OK).await;
    assert_eq!(view, json!({"id": me, "name": name, "defaultName": name, "named": false, "home": {"id": home}}));
    let rename = |body: Value| gateway.put("/api/v1/gateway/self").header(header.clone(), as_admin.clone()).json(&body);
    let named = message(rename(json!({"name": " Кухня "})), StatusCode::OK).await;
    assert_eq!((&named["name"], &named["named"], &named["defaultName"]), (&json!("Кухня"), &json!(true), &json!(name)));
    assert_eq!(who(&gateway).await, (home.clone(), me.clone(), "Кухня".to_string()));
    let (status, body) = send(rename(json!({"name": "я".repeat(NAME_MAX + 1)}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(send(rename(json!({"name": 5}))).await.0, StatusCode::BAD_REQUEST);

    // A member sees it and does not rename it.
    let anya = json!({"name": "Аня", "password": "anya-password", "role": "member"});
    message(gateway.post("/api/v1/users").header(header.clone(), as_admin.clone()).json(&anya), StatusCode::CREATED)
        .await;
    let login = gateway.post("/api/v1/auth/login").json(&json!({"name": "Аня", "password": "anya-password"}));
    let member = bearer(message(login, StatusCode::OK).await["token"].as_str().unwrap());
    let (status, seen) = send(gateway.get("/api/v1/gateway/self").header(member.0.clone(), member.1.clone())).await;
    assert_eq!((status, &seen["message"]["name"]), (StatusCode::OK, &json!("Кухня")));
    let put = gateway.put("/api/v1/gateway/self").header(member.0, member.1).json(&json!({"name": "Моя"}));
    assert_eq!(send(put).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(gateway.get("/api/v1/gateway/self")).await.0, StatusCode::UNAUTHORIZED);

    // Started again on its folder: the same home and key, the same gateway, its name.
    let gateway = gateway.restart().await;
    assert_eq!(who(&gateway).await, (home.clone(), me.clone(), "Кухня".to_string()));
    assert_eq!(gateway.app.secrets.get(HOME_KEY), Some(key));
    let back = gateway.put("/api/v1/gateway/self").header(header, as_admin).json(&json!({"name": null}));
    let back = message(back, StatusCode::OK).await;
    assert_eq!((&back["name"], &back["named"]), (&json!(name), &json!(false)), "the default again");
    gateway.stop().await;
}

/// Default 8 end to end: a computer and a station's gateway, each in a home of its own. While the station keeps its
/// settings, the computer only copies them and both homes stay. Once they live on the computer, its first sending
/// carries the computer's home, and the station starts again in it — the same home id, the same key — keeping its own
/// id. The computer's watch is the computer's build only, and so is this test.
#[cfg(feature = "pc")]
#[tokio::test]
async fn after_the_first_sync_the_station_is_in_the_computers_home_and_keeps_its_own_id() {
    use std::sync::atomic::Ordering;

    use duowave::core::availability::Placement;
    use duowave::placement::transfer;
    use duowave::settings::definitions::{SettingsHome, StationPlacement};

    const PEER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    let pc = Gateway::start(&[]).await;
    let station = Gateway::start(&[("DUOWAVE_ROLE", "station"), ("PEER_KEY", PEER)]).await;
    // A package the station takes starts it again: counted (`Gateway::restarts`), and done by the test.
    let restarts = station.restarts.clone();
    let (pc_home, pc_id, pc_name) = who(&pc).await;
    let (station_home, station_id, station_name) = who(&station).await;
    assert!(pc_home != station_home && pc_id != station_id, "two gateways made two homes");
    assert_eq!(station_name, "Колонка", "a station whose model nobody told yet");

    // The computer watches the station's gateway with the pair's key; the settings live on the station: a copy.
    pc.app.secrets.set("placement:peer", Some(PEER)).unwrap();
    let record = StationPlacement {
        placement: Placement::Station,
        address: "127.0.0.1".into(),
        port: station.port,
        device: None,
        settings_home: SettingsHome::Station,
        auto_update: false,
        managed: true,
        home_key_id: None,
        moved_at: None,
    };
    let watch = pc.app.station_gateway.watch("at:127.0.0.1");
    watch.set_record(Some(&record), None).unwrap();
    watch.poll().await;
    assert_eq!(watch.view().backups.len(), 1, "{:?}", watch.live().error);
    let seen = watch.live().health.expect("the station's /health");
    assert_eq!((&seen["home"]["id"], &seen["gateway"]["id"]), (&json!(station_home), &json!(station_id)));
    assert_eq!((who(&pc).await.0, who(&station).await.0), (pc_home.clone(), station_home), "each its own");
    assert_eq!(restarts.load(Ordering::SeqCst), 0, "nothing sent");

    // The settings move to the computer: its next look sends them, the home with them, for the station's next start.
    watch.set_settings_home(SettingsHome::Pc, None).await.unwrap();
    watch.poll().await;
    assert_eq!(restarts.load(Ordering::SeqCst), 1, "{:?}", watch.live().sync_error);
    let state = station.app.config.skills.state_dir.clone().expect("the station's state folder");
    let kept = std::fs::read(transfer::incoming_file(&state)).expect("kept for its next start");
    let kept: transfer::Bundle = serde_json::from_slice(&kept).unwrap();
    assert_eq!((kept.schema, &kept.settings["home_identity"]["id"]), (transfer::SCHEMA, &json!(pc_home)));
    assert!(!kept.settings.contains_key("gateway_self"), "the computer's own id stays on it");

    let station = station.restart().await;
    let (home, id, _) = who(&station).await;
    assert_eq!(home, pc_home, "one home");
    assert!(id == station_id && id != pc_id, "the station keeps its own id: {id}");
    assert_eq!(station.app.secrets.get(HOME_KEY), pc.app.secrets.get(HOME_KEY), "one key");
    assert_eq!(who(&pc).await, (pc_home, pc_id, pc_name), "the computer as it was");

    // Its own sync told its model: its name says it, in the reader's language.
    let told = "INSERT INTO stations (device, name, status, first_seen, last_seen, device_model)
                VALUES ('h:station', 'Колонка', 'approved', 'then', 'then', 'plum')";
    station.app.db.with(|c| c.execute(told, [])).unwrap();
    assert_eq!(who(&station).await.2, "Колонка Лайт 2");
    let english = message(station.get("/health").header("accept-language", "en"), StatusCode::OK).await;
    assert_eq!(english["gateway"]["name"], json!("Lite 2 speaker"));
    station.stop().await;
    pc.stop().await;
}
