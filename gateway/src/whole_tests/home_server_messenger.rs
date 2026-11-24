//! The tests of `home_server/messenger.rs` that need the whole gateway (`App`): its crate does not have it, the program
//! does.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use chrono::Utc;
use futures::future::BoxFuture;
use serde_json::{json, Value};

use crate::app::App;
use crate::config::{Config, Env};
use crate::core::availability::HomeServer;
use crate::helpers::contract::{Ability, HelperStatus};
use crate::helpers::remote::Messenger;
use crate::helpers::tests::{contract, stand, station, Stand, Station, ALL, KEY};
use crate::helpers::{DoorInput, HOME_COMPUTER};
use crate::home_server::messenger::*;
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::own::calls_skill::{CallsSkill, NoCalls, NO_RECIPIENT};
use crate::skills::skill::{ExecutionContext, OwnSkill};

/// The computer as the station sees it: a helper of its list, with its bot's contacts.
async fn computer(contacts: Option<&[&str]>) -> Arc<Stand> {
    let pc = stand(contract("pc", ALL)).await;
    *pc.contacts.lock().unwrap() = contacts.map(|names| names.iter().map(|name| name.to_string()).collect());
    pc
}

/// The station leaning on the computer at `url`, and its calls skill with no bot of its own.
fn station_on(url: &str) -> (Station, Arc<CallsSkill>) {
    let station = station(&[("HOME_SERVER_URL", url), ("HOME_SERVER_KEY", KEY)]);
    let none = Arc::new(NoCalls);
    let skill = CallsSkill::new(none.clone(), none.clone(), HomeMessenger::new(none, station.helpers.clone()));
    (station, skill)
}

async fn look(station: &Station) {
    station.helpers.check(HOME_COMPUTER).await.unwrap();
}

fn messenger(station: &Station) -> Messenger {
    let entry = station.helpers.entry(HOME_COMPUTER).unwrap();
    station.helpers.remote(&entry).unwrap().messenger()
}

fn message(contact: &str, text: &str) -> Intent {
    Intent {
        capability: CapabilityName::Calls,
        action: "message".into(),
        params: json!({ "contact": contact, "text": text }).as_object().cloned().unwrap(),
    }
}

async fn said(skill: &CallsSkill, intent: &Intent) -> String {
    let outcome = skill.execute(intent, &ExecutionContext::new("default", Utc::now())).await.unwrap();
    outcome.speech.unwrap_or_default()
}

/// Live 10.11 on the station: «Передай бабушке: купи хлеба.» answered «Не знаю, кому передать.» — the
/// station's build has no bot. Through the computer's it goes.
#[tokio::test]
async fn the_station_passes_a_message_on_through_the_computers_bot() {
    let pc = computer(Some(&["мама", "бабушка"])).await;
    let (station, skill) = station_on(&pc.url);
    let to_granny = message("Бабушке", "Купи хлеба.");
    assert!(!skill.ours(&to_granny, "default"), "the computer's contacts are not read yet");

    look(&station).await;
    assert_eq!(messenger(&station), Messenger::Contacts(vec!["мама".into(), "бабушка".into()]));
    assert!(skill.ours(&to_granny, "default"), "Бабушка is the computer's contact: ours, not Yandex's");
    let outcome = skill.execute(&to_granny, &ExecutionContext::new("default", Utc::now())).await.unwrap();
    assert_eq!(outcome.speech.as_deref(), Some("Передала."));
    assert_eq!(outcome.done.as_deref(), Some("контакту «бабушка» отправлено в Telegram сообщение «Купи хлеба.»"));
    assert_eq!(*pc.sent.lock().unwrap(), [("бабушка".to_string(), "Купи хлеба.".to_string())]);

    // Telegram refuses on the computer: said so.
    pc.refuses.store(true, Ordering::Relaxed);
    assert_eq!(said(&skill, &message("маме", "Я задержусь")).await, NOT_ACCEPTED);

    // «Только дома» never stops it: a helper at home is never kept out, and the bot's way to Telegram is no
    // road of the regulator's (the owner's answer, 15.11).
    pc.refuses.store(false, Ordering::Relaxed);
    station.helpers.set_away(crate::helpers::Away::Home, None).unwrap();
    assert_eq!(said(&skill, &message("маме", "Я задержусь")).await, "Передала.");
}

#[tokio::test]
async fn a_name_the_computer_does_not_know_is_not_ours_and_is_said_so() {
    let pc = computer(Some(&["мама"])).await;
    let (station, skill) = station_on(&pc.url);
    look(&station).await;
    let to_petya = message("Пете", "Привет");
    assert!(!skill.ours(&to_petya, "default"), "Yandex's, while it is up");
    assert_eq!(said(&skill, &to_petya).await, NO_RECIPIENT, "«Свой»: ours all the same");
    // The contact gone from the computer's bot since the last look: the computer's answer says so.
    *pc.contacts.lock().unwrap() = Some(vec!["папа".into()]);
    assert_eq!(said(&skill, &message("маме", "Привет")).await, NO_RECIPIENT);
    assert!(pc.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_computer_away_is_said_so_and_its_contacts_are_still_ours() {
    let pc = computer(Some(&["мама"])).await;
    let (station, skill) = station_on(&pc.url);
    look(&station).await;
    pc.set_away(true);
    look(&station).await;
    look(&station).await;
    assert_eq!(station.helpers.state(), HomeServer::Down);
    let to_mom = message("маме", "Я задержусь");
    assert!(skill.ours(&to_mom, "default"), "a contact seen before: ours, to say the computer is away");
    assert_eq!(said(&skill, &to_mom).await, AWAY);
    assert_eq!(said(&skill, &message("Пете", "Привет")).await, AWAY, "whoever it is for");

    // Up by the last look, gone by the send: the same words.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let (station, skill) = station_on(&closed);
    let entry = station.helpers.entry(HOME_COMPUTER).unwrap();
    let remote = station.helpers.remote(&entry).unwrap();
    remote.seen(&Ok(HelperStatus::read(&contract("pc", ALL)).unwrap()));
    remote.set_messenger(Messenger::Contacts(vec!["мама".into()]));
    assert_eq!(said(&skill, &to_mom).await, AWAY);
}

#[tokio::test]
async fn no_bot_on_the_computer_is_said_so_and_messages_not_taken_leave_it_alone() {
    let pc = computer(None).await;
    let (station, skill) = station_on(&pc.url);
    look(&station).await;
    assert_eq!((station.helpers.state(), messenger(&station)), (HomeServer::Up, Messenger::NoBot));
    let to_mom = message("маме", "Я задержусь");
    assert!(!skill.ours(&to_mom, "default"));
    assert_eq!(said(&skill, &to_mom).await, NO_BOT);

    // «Что даю колонке» without «Сообщения»: the station does not ask the computer, as with no computer.
    *pc.contacts.lock().unwrap() = Some(vec!["мама".into()]);
    look(&station).await;
    assert!(skill.ours(&to_mom, "default"));
    let parts = DoorInput { without: Some(vec!["messages".into()]), ..DoorInput::default() };
    station.helpers.door(parts, None).unwrap();
    assert!(!skill.ours(&to_mom, "default"));
    assert_eq!(said(&skill, &to_mom).await, NO_RECIPIENT);
    assert!(pc.sent.lock().unwrap().is_empty());
}

/// A computer of 0.11 and before answers its status without a contract and has the messenger's routes; an older
/// one does not: its 404 forgets messages, and it is never asked again.
#[tokio::test]
async fn an_older_computer_without_the_messengers_routes_passes_nothing() {
    let pc = stand(json!({ "version": "0.2.0", "peerApi": 2, "asr": true, "tts": true, "llm": false })).await;
    let (station, skill) = station_on(&pc.url);
    look(&station).await;
    let entry = station.helpers.entry(HOME_COMPUTER).unwrap();
    assert!(!station.helpers.remote(&entry).unwrap().declares(Ability::Messages), "0.2.0 had no messenger");
    assert_eq!(said(&skill, &message("маме", "Привет")).await, NO_RECIPIENT, "as with no computer");
    let old = stand(json!({ "version": "0.11.1", "peerApi": 2, "asr": true, "tts": true, "llm": false })).await;
    old.no_messenger.store(true, Ordering::SeqCst);
    let (station, _) = station_on(&old.url);
    look(&station).await;
    let remote = station.helpers.remote(&station.helpers.entry(HOME_COMPUTER).unwrap()).unwrap();
    assert!(!remote.declares(Ability::Messages), "its 404 forgets messages");
    look(&station).await;
    assert!(!remote.declares(Ability::Messages), "and its next status does not bring them back");
}

// ── The other way: the computer's bot's messages, said or played on the station. ──

const PAIR: &str = "0123456789abcdef0123456789abcdef";

/// What the station said, and the audio after it.
#[derive(Default)]
struct Spoken(Mutex<Vec<(String, Option<Vec<u8>>)>>);

impl crate::skills::own::reminders_skill::Speaker for Spoken {
    fn say(
        &self,
        _device: String,
        text: String,
        clip: Option<crate::skills::own::reminders_skill::AudioClip>,
    ) -> BoxFuture<'static, bool> {
        self.0.lock().unwrap().push((text, clip.map(|clip| clip.audio.to_vec())));
        Box::pin(async { true })
    }
}

/// The station's gateway (the build playing it), leaning on a computer, with the pair's key; its data in `dir`.
fn station_gateway(dir: &Path) -> (Arc<App>, Arc<Spoken>) {
    let data = dir.to_string_lossy().into_owned();
    let env = Env::from_pairs([
        ("STATE_DIR", ""),
        ("DATA_DIR", data.as_str()),
        ("PLUGINS_BUNDLED", "false"),
        ("DUOWAVE_ROLE", "station"),
        ("PEER_KEY", PAIR),
        ("HOME_SERVER_URL", "http://192.0.2.10"),
        ("HOME_SERVER_KEY", "vgw_home"),
    ]);
    let app = App::build(Config::for_tests(&env)).unwrap();
    let spoken = Arc::new(Spoken::default());
    app.skills.attach_speaker(spoken.clone());
    (app, spoken)
}

/// The computer's call from the home network: the pair's key, and the pass's headers when given.
async fn hand(app: &Arc<App>, key: Option<&str>, pass: &[(&str, &str)], body: Value) -> (StatusCode, Value) {
    use axum::extract::ConnectInfo;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let mut request =
        axum::http::Request::builder().method("POST").uri(MESSAGE_PATH).header("content-type", "application/json");
    if let Some(key) = key {
        request = request.header("authorization", format!("Bearer {key}"));
    }
    for (name, value) in pass {
        request = request.header(*name, *value);
    }
    let mut request = request.body(axum::body::Body::from(body.to_string())).unwrap();
    let home = std::net::SocketAddr::new("192.168.1.20".parse().unwrap(), 5000);
    request.extensions_mut().insert(ConnectInfo(home));
    let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn until(what: impl Fn() -> bool) -> bool {
    for _ in 0..300 {
        if what() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

/// Live 11.11: a text and a voice message to the bot, nothing played on the station. The computer hands them to
/// the station's gateway, which says the text and plays the voice after a word of who sent it.
#[tokio::test]
async fn the_station_says_a_text_and_plays_a_voice_message_the_computer_hands_it() {
    let dir = tempfile::tempdir().unwrap();
    let (app, spoken) = station_gateway(dir.path());
    let text = json!({ "from": "мама", "text": "Скоро буду", "at": "2026-11-11T10:00:00Z" });
    let (status, body) = hand(&app, Some(PAIR), &[], text).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(until(|| spoken.0.lock().unwrap().len() == 1).await);
    assert_eq!(spoken.0.lock().unwrap()[0], ("Мама пишет: Скоро буду".to_string(), None));

    use base64::Engine as _;
    let voice = json!({ "from": "мама", "voice": base64::engine::general_purpose::STANDARD.encode(b"OggS-voice") });
    let (status, body) = hand(&app, Some(PAIR), &[], voice).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(until(|| spoken.0.lock().unwrap().len() == 2).await);
    assert_eq!(spoken.0.lock().unwrap()[1], ("Голосовое сообщение: мама.".to_string(), Some(b"OggS-voice".to_vec())));
    // Its copy waited under the gateway's data folder, and is gone once played.
    let room = dir.path().join("tmp").join("messages");
    assert!(until(|| waiting(&room) == 0).await, "the copy is removed");
}

#[tokio::test]
async fn only_the_computer_of_the_pair_hands_messages_and_only_while_the_station_takes_them() {
    let dir = tempfile::tempdir().unwrap();
    let (app, spoken) = station_gateway(dir.path());
    let text = json!({ "from": "мама", "text": "Скоро буду" });
    let (status, body) = hand(&app, None, &[], text.clone()).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::FORBIDDEN, Some(only_pair().as_str())));
    let (status, _) = hand(&app, Some("another-key-0123456789abcdef0123"), &[], text.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // A guest at the computer, through the pass: not one who manages the stations.
    let guest = [(crate::placement::peer::HELPER_PERSON, "guest"), (crate::placement::peer::HELPER_ROLE, "guest")];
    let (status, _) = hand(&app, Some(PAIR), &guest, text.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = hand(&app, Some(PAIR), &[], json!({ "from": "мама" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    use base64::Engine as _;
    let long = base64::engine::general_purpose::STANDARD.encode(vec![0u8; VOICE_LIMIT + 1]);
    let (status, _) = hand(&app, Some(PAIR), &[], json!({ "from": "мама", "voice": long })).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

    // One switch for both ways: «Сообщения» off in «Что даю колонке» — the station does not take them.
    app.home_server.link(None, None, None, Some(vec!["messages".into()]), None).unwrap();
    let (status, body) = hand(&app, Some(PAIR), &[], text).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::CONFLICT, Some(not_taken().as_str())));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(spoken.0.lock().unwrap().is_empty(), "nothing said");
}
