//! The tests of `helpers/builtin.rs` that need the whole gateway (`App`): its crate does not have it, the program does.
//!
//! The built-in helper's tests, and what other modules' tests share: a computer's gateway, a model server on it, a row
//! of «Мозг».

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body as HttpBody;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use http_body_util::BodyExt;
use serde_json::{json, Map, Value};
use tower::ServiceExt;

use crate::app::App;
use crate::config::{Config, Env, LocalAsrConfig};
use crate::core::events::GatewayEvent;
use crate::dialog::brain::ProviderInput;
use crate::helpers::builtin::*;
use crate::helpers::contract::Ability;
use crate::helpers::{key_secret, Change, Giver, HelperKind, HelperSetting, HelperState, OwnModel, OwnParts, SELF};

/// A computer's gateway (not in the station role), its .env these, its data in a folder of its own: built, not
/// started — its tick does not run, a test looks itself (`check`).
pub(crate) fn computer(vars: &[(&str, &str)]) -> (Arc<App>, tempfile::TempDir) {
    computer_with(vars, |_| {})
}

/// The same, its config changed first.
pub(crate) fn computer_with(vars: &[(&str, &str)], change: impl FnOnce(&mut Config)) -> (Arc<App>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_string_lossy().to_string();
    let base = [("STATE_DIR", ""), ("DATA_DIR", data.as_str()), ("PLUGINS_BUNDLED", "false")];
    let env = Env::from_pairs(base.into_iter().chain(vars.iter().copied()));
    let mut config = Config::for_tests(&env);
    change(&mut config);
    (App::build(config).unwrap(), dir)
}

/// A model server on this computer (Ollama, LM Studio): each chat answered with `answer`, counted; closed, its
/// port takes no connection.
pub(crate) struct Server {
    /// `http://127.0.0.1:<port>/v1`.
    pub url: String,
    pub chats: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    pub(crate) fn chats(&self) -> usize {
        self.chats.load(Ordering::SeqCst)
    }

    /// It stops listening — for good once a connection to its port is refused: a connection may still complete
    /// for a moment after the listener is gone.
    pub(crate) async fn close(self) {
        let address = self.url.trim_start_matches("http://").trim_end_matches("/v1").to_string();
        self.task.abort();
        let _ = self.task.await;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while tokio::net::TcpStream::connect(address.as_str()).await.is_ok() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

pub(crate) async fn server(answer: &'static str) -> Server {
    let chats = Arc::new(AtomicUsize::new(0));
    let counted = chats.clone();
    let router = axum::Router::new()
        .route(
            "/v1/chat/completions",
            post(move || {
                let chats = counted.clone();
                async move {
                    chats.fetch_add(1, Ordering::SeqCst);
                    let chunk = json!({ "choices": [{ "delta": { "content": answer } }] });
                    ([("content-type", "text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
                }
            }),
        )
        .route("/v1/models", get(|| async { axum::Json(json!({ "data": [{ "id": "local-model" }] })) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Server { url, chats, task }
}

/// An address whose port takes connections while the listener lives (`http://127.0.0.1:<port>/v1`): a server of
/// this computer that nothing here asks.
pub(crate) async fn listening() -> (String, tokio::net::TcpListener) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    (format!("http://{}/v1", listener.local_addr().unwrap()), listener)
}

/// Waits until a connection to a closed listener's port is refused: one may still complete for a moment after it.
async fn refused(url: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while answers(url).await && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// What a model's slot answers to a question.
pub(crate) async fn said(slot: &crate::dialog::llm::brain::ModelSlot) -> String {
    use crate::dialog::llm::{LlmEvent, LlmMessage, LlmRequest};
    use futures::StreamExt;
    let request = LlmRequest {
        model: Some(slot.model.clone()),
        system: "s".into(),
        messages: vec![LlmMessage::User("Который час?".into())],
        ..LlmRequest::default()
    };
    let mut text = String::new();
    let mut events = slot.client.stream(request, tokio_util::sync::CancellationToken::new());
    while let Some(event) = events.next().await {
        if let Ok(LlmEvent::Text(piece)) = event {
            text.push_str(&piece);
        }
    }
    text
}

/// A row of «Мозг» as the panel adds it: its id.
pub(crate) fn provider(app: &App, preset: &str, url: &str) -> String {
    let input = ProviderInput::from_value(json!({ "preset": preset, "baseUrl": url }));
    app.brain.add(&input, None).unwrap().id
}

/// A request to `/home/v1` as a station sends it: from the home network, with a key that has `home.models`.
async fn from_station(app: &Arc<App>, request: axum::http::request::Builder, body: HttpBody) -> (StatusCode, Value) {
    let grants =
        Map::from_iter([("name".into(), json!("Колонка: домашний сервер")), ("grants".into(), json!(["home.models"]))]);
    let key = app.access.keys.create(&grants, None).unwrap().secret;
    let mut request = request.header("authorization", format!("Bearer {key}")).body(body).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new("192.168.1.20".parse().unwrap(), 5000)));
    let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// `/home/v1/status` as a station asks it.
async fn status(app: &Arc<App>) -> Value {
    let (code, body) = from_station(app, Request::builder().uri("/home/v1/status"), HttpBody::empty()).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    body
}

/// What a station's question for this computer's voice gets: the status, and the body when it is words.
async fn speak(app: &Arc<App>, text: &str) -> (StatusCode, Value) {
    let request = Request::builder().method("POST").uri("/home/v1/tts/audio/speech");
    from_station(app, request, HttpBody::from(json!({ "input": text }).to_string())).await
}

/// Waits for `helpers.changed {id: "self", change: "contract"}` (up to five seconds): the panels and «Мозг» read
/// again.
async fn told(events: &mut tokio::sync::broadcast::Receiver<Arc<GatewayEvent>>) -> bool {
    let contract = |event: &GatewayEvent| matches!(event, GatewayEvent::HelpersChanged { id: Some(id), change, .. } if id == SELF && change == "contract");
    let wait = async {
        loop {
            match events.recv().await {
                Ok(event) if contract(&event) => return true,
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), wait).await.unwrap_or(false)
}

/// The fields a station of 0.11 reads: the answer without its contract.
/// The fields a station of 0.11 and before reads: without the contract and the computer's state
/// (`system/device`, operations/device_state.rs), which the station's own home is told besides.
fn old_fields(status: &Value) -> Value {
    let mut old = status.clone();
    old.as_object_mut().unwrap().remove("contract");
    old.as_object_mut().unwrap().remove("device");
    old
}

fn old(asr: bool, tts: bool, voice: Value, llm: bool) -> Value {
    json!({
        "version": crate::VERSION, "peerApi": crate::core::build::PEER_API,
        "asr": asr, "tts": tts, "voice": voice, "llm": llm,
    })
}

/// Its own engine, never started here: its model not in.
fn loading_engine(dir: &std::path::Path) -> impl FnOnce(&mut Config) + '_ {
    move |config: &mut Config| {
        config.speech.asr.local = Some(LocalAsrConfig { dir: dir.to_path_buf(), mirror: None, gpu: false });
    }
}

#[tokio::test]
async fn the_built_in_helper_gives_what_the_computer_has() {
    let ollama = server("Ответ Ollama.").await;
    // Its recognition and voice servers take connections: their look finds them open.
    let ((asr, _asr_port), (tts, _tts_port)) = (listening().await, listening().await);
    let (app, _dir) = computer(&[("ASR_BASE_URL", asr.as_str()), ("TTS_OWN_URL", tts.as_str())]);
    provider(&app, "ollama", &ollama.url);
    app.pc.builtin.check().await;
    let contract = app.pc.builtin.contract();
    assert_eq!(contract.helper, app.identity.as_helper(), "the computer names itself");
    for ability in [Ability::Hearing, Ability::Voice, Ability::Model, Ability::Apps] {
        assert!(contract.ready(ability), "{ability:?}");
        assert!(app.pc.builtin.gives_here(ability), "{ability:?}");
    }
    assert!(contract.declares(Ability::Messages) && !contract.ready(Ability::Messages), "no bot set up here");
    assert!(!app.pc.builtin.gives_here(Ability::Messages));
    assert!(!contract.declares(Ability::Catalog), "no copy of a feed here");
    // The computer takes its parts from itself, first, without the network.
    for ability in [Ability::Hearing, Ability::Voice, Ability::Model, Ability::Apps, Ability::History] {
        assert!(matches!(app.helpers.pick(ability), Some(Giver::Itself)), "{ability:?}");
    }
    assert!(app.helpers.pick(Ability::Messages).is_none(), "no bot, no helper with one");
    // The works in work folders (router-and-works, stage 5): given here, ready once a folder is added.
    let works = contract.get(Ability::Works).expect("works are declared");
    assert!(!works.ready, "no work folder yet");
    assert_eq!(works.details["folders"], 0);
    assert_eq!(works.details["atOnce"], 1);
    assert_eq!(works.details["agents"][0], "claude-code");
    assert!(app.pc.builtin.gives_here(Ability::Works));
    assert!(matches!(app.helpers.pick(Ability::Works), Some(Giver::Itself)));
    let hears = app.helpers.own_recognizer().unwrap().description();
    assert!(hears.ends_with(&format!("via {asr}/audio/transcriptions")), "{hears}");
    assert!(Arc::ptr_eq(&app.pc.builtin.speech().unwrap(), app.voice.own_speech().unwrap()), "the very engine");

    // A cloud's recognition server is never handed on, nor the built-in helper's: the cloud stays the computer's
    // own road out, under the regulator. Its voice at a cloud is handed on as ever, and not taken as the built-in's.
    let (cloud, _cloud_dir) = computer(&[
        ("ASR_BASE_URL", "https://api.example.invalid/v1"),
        ("TTS_OWN_URL", "https://tts.example.invalid/v1"),
    ]);
    assert!(cloud.pc.builtin.hearing().is_none() && !cloud.pc.builtin.gives_here(Ability::Hearing));
    assert!(!cloud.pc.builtin.contract().declares(Ability::Hearing));
    assert!(cloud.pc.builtin.contract().declares(Ability::Voice), "/home/v1 answers as before");
    assert!(!cloud.pc.builtin.gives_here(Ability::Voice));
    ollama.close().await;
}

#[tokio::test]
async fn the_status_answers_as_before_and_adds_the_contract() {
    // Its recognition server, a voice and a model of its own.
    let ollama = server("Ответ Ollama.").await;
    let (app, _dir) = computer(&[
        ("ASR_BASE_URL", "http://127.0.0.1:8090/v1"),
        ("TTS_OWN_URL", "http://127.0.0.1:8091/v1"),
        ("TTS_OWN_MODEL", "piper"),
        ("TTS_OWN_VOICE", "ru_RU-irina-medium"),
    ]);
    provider(&app, "ollama", &ollama.url);
    let body = status(&app).await;
    assert_eq!(old_fields(&body), old(true, true, json!("piper/ru_RU-irina-medium"), true));
    assert_eq!(body["device"]["kind"], "computer", "its own home's centre reads the computer's state with it");
    assert_eq!(body["contract"]["version"], crate::helpers::contract::VERSION);
    // The status says besides that this computer speaks the link while it helps (stage 7).
    let mut said = body["contract"].clone();
    assert_eq!(said.as_object_mut().unwrap().remove("link"), Some(json!(crate::helpers::link::VERSION)));
    // A home at the home network's address reaches its things at hand (docs/plans/turbo-devices.md): said per caller.
    let devices = said["abilities"].as_array_mut().unwrap().iter_mut().find(|state| state["id"] == "devices").unwrap();
    assert_eq!(devices["details"].as_object_mut().unwrap().remove("home"), Some(json!(true)));
    assert_eq!(said, serde_json::to_value(app.pc.builtin.contract()).unwrap(), "the built-in's");

    // Its own engine, its model still loading: nothing ready for a station yet.
    let engine = tempfile::tempdir().unwrap();
    let (app, _dir) = computer_with(&[], loading_engine(engine.path()));
    let body = status(&app).await;
    assert_eq!(old_fields(&body), old(false, false, Value::Null, false));
    assert_eq!(body["contract"]["abilities"][0]["id"], "hearing");

    // Nothing of its own: recognition off, no voice, no model.
    let (app, _dir) = computer(&[("ASR_PROVIDER", "none")]);
    let body = status(&app).await;
    assert_eq!(old_fields(&body), old(false, false, Value::Null, false));
    let declared: Vec<&str> =
        body["contract"]["abilities"].as_array().unwrap().iter().filter_map(|ability| ability["id"].as_str()).collect();
    assert!(!declared.contains(&"hearing") && !declared.contains(&"voice") && !declared.contains(&"model"));
    ollama.close().await;
}

#[tokio::test]
async fn its_own_gateway_takes_a_loading_engine_others_see_it_not_ready() {
    let engine = tempfile::tempdir().unwrap();
    let (app, _dir) = computer_with(&[], loading_engine(engine.path()));
    assert!(app.pc.builtin.gives_here(Ability::Hearing) && !app.pc.builtin.hears());
    assert!(!app.pc.builtin.contract().ready(Ability::Hearing), "a station does not lean on it yet");
    assert!(matches!(app.helpers.pick(Ability::Hearing), Some(Giver::Itself)), "its own gateway waits for it");
    assert!(app.speech.recognizes());
    let asr = app.speech.status().asr;
    assert!(asr.contains("on this computer"), "the engine's own words: {asr}");
}

/// Its voice server at a cloud's address is handed on only while the regulator lets the clouds out: «Только дома»
/// keeps a station's text home through this computer too — declared as before (the old `tts`), not ready, its door
/// refusing — and the panels and «Мозг» are told. A voice at home is handed on under every position.
#[tokio::test]
async fn its_voice_at_a_cloud_is_not_handed_on_while_the_regulator_keeps_the_clouds_in() {
    let (app, _dir) = computer(&[("ASR_PROVIDER", "none"), ("TTS_OWN_URL", "https://tts.example.invalid/v1")]);
    app.pc.builtin.start();
    let mut events = app.events.subscribe();
    assert!(app.pc.builtin.contract().ready(Ability::Voice) && !app.pc.builtin.voice_kept_in(), "«Любые облака»");

    app.helpers.set_away(crate::helpers::Away::Home, None).unwrap();
    assert!(told(&mut events).await, "what it gives changed");
    let contract = app.pc.builtin.contract();
    assert!(contract.declares(Ability::Voice) && !contract.ready(Ability::Voice));
    let body = status(&app).await;
    assert_eq!(body["tts"], true, "a station of 0.11 reads only this: its door refuses instead");
    let (code, refused) = speak(&app, "Привет").await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(refused["message"], "Регулятор не пускает облако", "{refused}");

    // At home: handed on under «Только дома» too.
    let (home, _home_dir) = computer(&[("ASR_PROVIDER", "none"), ("TTS_OWN_URL", "http://127.0.0.1:8091/v1")]);
    home.helpers.set_away(crate::helpers::Away::Home, None).unwrap();
    assert!(home.pc.builtin.contract().ready(Ability::Voice) && !home.pc.builtin.voice_kept_in());
}

/// What it declares is followed as it changes — here a model of «Мозг» added on this computer —: taken at once
/// without a look, and told (`helpers.changed {id: "self", change: "contract"}`).
#[tokio::test]
async fn a_change_of_what_it_declares_is_followed_and_told_at_once() {
    let ollama = server("Ответ Ollama.").await;
    let (app, _dir) = computer(&[]);
    app.pc.builtin.start();
    let mut events = app.events.subscribe();
    assert!(!app.pc.builtin.gives_here(Ability::Model) && !app.pc.builtin.contract().declares(Ability::Model));
    provider(&app, "ollama", &ollama.url);
    assert!(told(&mut events).await, "the panels and «Мозг» read again");
    assert!(
        app.pc.builtin.gives_here(Ability::Model) && app.pc.builtin.contract().ready(Ability::Model),
        "no look yet"
    );
    assert!(matches!(app.helpers.pick(Ability::Model), Some(Giver::Itself)));
    ollama.close().await;
}

#[tokio::test]
async fn a_closed_local_model_server_is_seen_at_the_next_look_and_a_cloud_one_is_not_ours() {
    let ollama = server("Ответ Ollama.").await;
    let (app, _dir) = computer(&[]);
    app.pc.builtin.set_addresses(vec![Ipv4Addr::new(192, 168, 1, 20)]);
    let row = provider(&app, "ollama", &ollama.url);
    // Another machine of the home (a NAS), a cloud: models of «Мозг», none of them this computer's.
    provider(&app, "custom-openai", "http://192.168.1.50:11434/v1");
    provider(&app, "custom-openai", "https://llm.example.invalid/v1");
    let mut events = app.events.subscribe();
    app.pc.builtin.check().await;
    let looked = OwnModel { provider: row.clone(), base_url: ollama.url.clone(), up: Some(true) };
    assert_eq!(app.pc.builtin.models(), std::slice::from_ref(&looked));
    assert!(app.pc.builtin.contract().ready(Ability::Model));

    ollama.close().await;
    app.pc.builtin.check().await;
    assert_eq!(app.pc.builtin.models(), [OwnModel { up: Some(false), ..looked }]);
    let contract = app.pc.builtin.contract();
    assert!(contract.declares(Ability::Model) && !contract.ready(Ability::Model), "its server is closed");
    let body = status(&app).await;
    assert_eq!(body["llm"], true, "the old field stays: a model is set up here");
    let model = body["contract"]["abilities"].as_array().unwrap().iter().find(|ability| ability["id"] == "model");
    assert_eq!(model.map(|model| &model["ready"]), Some(&json!(false)), "a station reads it closed: {body}");
    let mut told = 0;
    while let Ok(event) = events.try_recv() {
        told += usize::from(matches!(
            &*event,
            GatewayEvent::HelpersChanged { id: Some(id), change, .. } if id == SELF && change == "contract"
        ));
    }
    assert_eq!(told, 2, "the first look, then the server closed");
}

/// Its recognition and voice servers are looked at with its model servers (review of the merge with main, 16.11).
/// Closed, they are not ready for the gateways it helps — a station of 0.11 reads `asr: false` too —, and «Где
/// сейчас» here names the road that answers and says why; the computer itself still tries them first.
#[tokio::test]
async fn a_closed_recognition_or_voice_server_is_seen_at_the_next_look() {
    let ((asr, asr_port), (tts, tts_port)) = (listening().await, listening().await);
    let (app, _dir) =
        computer(&[("ASR_BASE_URL", asr.as_str()), ("TTS_OWN_URL", tts.as_str()), ("TTS_PROVIDER", "own")]);
    let row = |ability: Ability| {
        crate::helpers::now::rows(&crate::platform::http::Provides::provide(&app))
            .into_iter()
            .find(|row| row.ability == ability)
            .expect("a row")
    };
    app.pc.builtin.check().await;
    let contract = app.pc.builtin.contract();
    assert!(contract.ready(Ability::Hearing) && contract.ready(Ability::Voice));
    assert!(row(Ability::Hearing).first && row(Ability::Voice).first, "this computer gives both");

    drop(asr_port);
    refused(&asr).await;
    app.pc.builtin.check().await;
    let contract = app.pc.builtin.contract();
    assert!(!contract.ready(Ability::Hearing) && contract.ready(Ability::Voice), "its voice server still answers");
    assert!(app.pc.builtin.gives_here(Ability::Hearing), "tried first here all the same");
    let hearing = row(Ability::Hearing);
    assert_eq!((hearing.first, hearing.why.as_deref()), (false, Some("слух на этом компьютере не отвечает")));
    assert_eq!(status(&app).await["asr"], false);

    drop(tts_port);
    refused(&tts).await;
    app.pc.builtin.check().await;
    assert!(!app.pc.builtin.contract().ready(Ability::Voice));
    let voice = row(Ability::Voice);
    assert_eq!((voice.first, voice.why.as_deref()), (false, Some("голос на этом компьютере не отвечает")));
}

#[tokio::test]
async fn every_model_server_of_this_computer_is_looked_at() {
    let (ollama, lmstudio) = (server("Ollama.").await, server("LM Studio.").await);
    let (app, _dir) = computer(&[]);
    let first = provider(&app, "ollama", &ollama.url);
    let second = provider(&app, "lmstudio", &lmstudio.url.replace("127.0.0.1", "localhost"));
    app.pc.builtin.check().await;
    let rows: Vec<(String, Option<bool>)> =
        app.pc.builtin.models().into_iter().map(|model| (model.provider, model.up)).collect();
    assert_eq!(rows, [(first.clone(), Some(true)), (second.clone(), Some(true))], "both, in the rows' order");

    lmstudio.close().await;
    app.pc.builtin.check().await;
    let rows: Vec<(String, Option<bool>)> =
        app.pc.builtin.models().into_iter().map(|model| (model.provider, model.up)).collect();
    assert_eq!(rows, [(first, Some(true)), (second, Some(false))], "the first unchanged");
    assert!(app.pc.builtin.contract().ready(Ability::Model), "the model it hands on is the first's");
    ollama.close().await;
}

#[tokio::test]
async fn an_entry_pointing_at_this_computer_is_itself() {
    let (app, _dir) = computer(&[("PORT", "7780"), ("PUBLIC_PORT", "7790")]);
    app.helpers.set_local_port(54321);
    app.pc.builtin.set_addresses(vec![Ipv4Addr::new(192, 168, 1, 20)]);
    let mine = [
        "http://127.0.0.1:7780",
        "http://localhost:7780",
        "http://192.168.1.20:7780",
        "http://127.0.0.1:7790",
        "http://[::1]:54321",
        "http://127.0.0.1:54321/",
    ];
    for url in mine {
        assert!(app.pc.builtin.is_me(url), "{url}");
    }
    let others =
        ["http://127.0.0.1:7781", "http://192.168.1.21:7780", "https://helper.example.com:7780", "http://192.168.1.20"];
    for url in others.into_iter().chain(["not an address"]) {
        assert!(!app.pc.builtin.is_me(url), "{url}");
    }
    // In the list it is this gateway itself: shown so, never looked at nor picked.
    let entry = HelperSetting {
        id: "home-me".into(),
        name: "Я".into(),
        url: "http://192.168.1.20:7780".into(),
        kind: HelperKind::Home,
        enabled: true,
        without: Vec::new(),
        helper_id: None,
    };
    app.secrets.set(&key_secret("home-me"), Some("vgw_me")).unwrap();
    app.helpers
        .write(None, |list| {
            list.push(entry.clone());
            Ok(Some(()))
        })
        .unwrap();
    assert!(app.helpers.is_self(&entry));
    assert_eq!(app.helpers.views()[1].state, HelperState::Itself);
    assert!(app.helpers.offering(Ability::Hearing).is_empty());
}

#[tokio::test]
async fn the_computers_list_starts_with_itself_and_its_own_parts_come_first() {
    let (app, _dir) = computer(&[
        ("ASR_BASE_URL", "http://127.0.0.1:8090/v1"),
        ("TTS_OWN_URL", "http://127.0.0.1:8091/v1"),
        ("TTS_PROVIDER", "mixed"),
    ]);
    let views = app.helpers.views();
    let me = &views[0];
    assert_eq!(
        (me.id.as_str(), me.name.as_str(), me.builtin, me.kind, me.state),
        (SELF, "Этот компьютер", true, HelperKind::Home, HelperState::Up)
    );
    assert!(me.enabled && me.allowed && me.without.is_empty() && me.url.is_none() && me.key.is_none());
    assert_eq!(me.contract, Some(crate::helpers::contract::VERSION));
    assert_eq!(me.helper.as_ref().map(|helper| helper.id.clone()), app.identity.gateway().map(|me| me.id));
    assert_eq!(me.abilities, app.pc.builtin.contract().abilities);
    let health = app.helpers.health();
    let brief = &health["list"][0];
    assert_eq!(
        (&brief["id"], &brief["builtin"], &brief["state"], &brief["allowed"]),
        (&json!("self"), &json!(true), &json!("up"), &json!(true))
    );
    // Nothing to move, rename, switch off or remove.
    assert!(app.helpers.order(&[SELF.to_string()], None).is_err());
    assert_eq!(app.helpers.update(SELF, Change::default(), None).unwrap_err().status, StatusCode::NOT_FOUND);
    assert_eq!(app.helpers.remove(SELF, None).unwrap_err().status, StatusCode::NOT_FOUND);
    // «Где сейчас»: its hearing and voice are the built-in helper's.
    let rows = crate::helpers::now::rows(&crate::platform::http::Provides::provide(&app));
    for ability in [Ability::Hearing, Ability::Voice] {
        let row = rows.iter().find(|row| row.ability == ability).unwrap();
        assert_eq!(
            (row.by, row.helper.as_deref(), row.name.as_deref(), row.reach, row.first),
            (
                crate::helpers::now::By::Helper,
                Some(SELF),
                Some("Этот компьютер"),
                Some(crate::helpers::Reach::Home),
                true
            ),
            "{ability:?}"
        );
    }
}
