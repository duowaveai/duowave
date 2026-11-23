//! Rooms (docs/plans/home-and-devices.md, «Комнаты», stage 2): inside one home only these may differ by device — its
//! name and room (its record in `stations`), Alice's mode, the volume and, if wished, who answers there. Everything
//! else — memory, people, keys, apps, «Мозг» — is the home's. Kept at the centre in the setting `device_rooms`; the
//! station's session takes its room's mode (src/gateway/mode), the answer its room's character
//! (src/dialog/persona_service.rs).
//!
//! `GET /api/v1/rooms` → `{rooms: {<device>: {mode, volume, character}}}`; `PUT /api/v1/rooms/<device>`
//! `{mode?, volume?, character?}` — a field `null` goes back to «как во всём доме».
use std::sync::Arc;

use axum::extract::Path;
use axum::routing::{get, put};
use axum::Router;
use serde_json::{json, Map, Value};

use crate::core::events::GatewayEvent;
use crate::core::identity::permissions::{SETTINGS_EDIT, STATION_VIEW};
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Uses};
use crate::settings::definitions::{PanelMode, ROOMS};

pub fn routes<S: crate::home::RouteNeeds>() -> Router<S> {
    Router::new().route("/api/v1/rooms", get(list)).route("/api/v1/rooms/{device}", put(change))
}

async fn list(Uses(settings): Uses<Arc<crate::settings::SettingsService>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(STATION_VIEW)?;
    let rooms = settings.get(&ROOMS, None).unwrap_or_default();
    let shown: Map<String, Value> = rooms
        .into_iter()
        .map(|(device, room)| {
            (device, json!({ "mode": room.mode, "volume": room.volume, "character": room.character }))
        })
        .collect();
    Ok(ok(json!({ "rooms": shown })))
}

/// One field of the change: absent — as it was; `null` — the home's; a value — the room's own.
enum Field<T> {
    Kept,
    Home,
    Own(T),
}

fn field<T>(
    body: &Map<String, Value>,
    name: &str,
    read: impl Fn(&Value) -> Option<T>,
    what: &str,
) -> Result<Field<T>, ApiError> {
    match body.get(name) {
        None => Ok(Field::Kept),
        Some(Value::Null) => Ok(Field::Home),
        Some(value) => read(value).map(Field::Own).ok_or_else(|| ApiError::bad_request(what.to_string())),
    }
}

/// A room's change reaches the stations, the settings and the mode, each an extractor of its own.
#[allow(clippy::too_many_arguments)]
async fn change(
    Uses(stations): Uses<Arc<crate::stations::StationsService>>,
    Uses(settings): Uses<Arc<crate::settings::SettingsService>>,
    Uses(modes): Uses<Arc<crate::gateway::mode::ModeService>>,
    Uses(characters): Uses<Arc<crate::characters::CharactersService>>,
    Uses(events): Uses<crate::core::events::EventBus>,
    caller: Caller,
    Path(device): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<Value> {
    caller.permitted(SETTINGS_EDIT)?;
    let Value::Object(body) = body else {
        return Err(ApiError::bad_request(crate::tr!("Комната — объект JSON: mode, volume, character.")));
    };
    if stations.find(&device).is_none() && device != "computer" {
        return Err(ApiError::not_found(crate::tr!("Такого устройства в доме нет.")));
    }
    let mode = field(&body, "mode", |value| value.as_str().and_then(PanelMode::parse), "mode: relay, gateway, own")?;
    let volume =
        field(&body, "volume", |value| value.as_u64().filter(|volume| *volume <= 10).map(|v| v as u8), "volume: 0–10")?;
    let character = field(&body, "character", |value| value.as_str().map(str::to_string), "character: id")?;
    let mut rooms = settings.get(&ROOMS, None).unwrap_or_default();
    let mut room = rooms.remove(&device).unwrap_or_default();
    let mut fields = Vec::new();
    match mode {
        Field::Kept => {}
        Field::Home => {
            room.mode = None;
            fields.push("mode".to_string());
        }
        Field::Own(mode) => {
            if let Err(why) = modes.possible_now(mode) {
                return Err(ApiError::conflict(why));
            }
            room.mode = Some(mode);
            fields.push("mode".to_string());
        }
    }
    match volume {
        Field::Kept => {}
        Field::Home => {
            room.volume = None;
            fields.push("volume".to_string());
        }
        Field::Own(volume) => {
            room.volume = Some(volume);
            fields.push("volume".to_string());
        }
    }
    match character {
        Field::Kept => {}
        Field::Home => {
            room.character = None;
            room.persona = None;
            fields.push("character".to_string());
        }
        Field::Own(id) => {
            room.persona = Some(characters.persona_for(&id)?);
            room.character = Some(id);
            fields.push("character".to_string());
        }
    }
    if !room.is_empty() {
        rooms.insert(device.clone(), room.clone());
    }
    settings.set(&ROOMS, &rooms, caller.actor())?;
    events.emit(GatewayEvent::RoomChanged { device: device.clone(), fields, actor: caller.actor() });
    Ok(ok(json!({ "device": device, "mode": room.mode, "volume": room.volume, "character": room.character })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_is_kept_given_back_to_the_home_or_the_rooms_own() {
        let body: Map<String, Value> = serde_json::from_value(json!({ "mode": null, "volume": 4 })).unwrap();
        let read = |value: &Value| value.as_u64();
        assert!(matches!(field(&body, "mode", |value| value.as_str().map(str::to_string), "m").unwrap(), Field::Home));
        assert!(matches!(field(&body, "volume", read, "v").unwrap(), Field::Own(4)));
        assert!(matches!(field(&body, "character", read, "c").unwrap(), Field::Kept));
        let wrong: Map<String, Value> = serde_json::from_value(json!({ "volume": "loud" })).unwrap();
        assert!(field(&wrong, "volume", read, "v").is_err());
    }
}
