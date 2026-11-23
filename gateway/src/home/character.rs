//! The home's character and its voice (docs/plans/home-and-devices.md, stage 2): the voice is part of the character,
//! and the character is the home's — chosen at the centre, from any panel. The centre speaks through the Турбо that
//! gives the voice (`/home/v1/tts`); so whenever its character changes, and whenever that Турбо comes back, the centre
//! names the character to it (`POST /home/v1/character`, src/home_server/serve.rs): the Турбо installs the
//! character's package by id and version when it lacks it — from the archive the centre's signed feed names, checked
//! by its hash there, else from its own feed — and speaks with its voice from then on.
//! A character of the built-in ones (Алиса's positions) has no voice of its own: nothing is sent.
//!
//! The model of that voice (clean-path check 18.11, item 40): `GET /api/v1/home/voice?character=<id>` (or
//! `?package=<id>` before the package is installed here) asks the Турбо how each model would stand there
//! (`POST /home/v1/character/plan`); `PUT /api/v1/home/voice` keeps the owner's choice here (`home_voice_engines`, by
//! the character, or by its package before it is installed) and names it to the Турбо — now, and each time it comes
//! back.
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::Query;
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};

use super::Parts;
use crate::characters::contributes::KNOWN_ENGINES;
use crate::characters::{Character, CharactersService};
use crate::core::events::EventBus;
use crate::core::identity::permissions::{MODELS_MANAGE, STATION_VIEW};
use crate::helpers::contract::Ability;
use crate::helpers::remote::RemoteHelper;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Uses};
use crate::settings::definitions::{HOME_VOICE_ENGINES, PERSONA};
use crate::settings::SettingsService;

/// The events after which the Турбо hears the character again.
fn tells(name: &str) -> bool {
    matches!(name, "persona.changed" | "helpers.up" | "helpers.changed")
}

/// Starts telling the voice's Турбо the home's character.
pub fn watch(events: &EventBus, parts: impl Fn() -> Option<Parts> + Clone + Send + Sync + 'static) {
    events.on("home.character", move |event| {
        if !tells(event.name()) {
            return;
        }
        let Some(home) = parts() else { return };
        tokio::spawn(async move {
            // A burst (the list read again, a pairing) settles first.
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Err(error) = tell(&home).await {
                tracing::info!(target: "home", "the character to the Турбо: {error}");
            }
        });
    });
}

/// The model the owner chose for a character's voice: by the character, else by its package (chosen before it was
/// installed). Only one of them is kept at a time (`keep_choice`): the owner's last word.
pub fn chosen_engine(
    settings: &Arc<crate::settings::SettingsService>,
    character: &str,
    package: &str,
) -> Option<String> {
    let chosen = settings.get(&HOME_VOICE_ENGINES, None)?;
    chosen.get(character).or_else(|| chosen.get(package)).cloned()
}

/// The owner's last choice wins (21.11): the one by the character went first, so a later choice by its package (the
/// wizard's) was lost. A choice by a package drops its characters' (`<package>:*`), one by a character drops its
/// package's.
fn keep_choice(chosen: &mut BTreeMap<String, String>, key: &str, by_character: bool, engine: &str) {
    if by_character {
        if let Some((package, _)) = key.split_once(':') {
            chosen.remove(package);
        }
    } else {
        let theirs = format!("{key}:");
        chosen.retain(|kept, _| !kept.starts_with(&theirs));
    }
    chosen.insert(key.to_string(), engine.to_string());
}

/// A package's models for its characters' voices dropped — by the package and by each of its characters
/// (`<package>:*`), in the centre's choices or a Турбо's named ones; whether any was there.
pub fn drop_package(chosen: &mut BTreeMap<String, String>, package: &str) -> bool {
    let theirs = format!("{package}:");
    let before = chosen.len();
    chosen.retain(|kept, _| kept != package && !kept.starts_with(&theirs));
    chosen.len() != before
}

/// A package removed: the models chosen for its characters' voices go with it — here (the centre's choices) and in what
/// a Турбо's centre named to it, so a package installed again starts afresh.
pub fn forget_package(settings: &SettingsService, characters: &CharactersService, package: &str) {
    if let Some(mut chosen) = settings.get(&HOME_VOICE_ENGINES, None) {
        if drop_package(&mut chosen, package) {
            if let Err(error) = settings.set(&HOME_VOICE_ENGINES, &chosen, None) {
                tracing::warn!(target: "home", "{package}: its voice models not forgotten: {}", error.message);
            }
        }
    }
    characters.forget_engines(package);
}

/// The character as the Турбо should hear it: its id and package, and the model chosen for its voice; none — a
/// built-in one.
pub fn of_home(
    settings: &Arc<crate::settings::SettingsService>,
    characters: &Arc<crate::characters::CharactersService>,
) -> Option<Value> {
    let id = settings.get(&PERSONA, None)?.preset_id?;
    let character = characters.character(&id)?;
    let mut told = json!({ "id": character.id, "package": { "id": character.package, "version": character.version } });
    if let Some(engine) = chosen_engine(settings, &character.id, &character.package) {
        told["engine"] = json!(engine);
    }
    Some(told)
}

/// Where the centre's signed feed has this version of a package: its address and hash (`{url, sha256, size}`); none
/// when the feed is not read here or does not have it.
async fn archive_of(catalog: &Arc<crate::plugins::PackageCatalog>, package: &str, version: &str) -> Option<Value> {
    let feed = catalog.feed(false).await?;
    let entry = feed.find(package, Some(version))?;
    Some(json!({ "url": entry.url.as_str(), "sha256": entry.sha256, "size": entry.size }))
}

/// The Турбо that gives the voice: the one the voice goes to now, else one on the line — its voice ready or not.
fn voice_turbo(parts: &Parts) -> Option<Arc<RemoteHelper>> {
    parts
        .helpers
        .pick_remote(Ability::Voice)
        .or_else(|| parts.helpers.offering(Ability::Voice).into_iter().find(|helper| helper.up()))
}

/// Names the home's character to the Турбо that gives the voice, if there is one on the line — its voice ready or
/// not: a Турбо without a voice yet is the one that must hear it most, it installs the character's voice then (clean-path
/// check 18.11, item 12: the Mac was on the line without a voice and was never told, so the home spoke Alice's voice).
/// With the model chosen for its voice and where the centre's feed has its package.
pub async fn tell(parts: &Parts) -> Result<(), String> {
    let Some(mut character) = of_home(&parts.settings, &parts.characters) else { return Ok(()) };
    let Some(remote) = voice_turbo(parts) else { return Ok(()) };
    let package = (character["package"]["id"].as_str(), character["package"]["version"].as_str());
    if let (Some(package), Some(version)) = package {
        if let Some(archive) = archive_of(&parts.catalog, package, version).await {
            character["archive"] = archive;
        }
    }
    let response = crate::utils::http::client()
        .post(format!("{}/home/v1/character", remote.base().trim_end_matches('/')))
        .bearer_auth(remote.credential())
        .json(&character)
        // Its package may be downloaded first.
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let text: String = response.text().await.unwrap_or_default().chars().take(200).collect();
        return Err(format!("HTTP {status}: {text}"));
    }
    tracing::info!(target: "home", "the Турбо speaks as {}", character["id"]);
    Ok(())
}

/// Gigabytes as people read them: «10», «5,5», «0,8».
fn gb(mb: u64) -> String {
    let gb = mb as f64 / 1024.0;
    if gb >= 10.0 {
        format!("{gb:.0}")
    } else {
        format!("{gb:.1}").trim_end_matches(".0").replace('.', ",")
    }
}

/// A voice model that would not fit on the Турбо's disk (clean-path check 18.11, item 40): never downloaded.
pub fn no_room(model: &str, need_mb: u64, free_mb: u64) -> ApiError {
    ApiError::new(
        axum::http::StatusCode::INSUFFICIENT_STORAGE,
        crate::tr!(
            "Модель {model} не поместится: нужно {need} ГБ, свободно {free} ГБ.",
            model = model,
            need = gb(need_mb),
            free = gb(free_mb)
        ),
    )
}

pub fn routes<S: crate::home::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/home/voice", get(voice).put(choose))
        .route("/api/v1/home/voice/pending", get(pending).post(answer_pending))
}

#[derive(Debug, Default, serde::Deserialize)]
struct Which {
    #[serde(default)]
    character: Option<String>,
    #[serde(default)]
    package: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct Choice {
    #[serde(default)]
    character: Option<String>,
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    engine: String,
}

#[derive(Debug, Default, serde::Deserialize)]
struct Answer {
    #[serde(default)]
    answer: String,
}

/// A request to the Турбо in the language of whoever asked here: its words (a plan's failure, a pending voice's
/// reason) come back in it.
fn to_turbo(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match crate::locale::text::language_now() {
        Some(language) => request.header(reqwest::header::ACCEPT_LANGUAGE, language),
        None => request,
    }
}

/// What the Турбо says waits there (`GET /home/v1/voice/pending`); none while it is away, or an older one (404).
async fn waiting_at(remote: &RemoteHelper) -> Option<Value> {
    let request = crate::utils::http::client()
        .get(format!("{}/home/v1/voice/pending", remote.base().trim_end_matches('/')))
        .bearer_auth(remote.credential())
        .timeout(Duration::from_secs(10));
    let response = to_turbo(request)
        .send()
        .await
        .map_err(|error| tracing::debug!(target: "home", "the waiting voice from the Турбо: {error}"))
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let answer = response.json::<Value>().await.ok()?;
    Some(answer["pending"].clone()).filter(|pending| !pending.is_null())
}

/// The voice waiting for the owner, wherever it waits (clean-path check 19.11: «Голос ждёт» lived only on the
/// computer's own page): this gateway's own (a computer that is the home's centre), else the voice's Турбо's —
/// `{turbo: {id, name, up} | null, pending: {…, title, character?} | null}`.
async fn waiting(parts: &Parts) -> Value {
    #[cfg(feature = "pc")]
    if let Some(pending) =
        crate::home_server::voice_plan::waiting(&parts.models, &parts.characters, &parts.voice_lab).await
    {
        return json!({ "turbo": null, "pending": pending });
    }
    let remote = voice_turbo(parts).or_else(|| parts.helpers.offering(Ability::Voice).into_iter().next());
    let Some(remote) = remote else {
        return json!({ "turbo": null, "pending": null });
    };
    let up = remote.up();
    let name = parts.helpers.views().into_iter().find(|view| view.id == remote.id()).map(|view| view.name);
    let pending = if up { waiting_at(&remote).await } else { None };
    json!({
        "turbo": { "id": remote.id(), "name": name.unwrap_or_else(|| remote.id().to_string()), "up": up },
        "pending": pending,
    })
}

async fn pending(Uses(parts): Uses<Parts>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(waiting(&parts).await))
}

/// The owner's answer to the waiting voice — `{"answer": "anyway" | "retry" | "cancel"}` — here, or passed to the
/// Турбо where it waits; then what waits now.
async fn answer_pending(Uses(parts): Uses<Parts>, caller: Caller, Body(answer): Body<Answer>) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    #[cfg(feature = "pc")]
    if parts.models.pending().is_some() {
        crate::home_server::voice_plan::answer(&parts.models, &answer.answer, caller.actor()).await?;
        return Ok(ok(waiting(&parts).await));
    }
    let Some(remote) = voice_turbo(&parts).filter(|remote| remote.up()) else {
        return Err(ApiError::conflict(crate::tr!("Компьютер с голосом не на связи — ответить ему пока нельзя.")));
    };
    let request = crate::utils::http::client()
        .post(format!("{}/home/v1/voice/pending", remote.base().trim_end_matches('/')))
        .bearer_auth(remote.credential())
        .json(&json!({ "answer": answer.answer }))
        // «Отменить» waits there for the stopped download's program to go.
        .timeout(Duration::from_secs(30));
    let response = to_turbo(request).send().await.map_err(|error| ApiError::bad_gateway(error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        let body: Value = response.json().await.unwrap_or_default();
        let message = body["message"].as_str().map(str::to_string).unwrap_or_else(|| format!("HTTP {status}"));
        let status = axum::http::StatusCode::from_u16(status.as_u16()).unwrap_or(axum::http::StatusCode::BAD_GATEWAY);
        return Err(ApiError::new(status, message));
    }
    Ok(ok(waiting(&parts).await))
}

/// What the Турбо's plan needs of an installed character.
fn plan_request(character: &Character, engine: Option<String>) -> Value {
    json!({
        "id": character.id,
        "package": { "id": character.package, "version": character.version },
        "voices": character.voices.keys().collect::<Vec<_>>(),
        "reference": character.reference.is_some(),
        "title": character.title,
        "gender": character.gender,
        "fallbackVoice": character.fallback_voice,
        "engine": engine,
    })
}

/// The plan's request and the owner's choice for a character, or for a package of the catalog not installed here yet:
/// its models from the feed's `needs` of kind `engine`, a reference taken as there.
async fn asked(
    characters: &Arc<crate::characters::CharactersService>,
    settings: &Arc<crate::settings::SettingsService>,
    catalog: &Arc<crate::plugins::PackageCatalog>,
    which: &Which,
) -> Result<(Value, Option<String>), ApiError> {
    let character = match (&which.character, &which.package) {
        (Some(id), _) => Some(
            characters.character(id).ok_or_else(|| ApiError::not_found(crate::tr!("Нет персонажа {id}.", id = id)))?,
        ),
        (None, Some(package)) => characters.of_package(package),
        (None, None) => return Err(ApiError::bad_request("character или package".to_string())),
    };
    if let Some(character) = character {
        let chosen = chosen_engine(settings, &character.id, &character.package);
        return Ok((plan_request(&character, chosen.clone()), chosen));
    }
    let package = which.package.clone().unwrap_or_default();
    let feed = catalog.feed(false).await;
    let entry = feed
        .as_ref()
        .and_then(|feed| feed.find(&package, None))
        .ok_or_else(|| ApiError::not_found(crate::tr!("В каталоге нет «{id}».", id = package)))?;
    let voices: Vec<&str> = entry
        .needs
        .as_array()
        .into_iter()
        .flatten()
        .filter(|need| need.get("kind").and_then(Value::as_str) == Some("engine"))
        .filter_map(|need| need.get("id").and_then(Value::as_str))
        .filter(|engine| KNOWN_ENGINES.contains(engine))
        .collect();
    let chosen = chosen_engine(settings, &package, &package);
    let request = json!({
        "id": package,
        "package": { "id": package, "version": entry.version.to_string() },
        "voices": voices,
        "reference": true,
        "title": entry.title,
        "engine": chosen,
    });
    Ok((request, chosen))
}

/// The Турбо's plan; none while it is away or when it does not know the plan (an older one: 404).
async fn plan_of(remote: &RemoteHelper, request: &Value) -> Option<Value> {
    let request = crate::utils::http::client()
        .post(format!("{}/home/v1/character/plan", remote.base().trim_end_matches('/')))
        .bearer_auth(remote.credential())
        .json(request)
        .timeout(Duration::from_secs(15));
    let response = to_turbo(request)
        .send()
        .await
        .map_err(|error| tracing::info!(target: "home", "the voice's plan from the Турбо: {error}"))
        .ok()?;
    if !response.status().is_success() {
        tracing::info!(target: "home", "the voice's plan from the Турбо: HTTP {}", response.status());
        return None;
    }
    response.json::<Value>().await.ok()
}

/// `{turbo: {id, name, up} | null, plan: VoicePlan | null, chosen}`.
async fn answer(parts: &Parts, which: &Which) -> Result<Value, ApiError> {
    let (request, chosen) = asked(&parts.characters, &parts.settings, &parts.catalog, which).await?;
    let remote = voice_turbo(parts).or_else(|| parts.helpers.offering(Ability::Voice).into_iter().next());
    let Some(remote) = remote else {
        return Ok(json!({ "turbo": null, "plan": null, "chosen": chosen }));
    };
    let up = remote.up();
    let name = parts.helpers.views().into_iter().find(|view| view.id == remote.id()).map(|view| view.name);
    let plan = if up { plan_of(&remote, &request).await } else { None };
    Ok(json!({
        "turbo": { "id": remote.id(), "name": name.unwrap_or_else(|| remote.id().to_string()), "up": up },
        "plan": plan,
        "chosen": chosen,
    }))
}

async fn voice(Uses(parts): Uses<Parts>, caller: Caller, Query(which): Query<Which>) -> ApiResult<Value> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(answer(&parts, &which).await?))
}

/// The owner's model for a character's voice (or a package's, before it is installed): kept here and named to the
/// Турбо at once, in the background — its package may be downloaded there first.
async fn choose(Uses(parts): Uses<Parts>, caller: Caller, Body(choice): Body<Choice>) -> ApiResult<Value> {
    caller.permitted(MODELS_MANAGE)?;
    if !KNOWN_ENGINES.contains(&choice.engine.as_str()) {
        return Err(ApiError::bad_request(crate::tr!(
            "engine: одна из моделей голоса — {engines}.",
            engines = KNOWN_ENGINES.join(", ")
        )));
    }
    let key = match (&choice.character, &choice.package) {
        (Some(id), _) => id.clone(),
        (None, Some(package)) if crate::plugins::manifest::valid_id(package) => package.clone(),
        _ => return Err(ApiError::bad_request("character или package".to_string())),
    };
    let which = Which { character: choice.character.clone(), package: choice.package.clone() };
    let (request, _) = asked(&parts.characters, &parts.settings, &parts.catalog, &which).await?;
    // A model that would not fit on the Турбо's disk is refused, and not kept: its plan says so while it is on the line.
    if let Some(remote) = voice_turbo(&parts).filter(|remote| remote.up()) {
        if let Some(plan) = plan_of(&remote, &request).await {
            let row = plan["engines"].as_array().into_iter().flatten().find(|row| row["id"] == choice.engine.as_str());
            if let Some(row) = row.filter(|row| row["diskFits"] == false) {
                let number = |name: &str| row[name].as_u64().unwrap_or_default();
                let title = row["title"].as_str().unwrap_or(&choice.engine);
                return Err(no_room(title, number("diskMb"), number("freeDiskMb")));
            }
        }
    }
    let mut chosen = parts.settings.get(&HOME_VOICE_ENGINES, None).unwrap_or_default();
    keep_choice(&mut chosen, &key, choice.character.is_some(), &choice.engine);
    parts.settings.set(&HOME_VOICE_ENGINES, &chosen, caller.actor())?;
    let told = parts.clone();
    tokio::spawn(async move {
        if let Err(error) = tell(&told).await {
            tracing::info!(target: "home", "the voice's model to the Турбо: {error}");
        }
    });
    Ok(ok(answer(&parts, &which).await?))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::body::Body as HttpBody;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use http_body_util::BodyExt;
    use serde_json::Map;
    use tower::ServiceExt;

    use super::*;
    use crate::app::App;
    use crate::characters::contributes::tests::{arthur_manifest, write_arthur};
    use crate::config::{Config, Env};
    use crate::helpers::tests::{contract, KEY};
    use crate::helpers::{AddInput, HelperKind};

    /// The Турбо as the centre reaches it: its status, the plan it answers (404 — an older one), what it was told; the
    /// voice waiting there, the owner's answers it took and the languages it was asked in.
    #[derive(Default)]
    struct Turbo {
        url: String,
        old: std::sync::atomic::AtomicBool,
        asked: Mutex<Vec<Value>>,
        told: Mutex<Vec<Value>>,
        waiting: Mutex<Value>,
        answers: Mutex<Vec<Value>>,
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
    async fn centre(dir: &std::path::Path, feed: Option<&str>) -> (Arc<App>, Arc<Turbo>) {
        let app = station(dir, feed).await;
        let turbo = turbo().await;
        let add =
            AddInput { url: turbo.url.clone(), key: KEY.into(), kind: Some(HelperKind::Own), ..AddInput::default() };
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

    fn key(app: &App, grants: &[&str]) -> String {
        let input = Map::from_iter([("name".into(), json!("Пульт")), ("grants".into(), json!(grants))]);
        format!("Bearer {}", app.access.keys.create(&input, None).unwrap().secret)
    }

    async fn send(app: &Arc<App>, method: &str, uri: &str, key: &str, body: Value) -> (StatusCode, Value) {
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

        let (status, view) =
            send(&app, "GET", "/api/v1/home/voice?character=arthur:arthur", &viewer, Value::Null).await;
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
        assert_eq!(
            (&view["plan"], &view["chosen"], &view["turbo"]["up"]),
            (&Value::Null, &json!("voxcpm"), &json!(true))
        );
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
    /// refusal comes back in its words. A computer that is the centre answers its own waiting voice itself. With no
    /// Турбо nothing waits, and an answer has nowhere to go.
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

        // A computer as the home's centre: the voice waiting on it is its own, answered here and not passed on.
        #[cfg(feature = "pc")]
        {
            use crate::settings::definitions::{PendingWait, VoicePending, VOICE_PENDING};
            let own = VoicePending {
                model_id: "piper-denis".into(),
                voice_id: None,
                wait_for: PendingWait { kind: "file".into(), id: "piper-denis".into() },
                anyway: false,
                state: "failed".into(),
                reason: Some("файл голоса не скачался".into()),
                memory: None,
                recipe: None,
                since: "2026-11-19T10:00:00Z".into(),
            };
            app.settings.set(&VOICE_PENDING, &own, None).unwrap();
            let (_, view) = send(&app, "GET", PENDING, &viewer, Value::Null).await;
            assert_eq!((&view["turbo"], &view["pending"]["modelId"]), (&Value::Null, &json!("piper-denis")), "{view}");
            assert_eq!(view["pending"]["reason"], "файл голоса не скачался");
            let (status, view) = send(&app, "POST", PENDING, &owner, json!({ "answer": "cancel" })).await;
            assert_eq!((status, &view["pending"]), (StatusCode::OK, &Value::Null), "{view}");
            assert!(app.models.pending().is_none(), "forgotten here");
            assert_eq!(turbo.answers.lock().unwrap().len(), 2, "the centre's own voice is not the Турбо's");
        }

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
}
