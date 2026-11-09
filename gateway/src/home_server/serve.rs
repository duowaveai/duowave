//! `/home/v1` on the computer: its own servers for the gateway on the station, OpenAI-compatible, only from the home
//! network and only with a key that has the right `home.models`. Its recognition and voice servers listen on
//! 127.0.0.1 and stay closed to the network: this gateway is the one door to them.
//!
//! - `/home/v1/status` — what is ready: recognition, our voice, a model of its own;
//! - `/home/v1/asr/audio/transcriptions`, `/home/v1/asr/models` — its recognition server, as it is;
//! - `/home/v1/tts/audio/speech` — the voice it speaks with now, the voice lab's too: Ogg/Opus, ready for a station;
//! - `/home/v1/llm/chat/completions`, `/home/v1/llm/models` — its own model (the first of «Мозг»'s providers at
//!   home: Ollama, LM Studio, llama.cpp).
//!
//! The paths of the plan (`/home/v1/audio/transcriptions`, `/home/v1/audio/speech`, `/home/v1/chat/completions`,
//! `/home/v1/models`) answer the same.
//!
//! - `/home/v1/plugins`, `/home/v1/plugins/<id>/call` — the plugins that are on here, and a call of one for the station
//!   (docs/plans/plugins.md, stage 7: a plugin with `prefer: home` runs on the computer while it is up).

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::core::identity::permissions::HOME_MODELS;
use crate::operations::networks::is_local_url;
use crate::platform::http::{ApiError, Caller, Zone};

/// A minute of speech as WAV is ~2 MB; a request with history for the model, less.
const BODY_LIMIT: usize = 16 * 1024 * 1024;

pub fn router() -> Router<Arc<App>> {
    Router::new()
        .route("/home/v1/status", get(status))
        .route("/home/v1/asr/audio/transcriptions", post(transcribe))
        .route("/home/v1/audio/transcriptions", post(transcribe))
        .route("/home/v1/asr/models", get(asr_models))
        .route("/home/v1/tts/audio/speech", post(speak))
        .route("/home/v1/audio/speech", post(speak))
        .route("/home/v1/llm/chat/completions", post(chat))
        .route("/home/v1/chat/completions", post(chat))
        .route("/home/v1/llm/models", get(llm_models))
        .route("/home/v1/models", get(llm_models))
        .route("/home/v1/plugins", get(plugins))
        .route("/home/v1/catalog", get(catalog))
        .route("/home/v1/catalog/file", get(catalog_file))
        .route("/home/v1/plugins/{id}/call", post(plugin_call))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

fn allowed(caller: &Caller) -> Result<(), ApiError> {
    caller.zone_at_least(Zone::Home)?;
    caller.permitted(HOME_MODELS)?;
    Ok(())
}

/// The packages' catalog for the station without internet of its own (docs/plans/plugins.md, stage 9, «через Mac»):
/// this computer's copy of the feed and its signature — the station checks them with its own key.
async fn catalog(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let copy = app.catalog.export_for_station().ok_or_else(|| ApiError::not_found("Копии каталога здесь нет."))?;
    Ok(axum::Json(copy).into_response())
}

#[derive(serde::Deserialize)]
struct FileQuery {
    url: String,
}

/// A file of the catalog for the station: an archive, only from the catalog's own origin; the station checks its hash.
async fn catalog_file(
    State(app): State<Arc<App>>,
    caller: Caller,
    axum::extract::Query(query): axum::extract::Query<FileQuery>,
) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let bytes = app.catalog.fetch_for_station(&query.url).await?;
    Ok(bytes.into_response())
}

/// The plugins on here and their versions: what the station may hand over.
async fn plugins(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let view = app.plugins.view().await;
    let on: Vec<Value> = view
        .packages
        .iter()
        .filter(|package| package.enabled && package.state != "error")
        .map(|package| json!({ "id": package.id, "version": package.version }))
        .collect();
    Ok(axum::Json(json!({ "plugins": on })).into_response())
}

/// A call of a plugin of this computer for the station: its function, its input, how long it may take.
async fn plugin_call(
    State(app): State<Arc<App>>,
    caller: Caller,
    axum::extract::Path(id): axum::extract::Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let request: Value =
        serde_json::from_slice(&body).map_err(|_| ApiError::bad_request("JSON: {function, input, kind}"))?;
    let function = request.get("function").and_then(Value::as_str).unwrap_or_default();
    let input = request.get("input").cloned().unwrap_or_else(|| json!({}));
    let kind = match request.get("kind").and_then(Value::as_str) {
        Some("voice") => crate::plugins::runtime::CallKind::Voice,
        Some("background") => crate::plugins::runtime::CallKind::Background,
        _ => crate::plugins::runtime::CallKind::Tool,
    };
    if !app.plugins.is_active(&id) {
        return Err(ApiError::conflict(format!("Плагин {id} здесь не включён.")));
    }
    let answer = app
        .plugins
        .call(&id, function, input, kind)
        .await
        .map_err(|error| ApiError::bad_gateway(format!("Плагин не ответил: {error}.")))?;
    Ok(axum::Json(json!({ "result": answer })).into_response())
}

/// This computer's recognition server, when it is its own (at home): a cloud's is never handed on.
fn recognition(app: &App) -> Option<(String, Option<String>)> {
    let asr = &app.config.speech.asr;
    (asr.enabled && is_local_url(Some(&asr.base_url)))
        .then(|| (asr.base_url.trim_end_matches('/').to_string(), asr.api_key.clone()))
}

/// This computer's own model: the first OpenAI-compatible provider of «Мозг» at home, and its key.
fn own_model(app: &App) -> Option<(String, Option<String>)> {
    let rows: Vec<(String, String)> = app
        .db
        .with(|connection| {
            connection
                .prepare(
                    "SELECT id, base_url FROM llm_providers WHERE kind = 'openai' AND base_url IS NOT NULL
                     ORDER BY created_at, rowid",
                )?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .ok()?;
    rows.into_iter()
        .find(|(_, url)| is_local_url(Some(url)))
        .map(|(id, url)| (url.trim_end_matches('/').to_string(), app.secrets.get(&format!("llm:{id}:key"))))
}

async fn status(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let voice = app.voice.own_choice().map(|pair| format!("{}/{}", pair.model, pair.voice));
    Ok(axum::Json(json!({
        "version": crate::VERSION,
        "peerApi": crate::core::build::PEER_API,
        "asr": recognition(&app).is_some(),
        "tts": app.voice.own_speech().is_some(),
        "voice": voice,
        "llm": own_model(&app).is_some(),
    }))
    .into_response())
}

/// A request handed on as it came, its answer handed back as it comes (a model's stream too).
async fn forward(
    method: reqwest::Method,
    url: String,
    key: Option<String>,
    content_type: Option<&str>,
    body: Option<Bytes>,
    wait: Duration,
) -> Result<Response, ApiError> {
    let mut request = crate::utils::http::client().request(method, &url).timeout(wait);
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    if let Some(kind) = content_type {
        request = request.header(header::CONTENT_TYPE, kind);
    }
    if let Some(body) = body {
        request = request.body(body);
    }
    let response = request.send().await.map_err(|error| {
        ApiError::new(StatusCode::BAD_GATEWAY, format!("сервер этого компьютера не ответил: {error}"))
    })?;
    let status = StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let kind = response.headers().get(header::CONTENT_TYPE).cloned();
    let mut answer = Response::builder().status(status);
    if let Some(kind) = kind {
        answer = answer.header(header::CONTENT_TYPE, kind);
    }
    answer.body(Body::from_stream(response.bytes_stream())).map_err(ApiError::internal)
}

fn content_type(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok())
}

async fn transcribe(
    State(app): State<Arc<App>>,
    caller: Caller,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let (base, key) =
        recognition(&app).ok_or_else(|| ApiError::unavailable("На этом компьютере нет своего распознавания."))?;
    let wait = Duration::from_millis(app.config.speech.asr.timeout_ms.max(5000));
    forward(
        reqwest::Method::POST,
        format!("{base}/audio/transcriptions"),
        key,
        content_type(&headers),
        Some(body),
        wait,
    )
    .await
}

async fn asr_models(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let (base, key) =
        recognition(&app).ok_or_else(|| ApiError::unavailable("На этом компьютере нет своего распознавания."))?;
    forward(reqwest::Method::GET, format!("{base}/models"), key, None, None, Duration::from_secs(10)).await
}

/// The voice this computer speaks with now, whatever the station asked: Ogg/Opus.
async fn speak(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let request: Value = serde_json::from_slice(&body).map_err(|_| ApiError::bad_request("input: текст"))?;
    let text = request.get("input").and_then(Value::as_str).unwrap_or("").to_string();
    if text.trim().is_empty() {
        return Err(ApiError::bad_request("input: текст"));
    }
    let own = app
        .voice
        .own_speech()
        .cloned()
        .ok_or_else(|| ApiError::unavailable("На этом компьютере нет своего голоса."))?;
    let spoken = own
        .synthesize(&text, &CancellationToken::new())
        .await
        .map_err(|error| ApiError::new(StatusCode::BAD_GATEWAY, format!("голос этого компьютера: {error:#}")))?;
    Ok(([(header::CONTENT_TYPE, "audio/ogg")], spoken.audio).into_response())
}

async fn chat(
    State(app): State<Arc<App>>,
    caller: Caller,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let (base, key) = own_model(&app).ok_or_else(|| ApiError::unavailable("На этом компьютере нет своей модели."))?;
    forward(
        reqwest::Method::POST,
        format!("{base}/chat/completions"),
        key,
        content_type(&headers).or(Some("application/json")),
        Some(body),
        Duration::from_secs(300),
    )
    .await
}

async fn llm_models(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let (base, key) = own_model(&app).ok_or_else(|| ApiError::unavailable("На этом компьютере нет своей модели."))?;
    forward(reqwest::Method::GET, format!("{base}/models"), key, None, None, Duration::from_secs(10)).await
}
