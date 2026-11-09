//! The Host API for apps over HTTP on this machine (`/api/v1/apps/host/…`): what a plugin's script gets as `host.*`, an
//! app's program gets here — with the token the supervisor gave it at its start (`DUOWAVE_APP_TOKEN`), and the same
//! rights its manifest asks for. Only from this machine: the app runs beside the gateway.
//!
//! - `POST /api/v1/apps/host/log` `{level, message}`
//! - `GET|PUT|DELETE /api/v1/apps/host/kv/{key}` — `PUT` takes the value as JSON
//! - `GET /api/v1/apps/host/kv?prefix=` — the keys
//! - `POST /api/v1/apps/host/say` `{text, device?}` → `{said}`
//! - `POST /api/v1/apps/host/fetch` `{url, method, headers, body, timeoutMs}` → the answer, by its `net:` rights
//! - `GET /api/v1/apps/host/settings`

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::app::App;
use crate::apps::supervisor::AppHost;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/apps/host/log", post(log))
        .route("/api/v1/apps/host/kv", get(keys))
        .route("/api/v1/apps/host/kv/{key}", get(kv_get).put(kv_set).delete(kv_delete))
        .route("/api/v1/apps/host/say", post(say))
        .route("/api/v1/apps/host/fetch", post(fetch))
        .route("/api/v1/apps/host/settings", get(settings))
        .route("/api/v1/apps/host/call/{name}", post(call))
}

fn refused(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// The app by its token, from this machine only; the refusal to answer otherwise.
fn app_of(app: &App, peer: std::net::SocketAddr, headers: &HeaderMap) -> Result<AppHost, Box<Response>> {
    if !peer.ip().is_loopback() {
        return Err(Box::new(refused(StatusCode::FORBIDDEN, "Host API приложений — только с этого компьютера")));
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    app.plugins
        .app_host(token)
        .ok_or_else(|| Box::new(refused(StatusCode::UNAUTHORIZED, "нет такого приложения: токен не тот")))
}

type Peer = ConnectInfo<std::net::SocketAddr>;

async fn log(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    match app_of(&app, peer, &headers) {
        Ok((_, host)) => {
            let level = body.get("level").and_then(Value::as_str).unwrap_or("info");
            host.log(level, body.get("message").and_then(Value::as_str).unwrap_or_default());
            StatusCode::NO_CONTENT.into_response()
        }
        Err(response) => *response,
    }
}

#[derive(serde::Deserialize)]
struct Prefix {
    #[serde(default)]
    prefix: String,
}

async fn keys(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Query(query): Query<Prefix>,
) -> Response {
    match app_of(&app, peer, &headers) {
        Ok((_, host)) => match host.kv_keys(&query.prefix) {
            Ok(keys) => Json(json!({ "keys": keys })).into_response(),
            Err(why) => refused(StatusCode::FORBIDDEN, &why),
        },
        Err(response) => *response,
    }
}

async fn kv_get(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Response {
    match app_of(&app, peer, &headers) {
        Ok((_, host)) => match host.kv_get(&key) {
            Ok(Some(value)) => Json(value).into_response(),
            Ok(None) => StatusCode::NOT_FOUND.into_response(),
            Err(why) => refused(StatusCode::FORBIDDEN, &why),
        },
        Err(response) => *response,
    }
}

async fn kv_set(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: Bytes,
) -> Response {
    match app_of(&app, peer, &headers) {
        Ok((_, host)) => {
            let Ok(value) = serde_json::from_slice::<Value>(&body) else {
                return refused(StatusCode::BAD_REQUEST, "значение — JSON");
            };
            match host.kv_set(&key, value) {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(why) => refused(StatusCode::FORBIDDEN, &why),
            }
        }
        Err(response) => *response,
    }
}

async fn kv_delete(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Response {
    match app_of(&app, peer, &headers) {
        Ok((_, host)) => match host.kv_delete(&key) {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(why) => refused(StatusCode::FORBIDDEN, &why),
        },
        Err(response) => *response,
    }
}

async fn say(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    match app_of(&app, peer, &headers) {
        Ok((_, host)) => {
            let text = body.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
            let device = body.get("device").and_then(Value::as_str).map(str::to_string);
            match host.say(text, device).await {
                Ok(said) => Json(json!({ "said": said })).into_response(),
                Err(why) => refused(StatusCode::FORBIDDEN, &why),
            }
        }
        Err(response) => *response,
    }
}

async fn fetch(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    match app_of(&app, peer, &headers) {
        Ok((_, host)) => match host.fetch(body).await {
            Ok(answer) => Json(answer).into_response(),
            Err(why) => refused(StatusCode::FORBIDDEN, &why),
        },
        Err(response) => *response,
    }
}

/// Host API 1.1: the gateway's services by name (`content.search`, `tools.call`, `smartHome.devices`…).
async fn call(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Path(name): Path<String>,
    body: Bytes,
) -> Response {
    match app_of(&app, peer, &headers) {
        Ok((_, host)) => {
            let input = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
            match host.call(&name, input).await {
                Ok(value) => Json(value).into_response(),
                Err(why) => refused(StatusCode::FORBIDDEN, &why),
            }
        }
        Err(response) => *response,
    }
}

async fn settings(State(app): State<Arc<App>>, ConnectInfo(peer): Peer, headers: HeaderMap) -> Response {
    match app_of(&app, peer, &headers) {
        Ok((_, host)) => Json(Value::Object(host.settings())).into_response(),
        Err(response) => *response,
    }
}
