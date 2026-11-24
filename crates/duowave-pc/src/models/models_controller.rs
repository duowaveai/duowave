//! The model shop: the catalog, downloads, activation (`/api/v1/models`). Anyone who sees the station sees what
//! speaks; changing it is `models.manage` — this is how the household speaks and listens.

use std::sync::Arc;

use axum::extract::rejection::PathRejection;
use axum::extract::{Path, RawQuery};
use axum::routing::{delete, get, post};
use axum::Router;
use serde_json::{json, Value};

use super::model_servers::ServerName;
use super::models_service::{Activated, Chosen, DownloadState, ModelsView, VoiceRequest};
use crate::core::identity::permissions::{MODELS_MANAGE, STATION_VIEW};
use crate::models::ModelsService;
use crate::platform::http::{created, ok, Api, ApiError, ApiResult, Body, Caller, Uses, Zone};
use crate::speech::asr::local::supervisor::LocalAsr;
use axum::http::StatusCode;

pub fn router<S: crate::models::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/models", get(view))
        .route("/api/v1/models/tts/{id}/install", post(install))
        .route("/api/v1/models/tts/{id}/cancel", post(cancel))
        .route("/api/v1/models/tts/{id}", delete(remove))
        .route("/api/v1/models/tts/silero/file", post(place_file))
        .route("/api/v1/models/tts/activate", post(activate_tts))
        .route("/api/v1/models/tts/pending", delete(forget_pending))
        .route("/api/v1/models/asr/activate", post(activate_asr))
        .route("/api/v1/models/asr/local", get(local_asr))
        .route("/api/v1/models/servers/{which}/restart", post(restart))
}

/// A path's segment; one that does not decode answers in the envelope like every other error.
fn segment(path: Result<Path<String>, PathRejection>) -> Result<String, ApiError> {
    path.map(|Path(value)| value).map_err(|rejection| ApiError::bad_request(rejection.body_text()))
}

pub async fn view(Uses(models): Uses<Arc<ModelsService>>, caller: Caller) -> ApiResult<ModelsView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(models.view().await?))
}

/// Recognition on this computer, the light way (the desk asks it every few seconds while the model is coming):
/// `{"local": {phase, bytes, total, serving, backend, error…}}`, `{"local": null}` when a server recognises.
pub async fn local_asr(Uses(local_asr): Uses<Option<Arc<LocalAsr>>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(STATION_VIEW)?;
    let local = local_asr.as_ref().map(|local| local.status());
    Ok(ok(json!({ "local": local })))
}

/// Downloads a voice — pinned or from the open registry; the progress rides in GET /models.
pub async fn install(
    Uses(models): Uses<Arc<ModelsService>>,
    caller: Caller,
    id: Result<Path<String>, PathRejection>,
) -> ApiResult<DownloadState> {
    caller.permitted(MODELS_MANAGE)?;
    let id = segment(id)?;
    Ok(created(models.install_tts(&id, caller.actor()).await?))
}

/// Stops a running download; the half file leaves with it.
pub async fn cancel(
    Uses(models): Uses<Arc<ModelsService>>,
    caller: Caller,
    id: Result<Path<String>, PathRejection>,
) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    models.cancel_tts(&segment(id)?);
    Ok(created(json!({"canceled": true})))
}

/// Deletes a downloaded voice — not the one speaking.
pub async fn remove(
    Uses(models): Uses<Arc<ModelsService>>,
    caller: Caller,
    id: Result<Path<String>, PathRejection>,
) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    let id = segment(id)?;
    models.uninstall_tts(&id, caller.actor()).await?;
    Ok(ok(json!({"deleted": true})))
}

/// A Silero model file by hand: the body is the file, ?name=v4_ru.pt. The models are given out on silero.ai, not in
/// catalogs. A .pt is code the voice server runs (a pickle): from this computer or the home network only.
pub async fn place_file(
    Uses(models): Uses<Arc<ModelsService>>,
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
    models.place_silero(name.as_deref(), body.into_data_stream(), caller.actor()).await?;
    Ok(created(json!({"placed": true})))
}

/// The voice for the next synthesis request, by catalog ids; `needsRestart` when the server has not loaded it yet.
/// An own voice whose server would start tight on memory: 428 with the warning, `"anyway": true` starts it. A voice
/// whose model or file is not here: 409 with `errors.needs`; `"install": true` installs or downloads it and the choice
/// waits — 202 `{pending}`, switched on by itself when it is there.
pub async fn activate_tts(
    Uses(models): Uses<Arc<ModelsService>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    if !body.get("modelId").is_some_and(Value::is_string) && !body.get("model").is_some_and(Value::is_string) {
        return Err(ApiError::bad_request(crate::tr!(
            "body: {\"modelId\": \"piper-ru_RU-denis-medium\"} или {\"modelId\": \"yandex\"}"
        )));
    }
    match models.choose(VoiceRequest::of(&body), caller.actor()).await? {
        Chosen::Active(activated) => Ok(created(serde_json::to_value(activated)?)),
        Chosen::Pending(pending) => Ok(Api(StatusCode::ACCEPTED, json!({ "pending": pending }))),
    }
}

/// Forgets the voice waiting for its model or file.
async fn forget_pending(Uses(models): Uses<Arc<ModelsService>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    let stopped = models.cancel_pending(caller.actor()).await;
    Ok(ok(json!({ "forgotten": true, "stopped": stopped })))
}

/// Notes the recognition model by its card id; serves after the recognition server's restart.
pub async fn activate_asr(
    Uses(models): Uses<Arc<ModelsService>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<Activated> {
    caller.permitted(MODELS_MANAGE)?;
    let id = body.get("id").and_then(Value::as_str).or_else(|| body.get("model").and_then(Value::as_str)).unwrap_or("");
    if id.is_empty() {
        return Err(ApiError::bad_request(r#"body: {"id": "gigaam-v3-e2e"}"#));
    }
    Ok(created(models.activate_asr(id, caller.actor()).await?))
}

/// Applies a new voice file or recognition model: the server rescans or loads over HTTP; an older one is restarted
/// the macOS deploy way (a Mac only — elsewhere 400 says to restart it by hand). A model that will not load says so:
/// the running server stays.
pub async fn restart(
    Uses(models): Uses<Arc<ModelsService>>,
    caller: Caller,
    which: Result<Path<String>, PathRejection>,
) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    let Some(which) = ServerName::parse(&segment(which)?) else {
        return Err(ApiError::not_found("asr or tts"));
    };
    models.restart_server(which, caller.actor()).await.map_err(|error| {
        ApiError::bad_request(crate::tr!(
            "{message} Работающий сервер не тронут — выбор остаётся ждать следующей попытки.",
            message = error.message
        ))
    })?;
    Ok(created(json!({"restarted": true})))
}
