//! `/api/v1/midi` (docs/plans/midi.md, «API»): the keyboards and who listens, the channel of notes' ticket and socket,
//! a `.mid` read into tracks. Whoever may use the apps (`use:plugins`), as the apps' own calls from the panel.
//!
//! The keyboards are the home's (docs/plans/turbo-devices.md): this gateway's own and those of its Турбо at home. A
//! page at a centre whose keyboards are at a Турбo hears them through the relay (relay.rs) by the same frames; a page
//! of the station's window on the computer the keyboard is plugged into (`?via=`, `station` in the ticket) hears this
//! computer's own keyboard at once, the station having said that the app may.
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Query, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::file::{SongView, FILE_MAX};
use super::relay::{self, Place, Relay, Way};
use super::socket::TICKET_LIFETIME;
use super::{Device, MidiView};
use crate::bus::providers::devices::{House, HERE};
use crate::core::identity::permissions::use_of;
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Uses, Zone};
use crate::skills::capabilities::CapabilityName;

/// What the MIDI routes use of the gateway: this computer's keyboards, the home's devices (a Турбо's keyboards), the
/// configuration (MIDI_REMOTE), the apps (whether a page may hear), and on a computer its stations (a station's window
/// on the keyboard plugged in here). The gateway makes it (src/links/http.rs).
#[derive(Clone)]
pub struct Parts {
    pub midi: Arc<super::MidiService>,
    pub house: Arc<dyn House>,
    pub config: Arc<crate::config::Config>,
    pub plugins: Arc<crate::plugins::PluginHost>,
    /// On a computer its stations (none on the station: no station's window on it).
    pub stations: Option<Arc<dyn PairedStations>>,
}

/// A station of this computer, as the window on it finds it (`PairedStations`).
pub enum StationPair {
    /// No such station here.
    Unknown,
    /// The station is here, but it has no pair key.
    Unpaired,
    /// Its address and the pair's key.
    Paired { base: String, key: String },
}

/// This computer's stations, as a station's window on the keyboard plugged in here asks one (src/placement keeps them;
/// links/http.rs gives them).
pub trait PairedStations: Send + Sync {
    /// The station by its key, and how it is reached.
    fn pair_of(&self, station: &str) -> StationPair;
}

pub fn routes<S: crate::midi::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/midi", get(view))
        .route("/api/v1/midi/ticket", post(ticket))
        .route("/api/v1/midi/ws", get(socket))
        .route("/api/v1/midi/read", post(read).layer(DefaultBodyLimit::max(FILE_MAX + 1024)))
}

fn may_use(caller: &Caller) -> Result<(), ApiError> {
    caller.permitted(&use_of(CapabilityName::Plugins)).map(|_| ())
}

/// A Турбо at home has a keyboard plugged in now: the centre relays it.
fn at_turbo(parts: &Parts) -> bool {
    parts.house.ready("midi").iter().any(|place| place.place != HERE)
}

/// May a page of this app, from this zone, hear the home's keyboards: this gateway's own as before; a Турбо's — at
/// home only (the owner's decision 2 of the MIDI plan), with the app's link `listen:device/midi`. Never the gateway's
/// own owners (`@…`) for a page.
pub fn may_hear(parts: &Parts, package: &str, zone: Zone) -> Result<(), String> {
    if super::own_owner(package) {
        return Err(crate::tr!("приложение «{app}» выключено или не стоит здесь", app = package));
    }
    if !at_turbo(parts) {
        if !parts.midi.available() && parts.house.knows("midi") {
            return Err(parts.house.why_none("midi"));
        }
        return parts.midi.may_hear(package, zone);
    }
    if zone == Zone::Remote && !parts.config.midi.remote {
        return Err(crate::tr!("Клавиатуру слышно только дома: пульт вне дома её не слышит (MIDI_REMOTE)."));
    }
    parts.plugins.may_hear_midi(package)
}

/// The places a page here hears: this gateway's own keyboards, and every Турбо's at home reached by its home address.
fn places(parts: &Parts) -> Vec<Place> {
    let mut places = Vec::new();
    for place in parts.house.ready("midi") {
        if place.place == HERE {
            places.push(Place { id: HERE.into(), name: place.name, way: Way::Local(parts.midi.clone()) });
        } else if let Some((url, key)) = parts.house.stream_of(&place.place) {
            places.push(Place { id: place.place, name: place.name, way: Way::Remote { url, key } });
        }
    }
    places
}

/// The keyboards plugged in, read now, and who listens; why there is no MIDI here, if there is none. A Турбо's are
/// named with their place («Keystation 49 · Mac») and their ids carry it (`home-computer:in-2`).
async fn view(Uses(parts): Uses<Parts>, caller: Caller) -> ApiResult<MidiView> {
    may_use(&caller)?;
    let midi = parts.midi.clone();
    let mut view = tokio::task::spawn_blocking(move || midi.view())
        .await
        .map_err(|_| ApiError::internal(crate::tr!("MIDI не прочитался")))?;
    if !at_turbo(&parts) {
        if !view.available && parts.house.knows("midi") {
            view.why = Some(parts.house.why_none("midi"));
        }
        return Ok(ok(view));
    }
    let names: Vec<(String, String)> =
        parts.house.places("midi").into_iter().map(|place| (place.place, place.name)).collect();
    let several = parts.house.ready("midi").len() > 1;
    let listed = parts.house.letter("midi", "keyboards", json!({})).await.unwrap_or(Value::Null);
    let mut devices: Vec<Device> = Vec::new();
    for item in listed.as_array().into_iter().flatten() {
        let text = |key: &str| item.get(key).and_then(Value::as_str).map(str::to_string);
        let Some(name) = text("name") else { continue };
        let place = text("place").unwrap_or_default();
        let named = match names.iter().find(|(id, _)| *id == place) {
            Some((_, place_name)) if several => format!("{name} · {place_name}"),
            _ => name,
        };
        devices.push(Device { name: named, input: text("input"), output: text("output") });
    }
    view.available = true;
    view.why = None;
    view.devices = devices;
    Ok(ok(view))
}

#[derive(Debug, Deserialize)]
struct TicketBody {
    package: String,
    /// The station's window on this computer (`?via=<station>`): the station decides, this computer's keyboard plays.
    #[serde(default)]
    station: Option<String>,
}

/// A ticket for the channel of notes of one app's page.
async fn ticket(
    Uses(parts): Uses<Parts>,
    caller: Caller,
    Body(body): Body<TicketBody>,
) -> ApiResult<serde_json::Value> {
    may_use(&caller)?;
    if let Some(station) = body.station.as_deref().map(str::trim).filter(|station| !station.is_empty()) {
        return station_ticket(&parts, &caller, station, &body.package).await;
    }
    may_hear(&parts, &body.package, caller.zone).map_err(ApiError::forbidden)?;
    let ticket = parts.midi.tickets.issue(&body.package, caller.zone);
    Ok(created(json!({ "ticket": ticket, "expiresIn": TICKET_LIFETIME.as_secs() })))
}

/// The station's window on the computer the keyboard is plugged into: the station says whether the app's page may hear
/// (its person, its app, its «да») — asked through the pair, as the window's own requests go —, and this computer gives
/// a ticket to its own keyboard, the centre's owner. No relay: the notes go nowhere but here.
async fn station_ticket(parts: &Parts, caller: &Caller, station: &str, package: &str) -> ApiResult<serde_json::Value> {
    // The station's own build has no station's window of its own.
    let Some(stations) = &parts.stations else {
        return Err(ApiError::not_found(crate::tr!("Такой колонки у этого компьютера нет.")));
    };
    caller.zone_at_least(Zone::Home)?;
    let principal = caller.permitted(crate::core::identity::permissions::STATION_VIEW)?.clone();
    let (base, key) = match stations.pair_of(station) {
        StationPair::Unknown => return Err(ApiError::not_found(crate::tr!("Такой колонки у этого компьютера нет."))),
        StationPair::Unpaired => {
            return Err(ApiError::conflict(crate::tr!("Колонка не связана с этим компьютером: нет ключа пары.")))
        }
        StationPair::Paired { base, key } => (base, key),
    };
    let answer = crate::platform::http::peer::as_person(
        &principal,
        crate::utils::http::client_without_redirects().post(format!("{base}/api/v1/midi/ticket")).bearer_auth(key),
    )
    .timeout(Duration::from_secs(8))
    .json(&json!({ "package": package }))
    .send()
    .await
    .map_err(|_| ApiError::new(StatusCode::BAD_GATEWAY, crate::tr!("Колонка не ответила.")))?;
    let status = answer.status();
    if !status.is_success() {
        let body: Value = answer.json().await.unwrap_or(Value::Null);
        let words =
            body.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| crate::tr!("отказ"));
        return Err(ApiError::forbidden(words));
    }
    parts.midi.may_hear(super::CENTRE, caller.zone).map_err(ApiError::forbidden)?;
    let ticket = parts.midi.tickets.issue(super::CENTRE, caller.zone);
    Ok(created(json!({ "ticket": ticket, "expiresIn": TICKET_LIFETIME.as_secs(), "here": true })))
}

#[derive(Debug, Deserialize)]
struct SocketQuery {
    #[serde(default)]
    ticket: Option<String>,
}

/// The channel of notes, by a ticket; the page is still at home and the app may still hear. Where a Турбо has a
/// keyboard, the relay: this gateway's own and every Турбо's at home in one channel.
async fn socket(
    Uses(parts): Uses<Parts>,
    caller: Caller,
    Query(query): Query<SocketQuery>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let (package, _) = query
        .ticket
        .as_deref()
        .and_then(|ticket| parts.midi.tickets.take(ticket))
        .ok_or_else(|| ApiError::unauthorized(crate::tr!("Билет канала нот не подошёл: возьмите новый.")))?;
    // The station's window here: this computer's own keyboard, as its ticket said.
    if super::own_owner(&package) {
        parts.midi.may_hear(&package, caller.zone).map_err(ApiError::forbidden)?;
        return Ok(super::socket::upgrade(upgrade, parts.midi.clone(), package, caller.zone));
    }
    may_hear(&parts, &package, caller.zone).map_err(ApiError::forbidden)?;
    let places = places(&parts);
    let only_here = places.iter().all(|place| place.id == HERE);
    if only_here {
        return Ok(super::socket::upgrade(upgrade, parts.midi.clone(), package, caller.zone));
    }
    let zone = caller.zone;
    let check = {
        let (parts, package) = (parts.clone(), package.clone());
        Arc::new(move || may_hear(&parts, &package, zone))
    };
    let choose = {
        let house = parts.house.clone();
        Arc::new(move || house.choose("midi", "").ok().map(|choice| (choice.place, choice.said)))
    };
    Ok(relay::upgrade(upgrade, Relay { places, package, zone, check, choose }))
}

/// A `.mid` read into tracks with their melodies; the file is kept nowhere. A centre whose build has no MIDI asks a
/// Турбо that has.
async fn read(Uses(parts): Uses<Parts>, caller: Caller, bytes: Bytes) -> ApiResult<Value> {
    may_use(&caller)?;
    if !cfg!(feature = "midi") && at_turbo(&parts) {
        let song = parts.house.read_midi(bytes.to_vec()).await;
        return song.map(ok).map_err(|why| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, why));
    }
    let reading = tokio::task::spawn_blocking(move || super::file::read(&bytes));
    match tokio::time::timeout(Duration::from_secs(1), reading).await {
        Ok(Ok(Ok(song))) => Ok(ok(serde_json::to_value::<SongView>(song).unwrap_or(Value::Null))),
        Ok(Ok(Err(why))) => Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, why)),
        _ => Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, crate::tr!("Файл не прочитался за секунду."))),
    }
}
