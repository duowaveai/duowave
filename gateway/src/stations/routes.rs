//! The stations' HTTP: the panel's list and edits (`/api/v1/stations`), the wizard that connects a station over SSH
//! (`/api/v1/stations/connect/...`, PC builds only), and the scripts the station fetches to connect itself
//! (`/station/connect.sh`, `/station/restore.sh`), served as plain text without the envelope by every build.
use std::sync::Arc;

use axum::body::Body as AxumBody;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::{get, patch};
use axum::Router;
use serde_json::Value;

use crate::app::App;
use crate::core::identity::permissions::{STATIONS_MANAGE, STATION_VIEW};
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller};
use crate::stations::connect::quasar_config::{gateway_addresses, parse_gateway_host};
use crate::stations::connect::station_scripts::{connect_script, restore_script};
use crate::stations::service::StationEntry;

pub fn router() -> Router<Arc<App>> {
    let router = Router::new()
        .route("/api/v1/stations", get(list))
        .route("/api/v1/stations/{device}", patch(update).delete(remove))
        .route("/station/connect.sh", get(connect_sh))
        .route("/station/restore.sh", get(restore_sh));
    #[cfg(feature = "pc")]
    let router = router
        .route("/api/v1/stations/connect/discover", get(connect::discover))
        .route("/api/v1/stations/connect/moved", get(connect::moved))
        .route("/api/v1/stations/connect/check", axum::routing::post(connect::check))
        .route("/api/v1/stations/connect", axum::routing::post(connect::start))
        .route("/api/v1/stations/connect/restore", axum::routing::post(connect::restore))
        .route("/api/v1/stations/connect/jobs/{id}", get(connect::job));
    router
}

async fn list(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Vec<StationEntry>> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(app.stations.list()))
}

/// Approve, block or name a station; set its look (model, color — null goes back to the default; sticker — a Lite 2's).
async fn update(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(device): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<StationEntry> {
    caller.permitted(STATIONS_MANAGE)?;
    Ok(ok(app.stations.update(&device, &body, caller.actor())?))
}

/// Forget a station: it is a stranger again, approved only as the policy says.
async fn remove(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(device): Path<String>,
) -> Result<StatusCode, ApiError> {
    caller.permitted(STATIONS_MANAGE)?;
    app.stations.remove(&device, caller.actor())?;
    Ok(StatusCode::NO_CONTENT)
}

/// The scripts for «Сделаю сам», fetched by the station itself:
///   ssh -p 2222 root@<station> 'wget -qO- http://<gateway>/station/connect.sh | sh'
/// The gateway's address in connect.sh is the one the station asked by: what it can reach. Nothing secret in them.
async fn connect_sh(State(app): State<Arc<App>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let host = headers.get(header::HOST).and_then(|value| value.to_str().ok()).unwrap_or("");
    let gateway = parse_gateway_host(host)
        .ok_or_else(|| ApiError::bad_request("the gateway is asked by an address the station cannot dial"))?;
    let script = connect_script(&gateway_addresses(&gateway, &app.config.gateway.uniproxy_path))
        .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(plain_text(script))
}

/// The way back for a station whose /data/opt/duowave/restore.sh is gone: the firmware's own config, our copy over it
/// unmounted.
async fn restore_sh() -> Result<Response, ApiError> {
    Ok(plain_text(restore_script()))
}

/// A plain-text body the station reads as it is, never cached.
fn plain_text(body: String) -> Response {
    let mut response = Response::new(AxumBody::from(body));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(feature = "pc")]
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
        gateway: Vec<String>,
        /// The port a station dials when the typed address has none: the gateway's own (the published one in Docker).
        port: u16,
    }

    /// Stations that answer mDNS on the home network (none inside a container's bridge: type the address).
    pub(super) async fn discover(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Discovered> {
        caller.permitted(STATIONS_MANAGE)?;
        let stations = app.station_connect.discover().await;
        let gateway = app.station_connect.home_addresses(&host_interfaces(), in_container());
        Ok(ok(Discovered { stations, gateway, port: app.station_connect.port() }))
    }

    /// Connected stations that still dial this computer's old port (PORT changed since): the panel's banner.
    pub(super) async fn moved(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Vec<MovedGateway>> {
        caller.permitted(STATIONS_MANAGE)?;
        Ok(ok(app.station_connect.moved(&host_interfaces(), in_container())))
    }

    /// The first look at one station: reading only, with the admin's consent in the panel.
    pub(super) async fn check(
        State(app): State<Arc<App>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> ApiResult<StationCheck> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = station_address(&body)?;
        let gateway = match body.get("gateway") {
            None => None,
            Some(_) => Some(gateway_host(&body, "gateway", app.station_connect.port())?),
        };
        Ok(ok(app.station_connect.check(&address, gateway.as_ref()).await?))
    }

    /// «Подключить»: a job the panel follows at jobs/:id.
    pub(super) async fn start(
        State(app): State<Arc<App>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> Result<Api<ConnectJob>, ApiError> {
        caller.permitted(STATIONS_MANAGE)?;
        let address = station_address(&body)?;
        let gateway = gateway_host(&body, "gateway", app.station_connect.port())?;
        let job = app.station_connect.connect(&address, &gateway, caller.actor())?;
        Ok(Api(StatusCode::ACCEPTED, job))
    }

    /// «Вернуть к Яндексу»: the addresses the station had before the gateway.
    pub(super) async fn restore(
        State(app): State<Arc<App>>,
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
        let job = app.station_connect.restore(&address, to, caller.actor())?;
        Ok(Api(StatusCode::ACCEPTED, job))
    }

    pub(super) async fn job(
        State(app): State<Arc<App>>,
        caller: Caller,
        Path(id): Path<String>,
    ) -> ApiResult<ConnectJob> {
        caller.permitted(STATIONS_MANAGE)?;
        Ok(ok(app.station_connect.job(&id)?))
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
                "Адрес шлюза — IP или имя в домашней сети, можно с портом: 192.168.1.50 или gateway.local:8080."
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::app::{router, App};
    use crate::config::{Config, Env};

    /// The whole stack on an in-memory gateway: the Host check, the envelope, the routes.
    fn gateway() -> axum::Router {
        let config = Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]));
        router(App::build(config).expect("the app builds in memory"))
    }

    /// A request from this computer (loopback), so a guarded route is reached rather than refused by zone.
    async fn call(method: &str, uri: &str, host: Option<&str>) -> (StatusCode, HeaderMap, String) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(host) = host {
            builder = builder.header(header::HOST, host);
        }
        let mut request = builder.body(AxumBody::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5000))));
        let response = gateway().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, headers, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn serves_the_connect_script_as_plain_text_without_the_envelope() {
        let (status, headers, body) = call("GET", "/station/connect.sh", Some("192.168.1.50:3000")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers.get(header::CONTENT_TYPE).unwrap(), "text/plain; charset=utf-8");
        assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
        // The gateway address is the one the station dialed (the Host), and there is no envelope around it.
        assert!(body.contains("voice.sh point 'ws://192.168.1.50:3000/uni.ws' ''"), "{body}");
        assert!(!body.contains("\"code\""), "a script, not the JSON envelope");
    }

    #[tokio::test]
    async fn serves_the_restore_script_back_to_the_firmwares_own_config() {
        let (status, _headers, body) = call("GET", "/station/restore.sh", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("voice.sh drop") && body.contains("reboot"), "{body}");
    }

    #[tokio::test]
    async fn the_stations_list_needs_signing_in() {
        // Loopback is allowed by zone, but nobody is signed in: 401, in the error envelope.
        let (status, _headers, body) = call("GET", "/api/v1/stations", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["code"], 401);
    }
}
