//! `/api/v1/tasks`: the apps' long tasks for the panel. Whoever may use the apps sees what they are at and may ask to
//! cancel it — the same right an app's window calls its backend with (plugins/routes.rs, `panel_call`).
//!
//! - `GET /api/v1/tasks?app=<id>` → `{tasks}`: of one app or of all, what runs first, then the newest;
//! - `GET /api/v1/tasks/{id}` → the task with its journal (`log`);
//! - `POST /api/v1/tasks/{id}/cancel` → the task: the app is asked to stop and ends it itself.

use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{TaskError, TaskLog};
use crate::core::identity::permissions::use_of;
use crate::platform::http::{ok, Api, ApiError, ApiResult, Caller, Uses};
use crate::skills::capabilities::CapabilityName;
use crate::tasks::Tasks;

pub fn routes<S: crate::tasks::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/tasks", get(list))
        .route("/api/v1/tasks/{id}", get(one))
        .route("/api/v1/tasks/{id}/cancel", post(cancel))
}

impl From<TaskError> for ApiError {
    fn from(error: TaskError) -> ApiError {
        let message = error.to_string();
        match error {
            TaskError::TooMany { .. } => ApiError::too_many(message),
            TaskError::NotFound => ApiError::not_found(message),
            TaskError::Ended(_) => ApiError::conflict(message),
            TaskError::Invalid(_) => ApiError::bad_request(message),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct Whose {
    #[serde(default)]
    app: Option<String>,
}

async fn list(Uses(tasks): Uses<Arc<Tasks>>, caller: Caller, Query(whose): Query<Whose>) -> ApiResult<Value> {
    caller.permitted(&use_of(CapabilityName::Plugins))?;
    Ok(ok(json!({ "tasks": tasks.list(whose.app.as_deref().filter(|app| !app.is_empty())) })))
}

async fn one(Uses(tasks): Uses<Arc<Tasks>>, caller: Caller, Path(id): Path<String>) -> ApiResult<TaskLog> {
    caller.permitted(&use_of(CapabilityName::Plugins))?;
    Ok(ok(tasks.get(&id).ok_or(TaskError::NotFound)?))
}

/// «Отменить»: answers 200, not 201 — nothing is made.
async fn cancel(Uses(tasks): Uses<Arc<Tasks>>, caller: Caller, Path(id): Path<String>) -> ApiResult<super::Task> {
    caller.permitted(&use_of(CapabilityName::Plugins))?;
    Ok(Api(axum::http::StatusCode::OK, tasks.cancel(&id)?))
}
