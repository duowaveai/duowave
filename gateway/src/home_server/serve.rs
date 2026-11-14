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
//! - `/home/v1/messenger/contacts`, `/home/v1/messenger/send` — this computer's Telegram bot for the station's
//!   «передай …» (messenger.rs): its contacts by name only, their chats stay here; a text to one of them.
//! - `/home/v1/history/have`, `…/objects`, `…/lines`, `…/fetch` — the station's apps' history kept here too
//!   (plugins/history/sync.rs): which objects are missing, the objects, its lines and marks, objects back.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
use crate::speech::asr::local::supervisor::LocalAsr;

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
        .route("/home/v1/messenger/contacts", get(messenger_contacts))
        .route("/home/v1/messenger/send", post(messenger_send))
        .route("/home/v1/history/have", post(history_have))
        .route("/home/v1/history/objects", post(history_objects))
        .route("/home/v1/history/lines", post(history_lines))
        .route("/home/v1/history/fetch", post(history_fetch))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

/// What the computer gave the station today: how many times and how fast — the helper's «Что даю колонке»
/// (docs/plans/desk.md, stage 4). Counted since the gateway started, by the computer's own day.
#[derive(Default)]
struct Served {
    day: String,
    counts: [(u64, u64); 4],
}

static SERVED: Mutex<Option<Served>> = Mutex::new(None);

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// One answered request: 0 — hearing, 1 — voice, 2 — the model, 3 — a message passed on.
fn count(which: usize, started: Instant) {
    let mut served = SERVED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let day = today();
    let served = served.get_or_insert_with(Served::default);
    if served.day != day {
        *served = Served { day, ..Served::default() };
    }
    served.counts[which].0 += 1;
    served.counts[which].1 += started.elapsed().as_millis() as u64;
}

/// A contact's message handed to a station (src/placement/messages.rs): «Сообщения» count both ways.
pub(crate) fn count_message(started: Instant) {
    count(3, started);
}

/// Today's counts: `{day, asr: {count, avgMs}, tts: …, llm: …, messages: …}`.
pub fn served_today() -> Value {
    let served = SERVED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let day = today();
    let counts = served.as_ref().filter(|served| served.day == day).map(|served| served.counts).unwrap_or_default();
    let one = |(count, ms): (u64, u64)| json!({ "count": count, "avgMs": ms.checked_div(count).unwrap_or(0) });
    json!({
        "day": day,
        "asr": one(counts[0]),
        "tts": one(counts[1]),
        "llm": one(counts[2]),
        "messages": one(counts[3]),
    })
}

fn allowed(caller: &Caller) -> Result<(), ApiError> {
    caller.zone_at_least(Zone::Home)?;
    caller.permitted(HOME_MODELS)?;
    Ok(())
}

/// The packages' catalog for the station without internet of its own (docs/plans/plugins.md, stage 9, «через Mac»):
/// this computer's copy of the feed and its signature — the station checks them with its own key — and the copies of
/// the extra feeds it names (`feeds: [{url, feed}]`, docs/plans/catalog-feeds.md).
async fn catalog(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let copy =
        app.catalog.export_for_station().ok_or_else(|| ApiError::not_found(crate::tr!("Копии каталога здесь нет.")))?;
    Ok(axum::Json(copy).into_response())
}

#[derive(serde::Deserialize)]
struct FileQuery {
    url: String,
}

/// A file of the catalog for the station: an archive, only from the origin of one of the catalog's feeds — the main
/// one or an extra one it names; the station checks its hash.
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
        return Err(ApiError::conflict(crate::tr!("Приложение {id} здесь не включено.", id = id)));
    }
    let answer = app
        .plugins
        .call(&id, function, input, kind)
        .await
        .map_err(|error| ApiError::bad_gateway(crate::tr!("Приложение не ответило: {error}.", error = error)))?;
    Ok(axum::Json(json!({ "result": answer })).into_response())
}

fn history_of(app: &App) -> Result<Arc<crate::plugins::history::History>, ApiError> {
    app.history.clone().ok_or_else(|| ApiError::conflict(crate::tr!("Истории приложений на этом компьютере нет.")))
}

fn ids_of(body: &Bytes) -> Result<Vec<String>, ApiError> {
    let request: Value = serde_json::from_slice(body).map_err(|_| ApiError::bad_request("JSON: {ids}"))?;
    Ok(request
        .get("ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect())
}

/// The station's peer name for its marks here: its key's principal, as plain letters.
fn peer_of(caller: &Caller) -> String {
    let raw = caller.principal().map(|principal| principal.id.clone()).unwrap_or_else(|| "station".into());
    let plain: String =
        raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).take(48).collect();
    let plain = plain.trim_matches('-').to_string();
    if plain.is_empty() {
        "station".into()
    } else {
        plain
    }
}

/// Of the station's history objects, the ones this computer lacks.
async fn history_have(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let history = history_of(&app)?;
    Ok(axum::Json(json!({ "missing": history.missing(&ids_of(&body)?) })).into_response())
}

/// The station's history objects: each kept only when its name is its hash.
async fn history_objects(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let history = history_of(&app)?;
    let request: Value = serde_json::from_slice(&body).map_err(|_| ApiError::bad_request("JSON: {objects}"))?;
    let objects = request.get("objects").and_then(Value::as_array).cloned().unwrap_or_default();
    let stored = tokio::task::spawn_blocking(move || history.receive(&objects)).await.map_err(ApiError::internal)?;
    Ok(axum::Json(json!({ "stored": stored })).into_response())
}

/// The station's lines and marks: its marks apart, its records into the apps' lines here.
async fn history_lines(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let history = history_of(&app)?;
    let request: Value = serde_json::from_slice(&body).map_err(|_| ApiError::bad_request("JSON: {apps}"))?;
    let apps = request.get("apps").and_then(Value::as_object).cloned().unwrap_or_default();
    let peer = peer_of(&caller);
    let joined = tokio::task::spawn_blocking(move || history.receive_lines(&peer, &apps))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::bad_request)?;
    Ok(axum::Json(json!({ "joined": joined })).into_response())
}

/// Objects the station cleaned and needs back (a step back, its feed).
async fn history_fetch(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let history = history_of(&app)?;
    Ok(axum::Json(json!({ "objects": history.give(&ids_of(&body)?) })).into_response())
}

pub fn no_bot() -> String {
    crate::tr!("На этом компьютере не настроен бот Telegram: «Приложения» → «Подключения» → Telegram.")
}
fn send_shape() -> String {
    crate::tr!("JSON: {contact, text} — кому и что передать")
}

/// The names of this computer's Telegram contacts, for the station's «передай …»: no chats.
async fn messenger_contacts(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let bot = &app.calls.telegram;
    if !bot.enabled() {
        return Err(ApiError::unavailable(no_bot()));
    }
    Ok(axum::Json(json!({ "contacts": bot.contact_names() })).into_response())
}

/// The station's «передай маме, что …»: a text to a contact of this computer's bot.
async fn messenger_send(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let request: Value = serde_json::from_slice(&body).map_err(|_| ApiError::bad_request(send_shape()))?;
    let field = |name: &str| request.get(name).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
    let (contact, text) = (field("contact"), field("text"));
    if contact.is_empty() || text.is_empty() {
        return Err(ApiError::bad_request(send_shape()));
    }
    let started = Instant::now();
    let name = pass_on(&app.calls.telegram, &contact, &text).await?;
    count(3, started);
    tracing::info!("home server: a message from the station passed on to «{name}»");
    Ok(axum::Json(json!({ "sent": true, "contact": name })).into_response())
}

/// The bot's part of a send: the contact by the name the station gives, then Telegram. The contact's name.
async fn pass_on(bot: &crate::calls::TelegramMessenger, contact: &str, text: &str) -> Result<String, ApiError> {
    if !bot.enabled() {
        return Err(ApiError::unavailable(no_bot()));
    }
    let found = bot.contact(contact).ok_or_else(|| {
        ApiError::not_found(crate::tr!(
            "У бота Telegram на этом компьютере нет контакта «{contact}».",
            contact = contact
        ))
    })?;
    bot.send_text(&found.chat_id, text, None).await.map_err(|error| {
        ApiError::bad_gateway(crate::tr!("Telegram не принял сообщение: {error}.", error = format!("{error:#}")))
    })?;
    Ok(found.name)
}

/// This computer's recognition server, when it is its own (at home): a cloud's is never handed on.
fn recognition(app: &App) -> Option<(String, Option<String>)> {
    let asr = &app.config.speech.asr;
    (asr.enabled && asr.local.is_none() && is_local_url(Some(&asr.base_url)))
        .then(|| (asr.base_url.trim_end_matches('/').to_string(), asr.api_key.clone()))
}

/// What hears for the station: this computer's own engine (src/speech/asr/local), or its recognition server.
enum Hearing {
    Local(Arc<LocalAsr>),
    Server(String, Option<String>),
}

fn hearing(app: &App) -> Option<Hearing> {
    if let Some(local) = &app.local_asr {
        return Some(Hearing::Local(local.clone()));
    }
    recognition(app).map(|(base, key)| Hearing::Server(base, key))
}

/// Whether the station may lean on this computer's hearing now: the own engine once its model is in.
fn hears(app: &App) -> bool {
    match hearing(app) {
        Some(Hearing::Local(local)) => local.ready(),
        Some(Hearing::Server(..)) => true,
        None => false,
    }
}

/// The parts of a multipart/form-data body by name (what an OpenAI-compatible client sends: `file`, `model`,
/// `language`…). Only as much of the format as such a client uses.
fn form_parts(headers: &HeaderMap, body: &[u8]) -> Vec<(String, Vec<u8>)> {
    let Some(boundary) = content_type(headers).and_then(|kind| {
        kind.split(';').map(str::trim).find_map(|part| part.strip_prefix("boundary=")).map(|b| b.trim_matches('"'))
    }) else {
        return Vec::new();
    };
    let delimiter = format!("--{boundary}").into_bytes();
    let find = |haystack: &[u8], needle: &[u8], from: usize| {
        haystack
            .get(from..)
            .and_then(|rest| rest.windows(needle.len()).position(|window| window == needle))
            .map(|at| at + from)
    };
    let mut parts = Vec::new();
    let mut at = match find(body, &delimiter, 0) {
        Some(start) => start + delimiter.len(),
        None => return parts,
    };
    while let Some(next) = find(body, &delimiter, at) {
        let part = &body[at..next];
        // Each part: CRLF, its headers, an empty line, its bytes, CRLF before the next delimiter.
        let part = part.strip_prefix(b"\r\n").unwrap_or(part);
        let part = part.strip_suffix(b"\r\n").unwrap_or(part);
        if let Some(split) = find(part, b"\r\n\r\n", 0) {
            let head = String::from_utf8_lossy(&part[..split]).to_string();
            let name = head
                .split(';')
                .map(str::trim)
                .find_map(|item| item.strip_prefix("name="))
                .map(|name| name.split("\r\n").next().unwrap_or("").trim_matches('"').to_string());
            if let Some(name) = name {
                parts.push((name, part[split + 4..].to_vec()));
            }
        }
        at = next + delimiter.len();
        if body.get(at..at + 2) == Some(b"--") {
            break;
        }
    }
    parts
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
        "asr": hears(&app),
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
        ApiError::new(StatusCode::BAD_GATEWAY, crate::tr!("Сервер этого компьютера не ответил: {error}", error = error))
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
    let hearing = hearing(&app)
        .ok_or_else(|| ApiError::unavailable(crate::tr!("На этом компьютере нет своего распознавания.")))?;
    let started = Instant::now();
    let (base, key) = match hearing {
        Hearing::Server(base, key) => (base, key),
        Hearing::Local(local) => {
            // The engine hears here: the WAV out of the form, the text back as an OpenAI-compatible server says it.
            let parts = form_parts(&headers, &body);
            let file = parts
                .into_iter()
                .find(|(name, _)| name == "file")
                .ok_or_else(|| ApiError::bad_request(crate::tr!("file: звук WAV")))?
                .1;
            let wav = crate::speech::audio::pcm::decode_wav(&file)
                .map_err(|error| ApiError::bad_request(format!("file: {error}")))?;
            let text =
                local.recognize(&wav.pcm, wav.sample_rate, &CancellationToken::new()).await.map_err(|error| {
                    ApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        crate::tr!("Распознавание этого компьютера: {error}", error = format!("{error:#}")),
                    )
                })?;
            count(0, started);
            return Ok(axum::Json(json!({ "text": text })).into_response());
        }
    };
    let wait = Duration::from_millis(app.config.speech.asr.timeout_ms.max(5000));
    let answer = forward(
        reqwest::Method::POST,
        format!("{base}/audio/transcriptions"),
        key,
        content_type(&headers),
        Some(body),
        wait,
    )
    .await?;
    if answer.status().is_success() {
        count(0, started);
    }
    Ok(answer)
}

async fn asr_models(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let (base, key) = match hearing(&app) {
        None => return Err(ApiError::unavailable(crate::tr!("На этом компьютере нет своего распознавания."))),
        // The one model the engine serves, as an OpenAI-compatible server lists it.
        Some(Hearing::Local(local)) => {
            let listed = json!({ "object": "list", "data": [{ "id": local.model(), "object": "model" }] });
            return Ok(axum::Json(listed).into_response());
        }
        Some(Hearing::Server(base, key)) => (base, key),
    };
    forward(reqwest::Method::GET, format!("{base}/models"), key, None, None, Duration::from_secs(10)).await
}

/// The voice this computer speaks with now, whatever the station asked: Ogg/Opus.
async fn speak(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let request: Value =
        serde_json::from_slice(&body).map_err(|_| ApiError::bad_request(crate::tr!("input: текст")))?;
    let text = request.get("input").and_then(Value::as_str).unwrap_or("").to_string();
    if text.trim().is_empty() {
        return Err(ApiError::bad_request(crate::tr!("input: текст")));
    }
    let own = app
        .voice
        .own_speech()
        .cloned()
        .ok_or_else(|| ApiError::unavailable(crate::tr!("На этом компьютере нет своего голоса.")))?;
    let started = Instant::now();
    let spoken = own.synthesize(&text, &CancellationToken::new()).await.map_err(|error| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            crate::tr!("Голос этого компьютера: {error}", error = format!("{error:#}")),
        )
    })?;
    count(1, started);
    Ok(([(header::CONTENT_TYPE, "audio/ogg")], spoken.audio).into_response())
}

async fn chat(
    State(app): State<Arc<App>>,
    caller: Caller,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let (base, key) =
        own_model(&app).ok_or_else(|| ApiError::unavailable(crate::tr!("На этом компьютере нет своей модели.")))?;
    let started = Instant::now();
    let answer = forward(
        reqwest::Method::POST,
        format!("{base}/chat/completions"),
        key,
        content_type(&headers).or(Some("application/json")),
        Some(body),
        Duration::from_secs(300),
    )
    .await?;
    // A stream counts by its first word: how soon the model began to answer.
    if answer.status().is_success() {
        count(2, started);
    }
    Ok(answer)
}

async fn llm_models(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
    allowed(&caller)?;
    let (base, key) =
        own_model(&app).ok_or_else(|| ApiError::unavailable(crate::tr!("На этом компьютере нет своей модели.")))?;
    forward(reqwest::Method::GET, format!("{base}/models"), key, None, None, Duration::from_secs(10)).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_form_an_openai_client_sends_comes_apart_by_name() {
        let body = b"--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFF\x00\x01\r\n--XyZ\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nhome\r\n--XyZ--\r\n";
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(axum::http::header::CONTENT_TYPE, "multipart/form-data; boundary=XyZ".parse().unwrap());
        let parts = super::form_parts(&headers, body);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].0, "file");
        assert_eq!(parts[0].1, b"RIFF\x00\x01");
        assert_eq!(parts[1], ("model".to_string(), b"home".to_vec()));
        assert!(super::form_parts(&axum::http::HeaderMap::new(), body).is_empty());
    }

    use std::net::SocketAddr;
    use std::sync::Mutex as StdMutex;

    use axum::body::Body as HttpBody;
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::Map;
    use tower::ServiceExt;

    use super::*;
    use crate::calls::TelegramMessenger;
    use crate::config::{Config, Env, TelegramConfig};

    /// The computer's gateway, its .env with these; its bot never started (no Telegram here).
    fn computer(vars: &[(&str, &str)]) -> Arc<App> {
        let env = Env::from_pairs([("STATE_DIR", "")].into_iter().chain(vars.iter().copied()));
        App::build(Config::for_tests(&env)).unwrap()
    }

    fn key(app: &App, grants: &[&str]) -> String {
        let input =
            Map::from_iter([("name".into(), json!("Колонка: домашний сервер")), ("grants".into(), json!(grants))]);
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
        assert_eq!(pass_on(&bot(&api), "Бабушке", "Купи хлеба.").await.unwrap(), "бабушка");
        assert_eq!(*sent.lock().unwrap(), [json!({ "chat_id": "43", "text": "Купи хлеба." })]);

        let (api, _) = bot_api(true).await;
        let refused = pass_on(&bot(&api), "мама", "Я задержусь").await.unwrap_err();
        assert_eq!(refused.status, StatusCode::BAD_GATEWAY);
        assert!(refused.message.starts_with("Telegram не принял сообщение"), "{}", refused.message);
        assert!(!refused.message.contains("123:test"), "never the bot's token");
    }
}
