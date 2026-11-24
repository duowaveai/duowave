//! `/api/v1/home/plugged` (docs/plans/turbo-devices.md, «Пульт»): what is plugged into the home's computers, for
//! «Дом» → «Устройства» — by kind, each thing with its computer («Keystation 49 · Mac»), a sleeping one grey with
//! «Разбудить» (owner 20.11, 8); and, on a computer that is a Турбо, «Что этот компьютер даёт дому».
//!
//! - `GET /api/v1/home/plugged` — the home's kinds with their places; who may use the apps sees them.
//! - `POST /api/v1/home/plugged/choice {kind, place}` — where the house does a kind's acts from now on.
//! - `POST /api/v1/home/plugged/wake {place}` — a sleeping Турбо woken over the home network, waited for.
//! - `GET|PUT /api/v1/home/plugged/shares` — this computer's own devices and what it gives its home (`turbo_shares`):
//!   who sees the connectors sees it, who manages them changes it.
use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{GivenKind, HouseDevices, WAKE_WAIT};
use crate::core::events::EventBus;
use crate::core::identity::permissions::{use_of, CONNECTORS_MANAGE, CONNECTORS_VIEW};
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Uses};
use crate::settings::definitions::{TurboShares, TURBO_SHARES};
use crate::settings::SettingsService;
use crate::skills::capabilities::CapabilityName;

pub fn routes<S: crate::home::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/home/plugged", get(view))
        .route("/api/v1/home/plugged/choice", post(choice))
        .route("/api/v1/home/plugged/wake", post(wake))
        .route("/api/v1/home/plugged/shares", get(shares).put(set_shares))
}

fn may_use(caller: &Caller) -> Result<(), ApiError> {
    caller.permitted(&use_of(CapabilityName::Plugins)).map(|_| ())
}

/// The home's kinds with their places, and which sleeping places can be woken.
async fn view(Uses(house): Uses<Arc<HouseDevices>>, caller: Caller) -> ApiResult<Value> {
    may_use(&caller)?;
    let kinds = house.view();
    let wakeable: Vec<String> = kinds
        .iter()
        .flat_map(|kind| kind.places.iter())
        .filter(|place| place.state == super::PlaceState::Asleep && !house.macs_of(&place.place).is_empty())
        .map(|place| place.place.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(ok(json!({ "kinds": kinds, "wakeable": wakeable })))
}

#[derive(Debug, Deserialize)]
struct ChoiceBody {
    kind: String,
    place: String,
}

/// Where the house does a kind's acts from now on (the panel's choice is the house's, not a room's).
async fn choice(
    Uses(house): Uses<Arc<HouseDevices>>,
    caller: Caller,
    Body(body): Body<ChoiceBody>,
) -> ApiResult<Value> {
    may_use(&caller)?;
    let chosen =
        house.set_choice(&body.kind, "", &body.place).map_err(|error| ApiError::conflict(error.to_string()))?;
    house.used(&body.kind, &chosen.place);
    Ok(ok(json!({ "kind": body.kind, "place": chosen.place, "name": chosen.name })))
}

#[derive(Debug, Deserialize)]
struct WakeBody {
    place: String,
}

/// A sleeping Турбо woken, and whether it came on the line in time.
async fn wake(Uses(house): Uses<Arc<HouseDevices>>, caller: Caller, Body(body): Body<WakeBody>) -> ApiResult<Value> {
    may_use(&caller)?;
    let awake = house.wake(&body.place, WAKE_WAIT).await.map_err(ApiError::conflict)?;
    Ok(ok(json!({ "place": body.place, "awake": awake })))
}

/// One kind of this computer's devices as «Что этот компьютер даёт дому» shows it.
fn shares_view(settings: &SettingsService, house: &HouseDevices) -> Value {
    let ticked: TurboShares = settings.get(&TURBO_SHARES, None).unwrap_or_default();
    let kinds: Vec<Value> = house
        .local()
        .list()
        .iter()
        .map(|device| {
            let given = GivenKind::of(device.as_ref());
            json!({
                "kind": given.kind,
                "title": given.title,
                "nature": given.nature,
                "sensitive": device.sensitive(),
                "given": ticked.gives(device.name(), device.sensitive()),
                "ticked": ticked.kinds.get(device.name()),
                "instances": given.instances,
            })
        })
        .collect();
    json!({ "kinds": kinds })
}

async fn shares(
    Uses(settings): Uses<Arc<SettingsService>>,
    Uses(house): Uses<Arc<HouseDevices>>,
    caller: Caller,
) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(shares_view(&settings, &house)))
}

#[derive(Debug, Deserialize)]
struct SharesBody {
    /// Kind → given or not; a kind left out goes back to its default.
    kinds: std::collections::BTreeMap<String, bool>,
}

async fn set_shares(
    Uses(settings): Uses<Arc<SettingsService>>,
    Uses(house): Uses<Arc<HouseDevices>>,
    Uses(events): Uses<EventBus>,
    caller: Caller,
    Body(body): Body<SharesBody>,
) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_MANAGE)?;
    let kinds = body
        .kinds
        .into_iter()
        .filter(|(kind, _)| {
            !kind.is_empty() && kind.len() <= 40 && kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
        })
        .collect();
    settings.set(&TURBO_SHARES, &TurboShares { kinds }, caller.actor())?;
    // What this computer gives changed: its contract says so at its home's next look.
    events.emit(crate::core::events::GatewayEvent::HelpersChanged {
        id: Some("self".into()),
        change: "contract".into(),
        actor: None,
    });
    Ok(ok(shares_view(&settings, &house)))
}
