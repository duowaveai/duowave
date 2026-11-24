//! The tests of `home_server/serve.rs` that need the whole gateway (`App`): its crate does not have it, the program
//! does.

use std::sync::Arc;

use axum::http::StatusCode;
use axum::routing::post;
use serde_json::{json, Value};

use crate::core::identity::permissions::HOME_MODELS;
use crate::helpers::contract::{Ability, Contract};

#[test]
fn only_the_owners_own_recording_counts_as_a_voice_on_top() {
    assert!(crate::home_server::serve::own_on_top("own:v-mine"));
    assert!(
        !crate::home_server::serve::own_on_top("own:c-p-0a1b2c3d4e5f-lena-f5"),
        "a character's voice is not on top of itself"
    );
    assert!(!crate::home_server::serve::own_on_top("silero-ru"), "a ready-made voice is chosen, not put on top");
}

#[test]
fn the_form_an_openai_client_sends_comes_apart_by_name() {
    let body = b"--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFF\x00\x01\r\n--XyZ\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nhome\r\n--XyZ--\r\n";
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(axum::http::header::CONTENT_TYPE, "multipart/form-data; boundary=XyZ".parse().unwrap());
    let parts = crate::home_server::serve::form_parts(&headers, body);
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].0, "file");
    assert_eq!(parts[0].1, b"RIFF\x00\x01");
    assert_eq!(parts[1], ("model".to_string(), b"home".to_vec()));
    assert!(crate::home_server::serve::form_parts(&axum::http::HeaderMap::new(), body).is_empty());
}

use std::net::SocketAddr;
use std::sync::Mutex as StdMutex;

use axum::body::Body as HttpBody;
use axum::extract::ConnectInfo;
use axum::http::Request;
use http_body_util::BodyExt;
use serde_json::Map;
use tower::ServiceExt;

use crate::app::App;
use crate::computer::calls::TelegramMessenger;
use crate::config::{Config, Env, TelegramConfig};
use crate::helpers::contract::{HelperIdentity, HelperStatus};
use crate::home_server::serve::*;

/// The computer's gateway, its .env with these; its bot never started (no Telegram here).
fn computer(vars: &[(&str, &str)]) -> Arc<App> {
    let env = Env::from_pairs([("STATE_DIR", "")].into_iter().chain(vars.iter().copied()));
    App::build(Config::for_tests(&env)).unwrap()
}

fn key(app: &App, grants: &[&str]) -> String {
    let input = Map::from_iter([("name".into(), json!("Колонка: домашний сервер")), ("grants".into(), json!(grants))]);
    format!("Bearer {}", app.access.keys.create(&input, None).unwrap().secret)
}

/// A request from the home network, with the key when given.
async fn send(app: &Arc<App>, method: &str, uri: &str, key: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri).header("content-type", "application/json");
    if let Some(key) = key {
        request = request.header("authorization", key);
    }
    let mut request = request.body(HttpBody::from(body.to_string())).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new("192.168.1.20".parse().unwrap(), 5000)));
    let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

const CONTACTS: &str = "/home/v1/messenger/contacts";
const SEND: &str = "/home/v1/messenger/send";
const STATUS: &str = "/home/v1/status";

#[test]
fn a_piper_voice_says_its_language_and_the_other_voices_none() {
    assert_eq!(piper_language("piper/ru_RU-irina-medium").as_deref(), Some("ru"));
    assert_eq!(piper_language("piper/en_US-amy-medium").as_deref(), Some("en"));
    assert_eq!(piper_language("silero/baya"), None);
    assert_eq!(piper_language("piper/denis"), None, "a voice by its name alone");
    assert_eq!(piper_language("omnivoice/3f2a9c1e-77b0-4c1d-9a55-0c1d2e3f4a5b"), None, "a voice lab's id");
    assert_eq!(piper_language("piper/RU_ru-irina-medium"), None);
}

#[tokio::test]
async fn the_computer_answers_its_status_with_the_contract_and_the_old_fields() {
    let app = computer(&[
        ("ASR_BASE_URL", "http://127.0.0.1:8090/v1"),
        ("ASR_LANGUAGE", "ru"),
        ("TTS_OWN_URL", "http://127.0.0.1:8091/v1"),
        ("TTS_OWN_MODEL", "piper"),
        ("TTS_OWN_VOICE", "ru_RU-irina-medium"),
    ]);
    let station = key(&app, &["home.models"]);
    let (status, body) = send(&app, "GET", STATUS, Some(&station), Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The fields a station of 0.11 reads, as they were.
    assert_eq!((&body["version"], &body["peerApi"]), (&json!(crate::VERSION), &json!(crate::core::build::PEER_API)));
    assert_eq!((&body["asr"], &body["tts"], &body["llm"]), (&json!(true), &json!(true), &json!(false)));
    assert_eq!(body["voice"], "piper/ru_RU-irina-medium");

    // And what it gives as a helper, by the contract.
    assert_eq!(body["contract"]["version"], crate::helpers::contract::VERSION);
    let read = HelperStatus::read(&body).unwrap();
    assert_eq!(read.program.as_deref(), Some(crate::VERSION));
    let contract = &read.contract;
    assert_eq!(serde_json::to_value(contract).unwrap(), body["contract"], "read as it was written");
    // This gateway as a helper: its own id and name (core/identity/home.rs), made at its start.
    let me = app.identity.gateway().expect("made at the start");
    assert_eq!((contract.helper.id.as_str(), contract.helper.name.as_str()), (me.id.as_str(), me.name.as_str()));
    let hearing = contract.get(Ability::Hearing).unwrap();
    assert!(hearing.ready, "its recognition server");
    assert_eq!(hearing.details, Map::from_iter([("languages".to_string(), json!(["ru"]))]));
    let voice = contract.get(Ability::Voice).unwrap();
    assert!(voice.ready);
    assert_eq!(voice.details["voice"], "piper/ru_RU-irina-medium");
    assert_eq!(voice.details["languages"], json!(["ru"]));
    assert!(!contract.declares(Ability::Model), "no model of its own");
    assert!(contract.declares(Ability::Messages) && !contract.ready(Ability::Messages), "no bot set up here");
    assert!(contract.ready(Ability::Apps));
    assert_eq!(contract.declares(Ability::History), app.history.is_some());
    assert!(!contract.declares(Ability::Catalog), "no feed here (no PACKAGES_FEED_URL, none built in): no copy");
    reads_the_same_without_the_contract(&body);

    // A model of its own at home, an id and a name of its own: the contract says them, the old field too.
    app.db
        .with(|c| {
            c.execute(
                "INSERT INTO llm_providers (id, preset, kind, title, base_url, created_at, updated_at)
                 VALUES ('ollama', 'ollama', 'openai', 'Ollama', 'http://127.0.0.1:11434/v1', ?1, ?1)",
                ["2026-11-15T10:00:00.000Z"],
            )
        })
        .unwrap();
    app.identity.rename(Some("Кабинет"), None).unwrap();
    let (_, body) = send(&app, "GET", STATUS, Some(&station), Value::Null).await;
    assert_eq!(body["llm"], true);
    let read = HelperStatus::read(&body).unwrap().contract;
    assert!(read.ready(Ability::Model));
    assert_eq!(read.helper, HelperIdentity { id: me.id.clone(), name: "Кабинет".into() });
    reads_the_same_without_the_contract(&body);

    // The contract is behind the same door as the rest of /home/v1.
    let (status, _) = send(&app, "GET", STATUS, None, Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// The answer read again without its `contract`, as a computer of 0.11 answers it: version 0 with the parts the
/// contract declares — hearing, voice, the model and the apps as ready, the voice by its name — and the messages.
/// What version 0 cannot know is left out: whether the bot is set up (its contacts' read says it), the history and
/// the catalog (taken by the release).
fn reads_the_same_without_the_contract(body: &Value) {
    let read = HelperStatus::read(body).unwrap().contract;
    let mut old = body.clone();
    old.as_object_mut().unwrap().remove("contract");
    let old = HelperStatus::read(&old).unwrap().contract;
    assert_eq!(old.version, 0);
    for ability in [Ability::Hearing, Ability::Voice, Ability::Model, Ability::Apps] {
        let both = |contract: &Contract| (contract.declares(ability), contract.ready(ability));
        assert_eq!(both(&old), both(&read), "{ability:?} in {body}");
    }
    let voice =
        |contract: &Contract| contract.get(Ability::Voice).and_then(|voice| voice.details.get("voice")).cloned();
    assert_eq!(voice(&old), voice(&read));
    assert!(old.declares(Ability::Messages) && read.declares(Ability::Messages));
}

#[tokio::test]
async fn a_computer_without_a_voice_or_a_model_of_its_own_declares_neither_and_so_reads_without_the_contract() {
    // Its recognition server (ASR_BASE_URL at home by default), no TTS_OWN_URL, no model at home.
    let app = computer(&[]);
    let station = key(&app, &["home.models"]);
    let (status, body) = send(&app, "GET", STATUS, Some(&station), Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (&body["asr"], &body["tts"], &body["voice"], &body["llm"]),
        (&json!(true), &json!(false), &Value::Null, &json!(false))
    );
    let contract = HelperStatus::read(&body).unwrap().contract;
    assert!(contract.ready(Ability::Hearing));
    assert!(!contract.declares(Ability::Voice) && !contract.declares(Ability::Model), "{body}");
    reads_the_same_without_the_contract(&body);
}

#[tokio::test]
async fn a_computer_whose_own_engine_still_loads_its_model_declares_hearing_not_ready_yet() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::from_pairs([("STATE_DIR", "")]);
    let mut config = Config::for_tests(&env);
    // Its own engine, never started here: the model not in.
    config.speech.asr.local =
        Some(crate::config::LocalAsrConfig { dir: dir.path().to_path_buf(), mirror: None, gpu: false });
    let app = App::build(config).unwrap();
    let station = key(&app, &["home.models"]);
    let (status, body) = send(&app, "GET", STATUS, Some(&station), Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["asr"], false, "a station of 0.11 leans on it once the model is in");
    let contract = HelperStatus::read(&body).unwrap().contract;
    let hearing = contract.get(Ability::Hearing).unwrap();
    assert!(!hearing.ready);
    let model = app.pc.local_asr.as_ref().unwrap().model();
    let known = crate::speech::asr::local::catalog::find(&model).unwrap();
    let details = [("model".to_string(), json!(model)), ("languages".to_string(), json!(known.languages))];
    assert_eq!(hearing.details, Map::from_iter(details));
    reads_the_same_without_the_contract(&body);
}

#[tokio::test]
async fn the_contract_says_the_bot_once_it_is_set_up_and_the_catalog_once_there_is_a_copy_for_the_station() {
    const FEED: &str = "https://packages.example.invalid/catalog.json";
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_str().unwrap();
    let app = computer(&[
        ("DATA_DIR", data),
        ("PLUGINS_BUNDLED", "false"),
        ("TELEGRAM_BOT_TOKEN", "123:test"),
        ("PACKAGES_FEED_URL", FEED),
    ]);
    let station = key(&app, &["home.models"]);
    let (_, body) = send(&app, "GET", STATUS, Some(&station), Value::Null).await;
    let contract = HelperStatus::read(&body).unwrap().contract;
    assert!(contract.ready(Ability::Messages), "its bot is set up");
    assert!(!contract.declares(Ability::Catalog), "its feed is named, not read yet");
    let (status, _) = send(&app, "GET", "/home/v1/catalog", Some(&station), Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    reads_the_same_without_the_contract(&body);

    // The feed read once (its copy and the note of whose copy it is): the station may take the catalog from here.
    let folder = &app.config.plugins.catalog_dir;
    std::fs::create_dir_all(folder).unwrap();
    std::fs::write(folder.join("catalog.json"), r#"{"format":1,"packages":[]}"#).unwrap();
    let state = json!({ "url": FEED, "fetchedAt": "2026-11-15T10:00:00.000Z" });
    std::fs::write(folder.join("state.json"), state.to_string()).unwrap();
    let (_, body) = send(&app, "GET", STATUS, Some(&station), Value::Null).await;
    assert!(HelperStatus::read(&body).unwrap().contract.ready(Ability::Catalog), "{body}");
    let (status, copy) = send(&app, "GET", "/home/v1/catalog", Some(&station), Value::Null).await;
    assert_eq!(
        (status, &copy["url"], &copy["feed"]),
        (StatusCode::OK, &json!(FEED), &json!(r#"{"format":1,"packages":[]}"#))
    );
    // When it was read goes with it: the station keeps the copy as old as it is (20.11).
    assert_eq!(copy["fetchedAt"], json!("2026-11-15T10:00:00.000Z"));
}

#[tokio::test]
async fn the_station_reads_the_bots_contacts_by_name_only_with_its_key() {
    let app = computer(&[("TELEGRAM_BOT_TOKEN", "123:test"), ("TELEGRAM_CONTACTS", "Мама=42; Бабушка=43")]);
    let station = key(&app, &["home.models"]);
    let (status, body) = send(&app, "GET", CONTACTS, Some(&station), Value::Null).await;
    assert_eq!((status, &body), (StatusCode::OK, &json!({ "contacts": ["мама", "бабушка"] })), "no chats");

    // Without a key, or with one that has no right to the computer's models: closed, as the other /home/v1.
    let (status, _) = send(&app, "GET", CONTACTS, None, Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let other = key(&app, &["station.view"]);
    let (status, _) = send(&app, "GET", CONTACTS, Some(&other), Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let message = json!({ "contact": "мама", "text": "Я задержусь" });
    let (status, _) = send(&app, "POST", SEND, Some(&other), message).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_send_says_plainly_what_went_wrong_before_telegram() {
    let app = computer(&[("TELEGRAM_BOT_TOKEN", "123:test"), ("TELEGRAM_CONTACTS", "мама=42")]);
    let station = key(&app, &["home.models"]);
    let unknown = json!({ "contact": "Пете", "text": "Привет" });
    let (status, body) = send(&app, "POST", SEND, Some(&station), unknown).await;
    assert_eq!(
        (status, body["message"].as_str()),
        (StatusCode::NOT_FOUND, Some("У бота Telegram на этом компьютере нет контакта «Пете»."))
    );
    let (status, body) = send(&app, "POST", SEND, Some(&station), json!({ "contact": "мама" })).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::BAD_REQUEST, Some(send_shape().as_str())));

    // No bot on the computer: its contacts and its sends say so.
    let app = computer(&[]);
    let station = key(&app, &["home.models"]);
    let (status, body) = send(&app, "GET", CONTACTS, Some(&station), Value::Null).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some(no_bot().as_str())));
    let message = json!({ "contact": "мама", "text": "Я задержусь" });
    let (status, body) = send(&app, "POST", SEND, Some(&station), message).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some(no_bot().as_str())));
}

/// A home's request over the link: its client and whether its home key proved it this computer's own.
async fn over_link(
    app: &Arc<App>,
    method: &str,
    uri: &str,
    client: &str,
    own_home: bool,
    body: Value,
) -> (StatusCode, Value) {
    over_link_in(app, method, uri, client, own_home, body, None).await
}

/// [`over_link`] asked in a language (`Accept-Language`, as the centre passes its asker's on).
async fn over_link_in(
    app: &Arc<App>,
    method: &str,
    uri: &str,
    client: &str,
    own_home: bool,
    body: Value,
    language: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri).header("content-type", "application/json");
    request = request.header("authorization", "Bearer anything");
    if let Some(language) = language {
        request = request.header("accept-language", language);
    }
    let mut request = request.body(HttpBody::from(body.to_string())).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new("127.0.0.1".parse().unwrap(), 5000)));
    request.extensions_mut().insert(crate::platform::http::LinkOrigin {
        client: client.to_string(),
        own_home,
        home: HelperIdentity { id: "home-1".into(), name: "Дача".into() },
        gateway: HelperIdentity { id: "gw-1".into(), name: "Колонка".into() },
    });
    let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// A home paired with this computer: its client id.
fn client(app: &App, own_home: bool) -> String {
    let (id, _) = app.pc.clients.pair("home-1", "gw-1", own_home, None).unwrap();
    assert!(app.pc.clients.confirm(&id, crate::helpers::clients::Names::default()));
    id
}

fn voice_ready(body: &Value) -> Option<bool> {
    let contract = HelperStatus::read(body).unwrap().contract;
    contract.get(Ability::Voice).map(|voice| voice.ready)
}

#[tokio::test]
async fn only_the_computers_own_home_names_the_character_it_speaks_as() {
    // A stock voice that another home may hear: still, only the own home changes whose voice it is.
    let app = computer(&[
        ("TTS_OWN_URL", "http://127.0.0.1:9/v1"),
        ("TTS_OWN_MODEL", "piper"),
        ("TTS_OWN_VOICE", "ru_RU-irina-medium"),
    ]);
    let character = json!({ "id": "butler:arthur" });
    let other = client(&app, false);
    let (status, body) = over_link(&app, "POST", "/home/v1/character", &other, false, character.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let own = client(&app, true);
    let (status, body) = over_link(&app, "POST", "/home/v1/character", &own, true, character).await;
    // No such character here and no package named: it says so, it is not refused.
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, _) = over_link(&app, "POST", "/home/v1/character", &own, true, json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// Clean-path check 19.11: «Голос ждёт» lived only on the computer's own page. The voice waiting here is shown to
/// the home's centre (`GET /home/v1/voice/pending`) with its name and tight memory as numbers, its reason written
/// from them in the asker's language — not the words of the moment it was found —, and the owner's answer is taken
/// from there (`POST`); only by this computer's own home.
#[tokio::test]
async fn the_turbo_shows_its_waiting_voice_to_its_own_home_and_takes_the_owners_answer() {
    use crate::settings::definitions::{MemoryTight, PendingWait, VoicePending, VOICE_PENDING};
    const PENDING: &str = "/home/v1/voice/pending";
    let app = computer(&[
        ("TTS_OWN_URL", "http://127.0.0.1:9/v1"),
        ("TTS_OWN_MODEL", "piper"),
        ("TTS_OWN_VOICE", "ru_RU-irina-medium"),
    ]);
    let other = client(&app, false);
    let (status, body) = over_link(&app, "GET", PENDING, &other, false, Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "another home never sees it: {body}");
    let own = client(&app, true);
    let (status, body) = over_link(&app, "GET", PENDING, &own, true, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["pending"], Value::Null, "nothing waits: {body}");
    assert!(body["computer"].is_string(), "{body}");

    // A voice waits for the owner's «всё равно»: memory is short. Its old reason was kept in other words.
    let waiting = VoicePending {
        model_id: "piper-denis".into(),
        voice_id: None,
        wait_for: PendingWait { kind: "file".into(), id: "piper-denis".into() },
        anyway: false,
        state: "tight".into(),
        reason: Some("ОЗУ впритык: Memory is tight: …".into()),
        memory: Some(MemoryTight::Short { free_mb: 2048, need_mb: 5120 }),
        recipe: None,
        since: "2026-11-19T10:00:00Z".into(),
    };
    app.settings.set(&VOICE_PENDING, &waiting, None).unwrap();
    let (status, body) = over_link_in(&app, "GET", PENDING, &own, true, Value::Null, Some("ru")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let pending = &body["pending"];
    assert_eq!((&pending["modelId"], &pending["state"]), (&json!("piper-denis"), &json!("tight")), "{body}");
    assert!(pending["title"].as_str().is_some_and(|title| !title.is_empty()), "{pending}");
    assert_eq!(pending["memory"], json!({ "kind": "short", "freeMb": 2048, "needMb": 5120 }));
    assert_eq!(
        pending["reason"],
        "ОЗУ мало: свободно 2,0 ГБ, а нужно около 5,0 ГБ — Мак начнёт подкачку с диска, колонка будет подтормаживать."
    );
    // Asked in English (a panel in English at the centre): the same numbers in its words.
    let (_, body) = over_link_in(&app, "GET", PENDING, &own, true, Value::Null, Some("en")).await;
    let reason = body["pending"]["reason"].as_str().unwrap_or_default();
    assert!(reason.starts_with("Memory is low: 2,0 GB is free, but about 5,0 GB is needed"), "{body}");

    // The owner's answer: never another home's; an unknown one is refused; «cancel» forgets the voice.
    let (status, _) = over_link(&app, "POST", PENDING, &other, false, json!({ "answer": "cancel" })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(app.pc.models.pending().is_some(), "another home's answer changes nothing");
    let (status, _) = over_link(&app, "POST", PENDING, &own, true, json!({ "answer": "maybe" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = over_link(&app, "POST", PENDING, &own, true, json!("cancel")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "not the answer's form");
    assert!(app.pc.models.pending().is_some());
    let (status, body) = over_link(&app, "POST", PENDING, &own, true, json!({ "answer": "cancel" })).await;
    assert_eq!((status, &body["pending"]), (StatusCode::OK, &Value::Null), "{body}");
    assert!(app.pc.models.pending().is_none());
    // Nothing waits any more: an answer says so.
    let (status, body) =
        over_link_in(&app, "POST", PENDING, &own, true, json!({ "answer": "anyway" }), Some("ru")).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::CONFLICT, Some("Голос уже не ждёт.")), "{body}");
}

/// Clean-path check 18.11, item 40: the centre asks how the character's voice would stand here before it is
/// installed — every model with how its voice comes (snapshot, reference), its state, room and memory against
/// what is free, the model it would go with; the model the centre names is kept and the plan reports it.
#[tokio::test]
async fn the_turbo_answers_the_plan_of_the_characters_voice_and_keeps_the_model_the_centre_names() {
    let lab = crate::computer::tests::voice_lab_testing::lab().await;
    let app = &lab.app;
    const PLAN: &str = "/home/v1/character/plan";
    let asked = json!({
        "id": "arthur:arthur", "package": { "id": "arthur", "version": "1.2.0" },
        "voices": ["omnivoice", "cosyvoice3"], "reference": true, "title": "Артур", "gender": "male",
        "fallbackVoice": "piper-denis"
    });
    let other = client(app, false);
    let (status, _) = over_link(app, "POST", PLAN, &other, false, asked.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "another home never sees it");
    let own = client(app, true);
    let (status, plan) = over_link(app, "POST", PLAN, &own, true, asked.clone()).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    let engines = plan["engines"].as_array().unwrap();
    let ids: Vec<&str> = engines.iter().map(|engine| engine["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["omnivoice", "cosyvoice3", "voxcpm"]);
    let omnivoice = &engines[0];
    assert_eq!(
        (&omnivoice["way"], &omnivoice["state"], &omnivoice["supported"], &omnivoice["device"]),
        (&json!("snapshot"), &json!("ready"), &json!(true), &json!("mps")),
        "{omnivoice}"
    );
    assert_eq!(omnivoice["recommended"], true);
    assert_eq!((&omnivoice["percent"], &omnivoice["doneMb"]), (&Value::Null, &Value::Null), "nothing coming");
    assert_eq!(omnivoice["totalMb"], 3_110 + crate::voice_lab::clone_engines::PACKAGES_MB);
    assert_eq!((&omnivoice["freeDiskMb"], &omnivoice["diskFits"]), (&json!(80_000), &json!(true)));
    let (ram, free) = (omnivoice["ramMb"].as_u64().unwrap(), omnivoice["freeRamMb"].as_u64().unwrap());
    assert!(ram > 0 && free > 0, "{omnivoice}");
    assert_eq!(omnivoice["ramFits"], free >= ram);
    let cosy = &engines[1];
    assert_eq!((&cosy["way"], &cosy["state"]), (&json!("snapshot"), &json!("missing")), "{cosy}");
    assert_eq!(cosy["diskFits"], cosy["freeDiskMb"].as_u64() >= cosy["diskMb"].as_u64());
    assert_eq!(engines[2]["way"], "reference", "no snapshot for it: taken from the reference");
    // The model installed here with its snapshot: this computer's pick.
    assert_eq!(plan["chosen"], "omnivoice");
    assert_eq!((&plan["progress"], &plan["voiceOn"]), (&Value::Null, &json!(false)));
    assert_eq!(plan["computer"], app.identity.gateway().unwrap().name);
    assert!(plan.get("fallback").is_some(), "{plan}");

    // The centre's owner wants CosyVoice: the plan asked with it says so.
    let mut wanted = asked.clone();
    wanted["engine"] = json!("cosyvoice3");
    let (_, plan) = over_link(app, "POST", PLAN, &own, true, wanted).await;
    assert_eq!(plan["chosen"], "cosyvoice3");

    // Named with the character: kept here — the next plan, asked without it, goes with it (the character is not
    // installed here, so the choice itself says so).
    let named = json!({ "id": "arthur:arthur", "engine": "cosyvoice3" });
    let (status, _) = over_link(app, "POST", "/home/v1/character", &own, true, named).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, plan) = over_link(app, "POST", PLAN, &own, true, asked.clone()).await;
    assert_eq!(plan["chosen"], "cosyvoice3", "{plan}");
    let unknown = json!({ "id": "arthur:arthur", "engine": "f5tts" });
    let (status, _) = over_link(app, "POST", "/home/v1/character", &own, true, unknown).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // A model the character has no voice for (no snapshot, no reference) is not taken.
    let bare = json!({ "id": "arthur:arthur", "voices": ["omnivoice"], "reference": false });
    let (_, plan) = over_link(app, "POST", PLAN, &own, true, bare).await;
    assert_eq!(plan["chosen"], "omnivoice", "{plan}");
}

/// A model that does not fit on this computer's disk is never downloaded: the plan does not choose it (none at all
/// when nothing fits), the centre naming it is refused and nothing kept, and a character whose models do not fit
/// gets its fallback voice with no install started.
#[tokio::test]
async fn a_model_that_does_not_fit_on_the_disk_is_never_chosen_nor_downloaded() {
    use std::sync::atomic::Ordering;
    let lab = crate::computer::tests::voice_lab_testing::lab().await;
    let app = &lab.app;
    // 8 GB free: OmniVoice is installed; CosyVoice (10.5 GB) and VoxCPM (9.5 GB) would not fit.
    lab.memory.free_disk_mb.store(8_000, Ordering::SeqCst);
    const PLAN: &str = "/home/v1/character/plan";
    let own = client(app, true);
    let asked = json!({
        "id": "arthur:arthur", "package": { "id": "arthur", "version": "1.2.0" },
        "voices": ["omnivoice", "cosyvoice3"], "reference": true, "engine": "cosyvoice3"
    });
    let (status, plan) = over_link(app, "POST", PLAN, &own, true, asked).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    let fits: Vec<(&str, bool)> = plan["engines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|engine| (engine["id"].as_str().unwrap(), engine["diskFits"].as_bool().unwrap()))
        .collect();
    assert_eq!(fits, [("omnivoice", true), ("cosyvoice3", false), ("voxcpm", false)]);
    assert_eq!(plan["chosen"], "omnivoice", "the wanted one does not fit: the one that does");
    // Only CosyVoice has the character's voice: nothing fits, nothing chosen.
    let only_cosy = json!({ "id": "arthur:arthur", "voices": ["cosyvoice3"], "reference": false });
    let (_, plan) = over_link(app, "POST", PLAN, &own, true, only_cosy).await;
    assert_eq!(plan["chosen"], Value::Null, "{plan}");

    // The centre names CosyVoice: refused in plain words, nothing kept.
    let named = json!({ "id": "arthur:arthur", "engine": "cosyvoice3" });
    let (status, body) = over_link(app, "POST", "/home/v1/character", &own, true, named).await;
    assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE, "{body}");
    let message = body["message"].as_str().unwrap();
    assert!(
        message.starts_with("Модель ") && message.contains("не поместится: нужно 10 ГБ, свободно 7,8 ГБ"),
        "{message}"
    );
    assert!(app.settings.get(&crate::settings::definitions::TURBO_VOICE_ENGINES, None).is_none());

    // A character whose only model does not fit, told with no model: chosen with no download.
    let mut cosy_only = crate::whole_tests::characters_contributes::arthur_manifest("1.2.0");
    let character = &mut cosy_only["contributes"]["characters"][0];
    character["voices"] = json!({ "cosyvoice3": { "file": "voices/cosyvoice3.pt", "model": "074ca6dc" } });
    character.as_object_mut().unwrap().remove("ref");
    crate::whole_tests::characters_contributes::write_arthur(&app.config.plugins.packages_dir, &cosy_only);
    app.characters.sync();
    let (status, body) =
        over_link(app, "POST", "/home/v1/character", &own, true, json!({ "id": "arthur:arthur" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_ne!(body["voice"]["state"], "installing", "{body}");
    assert!(body["voice"]["pending"].is_null(), "{body}");
    assert!(app.pc.voice_lab.job().is_none(), "no model download started");
    assert_eq!(app.personas.chosen().unwrap().preset_id.as_deref(), Some("arthur:arthur"));
}

/// The centre's package comes with its archive's address and hash from the centre's signed feed: a hash that does
/// not match installs nothing; the one that matches installs it, without this computer's own feed.
#[tokio::test]
async fn the_package_from_the_centre_installs_only_when_its_sha256_matches() {
    use crate::plugins::install::tests::{character_manifest, vgp};
    use sha2::Digest;
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_str().unwrap();
    let app = computer(&[("DATA_DIR", data), ("PLUGINS_BUNDLED", "false"), ("PACKAGES_FEED_URL", "off")]);
    let site = crate::plugins::catalog::tests::site().await;
    let archive = vgp(&character_manifest("arthur", "1.2.0"));
    site.publish(&[(crate::plugins::catalog::tests::arthur("1.2.0"), archive.clone())]);
    let url = site.url.replace("catalog.json", "packages/arthur-1.2.0.vgp");
    let sha256 = hex::encode(sha2::Sha256::digest(&archive));
    let own = client(&app, true);
    let told = |sha256: &str| {
        json!({
            "id": "arthur:arthur", "package": { "id": "arthur", "version": "1.2.0" },
            "archive": { "url": url, "sha256": sha256 }
        })
    };
    let wrong = "0".repeat(64);
    let (status, body) = over_link(&app, "POST", "/home/v1/character", &own, true, told(&wrong)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["message"],
        "Не поставлено: архив «arthur» от хаба не совпал по sha256 — он подменён или испорчен."
    );
    assert!(app.plugins.installed("arthur").is_none(), "nothing installed");
    assert!(!app.config.plugins.packages_dir.join("arthur").join("1.2.0").exists());

    let (status, body) = over_link(&app, "POST", "/home/v1/character", &own, true, told(&sha256)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let installed = app.plugins.installed("arthur").expect("installed from the centre's archive");
    assert_eq!(installed.manifest.version.to_string(), "1.2.0");
    assert_eq!(app.personas.chosen().unwrap().preset_id.as_deref(), Some("arthur:arthur"));
}

/// Item 41: the home's own centre names a character it installed from its own source with a voice of no known
/// origin — this Турбо takes it; another home is refused before anything, a caller other than the home's link gets
/// the catalog's rule, and a package that is not a character is never taken this way.
#[tokio::test]
async fn the_centres_character_with_a_voice_of_unknown_origin_installs_only_for_its_own_home() {
    use crate::plugins::install::tests::{character_manifest, vgp};
    use sha2::Digest;
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_str().unwrap();
    let app = computer(&[("DATA_DIR", data), ("PLUGINS_BUNDLED", "false"), ("PACKAGES_FEED_URL", "off")]);
    let site = crate::plugins::catalog::tests::site().await;
    let mut manifest = character_manifest("arthur", "1.2.0");
    manifest["contributes"]["characters"][0]["voiceOrigin"] = json!("unknown");
    let archive = vgp(&manifest);
    let mut theme = character_manifest("arthur", "1.3.0");
    theme["kind"] = json!("theme");
    let not_character = vgp(&theme);
    site.publish(&[
        (crate::plugins::catalog::tests::arthur("1.2.0"), archive.clone()),
        (crate::plugins::catalog::tests::arthur("1.3.0"), not_character.clone()),
    ]);
    let url = |version: &str| site.url.replace("catalog.json", &format!("packages/arthur-{version}.vgp"));
    let sha256 = hex::encode(sha2::Sha256::digest(&archive));
    let told = json!({
        "id": "arthur:arthur", "package": { "id": "arthur", "version": "1.2.0" },
        "archive": { "url": url("1.2.0"), "sha256": sha256 }
    });

    let other = client(&app, false);
    let (status, body) = over_link(&app, "POST", "/home/v1/character", &other, false, told.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(app.plugins.installed("arthur").is_none(), "another home installs nothing");

    // Not the home's link (a person at this computer, a script): the catalog's rule stands.
    let refused = app.catalog.install_from_centre("arthur", "1.2.0", &url("1.2.0"), &sha256, false, None).await;
    let refused = refused.expect_err("an unknown voice without the home's word");
    assert!(refused.message.contains("voiceOrigin unknown"), "{}", refused.message);
    assert!(app.plugins.installed("arthur").is_none());
    assert!(!app.config.plugins.packages_dir.join("arthur").join("1.2.0").exists(), "nothing left on the disk");

    let own = client(&app, true);
    let (status, body) = over_link(&app, "POST", "/home/v1/character", &own, true, told).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let installed = app.plugins.installed("arthur").expect("installed from the centre's archive");
    assert_eq!(installed.manifest.version.to_string(), "1.2.0");
    assert_eq!(app.personas.chosen().unwrap().preset_id.as_deref(), Some("arthur:arthur"));

    // Not a character: never taken from the centre's archive, even for the own home.
    let theme = json!({
        "id": "arthur:arthur", "package": { "id": "arthur", "version": "1.3.0" },
        "archive": { "url": url("1.3.0"), "sha256": hex::encode(sha2::Sha256::digest(&not_character)) }
    });
    let (status, body) = over_link(&app, "POST", "/home/v1/character", &own, true, theme).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(app.plugins.installed("arthur").unwrap().manifest.version.to_string(), "1.2.0", "the character stays");
}

/// Item 41б: the station calls this computer by address, not over the link — from the home network, with the key
/// made for it (`home.models`). That is the home too: the character with a voice of unknown origin is taken; a key
/// without the help's right never gets that far.
#[tokio::test]
async fn the_station_by_address_with_the_homes_key_is_the_home_for_the_centres_character() {
    use crate::plugins::install::tests::{character_manifest, vgp};
    use sha2::Digest;
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_str().unwrap();
    let app = computer(&[("DATA_DIR", data), ("PLUGINS_BUNDLED", "false"), ("PACKAGES_FEED_URL", "off")]);
    let site = crate::plugins::catalog::tests::site().await;
    let mut manifest = character_manifest("arthur", "1.2.0");
    manifest["contributes"]["characters"][0]["voiceOrigin"] = json!("unknown");
    let archive = vgp(&manifest);
    site.publish(&[(crate::plugins::catalog::tests::arthur("1.2.0"), archive.clone())]);
    let told = json!({
        "id": "arthur:arthur", "package": { "id": "arthur", "version": "1.2.0" },
        "archive": {
            "url": site.url.replace("catalog.json", "packages/arthur-1.2.0.vgp"),
            "sha256": hex::encode(sha2::Sha256::digest(&archive))
        }
    });

    let stranger = key(&app, &["station.view"]);
    let (status, body) = send(&app, "POST", "/home/v1/character", Some(&stranger), told.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(app.plugins.installed("arthur").is_none(), "a key without the help's right installs nothing");

    let station = key(&app, &[HOME_MODELS]);
    let (status, body) = send(&app, "POST", "/home/v1/character", Some(&station), told).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let installed = app.plugins.installed("arthur").expect("installed from the centre's archive");
    assert_eq!(installed.manifest.version.to_string(), "1.2.0");
    assert_eq!(app.personas.chosen().unwrap().preset_id.as_deref(), Some("arthur:arthur"));
}

#[tokio::test]
async fn only_the_computers_own_home_installs_an_apps_code_here() {
    let app = computer(&[]);
    let install = json!({ "id": "feed", "version": "0.4.0" });
    let other = client(&app, false);
    let (status, body) = over_link(&app, "POST", "/home/v1/plugins/install", &other, false, install.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let own = client(&app, true);
    let (status, body) = over_link(&app, "POST", "/home/v1/plugins/install", &own, true, install).await;
    // No catalog in a test: the install itself says so — the home was let through.
    assert_ne!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _) = over_link(&app, "POST", "/home/v1/plugins/install", &own, true, json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// The clean-path check of 19.11: the station asked the Mac for YouTube's code by id, YouTube came from a demo feed
/// only the station reads, and the Mac answered «not in the catalog» — so the app never ran there. The centre now
/// names the feed: the Турбо installs and switches on what its own signed main feed pinned, and nothing else.
#[tokio::test]
async fn the_centre_names_the_feed_of_a_faces_code_and_the_turbo_installs_only_what_it_pinned() {
    use crate::plugins::catalog::tests::{arthur, plugin_vgp, site};
    let keys = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    let (main, demo) = (site().await, site().await);
    main.publish(&[]);
    let app_of = |id: &str| {
        let manifest = json!({
            "schema": 1, "kind": "plugin", "id": id, "version": "1.0.0", "title": id, "api": "^2",
            "runs": ["pc"], "permissions": [],
            "contributes": { "intents": [{ "phrases": [format!("включи {id}")], "handler": "play" }] }
        });
        let mut entry = arthur("1.0.0");
        (entry["id"], entry["kind"], entry["runs"]) = (json!(id), json!("plugin"), json!(["pc"]));
        entry["budget"] = json!({ "memoryMb": 4 });
        (entry, plugin_vgp(&manifest, "export function play() { return 'играю'; }"))
    };
    demo.publish(&[app_of("tube"), app_of("moon")]);
    let pinned: Value = serde_json::from_str(&demo.feed.lock().unwrap()).unwrap();
    let mut feed: Value = serde_json::from_str(&main.feed.lock().unwrap()).unwrap();
    feed["recommends"] = json!([{ "id": "tube", "sha256": pinned["packages"][0]["sha256"] }]);
    let text = feed.to_string();
    let signature =
        minisign::sign(Some(&keys.pk), &keys.sk, std::io::Cursor::new(text.as_bytes()), None, None).unwrap();
    *main.feed.lock().unwrap() = text;
    main.files.lock().unwrap().insert("catalog.json.minisig".into(), signature.to_string().into_bytes());

    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_str().unwrap();
    let key = keys.pk.to_base64();
    let app = computer(&[
        ("DATA_DIR", data),
        ("PLUGINS_BUNDLED", "false"),
        ("PACKAGES_FEED_URL", main.url.as_str()),
        ("PACKAGES_PUBKEY", key.as_str()),
    ]);
    let own = client(&app, true);
    let install = |id: &str, feed: Option<&str>| {
        let mut body = json!({ "id": id, "version": "1.0.0" });
        if let Some(feed) = feed {
            body["feed"] = json!(feed);
        }
        body
    };
    // As before: by id alone, this computer's catalog has no such package.
    let (status, body) = over_link(&app, "POST", "/home/v1/plugins/install", &own, true, install("tube", None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    // With the centre's feed: installed and switched on — the station's calls find it now.
    let (status, body) =
        over_link(&app, "POST", "/home/v1/plugins/install", &own, true, install("tube", Some(&demo.url))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(app.plugins.is_active("tube"), "{body}");
    // Not pinned: refused, nothing installed.
    let (status, body) =
        over_link(&app, "POST", "/home/v1/plugins/install", &own, true, install("moon", Some(&demo.url))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(app.plugins.installed("moon").is_none());

    // The code here already at that version or newer, switched off (20.11): switched on as it is — before, the
    // catalog was asked for the older one, answered «Новее 1.0.0 в каталоге нет», and it stayed off.
    app.plugins.disable("tube", None).await.unwrap();
    let mut older = install("tube", Some(&demo.url));
    older["version"] = json!("0.9.0");
    let (status, body) = over_link(&app, "POST", "/home/v1/plugins/install", &own, true, older).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(app.plugins.is_active("tube"), "{body}");
    assert_eq!(body["version"], json!("1.0.0"), "its own code, switched on");
}

#[tokio::test]
async fn another_home_hears_nothing_of_the_owners_own_voice() {
    let speech = json!({ "input": "Привет" });
    // The owner's voice at a cloud address: never another home's.
    let app = computer(&[
        ("TTS_OWN_URL", "https://tts.example.invalid/v1"),
        ("TTS_OWN_MODEL", "piper"),
        ("TTS_OWN_VOICE", "ru_RU-irina-medium"),
    ]);
    assert!(!voice_for_others(&app.voice));
    let other = client(&app, false);
    let (status, body) = over_link(&app, "GET", STATUS, &other, false, Value::Null).await;
    assert_eq!((status, voice_ready(&body)), (StatusCode::OK, Some(false)), "{body}");
    let (status, body) = over_link(&app, "POST", "/home/v1/tts/audio/speech", &other, false, speech.clone()).await;
    assert_eq!(
        (status, body["message"].as_str()),
        (StatusCode::FORBIDDEN, Some("Это Турбо не даёт этого вашему дому."))
    );
    // Its own home hears it.
    let own = client(&app, true);
    let (_, body) = over_link(&app, "GET", STATUS, &own, true, Value::Null).await;
    assert_eq!(voice_ready(&body), Some(true));
    let (status, _) = over_link(&app, "POST", "/home/v1/tts/audio/speech", &own, true, speech.clone()).await;
    assert_ne!(status, StatusCode::FORBIDDEN);
    // A voice of the voice lab's engine, even at home: the owner's own.
    let app = computer(&[
        ("TTS_OWN_URL", "http://127.0.0.1:9/v1"),
        ("TTS_OWN_MODEL", "omnivoice"),
        ("TTS_OWN_VOICE", "3f2a9c1e"),
    ]);
    assert!(!voice_for_others(&app.voice));
    // A stock voice of this computer: served to another home too (the server here does not answer: 502, not 403).
    let app = computer(&[
        ("TTS_OWN_URL", "http://127.0.0.1:9/v1"),
        ("TTS_OWN_MODEL", "piper"),
        ("TTS_OWN_VOICE", "ru_RU-irina-medium"),
    ]);
    assert!(voice_for_others(&app.voice));
    let other = client(&app, false);
    let (_, body) = over_link(&app, "GET", STATUS, &other, false, Value::Null).await;
    assert_eq!(voice_ready(&body), Some(true));
    let (status, _) = over_link(&app, "POST", "/home/v1/tts/audio/speech", &other, false, speech).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

/// The built-in helper's voice at a cloud's address (stage 6) over the link: its own home hears it only while the
/// regulator lets the clouds out — «Только дома» keeps the text home for its own home too: the status says the
/// voice not ready, and the voice refuses in the regulator's words, not the link's.
#[tokio::test]
async fn the_regulator_keeps_the_computers_cloud_voice_from_its_own_home_over_the_link_too() {
    let app = computer(&[
        ("TTS_OWN_URL", "https://tts.example.invalid/v1"),
        ("TTS_OWN_MODEL", "piper"),
        ("TTS_OWN_VOICE", "ru_RU-irina-medium"),
    ]);
    let own = client(&app, true);
    let (_, body) = over_link(&app, "GET", STATUS, &own, true, Value::Null).await;
    assert_eq!(voice_ready(&body), Some(true), "«Любые облака»");
    app.helpers.set_away(crate::helpers::Away::Home, None).unwrap();
    let (_, body) = over_link(&app, "GET", STATUS, &own, true, Value::Null).await;
    assert_eq!(voice_ready(&body), Some(false), "{body}");
    let speech = json!({ "input": "Привет" });
    let (status, body) = over_link(&app, "POST", "/home/v1/tts/audio/speech", &own, true, speech).await;
    assert_eq!(
        (status, body["message"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("Регулятор не пускает облако"))
    );
}

#[tokio::test]
async fn every_route_names_what_it_needs() {
    let app = computer(&[("TTS_OWN_URL", "http://127.0.0.1:9/v1"), ("TTS_OWN_MODEL", "piper")]);
    let nothing = client(&app, false);
    app.pc.clients.set_gives(&nothing, Vec::new(), None).unwrap();
    let routes = [
        ("POST", "/home/v1/asr/audio/transcriptions"),
        ("POST", "/home/v1/audio/transcriptions"),
        ("GET", "/home/v1/asr/models"),
        ("POST", "/home/v1/tts/audio/speech"),
        ("POST", "/home/v1/audio/speech"),
        ("POST", "/home/v1/llm/chat/completions"),
        ("POST", "/home/v1/chat/completions"),
        ("GET", "/home/v1/llm/models"),
        ("GET", "/home/v1/models"),
        ("GET", "/home/v1/plugins"),
        ("GET", "/home/v1/catalog"),
        ("GET", "/home/v1/catalog/file?url=https://packages.example.invalid/a.tgz"),
        ("GET", "/home/v1/catalog/feed?url=https://store.example.invalid/catalog.json"),
        ("POST", "/home/v1/plugins/weather/call"),
        ("GET", "/home/v1/messenger/contacts"),
        ("POST", "/home/v1/messenger/send"),
        ("POST", "/home/v1/history/have"),
        ("POST", "/home/v1/history/objects"),
        ("POST", "/home/v1/history/lines"),
        ("POST", "/home/v1/history/fetch"),
    ];
    for (method, uri) in routes {
        let (status, body) = over_link(&app, method, uri, &nothing, false, json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {body}");
        assert_eq!(body["message"], "Это Турбо не даёт этого вашему дому.", "{method} {uri}");
    }
    // The status only says what it gives: nothing.
    let (status, body) = over_link(&app, "GET", STATUS, &nothing, false, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let contract = HelperStatus::read(&body).unwrap().contract;
    assert!(contract.abilities.is_empty() && contract.link == Some(crate::helpers::link::VERSION), "{body}");
    // An unknown client is given nothing either; and a link caller is outside, never this computer.
    let (status, _) = over_link(&app, "GET", "/home/v1/plugins", "link:000000000000", false, Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// A Bot API server of our own: what was sent to it; `refuse` answers 400 as Telegram does to a bad chat.
async fn bot_api(refuse: bool) -> (String, Arc<StdMutex<Vec<Value>>>) {
    let sent: Arc<StdMutex<Vec<Value>>> = Arc::default();
    let kept = sent.clone();
    let router = axum::Router::new().route(
        "/{bot}/sendMessage",
        post(move |axum::Json(body): axum::Json<Value>| {
            let kept = kept.clone();
            async move {
                if refuse {
                    return Err(StatusCode::BAD_REQUEST);
                }
                kept.lock().unwrap().push(body);
                Ok(axum::Json(json!({ "ok": true, "result": {} })))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await });
    (base, sent)
}

fn bot(api: &str) -> Arc<TelegramMessenger> {
    let config = TelegramConfig {
        token: "123:test".into(),
        contacts: vec![("мама".to_string(), "42".to_string()), ("бабушка".to_string(), "43".to_string())],
        inbox_dir: std::env::temp_dir(),
        alert_chat: None,
    };
    TelegramMessenger::with_api(Some(config), api)
}

#[tokio::test]
async fn the_bot_passes_the_stations_message_on_or_says_telegram_refused() {
    let (api, sent) = bot_api(false).await;
    assert_eq!(
        pass_on(&crate::computer::links::calls::SkillsTelegram(bot(&api)), "Бабушке", "Купи хлеба.").await.unwrap(),
        "бабушка"
    );
    assert_eq!(*sent.lock().unwrap(), [json!({ "chat_id": "43", "text": "Купи хлеба." })]);

    let (api, _) = bot_api(true).await;
    let refused =
        pass_on(&crate::computer::links::calls::SkillsTelegram(bot(&api)), "мама", "Я задержусь").await.unwrap_err();
    assert_eq!(refused.status, StatusCode::BAD_GATEWAY);
    assert!(refused.message.starts_with("Telegram не принял сообщение"), "{}", refused.message);
    assert!(!refused.message.contains("123:test"), "never the bot's token");
}
