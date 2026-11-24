//! The stations' HTTP with the computer's own: the wizard that connects a station over SSH
//! (`/api/v1/stations/connect/...`).

use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use serde_json::Value;

use crate::core::identity::permissions::STATIONS_MANAGE;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Uses};
use crate::stations::connect::station_connect::StationConnectService;

pub fn router<S>() -> Router<S>
where
    S: crate::stations::RouteNeeds + duowave_ears::stations::RouteNeeds,
{
    duowave_ears::stations::routes::router::<S>()
        .route("/api/v1/stations/connect/discover", get(connect::discover))
        .route("/api/v1/stations/connect/moved", get(connect::moved))
        .route("/api/v1/stations/connect/check", axum::routing::post(connect::check))
        .route("/api/v1/stations/connect", axum::routing::post(connect::start))
        .route("/api/v1/stations/connect/restore", axum::routing::post(connect::restore))
        .route("/api/v1/stations/connect/uninstall", axum::routing::post(connect::uninstall))
        .route("/api/v1/stations/connect/turn-off", axum::routing::post(connect::turn_off))
        .route("/api/v1/stations/connect/jobs/{id}", get(connect::job))
}

mod connect {
    use super::*;
    use crate::platform::http::Api;
    use crate::stations::connect::discovery::FoundStation;
    use crate::stations::connect::quasar_config::{parse_gateway_host_or, plain_address, GatewayHost};
    use crate::stations::connect::station_connect::{
        host_interfaces, in_container, ConnectJob, MovedGateway, RestoreTo, StationCheck,
    };

    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    pub(super) struct Discovered {
        stations: Vec<FoundStation>,
        /// DuoWave gateways on computers and servers that announce themselves (stage 6 of the home-setup-flow plan):
        /// TXT `role` (centre, turbo, ear), `port`, `version`, `name`, `home`, `homeId`, `centre`. Not this computer.
        homes: Vec<FoundStation>,
        gateway: Vec<String>,
        /// The port a station dials when the typed address has none: the gateway's own (the published one in Docker).
        port: u16,
        /// None found on Windows: what on this computer may keep the answers out (its firewall, a public network).
        #[serde(skip_serializing_if = "Option::is_none")]
        why_none: Option<String>,
    }

    /// Stations that answer mDNS on the home network (none inside a container's bridge: type the address).
    pub(super) async fn discover(
        Uses(station_connect): Uses<Arc<StationConnectService>>,
        Uses(identity): Uses<Arc<crate::core::identity::home::Identity>>,
        caller: Caller,
    ) -> ApiResult<Discovered> {
        caller.permitted(STATIONS_MANAGE)?;
        let (stations, homes) =
            tokio::join!(station_connect.discover(), crate::stations::connect::discovery::discover_homes(2000));
        let interfaces = host_interfaces();
        // This computer answers its own question too: not a home to find.
        let own: Vec<String> =
            interfaces.iter().filter(|entry| entry.ipv4).map(|entry| entry.address.clone()).collect();
        let me = identity.as_helper();
        let homes: Vec<FoundStation> = homes
            .into_iter()
            .filter(|home| !crate::stations::connect::discovery::is_this_gateway(home, &own, &me.id, &me.name))
            .collect();
        let gateway = station_connect.home_addresses(&interfaces, in_container());
        tracing::info!(
            "station search: {} found; this computer's adapters: {}",
            stations.len(),
            interfaces
                .iter()
                .filter(|entry| entry.ipv4 && !entry.internal)
                .map(|entry| format!("{} {}", entry.name, entry.address))
                .collect::<Vec<_>>()
                .join(", ")
        );
        // Windows: the answers come in as UDP — its firewall or a «Общедоступная» network keeps them out (test 21.11).
        // macOS: the search could not even ask — DuoWave has no «Local Network» (step 11 check, finding 3).
        let why_none = if cfg!(target_os = "macos") && crate::stations::connect::discovery::OFF_NETWORK.now() {
            Some(crate::tr!(
                "macOS не пускает DuoWave в домашнюю сеть, поэтому колонок и других компьютеров не видно. Разрешите в «Системных настройках»: «Конфиденциальность и безопасность» → «Локальная сеть» → DuoWave."
            ))
        } else if stations.is_empty() && cfg!(windows) {
            let network = crate::platform::windows_net::refresh().await;
            Some(network.as_ref().and_then(crate::platform::windows_net::advice).unwrap_or_else(|| {
                crate::tr!("На Windows поиск колонок может закрывать брандмауэр: введите адрес колонки сами.")
            }))
        } else {
            None
        };
        Ok(ok(Discovered { stations, homes, gateway, port: station_connect.port(), why_none }))
    }

    /// Connected stations that still dial this computer's old port (PORT changed since): the panel's banner.
    pub(super) async fn moved(
        Uses(station_connect): Uses<Arc<StationConnectService>>,
        caller: Caller,
    ) -> ApiResult<Vec<MovedGateway>> {
        caller.permitted(STATIONS_MANAGE)?;
        Ok(ok(station_connect.moved(&host_interfaces(), in_container())))
    }

    /// The first look at one station: reading only, with the admin's consent in the panel.
    pub(super) async fn check(
        Uses(station_connect): Uses<Arc<StationConnectService>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> ApiResult<StationCheck> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = station_address(&body)?;
        let gateway = match body.get("gateway") {
            None => None,
            Some(_) => Some(gateway_host(&body, "gateway", station_connect.port())?),
        };
        // The station's own root password, when the empty one was refused: kept sealed, never logged or answered.
        let password = body.get("password").and_then(Value::as_str);
        Ok(ok(station_connect.check_with_password(&address, gateway.as_ref(), password).await?))
    }

    /// «Подключить»: a job the panel follows at jobs/:id.
    pub(super) async fn start(
        Uses(station_connect): Uses<Arc<StationConnectService>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> Result<Api<ConnectJob>, ApiError> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = station_address(&body)?;
        let gateway = gateway_host(&body, "gateway", station_connect.port())?;
        let job = station_connect.connect(&address, &gateway, caller.actor())?;
        Ok(Api(StatusCode::ACCEPTED, job))
    }

    /// «Вернуть к Яндексу»: the addresses the station had before the gateway.
    pub(super) async fn restore(
        Uses(station_connect): Uses<Arc<StationConnectService>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> Result<Api<ConnectJob>, ApiError> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = station_address(&body)?;
        let to = if body.get("to").and_then(Value::as_str) == Some("previous") {
            RestoreTo::Previous
        } else {
            RestoreTo::Yandex
        };
        let job = station_connect.restore(&address, to, caller.actor())?;
        Ok(Api(StatusCode::ACCEPTED, job))
    }

    /// «Удалить DuoWave с колонки»: uninstall.sh over SSH, a check that nothing of ours is left, a reboot.
    pub(super) async fn uninstall(
        Uses(station_connect): Uses<Arc<StationConnectService>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> Result<Api<ConnectJob>, ApiError> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = station_address(&body)?;
        let job = station_connect.uninstall(&address, caller.actor())?;
        Ok(Api(StatusCode::ACCEPTED, job))
    }

    /// «Выключить» another mod on the station by its own switch: `{address, id}`, the id from the look's list.
    pub(super) async fn turn_off(
        Uses(station_connect): Uses<Arc<StationConnectService>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = station_address(&body)?;
        let addon = body
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::bad_request(crate::tr!("Не сказано, какой мод выключить.")))?;
        station_connect.turn_off(&address, addon, caller.actor()).await?;
        Ok(ok(serde_json::json!({ "turnedOff": addon })))
    }

    pub(super) async fn job(
        Uses(station_connect): Uses<Arc<StationConnectService>>,
        caller: Caller,
        Path(id): Path<String>,
    ) -> ApiResult<ConnectJob> {
        caller.permitted(STATIONS_MANAGE)?;
        Ok(ok(station_connect.job(&id)?))
    }

    fn station_address(body: &Value) -> Result<String, ApiError> {
        body.get("address")
            .and_then(Value::as_str)
            .and_then(plain_address)
            .ok_or_else(|| ApiError::bad_request(crate::tr!("Адрес колонки — IPv4, например 192.168.1.23.")))
    }

    /// The gateway's address as typed in the panel: without a port, the port a station dials for this gateway.
    fn gateway_host(body: &Value, key: &str, port: u16) -> Result<GatewayHost, ApiError> {
        body.get(key).and_then(Value::as_str).and_then(|text| parse_gateway_host_or(text, port)).ok_or_else(|| {
            ApiError::bad_request(crate::tr!(
                "Адрес хаба — IP или имя в домашней сети, можно с портом: 192.168.1.50 или duowave.local:8080."
            ))
        })
    }
}
