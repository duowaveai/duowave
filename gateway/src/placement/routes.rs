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
//!   for an admin who downloads or uploads one. A package received is applied at a restart.

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Query, State};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::availability::AvailabilityView;
use super::service::Role;
use super::station_fs::{RealSystem, StationFs, Target};
use super::transfer::{self, Bundle};
use crate::app::App;
use crate::core::availability::Placement;
use crate::core::identity::permissions::{SETTINGS_EDIT, STATIONS_MANAGE, STATION_VIEW, USERS_MANAGE};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller};
use crate::settings::definitions::{GatewayState, SettingsHome};

/// A settings package and a sealed one are small (tens of KB), a few MB at most with many people and servers.
const BODY_LIMIT: usize = 8 * 1024 * 1024;

pub fn router() -> Router<Arc<App>> {
    let router = Router::new()
        .route("/api/v1/availability", get(availability))
        .route("/api/v1/placement", get(view))
        .route("/api/v1/placement/settings-home", post(settings_home))
        .route("/api/v1/placement/safety", post(safety))
        .route("/api/v1/placement/give-back", post(give_back))
        .route("/api/v1/placement/point-local", post(point_local))
        .route("/api/v1/placement/restart", post(restart))
        .route("/api/v1/transfer", get(export).post(import));
    #[cfg(feature = "pc")]
    let router = router
        .route("/api/v1/placement/move", post(pc::start_move))
        .route("/api/v1/placement/move/{id}", get(pc::move_job))
        .route("/api/v1/placement/backup", post(pc::backup))
        .route("/api/v1/placement/restore", post(pc::restore))
        .route("/api/v1/placement/auto-update", post(pc::auto_update))
        .route("/api/v1/placement/update-station", post(pc::update_station))
        .route("/api/v1/placement/inspect", post(pc::inspect))
        .route("/api/v1/placement/station-voice", post(pc::station_voice))
        .route("/api/v1/placement/link", post(pc::link))
        .route("/api/v1/placement/helper", post(pc::helper))
        .route("/api/v1/placement/retry", post(pc::retry));
    router.layer(DefaultBodyLimit::max(BODY_LIMIT))
}

/// The settings a station's panel changes. When they live on the computer, the station works on the last copy it was
/// sent and these are read-only here: a change would be overwritten by the next one (section 10).
const SETTINGS_PATHS: &[&str] = &[
    "/api/v1/station/routes",
    "/api/v1/brain",
    "/api/v1/persona",
    "/api/v1/connectors",
    "/api/v1/mcp",
    "/api/v1/users",
    "/api/v1/api-keys",
    "/api/v1/household",
    "/api/v1/stations",
    "/api/v1/access",
];

pub const READ_ONLY: &str =
    "Настройки этой колонки хранятся на компьютере: меняйте их в пульте компьютера, он пришлёт их колонке.";

/// Refuses a change of the settings on the station while they live on the computer; the computer's own pushes pass.
pub async fn guard(
    State(app): State<Arc<App>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::Method;
    use axum::response::IntoResponse;
    let changing = !matches!(*request.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if changing
        && app.placement.role() == Role::Station
        && SETTINGS_PATHS.iter().any(|path| request.uri().path().starts_with(path))
        && app.placement.state().settings_home == SettingsHome::Pc
    {
        let (mut parts, body) = request.into_parts();
        let peer =
            match <Caller as axum::extract::FromRequestParts<Arc<App>>>::from_request_parts(&mut parts, &app).await {
                Ok(caller) => super::peer::from_peer(&caller, app.placement.peer_key()),
                Err(_) => false,
            };
        if !peer {
            return ApiError::conflict(READ_ONLY).into_response();
        }
        return next.run(axum::extract::Request::from_parts(parts, body)).await;
    }
    next.run(request).await
}

async fn availability(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<AvailabilityView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(app.availability.view()))
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
    /// The computer: the station asked about (`?station=`), else the one with a placement here when it is the only
    /// one — its gateway as the computer sees it.
    #[cfg(feature = "pc")]
    #[serde(skip_serializing_if = "Option::is_none")]
    station: Option<super::watch::StationGatewayView>,
    /// The computer: every station with a placement here.
    #[cfg(feature = "pc")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stations: Vec<super::watch::StationGatewayView>,
    /// The computer: the move going now.
    #[cfg(feature = "pc")]
    #[serde(skip_serializing_if = "Option::is_none")]
    job: Option<super::moves::MoveJob>,
    /// The computer as a station would dial it, for the choice.
    #[cfg(feature = "pc")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    home_addresses: Vec<String>,
}

/// Which station a request is about: `?station=` (its device, or «at:<address>»); none — the only one with a
/// placement on this computer.
#[derive(Debug, Default, Deserialize)]
#[cfg_attr(not(feature = "pc"), allow(dead_code))]
struct Pick {
    #[serde(default)]
    station: Option<String>,
}

#[cfg(feature = "pc")]
fn chosen(app: &App, pick: &Pick) -> Result<Arc<super::watch::StationGateway>, ApiError> {
    app.station_gateway
        .pick(pick.station.as_deref())
        .ok_or_else(|| ApiError::conflict("Какая колонка? У нескольких свои шлюзы — укажите её (station)."))
}

#[cfg_attr(not(feature = "pc"), allow(unused_variables))]
async fn view(State(app): State<Arc<App>>, caller: Caller, Query(pick): Query<Pick>) -> ApiResult<PlacementView> {
    caller.permitted(STATION_VIEW)?;
    let placement = &app.placement;
    Ok(ok(PlacementView {
        role: placement.role(),
        placement: crate::placement::PlacementSource::placement(placement.as_ref()),
        peer_api: placement.peer_api(),
        state: placement.state(),
        health: placement.health_value(),
        managed_by: placement.managed_by(),
        #[cfg(feature = "pc")]
        station: if placement.role() == Role::Pc {
            app.station_gateway.pick(pick.station.as_deref()).map(|station| station.view())
        } else {
            None
        },
        #[cfg(feature = "pc")]
        stations: if placement.role() == Role::Pc { app.station_gateway.views() } else { Vec::new() },
        #[cfg(feature = "pc")]
        job: if placement.role() == Role::Pc { app.moves.running() } else { None },
        #[cfg(feature = "pc")]
        home_addresses: if placement.role() == Role::Pc {
            app.station_connect.home_addresses(
                &crate::stations::connect::station_connect::host_interfaces(),
                crate::stations::connect::station_connect::in_container(),
            )
        } else {
            Vec::new()
        },
    }))
}

fn home_of(body: &Value) -> Result<SettingsHome, ApiError> {
    match body.get("home").and_then(Value::as_str) {
        Some("station") => Ok(SettingsHome::Station),
        Some("pc") => Ok(SettingsHome::Pc),
        _ => Err(ApiError::bad_request("home: station или pc")),
    }
}

/// Where the main settings live. The station's gateway keeps the choice in its record (the computer tells it); the
/// computer keeps it in its record of the station and tells the station.
#[cfg_attr(not(feature = "pc"), allow(unused_variables))]
async fn settings_home(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(pick): Query<Pick>,
    Body(body): Body<Value>,
) -> ApiResult<Value> {
    let home = home_of(&body)?;
    match app.placement.role() {
        Role::Station => {
            super::peer::peer_or_permitted(&caller, app.placement.peer_key(), SETTINGS_EDIT)?;
            let state = app.placement.set_settings_home(home, caller.actor())?;
            Ok(created(json!({ "state": state })))
        }
        Role::Pc => {
            caller.permitted(STATIONS_MANAGE)?;
            #[cfg(feature = "pc")]
            chosen(&app, &pick)?.set_settings_home(home, caller.actor()).await.map_err(ApiError::conflict)?;
            Ok(created(json!({ "home": body["home"] })))
        }
    }
}

/// The safety net on or off: on the station itself; from the computer, told to the station.
#[cfg_attr(not(feature = "pc"), allow(unused_variables))]
async fn safety(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(pick): Query<Pick>,
    Body(body): Body<Value>,
) -> ApiResult<Value> {
    let on = body.get("on").and_then(Value::as_bool).ok_or_else(|| ApiError::bad_request("on: true или false"))?;
    match app.placement.role() {
        Role::Station => {
            super::peer::peer_or_permitted(&caller, app.placement.peer_key(), SETTINGS_EDIT)?;
            Ok(created(json!({ "state": app.placement.set_safety(on, caller.actor())? })))
        }
        Role::Pc => {
            caller.permitted(STATIONS_MANAGE)?;
            #[cfg(feature = "pc")]
            {
                let client = chosen(&app, &pick)?.client().ok_or_else(|| ApiError::conflict("Шлюз не на колонке."))?;
                client.post("/api/v1/placement/safety", json!({ "on": on })).await.map_err(ApiError::unavailable)?;
            }
            Ok(created(json!({ "on": on })))
        }
    }
}

fn on_station(app: &App) -> Result<(), ApiError> {
    if app.placement.role() != Role::Station {
        return Err(ApiError::conflict("Это делает шлюз на самой колонке."));
    }
    Ok(())
}

/// «Отдать голос Яндексу» from the station's own panel: no computer needed — the gateway runs as root on the
/// station and changes quasar.cfg itself (a dated copy, the check), then the station reboots. Its gateway keeps
/// running for its panel: «Вернуть голос DuoWave» undoes it.
async fn give_back(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    on_station(&app)?;
    super::peer::peer_or_permitted(&caller, app.placement.peer_key(), STATIONS_MANAGE)?;
    let pointed = tokio::task::spawn_blocking(|| StationFs::real().point(&Target::Yandex, true, &RealSystem))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::conflict)?;
    app.events.emit(crate::core::events::GatewayEvent::StationSwitched {
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
async fn point_local(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    on_station(&app)?;
    super::peer::peer_or_permitted(&caller, app.placement.peer_key(), STATIONS_MANAGE)?;
    let pointed = tokio::task::spawn_blocking(|| StationFs::real().point(&Target::Local, true, &RealSystem))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::conflict)?;
    app.events.emit(crate::core::events::GatewayEvent::StationSwitched {
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
async fn restart(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    if !super::peer::from_peer(&caller, app.placement.peer_key()) {
        return Err(ApiError::forbidden("Только для компьютера пары."));
    }
    app.placement.restart();
    Ok(created(json!({ "restarting": true })))
}

/// The settings package: sealed for the computer of the pair, plain for an admin (it holds the keys of the clouds).
async fn export(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    let peer = super::peer::peer_or_permitted(&caller, app.placement.peer_key(), USERS_MANAGE)?;
    let from = match app.placement.role() {
        Role::Pc => "pc",
        Role::Station => "station",
    };
    let mut bundle = transfer::export(&app.db, &app.secrets, from)?;
    bundle.revision = app.placement.settings_revision().unwrap_or(0);
    if peer {
        transfer::note_sync(&app.db, "copied");
        let key = app.placement.peer_key().unwrap_or_default();
        return Ok(ok(transfer::seal(&bundle, key)));
    }
    Ok(ok(serde_json::to_value(bundle)?))
}

/// A settings package received: kept beside the database and applied as the gateway starts again, a moment later.
async fn import(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<Value> {
    let peer = super::peer::peer_or_permitted(&caller, app.placement.peer_key(), USERS_MANAGE)?;
    let bundle: Bundle = if peer {
        transfer::open(&body, app.placement.peer_key().unwrap_or_default()).map_err(ApiError::bad_request)?
    } else {
        serde_json::from_value(body).map_err(|_| ApiError::bad_request("Это не пакет настроек шлюза."))?
    };
    if bundle.format != transfer::FORMAT || bundle.schema > transfer::SCHEMA {
        return Err(ApiError::bad_request("Пакет настроек не для этого шлюза: обновите его."));
    }
    let dir = app
        .config
        .skills
        .state_dir
        .clone()
        .ok_or_else(|| ApiError::conflict("У этого шлюза нет папки данных: настройки не сохранить."))?;
    if peer {
        transfer::note_sync(&app.db, "received");
    }
    let file = transfer::incoming_file(&dir);
    let text = serde_json::to_vec(&bundle)?;
    super::station_fs::write_atomic(&file, &text).map_err(ApiError::internal)?;
    tracing::info!("settings: a package from the {} received; the gateway starts again on it", bundle.from);
    app.events.emit(crate::core::events::GatewayEvent::GatewayStateChanged {
        what: format!("settings from the {}", bundle.from),
        actor: caller.actor(),
    });
    app.placement.restart();
    Ok(created(json!({ "applied": "restart" })))
}

#[cfg(feature = "pc")]
mod pc {
    use super::*;
    use crate::core::identity::permissions::SYSTEM_UPDATE;
    use crate::placement::moves::{MoveJob, MoveRequest, MoveTo};
    use crate::stations::connect::quasar_config::{parse_gateway_host, parse_gateway_host_or, plain_address};

    pub async fn start_move(
        State(app): State<Arc<App>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> ApiResult<MoveJob> {
        caller.permitted(STATIONS_MANAGE)?;
        let to: MoveTo = serde_json::from_value(body.get("to").cloned().unwrap_or(Value::Null))
            .map_err(|_| ApiError::bad_request("to: station, stationServer, pc или yandex"))?;
        let picked = body.get("station").and_then(Value::as_str);
        let record = app.station_gateway.pick(picked).and_then(|station| station.record());
        let address = body
            .get("address")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| record.as_ref().map(|record| record.address.clone()))
            .and_then(|address| plain_address(&address))
            .ok_or_else(|| ApiError::bad_request("address: адрес колонки, IPv4"))?;
        let gateway = match body.get("gateway").and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty()) {
            // Typed without a port: this gateway's own port, not 80.
            Some(text) => Some(
                parse_gateway_host_or(text, app.station_connect.port())
                    .ok_or_else(|| ApiError::bad_request("gateway: адрес[:порт]"))?,
            ),
            // Nobody typed it: this computer's address in the station's network.
            None if to != MoveTo::Yandex => {
                let addresses = app.station_connect.home_addresses(
                    &crate::stations::connect::station_connect::host_interfaces(),
                    crate::stations::connect::station_connect::in_container(),
                );
                crate::placement::watch::pick_home(&addresses, &address).and_then(|text| parse_gateway_host(&text))
            }
            None => None,
        };
        let settings_home = match body.get("settingsHome") {
            None | Some(Value::Null) => SettingsHome::Station,
            Some(_) => home_of(&json!({ "home": body["settingsHome"] }))?,
        };
        let request = MoveRequest {
            to,
            address,
            gateway,
            settings_home,
            remove: body.get("remove").and_then(Value::as_bool).unwrap_or(false),
        };
        Ok(created(app.moves.start(request, caller.actor())?))
    }

    pub async fn move_job(
        State(app): State<Arc<App>>,
        caller: Caller,
        axum::extract::Path(id): axum::extract::Path<String>,
    ) -> ApiResult<MoveJob> {
        caller.permitted(STATION_VIEW)?;
        Ok(ok(app.moves.job(&id)?))
    }

    pub async fn backup(State(app): State<Arc<App>>, caller: Caller, Query(pick): Query<Pick>) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let entry = chosen(&app, &pick)?.backup_now().await.map_err(ApiError::unavailable)?;
        Ok(created(json!({ "backup": entry })))
    }

    pub async fn restore(
        State(app): State<Arc<App>>,
        caller: Caller,
        Query(pick): Query<Pick>,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let id = body.get("id").and_then(Value::as_str).ok_or_else(|| ApiError::bad_request("id: какая копия"))?;
        chosen(&app, &pick)?.restore(id, caller.actor()).await.map_err(ApiError::unavailable)?;
        Ok(created(json!({ "restored": id })))
    }

    pub async fn auto_update(
        State(app): State<Arc<App>>,
        caller: Caller,
        Query(pick): Query<Pick>,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(SYSTEM_UPDATE)?;
        let on = body.get("on").and_then(Value::as_bool).ok_or_else(|| ApiError::bad_request("on: true или false"))?;
        chosen(&app, &pick)?.set_auto_update(on, caller.actor())?;
        Ok(created(json!({ "on": on })))
    }

    pub async fn update_station(
        State(app): State<Arc<App>>,
        caller: Caller,
        Query(pick): Query<Pick>,
    ) -> ApiResult<Value> {
        caller.permitted(SYSTEM_UPDATE)?;
        chosen(&app, &pick)?.start_update(caller.actor())?;
        Ok(created(json!({ "updating": true })))
    }

    fn station_address(app: &App, pick: &Pick) -> Result<String, ApiError> {
        chosen(app, pick)?
            .record()
            .map(|record| record.address)
            .ok_or_else(|| ApiError::conflict("Шлюз не на колонке."))
    }

    /// «Что случилось»: the station's own account, over SSH.
    pub async fn inspect(State(app): State<Arc<App>>, caller: Caller, Query(pick): Query<Pick>) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = station_address(&app, &pick)?;
        Ok(created(app.moves.inspect(&address).await.map_err(ApiError::unavailable)?))
    }

    /// The station's voice from the computer's panel: back to its own gateway, or to Yandex — the station's gateway
    /// does it, asked with the pair's key.
    pub async fn station_voice(
        State(app): State<Arc<App>>,
        caller: Caller,
        Query(pick): Query<Pick>,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let path = match body.get("to").and_then(Value::as_str) {
            Some("local") => "/api/v1/placement/point-local",
            Some("yandex") => "/api/v1/placement/give-back",
            _ => return Err(ApiError::bad_request("to: local или yandex")),
        };
        let client = chosen(&app, &pick)?.client().ok_or_else(|| ApiError::conflict("Шлюз не на колонке."))?;
        let answer = client.post(path, json!({})).await.map_err(ApiError::unavailable)?;
        Ok(created(answer))
    }

    /// A gateway put on the station by hand (yse, `duowave station install`): the computer links to it with its
    /// address and the pair's key, without SSH — it then watches it, keeps its settings' copies, updates it.
    pub async fn link(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = body
            .get("address")
            .and_then(Value::as_str)
            .and_then(plain_address)
            .ok_or_else(|| ApiError::bad_request("address: адрес колонки, IPv4"))?;
        let port =
            body.get("port").and_then(Value::as_u64).filter(|port| (1..=65535).contains(port)).unwrap_or(8790) as u16;
        let key = body.get("key").and_then(Value::as_str).map(str::trim).unwrap_or("");
        if key.len() < 16 {
            return Err(ApiError::bad_request("key: ключ пары — PEER_KEY из .env шлюза на колонке"));
        }
        let view = app.station_gateway.link(&address, port, key, caller.actor()).await.map_err(ApiError::conflict)?;
        Ok(created(serde_json::to_value(view)?))
    }

    /// «ПК помогает колонке»: on — the station leans on this computer (a key made here first, if it has none of
    /// ours); off — it works alone, the address and the key kept.
    pub async fn helper(
        State(app): State<Arc<App>>,
        caller: Caller,
        Query(pick): Query<Pick>,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let station = chosen(&app, &pick)?;
        let on = body.get("on").and_then(Value::as_bool).ok_or_else(|| ApiError::bad_request("on: true или false"))?;
        let key = if on && station.needs_home_key() {
            let created = app.access.keys.create(
                &serde_json::Map::from_iter([
                    ("name".into(), json!("Колонка: домашний сервер")),
                    ("grants".into(), json!(["home.models"])),
                ]),
                caller.actor(),
            )?;
            Some((created.secret, created.key.id))
        } else {
            None
        };
        station.set_helper(on, key, caller.actor()).await.map_err(ApiError::unavailable)?;
        Ok(created(serde_json::to_value(station.view())?))
    }

    /// «Попробовать снова» after the safety net.
    pub async fn retry(State(app): State<Arc<App>>, caller: Caller, Query(pick): Query<Pick>) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = station_address(&app, &pick)?;
        app.moves.retry(&address).await.map_err(ApiError::unavailable)?;
        Ok(created(json!({ "retried": true })))
    }
}
