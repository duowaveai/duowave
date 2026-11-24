//! `/home/v1/devices/*` on a computer that is a Турбо (docs/plans/turbo-devices.md): its devices for its home's centre.
//! The centre has already decided who may do what — the person, the app's links, the «да»; this computer checks only
//! that the home is one it gives devices to (`devices`, the home's key, «Кому помогаю»), that it gives this kind
//! («Что этот компьютер даёт дому», `turbo_shares`), and that a thing at hand is reached from home, not over the link
//! (owner 20.11, 6).
//!
//! - `POST /home/v1/devices/<kind>/call {handle, input}` — a letter to one of its devices, its answer back.
//! - `GET /home/v1/devices/midi/stream` — the notes' channel for the centre (src/midi/relay.rs): the same frames as a
//!   page's `/api/v1/midi/ws`, the centre's own owner; only from home — the link carries no WebSocket.
//! - `POST /home/v1/devices/midi/read` — a `.mid` read into tracks for a centre whose build has no MIDI.
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::Value;

use super::serve::{allowed, Door, Needs};
use crate::bus::providers::devices::{Device, Nature};
use crate::bus::Bus;
use crate::bus::BusFolders;
use crate::helpers::contract::Ability;
use crate::home::turbo_devices::HouseDevices;
use crate::midi::MidiService;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Uses};
use crate::settings::SettingsService;

pub fn router<S: crate::home_server::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/home/v1/devices/{kind}/call", post(call))
        .route("/home/v1/devices/midi/stream", get(stream))
        .route("/home/v1/devices/midi/read", post(read))
        .route("/home/v1/files/call", post(files_call))
}

#[derive(Debug, Deserialize)]
struct FilesLetter {
    /// `files/home/…`: the household's folders only — never an app's, a person's or a project's.
    to: String,
    handle: String,
    #[serde(default)]
    input: Value,
}

/// A letter to this computer's home folder from its home's centre (the owner's decision 1, 6 of 20.11: the home's
/// folders are every place's; folders reach from afar, a flash drive only from home). The centre has decided who may;
/// this computer checks its own «Что даёт дому».
async fn files_call(
    Uses(door): Uses<Door>,
    Uses(settings): Uses<Arc<SettingsService>>,
    Uses(bus): Uses<Arc<Bus>>,
    caller: Caller,
    Body(letter): Body<FilesLetter>,
) -> ApiResult<Value> {
    allowed(&door, &caller, Needs::Ability(Ability::Files))?;
    let shares = settings.get(&crate::settings::definitions::TURBO_SHARES, None).unwrap_or_default();
    if !shares.gives("folders", false) {
        return Err(ApiError::forbidden(crate::tr!(
            "Этот компьютер не даёт дому свои папки: это включают на нём, на странице «Этот компьютер»."
        )));
    }
    let to = crate::bus::Address::parse(&letter.to).map_err(ApiError::bad_request)?;
    let home = format!("{}/home/", crate::bus::address::FILES);
    if !to.as_str().starts_with(&home) {
        return Err(ApiError::forbidden(crate::tr!("Дому отдаются только папки дома.")));
    }
    let attached = format!("{home}{}", crate::bus::providers::files::ATTACHED);
    if to.as_str().starts_with(&attached) {
        if !shares.gives("storage", false) {
            return Err(ApiError::forbidden(crate::tr!(
                "Этот компьютер не даёт дому флешки: это включают на нём, на странице «Этот компьютер»."
            )));
        }
        // A drive is a thing at hand: the home's only while this computer is at home.
        if caller.link().is_some() {
            return Err(ApiError::conflict(crate::tr!(
                "Флешки этого компьютера дому не видны: он сейчас не дома, на связи не из домашней сети."
            )));
        }
    }
    let files = bus.files().ok_or_else(|| ApiError::not_found(crate::tr!("папок здесь нет")))?;
    let from = crate::bus::Address::new(crate::bus::address::GATEWAY, "centre").map_err(ApiError::internal)?;
    let letter = crate::bus::Letter {
        id: 0,
        from,
        to,
        handle: letter.handle,
        input: letter.input,
        files: Vec::new(),
        trail: crate::bus::Trail::system(),
    };
    use crate::bus::Provider;
    match files.call(letter).await {
        Ok(answer) => Ok(ok(answer)),
        Err(crate::bus::BusError::NotFound(what)) => Err(ApiError::not_found(what)),
        Err(crate::bus::BusError::Denied(why)) => Err(ApiError::forbidden(why)),
        Err(error) => Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, error.to_string())),
    }
}

#[derive(Debug, Deserialize)]
struct Letter {
    handle: String,
    #[serde(default)]
    input: Value,
}

/// The device of a kind this computer gives its home now; a thing at hand only to a home that reaches it from home.
fn given(
    door: &Door,
    house: &HouseDevices,
    settings: &SettingsService,
    caller: &Caller,
    kind: &str,
) -> Result<Arc<dyn Device>, ApiError> {
    allowed(door, caller, Needs::Ability(Ability::Devices))?;
    let device = house
        .local()
        .get(kind)
        .ok_or_else(|| ApiError::not_found(crate::tr!("У этого компьютера нет «{kind}».", kind = kind)))?;
    let shares = settings.get(&crate::settings::definitions::TURBO_SHARES, None).unwrap_or_default();
    if !shares.gives(kind, device.sensitive()) {
        return Err(ApiError::forbidden(crate::tr!(
            "Этот компьютер не даёт дому «{title}»: это включают на нём, на странице «Этот компьютер».",
            title = device.title()
        )));
    }
    if device.nature() == Nature::Physical && caller.link().is_some() {
        return Err(ApiError::conflict(crate::tr!(
            "«{title}» — у этого компьютера, а он сейчас не дома: на связи не из домашней сети.",
            title = device.title()
        )));
    }
    Ok(device)
}

/// A letter to one of its devices.
async fn call(
    Uses(door): Uses<Door>,
    Uses(house): Uses<Arc<HouseDevices>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    caller: Caller,
    Path(kind): Path<String>,
    Body(letter): Body<Letter>,
) -> ApiResult<Value> {
    let device = given(&door, &house, &settings, &caller, &kind)?;
    match device.call(&letter.handle, letter.input).await {
        Ok(answer) => Ok(ok(answer)),
        Err(crate::bus::BusError::NotFound(what)) => {
            Err(ApiError::not_found(crate::tr!("Такого умения нет: {what}", what = what)))
        }
        Err(error) => Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, error.to_string())),
    }
}

/// The centre's channel of notes: only from the home network, never over the link (which carries no WebSocket).
async fn stream(
    Uses(door): Uses<Door>,
    Uses(house): Uses<Arc<HouseDevices>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    Uses(midi): Uses<Arc<MidiService>>,
    caller: Caller,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    given(&door, &house, &settings, &caller, "midi")?;
    midi.may_hear(crate::midi::CENTRE, caller.zone).map_err(ApiError::forbidden)?;
    Ok(crate::midi::socket::upgrade(upgrade, midi.clone(), crate::midi::CENTRE.to_string(), caller.zone))
}

/// A `.mid` read into tracks, for a centre whose build has no MIDI; the file is kept nowhere.
async fn read(Uses(door): Uses<Door>, caller: Caller, bytes: Bytes) -> ApiResult<crate::midi::file::SongView> {
    allowed(&door, &caller, Needs::Ability(Ability::Devices))?;
    if bytes.len() > crate::midi::file::FILE_MAX {
        return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, crate::tr!("Файл MIDI слишком большой.")));
    }
    let reading = tokio::task::spawn_blocking(move || crate::midi::file::read(&bytes));
    match tokio::time::timeout(std::time::Duration::from_secs(1), reading).await {
        Ok(Ok(Ok(song))) => Ok(ok(song)),
        Ok(Ok(Err(why))) => Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, why)),
        _ => Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, crate::tr!("Файл не прочитался за секунду."))),
    }
}
