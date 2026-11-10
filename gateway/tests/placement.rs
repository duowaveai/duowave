//! The pair end to end (docs/plans/station-placement.md): two real gateways on free ports of 127.0.0.1 — the
//! computer's and the station's (a computer build playing it: DUOWAVE_ROLE=station) — and a pretend recognition server
//! behind the computer. The station leans on the computer with a key of the computer's (`home.models`); the computer
//! watches the station's gateway with the pair's key and keeps a copy of its settings; while they live on the
//! computer, the station's own panel does not change them. No station, no network beyond 127.0.0.1. The computer's
//! side (its watch over the station) is in the computer's build only, and so is this test.

#![cfg(feature = "pc")]

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::routing::{get, post};
use duowave::config::TtsProvider;
use duowave::core::availability::{HomeServer, Placement};
use duowave::placement::transfer;
use duowave::settings::definitions::{SettingsHome, StationPlacement, KEEP_REQUESTS};
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
    // A package the station takes would start it again: counted here, not done.
    let restarts = Arc::new(AtomicUsize::new(0));
    let counted = restarts.clone();
    station.app.placement.set_restarter(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
    }));
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
    // The schema its import knows: the computer sends it what that import takes.
    assert_eq!(placement["station"]["live"]["health"]["build"]["settingsSchema"], transfer::SCHEMA);

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
    // Review G1: a character chosen and the journal's days travel too; the mode is the station's own and goes on.
    let changes = [
        (station.post("/api/v1/characters/alice/choose"), json!({})),
        (station.put("/api/v1/requests/keep"), json!({"days": 7})),
    ];
    for (request, body) in changes {
        let (status, answer) = send(request.header(name.clone(), station_admin.clone()).json(&body)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{answer}");
    }
    let mode = station.post("/api/v1/station/mode").header(name.clone(), station_admin.clone());
    let (status, body) = send(mode.json(&json!({"mode": "gateway"}))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // «Вернуть настройки на колонку»: an edit of the computer not sent yet goes first — the routine package, the
    // station's /health says it knows schema 2 — and waits there for its next start; only then do the settings live on
    // the station again.
    let before = restarts.load(Ordering::SeqCst);
    pc.app.settings.set(&KEEP_REQUESTS, &9, None).unwrap();
    let back = pc.post("/api/v1/placement/settings-home").header(name.clone(), pc_admin.clone());
    message(back.json(&json!({"home": "station"})), StatusCode::CREATED).await;
    let state = station.app.config.skills.state_dir.clone().expect("the station's state folder");
    let kept = std::fs::read(transfer::incoming_file(&state)).expect("kept for its next start");
    let kept: transfer::Bundle = serde_json::from_slice(&kept).unwrap();
    assert_eq!((kept.schema, &kept.settings["requests_keep_days"]), (transfer::SCHEMA, &json!(9)));
    assert!(!kept.settings.contains_key("gateway_mode"), "the station's mode is its own");
    assert!(restarts.load(Ordering::SeqCst) > before, "it would start again on it");
    assert_eq!(station.app.placement.state().settings_home, SettingsHome::Station);
    assert_eq!(watch.record().unwrap().settings_home, SettingsHome::Station);
    let _ = Arc::strong_count(&pc.app);
    station.stop().await;
    pc.stop().await;
}

/// The station's «Голос» while the computer gives it its voice (docs/plans/desk-after-station.md, В): the station's
/// build has no model shop, so its window offers the computer's voice by its name or Alice's — the station's own
/// choice, set through its API and kept for its next start. The computer's own gateway chooses in its shop.
#[tokio::test]
async fn the_station_chooses_between_the_computers_voice_and_alices() {
    let pc = Gateway::start(&[("TTS_OWN_URL", "http://127.0.0.1:9/v1"), ("TTS_OWN_VOICE", "baya")]).await;
    let (name, pc_admin) = admin(&pc, "pair-computer-password").await;
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
    ])
    .await;
    station.app.home_server.check().await;
    let (_, station_admin) = admin(&station, "pair-station-password").await;
    let view =
        message(station.get("/api/v1/models/home-voice").header(name.clone(), station_admin.clone()), StatusCode::OK)
            .await;
    assert_eq!(
        (&view["offered"], &view["up"], &view["voice"], &view["chosen"]),
        (&json!(true), &json!(true), &json!("Baya"), &json!("alice")),
        "{view}"
    );
    let chosen = message(
        station
            .post("/api/v1/models/home-voice")
            .header(name.clone(), station_admin.clone())
            .json(&json!({"voice": "computer"})),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!((&chosen["chosen"], &chosen["provider"]), (&json!("computer"), &json!("mixed")));
    assert_eq!(station.app.voice.provider(true), TtsProvider::Mixed, "«Шлюз»: our answers in the computer's voice");
    assert_eq!(station.app.settings.raw("tts_provider"), Some(json!("mixed")), "kept for its next start");
    message(
        station
            .post("/api/v1/models/home-voice")
            .header(name.clone(), station_admin.clone())
            .json(&json!({"voice": "alice"})),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(station.app.voice.provider(true), TtsProvider::Yandex);

    let (status, _) = send(
        pc.post("/api/v1/models/home-voice").header(name.clone(), pc_admin.clone()).json(&json!({"voice": "alice"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "the computer chooses its voice in its shop");
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

/// «передай …» on the station (live 10.11: «Не знаю, кому передать.»): its build has no Telegram bot, so the
/// computer's passes the message on (src/home_server/messenger.rs). This computer has no bot set up: the station
/// asks it with its home-server key and says so; with the computer gone, it says that. The send itself, to a pretend
/// computer and a pretend Bot API: the unit tests of messenger.rs and serve.rs.
#[tokio::test]
async fn the_station_passes_messages_through_the_computers_bot_and_says_why_it_cannot() {
    use duowave::home_server::messenger::{AWAY, NO_BOT};
    use duowave::home_server::Messenger;
    use duowave::skills::capabilities::{CapabilityName, Intent};
    use duowave::skills::{ExecutionContext, OwnSkill};

    let pc = Gateway::start(&[]).await;
    let (name, pc_admin) = admin(&pc, "pair-computer-password").await;
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
    ])
    .await;
    station.app.home_server.check().await;
    assert_eq!(station.app.home_server.state(), HomeServer::Up);
    assert_eq!(station.app.home_server.messenger(), Messenger::NoBot, "read with the look at the computer");

    // The computer's routes: its key only, and the reason in words.
    let (status, body) =
        send(pc.get("/home/v1/messenger/contacts").header(name.clone(), format!("Bearer {key}"))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body["message"].as_str().unwrap().contains("не настроен бот Telegram"), "{body}");
    let (status, _) =
        send(pc.post("/home/v1/messenger/send").json(&json!({"contact": "мама", "text": "Я задержусь"}))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let skill = station.app.skills.calls.clone();
    let to_mom = Intent {
        capability: CapabilityName::Calls,
        action: "message".into(),
        params: json!({"contact": "маме", "text": "Я задержусь"}).as_object().cloned().unwrap(),
    };
    let said = |intent: Intent| {
        let skill = skill.clone();
        async move {
            let context = ExecutionContext::new("default", chrono::Utc::now());
            skill.execute(&intent, &context).await.unwrap().speech.unwrap_or_default()
        }
    };
    assert!(!skill.ours(&to_mom, "default"), "no bot anywhere: Yandex's while it is up");
    assert_eq!(said(to_mom.clone()).await, NO_BOT);

    // The computer goes: two looks without it, and the station says it has nobody to pass the message to.
    pc.stop().await;
    station.app.home_server.check().await;
    station.app.home_server.check().await;
    assert_eq!(station.app.home_server.state(), HomeServer::Down);
    assert_eq!(said(to_mom).await, AWAY);
    station.stop().await;
}

/// What the station said, and how much audio after it.
#[derive(Default)]
struct Spoken(std::sync::Mutex<Vec<(String, usize)>>);

impl duowave::skills::own::reminders_skill::Speaker for Spoken {
    fn say(
        &self,
        _device: String,
        text: String,
        clip: Option<duowave::skills::own::reminders_skill::AudioClip>,
    ) -> futures::future::BoxFuture<'static, bool> {
        self.0.lock().unwrap().push((text, clip.map_or(0, |clip| clip.audio.len())));
        Box::pin(async { true })
    }
}

/// The bot's messages the other way (live 11.11: a text and a voice message to the bot, nothing played — the station
/// was not at the computer's /uni.ws). The computer hands them to the station's gateway with the pair's key; the
/// station says the text and plays the voice. «Сообщения» off in «Что даю колонке» — the station's /health says so —
/// and nothing goes: one switch for both ways.
#[tokio::test]
async fn the_bots_messages_reach_the_station_the_computer_helps_while_it_takes_messages() {
    use duowave::skills::own::calls_skill::IncomingMessage;

    let pc = Gateway::start(&[]).await;
    let station = Gateway::start(&[
        ("DUOWAVE_ROLE", "station"),
        ("HOME_SERVER_URL", pc.base.as_str()),
        ("HOME_SERVER_KEY", "vgw_not_needed_here"),
        ("PEER_KEY", PEER),
    ])
    .await;
    let spoken = Arc::new(Spoken::default());
    station.app.skills.attach_speaker(spoken.clone());

    // The computer helps the station («Колонка + ПК») and knows its gateway's /health.
    pc.app.secrets.set("placement:peer", Some(PEER)).unwrap();
    let record = StationPlacement {
        placement: Placement::StationServer,
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
    assert!(watch.live().reachable, "{:?}", watch.live().error);

    // What the bot hears, as it tells the calls skill (links/calls.rs): a text, then a voice message from its inbox.
    let inbox = tempfile::tempdir().unwrap();
    let file = inbox.path().join("voice.ogg");
    std::fs::write(&file, vec![7u8; 4096]).unwrap();
    let from_mom = |text: Option<&str>, file: Option<std::path::PathBuf>| IncomingMessage {
        from: "мама".into(),
        chat_id: "42".into(),
        file,
        text: text.map(str::to_string),
        discard: false,
    };
    pc.app.skills.calls.on_message(from_mom(Some("Скоро буду"), None));
    let heard = |count: usize| {
        let spoken = spoken.clone();
        async move {
            for _ in 0..300 {
                if spoken.0.lock().unwrap().len() >= count {
                    return true;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            false
        }
    };
    assert!(heard(1).await, "the text reaches the station");
    pc.app.skills.calls.on_message(from_mom(None, Some(file.clone())));
    assert!(heard(2).await, "the voice message too");
    assert_eq!(
        *spoken.0.lock().unwrap(),
        [("Мама пишет: Скоро буду".to_string(), 0), ("Голосовое сообщение: мама.".to_string(), 4096)]
    );
    assert!(file.exists(), "the computer's inbox keeps it");
    let served = duowave::home_server::serve::served_today();
    assert!(served["messages"]["count"].as_u64().unwrap_or_default() >= 2, "{served}");

    // «Сообщения» off on the station: the computer reads it from its /health and hands nothing on.
    use duowave::placement::messages::takes_messages;
    assert!(takes_messages(&record, watch.live().health.as_ref()), "given until now");
    station.app.home_server.link(None, None, None, Some(vec!["messages".into()]), None).unwrap();
    watch.poll().await;
    assert!(!takes_messages(&record, watch.live().health.as_ref()), "the computer sees the part off");
    pc.app.skills.calls.on_message(from_mom(Some("Ещё одно"), None));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(spoken.0.lock().unwrap().len(), 2, "nothing more");
    station.stop().await;
    pc.stop().await;
}
