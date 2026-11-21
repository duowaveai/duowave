//! `/home/v1` on the computer: its own servers for the gateway on the station, OpenAI-compatible, only from the home
//! network and only with a key that has the right `home.models`. Its recognition and voice servers listen on
//! 127.0.0.1 and stay closed to the network: this gateway is the one door to them. What it has to give is the built-in
//! helper's (src/helpers/builtin.rs) — the same parts the computer takes for itself.
//!
//! - `/home/v1/status` — what is ready: recognition, our voice, a model of its own; and in `contract`, what this
//!   computer gives as a helper, by the helper's contract (helpers/contract.rs: `{version, helper, abilities}`);
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
use crate::helpers::builtin::Hearing;
use crate::helpers::contract::{Ability, Contract};
use crate::helpers::OwnParts;
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
        .route("/home/v1/plugins/install", post(plugin_install))
        .route("/home/v1/character", post(character))
        .route("/home/v1/character/plan", post(character_plan))
        .route("/home/v1/voice/pending", get(voice_pending).post(voice_pending_answer))
        .route("/home/v1/messenger/bot", post(messenger_bot))
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

/// What a handler needs of its caller besides the right: only the status, or one ability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Needs {
    Status,
    Ability(Ability),
}

/// Who may ask `/home/v1`: a home over the link (stage 7) with the right `home.models` and — for an ability — only
/// what this computer gives that home (another home's voice only a stock one, `voice_for_others`); anyone else only
/// from this computer or the home network (a proxied request is judged by its client), with the right. Then the
/// caller is seen for «Помогаю».
fn allowed(caller: &Caller, needs: Needs) -> Result<(), ApiError> {
    let app = caller.app();
    if let Some(link) = caller.link() {
        caller.permitted(HOME_MODELS)?;
        if let Needs::Ability(ability) = needs {
            let given = app.clients.gives(&link.client, ability)
                && (ability != Ability::Voice || link.own_home || voice_for_others(app));
            if !given {
                return Err(ApiError::forbidden(crate::tr!("Это Турбо не даёт этого вашему дому.")));
            }
        }
    } else {
        caller.zone_at_least(Zone::Home)?;
        caller.permitted(HOME_MODELS)?;
    }
    app.clients.saw(caller);
    Ok(())
}

/// Whether another home may hear this computer's voice now (D31): only a stock engine of this computer — Silero or
/// Piper at a home address —, never «Мой голос», a voice lab's voice or a cloud voice.
pub(crate) fn voice_for_others(app: &App) -> bool {
    let Some(own) = app.voice.own_speech() else { return false };
    let stock = app.voice.own_choice().is_some_and(|pair| matches!(pair.model.as_str(), "silero" | "piper"));
    stock && is_local_url(Some(&own.base_url()))
}

/// One answer given: today's «Что даю колонке» and the client's own count.
fn served(caller: &Caller, which: usize, ability: Ability, started: Instant) {
    count(which, started);
    caller.app().clients.served(caller, ability, started);
}

#[derive(serde::Deserialize)]
struct CatalogQuery {
    /// The station's person asked to check now: this computer reads its feed again first.
    fresh: Option<String>,
}

/// The packages' catalog for the station without internet of its own (docs/plans/plugins.md, stage 9, «через Mac»):
/// this computer's copy of the feed and its signature — the station checks them with its own key — and the copies of
/// the extra feeds it names (`feeds: [{url, feed}]`, docs/plans/catalog-feeds.md), with when it was read
/// (`fetchedAt`). The copy is kept as fresh as this computer keeps its own: read again when it is older than its six
/// hours, at once with `fresh`. Before 20.11 it went as it was, and the station took it for read that moment: a
/// station without internet of its own never saw an update while the Mac's panel was the station's.
async fn catalog(
    State(app): State<Arc<App>>,
    caller: Caller,
    axum::extract::Query(query): axum::extract::Query<CatalogQuery>,
) -> Result<Response, ApiError> {
    allowed(&caller, Needs::Ability(Ability::Catalog))?;
    let _ = app.catalog.feed(query.fresh.is_some()).await;
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
    allowed(&caller, Needs::Ability(Ability::Catalog))?;
    let bytes = app.catalog.fetch_for_station(&query.url).await?;
    Ok(bytes.into_response())
}

/// The packages on here (docs/plans/home-and-devices.md, stage 5а): each with its version, title, kind, engine and
/// state — the home's one list «Установлено» shows them with this Турбо's name as their place. `ready` — on and
/// running (what the station may hand over).
async fn plugins(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
    allowed(&caller, Needs::Ability(Ability::Apps))?;
    let view = app.plugins.view().await;
    let all: Vec<Value> = view
        .packages
        .iter()
        .map(|package| {
            json!({
                "id": package.id,
                "version": package.version,
                "title": package.title,
                "kind": package.kind,
                "runtime": package.runtime,
                "enabled": package.enabled,
                "state": package.state,
                "ready": package.enabled && package.state != "error",
            })
        })
        .collect();
    Ok(axum::Json(json!({ "plugins": all })).into_response())
}

/// Only the computer's own home names the character it speaks as, or asks how its voice would stand here.
fn own_home_only(caller: &Caller) -> Result<(), ApiError> {
    allowed(caller, Needs::Ability(Ability::Voice))?;
    if caller.link().is_some_and(|link| !link.own_home) {
        return Err(ApiError::forbidden(crate::tr!("Голос персонажа меняет только свой дом этого компьютера.")));
    }
    Ok(())
}

/// Whether the caller speaks for this computer's own home (clean-path check 18.11, items 41 and 41б): its link with
/// the home's proof, or — the station's plain entry, which calls this computer by address, not over the link — an API
/// key with `home.models` from the home network: the key this computer's administrator made for the home's gateway
/// (a key client shows in «Помогаю» as the own home). A person's sign-in, whatever their role, is not the home.
fn speaks_for_own_home(caller: &Caller) -> bool {
    match caller.link() {
        Some(link) => link.own_home,
        None => {
            caller.zone_at_least(Zone::Home).is_ok()
                && caller.principal().is_some_and(|principal| {
                    principal.kind == crate::core::identity::principal::PrincipalKind::ApiKey
                        && crate::core::identity::can(Some(principal), HOME_MODELS)
                })
        }
    }
}

/// The home's character (docs/plans/home-and-devices.md, stage 2): its voice is part of it, and this Турбо speaks for
/// the home. The centre names the character it chose (`id`, and its package by id and version from the signed feed):
/// a package not here is installed — from `archive` (`{url, sha256}`, the address and hash the centre's own signed feed
/// gives, checked here) when the centre sends one, else from this computer's catalog —, the character chosen here
/// with its voice — so the station speaks it through `/home/v1/tts`. `engine`, the model the owner chose at the centre
/// (clean-path check 18.11, item 40), is kept for the character and its voice goes with it instead of this computer's
/// pick. Only the computer's own home: another home gets a stock voice only.
async fn character(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> Result<Response, ApiError> {
    own_home_only(&caller)?;
    let request: Value = serde_json::from_slice(&body)
        .map_err(|_| ApiError::bad_request("JSON: {id, package: {id, version}, engine, archive}".to_string()))?;
    let id = request.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("id".to_string()));
    }
    if let Some(engine) = request.get("engine").and_then(Value::as_str) {
        if crate::voice_lab::clone_engines::EngineId::parse(engine).is_none() {
            return Err(ApiError::bad_request(crate::tr!("Нет такой модели голоса: {engine}.", engine = engine)));
        }
        // A model that does not fit on the disk here is never downloaded, nor kept.
        super::voice_plan::check_room(&app, engine).await?;
        app.characters.name_engine(&id, engine)?;
    }
    if let Some(package) = request.get("package").and_then(|package| package.get("id")).and_then(Value::as_str) {
        let version = request.get("package").and_then(|package| package.get("version")).and_then(Value::as_str);
        let here = app.plugins.installed(package).map(|installed| installed.manifest.version.to_string());
        if here.is_none() || (version.is_some() && here.as_deref() != version) {
            let archive = request.get("archive").and_then(|archive| {
                Some((archive.get("url")?.as_str()?.to_string(), archive.get("sha256")?.as_str()?.to_string()))
            });
            match (archive, version) {
                (Some((url, sha256)), Some(version)) => {
                    // The home's own centre vouches for the package's voice (items 41, 41б).
                    let vouched = speaks_for_own_home(&caller);
                    app.catalog.install_from_centre(package, version, &url, &sha256, vouched, None).await?;
                }
                (Some(_), None) => return Err(ApiError::bad_request("archive: package.version".to_string())),
                (None, _) => {
                    app.catalog.install(package, version, here.is_some(), None).await?;
                }
            }
        }
    }
    // The owner's own voice put on top of the character's (test 19.11, remark 11) stays when the centre names the same
    // character again — on every persona save and every reconnect of this computer it does. Another character comes
    // with its own voice, as decided on 17.11; «Вернуть голос персонажа» in «Голос» takes the character's back.
    let on_top = app.models.chosen_voice().is_some_and(|chosen| own_on_top(&chosen.model_id));
    let same = app.characters.active_id().as_deref() == Some(id.as_str());
    let body = crate::characters::ChooseBody { voice: Some(!(on_top && same)), install: true, ..Default::default() };
    let answer = app.characters.choose(&id, body, true, None).await?;
    Ok(axum::Json(json!({ "character": id, "voice": answer.voice })).into_response())
}

/// An own voice of the voice lab that is no character's (`own:<id>`, not `own:c-…`): the owner's own recording.
fn own_on_top(model_id: &str) -> bool {
    model_id.strip_prefix(crate::models::models_service::OWN_PREFIX).is_some_and(|id| !id.starts_with("c-"))
}

/// How the character's voice would stand on this computer (clean-path check 18.11, item 40): the plan the centre's
/// panel shows before and while the voice comes (voice_plan.rs). Only the computer's own home.
async fn character_plan(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> Result<Response, ApiError> {
    own_home_only(&caller)?;
    let request: super::voice_plan::PlanRequest = serde_json::from_slice(&body)
        .map_err(|_| ApiError::bad_request("JSON: {id, package: {id, version}, voices, reference}".to_string()))?;
    if request.id.is_empty() {
        return Err(ApiError::bad_request("id".to_string()));
    }
    Ok(axum::Json(super::voice_plan::plan(&app, &request).await).into_response())
}

/// The voice waiting on this computer (voice_plan.rs, `waiting`): `{pending: … | null, computer}`. Only its own home.
async fn voice_pending(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
    own_home_only(&caller)?;
    Ok(axum::Json(waiting_answer(&app)).into_response())
}

/// The owner's answer to the waiting voice, from the home's centre: `{"answer": "anyway" | "retry" | "cancel"}` →
/// what waits now. Only its own home.
async fn voice_pending_answer(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> Result<Response, ApiError> {
    own_home_only(&caller)?;
    let request: Value = serde_json::from_slice(&body)
        .map_err(|_| ApiError::bad_request(r#"JSON: {"answer": "anyway" | "retry" | "cancel"}"#.to_string()))?;
    let answer = request.get("answer").and_then(Value::as_str).unwrap_or_default();
    super::voice_plan::answer(&app, answer, caller.actor()).await?;
    Ok(axum::Json(waiting_answer(&app)).into_response())
}

fn waiting_answer(app: &App) -> Value {
    json!({
        "pending": super::voice_plan::waiting(app),
        "computer": app.identity.gateway().map(|me| me.name).unwrap_or_default(),
    })
}

/// The home's Telegram (docs/plans/home-and-devices.md, stage 2): the centre keeps the token and the contacts, this
/// Турбо runs the bot with them (`{enabled, contacts, alertChat, token}`). Only the computer's own home.
async fn messenger_bot(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> Result<Response, ApiError> {
    allowed(&caller, Needs::Ability(Ability::Messages))?;
    if caller.link().is_some_and(|link| !link.own_home) {
        return Err(ApiError::forbidden(crate::tr!("Бот Telegram этого компьютера настраивает только его свой дом.")));
    }
    let request: Value = serde_json::from_slice(&body)
        .map_err(|_| ApiError::bad_request("JSON: {enabled, contacts, alertChat, token}".to_string()))?;
    let setting: crate::settings::definitions::TelegramSetting =
        serde_json::from_value(request.clone()).map_err(|error| ApiError::bad_request(format!("Telegram: {error}")))?;
    let token = request.get("token").and_then(Value::as_str);
    app.connector_settings.take_home_telegram(&setting, token)?;
    Ok(axum::Json(json!({ "enabled": setting.enabled, "contacts": setting.contacts.len() })).into_response())
}

/// An app's code for the home (docs/plans/home-and-devices.md, stage 5б): the centre installed its face and asks this
/// Турбо for the code — by id and version from this computer's own signed feed (its catalog checks the signature and
/// the archive's hash), installed or updated to that version and switched on. The centre checked the person's right;
/// only the computer's own home asks this, another home never installs anything here.
async fn plugin_install(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> Result<Response, ApiError> {
    allowed(&caller, Needs::Ability(Ability::Apps))?;
    if caller.link().is_some_and(|link| !link.own_home) {
        return Err(ApiError::forbidden(crate::tr!("Чужой дом ничего не ставит на этот компьютер.")));
    }
    let request: Value =
        serde_json::from_slice(&body).map_err(|_| ApiError::bad_request("JSON: {id, version}".to_string()))?;
    let id = request.get("id").and_then(Value::as_str).unwrap_or_default();
    let version = request.get("version").and_then(Value::as_str);
    if id.is_empty() {
        return Err(ApiError::bad_request("id".to_string()));
    }
    let here = app.plugins.installed(id).map(|installed| installed.manifest.version.to_string());
    // Here already at that version or newer — switched off, say: switched on as it is. Asking the catalog for an older
    // one answered «Новее X в каталоге нет» and left it off (before 20.11).
    let newer_here = matches!((here.as_deref(), version), (Some(here), Some(wanted)) if crate::home::apps::newer_or_same(here, wanted));
    if !newer_here {
        match app.catalog.install(id, version, here.is_some(), None).await {
            Ok(_) => {}
            // Not in this computer's catalog: the feed the centre has it from (clean-path check 19.11, YouTube from a
            // demo feed the station's admin added), taken only as this catalog pinned it.
            Err(error) if error.status == StatusCode::NOT_FOUND => {
                let Some(feed) = request.get("feed").and_then(Value::as_str) else { return Err(error) };
                app.catalog.install_pinned_from(id, version, feed, None).await?;
            }
            Err(error) => return Err(error),
        }
    }
    let view = app.plugins.enable(id, None).await?;
    Ok(axum::Json(json!({ "id": id, "version": view.version, "state": view.state })).into_response())
}

/// A call of a plugin of this computer for the station: its function, its input, how long it may take.
async fn plugin_call(
    State(app): State<Arc<App>>,
    caller: Caller,
    axum::extract::Path(id): axum::extract::Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&caller, Needs::Ability(Ability::Apps))?;
    let request: Value =
        serde_json::from_slice(&body).map_err(|_| ApiError::bad_request("JSON: {function, input, kind}"))?;
    let function = request.get("function").and_then(Value::as_str).unwrap_or_default();
    let input = request.get("input").cloned().unwrap_or_else(|| json!({}));
    let kind = match request.get("kind").and_then(Value::as_str) {
        Some("voice") => crate::plugins::runtime::CallKind::Voice,
        Some("background") => crate::plugins::runtime::CallKind::Background,
        Some("agent") => crate::plugins::runtime::CallKind::Agent,
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
    allowed(&caller, Needs::Ability(Ability::History))?;
    let history = history_of(&app)?;
    Ok(axum::Json(json!({ "missing": history.missing(&ids_of(&body)?) })).into_response())
}

/// The station's history objects: each kept only when its name is its hash.
async fn history_objects(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> Result<Response, ApiError> {
    allowed(&caller, Needs::Ability(Ability::History))?;
    let history = history_of(&app)?;
    let request: Value = serde_json::from_slice(&body).map_err(|_| ApiError::bad_request("JSON: {objects}"))?;
    let objects = request.get("objects").and_then(Value::as_array).cloned().unwrap_or_default();
    let stored = tokio::task::spawn_blocking(move || history.receive(&objects)).await.map_err(ApiError::internal)?;
    Ok(axum::Json(json!({ "stored": stored })).into_response())
}

/// The station's lines and marks: its marks apart, its records into the apps' lines here.
async fn history_lines(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> Result<Response, ApiError> {
    allowed(&caller, Needs::Ability(Ability::History))?;
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
    allowed(&caller, Needs::Ability(Ability::History))?;
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
    allowed(&caller, Needs::Ability(Ability::Messages))?;
    let bot = &app.calls.telegram;
    if !bot.enabled() {
        return Err(ApiError::unavailable(no_bot()));
    }
    Ok(axum::Json(json!({ "contacts": bot.contact_names() })).into_response())
}

/// The station's «передай маме, что …»: a text to a contact of this computer's bot.
async fn messenger_send(State(app): State<Arc<App>>, caller: Caller, body: Bytes) -> Result<Response, ApiError> {
    allowed(&caller, Needs::Ability(Ability::Messages))?;
    let request: Value = serde_json::from_slice(&body).map_err(|_| ApiError::bad_request(send_shape()))?;
    let field = |name: &str| request.get(name).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
    let (contact, text) = (field("contact"), field("text"));
    if contact.is_empty() || text.is_empty() {
        return Err(ApiError::bad_request(send_shape()));
    }
    let started = Instant::now();
    let name = pass_on(&app.calls.telegram, &contact, &text).await?;
    served(&caller, 3, Ability::Messages, started);
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

/// What hears for the station: the built-in helper's hearing (helpers/builtin.rs) — this computer's own engine, or its
/// recognition server at home.
fn hearing(app: &App) -> Option<Hearing> {
    app.builtin.hearing().cloned()
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

/// This computer's own model: the built-in helper's (helpers/builtin.rs) — the first OpenAI-compatible provider of
/// «Мозг» at home, and its key, never a helper's model: a computer does not pass a station's questions on to another
/// helper, nor to a link's bridge.
fn own_model(app: &App) -> Option<(String, Option<String>)> {
    app.builtin.model_route()
}

/// The language of a Piper voice by its id: «piper/ru_RU-irina-medium» — «ru». Other voices say none. The built-in
/// helper's contract says it of its voice.
pub(crate) fn piper_language(pair: &str) -> Option<String> {
    let voice = pair.rsplit('/').next()?;
    let [locale, _name, _quality] = voice.split('-').collect::<Vec<_>>()[..] else { return None };
    let (language, region) = locale.split_once('_')?;
    let letters = |part: &str, upper: bool| {
        (2..=3).contains(&part.len())
            && part.chars().all(|c| if upper { c.is_ascii_uppercase() } else { c.is_ascii_lowercase() })
    };
    (letters(language, false) && letters(region, true)).then(|| language.to_string())
}

/// What this computer serves now as a helper — the contract of `/home/v1/status` (helpers/contract.rs): the built-in
/// helper's (helpers/builtin.rs), one answer for the gateways it helps and for itself.
pub(crate) fn contract(app: &App) -> Contract {
    app.builtin.contract()
}

/// What a home over the link is given (stage 7): its abilities, and whether its home key proved it this computer's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Given {
    pub abilities: crate::helpers::contract::Abilities,
    pub own_home: bool,
}

/// What is ready here, for the station: the fields a station of 0.11 and before reads, and the contract they are
/// worked out of — one look at the parts. The contract says this computer speaks the link while it helps; a home
/// over the link sees only what it is given, and another home's voice ready only when it is a stock one.
pub(crate) fn status_of(app: &App, given: Option<Given>) -> Value {
    let mut contract = contract(app);
    if app.link.helps() {
        contract.link = Some(crate::helpers::link::VERSION);
    }
    if let Some(given) = given {
        contract.abilities.retain(|state| given.abilities.has(state.id));
        if !given.own_home && !voice_for_others(app) {
            for state in contract.abilities.iter_mut().filter(|state| state.id == Ability::Voice) {
                state.ready = false;
            }
        }
    }
    let voice = contract.get(Ability::Voice).and_then(|voice| voice.details.get("voice")).cloned();
    let mut status = json!({
        "version": crate::VERSION,
        "peerApi": crate::core::build::PEER_API,
        "asr": contract.ready(Ability::Hearing),
        "tts": contract.declares(Ability::Voice),
        "voice": voice,
        "llm": contract.declares(Ability::Model),
        "contract": contract,
    });
    // This computer's state for the apps of its own home's centre (`system/device`, operations/device_state.rs),
    // read with the status every 15 s; another home over the link is not told it.
    if given.is_none_or(|given| given.own_home) {
        status["device"] = crate::operations::device_state::shared().snapshot();
    }
    status
}

async fn status(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
    allowed(&caller, Needs::Status)?;
    let given = caller.link().map(|link| Given { abilities: app.clients.given(&link.client), own_home: link.own_home });
    Ok(axum::Json(status_of(&app, given)).into_response())
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
    allowed(&caller, Needs::Ability(Ability::Hearing))?;
    let hearing = hearing(&app)
        .ok_or_else(|| ApiError::unavailable(crate::tr!("На этом компьютере нет своего распознавания.")))?;
    let started = Instant::now();
    let (base, key) = match hearing {
        Hearing::Server(base, key) => (base, key),
        Hearing::Local(local) => {
            let answer = recognize_here(&local, &headers, &body).await?;
            served(&caller, 0, Ability::Hearing, started);
            return Ok(answer);
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
        served(&caller, 0, Ability::Hearing, started);
    }
    Ok(answer)
}

/// Recognition inside this program, as an OpenAI-compatible server answers it: the WAV out of the form, `{text}` back.
/// The home's `/home/v1/asr/audio/transcriptions` and the voice lab's loopback route for imprint.py
/// (voice_lab/controller.rs) both hear so.
pub(crate) async fn recognize_here(
    local: &Arc<LocalAsr>,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, ApiError> {
    let parts = form_parts(headers, body);
    let file = parts
        .into_iter()
        .find(|(name, _)| name == "file")
        .ok_or_else(|| ApiError::bad_request(crate::tr!("file: звук WAV")))?
        .1;
    let wav = crate::speech::audio::pcm::decode_wav(&file)
        .map_err(|error| ApiError::bad_request(format!("file: {error}")))?;
    let text = local.recognize(&wav.pcm, wav.sample_rate, &CancellationToken::new()).await.map_err(|error| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            crate::tr!("Распознавание этого компьютера: {error}", error = format!("{error:#}")),
        )
    })?;
    Ok(axum::Json(json!({ "text": text })).into_response())
}

async fn asr_models(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
    allowed(&caller, Needs::Ability(Ability::Hearing))?;
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
    allowed(&caller, Needs::Ability(Ability::Voice))?;
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
    // Its voice at a cloud's address while the regulator keeps the clouds in: the text stays home (a station of 0.11
    // reads only `tts`).
    if app.builtin.voice_kept_in() {
        return Err(ApiError::unavailable(crate::tr!("Регулятор не пускает облако")));
    }
    let started = Instant::now();
    let spoken = own.synthesize(&text, &CancellationToken::new()).await.map_err(|error| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            crate::tr!("Голос этого компьютера: {error}", error = format!("{error:#}")),
        )
    })?;
    served(&caller, 1, Ability::Voice, started);
    Ok(([(header::CONTENT_TYPE, "audio/ogg")], spoken.audio).into_response())
}

async fn chat(
    State(app): State<Arc<App>>,
    caller: Caller,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&caller, Needs::Ability(Ability::Model))?;
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
        served(&caller, 2, Ability::Model, started);
    }
    Ok(answer)
}

async fn llm_models(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
    allowed(&caller, Needs::Ability(Ability::Model))?;
    let (base, key) =
        own_model(&app).ok_or_else(|| ApiError::unavailable(crate::tr!("На этом компьютере нет своей модели.")))?;
    forward(reqwest::Method::GET, format!("{base}/models"), key, None, None, Duration::from_secs(10)).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_the_owners_own_recording_counts_as_a_voice_on_top() {
        assert!(super::own_on_top("own:v-mine"));
        assert!(!super::own_on_top("own:c-p-0a1b2c3d4e5f-lena-f5"), "a character's voice is not on top of itself");
        assert!(!super::own_on_top("silero-ru"), "a ready-made voice is chosen, not put on top");
    }

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
    use crate::helpers::contract::{HelperIdentity, HelperStatus};

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
    const STATUS: &str = "/home/v1/status";

    #[test]
    fn a_piper_voice_says_its_language_and_the_other_voices_none() {
        assert_eq!(piper_language("piper/ru_RU-irina-medium").as_deref(), Some("ru"));
        assert_eq!(piper_language("piper/en_US-amy-medium").as_deref(), Some("en"));
        assert_eq!(piper_language("silero/baya"), None);
        assert_eq!(piper_language("piper/denis"), None, "a voice by its name alone");
        assert_eq!(piper_language("omnivoice/3f2a9c1e-77b0-4c1d-9a55-0c1d2e3f4a5b"), None, "a voice lab's id");
        assert_eq!(piper_language("piper/RU_ru-irina-medium"), None);
    }

    #[tokio::test]
    async fn the_computer_answers_its_status_with_the_contract_and_the_old_fields() {
        let app = computer(&[
            ("ASR_BASE_URL", "http://127.0.0.1:8090/v1"),
            ("ASR_LANGUAGE", "ru"),
            ("TTS_OWN_URL", "http://127.0.0.1:8091/v1"),
            ("TTS_OWN_MODEL", "piper"),
            ("TTS_OWN_VOICE", "ru_RU-irina-medium"),
        ]);
        let station = key(&app, &["home.models"]);
        let (status, body) = send(&app, "GET", STATUS, Some(&station), Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        // The fields a station of 0.11 reads, as they were.
        assert_eq!(
            (&body["version"], &body["peerApi"]),
            (&json!(crate::VERSION), &json!(crate::core::build::PEER_API))
        );
        assert_eq!((&body["asr"], &body["tts"], &body["llm"]), (&json!(true), &json!(true), &json!(false)));
        assert_eq!(body["voice"], "piper/ru_RU-irina-medium");

        // And what it gives as a helper, by the contract.
        assert_eq!(body["contract"]["version"], crate::helpers::contract::VERSION);
        let read = HelperStatus::read(&body).unwrap();
        assert_eq!(read.program.as_deref(), Some(crate::VERSION));
        let contract = &read.contract;
        assert_eq!(serde_json::to_value(contract).unwrap(), body["contract"], "read as it was written");
        // This gateway as a helper: its own id and name (core/identity/home.rs), made at its start.
        let me = app.identity.gateway().expect("made at the start");
        assert_eq!((contract.helper.id.as_str(), contract.helper.name.as_str()), (me.id.as_str(), me.name.as_str()));
        let hearing = contract.get(Ability::Hearing).unwrap();
        assert!(hearing.ready, "its recognition server");
        assert_eq!(hearing.details, Map::from_iter([("languages".to_string(), json!(["ru"]))]));
        let voice = contract.get(Ability::Voice).unwrap();
        assert!(voice.ready);
        assert_eq!(voice.details["voice"], "piper/ru_RU-irina-medium");
        assert_eq!(voice.details["languages"], json!(["ru"]));
        assert!(!contract.declares(Ability::Model), "no model of its own");
        assert!(contract.declares(Ability::Messages) && !contract.ready(Ability::Messages), "no bot set up here");
        assert!(contract.ready(Ability::Apps));
        assert_eq!(contract.declares(Ability::History), app.history.is_some());
        assert!(!contract.declares(Ability::Catalog), "no feed here (no PACKAGES_FEED_URL, none built in): no copy");
        reads_the_same_without_the_contract(&body);

        // A model of its own at home, an id and a name of its own: the contract says them, the old field too.
        app.db
            .with(|c| {
                c.execute(
                    "INSERT INTO llm_providers (id, preset, kind, title, base_url, created_at, updated_at)
                     VALUES ('ollama', 'ollama', 'openai', 'Ollama', 'http://127.0.0.1:11434/v1', ?1, ?1)",
                    ["2026-11-15T10:00:00.000Z"],
                )
            })
            .unwrap();
        app.identity.rename(Some("Кабинет"), None).unwrap();
        let (_, body) = send(&app, "GET", STATUS, Some(&station), Value::Null).await;
        assert_eq!(body["llm"], true);
        let read = HelperStatus::read(&body).unwrap().contract;
        assert!(read.ready(Ability::Model));
        assert_eq!(read.helper, HelperIdentity { id: me.id.clone(), name: "Кабинет".into() });
        reads_the_same_without_the_contract(&body);

        // The contract is behind the same door as the rest of /home/v1.
        let (status, _) = send(&app, "GET", STATUS, None, Value::Null).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    /// The answer read again without its `contract`, as a computer of 0.11 answers it: version 0 with the parts the
    /// contract declares — hearing, voice, the model and the apps as ready, the voice by its name — and the messages.
    /// What version 0 cannot know is left out: whether the bot is set up (its contacts' read says it), the history and
    /// the catalog (taken by the release).
    fn reads_the_same_without_the_contract(body: &Value) {
        let read = HelperStatus::read(body).unwrap().contract;
        let mut old = body.clone();
        old.as_object_mut().unwrap().remove("contract");
        let old = HelperStatus::read(&old).unwrap().contract;
        assert_eq!(old.version, 0);
        for ability in [Ability::Hearing, Ability::Voice, Ability::Model, Ability::Apps] {
            let both = |contract: &Contract| (contract.declares(ability), contract.ready(ability));
            assert_eq!(both(&old), both(&read), "{ability:?} in {body}");
        }
        let voice =
            |contract: &Contract| contract.get(Ability::Voice).and_then(|voice| voice.details.get("voice")).cloned();
        assert_eq!(voice(&old), voice(&read));
        assert!(old.declares(Ability::Messages) && read.declares(Ability::Messages));
    }

    #[tokio::test]
    async fn a_computer_without_a_voice_or_a_model_of_its_own_declares_neither_and_so_reads_without_the_contract() {
        // Its recognition server (ASR_BASE_URL at home by default), no TTS_OWN_URL, no model at home.
        let app = computer(&[]);
        let station = key(&app, &["home.models"]);
        let (status, body) = send(&app, "GET", STATUS, Some(&station), Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            (&body["asr"], &body["tts"], &body["voice"], &body["llm"]),
            (&json!(true), &json!(false), &Value::Null, &json!(false))
        );
        let contract = HelperStatus::read(&body).unwrap().contract;
        assert!(contract.ready(Ability::Hearing));
        assert!(!contract.declares(Ability::Voice) && !contract.declares(Ability::Model), "{body}");
        reads_the_same_without_the_contract(&body);
    }

    #[tokio::test]
    async fn a_computer_whose_own_engine_still_loads_its_model_declares_hearing_not_ready_yet() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::from_pairs([("STATE_DIR", "")]);
        let mut config = Config::for_tests(&env);
        // Its own engine, never started here: the model not in.
        config.speech.asr.local =
            Some(crate::config::LocalAsrConfig { dir: dir.path().to_path_buf(), mirror: None, gpu: false });
        let app = App::build(config).unwrap();
        let station = key(&app, &["home.models"]);
        let (status, body) = send(&app, "GET", STATUS, Some(&station), Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["asr"], false, "a station of 0.11 leans on it once the model is in");
        let contract = HelperStatus::read(&body).unwrap().contract;
        let hearing = contract.get(Ability::Hearing).unwrap();
        assert!(!hearing.ready);
        let model = app.local_asr.as_ref().unwrap().model();
        let known = crate::speech::asr::local::catalog::find(&model).unwrap();
        let details = [("model".to_string(), json!(model)), ("languages".to_string(), json!(known.languages))];
        assert_eq!(hearing.details, Map::from_iter(details));
        reads_the_same_without_the_contract(&body);
    }

    #[tokio::test]
    async fn the_contract_says_the_bot_once_it_is_set_up_and_the_catalog_once_there_is_a_copy_for_the_station() {
        const FEED: &str = "https://packages.example.invalid/catalog.json";
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_str().unwrap();
        let app = computer(&[
            ("DATA_DIR", data),
            ("PLUGINS_BUNDLED", "false"),
            ("TELEGRAM_BOT_TOKEN", "123:test"),
            ("PACKAGES_FEED_URL", FEED),
        ]);
        let station = key(&app, &["home.models"]);
        let (_, body) = send(&app, "GET", STATUS, Some(&station), Value::Null).await;
        let contract = HelperStatus::read(&body).unwrap().contract;
        assert!(contract.ready(Ability::Messages), "its bot is set up");
        assert!(!contract.declares(Ability::Catalog), "its feed is named, not read yet");
        let (status, _) = send(&app, "GET", "/home/v1/catalog", Some(&station), Value::Null).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        reads_the_same_without_the_contract(&body);

        // The feed read once (its copy and the note of whose copy it is): the station may take the catalog from here.
        let folder = &app.config.plugins.catalog_dir;
        std::fs::create_dir_all(folder).unwrap();
        std::fs::write(folder.join("catalog.json"), r#"{"format":1,"packages":[]}"#).unwrap();
        let state = json!({ "url": FEED, "fetchedAt": "2026-11-15T10:00:00.000Z" });
        std::fs::write(folder.join("state.json"), state.to_string()).unwrap();
        let (_, body) = send(&app, "GET", STATUS, Some(&station), Value::Null).await;
        assert!(HelperStatus::read(&body).unwrap().contract.ready(Ability::Catalog), "{body}");
        let (status, copy) = send(&app, "GET", "/home/v1/catalog", Some(&station), Value::Null).await;
        assert_eq!(
            (status, &copy["url"], &copy["feed"]),
            (StatusCode::OK, &json!(FEED), &json!(r#"{"format":1,"packages":[]}"#))
        );
        // When it was read goes with it: the station keeps the copy as old as it is (20.11).
        assert_eq!(copy["fetchedAt"], json!("2026-11-15T10:00:00.000Z"));
    }

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

    /// A home's request over the link: its client and whether its home key proved it this computer's own.
    async fn over_link(
        app: &Arc<App>,
        method: &str,
        uri: &str,
        client: &str,
        own_home: bool,
        body: Value,
    ) -> (StatusCode, Value) {
        over_link_in(app, method, uri, client, own_home, body, None).await
    }

    /// [`over_link`] asked in a language (`Accept-Language`, as the centre passes its asker's on).
    async fn over_link_in(
        app: &Arc<App>,
        method: &str,
        uri: &str,
        client: &str,
        own_home: bool,
        body: Value,
        language: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri).header("content-type", "application/json");
        request = request.header("authorization", "Bearer anything");
        if let Some(language) = language {
            request = request.header("accept-language", language);
        }
        let mut request = request.body(HttpBody::from(body.to_string())).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new("127.0.0.1".parse().unwrap(), 5000)));
        request.extensions_mut().insert(crate::platform::http::LinkOrigin {
            client: client.to_string(),
            own_home,
            home: HelperIdentity { id: "home-1".into(), name: "Дача".into() },
            gateway: HelperIdentity { id: "gw-1".into(), name: "Колонка".into() },
        });
        let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// A home paired with this computer: its client id.
    fn client(app: &App, own_home: bool) -> String {
        let (id, _) = app.clients.pair("home-1", "gw-1", own_home, None).unwrap();
        assert!(app.clients.confirm(&id, crate::helpers::clients::Names::default()));
        id
    }

    fn voice_ready(body: &Value) -> Option<bool> {
        let contract = HelperStatus::read(body).unwrap().contract;
        contract.get(Ability::Voice).map(|voice| voice.ready)
    }

    #[tokio::test]
    async fn only_the_computers_own_home_names_the_character_it_speaks_as() {
        // A stock voice that another home may hear: still, only the own home changes whose voice it is.
        let app = computer(&[
            ("TTS_OWN_URL", "http://127.0.0.1:9/v1"),
            ("TTS_OWN_MODEL", "piper"),
            ("TTS_OWN_VOICE", "ru_RU-irina-medium"),
        ]);
        let character = json!({ "id": "butler:arthur" });
        let other = client(&app, false);
        let (status, body) = over_link(&app, "POST", "/home/v1/character", &other, false, character.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        let own = client(&app, true);
        let (status, body) = over_link(&app, "POST", "/home/v1/character", &own, true, character).await;
        // No such character here and no package named: it says so, it is not refused.
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        let (status, _) = over_link(&app, "POST", "/home/v1/character", &own, true, json!({})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// Clean-path check 19.11: «Голос ждёт» lived only on the computer's own page. The voice waiting here is shown to
    /// the home's centre (`GET /home/v1/voice/pending`) with its name and tight memory as numbers, its reason written
    /// from them in the asker's language — not the words of the moment it was found —, and the owner's answer is taken
    /// from there (`POST`); only by this computer's own home.
    #[tokio::test]
    async fn the_turbo_shows_its_waiting_voice_to_its_own_home_and_takes_the_owners_answer() {
        use crate::settings::definitions::{MemoryTight, PendingWait, VoicePending, VOICE_PENDING};
        const PENDING: &str = "/home/v1/voice/pending";
        let app = computer(&[
            ("TTS_OWN_URL", "http://127.0.0.1:9/v1"),
            ("TTS_OWN_MODEL", "piper"),
            ("TTS_OWN_VOICE", "ru_RU-irina-medium"),
        ]);
        let other = client(&app, false);
        let (status, body) = over_link(&app, "GET", PENDING, &other, false, Value::Null).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "another home never sees it: {body}");
        let own = client(&app, true);
        let (status, body) = over_link(&app, "GET", PENDING, &own, true, Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["pending"], Value::Null, "nothing waits: {body}");
        assert!(body["computer"].is_string(), "{body}");

        // A voice waits for the owner's «всё равно»: memory is short. Its old reason was kept in other words.
        let waiting = VoicePending {
            model_id: "piper-denis".into(),
            voice_id: None,
            wait_for: PendingWait { kind: "file".into(), id: "piper-denis".into() },
            anyway: false,
            state: "tight".into(),
            reason: Some("ОЗУ впритык: Memory is tight: …".into()),
            memory: Some(MemoryTight::Short { free_mb: 2048, need_mb: 5120 }),
            recipe: None,
            since: "2026-11-19T10:00:00Z".into(),
        };
        app.settings.set(&VOICE_PENDING, &waiting, None).unwrap();
        let (status, body) = over_link_in(&app, "GET", PENDING, &own, true, Value::Null, Some("ru")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let pending = &body["pending"];
        assert_eq!((&pending["modelId"], &pending["state"]), (&json!("piper-denis"), &json!("tight")), "{body}");
        assert!(pending["title"].as_str().is_some_and(|title| !title.is_empty()), "{pending}");
        assert_eq!(pending["memory"], json!({ "kind": "short", "freeMb": 2048, "needMb": 5120 }));
        assert_eq!(
            pending["reason"],
            "ОЗУ мало: свободно 2,0 ГБ, а нужно около 5,0 ГБ — Мак начнёт подкачку с диска, колонка будет подтормаживать."
        );
        // Asked in English (a panel in English at the centre): the same numbers in its words.
        let (_, body) = over_link_in(&app, "GET", PENDING, &own, true, Value::Null, Some("en")).await;
        let reason = body["pending"]["reason"].as_str().unwrap_or_default();
        assert!(reason.starts_with("Memory is low: 2,0 GB is free, but about 5,0 GB is needed"), "{body}");

        // The owner's answer: never another home's; an unknown one is refused; «cancel» forgets the voice.
        let (status, _) = over_link(&app, "POST", PENDING, &other, false, json!({ "answer": "cancel" })).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(app.models.pending().is_some(), "another home's answer changes nothing");
        let (status, _) = over_link(&app, "POST", PENDING, &own, true, json!({ "answer": "maybe" })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = over_link(&app, "POST", PENDING, &own, true, json!("cancel")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "not the answer's form");
        assert!(app.models.pending().is_some());
        let (status, body) = over_link(&app, "POST", PENDING, &own, true, json!({ "answer": "cancel" })).await;
        assert_eq!((status, &body["pending"]), (StatusCode::OK, &Value::Null), "{body}");
        assert!(app.models.pending().is_none());
        // Nothing waits any more: an answer says so.
        let (status, body) =
            over_link_in(&app, "POST", PENDING, &own, true, json!({ "answer": "anyway" }), Some("ru")).await;
        assert_eq!((status, body["message"].as_str()), (StatusCode::CONFLICT, Some("Голос уже не ждёт.")), "{body}");
    }

    /// Clean-path check 18.11, item 40: the centre asks how the character's voice would stand here before it is
    /// installed — every model with how its voice comes (snapshot, reference), its state, room and memory against
    /// what is free, the model it would go with; the model the centre names is kept and the plan reports it.
    #[tokio::test]
    async fn the_turbo_answers_the_plan_of_the_characters_voice_and_keeps_the_model_the_centre_names() {
        let lab = crate::voice_lab::service::testing::lab().await;
        let app = &lab.app;
        const PLAN: &str = "/home/v1/character/plan";
        let asked = json!({
            "id": "arthur:arthur", "package": { "id": "arthur", "version": "1.2.0" },
            "voices": ["omnivoice", "cosyvoice3"], "reference": true, "title": "Артур", "gender": "male",
            "fallbackVoice": "piper-denis"
        });
        let other = client(app, false);
        let (status, _) = over_link(app, "POST", PLAN, &other, false, asked.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "another home never sees it");
        let own = client(app, true);
        let (status, plan) = over_link(app, "POST", PLAN, &own, true, asked.clone()).await;
        assert_eq!(status, StatusCode::OK, "{plan}");
        let engines = plan["engines"].as_array().unwrap();
        let ids: Vec<&str> = engines.iter().map(|engine| engine["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["omnivoice", "cosyvoice3", "voxcpm"]);
        let omnivoice = &engines[0];
        assert_eq!(
            (&omnivoice["way"], &omnivoice["state"], &omnivoice["supported"], &omnivoice["device"]),
            (&json!("snapshot"), &json!("ready"), &json!(true), &json!("mps")),
            "{omnivoice}"
        );
        assert_eq!(omnivoice["recommended"], true);
        assert_eq!((&omnivoice["percent"], &omnivoice["doneMb"]), (&Value::Null, &Value::Null), "nothing coming");
        assert_eq!(omnivoice["totalMb"], 3_110 + crate::voice_lab::clone_engines::PACKAGES_MB);
        assert_eq!((&omnivoice["freeDiskMb"], &omnivoice["diskFits"]), (&json!(80_000), &json!(true)));
        let (ram, free) = (omnivoice["ramMb"].as_u64().unwrap(), omnivoice["freeRamMb"].as_u64().unwrap());
        assert!(ram > 0 && free > 0, "{omnivoice}");
        assert_eq!(omnivoice["ramFits"], free >= ram);
        let cosy = &engines[1];
        assert_eq!((&cosy["way"], &cosy["state"]), (&json!("snapshot"), &json!("missing")), "{cosy}");
        assert_eq!(cosy["diskFits"], cosy["freeDiskMb"].as_u64() >= cosy["diskMb"].as_u64());
        assert_eq!(engines[2]["way"], "reference", "no snapshot for it: taken from the reference");
        // The model installed here with its snapshot: this computer's pick.
        assert_eq!(plan["chosen"], "omnivoice");
        assert_eq!((&plan["progress"], &plan["voiceOn"]), (&Value::Null, &json!(false)));
        assert_eq!(plan["computer"], app.identity.gateway().unwrap().name);
        assert!(plan.get("fallback").is_some(), "{plan}");

        // The centre's owner wants CosyVoice: the plan asked with it says so.
        let mut wanted = asked.clone();
        wanted["engine"] = json!("cosyvoice3");
        let (_, plan) = over_link(app, "POST", PLAN, &own, true, wanted).await;
        assert_eq!(plan["chosen"], "cosyvoice3");

        // Named with the character: kept here — the next plan, asked without it, goes with it (the character is not
        // installed here, so the choice itself says so).
        let named = json!({ "id": "arthur:arthur", "engine": "cosyvoice3" });
        let (status, _) = over_link(app, "POST", "/home/v1/character", &own, true, named).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (_, plan) = over_link(app, "POST", PLAN, &own, true, asked.clone()).await;
        assert_eq!(plan["chosen"], "cosyvoice3", "{plan}");
        let unknown = json!({ "id": "arthur:arthur", "engine": "f5tts" });
        let (status, _) = over_link(app, "POST", "/home/v1/character", &own, true, unknown).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        // A model the character has no voice for (no snapshot, no reference) is not taken.
        let bare = json!({ "id": "arthur:arthur", "voices": ["omnivoice"], "reference": false });
        let (_, plan) = over_link(app, "POST", PLAN, &own, true, bare).await;
        assert_eq!(plan["chosen"], "omnivoice", "{plan}");
    }

    /// A model that does not fit on this computer's disk is never downloaded: the plan does not choose it (none at all
    /// when nothing fits), the centre naming it is refused and nothing kept, and a character whose models do not fit
    /// gets its fallback voice with no install started.
    #[tokio::test]
    async fn a_model_that_does_not_fit_on_the_disk_is_never_chosen_nor_downloaded() {
        use std::sync::atomic::Ordering;
        let lab = crate::voice_lab::service::testing::lab().await;
        let app = &lab.app;
        // 8 GB free: OmniVoice is installed; CosyVoice (10.5 GB) and VoxCPM (9.5 GB) would not fit.
        lab.memory.free_disk_mb.store(8_000, Ordering::SeqCst);
        const PLAN: &str = "/home/v1/character/plan";
        let own = client(app, true);
        let asked = json!({
            "id": "arthur:arthur", "package": { "id": "arthur", "version": "1.2.0" },
            "voices": ["omnivoice", "cosyvoice3"], "reference": true, "engine": "cosyvoice3"
        });
        let (status, plan) = over_link(app, "POST", PLAN, &own, true, asked).await;
        assert_eq!(status, StatusCode::OK, "{plan}");
        let fits: Vec<(&str, bool)> = plan["engines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|engine| (engine["id"].as_str().unwrap(), engine["diskFits"].as_bool().unwrap()))
            .collect();
        assert_eq!(fits, [("omnivoice", true), ("cosyvoice3", false), ("voxcpm", false)]);
        assert_eq!(plan["chosen"], "omnivoice", "the wanted one does not fit: the one that does");
        // Only CosyVoice has the character's voice: nothing fits, nothing chosen.
        let only_cosy = json!({ "id": "arthur:arthur", "voices": ["cosyvoice3"], "reference": false });
        let (_, plan) = over_link(app, "POST", PLAN, &own, true, only_cosy).await;
        assert_eq!(plan["chosen"], Value::Null, "{plan}");

        // The centre names CosyVoice: refused in plain words, nothing kept.
        let named = json!({ "id": "arthur:arthur", "engine": "cosyvoice3" });
        let (status, body) = over_link(app, "POST", "/home/v1/character", &own, true, named).await;
        assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE, "{body}");
        let message = body["message"].as_str().unwrap();
        assert!(
            message.starts_with("Модель ") && message.contains("не поместится: нужно 10 ГБ, свободно 7,8 ГБ"),
            "{message}"
        );
        assert!(app.settings.get(&crate::settings::definitions::TURBO_VOICE_ENGINES, None).is_none());

        // A character whose only model does not fit, told with no model: chosen with no download.
        let mut cosy_only = crate::characters::contributes::tests::arthur_manifest("1.2.0");
        let character = &mut cosy_only["contributes"]["characters"][0];
        character["voices"] = json!({ "cosyvoice3": { "file": "voices/cosyvoice3.pt", "model": "074ca6dc" } });
        character.as_object_mut().unwrap().remove("ref");
        crate::characters::contributes::tests::write_arthur(&app.config.plugins.packages_dir, &cosy_only);
        app.characters.sync();
        let (status, body) =
            over_link(app, "POST", "/home/v1/character", &own, true, json!({ "id": "arthur:arthur" })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_ne!(body["voice"]["state"], "installing", "{body}");
        assert!(body["voice"]["pending"].is_null(), "{body}");
        assert!(app.voice_lab.job().is_none(), "no model download started");
        assert_eq!(app.personas.chosen().unwrap().preset_id.as_deref(), Some("arthur:arthur"));
    }

    /// The centre's package comes with its archive's address and hash from the centre's signed feed: a hash that does
    /// not match installs nothing; the one that matches installs it, without this computer's own feed.
    #[tokio::test]
    async fn the_package_from_the_centre_installs_only_when_its_sha256_matches() {
        use crate::plugins::install::tests::{character_manifest, vgp};
        use sha2::Digest;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_str().unwrap();
        let app = computer(&[("DATA_DIR", data), ("PLUGINS_BUNDLED", "false"), ("PACKAGES_FEED_URL", "off")]);
        let site = crate::plugins::catalog::tests::site().await;
        let archive = vgp(&character_manifest("arthur", "1.2.0"));
        site.publish(&[(crate::plugins::catalog::tests::arthur("1.2.0"), archive.clone())]);
        let url = site.url.replace("catalog.json", "packages/arthur-1.2.0.vgp");
        let sha256 = hex::encode(sha2::Sha256::digest(&archive));
        let own = client(&app, true);
        let told = |sha256: &str| {
            json!({
                "id": "arthur:arthur", "package": { "id": "arthur", "version": "1.2.0" },
                "archive": { "url": url, "sha256": sha256 }
            })
        };
        let wrong = "0".repeat(64);
        let (status, body) = over_link(&app, "POST", "/home/v1/character", &own, true, told(&wrong)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(
            body["message"],
            "Не поставлено: архив «arthur» от центра не совпал по sha256 — он подменён или испорчен."
        );
        assert!(app.plugins.installed("arthur").is_none(), "nothing installed");
        assert!(!app.config.plugins.packages_dir.join("arthur").join("1.2.0").exists());

        let (status, body) = over_link(&app, "POST", "/home/v1/character", &own, true, told(&sha256)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let installed = app.plugins.installed("arthur").expect("installed from the centre's archive");
        assert_eq!(installed.manifest.version.to_string(), "1.2.0");
        assert_eq!(app.personas.chosen().unwrap().preset_id.as_deref(), Some("arthur:arthur"));
    }

    /// Item 41: the home's own centre names a character it installed from its own source with a voice of no known
    /// origin — this Турбо takes it; another home is refused before anything, a caller other than the home's link gets
    /// the catalog's rule, and a package that is not a character is never taken this way.
    #[tokio::test]
    async fn the_centres_character_with_a_voice_of_unknown_origin_installs_only_for_its_own_home() {
        use crate::plugins::install::tests::{character_manifest, vgp};
        use sha2::Digest;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_str().unwrap();
        let app = computer(&[("DATA_DIR", data), ("PLUGINS_BUNDLED", "false"), ("PACKAGES_FEED_URL", "off")]);
        let site = crate::plugins::catalog::tests::site().await;
        let mut manifest = character_manifest("arthur", "1.2.0");
        manifest["contributes"]["characters"][0]["voiceOrigin"] = json!("unknown");
        let archive = vgp(&manifest);
        let mut theme = character_manifest("arthur", "1.3.0");
        theme["kind"] = json!("theme");
        let not_character = vgp(&theme);
        site.publish(&[
            (crate::plugins::catalog::tests::arthur("1.2.0"), archive.clone()),
            (crate::plugins::catalog::tests::arthur("1.3.0"), not_character.clone()),
        ]);
        let url = |version: &str| site.url.replace("catalog.json", &format!("packages/arthur-{version}.vgp"));
        let sha256 = hex::encode(sha2::Sha256::digest(&archive));
        let told = json!({
            "id": "arthur:arthur", "package": { "id": "arthur", "version": "1.2.0" },
            "archive": { "url": url("1.2.0"), "sha256": sha256 }
        });

        let other = client(&app, false);
        let (status, body) = over_link(&app, "POST", "/home/v1/character", &other, false, told.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(app.plugins.installed("arthur").is_none(), "another home installs nothing");

        // Not the home's link (a person at this computer, a script): the catalog's rule stands.
        let refused = app.catalog.install_from_centre("arthur", "1.2.0", &url("1.2.0"), &sha256, false, None).await;
        let refused = refused.expect_err("an unknown voice without the home's word");
        assert!(refused.message.contains("voiceOrigin unknown"), "{}", refused.message);
        assert!(app.plugins.installed("arthur").is_none());
        assert!(!app.config.plugins.packages_dir.join("arthur").join("1.2.0").exists(), "nothing left on the disk");

        let own = client(&app, true);
        let (status, body) = over_link(&app, "POST", "/home/v1/character", &own, true, told).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let installed = app.plugins.installed("arthur").expect("installed from the centre's archive");
        assert_eq!(installed.manifest.version.to_string(), "1.2.0");
        assert_eq!(app.personas.chosen().unwrap().preset_id.as_deref(), Some("arthur:arthur"));

        // Not a character: never taken from the centre's archive, even for the own home.
        let theme = json!({
            "id": "arthur:arthur", "package": { "id": "arthur", "version": "1.3.0" },
            "archive": { "url": url("1.3.0"), "sha256": hex::encode(sha2::Sha256::digest(&not_character)) }
        });
        let (status, body) = over_link(&app, "POST", "/home/v1/character", &own, true, theme).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(
            app.plugins.installed("arthur").unwrap().manifest.version.to_string(),
            "1.2.0",
            "the character stays"
        );
    }

    /// Item 41б: the station calls this computer by address, not over the link — from the home network, with the key
    /// made for it (`home.models`). That is the home too: the character with a voice of unknown origin is taken; a key
    /// without the help's right never gets that far.
    #[tokio::test]
    async fn the_station_by_address_with_the_homes_key_is_the_home_for_the_centres_character() {
        use crate::plugins::install::tests::{character_manifest, vgp};
        use sha2::Digest;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_str().unwrap();
        let app = computer(&[("DATA_DIR", data), ("PLUGINS_BUNDLED", "false"), ("PACKAGES_FEED_URL", "off")]);
        let site = crate::plugins::catalog::tests::site().await;
        let mut manifest = character_manifest("arthur", "1.2.0");
        manifest["contributes"]["characters"][0]["voiceOrigin"] = json!("unknown");
        let archive = vgp(&manifest);
        site.publish(&[(crate::plugins::catalog::tests::arthur("1.2.0"), archive.clone())]);
        let told = json!({
            "id": "arthur:arthur", "package": { "id": "arthur", "version": "1.2.0" },
            "archive": {
                "url": site.url.replace("catalog.json", "packages/arthur-1.2.0.vgp"),
                "sha256": hex::encode(sha2::Sha256::digest(&archive))
            }
        });

        let stranger = key(&app, &["station.view"]);
        let (status, body) = send(&app, "POST", "/home/v1/character", Some(&stranger), told.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(app.plugins.installed("arthur").is_none(), "a key without the help's right installs nothing");

        let station = key(&app, &[HOME_MODELS]);
        let (status, body) = send(&app, "POST", "/home/v1/character", Some(&station), told).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let installed = app.plugins.installed("arthur").expect("installed from the centre's archive");
        assert_eq!(installed.manifest.version.to_string(), "1.2.0");
        assert_eq!(app.personas.chosen().unwrap().preset_id.as_deref(), Some("arthur:arthur"));
    }

    #[tokio::test]
    async fn only_the_computers_own_home_installs_an_apps_code_here() {
        let app = computer(&[]);
        let install = json!({ "id": "feed", "version": "0.4.0" });
        let other = client(&app, false);
        let (status, body) = over_link(&app, "POST", "/home/v1/plugins/install", &other, false, install.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        let own = client(&app, true);
        let (status, body) = over_link(&app, "POST", "/home/v1/plugins/install", &own, true, install).await;
        // No catalog in a test: the install itself says so — the home was let through.
        assert_ne!(status, StatusCode::FORBIDDEN, "{body}");
        let (status, _) = over_link(&app, "POST", "/home/v1/plugins/install", &own, true, json!({})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// The clean-path check of 19.11: the station asked the Mac for YouTube's code by id, YouTube came from a demo feed
    /// only the station reads, and the Mac answered «not in the catalog» — so the app never ran there. The centre now
    /// names the feed: the Турбо installs and switches on what its own signed main feed pinned, and nothing else.
    #[tokio::test]
    async fn the_centre_names_the_feed_of_a_faces_code_and_the_turbo_installs_only_what_it_pinned() {
        use crate::plugins::catalog::tests::{arthur, plugin_vgp, site};
        let keys = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let (main, demo) = (site().await, site().await);
        main.publish(&[]);
        let app_of = |id: &str| {
            let manifest = json!({
                "schema": 1, "kind": "plugin", "id": id, "version": "1.0.0", "title": id, "api": "^2",
                "runs": ["pc"], "permissions": [],
                "contributes": { "intents": [{ "phrases": [format!("включи {id}")], "handler": "play" }] }
            });
            let mut entry = arthur("1.0.0");
            (entry["id"], entry["kind"], entry["runs"]) = (json!(id), json!("plugin"), json!(["pc"]));
            entry["budget"] = json!({ "memoryMb": 4 });
            (entry, plugin_vgp(&manifest, "export function play() { return 'играю'; }"))
        };
        demo.publish(&[app_of("tube"), app_of("moon")]);
        let pinned: Value = serde_json::from_str(&demo.feed.lock().unwrap()).unwrap();
        let mut feed: Value = serde_json::from_str(&main.feed.lock().unwrap()).unwrap();
        feed["recommends"] = json!([{ "id": "tube", "sha256": pinned["packages"][0]["sha256"] }]);
        let text = feed.to_string();
        let signature =
            minisign::sign(Some(&keys.pk), &keys.sk, std::io::Cursor::new(text.as_bytes()), None, None).unwrap();
        *main.feed.lock().unwrap() = text;
        main.files.lock().unwrap().insert("catalog.json.minisig".into(), signature.to_string().into_bytes());

        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_str().unwrap();
        let key = keys.pk.to_base64();
        let app = computer(&[
            ("DATA_DIR", data),
            ("PLUGINS_BUNDLED", "false"),
            ("PACKAGES_FEED_URL", main.url.as_str()),
            ("PACKAGES_PUBKEY", key.as_str()),
        ]);
        let own = client(&app, true);
        let install = |id: &str, feed: Option<&str>| {
            let mut body = json!({ "id": id, "version": "1.0.0" });
            if let Some(feed) = feed {
                body["feed"] = json!(feed);
            }
            body
        };
        // As before: by id alone, this computer's catalog has no such package.
        let (status, body) =
            over_link(&app, "POST", "/home/v1/plugins/install", &own, true, install("tube", None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        // With the centre's feed: installed and switched on — the station's calls find it now.
        let (status, body) =
            over_link(&app, "POST", "/home/v1/plugins/install", &own, true, install("tube", Some(&demo.url))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(app.plugins.is_active("tube"), "{body}");
        // Not pinned: refused, nothing installed.
        let (status, body) =
            over_link(&app, "POST", "/home/v1/plugins/install", &own, true, install("moon", Some(&demo.url))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(app.plugins.installed("moon").is_none());

        // The code here already at that version or newer, switched off (20.11): switched on as it is — before, the
        // catalog was asked for the older one, answered «Новее 1.0.0 в каталоге нет», and it stayed off.
        app.plugins.disable("tube", None).await.unwrap();
        let mut older = install("tube", Some(&demo.url));
        older["version"] = json!("0.9.0");
        let (status, body) = over_link(&app, "POST", "/home/v1/plugins/install", &own, true, older).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(app.plugins.is_active("tube"), "{body}");
        assert_eq!(body["version"], json!("1.0.0"), "its own code, switched on");
    }

    #[tokio::test]
    async fn another_home_hears_nothing_of_the_owners_own_voice() {
        let speech = json!({ "input": "Привет" });
        // The owner's voice at a cloud address: never another home's.
        let app = computer(&[
            ("TTS_OWN_URL", "https://tts.example.invalid/v1"),
            ("TTS_OWN_MODEL", "piper"),
            ("TTS_OWN_VOICE", "ru_RU-irina-medium"),
        ]);
        assert!(!voice_for_others(&app));
        let other = client(&app, false);
        let (status, body) = over_link(&app, "GET", STATUS, &other, false, Value::Null).await;
        assert_eq!((status, voice_ready(&body)), (StatusCode::OK, Some(false)), "{body}");
        let (status, body) = over_link(&app, "POST", "/home/v1/tts/audio/speech", &other, false, speech.clone()).await;
        assert_eq!(
            (status, body["message"].as_str()),
            (StatusCode::FORBIDDEN, Some("Это Турбо не даёт этого вашему дому."))
        );
        // Its own home hears it.
        let own = client(&app, true);
        let (_, body) = over_link(&app, "GET", STATUS, &own, true, Value::Null).await;
        assert_eq!(voice_ready(&body), Some(true));
        let (status, _) = over_link(&app, "POST", "/home/v1/tts/audio/speech", &own, true, speech.clone()).await;
        assert_ne!(status, StatusCode::FORBIDDEN);
        // A voice of the voice lab's engine, even at home: the owner's own.
        let app = computer(&[
            ("TTS_OWN_URL", "http://127.0.0.1:9/v1"),
            ("TTS_OWN_MODEL", "omnivoice"),
            ("TTS_OWN_VOICE", "3f2a9c1e"),
        ]);
        assert!(!voice_for_others(&app));
        // A stock voice of this computer: served to another home too (the server here does not answer: 502, not 403).
        let app = computer(&[
            ("TTS_OWN_URL", "http://127.0.0.1:9/v1"),
            ("TTS_OWN_MODEL", "piper"),
            ("TTS_OWN_VOICE", "ru_RU-irina-medium"),
        ]);
        assert!(voice_for_others(&app));
        let other = client(&app, false);
        let (_, body) = over_link(&app, "GET", STATUS, &other, false, Value::Null).await;
        assert_eq!(voice_ready(&body), Some(true));
        let (status, _) = over_link(&app, "POST", "/home/v1/tts/audio/speech", &other, false, speech).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
    }

    /// The built-in helper's voice at a cloud's address (stage 6) over the link: its own home hears it only while the
    /// regulator lets the clouds out — «Только дома» keeps the text home for its own home too: the status says the
    /// voice not ready, and the voice refuses in the regulator's words, not the link's.
    #[tokio::test]
    async fn the_regulator_keeps_the_computers_cloud_voice_from_its_own_home_over_the_link_too() {
        let app = computer(&[
            ("TTS_OWN_URL", "https://tts.example.invalid/v1"),
            ("TTS_OWN_MODEL", "piper"),
            ("TTS_OWN_VOICE", "ru_RU-irina-medium"),
        ]);
        let own = client(&app, true);
        let (_, body) = over_link(&app, "GET", STATUS, &own, true, Value::Null).await;
        assert_eq!(voice_ready(&body), Some(true), "«Любые облака»");
        app.helpers.set_away(crate::helpers::Away::Home, None).unwrap();
        let (_, body) = over_link(&app, "GET", STATUS, &own, true, Value::Null).await;
        assert_eq!(voice_ready(&body), Some(false), "{body}");
        let speech = json!({ "input": "Привет" });
        let (status, body) = over_link(&app, "POST", "/home/v1/tts/audio/speech", &own, true, speech).await;
        assert_eq!(
            (status, body["message"].as_str()),
            (StatusCode::SERVICE_UNAVAILABLE, Some("Регулятор не пускает облако"))
        );
    }

    #[tokio::test]
    async fn every_route_names_what_it_needs() {
        let app = computer(&[("TTS_OWN_URL", "http://127.0.0.1:9/v1"), ("TTS_OWN_MODEL", "piper")]);
        let nothing = client(&app, false);
        app.clients.set_gives(&nothing, Vec::new(), None).unwrap();
        let routes = [
            ("POST", "/home/v1/asr/audio/transcriptions"),
            ("POST", "/home/v1/audio/transcriptions"),
            ("GET", "/home/v1/asr/models"),
            ("POST", "/home/v1/tts/audio/speech"),
            ("POST", "/home/v1/audio/speech"),
            ("POST", "/home/v1/llm/chat/completions"),
            ("POST", "/home/v1/chat/completions"),
            ("GET", "/home/v1/llm/models"),
            ("GET", "/home/v1/models"),
            ("GET", "/home/v1/plugins"),
            ("GET", "/home/v1/catalog"),
            ("GET", "/home/v1/catalog/file?url=https://packages.example.invalid/a.tgz"),
            ("POST", "/home/v1/plugins/weather/call"),
            ("GET", "/home/v1/messenger/contacts"),
            ("POST", "/home/v1/messenger/send"),
            ("POST", "/home/v1/history/have"),
            ("POST", "/home/v1/history/objects"),
            ("POST", "/home/v1/history/lines"),
            ("POST", "/home/v1/history/fetch"),
        ];
        for (method, uri) in routes {
            let (status, body) = over_link(&app, method, uri, &nothing, false, json!({})).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {body}");
            assert_eq!(body["message"], "Это Турбо не даёт этого вашему дому.", "{method} {uri}");
        }
        // The status only says what it gives: nothing.
        let (status, body) = over_link(&app, "GET", STATUS, &nothing, false, Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        let contract = HelperStatus::read(&body).unwrap().contract;
        assert!(contract.abilities.is_empty() && contract.link == Some(crate::helpers::link::VERSION), "{body}");
        // An unknown client is given nothing either; and a link caller is outside, never this computer.
        let (status, _) = over_link(&app, "GET", "/home/v1/plugins", "link:000000000000", false, Value::Null).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
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
