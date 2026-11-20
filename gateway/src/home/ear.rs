//! A second station as an ear of the home's centre (docs/plans/home-and-devices.md, stage 4). One home, one centre:
//! a station that joins a home with a centre already keeps our program, but its own gateway is the centre's ear —
//!
//! - its station's voice socket goes on to the centre (`bridge`): the centre decides who answers, its room's mode and
//!   character included (src/home/rooms.rs), and the answer sounds here. The station's own handshake goes with it, so
//!   the centre opens Yandex with this station's account (the mode «Алиса», Alice's voice);
//! - it keeps a copy of the home (`keep`, src/home/copy.rs) and answers from it while the centre is away: the voice
//!   socket is served here then, as by any centre;
//! - its panel shows the home: the device list and the apps come from the centre (src/home/devices.rs).
//!
//! «Сделать центром» (`make_centre`): the centre asks one of its ears to become the centre — it has the home's copy
//! up to the last revision — and becomes that one's ear itself, over the home network with the pair's keys, without
//! SSH. The same way brings the home back when a station is closed by its firmware (0.374): another device becomes the
//! centre.
//!
//! `GET|PUT|DELETE /api/v1/home/ear` — this gateway's centre, set by an admin (`{address, port, key}` — the centre's
//! pair key, which its admin reads); `POST /api/v1/home/centre` `{device}` — at the centre: make that ear the centre;
//! `POST /api/v1/home/become-centre` — at an ear, from its centre with this ear's pair key.
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message as StationMessage, WebSocket};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::Router;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as CentreMessage;

use crate::app::App;
use crate::core::events::GatewayEvent;
use crate::core::identity::permissions::{SETTINGS_EDIT, STATION_VIEW};
use crate::core::settings::{SettingDefinition, SettingScope};
use crate::placement::peer::{from_peer, PeerClient};
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller};
use crate::utils::time::now_iso;

/// This gateway's centre, when it is an ear: where the centre listens. Its pair key is a secret (`KEY_SECRET`). Each
/// device's own (`transfer::LOCAL_SETTINGS`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EarOf {
    pub address: String,
    pub port: u16,
    #[serde(default)]
    pub name: Option<String>,
    pub since: String,
}

pub const HOME_EAR: SettingDefinition<EarOf> = SettingDefinition {
    key: "home_ear",
    scope: SettingScope::Global,
    description: "Центр дома",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// The centre's pair key, kept by its ear. Not among the travelling prefixes.
pub const KEY_SECRET: &str = "earof:key";
/// An ear's pair key, kept by its centre to ask it to become the centre (`make_centre`).
pub fn ear_key_secret(device: &str) -> String {
    format!("earkey:{device}")
}

/// How long the centre is given to take a station's socket before the ear serves it itself.
const DIAL: Duration = Duration::from_secs(2);
/// How often an ear looks at its centre for the home's copy.
const LOOK: Duration = Duration::from_secs(30);

/// The centre this gateway is the ear of, with its pair key.
pub fn centre(app: &App) -> Option<(EarOf, String)> {
    let ear = app.settings.get(&HOME_EAR, None)?;
    let key = app.secrets.get(KEY_SECRET)?;
    Some((ear, key))
}

pub fn client(ear: &EarOf, key: &str) -> PeerClient {
    PeerClient::new(&ear.address, ear.port, key)
}

/// The station's handshake headers that go on to the centre: all but the connection's own.
fn forwarded(headers: &HeaderMap) -> Vec<(axum::http::HeaderName, axum::http::HeaderValue)> {
    headers
        .iter()
        .filter(|(name, _)| {
            let name = name.as_str();
            !matches!(name, "host" | "connection" | "upgrade" | "content-length") && !name.starts_with("sec-websocket-")
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

type CentreSocket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The station's socket opened on to the centre, within `DIAL`; none — the centre is away.
pub async fn dial(app: &App, headers: &HeaderMap) -> Option<CentreSocket> {
    let (ear, _) = centre(app)?;
    let url = format!("ws://{}:{}{}", ear.address, ear.port, app.config.gateway.uniproxy_path);
    let mut request = url.into_client_request().ok()?;
    for (name, value) in forwarded(headers) {
        request.headers_mut().insert(name, value);
    }
    match tokio::time::timeout(DIAL, tokio_tungstenite::connect_async(request)).await {
        Ok(Ok((socket, _))) => Some(socket),
        Ok(Err(error)) => {
            tracing::info!(target: "home", "the centre did not take the station's socket: {error}; answering here");
            None
        }
        Err(_) => {
            tracing::info!(target: "home", "the centre did not answer in time; answering here");
            None
        }
    }
}

/// Frames both ways between the station and the centre, until either side closes.
pub async fn bridge(station: WebSocket, centre: CentreSocket) {
    let (mut to_station, mut from_station) = station.split();
    let (mut to_centre, mut from_centre) = centre.split();
    let up = async {
        while let Some(Ok(message)) = from_station.next().await {
            let message = match message {
                StationMessage::Text(text) => CentreMessage::Text(text.as_str().into()),
                StationMessage::Binary(bytes) => CentreMessage::Binary(bytes),
                StationMessage::Ping(bytes) => CentreMessage::Ping(bytes),
                StationMessage::Pong(bytes) => CentreMessage::Pong(bytes),
                StationMessage::Close(_) => {
                    let _ = to_centre.send(CentreMessage::Close(None)).await;
                    break;
                }
            };
            if to_centre.send(message).await.is_err() {
                break;
            }
        }
    };
    let down = async {
        while let Some(Ok(message)) = from_centre.next().await {
            let message = match message {
                CentreMessage::Text(text) => StationMessage::Text(text.as_str().into()),
                CentreMessage::Binary(bytes) => StationMessage::Binary(bytes),
                CentreMessage::Ping(bytes) => StationMessage::Ping(bytes),
                CentreMessage::Pong(bytes) => StationMessage::Pong(bytes),
                CentreMessage::Close(_) => {
                    let _ = to_station.send(StationMessage::Close(None)).await;
                    break;
                }
                CentreMessage::Frame(_) => continue,
            };
            if to_station.send(message).await.is_err() {
                break;
            }
        }
    };
    tokio::select! {
        _ = up => {}
        _ = down => {}
    }
}

/// The ear's copy of the home, every `LOOK` and at once when it is set: the centre's revision taken, the memory both
/// ways, the changes made while it was away sent (src/home/copy.rs).
pub async fn keep(app: &App) -> Result<(), String> {
    let Some((ear, key)) = centre(app) else { return Ok(()) };
    let client = client(&ear, &key);
    let health = client.health().await?;
    let revision = health.pointer("/placement/settingsRevision").and_then(Value::as_u64);
    let me = app.identity.gateway().map(|me| me.id);
    let (device, _) = app.placement.station_self();
    super::copy::Keeper {
        db: &app.db,
        secrets: &app.secrets,
        settings: &app.settings,
        events: &app.events,
        client: &client,
        revision,
        me,
    }
    .keep()
    .await?;
    // The centre learns where this ear listens and its key: «Сделать центром» asks it with them.
    let key = app.placement.pair_key().map_err(|error| error.message)?;
    client
        .post(
            "/api/v1/home/held",
            json!({
                "gateway": app.identity.gateway().map(|me| me.id).unwrap_or_default(),
                "revision": revision.unwrap_or(0),
                "device": device,
                "port": app.config.app.public_port.unwrap_or(app.config.app.port),
                "key": key,
            }),
        )
        .await?;
    Ok(())
}

/// The ear's look at its centre, from the start.
pub fn start(app: &Arc<App>) {
    let weak = Arc::downgrade(app);
    tokio::spawn(async move {
        loop {
            let Some(app) = weak.upgrade() else { return };
            if centre(&app).is_some() {
                if let Err(error) = keep(&app).await {
                    tracing::info!(target: "home", "the home's copy from the centre: {error}");
                }
            }
            drop(app);
            tokio::time::sleep(LOOK).await;
        }
    });
}

/// What an ear changes for itself: its own centre, the centre's request to take over, its sign-in, its own desk and
/// panel memory, a phrase said to it (answered here while the centre is away), its updates and its placement.
fn its_own(path: &str) -> bool {
    [
        "/api/v1/home/ear",
        "/api/v1/home/become-centre",
        "/api/v1/home/held",
        "/api/v1/home/memory",
        "/api/v1/auth/",
        "/api/v1/desk/",
        "/api/v1/panel/state",
        "/api/v1/talk",
        "/api/v1/listen",
        "/api/v1/updates",
        "/api/v1/placement",
        "/api/v1/transfer",
        "/api/v1/live/ticket",
        "/api/v1/stations/",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
}

/// The home is written only at its centre: on an ear a change of it is refused with words — made here, it would be
/// lost at the next copy. Its own things pass (`its_own`), and the pair's own calls do.
pub async fn guard(
    State(app): State<Arc<App>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::Method;
    use axum::response::IntoResponse;
    let changing = !matches!(*request.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    let path = request.uri().path().to_string();
    if changing && path.starts_with("/api/v1/") && !its_own(&path) {
        if let Some(ear) = app.settings.get(&HOME_EAR, None) {
            let name = ear.name.unwrap_or(ear.address);
            return ApiError::conflict(crate::tr!(
                "Это устройство слышит для центра дома «{name}»: дом правят в его пульте, здесь правка потерялась бы.",
                name = name
            ))
            .into_response();
        }
    }
    next.run(request).await
}

/// The centre of this ear for its pair (a computer that watched this station as the centre, before it moved): where
/// the centre is now and its key, so the computer follows it.
async fn centre_for_pair(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    if !from_peer(&caller, app.placement.peer_key()) {
        return Err(ApiError::forbidden(crate::tr!("Это знает только устройство пары.")));
    }
    let Some((ear, key)) = centre(&app) else { return Ok(ok(json!({ "centre": null }))) };
    Ok(ok(json!({ "centre": { "address": ear.address, "port": ear.port, "name": ear.name, "key": key } })))
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/home/ear", get(view).put(set).delete(unset))
        .route("/api/v1/home/centre", post(make_centre))
        .route("/api/v1/home/become-centre", post(become_centre))
        .route("/api/v1/home/ear-centre", get(centre_for_pair))
        .route("/api/v1/home/join-ear", post(join))
}

async fn view(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(STATION_VIEW)?;
    let ear = app.settings.get(&HOME_EAR, None);
    Ok(ok(json!({ "earOf": ear, "copiedRevision": super::copy::copied_revision(&app.settings) })))
}

#[derive(Deserialize)]
struct SetEar {
    address: String,
    port: Option<u16>,
    key: String,
    name: Option<String>,
}

/// This gateway becomes the ear of that centre: checked first with its pair key, then its copy taken at once.
async fn set(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<Value> {
    // An admin here, or the computer of its pair joining it to the home (`join`, stage 6).
    crate::placement::peer::peer_or_permitted(&caller, app.placement.peer_key(), SETTINGS_EDIT)?;
    let wanted: SetEar =
        serde_json::from_value(body).map_err(|_| ApiError::bad_request("{address, port, key}".to_string()))?;
    let ear = EarOf {
        address: wanted.address.trim().to_string(),
        port: wanted.port.unwrap_or(crate::config::DEFAULT_PORT),
        name: wanted.name.filter(|name| !name.trim().is_empty()),
        since: now_iso(),
    };
    if ear.address.is_empty() || wanted.key.trim().len() < 16 {
        return Err(ApiError::bad_request(crate::tr!("Нужны адрес центра и его ключ пары.")));
    }
    let probe = client(&ear, wanted.key.trim());
    probe.settings().await.map_err(|_| ApiError::conflict(crate::tr!("Центр не принял ключ пары.")))?;
    app.secrets.set(KEY_SECRET, Some(wanted.key.trim()))?;
    app.settings.set(&HOME_EAR, &ear, caller.actor())?;
    app.events.emit(GatewayEvent::HomeDevicesChanged { reason: "ear".into() });
    if let Err(error) = keep(&app).await {
        tracing::info!(target: "home", "the first copy from the new centre: {error}");
    }
    Ok(ok(json!({ "earOf": ear })))
}

/// This gateway is its own centre again.
async fn unset(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(SETTINGS_EDIT)?;
    app.settings.forget(&HOME_EAR)?;
    app.secrets.delete(KEY_SECRET)?;
    app.events.emit(GatewayEvent::HomeDevicesChanged { reason: "ear".into() });
    Ok(ok(json!({ "earOf": null })))
}

#[derive(Deserialize)]
struct MakeCentre {
    device: String,
}

/// «Сделать центром» at the centre: the ear of that station becomes the centre, this gateway its ear.
async fn make_centre(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<Value> {
    caller.permitted(SETTINGS_EDIT)?;
    if centre(&app).is_some() {
        return Err(ApiError::conflict(crate::tr!(
            "Это устройство не центр: сделать центром может только центр дома."
        )));
    }
    let wanted: MakeCentre = serde_json::from_value(body).map_err(|_| ApiError::bad_request("{device}".to_string()))?;
    let held = super::copy::holder_of_device(&app.settings, &wanted.device)
        .ok_or_else(|| ApiError::not_found(crate::tr!("У этой колонки нет копии дома — она не ухо этого центра.")))?;
    let key = app
        .secrets
        .get(&ear_key_secret(&wanted.device))
        .ok_or_else(|| ApiError::conflict(crate::tr!("Колонка ещё не сказала свой ключ пары — подождите минуту.")))?;
    let address = held
        .address
        .clone()
        .or_else(|| app.stations.find(&wanted.device).and_then(|station| station.last_address))
        .ok_or_else(|| ApiError::not_found(crate::tr!("Адрес этой колонки неизвестен.")))?;
    let port = held.port.unwrap_or(crate::config::DEFAULT_PORT);
    let them = PeerClient::new(&address, port, &key);
    // The last of the home goes first: its copy takes the newest revision before it becomes the centre.
    let answer = them.post("/api/v1/home/become-centre", json!({})).await.map_err(ApiError::unavailable)?;
    let their_key = answer.get("key").and_then(Value::as_str).unwrap_or(&key).to_string();
    let name = app.stations.find(&wanted.device).map(|station| station.name);
    let ear = EarOf { address: address.clone(), port, name, since: now_iso() };
    app.secrets.set(KEY_SECRET, Some(&their_key))?;
    app.settings.set(&HOME_EAR, &ear, caller.actor())?;
    app.events.emit(GatewayEvent::HomeDevicesChanged { reason: "centre".into() });
    tracing::info!(target: "home", "the centre moved to {address}: this gateway is its ear now");
    // Its own station reconnects: its voice goes to the new centre from the next socket on.
    app.sessions.drop_all("the centre moved");
    Ok(ok(json!({ "centre": { "address": address, "port": port } })))
}

/// At an ear, from its centre: it becomes the centre with the copy it has; the old centre becomes its ear.
async fn become_centre(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    if !from_peer(&caller, app.placement.peer_key()) {
        return Err(ApiError::forbidden(crate::tr!("Сделать центром просит только центр этого дома.")));
    }
    if centre(&app).is_some() {
        // The newest of the home first, then no more an ear.
        if let Err(error) = keep(&app).await {
            tracing::warn!(target: "home", "the last copy before becoming the centre: {error}");
        }
    }
    app.settings.forget(&HOME_EAR)?;
    app.secrets.delete(KEY_SECRET)?;
    app.events.emit(GatewayEvent::HomeDevicesChanged { reason: "centre".into() });
    tracing::info!(target: "home", "this gateway is the home's centre now");
    let key = app.placement.pair_key()?;
    Ok(ok(json!({ "key": key, "port": app.config.app.public_port.unwrap_or(app.config.app.port) })))
}

#[cfg(feature = "pc")]
#[derive(Deserialize)]
struct Join {
    station: String,
}

/// «Ещё одна колонка» (docs/plans/home-and-devices.md, stage 6), on the computer that is the home's Турбо: a station it
/// put DuoWave on joins the home as the centre's ear — the computer knows the centre's address and pair key and hands
/// them to the new station with that one's pair key. Nothing else is asked; its room comes next on its page.
#[cfg(feature = "pc")]
async fn join(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<Value> {
    caller.permitted(crate::core::identity::permissions::STATIONS_MANAGE)?;
    let wanted: Join = serde_json::from_value(body).map_err(|_| ApiError::bad_request("{station}".to_string()))?;
    let centre = super::devices::centre_elsewhere(&app)
        .ok_or_else(|| ApiError::conflict(crate::tr!("Центр дома не на колонке: подключать ухом не к чему.")))?;
    if centre.key() == wanted.station {
        return Err(ApiError::conflict(crate::tr!("Это и есть центр дома.")));
    }
    let record = centre
        .record()
        .ok_or_else(|| ApiError::conflict(crate::tr!("Центр дома не на колонке: подключать ухом не к чему.")))?;
    let key = centre.pair_key().ok_or_else(|| ApiError::conflict(crate::tr!("Нет ключа пары центра.")))?;
    let found = app
        .station_gateway
        .find(&wanted.station)
        .ok_or_else(|| ApiError::not_found(crate::tr!("Такой колонки у этого компьютера нет.")))?;
    // The same station under a second record (by its device and «at:<address>»): still the centre itself — an ear of
    // itself would send its voice in a circle (Windows test 21.11 offered the centre among «not in the home»).
    if found.record().is_some_and(|other| other.address == record.address) {
        return Err(ApiError::conflict(crate::tr!("Это и есть центр дома.")));
    }
    let station =
        found.client().ok_or_else(|| ApiError::not_found(crate::tr!("Такой колонки у этого компьютера нет.")))?;
    let body = json!({ "address": record.address, "port": record.port, "key": key, "name": centre.name() });
    station.put("/api/v1/home/ear", body).await.map_err(ApiError::unavailable)?;
    app.events.emit(GatewayEvent::HomeDevicesChanged { reason: "join".into() });
    Ok(ok(json!({ "joined": wanted.station })))
}

#[cfg(not(feature = "pc"))]
async fn join(_caller: Caller, Body(_body): Body<Value>) -> ApiResult<Value> {
    Err(ApiError::not_found("Not Found".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stations_own_handshake_goes_on_but_the_connections_own_headers_do_not() {
        let mut headers = HeaderMap::new();
        headers.insert("x-uprx-device-id", "BEDROOM01".parse().unwrap());
        headers.insert("authorization", "OAuth station-token".parse().unwrap());
        headers.insert("host", "192.0.2.5:7780".parse().unwrap());
        headers.insert("sec-websocket-key", "abc".parse().unwrap());
        headers.insert("upgrade", "websocket".parse().unwrap());
        let names: Vec<String> = forwarded(&headers).into_iter().map(|(name, _)| name.to_string()).collect();
        assert!(names.contains(&"x-uprx-device-id".to_string()) && names.contains(&"authorization".to_string()));
        assert!(!names.iter().any(|name| name == "host" || name == "upgrade" || name.starts_with("sec-websocket")));
    }
}
