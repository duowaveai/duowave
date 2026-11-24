//! `/api/v1/voice-lab`: the owner's own voices — the engines, the takes, the imprints. Anyone who sees the station
//! sees what is there; changing it is `models.manage`. Takes are people's voices: they arrive only from the home
//! network.
//!
//! The `.ts` files named here are the Node gateway's, removed from the tree: git tag `node-gateway-last`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body as RawBody;
use axum::extract::{Path, RawQuery};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::Router;
use futures::StreamExt;
use serde_json::{json, Value};

use super::jobs::JobState;
use super::js;
use super::service::VoiceLabView;
use super::voice_store::{OwnVoice, TakeInfo, MAX_TAKE_BYTES};
use crate::core::identity::permissions::{MODELS_MANAGE, STATION_VIEW};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Uses, Zone};
use crate::voice_lab::VoiceLabService;

pub fn router<S: crate::voice_lab::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/voice-lab", get(view))
        .route("/api/v1/voice-lab/platform/recheck", post(recheck))
        .route("/api/v1/voice-lab/engines/{id}/install", post(install))
        .route("/api/v1/voice-lab/draft/takes", post(add_take))
        .route("/api/v1/voice-lab/draft/takes/{n}", get(take).delete(remove_take))
        .route("/api/v1/voice-lab/draft", delete(clear_draft))
        .route("/api/v1/voice-lab/imprint", post(imprint))
        .route("/api/v1/voice-lab/job/stop", post(stop))
        .route("/api/v1/voice-lab/voices/{id}/samples/{n}", get(sample))
        .route("/api/v1/voice-lab/voices/{id}", patch(rename).delete(remove))
        .route("/api/v1/voice-lab/voices/{id}/reimprint", post(reimprint))
}

/// How long an upload may stand still: a phone that left the Wi-Fi mid-take without a word frees its request, its
/// buffered take and the gateway's shutdown (Node ended such a request after its 300 s requestTimeout).
const UPLOAD_IDLE: Duration = Duration::from_secs(60);

/// The raw body of an upload, refused past the limit while it still arrives.
pub async fn read_body(body: RawBody, limit: usize) -> Result<Vec<u8>, ApiError> {
    read_body_within(body, limit, UPLOAD_IDLE).await
}

pub async fn read_body_within(body: RawBody, limit: usize, idle: Duration) -> Result<Vec<u8>, ApiError> {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    loop {
        let Ok(next) = tokio::time::timeout(idle, stream.next()).await else {
            return Err(ApiError::new(StatusCode::REQUEST_TIMEOUT, crate::tr!("Запись не дошла: связь прервалась.")));
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|error| ApiError::bad_request(error.to_string()))?;
        if bytes.len() + chunk.len() > limit {
            return Err(ApiError::bad_request(crate::tr!(
                "Запись больше {size} МБ.",
                size = js::round_i64(limit as f64 / 1_048_576.0)
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// `?passage=N`: which passage was read, when it is one whole number.
pub fn passage(query: Option<&str>) -> Option<i64> {
    let values: Vec<String> = url::form_urlencoded::parse(query.unwrap_or("").as_bytes())
        .filter(|(name, _)| name == "passage")
        .map(|(_, value)| value.into_owned())
        .collect();
    match values.as_slice() {
        [value] => js::integer(value),
        _ => None,
    }
}

/// A WAV from the store, never cached: a take re-recorded under the same number is a new one.
async fn wav(path: PathBuf, missing: String) -> Result<Response, ApiError> {
    let bytes = tokio::fs::read(&path).await.map_err(|_| ApiError::not_found(missing))?;
    let mut response = RawBody::from(bytes).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub async fn view(Uses(voice_lab): Uses<Arc<VoiceLabService>>, caller: Caller) -> ApiResult<VoiceLabView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(voice_lab.view().await))
}

async fn recheck(Uses(voice_lab): Uses<Arc<VoiceLabService>>, caller: Caller) -> ApiResult<VoiceLabView> {
    caller.permitted(MODELS_MANAGE)?;
    Ok(created(voice_lab.recheck().await))
}

pub async fn install(
    Uses(voice_lab): Uses<Arc<VoiceLabService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<JobState> {
    caller.permitted(MODELS_MANAGE)?;
    Ok(created(voice_lab.install(&id, caller.actor()).await?))
}

/// A take: the body is a 16-bit PCM WAV (the panel records or converts it), ?passage=N — which passage was read.
pub async fn add_take(
    Uses(voice_lab): Uses<Arc<VoiceLabService>>,
    caller: Caller,
    RawQuery(query): RawQuery,
    body: RawBody,
) -> ApiResult<TakeInfo> {
    caller.zone_at_least(Zone::Home)?;
    caller.permitted(MODELS_MANAGE)?;
    let passage = passage(query.as_deref());
    let body = read_body(body, MAX_TAKE_BYTES).await?;
    Ok(created(voice_lab.add_take(body, passage).await?))
}

pub async fn take(
    Uses(voice_lab): Uses<Arc<VoiceLabService>>,
    caller: Caller,
    Path(n): Path<String>,
) -> Result<Response, ApiError> {
    caller.permitted(MODELS_MANAGE)?;
    wav(voice_lab.take_path(js::integer(&n))?, crate::tr!("нет такого дубля")).await
}

async fn remove_take(
    Uses(voice_lab): Uses<Arc<VoiceLabService>>,
    caller: Caller,
    Path(n): Path<String>,
) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    voice_lab.remove_take(js::integer(&n))?;
    Ok(ok(json!({ "deleted": true })))
}

pub async fn clear_draft(Uses(voice_lab): Uses<Arc<VoiceLabService>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    voice_lab.clear_draft()?;
    Ok(ok(json!({ "cleared": true })))
}

/// «Снять отпечаток»: {engine, name, anyway?}; the progress rides in GET /voice-lab. Tight on memory: 428 with the
/// warning, and the same request with `"anyway": true` starts it.
pub async fn imprint(
    Uses(voice_lab): Uses<Arc<VoiceLabService>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<JobState> {
    caller.permitted(MODELS_MANAGE)?;
    let Some(engine) = body.get("engine").and_then(Value::as_str) else {
        return Err(ApiError::bad_request(crate::tr!("body: {\"engine\": \"omnivoice\", \"name\": \"Мой голос\"}")));
    };
    let anyway = body.get("anyway") == Some(&Value::Bool(true));
    Ok(created(voice_lab.imprint(engine, body.get("name"), caller.actor(), anyway).await?))
}

/// «Переснять для другой модели»: {engine, anyway?} — the voice imprinted anew under that engine from its reference
/// and words (or its takes), a new voice beside it; the progress rides in GET /voice-lab, 428 as for an imprint.
pub async fn reimprint(
    Uses(voice_lab): Uses<Arc<VoiceLabService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<JobState> {
    caller.permitted(MODELS_MANAGE)?;
    let Some(engine) = body.get("engine").and_then(Value::as_str) else {
        return Err(ApiError::bad_request(r#"body: {"engine": "cosyvoice3"}"#));
    };
    let anyway = body.get("anyway") == Some(&Value::Bool(true));
    Ok(created(voice_lab.reimprint(&id, engine, anyway, caller.actor()).await?))
}

pub async fn stop(Uses(voice_lab): Uses<Arc<VoiceLabService>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    Ok(created(json!({ "stopped": voice_lab.stop_job() })))
}

pub async fn sample(
    Uses(voice_lab): Uses<Arc<VoiceLabService>>,
    caller: Caller,
    Path((id, n)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    caller.permitted(MODELS_MANAGE)?;
    wav(voice_lab.sample_path(&id, js::integer(&n))?, crate::tr!("нет такой пробы")).await
}

pub async fn rename(
    Uses(voice_lab): Uses<Arc<VoiceLabService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<OwnVoice> {
    caller.permitted(MODELS_MANAGE)?;
    Ok(ok(voice_lab.rename(&id, body.get("name"), caller.actor())?))
}

pub async fn remove(
    Uses(voice_lab): Uses<Arc<VoiceLabService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    voice_lab.remove(&id, caller.actor())?;
    Ok(ok(json!({ "deleted": true })))
}
