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
use axum::extract::{DefaultBodyLimit, Query, State, WebSocketUpgrade};
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
use crate::app::App;
use crate::core::identity::permissions::use_of;
use crate::home::turbo_devices::HERE;
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Zone};
use crate::skills::capabilities::CapabilityName;

pub fn routes() -> Router<Arc<App>> {
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
fn at_turbo(app: &App) -> bool {
    app.house.ready("midi").iter().any(|place| place.place != HERE)
}

/// May a page of this app, from this zone, hear the home's keyboards: this gateway's own as before; a Турбо's — at
/// home only (the owner's decision 2 of the MIDI plan), with the app's link `listen:device/midi`. Never the gateway's
/// own owners (`@…`) for a page.
pub fn may_hear(app: &App, package: &str, zone: Zone) -> Result<(), String> {
    if super::own_owner(package) {
        return Err(crate::tr!("приложение «{app}» выключено или не стоит здесь", app = package));
    }
    if !at_turbo(app) {
        if !app.midi.available() && app.house.knows("midi") {
            return Err(app.house.why_none("midi"));
        }
        return app.midi.may_hear(package, zone);
    }
    if zone == Zone::Remote && !app.config.midi.remote {
        return Err(crate::tr!("Клавиатуру слышно только дома: пульт вне дома её не слышит (MIDI_REMOTE)."));
    }
    app.plugins.may_hear_midi(package)
}

/// The places a page here hears: this gateway's own keyboards, and every Турбо's at home reached by its home address.
fn places(app: &App) -> Vec<Place> {
    let mut places = Vec::new();
    for place in app.house.ready("midi") {
        if place.place == HERE {
            places.push(Place { id: HERE.into(), name: place.name, way: Way::Local(app.midi.clone()) });
        } else if let Some((url, key)) = app.house.stream_of(&place.place) {
            places.push(Place { id: place.place, name: place.name, way: Way::Remote { url, key } });
        }
    }
    places
}

/// The keyboards plugged in, read now, and who listens; why there is no MIDI here, if there is none. A Турбо's are
/// named with their place («Keystation 49 · Mac») and their ids carry it (`home-computer:in-2`).
async fn view(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<MidiView> {
    may_use(&caller)?;
    let midi = app.midi.clone();
    let mut view = tokio::task::spawn_blocking(move || midi.view())
        .await
        .map_err(|_| ApiError::internal(crate::tr!("MIDI не прочитался")))?;
    if !at_turbo(&app) {
        if !view.available && app.house.knows("midi") {
            view.why = Some(app.house.why_none("midi"));
        }
        return Ok(ok(view));
    }
    let names: Vec<(String, String)> =
        app.house.places("midi").into_iter().map(|place| (place.place, place.name)).collect();
    let several = app.house.ready("midi").len() > 1;
    let listed = app.house.letter("midi", "keyboards", json!({})).await.unwrap_or(Value::Null);
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
    State(app): State<Arc<App>>,
    caller: Caller,
    Body(body): Body<TicketBody>,
) -> ApiResult<serde_json::Value> {
    may_use(&caller)?;
    if let Some(station) = body.station.as_deref().map(str::trim).filter(|station| !station.is_empty()) {
        return station_ticket(&app, &caller, station, &body.package).await;
    }
    may_hear(&app, &body.package, caller.zone).map_err(ApiError::forbidden)?;
    let ticket = app.midi.tickets.issue(&body.package, caller.zone);
    Ok(created(json!({ "ticket": ticket, "expiresIn": TICKET_LIFETIME.as_secs() })))
}

/// The station's window on the computer the keyboard is plugged into: the station says whether the app's page may hear
/// (its person, its app, its «да») — asked through the pair, as the window's own requests go —, and this computer gives
/// a ticket to its own keyboard, the centre's owner. No relay: the notes go nowhere but here.
#[cfg(feature = "pc")]
async fn station_ticket(app: &App, caller: &Caller, station: &str, package: &str) -> ApiResult<serde_json::Value> {
    caller.zone_at_least(Zone::Home)?;
    let principal = caller.permitted(crate::core::identity::permissions::STATION_VIEW)?.clone();
    let gateway = app
        .placement
        .station()
        .and_then(|stations| stations.find(station))
        .ok_or_else(|| ApiError::not_found(crate::tr!("Такой колонки у этого компьютера нет.")))?;
    let peer = gateway
        .client()
        .ok_or_else(|| ApiError::conflict(crate::tr!("Колонка не связана с этим компьютером: нет ключа пары.")))?;
    let answer = crate::placement::proxy::as_person(
        &principal,
        crate::utils::http::client_without_redirects()
            .post(format!("{}/api/v1/midi/ticket", peer.base()))
            .bearer_auth(peer.key()),
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
    app.midi.may_hear(super::CENTRE, caller.zone).map_err(ApiError::forbidden)?;
    let ticket = app.midi.tickets.issue(super::CENTRE, caller.zone);
    Ok(created(json!({ "ticket": ticket, "expiresIn": TICKET_LIFETIME.as_secs(), "here": true })))
}

/// The station's own build has no station's window of its own.
#[cfg(not(feature = "pc"))]
async fn station_ticket(_app: &App, _caller: &Caller, _station: &str, _package: &str) -> ApiResult<serde_json::Value> {
    Err(ApiError::not_found(crate::tr!("Такой колонки у этого компьютера нет.")))
}

#[derive(Debug, Deserialize)]
struct SocketQuery {
    #[serde(default)]
    ticket: Option<String>,
}

/// The channel of notes, by a ticket; the page is still at home and the app may still hear. Where a Турбо has a
/// keyboard, the relay: this gateway's own and every Турбо's at home in one channel.
async fn socket(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(query): Query<SocketQuery>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let (package, _) = query
        .ticket
        .as_deref()
        .and_then(|ticket| app.midi.tickets.take(ticket))
        .ok_or_else(|| ApiError::unauthorized(crate::tr!("Билет канала нот не подошёл: возьмите новый.")))?;
    // The station's window here: this computer's own keyboard, as its ticket said.
    if super::own_owner(&package) {
        app.midi.may_hear(&package, caller.zone).map_err(ApiError::forbidden)?;
        return Ok(super::socket::upgrade(upgrade, app.midi.clone(), package, caller.zone));
    }
    may_hear(&app, &package, caller.zone).map_err(ApiError::forbidden)?;
    let places = places(&app);
    let only_here = places.iter().all(|place| place.id == HERE);
    if only_here {
        return Ok(super::socket::upgrade(upgrade, app.midi.clone(), package, caller.zone));
    }
    let zone = caller.zone;
    let check = {
        let (app, package) = (app.clone(), package.clone());
        Arc::new(move || may_hear(&app, &package, zone))
    };
    let choose = {
        let house = app.house.clone();
        Arc::new(move || house.choose("midi", "").ok().map(|choice| (choice.place, choice.said)))
    };
    Ok(relay::upgrade(upgrade, Relay { places, package, zone, check, choose }))
}

/// A `.mid` read into tracks with their melodies; the file is kept nowhere. A centre whose build has no MIDI asks a
/// Турбо that has.
async fn read(State(app): State<Arc<App>>, caller: Caller, bytes: Bytes) -> ApiResult<Value> {
    may_use(&caller)?;
    if !cfg!(feature = "midi") && at_turbo(&app) {
        let song = app.house.read_midi(bytes.to_vec()).await;
        return song.map(ok).map_err(|why| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, why));
    }
    let reading = tokio::task::spawn_blocking(move || super::file::read(&bytes));
    match tokio::time::timeout(Duration::from_secs(1), reading).await {
        Ok(Ok(Ok(song))) => Ok(ok(serde_json::to_value::<SongView>(song).unwrap_or(Value::Null))),
        Ok(Ok(Err(why))) => Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, why)),
        _ => Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, crate::tr!("Файл не прочитался за секунду."))),
    }
}
