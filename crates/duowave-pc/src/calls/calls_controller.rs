//! The call page, the calls' status, the calls' WebSocket, and the station client's HTTP audio streams. These
//! answer in their own format (the page, the station's wget and GStreamer), without the API's envelope; the zone
//! check before them answers as the API does.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, Query, Request};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::json;

use super::calls_service::{same_origin, CALLS_WS_PATH};
use super::http_stations::ListenFormat;
use super::livekit::{app_access, app_name, livekit_page, APP_TOKEN_TTL};
use super::page::call_page;
use crate::core::identity::permissions;
use crate::operations::networks::via_tunnel;
use crate::platform::http::{ApiError, Caller, Uses, Zone};

/// The largest message, and frame, of the calls' socket.
const MAX_MESSAGE: usize = 1024 * 1024;

pub fn router<S: crate::calls::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/calls", get(page))
        .route(CALLS_WS_PATH, get(socket))
        .route("/calls/livekit", get(livekit))
        .route("/calls/livekit/token", get(livekit_token))
        .route("/calls/status", get(status))
        .route("/calls/listen/{device}", get(listen))
        .route("/calls/next/{device}", get(next))
        .route("/calls/release/{device}", post(release))
        .route("/calls/talk/{device}", post(talk))
}

/// The caller's address as the socket gave it ("" without one).
fn peer_address(caller: &Caller) -> String {
    caller.peer.map(|peer| peer.ip().to_string()).unwrap_or_default()
}

/// The station client's routes answer an approved station's address only (any home address with
/// STATION_PAIRING=open).
fn refused(calls: &Arc<crate::calls::CallsService>, caller: &Caller) -> Option<Response> {
    if calls.trusted_address(&peer_address(caller)) {
        return None;
    }
    Some((StatusCode::FORBIDDEN, Json(json!({ "error": "only an approved station" }))).into_response())
}

async fn page() -> Html<String> {
    Html(call_page())
}

/// The LiveKit page: the family room through LiveKit, from home or outside.
async fn livekit(Uses(calls): Uses<Arc<crate::calls::CallsService>>) -> Html<String> {
    let server = calls.livekit_config().map(|config| config.public_url.clone().unwrap_or_else(|| config.url.clone()));
    Html(livekit_page(server.as_deref()))
}

/// A join token for the family room, given only inside the home network: a phone takes it at home and keeps it (a
/// month) to call from outside.
async fn livekit_token(
    Uses(calls): Uses<Arc<crate::calls::CallsService>>,
    caller: Caller,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    caller.zone_at_least(Zone::Home)?;
    let Some(config) = calls.livekit_config() else {
        return Ok(
            (StatusCode::NOT_FOUND, Json(json!({ "error": "no LiveKit: set it up in the panel" }))).into_response()
        );
    };
    let who = app_name(query.get("name").map(String::as_str));
    Ok(Json(app_access(&config, who, APP_TOKEN_TTL)).into_response())
}

/// Who is on the calls: names of contacts and their endpoints — for someone signed in.
async fn status(Uses(calls): Uses<Arc<crate::calls::CallsService>>, caller: Caller) -> Result<Response, ApiError> {
    caller.permitted(permissions::STATION_VIEW)?;
    Ok(Json(json!({ "endpoints": calls.hub.list(), "calls": calls.hub.snapshot() })).into_response())
}

/// The calls' socket: the call page, the simulator, a station client. Calls from outside go through LiveKit; the
/// hub's own socket is the home network's.
async fn socket(
    Uses(calls_service): Uses<Arc<crate::calls::CallsService>>,
    caller: Caller,
    upgrade: WebSocketUpgrade,
) -> Response {
    if via_tunnel(&caller.headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    // A browser always names the page that opens a socket: only our own call page may (no other site's script).
    let header = |name: header::HeaderName| caller.headers.get(name).and_then(|value| value.to_str().ok());
    if !same_origin(header(header::ORIGIN), header(header::HOST)) {
        tracing::warn!(
            "rejected the calls socket from {}: page {}",
            peer_address(&caller),
            header(header::ORIGIN).unwrap_or_default()
        );
        return StatusCode::FORBIDDEN.into_response();
    }
    let address = peer_address(&caller);
    let calls = calls_service.clone();
    // A frame is refused by its header, as ws's maxPayload did: a message's limit applies only once the whole frame
    // is in, and the frame's own limit is 16 MiB by default.
    upgrade
        .max_frame_size(MAX_MESSAGE)
        .max_message_size(MAX_MESSAGE)
        .on_upgrade(move |socket| calls.on_socket(socket, address))
}

/// The other party's audio for the station's call, ended with the call: 16 kHz mono PCM16LE, or Ogg/Opus with
/// `?format=ogg` (the station's GStreamer). The home network's only.
async fn listen(
    Uses(calls): Uses<Arc<crate::calls::CallsService>>,
    caller: Caller,
    Path(device): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    caller.zone_at_least(Zone::Home)?;
    if let Some(refused) = refused(&calls, &caller) {
        return Ok(refused);
    }
    let format =
        if query.get("format").map(String::as_str) == Some("ogg") { ListenFormat::Ogg } else { ListenFormat::Pcm };
    let address = peer_address(&caller);
    let stream = calls.listen(&device, format, Some(address.as_str()).filter(|address| !address.is_empty()));
    let mut response = Body::from_stream(stream.map(Ok::<_, Infallible>)).into_response();
    let kind = if format == ListenFormat::Ogg { "audio/ogg" } else { "application/octet-stream" };
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// The station client's poll: the station is a party of our calls while its client polls; the answer is `call`
/// once its call is answered (then the client opens `listen`), `idle` after 50 s. The home network's only.
async fn next(
    Uses(calls): Uses<Arc<crate::calls::CallsService>>,
    caller: Caller,
    Path(device): Path<String>,
) -> Result<Response, ApiError> {
    caller.zone_at_least(Zone::Home)?;
    if let Some(refused) = refused(&calls, &caller) {
        return Ok(refused);
    }
    let address = peer_address(&caller);
    let answer = calls.next(&device, Some(address.as_str()).filter(|address| !address.is_empty())).await;
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], answer.as_str()).into_response())
}

/// The station client stops: its call's stream ends and the station leaves our calls.
async fn release(
    Uses(calls): Uses<Arc<crate::calls::CallsService>>,
    caller: Caller,
    Path(device): Path<String>,
) -> Result<Response, ApiError> {
    caller.zone_at_least(Zone::Home)?;
    if let Some(refused) = refused(&calls, &caller) {
        return Ok(refused);
    }
    calls.release(&device);
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// The station's microphone, 16 kHz mono PCM16LE, as a chunked upload. A sample split between two chunks is put
/// back together.
async fn talk(
    Uses(calls): Uses<Arc<crate::calls::CallsService>>,
    caller: Caller,
    Path(device): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    caller.zone_at_least(Zone::Home)?;
    if let Some(refused) = refused(&calls, &caller) {
        return Ok(refused);
    }
    let mut body = request.into_body().into_data_stream();
    let mut odd: Option<u8> = None;
    while let Some(Ok(chunk)) = body.next().await {
        let mut bytes = Vec::with_capacity(chunk.len() + 1);
        bytes.extend(odd.take());
        bytes.extend_from_slice(&chunk);
        if bytes.len() % 2 == 1 {
            odd = bytes.pop();
        }
        calls.talk(&device, &super::call_hub::pcm_from_bytes(&bytes));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}
