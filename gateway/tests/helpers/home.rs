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

/// The home's copy end to end (docs/plans/home-and-devices.md, stage 3): a computer and a station's gateway, each
/// made a home at its first start. The station is the home's centre: at its first look the computer takes the copy of
/// the station's home and applies it at once — the same home id and key — keeping its own id; the station starts
/// nothing again. A change at the centre reaches the copy at the next look. The computer's watch is the computer's
/// build only, and so is this test.
#[cfg(feature = "pc")]
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
    let watch = pc.app.station_gateway.watch("at:127.0.0.1");
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

/// Two stations, one home (docs/plans/home-and-devices.md, stage 4): the second station's gateway becomes an ear of
/// the first — the centre — with its pair key. It takes the home's copy at once, the centre counts it among the
/// holders, its panel shows the centre's home, and a change of the home made on it is refused with words (it would be
/// lost at the next copy). Asked by its pair, it becomes the centre: an ear no more.
#[tokio::test]
async fn a_second_station_is_an_ear_of_the_centre_with_a_copy_and_becomes_the_centre_on_its_word() {
    const KITCHEN: &str = "kitchen-pair-key-0123456789abcdef0123456789abcdef";
    const BEDROOM: &str = "bedroom-pair-key-0123456789abcdef0123456789abcdef";
    let kitchen = Gateway::start(&[("DUOWAVE_ROLE", "station"), ("PEER_KEY", KITCHEN)]).await;
    let bedroom = Gateway::start(&[("DUOWAVE_ROLE", "station"), ("PEER_KEY", BEDROOM)]).await;
    let (header, kitchen_admin) = bearer(&admin(&kitchen).await);
    let (_, bedroom_admin) = bearer(&admin(&bedroom).await);
    let (kitchen_home, kitchen_id, _) = who(&kitchen).await;
    kitchen.app.settings.set(&duowave::settings::definitions::KEEP_REQUESTS, &9, None).unwrap();
    let _ = kitchen_admin;

    // The bedroom's admin makes it the kitchen's ear with the kitchen's pair key.
    let join = json!({"address": "127.0.0.1", "port": kitchen.port, "key": KITCHEN, "name": "Кухня"});
    message(bedroom.put("/api/v1/home/ear").header(header.clone(), bedroom_admin.clone()).json(&join), StatusCode::OK)
        .await;
    let kept = bedroom.app.settings.get(&duowave::settings::definitions::KEEP_REQUESTS, None);
    assert_eq!(kept, Some(9), "the home's copy at once");
    assert_eq!(who(&bedroom).await.0, kitchen_home, "one home");
    let bedroom_id = bedroom.app.identity.gateway().unwrap().id;
    assert!(duowave::home::copy::holds(&kitchen.app.settings, &bedroom_id), "the centre counts the ear's copy");

    // Its panel shows the centre's home; the bedroom signs in with the home's people now.
    let login = bedroom.post("/api/v1/auth/login").json(&json!({"password": "home-admin-password"}));
    let (_, as_home) = bearer(message(login, StatusCode::OK).await["token"].as_str().unwrap());
    let devices =
        message(bedroom.get("/api/v1/home/devices").header(header.clone(), as_home.clone()), StatusCode::OK).await;
    let centre =
        devices["devices"].as_array().unwrap().iter().find(|device| device["id"] == devices["centre"]).unwrap().clone();
    assert_eq!(centre["gatewayId"], json!(kitchen_id), "{devices}");

    // A change of the home on the ear is refused with words; its own things pass.
    let change = bedroom.put("/api/v1/requests/keep").header(header.clone(), as_home.clone()).json(&json!({"days": 3}));
    let (status, body) = send(change).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["message"].as_str().unwrap().contains("Кухня"), "{body}");

    // The bedroom station's voice socket goes on to the centre: the kitchen serves its session.
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = format!("ws://127.0.0.1:{}/uni.ws", bedroom.port).into_client_request().unwrap();
    request.headers_mut().insert("x-uprx-device-id", "BEDROOM01".parse().unwrap());
    let (socket, _) = tokio_tungstenite::connect_async(request).await.expect("the ear takes the station's socket");
    let mut served = false;
    for _ in 0..50 {
        if kitchen.app.sessions.active_count() == 1 {
            served = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
    assert!(served, "the centre serves the bedroom's session");
    assert_eq!(bedroom.app.sessions.active_count(), 0, "the ear only passes it on");
    drop(socket);

    // The centre away: the ear answers itself, from the home's copy.
    kitchen.stop().await;
    let mut request = format!("ws://127.0.0.1:{}/uni.ws", bedroom.port).into_client_request().unwrap();
    request.headers_mut().insert("x-uprx-device-id", "BEDROOM01".parse().unwrap());
    let (socket, _) = tokio_tungstenite::connect_async(request).await.expect("the ear takes it all the same");
    let mut here = false;
    for _ in 0..100 {
        if bedroom.app.sessions.active_count() == 1 {
            here = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
    assert!(here, "served here while the centre is away");
    drop(socket);

    // Its pair asks it to become the centre: it is its own again.
    let take_over = bedroom.post("/api/v1/home/become-centre").header(header.clone(), format!("Bearer {BEDROOM}"));
    let answer = message(take_over, StatusCode::OK).await;
    assert_eq!(answer["key"], json!(BEDROOM));
    let ear = message(bedroom.get("/api/v1/home/ear").header(header.clone(), as_home.clone()), StatusCode::OK).await;
    assert!(ear["earOf"].is_null(), "{ear}");
    let (status, _) =
        send(bedroom.put("/api/v1/requests/keep").header(header, as_home).json(&json!({"days": 3}))).await;
    assert!(status.is_success(), "the centre now: the home is written here");
    bedroom.stop().await;
}

/// «Сделать центром» (docs/plans/home-and-devices.md, stage 4): at the centre, its admin makes the bedroom's station the
/// centre — over the home network with the pair's keys, no SSH: the bedroom takes the home's last revision and is the
/// centre, the kitchen becomes its ear and the home is written at the bedroom from now on.
#[tokio::test]
async fn the_centre_moves_to_its_ear_and_becomes_that_ones_ear() {
    use duowave::settings::definitions::{StationSelf, KEEP_REQUESTS, STATION_SELF};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    const KITCHEN: &str = "kitchen-pair-key-0123456789abcdef0123456789abcdef";
    const BEDROOM: &str = "bedroom-pair-key-0123456789abcdef0123456789abcdef";
    let kitchen = Gateway::start(&[("DUOWAVE_ROLE", "station"), ("PEER_KEY", KITCHEN)]).await;
    let bedroom = Gateway::start(&[("DUOWAVE_ROLE", "station"), ("PEER_KEY", BEDROOM)]).await;
    let (header, kitchen_admin) = bearer(&admin(&kitchen).await);
    let (_, bedroom_admin) = bearer(&admin(&bedroom).await);
    // The bedroom knows its station (a move or the first steps wrote it).
    let own = StationSelf { device: Some("BEDROOM01".into()), name: Some("Спальня".into()) };
    bedroom.app.settings.set(&STATION_SELF, &own, None).unwrap();
    let join = json!({"address": "127.0.0.1", "port": kitchen.port, "key": KITCHEN, "name": "Кухня"});
    message(bedroom.put("/api/v1/home/ear").header(header.clone(), bedroom_admin).json(&join), StatusCode::OK).await;
    // Its station spoke through it once: the centre knows it and its address.
    let mut request = format!("ws://127.0.0.1:{}/uni.ws", bedroom.port).into_client_request().unwrap();
    request.headers_mut().insert("x-uprx-device-id", "BEDROOM01".parse().unwrap());
    let (socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    for _ in 0..50 {
        if kitchen.app.stations.find("BEDROOM01").is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
    drop(socket);
    kitchen.app.settings.set(&KEEP_REQUESTS, &11, None).unwrap();

    let make = kitchen
        .post("/api/v1/home/centre")
        .header(header.clone(), kitchen_admin.clone())
        .json(&json!({"device": "BEDROOM01"}));
    let moved = message(make, StatusCode::OK).await;
    assert_eq!(moved["centre"]["port"], json!(bedroom.port));
    assert_eq!(bedroom.app.settings.get(&KEEP_REQUESTS, None), Some(11), "the newest of the home went first");
    assert!(bedroom.app.settings.get(&duowave::home::ear::HOME_EAR, None).is_none(), "the bedroom is the centre");
    let ear = kitchen.app.settings.get(&duowave::home::ear::HOME_EAR, None).expect("the kitchen is its ear");
    assert_eq!(ear.port, bedroom.port);
    let (status, _) =
        send(kitchen.put("/api/v1/requests/keep").header(header.clone(), kitchen_admin).json(&json!({"days": 3})))
            .await;
    assert_eq!(status, StatusCode::CONFLICT, "the home is written at the bedroom now");
    bedroom.stop().await;
    kitchen.stop().await;
}
