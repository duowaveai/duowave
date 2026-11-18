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
use crate::core::identity::home::GatewayView;
use crate::core::identity::permissions::{SETTINGS_EDIT, STATIONS_MANAGE, STATION_VIEW, USERS_MANAGE};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller};
use crate::settings::definitions::GatewayState;

/// A settings package and a sealed one are small (tens of KB), a few MB at most with many people and servers.
const BODY_LIMIT: usize = 8 * 1024 * 1024;

pub fn router() -> Router<Arc<App>> {
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
    #[cfg(feature = "pc")]
    let router = router
        .route("/api/v1/placement/move", post(pc::start_move))
        .route("/api/v1/placement/move/{id}", get(pc::move_job))
        .route("/api/v1/placement/auto-update", post(pc::auto_update))
        .route("/api/v1/placement/update-station", post(pc::update_station))
        .route("/api/v1/placement/inspect", post(pc::inspect))
        .route("/api/v1/placement/station-voice", post(pc::station_voice))
        .route("/api/v1/placement/link", post(pc::link))
        .route("/api/v1/placement/pair-station", post(pc::pair_station))
        .route("/api/v1/placement/probe", get(pc::probe))
        .route("/api/v1/desktop/awake", get(pc::awake).put(pc::set_awake))
        .route("/api/v1/placement/helper", post(pc::helper))
        .route("/api/v1/placement/retry", post(pc::retry));
    router.layer(DefaultBodyLimit::max(BODY_LIMIT))
}

/// The station gives its pair's key to its admin, from the home network: the computer that knows the admin's password
/// becomes its helper without SSH and without copying PEER_KEY by hand (docs/plans/desk.md, stage 4). A station whose
/// .env has no key makes one now and keeps it. The key is not taken back yet (the plan's «Открыто»), so it goes only to
/// a person signed in here as the admin — not to a key, nor through the computer's pass.
async fn pair(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    use crate::core::identity::principal::PrincipalKind;
    use crate::core::identity::roles::Role as PersonRole;
    caller.zone_at_least(crate::platform::http::Zone::Home)?;
    if super::peer::from_peer(&caller, app.placement.peer_key()) {
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
    if app.placement.role() != Role::Station {
        return Err(ApiError::conflict(crate::tr!("Ключ пары отдаёт колонка, а это компьютер.")));
    }
    let key = app.placement.pair_key()?;
    tracing::info!(target: "placement", "pair: the key given to a computer from {}", caller.source);
    Ok(ok(json!({ "key": key })))
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
    app.station_gateway.pick(pick.station.as_deref()).ok_or_else(|| {
        ApiError::conflict(crate::tr!("Какая колонка? Центр есть на нескольких — укажите её (station)."))
    })
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

/// The safety net on or off: on the station itself; from the computer, told to the station.
#[cfg_attr(not(feature = "pc"), allow(unused_variables))]
async fn safety(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(pick): Query<Pick>,
    Body(body): Body<Value>,
) -> ApiResult<Value> {
    let on = body
        .get("on")
        .and_then(Value::as_bool)
        .ok_or_else(|| ApiError::bad_request(crate::tr!("on: true или false")))?;
    match app.placement.role() {
        Role::Station => {
            super::peer::peer_or_permitted(&caller, app.placement.peer_key(), SETTINGS_EDIT)?;
            Ok(created(json!({ "state": app.placement.set_safety(on, caller.actor())? })))
        }
        Role::Pc => {
            caller.permitted(STATIONS_MANAGE)?;
            #[cfg(feature = "pc")]
            {
                let client = chosen(&app, &pick)?
                    .client()
                    .ok_or_else(|| ApiError::conflict(crate::tr!("Центр не на колонке.")))?;
                client.post("/api/v1/placement/safety", json!({ "on": on })).await.map_err(ApiError::unavailable)?;
            }
            Ok(created(json!({ "on": on })))
        }
    }
}

fn on_station(app: &App) -> Result<(), ApiError> {
    if app.placement.role() != Role::Station {
        return Err(ApiError::conflict(crate::tr!("Это делает центр на самой колонке.")));
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
        return Err(ApiError::forbidden(crate::tr!("Только для компьютера пары.")));
    }
    app.placement.restart();
    Ok(created(json!({ "restarting": true })))
}

/// This gateway as its panel's rename reads it: its own id, the name people see, the default and its home.
async fn gateway_self(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<GatewayView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(app.identity.view()?))
}

/// A person's name for this gateway, `{"name": "Кухня"}`; empty or null — the default again. The name is this gateway's
/// own, not a setting the computer sends: a station whose settings live on the computer is renamed too — in its panel,
/// or from the computer through its pass (`/station/<key>/api/v1/gateway/self`).
async fn rename(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<GatewayView> {
    caller.permitted(SETTINGS_EDIT)?;
    let name = match body.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => Some(name.as_str()),
        Some(_) => return Err(ApiError::bad_request(crate::tr!("{name}: нужна строка.", name = "name"))),
    };
    Ok(ok(app.identity.rename(name, caller.actor())?))
}

/// What the package is asked with: `?plugins=1` — the plugins' state too (a move back takes it, moves.rs).
#[derive(Debug, Default, Deserialize)]
struct Asked {
    #[serde(default)]
    plugins: Option<String>,
}

/// The settings package: sealed for the computer of the pair, plain for an admin (it holds the keys of the clouds).
/// With `?plugins=1` the plugins' state goes in it as well — only a move carries that (transfer.rs).
async fn export(State(app): State<Arc<App>>, caller: Caller, Query(asked): Query<Asked>) -> ApiResult<Value> {
    let peer = super::peer::peer_or_permitted(&caller, app.placement.peer_key(), USERS_MANAGE)?;
    let from = match app.placement.role() {
        Role::Pc => "pc",
        Role::Station => "station",
    };
    let mut bundle = transfer::export(&app.db, &app.secrets, from)?;
    bundle.revision = app.placement.settings_revision().unwrap_or(0);
    if asked.plugins.as_deref().is_some_and(|value| matches!(value, "1" | "true")) {
        bundle.plugins = Some(transfer::plugins(&app.db)?);
        // A move to the computer: every picture of the plugins goes along (plugins/files.rs).
        bundle.pictures = Some(transfer::pictures(app.files.root(), None));
    }
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
        serde_json::from_value(body).map_err(|_| ApiError::bad_request(crate::tr!("Это не набор настроек DuoWave.")))?
    };
    if bundle.format != transfer::FORMAT || bundle.schema > transfer::SCHEMA {
        return Err(ApiError::bad_request(transfer::NOT_FOR_THIS_GATEWAY));
    }
    let dir =
        app.config.skills.state_dir.clone().ok_or_else(|| {
            ApiError::conflict(crate::tr!("У DuoWave здесь нет папки данных: настройки не сохранить."))
        })?;
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
            .map_err(|_| ApiError::bad_request(crate::tr!("to: station, stationServer, pc или yandex")))?;
        let picked = body.get("station").and_then(Value::as_str);
        let record = app.station_gateway.pick(picked).and_then(|station| station.record());
        let address = body
            .get("address")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| record.as_ref().map(|record| record.address.clone()))
            .and_then(|address| plain_address(&address))
            .ok_or_else(|| ApiError::bad_request(crate::tr!("address: адрес колонки, IPv4")))?;
        let gateway = match body.get("gateway").and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty()) {
            // Typed without a port: this gateway's own port, not 80.
            Some(text) => Some(
                parse_gateway_host_or(text, app.station_connect.port())
                    .ok_or_else(|| ApiError::bad_request(crate::tr!("gateway: адрес[:порт]")))?,
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
        // The home's settings live at its centre, never on a computer (docs/plans/home-and-devices.md, stage 3).
        let settings_home = crate::settings::definitions::SettingsHome::Station;
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

    pub async fn auto_update(
        State(app): State<Arc<App>>,
        caller: Caller,
        Query(pick): Query<Pick>,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(SYSTEM_UPDATE)?;
        let on = body
            .get("on")
            .and_then(Value::as_bool)
            .ok_or_else(|| ApiError::bad_request(crate::tr!("on: true или false")))?;
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
            .ok_or_else(|| ApiError::conflict(crate::tr!("Центр не на колонке.")))
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
            _ => return Err(ApiError::bad_request(crate::tr!("to: local или yandex"))),
        };
        let client =
            chosen(&app, &pick)?.client().ok_or_else(|| ApiError::conflict(crate::tr!("Центр не на колонке.")))?;
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
            .ok_or_else(|| ApiError::bad_request(crate::tr!("address: адрес колонки, IPv4")))?;
        let port =
            body.get("port").and_then(Value::as_u64).filter(|port| (1..=65535).contains(port)).unwrap_or(8790) as u16;
        let key = body.get("key").and_then(Value::as_str).map(str::trim).unwrap_or("");
        if key.len() < 16 {
            return Err(ApiError::bad_request(crate::tr!("key: ключ пары — PEER_KEY из .env DuoWave на колонке")));
        }
        let view = app.station_gateway.link(&address, port, key, caller.actor()).await.map_err(ApiError::conflict)?;
        Ok(created(serde_json::to_value(view)?))
    }

    /// First run with a gateway already on the station: the computer signs in to the station's panel with the admin's
    /// password, takes the pair's key (`POST /api/v1/placement/pair` there) and links — no SSH, no key copied by hand.
    /// The password goes to the station only and is not kept.
    pub async fn pair_station(
        State(app): State<Arc<App>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = body
            .get("address")
            .and_then(Value::as_str)
            .and_then(plain_address)
            .ok_or_else(|| ApiError::bad_request(crate::tr!("address: адрес колонки, IPv4")))?;
        let port =
            body.get("port").and_then(Value::as_u64).filter(|port| (1..=65535).contains(port)).unwrap_or(8790) as u16;
        let password = body.get("password").and_then(Value::as_str).unwrap_or("");
        if password.is_empty() {
            return Err(ApiError::bad_request(crate::tr!("password: пароль пульта колонки")));
        }
        let name = body.get("name").and_then(Value::as_str).map(str::trim).filter(|name| !name.is_empty());
        let base = format!("http://{address}:{port}");
        let client = crate::utils::http::client();
        let login = client
            .post(format!("{base}/api/v1/auth/login"))
            .json(&json!({ "name": name, "password": password }))
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| ApiError::unavailable(crate::tr!("DuoWave на колонке не отвечает по этому адресу.")))?;
        let status = login.status();
        let answer: Value = login.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let said = answer
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| crate::tr!("пароль не подошёл"));
            return Err(ApiError::conflict(crate::tr!("Пульт колонки не пустил: {said}", said = said)));
        }
        let token = answer.pointer("/message/token").and_then(Value::as_str).ok_or_else(|| {
            ApiError::conflict(crate::tr!("Пульт колонки ответил без входа: нужен пароль администратора."))
        })?;
        let paired = client
            .post(format!("{base}/api/v1/placement/pair"))
            .bearer_auth(token)
            .json(&json!({}))
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| ApiError::unavailable(crate::tr!("DuoWave на колонке не отвечает по этому адресу.")))?;
        let ok_pair = paired.status().is_success();
        let answer: Value = paired.json().await.unwrap_or(Value::Null);
        let Some(key) = answer.pointer("/message/key").and_then(Value::as_str).filter(|_| ok_pair) else {
            let said = answer
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| crate::tr!("DuoWave на колонке постарше: обновите его"));
            return Err(ApiError::conflict(crate::tr!("Ключ пары колонка не отдала: {said}", said = said)));
        };
        let view = app.station_gateway.link(&address, port, key, caller.actor()).await.map_err(ApiError::conflict)?;
        Ok(created(serde_json::to_value(view)?))
    }

    /// «Не давать компьютеру спать» of the desktop app, from the helper's panel (docs/plans/desk.md, stage 4): the
    /// app's own setting beside the gateway's data (desktop.json, `keepAwake`, on unless switched off); the app reads
    /// it again every few seconds and holds the computer awake or lets it sleep.
    fn desktop_json(app: &App) -> Result<std::path::PathBuf, ApiError> {
        if app.config.placement.managed_by.as_deref() != Some("desktop") {
            return Err(ApiError::conflict(crate::tr!("Это настройка приложения DuoWave на компьютере.")));
        }
        app.config
            .data_dir
            .as_ref()
            .map(|dir| dir.join("desktop.json"))
            .ok_or_else(|| ApiError::conflict(crate::tr!("Нет папки данных.")))
    }

    fn read_desktop(path: &std::path::Path) -> Value {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}))
    }

    pub async fn awake(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
        caller.permitted(STATION_VIEW)?;
        let Ok(path) = desktop_json(&app) else { return Ok(ok(json!({ "managed": false }))) };
        let on = read_desktop(&path).get("keepAwake").and_then(Value::as_bool).unwrap_or(true);
        Ok(ok(json!({ "managed": true, "keepAwake": on })))
    }

    pub async fn set_awake(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<Value> {
        caller.permitted(SETTINGS_EDIT)?;
        let on = body
            .get("on")
            .and_then(Value::as_bool)
            .ok_or_else(|| ApiError::bad_request(crate::tr!("on: true или false")))?;
        let path = desktop_json(&app)?;
        let mut settings = read_desktop(&path);
        settings["keepAwake"] = Value::Bool(on);
        std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&settings).unwrap_or_default()))
            .map_err(|error| ApiError::unavailable(crate::tr!("Настройка не записалась: {error}", error = error)))?;
        Ok(ok(json!({ "managed": true, "keepAwake": on })))
    }

    /// Is a DuoWave gateway on this station already? Its /health answers in our shape: the first run on a computer
    /// offers to help it rather than take its place.
    pub async fn probe(
        caller: Caller,
        Query(query): Query<std::collections::HashMap<String, String>>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = query
            .get("address")
            .and_then(|address| plain_address(address))
            .ok_or_else(|| ApiError::bad_request(crate::tr!("address: адрес колонки, IPv4")))?;
        let port = query.get("port").and_then(|port| port.parse::<u16>().ok()).unwrap_or(8790);
        let answer = crate::utils::http::client()
            .get(format!("http://{address}:{port}/health"))
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await;
        let Ok(answer) = answer else { return Ok(ok(json!({ "gateway": false }))) };
        let health: Value = answer.json().await.unwrap_or(Value::Null);
        let (gateway, version) = recognise(&health);
        Ok(ok(json!({ "gateway": gateway, "version": version, "port": port })))
    }

    /// A /health answer: a DuoWave gateway that is not a computer's, and its version. The answer comes in the API's
    /// envelope ({code, message}); a gateway from before the envelope answered bare.
    pub(super) fn recognise(health: &Value) -> (bool, Option<&str>) {
        let status = health.get("message").filter(|message| message.is_object()).unwrap_or(health);
        let role = status.pointer("/placement/role").and_then(Value::as_str);
        let version = status.pointer("/build/version").and_then(Value::as_str);
        (version.is_some() && role != Some("pc"), version)
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
        // «Что даю колонке»: слух, голос, модель и сообщения по отдельности — без переключения самой помощи.
        if let Some(without) = body.get("without").and_then(Value::as_array) {
            let without: Vec<String> = without.iter().filter_map(Value::as_str).map(str::to_string).collect();
            station.set_helper_parts(without, caller.actor()).await.map_err(ApiError::unavailable)?;
            if body.get("on").is_none() {
                return Ok(created(serde_json::to_value(station.view())?));
            }
        }
        let on = body
            .get("on")
            .and_then(Value::as_bool)
            .ok_or_else(|| ApiError::bad_request(crate::tr!("on: true или false")))?;
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

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use axum::body::Body as HttpBody;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::Map;
    use tower::ServiceExt;

    use super::*;
    use crate::config::{Config, Env};
    use crate::placement::peer::{HELPER_PERSON, HELPER_ROLE};

    const PAIR: &str = "0123456789abcdef0123456789abcdef";

    /// The station's gateway (the computer's build playing it); its .env with these.
    fn station(vars: &[(&str, &str)]) -> Arc<App> {
        let base = [("STATE_DIR", ""), ("DUOWAVE_ROLE", "station")];
        let env = Env::from_pairs(base.into_iter().chain(vars.iter().copied()));
        App::build(Config::for_tests(&env)).unwrap()
    }

    /// A request from the home network with these headers.
    async fn send(
        app: &Arc<App>,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Value,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri).header("content-type", "application/json");
        for (name, value) in headers {
            request = request.header(*name, axum::http::HeaderValue::from_bytes(value.as_bytes()).unwrap());
        }
        let mut request = request.body(HttpBody::from(body.to_string())).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new("192.168.1.20".parse().unwrap(), 5000)));
        let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// The first run's probe reads a real /health answer, envelope and all: a station's gateway is one, a computer's
    /// is not; a gateway from before the envelope (bare) still is.
    #[cfg(feature = "pc")]
    #[tokio::test]
    async fn the_probe_recognises_a_station_gateway_by_its_real_health_answer() {
        let (status, health) = send(&station(&[]), "GET", "/health", &[], Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{health}");
        assert_eq!(pc::recognise(&health), (true, Some(crate::VERSION)), "{health}");
        assert_eq!(pc::recognise(&health["message"]), (true, Some(crate::VERSION)), "bare");

        let computer = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
        let (_, health) = send(&computer, "GET", "/health", &[], Value::Null).await;
        assert_eq!(pc::recognise(&health), (false, Some(crate::VERSION)), "{health}");
        assert_eq!(pc::recognise(&json!({ "code": 200, "message": { "status": "ok" } })), (false, None), "a stranger");
    }

    #[tokio::test]
    async fn the_station_gives_its_pair_key_only_to_its_admin_signed_in_with_the_password() {
        let app = station(&[]);
        let admin = app.access.auth.setup("admin-password-1", "Мама", "test").await.unwrap().unwrap().token;
        let admin = format!("Bearer {admin}");
        let pair_uri = "/api/v1/placement/pair";
        let (status, body) = send(&app, "POST", pair_uri, &[("authorization", admin.as_str())], json!({})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let key = body["message"]["key"].as_str().unwrap().to_string();
        assert_eq!(app.placement.peer_key(), Some(key.as_str()), "made here and kept");

        // A key with every right is not a person; the pair's key itself (the computer's pass) does not take it either.
        let grants = Map::from_iter([("name".into(), json!("Скрипт")), ("grants".into(), json!(["*"]))]);
        let script = format!("Bearer {}", app.access.keys.create(&grants, None).unwrap().secret);
        let (status, _) = send(&app, "POST", pair_uri, &[("authorization", script.as_str())], json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let pair = format!("Bearer {key}");
        let through = [("authorization", pair.as_str()), (HELPER_PERSON, "Мама"), (HELPER_ROLE, "admin")];
        assert_eq!(send(&app, "POST", pair_uri, &through, json!({})).await.0, StatusCode::FORBIDDEN);
    }

    /// A package that needs a newer schema than this gateway knows is refused here, before anything is kept and
    /// before the gateway starts again — as every build's route since it came refuses one, in these words: a station of
    /// an earlier build so refuses a routine sending of schema 2 that reaches it all the same (transfer.rs), and the
    /// computer knows the refusal by them (watch.rs).
    #[tokio::test]
    async fn a_package_that_needs_a_newer_schema_is_refused_before_anything_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().to_string_lossy().into_owned();
        let app = station(&[("PEER_KEY", PAIR), ("STATE_DIR", state.as_str())]);
        assert_eq!(app.config.skills.state_dir.as_deref(), Some(dir.path()), "a package would be kept here");
        let pair = format!("Bearer {PAIR}");
        let mut bundle = transfer::export(&app.db, &app.secrets, "pc").unwrap();
        bundle.schema = transfer::SCHEMA + 1;
        let sealed = transfer::seal(&bundle, PAIR);
        let (status, body) = send(&app, "POST", "/api/v1/transfer", &[("authorization", pair.as_str())], sealed).await;
        let refused = Some("Набор настроек не для этого шлюза: обновите его.");
        assert_eq!((status, body["message"].as_str()), (StatusCode::BAD_REQUEST, refused), "{body}");
        assert!(!transfer::incoming_file(dir.path()).exists(), "nothing kept for the next start");
        assert_eq!(transfer::synced_at(&app.db, "received"), None, "nor noted as received");
    }
}
