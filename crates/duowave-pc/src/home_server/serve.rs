//! `/home/v1` on the computer: its own servers for the gateway on the station, OpenAI-compatible, only from the home
//! network and only with a key that has the right `home.models`. Its recognition and voice servers listen on
//! 127.0.0.1 and stay closed to the network: this gateway is the one door to them. What it has to give is the built-in
//! helper's (src/helpers/builtin.rs) — the same parts the computer takes for itself.
//!
//! - `/home/v1/status` — what is ready: recognition, our voice, a model of its own; and in `contract`, what this
//!   computer gives as a helper, by the helper's contract (core/helpers/contract.rs: `{version, helper, abilities}`);
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
use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::characters::CharactersService;
use crate::config::Config;
use crate::connectors::settings::ConnectorSettings;
use crate::core::identity::home::Identity;
use crate::core::identity::permissions::HOME_MODELS;
use crate::helpers::builtin::Hearing;
use crate::helpers::clients::Clients;
use crate::helpers::contract::{Ability, Contract};
use crate::helpers::OwnParts;
use crate::operations::networks::is_local_url;
use crate::platform::http::{ApiError, Caller, Uses, Zone};
use crate::plugins::{PackageCatalog, PluginHost};
use crate::skills::own::calls_skill::MessengerAccess;
use crate::speech::asr::local::supervisor::LocalAsr;
use crate::speech::VoiceService;

/// A minute of speech as WAV is ~2 MB; a request with history for the model, less.
const BODY_LIMIT: usize = 16 * 1024 * 1024;

pub fn router<S: crate::home_server::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/home/v1/status", get(status))
        // The voice lab's imprint.py, its recognition inside this program: loopback, by the gateway's key.
        .route(
            "/api/v1/voice-lab/asr/{key}/v1/audio/transcriptions",
            post(imprint_asr).layer(axum::extract::DefaultBodyLimit::max(IMPRINT_ASR_LIMIT)),
        )
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
        .route("/home/v1/catalog/feed", get(catalog_feed))
        .route("/home/v1/plugins/{id}/call", post(plugin_call))
        .route("/home/v1/messenger/contacts", get(messenger_contacts))
        .route("/home/v1/messenger/send", post(messenger_send))
        .route("/home/v1/history/have", post(history_have))
        .route("/home/v1/history/objects", post(history_objects))
        .route("/home/v1/history/lines", post(history_lines))
        .route("/home/v1/history/fetch", post(history_fetch))
        .merge(super::devices::router())
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

/// The door of `/home/v1`: the homes this computer serves — what it gives each and what it gave today — and its voice
/// (another home hears only a stock one). The gateway makes it (src/links/http.rs).
#[derive(Clone)]
pub struct Door {
    pub clients: Arc<Clients>,
    pub voice: Arc<crate::speech::VoiceService>,
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
pub(crate) fn allowed(door: &Door, caller: &Caller, needs: Needs) -> Result<(), ApiError> {
    if let Some(link) = caller.link() {
        caller.permitted(HOME_MODELS)?;
        if let Needs::Ability(ability) = needs {
            let given = door.clients.gives(&link.client, ability)
                && (ability != Ability::Voice || link.own_home || voice_for_others(&door.voice));
            if !given {
                return Err(ApiError::forbidden(crate::tr!("Это Турбо не даёт этого вашему дому.")));
            }
        }
    } else {
        caller.zone_at_least(Zone::Home)?;
        caller.permitted(HOME_MODELS)?;
    }
    door.clients.saw(caller);
    Ok(())
}

/// Whether another home may hear this computer's voice now (D31): only a stock engine of this computer — Silero or
/// Piper at a home address —, never «Мой голос», a voice lab's voice or a cloud voice.
pub fn voice_for_others(voice: &Arc<VoiceService>) -> bool {
    let Some(own) = voice.own_speech() else { return false };
    let stock = voice.own_choice().is_some_and(|pair| matches!(pair.model.as_str(), "silero" | "piper"));
    stock && is_local_url(Some(&own.base_url()))
}

/// One answer given: today's «Что даю колонке» and the client's own count.
fn served(door: &Door, caller: &Caller, which: usize, ability: Ability, started: Instant) {
    count(which, started);
    door.clients.served(caller, ability, started);
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
    Uses(door): Uses<Door>,
    Uses(catalog): Uses<Arc<PackageCatalog>>,
    caller: Caller,
    axum::extract::Query(query): axum::extract::Query<CatalogQuery>,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::Catalog))?;
    let _ = catalog.feed(query.fresh.is_some()).await;
    let copy =
        catalog.export_for_station().ok_or_else(|| ApiError::not_found(crate::tr!("Копии каталога здесь нет.")))?;
    Ok(axum::Json(copy).into_response())
}

#[derive(serde::Deserialize)]
struct FileQuery {
    url: String,
}

/// One extra feed for the station that cannot read it itself (a tester's Station Mini 2, 22.11: DuoWave Store and the
/// demo feed closed to it, open to the computer beside it): this computer's copy of it, only of a feed it reads too
/// (`PackageCatalog::feed_for_station`); none — 404.
async fn catalog_feed(
    Uses(door): Uses<Door>,
    Uses(catalog): Uses<Arc<PackageCatalog>>,
    caller: Caller,
    axum::extract::Query(query): axum::extract::Query<FileQuery>,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::Catalog))?;
    let copy =
        catalog.feed_for_station(&query.url).ok_or_else(|| ApiError::not_found(crate::tr!("Этой ленты здесь нет.")))?;
    Ok(axum::Json(copy).into_response())
}

/// A file of the catalog for the station: an archive, only from the origin of one of the catalog's feeds — the main
/// one, an extra one it names or an own source; the station checks its hash.
async fn catalog_file(
    Uses(door): Uses<Door>,
    Uses(catalog): Uses<Arc<PackageCatalog>>,
    caller: Caller,
    axum::extract::Query(query): axum::extract::Query<FileQuery>,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::Catalog))?;
    let bytes = catalog.fetch_for_station(&query.url).await?;
    Ok(bytes.into_response())
}

/// The packages on here (docs/plans/home-and-devices.md, stage 5а): each with its version, title, kind, engine and
/// state — the home's one list «Установлено» shows them with this Турбо's name as their place. `ready` — on and
/// running (what the station may hand over).
async fn plugins(
    Uses(door): Uses<Door>,
    Uses(plugins): Uses<Arc<PluginHost>>,
    caller: Caller,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::Apps))?;
    let view = plugins.view().await;
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
fn own_home_only(door: &Door, caller: &Caller) -> Result<(), ApiError> {
    allowed(door, caller, Needs::Ability(Ability::Voice))?;
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
/// pick. Only the computer's own home: another home gets a stock voice only. A character settles in every part it
/// lives in — the voice lab, the characters, the packages, the catalog, the models — each an extractor of its own.
#[allow(clippy::too_many_arguments)]
async fn character(
    Uses(door): Uses<Door>,
    Uses(voice_lab): Uses<Arc<crate::voice_lab::VoiceLabService>>,
    Uses(characters): Uses<Arc<CharactersService>>,
    Uses(plugins): Uses<Arc<PluginHost>>,
    Uses(catalog): Uses<Arc<PackageCatalog>>,
    Uses(models): Uses<Arc<crate::models::ModelsService>>,
    caller: Caller,
    body: Bytes,
) -> Result<Response, ApiError> {
    own_home_only(&door, &caller)?;
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
        super::voice_plan::check_room(&voice_lab, engine).await?;
        characters.name_engine(&id, engine)?;
    }
    if let Some(package) = request.get("package").and_then(|package| package.get("id")).and_then(Value::as_str) {
        let version = request.get("package").and_then(|package| package.get("version")).and_then(Value::as_str);
        let here = plugins.installed(package).map(|installed| installed.manifest.version.to_string());
        if here.is_none() || (version.is_some() && here.as_deref() != version) {
            let archive = request.get("archive").and_then(|archive| {
                Some((archive.get("url")?.as_str()?.to_string(), archive.get("sha256")?.as_str()?.to_string()))
            });
            match (archive, version) {
                (Some((url, sha256)), Some(version)) => {
                    // The home's own centre vouches for the package's voice (items 41, 41б).
                    let vouched = speaks_for_own_home(&caller);
                    catalog.install_from_centre(package, version, &url, &sha256, vouched, None).await?;
                }
                (Some(_), None) => return Err(ApiError::bad_request("archive: package.version".to_string())),
                (None, _) => {
                    catalog.install(package, version, here.is_some(), None).await?;
                }
            }
        }
    }
    // The owner's own voice put on top of the character's (test 19.11, remark 11) stays when the centre names the same
    // character again — on every persona save and every reconnect of this computer it does. Another character comes
    // with its own voice, as decided on 17.11; «Вернуть голос персонажа» in «Голос» takes the character's back.
    let on_top = models.chosen_voice().is_some_and(|chosen| own_on_top(&chosen.model_id));
    let same = characters.active_id().as_deref() == Some(id.as_str());
    let body = crate::characters::ChooseBody { voice: Some(!(on_top && same)), install: true, ..Default::default() };
    let answer = characters.choose(&id, body, true, None).await?;
    Ok(axum::Json(json!({ "character": id, "voice": answer.voice })).into_response())
}

/// An own voice of the voice lab that is no character's (`own:<id>`, not `own:c-…`): the owner's own recording.
pub fn own_on_top(model_id: &str) -> bool {
    model_id.strip_prefix(crate::models::models_service::OWN_PREFIX).is_some_and(|id| !id.starts_with("c-"))
}

/// How the character's voice would stand on this computer (clean-path check 18.11, item 40): the plan the centre's
/// panel shows before and while the voice comes (voice_plan.rs). Only the computer's own home.
async fn character_plan(
    Uses(door): Uses<Door>,
    Uses(characters): Uses<Arc<CharactersService>>,
    Uses(voice_lab): Uses<Arc<crate::voice_lab::VoiceLabService>>,
    Uses(models): Uses<Arc<crate::models::ModelsService>>,
    Uses(identity): Uses<Arc<Identity>>,
    caller: Caller,
    body: Bytes,
) -> Result<Response, ApiError> {
    own_home_only(&door, &caller)?;
    let request: super::voice_plan::PlanRequest = serde_json::from_slice(&body)
        .map_err(|_| ApiError::bad_request("JSON: {id, package: {id, version}, voices, reference}".to_string()))?;
    if request.id.is_empty() {
        return Err(ApiError::bad_request("id".to_string()));
    }
    Ok(axum::Json(super::voice_plan::plan(&characters, &voice_lab, &models, &identity, &request).await).into_response())
}

/// The voice waiting on this computer (voice_plan.rs, `waiting`): `{pending: … | null, computer}`. Only its own home.
async fn voice_pending(
    Uses(door): Uses<Door>,
    Uses(models): Uses<Arc<crate::models::ModelsService>>,
    Uses(characters): Uses<Arc<CharactersService>>,
    Uses(voice_lab): Uses<Arc<crate::voice_lab::VoiceLabService>>,
    Uses(identity): Uses<Arc<Identity>>,
    caller: Caller,
) -> Result<Response, ApiError> {
    own_home_only(&door, &caller)?;
    Ok(axum::Json(waiting_answer(&models, &characters, &voice_lab, &identity).await).into_response())
}

/// The owner's answer to the waiting voice, from the home's centre: `{"answer": "anyway" | "retry" | "cancel"}` →
/// what waits now. Only its own home.
async fn voice_pending_answer(
    Uses(door): Uses<Door>,
    Uses(models): Uses<Arc<crate::models::ModelsService>>,
    Uses(characters): Uses<Arc<CharactersService>>,
    Uses(voice_lab): Uses<Arc<crate::voice_lab::VoiceLabService>>,
    Uses(identity): Uses<Arc<Identity>>,
    caller: Caller,
    body: Bytes,
) -> Result<Response, ApiError> {
    own_home_only(&door, &caller)?;
    let request: Value = serde_json::from_slice(&body)
        .map_err(|_| ApiError::bad_request(r#"JSON: {"answer": "anyway" | "retry" | "cancel"}"#.to_string()))?;
    let answer = request.get("answer").and_then(Value::as_str).unwrap_or_default();
    super::voice_plan::answer(&models, answer, caller.actor()).await?;
    Ok(axum::Json(waiting_answer(&models, &characters, &voice_lab, &identity).await).into_response())
}

async fn waiting_answer(
    models: &Arc<crate::models::ModelsService>,
    characters: &Arc<CharactersService>,
    voice_lab: &Arc<crate::voice_lab::VoiceLabService>,
    identity: &Arc<Identity>,
) -> Value {
    json!({
        "pending": super::voice_plan::waiting(models, characters, voice_lab).await,
        "computer": identity.gateway().map(|me| me.name).unwrap_or_default(),
    })
}

/// The home's Telegram (docs/plans/home-and-devices.md, stage 2): the centre keeps the token and the contacts, this
/// Турбо runs the bot with them (`{enabled, contacts, alertChat, token}`). Only the computer's own home.
async fn messenger_bot(
    Uses(door): Uses<Door>,
    Uses(connector_settings): Uses<Arc<ConnectorSettings>>,
    caller: Caller,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::Messages))?;
    if caller.link().is_some_and(|link| !link.own_home) {
        return Err(ApiError::forbidden(crate::tr!("Бот Telegram этого компьютера настраивает только его свой дом.")));
    }
    let request: Value = serde_json::from_slice(&body)
        .map_err(|_| ApiError::bad_request("JSON: {enabled, contacts, alertChat, token}".to_string()))?;
    let setting: crate::settings::definitions::TelegramSetting =
        serde_json::from_value(request.clone()).map_err(|error| ApiError::bad_request(format!("Telegram: {error}")))?;
    let token = request.get("token").and_then(Value::as_str);
    connector_settings.take_home_telegram(&setting, token)?;
    Ok(axum::Json(json!({ "enabled": setting.enabled, "contacts": setting.contacts.len() })).into_response())
}

/// An app's code for the home (docs/plans/home-and-devices.md, stage 5б): the centre installed its face and asks this
/// Турбо for the code — by id and version from this computer's own signed feed (its catalog checks the signature and
/// the archive's hash), installed or updated to that version and switched on. The centre checked the person's right;
/// only the computer's own home asks this, another home never installs anything here.
async fn plugin_install(
    Uses(door): Uses<Door>,
    Uses(plugins): Uses<Arc<PluginHost>>,
    Uses(catalog): Uses<Arc<PackageCatalog>>,
    caller: Caller,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::Apps))?;
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
    let here = plugins.installed(id).map(|installed| installed.manifest.version.to_string());
    // Here already at that version or newer — switched off, say: switched on as it is. Asking the catalog for an older
    // one answered «Новее X в каталоге нет» and left it off (before 20.11).
    let newer_here = matches!((here.as_deref(), version), (Some(here), Some(wanted)) if crate::home::apps::newer_or_same(here, wanted));
    if !newer_here {
        match catalog.install(id, version, here.is_some(), None).await {
            Ok(_) => {}
            // Not in this computer's catalog: the feed the centre has it from (clean-path check 19.11, YouTube from a
            // demo feed the station's admin added), taken only as this catalog pinned it.
            Err(error) if error.status == StatusCode::NOT_FOUND => {
                let Some(feed) = request.get("feed").and_then(Value::as_str) else { return Err(error) };
                catalog.install_pinned_from(id, version, feed, None).await?;
            }
            Err(error) => return Err(error),
        }
    }
    let view = plugins.enable(id, None).await?;
    Ok(axum::Json(json!({ "id": id, "version": view.version, "state": view.state })).into_response())
}

/// A call of a plugin of this computer for the station: its function, its input, how long it may take.
async fn plugin_call(
    Uses(door): Uses<Door>,
    Uses(plugins): Uses<Arc<PluginHost>>,
    caller: Caller,
    axum::extract::Path(id): axum::extract::Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::Apps))?;
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
    if !plugins.is_active(&id) {
        return Err(ApiError::conflict(crate::tr!("Приложение {id} здесь не включено.", id = id)));
    }
    let answer = plugins
        .call(&id, function, input, kind)
        .await
        .map_err(|error| ApiError::bad_gateway(crate::tr!("Приложение не ответило: {error}.", error = error)))?;
    Ok(axum::Json(json!({ "result": answer })).into_response())
}

fn history_of(
    history: &Option<Arc<crate::plugins::history::History>>,
) -> Result<Arc<crate::plugins::history::History>, ApiError> {
    history.clone().ok_or_else(|| ApiError::conflict(crate::tr!("Истории приложений на этом компьютере нет.")))
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
async fn history_have(
    Uses(door): Uses<Door>,
    Uses(history_service): Uses<Option<Arc<crate::plugins::history::History>>>,
    caller: Caller,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::History))?;
    let history = history_of(&history_service)?;
    Ok(axum::Json(json!({ "missing": history.missing(&ids_of(&body)?) })).into_response())
}

/// The station's history objects: each kept only when its name is its hash.
async fn history_objects(
    Uses(door): Uses<Door>,
    Uses(history_service): Uses<Option<Arc<crate::plugins::history::History>>>,
    caller: Caller,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::History))?;
    let history = history_of(&history_service)?;
    let request: Value = serde_json::from_slice(&body).map_err(|_| ApiError::bad_request("JSON: {objects}"))?;
    let objects = request.get("objects").and_then(Value::as_array).cloned().unwrap_or_default();
    let stored = tokio::task::spawn_blocking(move || history.receive(&objects)).await.map_err(ApiError::internal)?;
    Ok(axum::Json(json!({ "stored": stored })).into_response())
}

/// The station's lines and marks: its marks apart, its records into the apps' lines here.
async fn history_lines(
    Uses(door): Uses<Door>,
    Uses(history_service): Uses<Option<Arc<crate::plugins::history::History>>>,
    caller: Caller,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::History))?;
    let history = history_of(&history_service)?;
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
async fn history_fetch(
    Uses(door): Uses<Door>,
    Uses(history_service): Uses<Option<Arc<crate::plugins::history::History>>>,
    caller: Caller,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::History))?;
    let history = history_of(&history_service)?;
    Ok(axum::Json(json!({ "objects": history.give(&ids_of(&body)?) })).into_response())
}

pub fn no_bot() -> String {
    crate::tr!("На этом компьютере не настроен бот Telegram: «Приложения» → «Подключения» → Telegram.")
}
pub fn send_shape() -> String {
    crate::tr!("JSON: {contact, text} — кому и что передать")
}

/// This computer's own Telegram bot (src/calls; links/http.rs gives it), not a helper's: the station's «передай …»
/// reaches it.
#[derive(Clone)]
pub struct OwnBot(pub Arc<dyn MessengerAccess>);

/// The names of this computer's Telegram contacts, for the station's «передай …»: no chats.
async fn messenger_contacts(
    Uses(door): Uses<Door>,
    Uses(OwnBot(bot)): Uses<OwnBot>,
    caller: Caller,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::Messages))?;
    if !bot.enabled() {
        return Err(ApiError::unavailable(no_bot()));
    }
    Ok(axum::Json(json!({ "contacts": bot.contact_names() })).into_response())
}

/// The station's «передай маме, что …»: a text to a contact of this computer's bot.
async fn messenger_send(
    Uses(door): Uses<Door>,
    Uses(OwnBot(bot)): Uses<OwnBot>,
    caller: Caller,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::Messages))?;
    let request: Value = serde_json::from_slice(&body).map_err(|_| ApiError::bad_request(send_shape()))?;
    let field = |name: &str| request.get(name).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
    let (contact, text) = (field("contact"), field("text"));
    if contact.is_empty() || text.is_empty() {
        return Err(ApiError::bad_request(send_shape()));
    }
    let started = Instant::now();
    let name = pass_on(&*bot, &contact, &text).await?;
    served(&door, &caller, 3, Ability::Messages, started);
    tracing::info!("home server: a message from the station passed on to «{name}»");
    Ok(axum::Json(json!({ "sent": true, "contact": name })).into_response())
}

/// The bot's part of a send: the contact by the name the station gives, then Telegram. The contact's name.
pub async fn pass_on(bot: &dyn MessengerAccess, contact: &str, text: &str) -> Result<String, ApiError> {
    if !bot.enabled() {
        return Err(ApiError::unavailable(no_bot()));
    }
    let found = bot.contact(contact).ok_or_else(|| {
        ApiError::not_found(crate::tr!(
            "У бота Telegram на этом компьютере нет контакта «{contact}».",
            contact = contact
        ))
    })?;
    bot.send_text(found.chat_id, text.to_string(), None).await.map_err(|error| {
        ApiError::bad_gateway(crate::tr!("Telegram не принял сообщение: {error}.", error = format!("{error:#}")))
    })?;
    Ok(found.name)
}

/// What hears for the station: the built-in helper's hearing (helpers/builtin.rs) — this computer's own engine, or its
/// recognition server at home.
fn hearing(builtin: &Arc<crate::helpers::builtin::BuiltinHelper>) -> Option<Hearing> {
    builtin.hearing().cloned()
}

/// The parts of a multipart/form-data body by name (what an OpenAI-compatible client sends: `file`, `model`,
/// `language`…). Only as much of the format as such a client uses.
pub fn form_parts(headers: &HeaderMap, body: &[u8]) -> Vec<(String, Vec<u8>)> {
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
fn own_model(builtin: &Arc<crate::helpers::builtin::BuiltinHelper>) -> Option<(String, Option<String>)> {
    builtin.model_route()
}

/// The language of a Piper voice by its id: «piper/ru_RU-irina-medium» — «ru». Other voices say none. The built-in
/// helper's contract says it of its voice.
pub fn piper_language(pair: &str) -> Option<String> {
    let voice = pair.rsplit('/').next()?;
    let [locale, _name, _quality] = voice.split('-').collect::<Vec<_>>()[..] else { return None };
    let (language, region) = locale.split_once('_')?;
    let letters = |part: &str, upper: bool| {
        (2..=3).contains(&part.len())
            && part.chars().all(|c| if upper { c.is_ascii_uppercase() } else { c.is_ascii_lowercase() })
    };
    (letters(language, false) && letters(region, true)).then(|| language.to_string())
}

/// What this computer serves now as a helper — the contract of `/home/v1/status` (core/helpers/contract.rs): the built-in
/// helper's (helpers/builtin.rs), one answer for the gateways it helps and for itself.
pub(crate) fn contract(builtin: &Arc<crate::helpers::builtin::BuiltinHelper>) -> Contract {
    builtin.contract()
}

/// What a home over the link is given (stage 7): its abilities, and whether its home key proved it this computer's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Given {
    pub abilities: crate::helpers::contract::Abilities,
    pub own_home: bool,
}

/// What is ready here, for the station: the fields a station of 0.11 and before reads, and the contract they are
/// worked out of — one look at the parts. The contract says this computer speaks the link while it helps; a home
/// over the link sees only what it is given, and another home's voice ready only when it is a stock one.
pub fn status_of(
    builtin: &Arc<crate::helpers::builtin::BuiltinHelper>,
    link: &Arc<crate::helpers::link_serve::LinkServer>,
    voice_service: &Arc<VoiceService>,
    given: Option<Given>,
) -> Value {
    let mut contract = contract(builtin);
    if link.helps() {
        contract.link = Some(crate::helpers::link::VERSION);
    }
    if let Some(given) = &given {
        contract.abilities.retain(|state| given.abilities.has(state.id));
        if !given.own_home && !voice_for_others(voice_service) {
            for state in contract.abilities.iter_mut().filter(|state| state.id == Ability::Voice) {
                state.ready = false;
            }
        }
    }
    // A home over the link reaches this computer from outside: its things at hand are not that home's now (owner
    // 20.11, 6); a home at the home network's address does.
    for state in contract.abilities.iter_mut().filter(|state| state.id == Ability::Devices) {
        state.details.insert("home".into(), json!(given.is_none()));
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

async fn status(
    Uses(door): Uses<Door>,
    Uses(clients): Uses<Arc<crate::helpers::clients::Clients>>,
    Uses(builtin): Uses<Arc<crate::helpers::builtin::BuiltinHelper>>,
    Uses(link_service): Uses<Arc<crate::helpers::link_serve::LinkServer>>,
    Uses(voice): Uses<Arc<VoiceService>>,
    caller: Caller,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Status)?;
    let given = caller.link().map(|link| Given { abilities: clients.given(&link.client), own_home: link.own_home });
    Ok(axum::Json(status_of(&builtin, &link_service, &voice, given)).into_response())
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
    Uses(door): Uses<Door>,
    Uses(builtin): Uses<Arc<crate::helpers::builtin::BuiltinHelper>>,
    Uses(config): Uses<Arc<Config>>,
    caller: Caller,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::Hearing))?;
    let hearing = hearing(&builtin)
        .ok_or_else(|| ApiError::unavailable(crate::tr!("На этом компьютере нет своего распознавания.")))?;
    let started = Instant::now();
    let (base, key) = match hearing {
        Hearing::Server(base, key) => (base, key),
        Hearing::Local(local) => {
            let answer = recognize_here(&local, &headers, &body).await?;
            served(&door, &caller, 0, Ability::Hearing, started);
            return Ok(answer);
        }
    };
    let wait = Duration::from_millis(config.speech.asr.timeout_ms.max(5000));
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
        served(&door, &caller, 0, Ability::Hearing, started);
    }
    Ok(answer)
}

/// A reference clip for the words: tens of seconds of 16 kHz mono WAV are a megabyte or two.
const IMPRINT_ASR_LIMIT: usize = 16 * 1024 * 1024;

/// The words of a take for the voice lab's imprint.py (voice_lab/service.rs `imprint_asr_url`) when recognition runs
/// inside this program
/// (ASR_PROVIDER=local, the desktop app): only from this computer and only with the gateway's key in the path — the
/// script sends no sign-in — and heard by the same engine as the home's recognition (home_server/serve.rs). Anything
/// else is not there.
pub async fn imprint_asr(
    Uses(voice_lab): Uses<Arc<crate::voice_lab::VoiceLabService>>,
    Uses(builtin): Uses<Arc<crate::helpers::builtin::BuiltinHelper>>,
    caller: Caller,
    axum::extract::Path(key): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    if caller.zone != Zone::Local || !voice_lab.asr_key_is(&key) {
        return Err(ApiError::not_found("Not Found"));
    }
    match builtin.hearing() {
        Some(crate::helpers::builtin::Hearing::Local(local)) => recognize_here(local, &headers, &body).await,
        _ => Err(ApiError::unavailable(crate::tr!("На этом компьютере нет своего распознавания."))),
    }
}

/// Recognition inside this program, as an OpenAI-compatible server answers it: the WAV out of the form, `{text}` back.
/// The home's `/home/v1/asr/audio/transcriptions` and the voice lab's loopback route for imprint.py (`imprint_asr`,
/// above) both hear so.
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

async fn asr_models(
    Uses(door): Uses<Door>,
    Uses(builtin): Uses<Arc<crate::helpers::builtin::BuiltinHelper>>,
    caller: Caller,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::Hearing))?;
    let (base, key) = match hearing(&builtin) {
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
async fn speak(
    Uses(door): Uses<Door>,
    Uses(voice): Uses<Arc<VoiceService>>,
    Uses(builtin): Uses<Arc<crate::helpers::builtin::BuiltinHelper>>,
    caller: Caller,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::Voice))?;
    let request: Value =
        serde_json::from_slice(&body).map_err(|_| ApiError::bad_request(crate::tr!("input: текст")))?;
    let text = request.get("input").and_then(Value::as_str).unwrap_or("").to_string();
    if text.trim().is_empty() {
        return Err(ApiError::bad_request(crate::tr!("input: текст")));
    }
    let own = voice
        .own_speech()
        .cloned()
        .ok_or_else(|| ApiError::unavailable(crate::tr!("На этом компьютере нет своего голоса.")))?;
    // Its voice at a cloud's address while the regulator keeps the clouds in: the text stays home (a station of 0.11
    // reads only `tts`).
    if builtin.voice_kept_in() {
        return Err(ApiError::unavailable(crate::tr!("Регулятор не пускает облако")));
    }
    let started = Instant::now();
    let spoken = own.synthesize(&text, &CancellationToken::new()).await.map_err(|error| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            crate::tr!("Голос этого компьютера: {error}", error = format!("{error:#}")),
        )
    })?;
    served(&door, &caller, 1, Ability::Voice, started);
    Ok(([(header::CONTENT_TYPE, "audio/ogg")], spoken.audio).into_response())
}

async fn chat(
    Uses(door): Uses<Door>,
    Uses(builtin): Uses<Arc<crate::helpers::builtin::BuiltinHelper>>,
    caller: Caller,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::Model))?;
    let (base, key) =
        own_model(&builtin).ok_or_else(|| ApiError::unavailable(crate::tr!("На этом компьютере нет своей модели.")))?;
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
        served(&door, &caller, 2, Ability::Model, started);
    }
    Ok(answer)
}

async fn llm_models(
    Uses(door): Uses<Door>,
    Uses(builtin): Uses<Arc<crate::helpers::builtin::BuiltinHelper>>,
    caller: Caller,
) -> Result<Response, ApiError> {
    allowed(&door, &caller, Needs::Ability(Ability::Model))?;
    let (base, key) =
        own_model(&builtin).ok_or_else(|| ApiError::unavailable(crate::tr!("На этом компьютере нет своей модели.")))?;
    forward(reqwest::Method::GET, format!("{base}/models"), key, None, None, Duration::from_secs(10)).await
}
