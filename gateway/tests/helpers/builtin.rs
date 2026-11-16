//! The built-in helper end to end (docs/plans/home-and-helpers.md, block 1, stage 6): «на компьютере слух, голос и
//! модель идут через встроенного помощника». The computer is this build in its own role; its recognition server, its
//! voice server and its model servers (Ollama, LM Studio) are stand-ins on 127.0.0.1 — on this computer. That `/home/v1`
//! answers as before is told by serve.rs's and tests/placement.rs's tests, unchanged, and the built-in helper's own
//! (`the_status_answers_as_before_and_adds_the_contract`). The owner's default 6 on a Mac whose model is on this
//! computer, with a cloud key, is here too: no request leaves 127.0.0.1 — the cloud is only chosen, never asked. What
//! only the helpers' way gives a computer is here as well: its clouds in .env under the regulator, and the hearing and
//! voice of a helper of its list when it has none of its own (their clouds are `.invalid` names: never reached).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use duowave::config::ModelProfile;
use duowave::dialog::BrainSource;
use duowave::helpers::contract::Ability;
use duowave::helpers::{Giver, OwnParts, CHECK_EVERY};
use reqwest::header::HeaderName;
use reqwest::StatusCode;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::stand::{contract, stand, ALL, KEY};
use crate::support::{bearer, message, until, Gateway};

const PEER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
/// What only a wait on a closed server's connection would pass.
const AT_ONCE: Duration = Duration::from_secs(5);
/// Why another model answers in «Где сейчас».
const SILENT: &str = "модель на этом компьютере не отвечает";

/// The first admin's header.
async fn admin(gateway: &Gateway) -> (HeaderName, String) {
    let setup = gateway.post("/api/v1/auth/setup").json(&json!({"password": "helpers-admin-password"}));
    bearer(message(setup, StatusCode::CREATED).await["token"].as_str().expect("a token"))
}

/// The computer's recognition server: whatever it hears is «включи свет».
async fn recognition() -> String {
    let router = axum::Router::new()
        .route("/v1/audio/transcriptions", axum::routing::post(|| async { axum::Json(json!({"text": "включи свет"})) }))
        .route("/v1/models", axum::routing::get(|| async { axum::Json(json!({"data": [{"id": "test-asr"}]})) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    url
}

/// The computer's voice server: every line a short WAV; the lines it was asked.
async fn voice() -> (String, Arc<Mutex<Vec<String>>>) {
    let lines: Arc<Mutex<Vec<String>>> = Arc::default();
    let kept = lines.clone();
    let router = axum::Router::new().route(
        "/v1/audio/speech",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let lines = kept.clone();
            async move {
                lines.lock().unwrap().push(body["input"].as_str().unwrap_or_default().to_string());
                duowave::speech::audio::pcm::encode_wav(&[0i16; 160], 16000)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, lines)
}

/// A model server on this computer: each chat answered with its words and counted; closed, its port takes nothing.
struct Model {
    url: String,
    chats: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Model {
    fn chats(&self) -> usize {
        self.chats.load(Ordering::SeqCst)
    }

    /// It stops listening — for good once a connection to its port is refused: a connection may still complete for
    /// a moment after the listener is gone.
    async fn close(self) {
        let address = self.url.trim_start_matches("http://").trim_end_matches("/v1").to_string();
        self.task.abort();
        let _ = self.task.await;
        let refused = || async { tokio::net::TcpStream::connect(address.as_str()).await.is_err() };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !refused().await && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

async fn model(answer: &'static str) -> Model {
    let chats = Arc::new(AtomicUsize::new(0));
    let counted = chats.clone();
    let router = axum::Router::new()
        .route(
            "/v1/chat/completions",
            axum::routing::post(move || {
                let chats = counted.clone();
                async move {
                    chats.fetch_add(1, Ordering::SeqCst);
                    let chunk = json!({ "choices": [{ "delta": { "content": answer } }] });
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                        format!("data: {chunk}\n\ndata: [DONE]\n\n"),
                    )
                }
            }),
        )
        .route("/v1/models", axum::routing::get(|| async { axum::Json(json!({"data": [{"id": "local-model"}]})) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Model { url, chats, task }
}

/// A row of «Мозг» added in the panel: its id.
async fn add(gateway: &Gateway, (header, auth): &(HeaderName, String), row: Value) -> String {
    let added = gateway.post("/api/v1/brain/providers").header(header.clone(), auth.clone()).json(&row);
    message(added, StatusCode::CREATED).await["id"].as_str().expect("an id").to_string()
}

/// «Мозг»'s choice, as the panel sets it: «Мозг» as it answers then.
async fn choose(gateway: &Gateway, (header, auth): &(HeaderName, String), choice: Value) -> Value {
    message(gateway.put("/api/v1/brain/choice").header(header.clone(), auth.clone()).json(&choice), StatusCode::OK)
        .await
}

/// «Помощники» as the panel reads it.
async fn helpers(gateway: &Gateway, (header, auth): &(HeaderName, String)) -> Value {
    message(gateway.get("/api/v1/helpers").header(header.clone(), auth.clone()), StatusCode::OK).await
}

fn now<'a>(view: &'a Value, ability: &str) -> &'a Value {
    view["now"].as_array().unwrap().iter().find(|row| row["ability"] == ability).expect("a row")
}

/// «Спросить мозг»: the events of its answer, and how long it took.
async fn ask(gateway: &Gateway, (header, auth): &(HeaderName, String), question: &str) -> (Vec<Value>, Duration) {
    let started = Instant::now();
    let response = gateway
        .post("/api/v1/brain/ask")
        .header(header.clone(), auth.clone())
        .json(&json!({"question": question}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    let events =
        body.lines().filter_map(|line| line.strip_prefix("data: ")).filter_map(|data| serde_json::from_str(data).ok());
    (events.collect(), started.elapsed())
}

fn of_kind<'a>(events: &'a [Value], kind: &str) -> Vec<&'a Value> {
    events.iter().filter(|event| event["type"] == kind).collect()
}

#[tokio::test]
async fn the_computer_hears_speaks_and_answers_through_its_built_in_helper() {
    let asr = recognition().await;
    let (voice_url, lines) = voice().await;
    let (ollama, lmstudio) = (model("Ответ Ollama.").await, model("Ответ LM Studio.").await);
    let pc = Gateway::start(&[
        ("ASR_PROVIDER", "openai"),
        ("ASR_BASE_URL", asr.as_str()),
        ("ASR_MODEL", "test-asr"),
        ("TTS_OWN_URL", voice_url.as_str()),
        ("TTS_PROVIDER", "mixed"),
    ])
    .await;
    let auth = admin(&pc).await;

    // Hearing: the chain says what the built-in helper's transcriber says, and hears through it.
    let health = message(pc.get("/health"), StatusCode::OK).await;
    assert_eq!(health["speech"]["asr"], format!("test-asr via {asr}/audio/transcriptions"));
    assert!(matches!(pc.app.helpers.pick(Ability::Hearing), Some(Giver::Itself)));
    let recognizer = pc.app.speech.recognizer().expect("the computer hears");
    let heard = recognizer.recognize(&[0i16; 1600], 16000, &CancellationToken::new()).await;
    assert_eq!(heard.unwrap(), "включи свет");

    // Voice: its own engine, the built-in helper's.
    assert!(matches!(pc.app.helpers.pick(Ability::Voice), Some(Giver::Itself)));
    assert!(Arc::ptr_eq(&pc.app.voice.speaker().unwrap(), pc.app.voice.own_speech().unwrap()));
    let said = pc.post("/api/v1/talk/speech").header(auth.0.clone(), auth.1.clone()).json(&json!({"text": "Привет"}));
    let response = said.send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(lines.lock().unwrap().iter().any(|line| line == "Привет"), "spoken by its voice server");

    // The model: Ollama on this computer answers, LM Studio on it is the backup. The built-in helper follows «Мозг» at
    // once, before any look of its: the computer takes its model from itself.
    let first = add(&pc, &auth, json!({"preset": "ollama", "baseUrl": ollama.url})).await;
    let second = add(&pc, &auth, json!({"preset": "lmstudio", "baseUrl": lmstudio.url})).await;
    let pick = |provider: &str| json!({"provider": provider, "model": "local-model"});
    choose(&pc, &auth, json!({"fast": pick(&first), "backup": pick(&second)})).await;
    assert!(until(5000, || matches!(pc.app.helpers.pick(Ability::Model), Some(Giver::Itself))).await);
    let (events, _) = ask(&pc, &auth, "Который час?").await;
    assert_eq!(of_kind(&events, "text")[0]["text"], "Ответ Ollama.", "{events:?}");
    assert_eq!(ollama.chats(), 1);

    // «Помощники»: this computer first; «Где сейчас»: its hearing, voice and model from it.
    let view = helpers(&pc, &auth).await;
    let me = &view["helpers"][0];
    assert_eq!(
        (me["id"].as_str(), me["name"].as_str(), me["builtin"].as_bool(), me["state"].as_str()),
        (Some("self"), Some("Этот компьютер"), Some(true), Some("up")),
        "{me}"
    );
    assert!(me.get("url").is_none() && me.get("key").is_none());
    for ability in ["hearing", "voice", "model"] {
        let row = now(&view, ability);
        assert_eq!(
            (row["by"].as_str(), row["helper"].as_str(), row["name"].as_str(), row["first"].as_bool()),
            (Some("helper"), Some("self"), Some("Этот компьютер"), Some(true)),
            "{ability}: {row}"
        );
    }

    // Ollama closes: the helpers' next tick sees it — nobody asks for a look —, and LM Studio answers at once; the
    // closed one is not waited for.
    ollama.close().await;
    let ticks = 2 * CHECK_EVERY.as_millis() as u64;
    let backup = || BrainSource::slot(&*pc.app.brain, ModelProfile::Fast).is_some_and(|slot| slot.provider == second);
    assert!(until(ticks, backup).await, "the backup without a request to the closed server, after a tick");
    let row = now(&helpers(&pc, &auth).await, "model").clone();
    assert_eq!((row["first"].as_bool(), row["why"].as_str()), (Some(false), Some(SILENT)), "{row}");
    let (events, took) = ask(&pc, &auth, "А погода?").await;
    assert_eq!(of_kind(&events, "text")[0]["text"], "Ответ LM Studio.", "{events:?}");
    assert!(of_kind(&events, "error").is_empty(), "{events:?}");
    assert_eq!(lmstudio.chats(), 1);
    assert!(took < AT_ONCE, "{took:?}");
    lmstudio.close().await;
    pc.stop().await;
}

#[tokio::test]
async fn a_computer_in_the_station_role_takes_nothing_from_itself() {
    let computer = stand(contract("pc", ALL)).await;
    let station = Gateway::start(&[("DUOWAVE_ROLE", "station"), ("PEER_KEY", PEER)]).await;
    let auth = admin(&station).await;
    let body = json!({"url": computer.url, "key": KEY, "name": "Компьютер дома"});
    let added = message(
        station.post("/api/v1/helpers").header(auth.0.clone(), auth.1.clone()).json(&body),
        StatusCode::CREATED,
    )
    .await;
    let id = added["helpers"][0]["id"].as_str().unwrap().to_string();
    let view = helpers(&station, &auth).await;
    let ids: Vec<&str> =
        view["helpers"].as_array().unwrap().iter().filter_map(|helper| helper["id"].as_str()).collect();
    assert_eq!(ids, [id.as_str()], "no row of its own");
    assert!(view["now"].as_array().unwrap().iter().all(|row| row["helper"] != "self"), "{}", view["now"]);
    let health = message(station.get("/health"), StatusCode::OK).await;
    assert!(health["placement"]["helpers"]["list"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry.get("builtin").is_none()));
    // Its apps, hearing and voice are the helper's, as a speaker's are.
    let apps = station.app.helpers.link_for(Ability::Apps);
    assert_eq!(apps.up().map(|(base, _)| base), Some(computer.url.clone()));
    for ability in [Ability::Hearing, Ability::Voice, Ability::Model, Ability::History, Ability::Catalog] {
        let giver = station.app.helpers.pick(ability);
        assert!(matches!(&giver, Some(Giver::Helper(remote)) if remote.id() == id), "{ability:?}");
    }
    station.stop().await;
}

/// The computer's own clouds in .env — its recognition and voice servers at a cloud's address — go under the regulator
/// as a station's do: «Любые облака» hears and speaks through them; «Только дома» keeps them in («off», no voice of
/// ours), and «Где сейчас» says why. A cloud's server is never the built-in helper's.
#[tokio::test]
async fn the_computers_own_clouds_go_under_the_regulator() {
    let pc = Gateway::start(&[
        ("ASR_PROVIDER", "openai"),
        ("ASR_BASE_URL", "https://asr.example.invalid/v1"),
        ("TTS_OWN_URL", "https://tts.example.invalid/v1"),
        ("TTS_PROVIDER", "mixed"),
    ])
    .await;
    let auth = admin(&pc).await;
    assert!(pc.app.speech.recognizes() && pc.app.speech.recognizer().is_some(), "«Любые облака»: its cloud hears");
    let asr = message(pc.get("/health"), StatusCode::OK).await["speech"]["asr"].clone();
    assert!(asr.as_str().unwrap().contains("https://asr.example.invalid/v1"), "{asr}");
    let speaker = pc.app.voice.speaker().map(|speaker| speaker.base_url());
    assert_eq!(speaker.as_deref(), Some("https://tts.example.invalid/v1"), "and speaks");
    assert!(!pc.app.builtin.gives_here(Ability::Hearing) && !pc.app.builtin.gives_here(Ability::Voice));
    // The registry says where as `/health` does: through the clouds, never «на этом компьютере».
    let ways = |registry: Value| -> Vec<(String, String)> {
        let features = registry["features"].as_array().unwrap().clone();
        ["asr.own", "voice.own"]
            .into_iter()
            .map(|id| features.iter().find(|feature| feature["id"] == id).expect("a feature").clone())
            .map(|way| (way["state"].as_str().unwrap().to_string(), way["runs"].as_str().unwrap().to_string()))
            .collect()
    };
    let registry = || pc.get("/api/v1/availability").header(auth.0.clone(), auth.1.clone());
    let through = |runs: &str| vec![("fallback".to_string(), runs.to_string()); 2];
    assert_eq!(ways(message(registry(), StatusCode::OK).await), through("cloud"));

    let away = pc.put("/api/v1/helpers/away").header(auth.0.clone(), auth.1.clone()).json(&json!({"away": "home"}));
    let view = message(away, StatusCode::OK).await;
    assert_eq!(message(pc.get("/health"), StatusCode::OK).await["speech"]["asr"], "off");
    assert!(!pc.app.speech.recognizes() && pc.app.speech.recognizer().is_none(), "the cloud does not hear");
    assert!(pc.app.voice.speaker().is_none(), "the cloud voice is kept in");
    for ability in ["hearing", "voice"] {
        let row = now(&view, ability);
        assert_eq!(row["why"], "Регулятор не пускает облако", "{ability}: {row}");
    }
    assert_eq!(ways(message(registry(), StatusCode::OK).await), through("yandex"), "nothing of its own hears");
    pc.stop().await;
}

/// A computer with no hearing or voice of its own takes them from a helper of its list, as a station does: the chain
/// hears through it, and our voice is its.
#[tokio::test]
async fn a_computer_without_hearing_or_voice_of_its_own_takes_them_from_a_helper_of_its_list() {
    let other = stand(contract("laptop", ALL)).await;
    let pc = Gateway::start(&[]).await;
    let auth = admin(&pc).await;
    assert_eq!(message(pc.get("/health"), StatusCode::OK).await["speech"]["asr"], "off", "recognition off here");
    assert!(pc.app.speech.recognizer().is_none() && pc.app.voice.speaker().is_none());

    let body = json!({"url": other.url, "key": KEY, "name": "Ноутбук"});
    let added =
        message(pc.post("/api/v1/helpers").header(auth.0.clone(), auth.1.clone()).json(&body), StatusCode::CREATED)
            .await;
    let ids: Vec<&str> =
        added["helpers"].as_array().unwrap().iter().filter_map(|helper| helper["id"].as_str()).collect();
    assert_eq!((ids.len(), ids[0]), (2, "self"), "itself first, then the helper: {ids:?}");
    let id = ids[1].to_string();
    let asr = message(pc.get("/health"), StatusCode::OK).await["speech"]["asr"].clone();
    assert!(asr.as_str().unwrap().starts_with(&format!("helper {id}: ")), "{asr}");
    let recognizer = pc.app.speech.recognizer().expect("it hears through its helper");
    let heard = recognizer.recognize(&[0i16; 1600], 16000, &CancellationToken::new()).await;
    assert_eq!(heard.unwrap(), "слышу");
    let speaker = pc.app.voice.speaker().expect("our voice is its helper's");
    assert!(speaker.base_url().starts_with(&other.url), "{}", speaker.base_url());
    // The registry says where as «Где сейчас» does: at the helper, not «на этом компьютере».
    let registry = message(pc.get("/api/v1/availability").header(auth.0.clone(), auth.1.clone()), StatusCode::OK).await;
    let feature = |id: &str| {
        let features = registry["features"].as_array().unwrap();
        features.iter().find(|feature| feature["id"] == id).expect("a feature").clone()
    };
    for id in ["asr.own", "voice.own"] {
        let way = feature(id);
        assert_eq!((way["runs"].as_str(), way["reason"].as_str()), (Some("homeServer"), Some("у Турбо")), "{way}");
    }
    let builtin = registry["helpers"]["builtin"].as_array().unwrap();
    assert!(!builtin.contains(&json!("hearing")) && !builtin.contains(&json!("voice")), "{}", registry["helpers"]);
    let view = message(pc.get("/api/v1/helpers").header(auth.0.clone(), auth.1.clone()), StatusCode::OK).await;
    assert_eq!(now(&view, "hearing")["helper"].as_str(), Some(id.as_str()), "{}", view["now"]);
    pc.stop().await;
}

/// The owner's default 6 on a Mac (hand check 2–4): the main model on this computer (Ollama), no backup, a cloud key
/// (OpenAI) — the gateway takes the cloud's fast model as the backup itself and «Мозг» says so; the model may leave the
/// house from then on, so the folders without their tick are closed to it and the apps' questions in words want
/// «Вопросы словами через облако»; Ollama closed, the cloud answers at once; «Только дома», honest words at once.
#[tokio::test]
async fn a_mac_whose_model_is_on_this_computer_gets_a_cloud_backup_by_itself_and_says_so() {
    let ollama = model("Ответ Ollama.").await;
    let pc = Gateway::start(&[]).await;
    let auth = admin(&pc).await;
    let local = add(&pc, &auth, json!({"preset": "ollama", "baseUrl": ollama.url})).await;
    let brain = choose(&pc, &auth, json!({"fast": {"provider": local, "model": "local-model"}})).await;
    assert_eq!(brain["note"]["kind"], "backupNeeded", "no cloud key yet: {}", brain["note"]);

    // A cloud key, OpenAI's, whose models «Обновить» has listed (here written as it leaves them: no request out).
    let openai = add(&pc, &auth, json!({"preset": "openai"})).await;
    let listed = json!(["gpt-5", "gpt-5-mini"]).to_string();
    pc.app
        .db
        .with(|c| c.execute("UPDATE llm_providers SET models = ?2 WHERE id = ?1", rusqlite::params![openai, listed]))
        .unwrap();
    let key = pc.patch(&format!("/api/v1/brain/providers/{openai}")).header(auth.0.clone(), auth.1.clone());
    message(key.json(&json!({"key": "sk-test-openai-0001"})), StatusCode::OK).await;
    assert!(until(5000, || pc.app.brain.view().choice.backup.is_some()).await, "the gateway sets the backup itself");
    let brain = message(pc.get("/api/v1/brain").header(auth.0.clone(), auth.1.clone()), StatusCode::OK).await;
    assert_eq!(brain["choice"]["backup"], json!({"provider": openai, "model": "gpt-5-mini"}));
    assert_eq!(brain["choice"]["backupBy"], "gateway");
    let note = brain["note"]["text"].as_str().unwrap_or_default();
    assert_eq!(brain["note"]["kind"], "backupAuto", "{note}");
    assert!(note.contains("gpt-5-mini (OpenAI)") && note.contains("могут стоить денег"), "{note}");
    // A cloud backup behind the model at home: what the model reads may leave the house now.
    assert!(pc.app.dialog.in_cloud(), "folders without their tick closed, questions in words need the switch");
    assert!(pc.app.bus.files().expect("the folders").leaving().1, "the panel's amber");
    assert_eq!(ollama.chats(), 0, "nothing asked of the model to set it");

    // Ollama closes: OpenAI's model answers at once, and «Где сейчас» says why. The tick's look is taken here at once
    // (the first test waits for a real tick).
    ollama.close().await;
    pc.app.builtin.check().await;
    let slot = BrainSource::slot(&*pc.app.brain, ModelProfile::Fast).unwrap();
    assert_eq!((slot.provider.as_str(), slot.model.as_str(), slot.cloud), (openai.as_str(), "gpt-5-mini", true));
    let row = now(&helpers(&pc, &auth).await, "model").clone();
    assert_eq!(
        (row["by"].as_str(), row["name"].as_str(), row["first"].as_bool(), row["why"].as_str()),
        (Some("cloud"), Some("OpenAI"), Some(false), Some(SILENT)),
        "{row}"
    );

    // «Только дома»: honest words at once, the cloud is not called.
    let away = pc.put("/api/v1/helpers/away").header(auth.0.clone(), auth.1.clone()).json(&json!({"away": "home"}));
    message(away, StatusCode::OK).await;
    let (events, took) = ask(&pc, &auth, "Который час?").await;
    let errors = of_kind(&events, "error");
    assert_eq!(errors.len(), 1, "{events:?}");
    assert_eq!(
        errors[0]["message"],
        "Модель на этом компьютере не отвечает, а облака сейчас закрыты — запасная модель не ответит."
    );
    assert!(took < AT_ONCE, "{took:?}");
    let brain = message(pc.get("/api/v1/brain").header(auth.0.clone(), auth.1.clone()), StatusCode::OK).await;
    assert_eq!(brain["note"]["kind"], "backupClosed");
    pc.stop().await;
}
