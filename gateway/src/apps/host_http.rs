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
//!
//! Long tasks (src/tasks; docs/plugins/AUTHORING.md, «Долгие задачи»): work that takes longer than a call may.
//!
//! - `POST /api/v1/apps/host/tasks` `{title, text?, percent?}` → 201, the task with its `id`; 429 when the app already
//!   runs as many as it may (`budget.tasks`, one by default)
//! - `GET /api/v1/apps/host/tasks` → `{tasks}`: its own, what was interrupted while it was down too
//! - `GET /api/v1/apps/host/tasks/{id}?waitMs=` → the task; with `waitMs` (up to a minute) the answer comes once a
//!   person asked to cancel it (`cancelRequested`) or it ended — or when the wait is over
//! - `POST /api/v1/apps/host/tasks/{id}/progress` `{percent?, text?}` → `{cancelRequested}`
//! - `POST /api/v1/apps/host/tasks/{id}/done` `{result?}`, `…/fail` `{error}`, `…/cancelled` → the task as it ended;
//!   409 with its `state` when it is over already (cancelled after the grace, interrupted)

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
use crate::tasks::{Outcome, TaskError};

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/apps/host/log", post(log))
        .route("/api/v1/apps/host/kv", get(keys))
        .route("/api/v1/apps/host/kv/{key}", get(kv_get).put(kv_set).delete(kv_delete))
        .route("/api/v1/apps/host/say", post(say))
        .route("/api/v1/apps/host/fetch", post(fetch))
        .route("/api/v1/apps/host/settings", get(settings))
        .route("/api/v1/apps/host/call/{name}", post(call))
        .route("/api/v1/apps/host/tasks", get(tasks).post(task_start))
        .route("/api/v1/apps/host/tasks/{id}", get(task))
        .route("/api/v1/apps/host/tasks/{id}/progress", post(task_progress))
        .route("/api/v1/apps/host/tasks/{id}/{end}", post(task_end))
}

fn refused(status: StatusCode, message: impl AsRef<str>) -> Response {
    (status, Json(json!({ "error": message.as_ref() }))).into_response()
}

/// The app by its token, from this machine only; the refusal to answer otherwise.
fn app_of(app: &App, peer: std::net::SocketAddr, headers: &HeaderMap) -> Result<AppHost, Box<Response>> {
    if !peer.ip().is_loopback() {
        return Err(Box::new(refused(
            StatusCode::FORBIDDEN,
            crate::tr!("Host API приложений — только с этого компьютера"),
        )));
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    app.plugins
        .app_host(token)
        .ok_or_else(|| Box::new(refused(StatusCode::UNAUTHORIZED, crate::tr!("нет такого приложения: токен не тот"))))
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
                return refused(StatusCode::BAD_REQUEST, crate::tr!("значение — JSON"));
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

/// A refusal of the tasks: its words, and how the task ended when it is over.
fn task_refused(error: TaskError) -> Response {
    let status = match error {
        TaskError::TooMany { .. } => StatusCode::TOO_MANY_REQUESTS,
        TaskError::NotFound => StatusCode::NOT_FOUND,
        TaskError::Ended(_) => StatusCode::CONFLICT,
        TaskError::Invalid(_) => StatusCode::BAD_REQUEST,
    };
    let mut body = json!({ "error": error.to_string() });
    if let TaskError::Ended(state) = error {
        body["state"] = json!(state);
    }
    (status, Json(body)).into_response()
}

async fn tasks(State(app): State<Arc<App>>, ConnectInfo(peer): Peer, headers: HeaderMap) -> Response {
    match app_of(&app, peer, &headers) {
        Ok((id, _)) => Json(json!({ "tasks": app.tasks.list(Some(&id)) })).into_response(),
        Err(response) => *response,
    }
}

async fn task_start(State(app): State<Arc<App>>, ConnectInfo(peer): Peer, headers: HeaderMap, body: Bytes) -> Response {
    match app_of(&app, peer, &headers) {
        Ok((id, host)) => {
            let body = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
            let started = app.tasks.start(
                &id,
                body.get("title").and_then(Value::as_str).unwrap_or_default(),
                body.get("text").and_then(Value::as_str),
                body.get("percent").and_then(Value::as_f64),
                app.plugins.tasks_at_once(&id),
            );
            match started {
                Ok(task) => {
                    host.log("info", &crate::tr!("задача «{title}» начата", title = task.title));
                    (StatusCode::CREATED, Json(task)).into_response()
                }
                Err(error) => task_refused(error),
            }
        }
        Err(response) => *response,
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Wait {
    #[serde(default)]
    wait_ms: u64,
}

/// The longest an app's wait for a cancel is held: it asks again after.
const WAIT_MS: u64 = 60_000;

async fn task(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Path(task): Path<String>,
    Query(wait): Query<Wait>,
) -> Response {
    match app_of(&app, peer, &headers) {
        Ok((id, _)) => {
            let found = if wait.wait_ms == 0 {
                app.tasks.own(&id, &task)
            } else {
                app.tasks.wait(&id, &task, std::time::Duration::from_millis(wait.wait_ms.min(WAIT_MS))).await
            };
            match found {
                Ok(task) => Json(task).into_response(),
                Err(error) => task_refused(error),
            }
        }
        Err(response) => *response,
    }
}

async fn task_progress(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Path(task): Path<String>,
    body: Bytes,
) -> Response {
    match app_of(&app, peer, &headers) {
        Ok((id, _)) => {
            let body = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
            let percent = body.get("percent").and_then(Value::as_f64);
            match app.tasks.progress(&id, &task, percent, body.get("text").and_then(Value::as_str)) {
                Ok(asked) => Json(json!({ "cancelRequested": asked })).into_response(),
                Err(error) => task_refused(error),
            }
        }
        Err(response) => *response,
    }
}

/// How the app ends its task: `done` `{result?}`, `fail` `{error}`, `cancelled`.
async fn task_end(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Path((task, end)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    match app_of(&app, peer, &headers) {
        Ok((id, host)) => {
            let mut body = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
            let outcome = match end.as_str() {
                "done" => Outcome::Done(body.get_mut("result").map(Value::take)),
                "fail" => Outcome::Failed(body.get("error").and_then(Value::as_str).unwrap_or_default().to_string()),
                "cancelled" => Outcome::Cancelled,
                _ => return refused(StatusCode::NOT_FOUND, crate::tr!("задачу заканчивают done, fail или cancelled")),
            };
            match app.tasks.finish(&id, &task, outcome) {
                Ok(task) => {
                    let how = match (&task.state, &task.error) {
                        (crate::tasks::TaskState::Failed, Some(error)) => {
                            crate::tr!("не получилась: {error}", error = error)
                        }
                        (crate::tasks::TaskState::Cancelled, _) => crate::tr!("отменена").to_string(),
                        _ => crate::tr!("готова").to_string(),
                    };
                    host.log("info", &crate::tr!("задача «{title}» {how}", title = task.title, how = how));
                    Json(task).into_response()
                }
                Err(error) => task_refused(error),
            }
        }
        Err(response) => *response,
    }
}
