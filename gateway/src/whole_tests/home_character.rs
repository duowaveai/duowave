//! The tests of `home/character.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body as HttpBody;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Map, Value};
use tower::ServiceExt;

use crate::app::App;
use crate::config::{Config, Env};
use crate::helpers::tests::{contract, KEY};
use crate::helpers::{AddInput, HelperKind};
use crate::home::character::*;
use crate::settings::definitions::HOME_VOICE_ENGINES;
use crate::whole_tests::characters_contributes::{arthur_manifest, write_arthur};

/// The Турбо as the centre reaches it: its status, the plan it answers (404 — an older one), what it was told; the
/// voice waiting there, the owner's answers it took and the languages it was asked in.
#[derive(Default)]
pub(crate) struct Turbo {
    url: String,
    old: std::sync::atomic::AtomicBool,
    asked: Mutex<Vec<Value>>,
    told: Mutex<Vec<Value>>,
    waiting: Mutex<Value>,
    pub(crate) answers: Mutex<Vec<Value>>,
    languages: Mutex<Vec<Option<String>>>,
}

async fn turbo() -> Arc<Turbo> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let turbo = Arc::new(Turbo { url: format!("http://{}", listener.local_addr().unwrap()), ..Turbo::default() });
    let (plans, tells) = (turbo.clone(), turbo.clone());
    let (looks, answers) = (turbo.clone(), turbo.clone());
    let router = Router::new()
        .route("/home/v1/status", get(|| async { axum::Json(contract("mac", &[("voice", false)])) }))
        .route(
            "/home/v1/character/plan",
            post(move |axum::Json(body): axum::Json<Value>| {
                let turbo = plans.clone();
                async move {
                    if turbo.old.load(std::sync::atomic::Ordering::SeqCst) {
                        return Err(StatusCode::NOT_FOUND);
                    }
                    let chosen = body["engine"].clone();
                    turbo.asked.lock().unwrap().push(body);
                    // CosyVoice does not fit on its disk.
                    let engines = json!([
                        { "id": "omnivoice", "title": "OmniVoice", "diskMb": 5_600, "freeDiskMb": 8_000, "diskFits": true },
                        { "id": "cosyvoice3", "title": "CosyVoice 3", "diskMb": 10_500, "freeDiskMb": 8_000, "diskFits": false },
                        { "id": "voxcpm", "title": "VoxCPM", "diskMb": 0, "freeDiskMb": 8_000, "diskFits": true }
                    ]);
                    Ok(axum::Json(json!({ "engines": engines, "chosen": chosen, "voiceOn": false, "computer": "Mac" })))
                }
            }),
        )
        .route(
            "/home/v1/character",
            post(move |axum::Json(body): axum::Json<Value>| {
                let turbo = tells.clone();
                async move {
                    turbo.told.lock().unwrap().push(body.clone());
                    axum::Json(json!({ "character": body["id"] }))
                }
            }),
        )
        .route(
            "/home/v1/voice/pending",
            get(move |headers: axum::http::HeaderMap| {
                let turbo = looks.clone();
                async move {
                    let language = headers.get("accept-language").and_then(|value| value.to_str().ok());
                    turbo.languages.lock().unwrap().push(language.map(str::to_string));
                    axum::Json(json!({ "pending": turbo.waiting.lock().unwrap().clone(), "computer": "Mac" }))
                }
            })
            .post(move |axum::Json(body): axum::Json<Value>| {
                let turbo = answers.clone();
                async move {
                    turbo.answers.lock().unwrap().push(body);
                    let mut waiting = turbo.waiting.lock().unwrap();
                    if waiting.is_null() {
                        let refused = json!({ "code": "conflict", "message": "Голос уже не ждёт." });
                        return Err((StatusCode::CONFLICT, axum::Json(refused)));
                    }
                    *waiting = Value::Null;
                    Ok(axum::Json(json!({ "pending": null, "computer": "Mac" })))
                }
            }),
        );
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    turbo
}

/// The centre (a station's gateway) with Артур installed and chosen, its Турбо on the line; its feed when given.
pub(crate) async fn centre(dir: &std::path::Path, feed: Option<&str>) -> (Arc<App>, Arc<Turbo>) {
    let app = station(dir, feed).await;
    let turbo = turbo().await;
    let add = AddInput { url: turbo.url.clone(), key: KEY.into(), kind: Some(HelperKind::Own), ..AddInput::default() };
    app.helpers.add(add, None).await.unwrap();
    (app, turbo)
}

/// A station's gateway with Артур installed and chosen, no Турбо in its home yet; its feed when given.
async fn station(dir: &std::path::Path, feed: Option<&str>) -> Arc<App> {
    let text = |sub: &str| dir.join(sub).to_string_lossy().into_owned();
    let env = Env::from_pairs([
        ("STATE_DIR", String::new()),
        ("DATA_DIR", text("data")),
        ("PACKAGES_DIR", text("packages")),
        ("PLUGINS_BUNDLED", "false".into()),
        ("DUOWAVE_ROLE", "station".into()),
        ("PACKAGES_FEED_URL", feed.unwrap_or("off").into()),
    ]);
    let app = App::build(Config::for_tests(&env)).unwrap();
    write_arthur(&app.config.plugins.packages_dir, &arthur_manifest("1.2.0"));
    let body = crate::characters::ChooseBody { voice: Some(false), ..Default::default() };
    app.characters.choose("arthur:arthur", body, false, None).await.unwrap();
    app
}

pub(crate) fn key(app: &App, grants: &[&str]) -> String {
    let input = Map::from_iter([("name".into(), json!("Пульт")), ("grants".into(), json!(grants))]);
    format!("Bearer {}", app.access.keys.create(&input, None).unwrap().secret)
}

pub(crate) async fn send(app: &Arc<App>, method: &str, uri: &str, key: &str, body: Value) -> (StatusCode, Value) {
    send_in(app, method, uri, key, body, None).await
}

/// [`send`] from a panel in a language (`Accept-Language`).
async fn send_in(
    app: &Arc<App>,
    method: &str,
    uri: &str,
    key: &str,
    body: Value,
    language: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", key);
    if let Some(language) = language {
        request = request.header("accept-language", language);
    }
    let mut request = request.body(HttpBody::from(body.to_string())).unwrap();
    request.extensions_mut().insert(ConnectInfo(std::net::SocketAddr::new("192.168.1.20".parse().unwrap(), 5000)));
    let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    // The gateway's wrapper: `{code, message}`, the answer in `message`.
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body["message"].clone())
}

/// What the Турбо was told last, once it has been told.
async fn told(turbo: &Turbo, count: usize) -> Value {
    for _ in 0..100 {
        if let Some(last) = turbo.told.lock().unwrap().get(count - 1).cloned() {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("the Турбо was not told");
}

/// Clean-path check 18.11, item 40: the panel reads the Турбо's plan through the centre with the character's
/// models; the owner's choice is kept at the centre (by the character, or by the package before it is installed),
/// named to the Турбо at once and again whenever it is told (`helpers.up`); an older Турбо has no plan.
#[tokio::test]
async fn the_centre_asks_the_turbos_plan_and_names_the_chosen_model_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let (app, turbo) = centre(dir.path(), None).await;
    let (viewer, owner) = (key(&app, &["station.view"]), key(&app, &["station.view", "models.manage"]));

    let (status, view) = send(&app, "GET", "/api/v1/home/voice?character=arthur:arthur", &viewer, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["turbo"]["up"], true, "{view}");
    assert_eq!((&view["plan"]["computer"], &view["chosen"]), (&json!("Mac"), &Value::Null));
    let asked = turbo.asked.lock().unwrap()[0].clone();
    assert_eq!(asked["id"], "arthur:arthur");
    assert_eq!(asked["package"], json!({ "id": "arthur", "version": "1.2.0" }));
    assert_eq!(asked["voices"], json!(["cosyvoice3", "omnivoice"]));
    assert_eq!((&asked["reference"], &asked["gender"]), (&json!(true), &json!("male")));
    assert_eq!(asked["fallbackVoice"], "piper-denis");

    // Choosing takes the right to the voices.
    let choice = json!({ "character": "arthur:arthur", "engine": "voxcpm" });
    let (status, _) = send(&app, "PUT", "/api/v1/home/voice", &viewer, choice.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let wrong = json!({ "character": "arthur:arthur", "engine": "f5tts" });
    let (status, _) = send(&app, "PUT", "/api/v1/home/voice", &owner, wrong).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // A model that does not fit on the Турбо's disk: refused, not kept, nothing told.
    let big = json!({ "character": "arthur:arthur", "engine": "cosyvoice3" });
    let (status, refused) = send(&app, "PUT", "/api/v1/home/voice", &owner, big).await;
    assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE);
    assert_eq!(refused, "Модель CosyVoice 3 не поместится: нужно 10 ГБ, свободно 7,8 ГБ.");
    assert!(app.settings.get(&HOME_VOICE_ENGINES, None).is_none());
    let (status, view) = send(&app, "PUT", "/api/v1/home/voice", &owner, choice).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!((&view["chosen"], &view["plan"]["chosen"]), (&json!("voxcpm"), &json!("voxcpm")));
    // Named to the Турбо at once; no feed here, so no archive.
    let first = told(&turbo, 1).await;
    assert_eq!((&first["id"], &first["engine"]), (&json!("arthur:arthur"), &json!("voxcpm")));
    assert!(first.get("archive").is_none(), "{first}");
    assert_eq!(turbo.told.lock().unwrap().len(), 1, "the refused one was never told");
    // The Турбо back on the line is told the same.
    tell(&crate::platform::http::Provides::provide(&app)).await.unwrap();
    assert_eq!(told(&turbo, 2).await["engine"], "voxcpm");

    // An older Турбо: no plan, the panel goes as before.
    turbo.old.store(true, std::sync::atomic::Ordering::SeqCst);
    let (_, view) = send(&app, "GET", "/api/v1/home/voice?character=arthur:arthur", &viewer, Value::Null).await;
    assert_eq!((&view["plan"], &view["chosen"], &view["turbo"]["up"]), (&Value::Null, &json!("voxcpm"), &json!(true)));
}

/// Before the package is installed at the centre: its plan from the feed's entry, the choice kept by the package and
/// named with the character once it is installed — with the archive the centre's feed names for that version.
#[tokio::test]
async fn a_choice_made_before_the_install_goes_with_the_character_and_its_archive() {
    use crate::plugins::catalog::tests::{arthur, site};
    use crate::plugins::install::tests::{character_manifest, vgp};
    use sha2::Digest;
    let site = site().await;
    let mut jarvis = arthur("1.0.0");
    jarvis["id"] = json!("jarvis");
    jarvis["needs"] = json!([
        { "kind": "engine", "id": "omnivoice" }, { "kind": "engine", "id": "f5tts" }, { "kind": "memory", "mb": 10 }
    ]);
    let archive = vgp(&character_manifest("arthur", "1.2.0"));
    site.publish(&[(jarvis, b"jarvis archive".to_vec()), (arthur("1.2.0"), archive.clone())]);
    let dir = tempfile::tempdir().unwrap();
    let (app, turbo) = centre(dir.path(), Some(&site.url)).await;
    let owner = key(&app, &["station.view", "models.manage"]);

    let (status, view) = send(&app, "GET", "/api/v1/home/voice?package=jarvis", &owner, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let asked = turbo.asked.lock().unwrap().last().cloned().unwrap();
    assert_eq!(asked["package"], json!({ "id": "jarvis", "version": "1.0.0" }));
    assert_eq!((&asked["voices"], &asked["reference"]), (&json!(["omnivoice"]), &json!(true)));
    let (status, _) = send(&app, "GET", "/api/v1/home/voice?package=nobody", &owner, Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Arthur's package chosen by the package: its installed character goes with it, with the feed's archive.
    let choice = json!({ "package": "arthur", "engine": "omnivoice" });
    let (status, view) = send(&app, "PUT", "/api/v1/home/voice", &owner, choice).await;
    assert_eq!((status, &view["chosen"]), (StatusCode::OK, &json!("omnivoice")), "{view}");
    let first = told(&turbo, 1).await;
    assert_eq!((&first["id"], &first["engine"]), (&json!("arthur:arthur"), &json!("omnivoice")));
    assert_eq!(first["archive"]["url"], site.url.replace("catalog.json", "packages/arthur-1.2.0.vgp"));
    assert_eq!(first["archive"]["sha256"], hex::encode(sha2::Sha256::digest(&archive)));
    // The character's own choice goes before its package's.
    let mut chosen = app.settings.get(&HOME_VOICE_ENGINES, None).unwrap();
    chosen.insert("arthur:arthur".into(), "voxcpm".into());
    app.settings.set(&HOME_VOICE_ENGINES, &chosen, None).unwrap();
    assert_eq!(of_home(&app.settings, &app.characters).unwrap()["engine"], "voxcpm");

    // The owner's last word wins (21.11): chosen again by the package (the wizard's), it is that one, not the
    // character's older one.
    let again = json!({ "package": "arthur", "engine": "omnivoice" });
    let (status, view) = send(&app, "PUT", "/api/v1/home/voice", &owner, again).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(of_home(&app.settings, &app.characters).unwrap()["engine"], "omnivoice");
    assert_eq!(told(&turbo, 2).await["engine"], "omnivoice");
    let kept = app.settings.get(&HOME_VOICE_ENGINES, None).unwrap();
    assert_eq!(kept, BTreeMap::from([("arthur".to_string(), "omnivoice".to_string())]));
    // Removed, the package's choices go with it.
    forget_package(&app.settings, &app.characters, "arthur");
    assert!(app.settings.get(&HOME_VOICE_ENGINES, None).unwrap().is_empty());
    assert!(of_home(&app.settings, &app.characters).unwrap().get("engine").is_none());
}

/// One choice for a character's voice at a time: by its package or by itself, whichever came last; a package's
/// removal drops both, and nothing of another package.
#[test]
fn the_last_choice_of_a_voice_model_is_the_one_kept() {
    let mut chosen = BTreeMap::from([
        ("rick:rick".to_string(), "voxcpm".to_string()),
        ("rick:summer".to_string(), "voxcpm".to_string()),
        ("arthur:arthur".to_string(), "omnivoice".to_string()),
    ]);
    keep_choice(&mut chosen, "rick", false, "cosyvoice3");
    let expected = |pairs: &[(&str, &str)]| {
        pairs.iter().map(|(key, engine)| (key.to_string(), engine.to_string())).collect::<BTreeMap<_, _>>()
    };
    assert_eq!(chosen, expected(&[("rick", "cosyvoice3"), ("arthur:arthur", "omnivoice")]));
    keep_choice(&mut chosen, "rick:rick", true, "omnivoice");
    assert_eq!(chosen, expected(&[("rick:rick", "omnivoice"), ("arthur:arthur", "omnivoice")]));
    // A package whose id starts like another's is not that one.
    keep_choice(&mut chosen, "ric", false, "voxcpm");
    assert_eq!(chosen.len(), 3);
    assert!(drop_package(&mut chosen, "rick"));
    assert_eq!(chosen, expected(&[("arthur:arthur", "omnivoice"), ("ric", "voxcpm")]));
    assert!(!drop_package(&mut chosen, "morty"));
}

/// Clean-path check 19.11: «Голос ждёт» lived only on the computer's own page. The centre shows any panel of the
/// home the voice waiting on its Турбо (`GET /api/v1/home/voice/pending`, asked in the panel's language so the
/// reason comes back in it) and passes the owner's answer there (`POST`, with the right to the voices); the Турбо's
/// refusal comes back in its words. With no Турбо nothing waits, and an answer has nowhere to go. A computer that is
/// the centre answers its own waiting voice itself (src/computer/tests/home_character_computer.rs).
#[tokio::test]
async fn the_centre_shows_the_voice_waiting_on_its_turbo_and_passes_the_owners_answer_there() {
    const PENDING: &str = "/api/v1/home/voice/pending";
    let dir = tempfile::tempdir().unwrap();
    let (app, turbo) = centre(dir.path(), None).await;
    let (viewer, owner) = (key(&app, &["station.view"]), key(&app, &["station.view", "models.manage"]));

    let (status, view) = send(&app, "GET", PENDING, &viewer, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    // The Турбо by its entry here and its own name.
    let entry = app.helpers.views().into_iter().next().unwrap().id;
    assert_eq!((&view["turbo"]["id"], &view["turbo"]["up"]), (&json!(entry), &json!(true)), "{view}");
    assert_eq!(view["turbo"]["name"], "Турбо mac", "{view}");
    assert_eq!(view["pending"], Value::Null, "nothing waits there: {view}");

    // A voice waits on the Турбо: seen through the centre, the Турбо asked in the panel's language.
    let waiting = json!({
        "modelId": "own:c-arthur-arthur-omnivoice", "state": "tight", "title": "Артур",
        "memory": { "kind": "short", "freeMb": 2048, "needMb": 5120 },
        "reason": "Memory is low: 2,0 GB is free, but about 5,0 GB is needed"
    });
    *turbo.waiting.lock().unwrap() = waiting.clone();
    let (status, view) = send_in(&app, "GET", PENDING, &viewer, Value::Null, Some("en-GB,en;q=0.9")).await;
    assert_eq!((status, &view["pending"]), (StatusCode::OK, &waiting), "{view}");
    assert_eq!(turbo.languages.lock().unwrap().last().cloned().flatten().as_deref(), Some("en"));

    // The answer takes the right to the voices; it goes to the Турбо, and what waits now comes back.
    let (status, _) = send(&app, "POST", PENDING, &viewer, json!({ "answer": "anyway" })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(turbo.answers.lock().unwrap().is_empty(), "a viewer's answer never reaches the Турбо");
    let (status, view) = send(&app, "POST", PENDING, &owner, json!({ "answer": "anyway" })).await;
    assert_eq!((status, &view["pending"]), (StatusCode::OK, &Value::Null), "{view}");
    assert_eq!(view["turbo"]["id"], json!(entry), "{view}");
    assert_eq!(turbo.answers.lock().unwrap().clone(), [json!({ "answer": "anyway" })]);
    // The Турбо refuses — nothing waits there any more: its status, its words.
    let (status, refused) = send(&app, "POST", PENDING, &owner, json!({ "answer": "cancel" })).await;
    assert_eq!((status, &refused), (StatusCode::CONFLICT, &json!("Голос уже не ждёт.")));

    // No Турбо in the home: nothing waits, an answer has nowhere to go.
    let lone = tempfile::tempdir().unwrap();
    let app = station(lone.path(), None).await;
    let owner = key(&app, &["station.view", "models.manage"]);
    let (status, view) = send(&app, "GET", PENDING, &owner, Value::Null).await;
    assert_eq!((status, &view), (StatusCode::OK, &json!({ "turbo": null, "pending": null })));
    let (status, refused) = send_in(&app, "POST", PENDING, &owner, json!({ "answer": "anyway" }), Some("ru")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused, "Компьютер с голосом не на связи — ответить ему пока нельзя.");
}
