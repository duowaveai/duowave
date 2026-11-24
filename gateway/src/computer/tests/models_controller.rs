//! The tests of `models/models_controller.rs` that need the whole gateway (`App`): its crate does not have it, the
//! program does.

use std::net::SocketAddr;
use std::path::Path as FsPath;
use std::sync::Arc;

use axum::body::Body as AxumBody;
use axum::extract::rejection::PathRejection;
use axum::extract::{ConnectInfo, Path, RawQuery};
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::app::App;
use crate::config::{Config, Env};
use crate::core::identity::principal::user_principal;
use crate::core::identity::Role;
use crate::models::models_controller::*;
use crate::platform::http::{ApiError, ApiResult, Body, Caller, Uses};

/// An empty registry: the catalog is the showcase.
async fn registry() -> String {
    let router = Router::new().route("/tree", get(|| async { axum::Json(json!([])) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}/tree")
}

/// The gateway with nothing on disk but a folder for the voices: no recognition of ours, no voice server.
async fn app(models: &FsPath) -> Arc<App> {
    let env =
        Env::from_pairs([("STATE_DIR", ""), ("ASR_PROVIDER", "none"), ("TTS_MODELS_DIR", models.to_str().unwrap())]);
    let mut config = Config::for_tests(&env);
    config.models.piper_registry = registry().await;
    let app = App::build(config).unwrap();
    app.pc.models.start().await.unwrap();
    app
}

async fn call(app: &Arc<App>, method: &str, uri: &str, from: [u8; 4]) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri).body(AxumBody::empty()).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::from((from, 50_000))));
    let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn answer<T: serde::Serialize>(result: ApiResult<T>) -> (StatusCode, Value) {
    let response = result.into_response();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn admin(app: &Arc<App>) -> Caller {
    Caller::local(app.clone(), Some(user_principal("u1", "Аня", Role::Admin, None)))
}

fn member(app: &Arc<App>) -> Caller {
    Caller::local(app.clone(), Some(user_principal("u2", "Боря", Role::Member, None)))
}

fn id(value: &str) -> Result<Path<String>, PathRejection> {
    Ok(Path(value.to_string()))
}

/// The refusal a handler was due to give.
fn refused<T>(result: ApiResult<T>) -> ApiError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("an answer where a refusal was due"),
    }
}

const LOCAL: [u8; 4] = [127, 0, 0, 1];

#[tokio::test]
async fn answers_in_the_envelope_and_asks_for_a_sign_in_on_every_route() {
    let models = tempfile::tempdir().unwrap();
    let app = app(models.path()).await;
    let (status, body) = call(&app, "GET", "/api/v1/models", LOCAL).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!((body["code"].as_u64(), body["message"].as_str()), (Some(401), Some("Нужно войти.")));
    assert_eq!(body["path"], "/api/v1/models");
    // «silero» is a card's id as well as the file route's segment: both reach their handlers.
    for (method, uri) in [
        ("POST", "/api/v1/models/tts/silero/install"),
        ("POST", "/api/v1/models/tts/silero/cancel"),
        ("DELETE", "/api/v1/models/tts/silero"),
        ("POST", "/api/v1/models/tts/silero/file"),
        ("POST", "/api/v1/models/tts/activate"),
        ("POST", "/api/v1/models/asr/activate"),
        ("GET", "/api/v1/models/asr/local"),
        ("POST", "/api/v1/models/servers/asr/restart"),
    ] {
        assert_eq!(call(&app, method, uri, LOCAL).await.0, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
    let (status, body) = call(&app, "POST", "/api/v1/models/tts/silero/file", [8, 8, 8, 8]).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::FORBIDDEN, Some("Только из домашней сети.")));
    let (status, body) = call(&app, "GET", "/api/v1/models/nowhere", LOCAL).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::NOT_FOUND, Some("Cannot GET /api/v1/models/nowhere")));
}

#[tokio::test]
async fn shows_the_shop_to_who_sees_the_station_and_changes_it_for_who_manages_models() {
    let models = tempfile::tempdir().unwrap();
    let app = app(models.path()).await;
    // A gateway whose recognition is a server: nothing local to show.
    let (status, body) = answer(local_asr(Uses::of(&app), member(&app)).await).await;
    assert_eq!((status, &body["message"]["local"]), (StatusCode::OK, &Value::Null));
    let (status, body) = answer(view(Uses::of(&app), member(&app)).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 200);
    let shop = &body["message"];
    assert_eq!(shop["tts"][0]["id"], "yandex");
    assert_eq!(shop["tts"][1]["id"], "silero");
    assert_eq!(shop["tts"][1]["downloadable"], true, "models.silero.ai over https");
    assert_eq!(shop["asrLoaded"], Value::Null);
    assert_eq!(shop["canRestart"], false);
    assert_eq!(shop["restarting"], Value::Null);
    assert_eq!(shop["downloads"], json!([]));

    let forbidden = refused(install(Uses::of(&app), member(&app), id("silero")).await);
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    assert!(forbidden.message.starts_with("Нет права: "), "{}", forbidden.message);

    let empty = refused(activate_tts(Uses::of(&app), admin(&app), Body(json!({}))).await);
    assert_eq!(empty.message, r#"body: {"modelId": "piper-ru_RU-denis-medium"} или {"modelId": "yandex"}"#);
    // The voice is linked (src/links), but no voice server of ours is configured here.
    let no_server = refused(activate_tts(Uses::of(&app), admin(&app), Body(json!({"modelId": "piper-denis"}))).await);
    assert_eq!(
        (no_server.status, no_server.message.as_str()),
        (StatusCode::BAD_REQUEST, "Свой голосовой сервер не настроен (TTS_OWN_URL).")
    );

    let empty = refused(activate_asr(Uses::of(&app), admin(&app), Body(json!({"id": 5}))).await);
    assert_eq!(empty.message, r#"body: {"id": "gigaam-v3-e2e"}"#);
    let (status, body) =
        answer(activate_asr(Uses::of(&app), admin(&app), Body(json!({"model": "v3_ctc"}))).await).await;
    assert_eq!((status, body["message"].clone()), (StatusCode::CREATED, json!({"needsRestart": true})));

    let unknown = refused(restart(Uses::of(&app), admin(&app), id("fly")).await);
    assert_eq!((unknown.status, unknown.message.as_str()), (StatusCode::NOT_FOUND, "asr or tts"));
    let by_hand = refused(restart(Uses::of(&app), admin(&app), id("tts")).await);
    assert_eq!(by_hand.status, StatusCode::BAD_REQUEST);
    assert!(
        by_hand.message.ends_with(" Работающий сервер не тронут — выбор остаётся ждать следующей попытки."),
        "{}",
        by_hand.message
    );

    let (status, body) = answer(cancel(Uses::of(&app), admin(&app), id("silero")).await).await;
    assert_eq!((status, body["message"].clone()), (StatusCode::CREATED, json!({"canceled": true})));
    let missing = refused(remove(Uses::of(&app), admin(&app), id("piper-denis")).await);
    assert_eq!((missing.status, missing.message.as_str()), (StatusCode::NOT_FOUND, "Этот голос не скачан."));
}

#[tokio::test]
async fn takes_a_silero_file_from_the_body_by_its_name() {
    let models = tempfile::tempdir().unwrap();
    let app = app(models.path()).await;
    // Larger than any JSON body may be: the file streams to disk.
    let file = AxumBody::from(vec![1u8; 3 * 1024 * 1024]);
    let placed = place_file(Uses::of(&app), admin(&app), RawQuery(Some("name=v4_ru.pt".into())), file).await;
    let (status, body) = answer(placed).await;
    assert_eq!((status, body["message"].clone()), (StatusCode::CREATED, json!({"placed": true})));
    assert_eq!(std::fs::metadata(models.path().join("silero/v4_ru.pt")).unwrap().len(), 3 * 1024 * 1024);
    let other = place_file(Uses::of(&app), admin(&app), RawQuery(Some("name=evil.sh".into())), AxumBody::empty());
    assert_eq!(refused(other.await).message, "Ожидается файл Silero из каталога: v4_ru.pt, v5_ru.pt.");
    let nameless = place_file(Uses::of(&app), admin(&app), RawQuery(None), AxumBody::empty()).await;
    assert_eq!(refused(nameless).status, StatusCode::BAD_REQUEST);
}
