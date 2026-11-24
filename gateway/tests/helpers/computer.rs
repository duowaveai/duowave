//! The computer's side of «Дом» и помощники end to end: what only a computer's build has — its watch over the
//! station's home, its switch «ПК помогает колонке» — in the computer's test binary (tests/helpers_computer.rs).

use duowave::core::identity::home::HOME_KEY;
use reqwest::header::HeaderName;
use reqwest::StatusCode;
use serde_json::{json, Value};

use crate::support::{bearer, message, send, Gateway};

const PEER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Who a gateway says it is: its home's id, its own id and name.
async fn who(gateway: &Gateway) -> (String, String, String) {
    let health = message(gateway.get("/health"), StatusCode::OK).await;
    let text = |pointer: &str| health.pointer(pointer).and_then(Value::as_str).unwrap_or_default().to_string();
    (text("/home/id"), text("/gateway/id"), text("/gateway/name"))
}

/// A station's gateway with these more variables, and its admin.
async fn station(vars: &[(&str, &str)]) -> (Gateway, (HeaderName, String)) {
    let mut all = vec![("DUOWAVE_ROLE", "station"), ("PEER_KEY", PEER)];
    all.extend_from_slice(vars);
    let station = Gateway::start(&all).await;
    let auth = admin(&station).await;
    (station, auth)
}

/// The first admin's header.
async fn admin(gateway: &Gateway) -> (HeaderName, String) {
    let setup = gateway.post("/api/v1/auth/setup").json(&json!({"password": "helpers-admin-password"}));
    bearer(message(setup, StatusCode::CREATED).await["token"].as_str().expect("a token"))
}

/// The home's copy end to end (docs/plans/home-and-devices.md, stage 3): a computer and a station's gateway, each
/// made a home at its first start. The station is the home's centre: at its first look the computer takes the copy of
/// the station's home and applies it at once — the same home id and key — keeping its own id; the station starts
/// nothing again. A change at the centre reaches the copy at the next look. The computer's watch is the computer's
/// build only, and so is this test.
#[tokio::test]
async fn the_computer_takes_the_stations_home_as_its_copy_and_keeps_its_own_id() {
    use std::sync::atomic::Ordering;

    use duowave::core::availability::Placement;
    use duowave::placement::transfer;
    use duowave::settings::definitions::{SettingsHome, StationPlacement, TelegramSetting};

    const PEER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    let pc = Gateway::start(&[]).await;
    let station = Gateway::start(&[("DUOWAVE_ROLE", "station"), ("PEER_KEY", PEER)]).await;
    // A package the station takes starts it again: counted (`Gateway::restarts`), and done by the test.
    let restarts = station.restarts.clone();
    let (pc_home, pc_id, pc_name) = who(&pc).await;
    let (station_home, station_id, station_name) = who(&station).await;
    assert!(pc_home != station_home && pc_id != station_id, "two gateways made two homes");
    assert_eq!(station_name, "Колонка", "a station whose model nobody told yet");

    // Before it joined, the computer had a Telegram bot of its own (off here: no network in a test); the station's
    // home has none.
    let bot = TelegramSetting { enabled: false, alert_chat: Some("42".into()), ..TelegramSetting::default() };
    pc.app.connector_settings.take_home_telegram(&bot, Some("123:not-a-real-token")).unwrap();

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
    let watch = pc.app.pc.station_gateway.watch("at:127.0.0.1");
    watch.set_record(Some(&record), None).unwrap();
    watch.poll().await;
    assert!(watch.view().copied_at.is_some(), "{:?} {:?}", watch.live().error, watch.live().sync_error);
    let seen = watch.live().health.expect("the station's /health");
    assert_eq!((&seen["home"]["id"], &seen["gateway"]["id"]), (&json!(station_home), &json!(station_id)));
    let (home, id, _) = who(&pc).await;
    assert_eq!(home, station_home, "one home: the centre's");
    assert!(id == pc_id && id != station_id, "the computer keeps its own id: {id}");
    assert_eq!(pc.app.secrets.get(HOME_KEY), station.app.secrets.get(HOME_KEY), "one key");
    assert_eq!(who(&station).await.0, station_home, "the centre as it was");
    assert_eq!(restarts.load(Ordering::SeqCst), 0, "nothing sent to the station, no restart");

    // The computer's bot is the home's now, and the copy brought it back: the same bot on the computer.
    const TOKEN: &str = "connector:telegram:token";
    assert_eq!(station.app.secrets.get(TOKEN).as_deref(), Some("123:not-a-real-token"), "the home took the bot");
    assert_eq!(station.app.connector_settings.telegram_for_turbo().0, bot);
    assert_eq!(pc.app.secrets.get(TOKEN).as_deref(), Some("123:not-a-real-token"), "the bot stays on the computer");
    assert_eq!(pc.app.connector_settings.telegram_for_turbo().0, bot);
    // Only a device of the home hands its bot, and only while the home has none.
    let adopt = |key: Option<&str>| {
        let request = station.post("/api/v1/home/telegram-adopt").json(&json!({"token": "456:another"}));
        match key {
            Some(key) => request.bearer_auth(key),
            None => request,
        }
    };
    assert_eq!(send(adopt(None)).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(adopt(Some(PEER))).await.0, StatusCode::CONFLICT, "the home's bot stays");
    assert_eq!(station.app.secrets.get(TOKEN).as_deref(), Some("123:not-a-real-token"));

    // A change at the centre reaches the copy at the next look, applied at once.
    let set = |gateway: &Gateway| gateway.app.settings.get(&duowave::settings::definitions::KEEP_REQUESTS, None);
    station.app.settings.set(&duowave::settings::definitions::KEEP_REQUESTS, &9, None).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    watch.poll().await;
    assert_eq!(set(&pc), Some(9), "{:?}", watch.live().sync_error);
    assert_eq!(restarts.load(Ordering::SeqCst), 0);
    let _ = (pc_name, transfer::SCHEMA, SettingsHome::Station);

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

/// «ПК помогает колонке» from the computer's panel goes through the old door into the station's list.
#[tokio::test]
async fn the_pc_switch_puts_the_computer_into_the_stations_list_through_the_old_door() {
    use duowave::core::availability::Placement;
    use duowave::settings::definitions::{SettingsHome, StationPlacement};

    let pc = Gateway::start(&[]).await;
    let (name, pc_admin) = admin(&pc).await;
    let key = json!({"name": "Колонка: домашний сервер", "grants": ["home.models"]});
    let created =
        message(pc.post("/api/v1/api-keys").header(name.clone(), pc_admin.clone()).json(&key), StatusCode::CREATED)
            .await;
    let (station, _) = station(&[]).await;
    // A move brought the computer's address and key; its help is off.
    station.app.home_server.set(Some(&pc.base), created["secret"].as_str(), None).unwrap();
    station.app.home_server.link(None, None, Some(false), None, None).unwrap();
    assert!(!station.app.helpers.entry("home-computer").unwrap().enabled);

    pc.app.secrets.set("placement:peer", Some(PEER)).unwrap();
    let record = StationPlacement {
        placement: Placement::Station,
        address: "127.0.0.1".into(),
        port: station.port,
        device: None,
        settings_home: SettingsHome::Station,
        auto_update: false,
        managed: true,
        home_key_id: Some(created["key"]["id"].as_str().unwrap().into()),
        moved_at: None,
    };
    let watch = pc.app.pc.station_gateway.watch("at:127.0.0.1");
    watch.set_record(Some(&record), None).unwrap();
    watch.poll().await;
    // The computer took the home's copy (docs/plans/home-and-devices.md, stage 3): its people are the home's now —
    // the station's admin signs in here with the home's password; its own access keys stayed.
    let _ = pc_admin;
    let login = pc.post("/api/v1/auth/login").json(&json!({"password": "helpers-admin-password"}));
    let (name, pc_admin) = bearer(message(login, StatusCode::OK).await["token"].as_str().expect("a token"));
    let switch = pc.post("/api/v1/placement/helper").header(name, pc_admin).json(&json!({"on": true}));
    message(switch, StatusCode::CREATED).await;
    // The computer's address as it tells the station (its home network's, when it has one; this one else).
    let told = watch.home_url().unwrap_or_else(|| pc.base.clone());
    let door = station.app.helpers.entry("home-computer").unwrap();
    assert!(door.enabled && door.url == told, "{door:?}");
    let health = message(station.get("/health"), StatusCode::OK).await;
    assert_eq!(health["placement"]["placement"], "stationServer");
    assert_eq!(watch.record().unwrap().placement, Placement::StationServer);
    station.stop().await;
    pc.stop().await;
}
