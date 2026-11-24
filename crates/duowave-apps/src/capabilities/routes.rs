//! The panel's view of the capabilities layer:
//!
//! - `GET /api/v1/publishers` → `{publishers}`: where a file can go — the gateway's own publishers and the platforms'
//!   upload pages, for «Поделиться» anywhere in the panel.
//! - `GET /api/v1/voice/providers` → `{providers}`: who voices the apps' lines — the computer, ElevenLabs — ready or
//!   not, the key only as «задан · …a1b2».
//! - `PUT /api/v1/voice/providers/{id}/key` `{key}` / `DELETE` — the owner's key into the gateway's safe, typed by a
//!   person in the panel, never by an app or a script's key.
//! - `POST /api/v1/voice/providers/{id}/check` → `{voices, balance?, problem?}`: does the key work, what is left.
//! - `POST /api/v1/publishers/{id}/send` `{app?, file, input?}` — «Поделиться» by a gateway's publisher (Telegram):
//!   a file of the app's folder from its window, or a folder of the house by its address (`files/…`) from any screen
//!   of the panel, as the person may see it. The person's own press is the «да»; the line goes to the sender's journal
//!   — the app's, or the bus journal under the person's name.
//! - `POST /api/v1/publishers/shared` `{app?, where, file?}` — what the browser did by itself (the phone's menu,
//!   download, copied text, a platform's page opened; for an app also «в «Видео»», a copy into the house), noted in
//!   the same journal.
//! - `POST /api/v1/recordings?folder=files/…` (a WAV) — «Записать своим голосом» into a folder of the house, as the
//!   person may write there; an app's window records into its own folder (`/api/v1/packages/{id}/recording`).

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::bus::BusFolders;
use crate::bus::{address, Address, Bus, BusError, Trail};
use crate::capabilities::publish::{Kind, Publishers, Request, PANEL};
use crate::capabilities::voices::Voices;
use crate::core::identity::permissions::{use_of, CONNECTORS_MANAGE, CONNECTORS_VIEW};
use crate::core::identity::PrincipalKind;
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Uses, Zone};
use crate::plugins::files::PluginFiles;
use crate::plugins::PluginHost;
use crate::secrets::SecretsService;
use crate::skills::capabilities::CapabilityName;

pub fn routes<S: crate::capabilities::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/publishers", get(publishers))
        .route("/api/v1/publishers/shared", post(publish_noted))
        .route("/api/v1/publishers/{id}/send", post(publish_send))
        .route("/api/v1/recordings", post(recording))
        .route("/api/v1/voice/providers", get(voice_providers))
        .route("/api/v1/voice/providers/{id}/key", put(voice_key).delete(voice_key_removed))
        .route("/api/v1/voice/providers/{id}/check", post(voice_check))
}

/// Anyone signed in to the panel may see where a file can go: sending itself is the person's own act in «Поделиться».
async fn publishers(Uses(publishers): Uses<Arc<Publishers>>, _caller: Caller) -> ApiResult<Value> {
    Ok(ok(json!({ "publishers": publishers.cards() })))
}

async fn voice_providers(Uses(voices): Uses<Arc<Voices>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(json!({ "providers": voices.cards() })))
}

fn provider(voices: &Voices, id: &str) -> Result<Arc<dyn crate::capabilities::voices::VoiceProvider>, ApiError> {
    voices.get(id).ok_or_else(|| ApiError::not_found(crate::tr!("нет поставщика голосов {id}", id = id)))
}

/// Keys are typed by people, never by a script's key.
async fn voice_key(
    Uses(voices): Uses<Arc<Voices>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<Value> {
    if caller.permitted(CONNECTORS_MANAGE)?.kind != PrincipalKind::User {
        return Err(ApiError::forbidden(crate::tr!("Ключ вставляет человек, не ключ доступа.")));
    }
    let key = body.get("key").and_then(Value::as_str).map(str::trim).unwrap_or("");
    if key.is_empty() {
        return Err(ApiError::field("key", crate::tr!("ключ пустой")));
    }
    let provider = provider(&voices, &id)?;
    provider.set_key(Some(key)).map_err(|error| ApiError::field("key", &error))?;
    tracing::info!(target: "voices", "the key of {id} is set in the panel");
    Ok(ok(json!({ "provider": provider.card() })))
}

async fn voice_key_removed(
    Uses(voices): Uses<Arc<Voices>>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    if caller.permitted(CONNECTORS_MANAGE)?.kind != PrincipalKind::User {
        return Err(ApiError::forbidden(crate::tr!("Ключ убирает человек, не ключ доступа.")));
    }
    provider(&voices, &id)?.set_key(None).map_err(|error| ApiError::field("key", &error))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Asks the provider itself: how many voices the key gives and what is left on the plan.
async fn voice_check(
    Uses(voices_service): Uses<Arc<Voices>>,
    Uses(secrets): Uses<Arc<SecretsService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_MANAGE)?;
    let provider = provider(&voices_service, &id)?;
    let voices = provider.voices().await;
    let balance = provider.balance().await;
    let problem = voices.as_ref().err().or(balance.as_ref().err()).map(|error| secrets.redact(error));
    Ok(ok(json!({
        "voices": voices.as_ref().map(Vec::len).unwrap_or(0),
        "balance": balance.ok().flatten(),
        "problem": problem,
    })))
}

/// Who sends from «Поделиться»: an app's window (its folder, its journal) or a person in the panel (a folder of the
/// house by its address, the bus journal under their name).
pub enum Sender {
    App(String),
    Person { trail: Trail, from: Address },
}

/// The sender of a press in the panel: a person, never a script's key; for an app, one that is installed.
pub fn sender_of(plugins: &PluginHost, caller: &Caller, of: Option<&str>) -> Result<(Sender, String), ApiError> {
    let principal = caller.permitted(&use_of(CapabilityName::Plugins))?.clone();
    if principal.kind != PrincipalKind::User {
        return Err(ApiError::forbidden(crate::tr!("Отправляет человек своим нажатием, не ключ доступа.")));
    }
    match of.map(str::trim).filter(|id| !id.is_empty()) {
        Some(id) => {
            if plugins.installed(id).is_none() {
                return Err(ApiError::not_found(crate::tr!("Нет приложения «{id}».", id = id)));
            }
            Ok((Sender::App(id.to_string()), principal.name.clone()))
        }
        None => {
            let from = Address::new(address::PERSON, &principal.id).map_err(ApiError::bad_request)?;
            let trail = Trail::principal(&principal).from_afar(caller.zone == Zone::Remote);
            Ok((Sender::Person { trail, from }, principal.name.clone()))
        }
    }
}

fn bus_error(error: BusError) -> ApiError {
    match error {
        BusError::NotFound(why) => ApiError::not_found(why),
        BusError::Denied(why) => ApiError::forbidden(why),
        other => ApiError::bad_request(other.to_string()),
    }
}

/// The file on disk: the app's own, or a folder's file the person may see (the bus says so, as in «Папки»).
async fn file_of(plugin_files: &PluginFiles, bus: &Bus, sender: &Sender, file: &str) -> Result<PathBuf, ApiError> {
    let path = match sender {
        Sender::App(id) => plugin_files.inside(id, file).map_err(ApiError::bad_request)?,
        Sender::Person { trail, from } => {
            let to = Address::parse(file).map_err(ApiError::bad_request)?;
            if to.kind() != address::FILES {
                return Err(ApiError::field("file", crate::tr!("файл — адрес в папках дома: files/…")));
            }
            bus.call(trail.clone(), from.clone(), to.clone(), "stat", json!({}), vec![]).await.map_err(bus_error)?;
            let spot = bus
                .files()
                .and_then(|files| files.places().spot(&to))
                .ok_or_else(|| ApiError::not_found(crate::tr!("Нет файла {file}.", file = file)))?;
            crate::bus::providers::files::path::existing(&spot.root, &spot.rest)
                .map_err(|_| ApiError::not_found(crate::tr!("Нет файла {file}.", file = file)))?
        }
    };
    if !path.is_file() {
        return Err(ApiError::not_found(crate::tr!("Нет файла {file}.", file = file)));
    }
    Ok(path)
}

/// «отправлено: …» into the sender's journal.
fn note(plugins: &PluginHost, bus: &Bus, sender: &Sender, who: &str, how: &str, line: &str) {
    match sender {
        Sender::App(id) => {
            let by = if who.is_empty() { String::new() } else { format!(" ({who})") };
            plugins.note_line(id, "info", &format!("{line}{by}"));
        }
        Sender::Person { trail, from } => {
            if let Ok(to) = Address::parse(crate::bus::providers::system::PUBLISH) {
                bus.note_share(trail, from, &to, how, line);
            }
        }
    }
}

#[derive(Deserialize)]
struct SendBody {
    #[serde(default)]
    app: Option<String>,
    file: String,
    /// The publisher's own fields: Telegram's `to` and `caption`.
    #[serde(default)]
    input: Value,
}

async fn publish_send(
    Uses(plugins): Uses<Arc<PluginHost>>,
    Uses(files): Uses<Arc<PluginFiles>>,
    Uses(bus): Uses<Arc<Bus>>,
    Uses(publishers): Uses<Arc<Publishers>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<SendBody>,
) -> ApiResult<Value> {
    let (sender, who) = sender_of(&plugins, &caller, body.app.as_deref())?;
    let path = file_of(&files, &bus, &sender, &body.file).await?;
    let request = Request {
        sender: match &sender {
            Sender::App(id) => id.clone(),
            Sender::Person { .. } => PANEL.into(),
        },
        file: path,
        path: body.file.clone(),
        input: if body.input.is_object() { body.input } else { json!({}) },
    };
    let sent = publishers.send(&id, request).map_err(ApiError::bad_request)?;
    let published = sent.await.map_err(ApiError::bad_request)?;
    note(&plugins, &bus, &sender, &who, &id, &published.line);
    Ok(ok(published.answer))
}

#[derive(Debug, Default, Deserialize)]
pub struct SharedBody {
    #[serde(default)]
    pub app: Option<String>,
    /// Where it went: `videos` (an app's file copied into the house's «Видео»), `download`, `system` (the phone's
    /// menu), `copy` (the text), or a manual publisher's id (a platform's upload page).
    #[serde(rename = "where")]
    pub to: String,
    #[serde(default)]
    pub file: Option<String>,
}

async fn publish_noted(
    Uses(plugins): Uses<Arc<PluginHost>>,
    Uses(publishers): Uses<Arc<Publishers>>,
    Uses(files): Uses<Arc<PluginFiles>>,
    Uses(bus): Uses<Arc<Bus>>,
    caller: Caller,
    Body(body): Body<SharedBody>,
) -> ApiResult<Value> {
    Ok(ok(noted(&plugins, &publishers, &files, &bus, &caller, body).await?))
}

/// What the browser did by itself, in the sender's journal; «в «Видео»» copies an app's file into the house first.
/// The journal's line for what a person sent, and «В Видео»'s copy: the apps, the platforms' pages, the apps' files
/// and the household's folders (through the bus).
pub async fn noted(
    plugins: &PluginHost,
    publishers: &Publishers,
    plugin_files: &PluginFiles,
    bus: &Bus,
    caller: &Caller,
    body: SharedBody,
) -> Result<Value, ApiError> {
    let (sender, who) = sender_of(plugins, caller, body.app.as_deref())?;
    let manual = publishers.cards().into_iter().find(|card| card.kind == Kind::Manual && card.id == body.to);
    let place = match body.to.as_str() {
        "videos" if matches!(sender, Sender::App(_)) => crate::tr!("в «Видео»"),
        "download" => crate::tr!("скачано"),
        "system" => crate::tr!("меню «Поделиться» телефона"),
        "copy" => crate::tr!("текст скопирован"),
        _ => manual.map(|card| card.title).ok_or_else(|| {
            ApiError::field(
                "where",
                crate::tr!("куда: videos, download, system, copy или страница площадки из /api/v1/publishers"),
            )
        })?,
    };
    let mut saved = None;
    if let Some(file) = body.file.as_deref() {
        let from = file_of(plugin_files, bus, &sender, file).await?;
        if body.to == "videos" {
            saved = Some(save_to_videos(bus, &from)?);
        }
    }
    let what = body.file.clone().unwrap_or_else(|| crate::tr!("текст"));
    note(
        plugins,
        bus,
        &sender,
        &who,
        &body.to,
        &crate::tr!("отправлено: {what} → {place}", what = what, place = place),
    );
    Ok(json!({ "where": body.to, "saved": saved }))
}

/// The house's «Видео»: `files/home/Videos`, wherever the home folder is (`~/DuoWave` in the app, else the data's).
const VIDEOS: &str = "files/home/Videos";

/// A copy of an app's file in the house's «Видео», under a free name.
fn save_to_videos(bus: &Bus, from: &std::path::Path) -> Result<String, ApiError> {
    let address = Address::parse(VIDEOS).map_err(ApiError::internal)?;
    let folder = bus
        .files()
        .and_then(|files| files.places().spot(&address))
        .map(|spot| spot.path())
        .ok_or_else(|| ApiError::internal(crate::tr!("нет папки «Видео»")))?;
    std::fs::create_dir_all(&folder).map_err(ApiError::internal)?;
    let name = from.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let (stem, extension) = name.rsplit_once('.').unwrap_or((name.as_str(), ""));
    let mut target = folder.join(&name);
    let mut n = 2;
    while target.exists() {
        target = folder.join(format!("{stem}-{n}.{extension}"));
        n += 1;
    }
    std::fs::copy(from, &target).map_err(ApiError::internal)?;
    let name = target.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(format!("{VIDEOS}/{name}"))
}

#[derive(Deserialize)]
struct RecordingQuery {
    folder: String,
}

/// «Записать своим голосом» into a folder of the house: the panel opened the microphone; the WAV is written as the
/// person, where they may write — the bus refuses a folder they may not.
async fn recording(
    Uses(plugins): Uses<Arc<PluginHost>>,
    Uses(bus): Uses<Arc<Bus>>,
    caller: Caller,
    Query(query): Query<RecordingQuery>,
    body: axum::body::Bytes,
) -> ApiResult<Value> {
    use base64::Engine as _;
    let (sender, _) = sender_of(&plugins, &caller, None)?;
    let Sender::Person { trail, from } = sender else {
        return Err(ApiError::forbidden(crate::tr!("Запись — от человека.")));
    };
    let wav = crate::speech::audio::pcm::decode_wav(&body)
        .map_err(|_| ApiError::bad_request(crate::tr!("Запись — не WAV.")))?;
    let seconds = wav.pcm.len() as f64 / f64::from(wav.sample_rate.max(1));
    let folder = query.folder.trim().trim_end_matches('/');
    let name = format!("voice-{}.wav", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let to = Address::parse(&format!("{folder}/{name}")).map_err(ApiError::bad_request)?;
    if to.kind() != address::FILES {
        return Err(ApiError::field("folder", crate::tr!("папка — адрес в папках дома: files/…")));
    }
    let input = json!({ "base64": base64::engine::general_purpose::STANDARD.encode(&body) });
    bus.call(trail, from, to.clone(), "write", input, vec![]).await.map_err(bus_error)?;
    Ok(created(json!({ "file": to.as_str(), "seconds": (seconds * 10.0).round() / 10.0 })))
}
