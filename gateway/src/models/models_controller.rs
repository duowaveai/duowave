//! The model shop: the catalog, downloads, activation (`/api/v1/models`). Anyone who sees the station sees what
//! speaks; changing it is `models.manage` — this is how the household speaks and listens.

use std::sync::Arc;

use axum::extract::rejection::PathRejection;
use axum::extract::{Path, RawQuery, State};
use axum::routing::{delete, get, post};
use axum::Router;
use serde_json::{json, Value};

use super::model_servers::ServerName;
use super::models_service::{Activated, Chosen, DownloadState, ModelsView, VoiceRequest};
use crate::app::App;
use crate::core::identity::permissions::{MODELS_MANAGE, STATION_VIEW};
use crate::platform::http::{created, ok, Api, ApiError, ApiResult, Body, Caller, Zone};
use axum::http::StatusCode;

pub fn router() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/models", get(view))
        .route("/api/v1/models/tts/{id}/install", post(install))
        .route("/api/v1/models/tts/{id}/cancel", post(cancel))
        .route("/api/v1/models/tts/{id}", delete(remove))
        .route("/api/v1/models/tts/silero/file", post(place_file))
        .route("/api/v1/models/tts/activate", post(activate_tts))
        .route("/api/v1/models/tts/pending", delete(forget_pending))
        .route("/api/v1/models/asr/activate", post(activate_asr))
        .route("/api/v1/models/servers/{which}/restart", post(restart))
}

/// A path's segment; one that does not decode answers in the envelope like every other error.
fn segment(path: Result<Path<String>, PathRejection>) -> Result<String, ApiError> {
    path.map(|Path(value)| value).map_err(|rejection| ApiError::bad_request(rejection.body_text()))
}

async fn view(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<ModelsView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(app.models.view().await?))
}

/// Downloads a voice — pinned or from the open registry; the progress rides in GET /models.
async fn install(
    State(app): State<Arc<App>>,
    caller: Caller,
    id: Result<Path<String>, PathRejection>,
) -> ApiResult<DownloadState> {
    caller.permitted(MODELS_MANAGE)?;
    let id = segment(id)?;
    Ok(created(app.models.install_tts(&id, caller.actor()).await?))
}

/// Stops a running download; the half file leaves with it.
async fn cancel(
    State(app): State<Arc<App>>,
    caller: Caller,
    id: Result<Path<String>, PathRejection>,
) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    app.models.cancel_tts(&segment(id)?);
    Ok(created(json!({"canceled": true})))
}

/// Deletes a downloaded voice — not the one speaking.
async fn remove(
    State(app): State<Arc<App>>,
    caller: Caller,
    id: Result<Path<String>, PathRejection>,
) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    let id = segment(id)?;
    app.models.uninstall_tts(&id, caller.actor()).await?;
    Ok(ok(json!({"deleted": true})))
}

/// A Silero model file by hand: the body is the file, ?name=v4_ru.pt. The models are given out on silero.ai, not in
/// catalogs. A .pt is code the voice server runs (a pickle): from this computer or the home network only.
async fn place_file(
    State(app): State<Arc<App>>,
    caller: Caller,
    RawQuery(query): RawQuery,
    body: axum::body::Body,
) -> ApiResult<Value> {
    caller.zone_at_least(Zone::Home)?;
    caller.permitted(MODELS_MANAGE)?;
    let name = query.as_deref().and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == "name")
            .map(|(_, value)| value.into_owned())
    });
    app.models.place_silero(name.as_deref(), body.into_data_stream(), caller.actor()).await?;
    Ok(created(json!({"placed": true})))
}

/// The voice for the next synthesis request, by catalog ids; `needsRestart` when the server has not loaded it yet.
/// An own voice whose server would start tight on memory: 428 with the warning, `"anyway": true` starts it. A voice
/// whose model or file is not here: 409 with `errors.needs`; `"install": true` installs or downloads it and the choice
/// waits — 202 `{pending}`, switched on by itself when it is there.
async fn activate_tts(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    if !body.get("modelId").is_some_and(Value::is_string) && !body.get("model").is_some_and(Value::is_string) {
        return Err(ApiError::bad_request(
            r#"body: {"modelId": "piper-ru_RU-denis-medium"} или {"modelId": "yandex"}"#,
        ));
    }
    match app.models.choose(VoiceRequest::of(&body), caller.actor()).await? {
        Chosen::Active(activated) => Ok(created(serde_json::to_value(activated)?)),
        Chosen::Pending(pending) => Ok(Api(StatusCode::ACCEPTED, json!({ "pending": pending }))),
    }
}

/// Forgets the voice waiting for its model or file.
async fn forget_pending(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    app.models.clear_pending(caller.actor());
    Ok(ok(json!({ "forgotten": true })))
}

/// Notes the recognition model by its card id; serves after the recognition server's restart.
async fn activate_asr(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<Activated> {
    caller.permitted(MODELS_MANAGE)?;
    let id = body.get("id").and_then(Value::as_str).or_else(|| body.get("model").and_then(Value::as_str)).unwrap_or("");
    if id.is_empty() {
        return Err(ApiError::bad_request(r#"body: {"id": "gigaam-v3-e2e"}"#));
    }
    Ok(created(app.models.activate_asr(id, caller.actor()).await?))
}

/// Applies a new voice file or recognition model: the server rescans or loads over HTTP; an older one is restarted
/// the macOS deploy way (a Mac only — elsewhere 400 says to restart it by hand). A model that will not load says so:
/// the running server stays.
async fn restart(
    State(app): State<Arc<App>>,
    caller: Caller,
    which: Result<Path<String>, PathRejection>,
) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    let Some(which) = ServerName::parse(&segment(which)?) else {
        return Err(ApiError::not_found("asr or tts"));
    };
    app.models.restart_server(which, caller.actor()).await.map_err(|error| {
        ApiError::bad_request(format!(
            "{} Работающий сервер не тронут — выбор остаётся ждать следующей попытки.",
            error.message
        ))
    })?;
    Ok(created(json!({"restarted": true})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::path::Path as FsPath;

    use axum::body::Body as AxumBody;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use axum::response::IntoResponse;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::config::{Config, Env};
    use crate::core::identity::principal::user_principal;
    use crate::core::identity::Role;

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
        let env = Env::from_pairs([
            ("STATE_DIR", ""),
            ("ASR_PROVIDER", "none"),
            ("TTS_MODELS_DIR", models.to_str().unwrap()),
        ]);
        let mut config = Config::for_tests(&env);
        config.models.piper_registry = registry().await;
        let app = App::build(config).unwrap();
        app.models.start().await.unwrap();
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
            ("POST", "/api/v1/models/servers/asr/restart"),
        ] {
            assert_eq!(call(&app, method, uri, LOCAL).await.0, StatusCode::UNAUTHORIZED, "{method} {uri}");
        }
        let (status, body) = call(&app, "POST", "/api/v1/models/tts/silero/file", [8, 8, 8, 8]).await;
        assert_eq!((status, body["message"].as_str()), (StatusCode::FORBIDDEN, Some("Только из домашней сети.")));
        let (status, body) = call(&app, "GET", "/api/v1/models/nowhere", LOCAL).await;
        assert_eq!(
            (status, body["message"].as_str()),
            (StatusCode::NOT_FOUND, Some("Cannot GET /api/v1/models/nowhere"))
        );
    }

    #[tokio::test]
    async fn shows_the_shop_to_who_sees_the_station_and_changes_it_for_who_manages_models() {
        let models = tempfile::tempdir().unwrap();
        let app = app(models.path()).await;
        let (status, body) = answer(view(State(app.clone()), member(&app)).await).await;
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

        let forbidden = refused(install(State(app.clone()), member(&app), id("silero")).await);
        assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
        assert!(forbidden.message.starts_with("Нет права: "), "{}", forbidden.message);

        let empty = refused(activate_tts(State(app.clone()), admin(&app), Body(json!({}))).await);
        assert_eq!(empty.message, r#"body: {"modelId": "piper-ru_RU-denis-medium"} или {"modelId": "yandex"}"#);
        // The voice is linked (src/links), but no voice server of ours is configured here.
        let no_server =
            refused(activate_tts(State(app.clone()), admin(&app), Body(json!({"modelId": "piper-denis"}))).await);
        assert_eq!(
            (no_server.status, no_server.message.as_str()),
            (StatusCode::BAD_REQUEST, "Свой голосовой сервер не настроен (TTS_OWN_URL).")
        );

        let empty = refused(activate_asr(State(app.clone()), admin(&app), Body(json!({"id": 5}))).await);
        assert_eq!(empty.message, r#"body: {"id": "gigaam-v3-e2e"}"#);
        let (status, body) =
            answer(activate_asr(State(app.clone()), admin(&app), Body(json!({"model": "v3_ctc"}))).await).await;
        assert_eq!((status, body["message"].clone()), (StatusCode::CREATED, json!({"needsRestart": true})));

        let unknown = refused(restart(State(app.clone()), admin(&app), id("fly")).await);
        assert_eq!((unknown.status, unknown.message.as_str()), (StatusCode::NOT_FOUND, "asr or tts"));
        let by_hand = refused(restart(State(app.clone()), admin(&app), id("tts")).await);
        assert_eq!(by_hand.status, StatusCode::BAD_REQUEST);
        assert!(
            by_hand.message.ends_with(" Работающий сервер не тронут — выбор остаётся ждать следующей попытки."),
            "{}",
            by_hand.message
        );

        let (status, body) = answer(cancel(State(app.clone()), admin(&app), id("silero")).await).await;
        assert_eq!((status, body["message"].clone()), (StatusCode::CREATED, json!({"canceled": true})));
        let missing = refused(remove(State(app.clone()), admin(&app), id("piper-denis")).await);
        assert_eq!((missing.status, missing.message.as_str()), (StatusCode::NOT_FOUND, "Этот голос не скачан."));
    }

    #[tokio::test]
    async fn takes_a_silero_file_from_the_body_by_its_name() {
        let models = tempfile::tempdir().unwrap();
        let app = app(models.path()).await;
        // Larger than any JSON body may be: the file streams to disk.
        let file = AxumBody::from(vec![1u8; 3 * 1024 * 1024]);
        let placed = place_file(State(app.clone()), admin(&app), RawQuery(Some("name=v4_ru.pt".into())), file).await;
        let (status, body) = answer(placed).await;
        assert_eq!((status, body["message"].clone()), (StatusCode::CREATED, json!({"placed": true})));
        assert_eq!(std::fs::metadata(models.path().join("silero/v4_ru.pt")).unwrap().len(), 3 * 1024 * 1024);
        let other =
            place_file(State(app.clone()), admin(&app), RawQuery(Some("name=evil.sh".into())), AxumBody::empty());
        assert_eq!(refused(other.await).message, "Ожидается файл Silero из каталога: v4_ru.pt, v5_ru.pt.");
        let nameless = place_file(State(app.clone()), admin(&app), RawQuery(None), AxumBody::empty()).await;
        assert_eq!(refused(nameless).status, StatusCode::BAD_REQUEST);
    }
}
