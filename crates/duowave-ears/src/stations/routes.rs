//! The stations' HTTP: the panel's list and edits (`/api/v1/stations`), and the scripts the station fetches to connect itself
//! (`/station/connect.sh`, `/station/restore.sh`, `/station/uninstall.sh`), served as plain text without the envelope by every build.
use std::sync::Arc;

use axum::body::Body as AxumBody;
use axum::extract::Path;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::{get, patch};
use axum::Router;
use serde_json::Value;

use crate::config::Config;
use crate::core::identity::permissions::{STATIONS_MANAGE, STATION_VIEW};
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Uses};
use crate::stations::connect::quasar_config::{gateway_addresses, parse_gateway_host};
use crate::stations::connect::station_scripts::{connect_script, restore_script, uninstall_script};
use crate::stations::service::StationEntry;
use crate::stations::StationsService;

pub fn router<S: crate::stations::RouteNeeds>() -> Router<S> {
    let router = Router::new()
        .route("/api/v1/stations", get(list))
        .route("/api/v1/stations/{device}", patch(update).delete(remove))
        .route("/station/connect.sh", get(connect_sh))
        .route("/station/restore.sh", get(restore_sh))
        .route("/station/uninstall.sh", get(uninstall_sh));
    // The wizard that connects a station over SSH: a computer's (crates/duowave-pc, stations::routes).
    router
}

async fn list(Uses(stations): Uses<Arc<StationsService>>, caller: Caller) -> ApiResult<Vec<StationEntry>> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(stations.list()))
}

/// Approve, block or name a station; set its look (model, color — null goes back to the default; sticker — a Lite 2's).
async fn update(
    Uses(stations): Uses<Arc<StationsService>>,
    caller: Caller,
    Path(device): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<StationEntry> {
    caller.permitted(STATIONS_MANAGE)?;
    Ok(ok(stations.update(&device, &body, caller.actor())?))
}

/// Forget a station: it is a stranger again, approved only as the policy says.
async fn remove(
    Uses(stations): Uses<Arc<StationsService>>,
    caller: Caller,
    Path(device): Path<String>,
) -> Result<StatusCode, ApiError> {
    caller.permitted(STATIONS_MANAGE)?;
    stations.remove(&device, caller.actor())?;
    Ok(StatusCode::NO_CONTENT)
}

/// The scripts for «Сделаю сам», fetched by the station itself:
///   ssh -p 2222 root@<station> 'wget -qO- http://<gateway>/station/connect.sh | sh'
/// The gateway's address in connect.sh is the one the station asked by: what it can reach. Nothing secret in them.
async fn connect_sh(Uses(config): Uses<Arc<Config>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let host = headers.get(header::HOST).and_then(|value| value.to_str().ok()).unwrap_or("");
    let gateway = parse_gateway_host(host)
        .ok_or_else(|| ApiError::bad_request("the gateway is asked by an address the station cannot dial"))?;
    let script = connect_script(&gateway_addresses(&gateway, &config.gateway.uniproxy_path))
        .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(plain_text(script))
}

/// The way back for a station whose /data/opt/duowave/restore.sh is gone: the firmware's own config, our copy over it
/// unmounted.
async fn restore_sh() -> Result<Response, ApiError> {
    Ok(plain_text(restore_script()))
}

/// DuoWave off a station whose /data/opt/duowave/uninstall.sh is gone (or was never there: connected by an older
/// build): `ssh -p 2222 root@<station> 'wget -qO- http://<gateway>/station/uninstall.sh | sh'`.
async fn uninstall_sh() -> Result<Response, ApiError> {
    Ok(plain_text(uninstall_script()))
}

/// A plain-text body the station reads as it is, never cached.
fn plain_text(body: String) -> Response {
    let mut response = Response::new(AxumBody::from(body));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
