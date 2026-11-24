//! Where the gateway runs, for both panels (docs/plans/station-placement.md):
//!
//! - `GET /api/v1/availability` — what works where, now, and the comparison of the three placements;
//! - `GET /api/v1/placement` — this gateway's role, placement, state record; on the computer the station's gateway as
//!   it sees it, the move going now, its own addresses as a station dials them;
//! - `POST /api/v1/placement/settings-home|safety` — where the main settings live, the safety net;
//! - on the station: `give-back` (the station to Yandex, without a computer), `point-local` (its voice back to its own
//!   gateway), `restart` (the pair's key only);
//! - on the computer: `move`, `move/{id}`, `backup`, `restore`, `auto-update`, `update-station`, `inspect`, `retry`;
//! - `GET|POST /api/v1/transfer` — the settings package: sealed with the pair's key between the two gateways, plain
//!   for an admin who downloads or uploads one. A package received is applied at a restart;
//! - `GET|PUT /api/v1/gateway/self` — this gateway's own id and name (src/core/identity/home.rs).
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Query};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::availability::AvailabilityView;
use super::service::Role;
use super::station_fs::{RealSystem, StationFs, Target};
use super::transfer::{self, Bundle};
use crate::config::Config;
use crate::core::availability::Placement;
use crate::core::events::EventBus;
use crate::core::identity::home::{GatewayView, Identity};
use crate::core::identity::permissions::{SETTINGS_EDIT, STATIONS_MANAGE, STATION_VIEW, USERS_MANAGE};
use crate::placement::{AvailabilityService, PlacementService};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Uses};
use crate::plugins::files::PluginFiles;
use crate::secrets::SecretsService;
use crate::settings::definitions::GatewayState;
use crate::storage::Db;

/// A settings package and a sealed one are small (tens of KB), a few MB at most with many people and servers.
pub const BODY_LIMIT: usize = 8 * 1024 * 1024;

pub fn router<S: crate::placement::RouteNeeds>() -> Router<S> {
    let router = Router::new()
        .route("/api/v1/availability", get(availability))
        .route("/api/v1/placement", get(view))
        .route("/api/v1/placement/safety", post(safety))
        .route("/api/v1/placement/give-back", post(give_back))
        .route("/api/v1/placement/point-local", post(point_local))
        .route("/api/v1/placement/restart", post(restart))
        .route("/api/v1/placement/pair", post(pair))
        .route("/api/v1/transfer", get(export).post(import))
        .route("/api/v1/gateway/self", get(gateway_self).put(rename));
    // A computer's own: moves, its stations' updates, pairing and its helper (crates/duowave-pc, placement::routes).
    router.layer(DefaultBodyLimit::max(BODY_LIMIT))
}

/// The station gives its pair's key to its admin, from the home network: the computer that knows the admin's password
/// becomes its helper without SSH and without copying PEER_KEY by hand (docs/plans/desk.md, stage 4). A station whose
/// .env has no key makes one now and keeps it. The key is not taken back yet (the plan's «Открыто»), so it goes only to
/// a person signed in here as the admin — not to a key, nor through the computer's pass.
async fn pair(Uses(placement): Uses<Arc<PlacementService>>, caller: Caller) -> ApiResult<Value> {
    use crate::core::identity::principal::PrincipalKind;
    use crate::core::identity::roles::Role as PersonRole;
    caller.zone_at_least(crate::platform::http::Zone::Home)?;
    if super::peer::from_peer(&caller, placement.peer_key()) {
        return Err(ApiError::forbidden(crate::tr!(
            "Ключ пары отдаётся администратору, вошедшему в пульт самой колонки."
        )));
    }
    let principal = caller.permitted(USERS_MANAGE)?;
    if principal.kind != PrincipalKind::User || principal.role != Some(PersonRole::Admin) {
        return Err(ApiError::forbidden(crate::tr!(
            "Ключ пары отдаётся только администратору колонки, вошедшему по паролю."
        )));
    }
    if placement.role() != Role::Station {
        return Err(ApiError::conflict(crate::tr!("Ключ пары отдаёт колонка, а это компьютер.")));
    }
    let key = placement.pair_key()?;
    tracing::info!(target: "placement", "pair: the key given to a computer from {}", caller.source);
    Ok(ok(json!({ "key": key })))
}

async fn availability(
    Uses(availability): Uses<Arc<AvailabilityService>>,
    caller: Caller,
) -> ApiResult<AvailabilityView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(availability.view()))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlacementView {
    role: Role,
    placement: Placement,
    peer_api: u32,
    state: GatewayState,
    /// This gateway's own part of /health: on the station its space, memory, voice, home server.
    health: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    managed_by: Option<String>,
    /// The computer (`PlacementComputer::view`): the station asked about (`?station=`), else the one with a placement
    /// here when it is the only one — its gateway as the computer sees it —, every station with a placement here
    /// (`stations`), the move going now (`job`), the computer as a station would dial it (`homeAddresses`).
    #[serde(flatten)]
    computer: serde_json::Map<String, Value>,
}

/// Which station a request is about: `?station=` (its device, or «at:<address>»); none — the only one with a
/// placement on this computer.
#[derive(Debug, Default, Deserialize)]
pub struct Pick {
    #[serde(default)]
    pub station: Option<String>,
}

async fn view(
    Uses(placement_service): Uses<Arc<PlacementService>>,
    caller: Caller,
    Query(pick): Query<Pick>,
) -> ApiResult<PlacementView> {
    caller.permitted(STATION_VIEW)?;
    let placement = &placement_service;
    Ok(ok(PlacementView {
        role: placement.role(),
        placement: crate::placement::PlacementSource::placement(placement.as_ref()),
        peer_api: placement.peer_api(),
        state: placement.state(),
        health: placement.health_value(),
        managed_by: placement.managed_by(),
        computer: match placement.computer() {
            Some(computer) if placement.role() == Role::Pc => computer.view(pick.station.as_deref()),
            _ => serde_json::Map::new(),
        },
    }))
}

/// The safety net on or off: on the station itself; from the computer, told to the station.
async fn safety(
    Uses(placement): Uses<Arc<PlacementService>>,
    caller: Caller,
    Query(pick): Query<Pick>,
    Body(body): Body<Value>,
) -> ApiResult<Value> {
    let on = body
        .get("on")
        .and_then(Value::as_bool)
        .ok_or_else(|| ApiError::bad_request(crate::tr!("on: true или false")))?;
    match placement.role() {
        Role::Station => {
            super::peer::peer_or_permitted(&caller, placement.peer_key(), SETTINGS_EDIT)?;
            Ok(created(json!({ "state": placement.set_safety(on, caller.actor())? })))
        }
        Role::Pc => {
            caller.permitted(STATIONS_MANAGE)?;
            if let Some(computer) = placement.computer() {
                computer.safety(pick.station.as_deref(), on).await?;
            }
            Ok(created(json!({ "on": on })))
        }
    }
}

fn on_station(placement: &Arc<PlacementService>) -> Result<(), ApiError> {
    if placement.role() != Role::Station {
        return Err(ApiError::conflict(crate::tr!("Это делает хаб на самой колонке.")));
    }
    // Its voice switched in the station's files, then a reboot: none of it on a pretend station.
    if crate::stations::pretend::is_station() {
        return Err(crate::stations::pretend::refuse());
    }
    Ok(())
}

/// «Отдать голос Яндексу» from the station's own panel: no computer needed — the gateway runs as root on the
/// station and changes quasar.cfg itself (a dated copy, the check), then the station reboots. Its gateway keeps
/// running for its panel: «Вернуть голос DuoWave» undoes it.
async fn give_back(
    Uses(placement): Uses<Arc<PlacementService>>,
    Uses(events): Uses<EventBus>,
    caller: Caller,
) -> ApiResult<Value> {
    on_station(&placement)?;
    super::peer::peer_or_permitted(&caller, placement.peer_key(), STATIONS_MANAGE)?;
    let pointed = tokio::task::spawn_blocking(|| StationFs::real().point(&Target::Yandex, true, &RealSystem))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::conflict)?;
    events.emit(crate::core::events::GatewayEvent::StationSwitched {
        address: "127.0.0.1".into(),
        to: "yandex".into(),
        result: "done".into(),
        url: pointed.to.url.clone(),
        device: None,
        error: None,
        actor: caller.actor(),
    });
    Ok(created(json!({ "pointed": pointed })))
}

/// «Вернуть голос DuoWave»: the station's voice back to its own gateway, then a reboot.
async fn point_local(
    Uses(placement): Uses<Arc<PlacementService>>,
    Uses(events): Uses<EventBus>,
    caller: Caller,
) -> ApiResult<Value> {
    on_station(&placement)?;
    super::peer::peer_or_permitted(&caller, placement.peer_key(), STATIONS_MANAGE)?;
    let pointed = tokio::task::spawn_blocking(|| StationFs::real().point(&Target::Local, true, &RealSystem))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::conflict)?;
    events.emit(crate::core::events::GatewayEvent::StationSwitched {
        address: "127.0.0.1".into(),
        to: "gateway".into(),
        result: "done".into(),
        url: pointed.to.url.clone(),
        device: None,
        error: None,
        actor: caller.actor(),
    });
    Ok(created(json!({ "pointed": pointed })))
}

/// The computer asks the station's gateway to start again (new settings are applied at the start).
async fn restart(Uses(placement): Uses<Arc<PlacementService>>, caller: Caller) -> ApiResult<Value> {
    if !super::peer::from_peer(&caller, placement.peer_key()) {
        return Err(ApiError::forbidden(crate::tr!("Только для компьютера пары.")));
    }
    placement.restart();
    Ok(created(json!({ "restarting": true })))
}

/// This gateway as its panel's rename reads it: its own id, the name people see, the default and its home.
async fn gateway_self(Uses(identity): Uses<Arc<Identity>>, caller: Caller) -> ApiResult<GatewayView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(identity.view()?))
}

/// A person's name for this gateway, `{"name": "Кухня"}`; empty or null — the default again. The name is this gateway's
/// own, not a setting the computer sends: a station whose settings live on the computer is renamed too — in its panel,
/// or from the computer through its pass (`/station/<key>/api/v1/gateway/self`).
async fn rename(
    Uses(identity): Uses<Arc<Identity>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<GatewayView> {
    caller.permitted(SETTINGS_EDIT)?;
    let name = match body.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => Some(name.as_str()),
        Some(_) => return Err(ApiError::bad_request(crate::tr!("{name}: нужна строка.", name = "name"))),
    };
    Ok(ok(identity.rename(name, caller.actor())?))
}

/// What the package is asked with: `?plugins=1` — the plugins' state too (a move back takes it, moves.rs).
#[derive(Debug, Default, Deserialize)]
struct Asked {
    #[serde(default)]
    plugins: Option<String>,
}

/// The settings package: sealed for the computer of the pair, plain for an admin (it holds the keys of the clouds).
/// With `?plugins=1` the plugins' state goes in it as well — only a move carries that (transfer.rs).
async fn export(
    Uses(placement): Uses<Arc<PlacementService>>,
    Uses(db): Uses<Arc<Db>>,
    Uses(secrets): Uses<Arc<SecretsService>>,
    Uses(files): Uses<Arc<PluginFiles>>,
    caller: Caller,
    Query(asked): Query<Asked>,
) -> ApiResult<Value> {
    let peer = super::peer::peer_or_permitted(&caller, placement.peer_key(), USERS_MANAGE)?;
    let from = match placement.role() {
        Role::Pc => "pc",
        Role::Station => "station",
    };
    let mut bundle = transfer::export(&db, &secrets, from)?;
    bundle.revision = placement.settings_revision().unwrap_or(0);
    if asked.plugins.as_deref().is_some_and(|value| matches!(value, "1" | "true")) {
        bundle.plugins = Some(transfer::plugins(&db)?);
        // A move to the computer: every picture of the plugins goes along (plugins/files.rs).
        bundle.pictures = Some(transfer::pictures(files.root(), None));
    }
    if peer {
        transfer::note_sync(&db, "copied");
        let key = placement.peer_key().unwrap_or_default();
        return Ok(ok(transfer::seal(&bundle, key)));
    }
    Ok(ok(serde_json::to_value(bundle)?))
}

/// A settings package received: kept beside the database and applied as the gateway starts again, a moment later.
async fn import(
    Uses(placement): Uses<Arc<PlacementService>>,
    Uses(config): Uses<Arc<Config>>,
    Uses(db): Uses<Arc<Db>>,
    Uses(events): Uses<EventBus>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<Value> {
    let peer = super::peer::peer_or_permitted(&caller, placement.peer_key(), USERS_MANAGE)?;
    let bundle: Bundle = if peer {
        transfer::open(&body, placement.peer_key().unwrap_or_default()).map_err(ApiError::bad_request)?
    } else {
        serde_json::from_value(body).map_err(|_| ApiError::bad_request(crate::tr!("Это не набор настроек DuoWave.")))?
    };
    if bundle.format != transfer::FORMAT || bundle.schema > transfer::SCHEMA {
        return Err(ApiError::bad_request(transfer::NOT_FOR_THIS_GATEWAY));
    }
    let dir =
        config.skills.state_dir.clone().ok_or_else(|| {
            ApiError::conflict(crate::tr!("У DuoWave здесь нет папки данных: настройки не сохранить."))
        })?;
    if peer {
        transfer::note_sync(&db, "received");
    }
    let file = transfer::incoming_file(&dir);
    let text = serde_json::to_vec(&bundle)?;
    super::station_fs::write_atomic(&file, &text).map_err(ApiError::internal)?;
    tracing::info!("settings: a package from the {} received; the gateway starts again on it", bundle.from);
    events.emit(crate::core::events::GatewayEvent::GatewayStateChanged {
        what: format!("settings from the {}", bundle.from),
        actor: caller.actor(),
    });
    placement.restart();
    Ok(created(json!({ "applied": "restart" })))
}
