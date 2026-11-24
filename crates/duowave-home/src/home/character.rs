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
use crate::characters::Character;
use crate::core::events::EventBus;
use crate::core::identity::permissions::{MODELS_MANAGE, STATION_VIEW};
use crate::helpers::contract::Ability;
use crate::helpers::remote::RemoteHelper;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Uses};
use crate::settings::definitions::{HOME_VOICE_ENGINES, PERSONA};

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
pub fn keep_choice(chosen: &mut BTreeMap<String, String>, key: &str, by_character: bool, engine: &str) {
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

/// A package's models for its characters' voices dropped: the characters' own (characters/mod.rs).
pub use crate::characters::drop_package;

/// A package removed: the models chosen for its characters' voices go with it (characters/mod.rs).
pub use crate::characters::forget_package;

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
    // A computer that is the home's centre (crates/duowave-pc): the voice waits here.
    if let Some(computer) = &parts.computer {
        if let Some(pending) = computer.voice_waiting(&parts.characters).await {
            return json!({ "turbo": null, "pending": pending });
        }
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
    if let Some(computer) = &parts.computer {
        if let Some(answered) = computer.answer_voice(&answer.answer, caller.actor()).await {
            answered?;
            return Ok(ok(waiting(&parts).await));
        }
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
