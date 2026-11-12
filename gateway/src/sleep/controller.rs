//! The panel's sleep: `/api/v1/sleep/…` — its hour, «Запустить сон сейчас», the nights and «Откатить ночь». Its only
//! part so far is the memory's reflection, so looking takes `memory.view`, and setting the hour, starting a night and
//! rolling one back take `memory.manage`.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use super::{Run, RunDetail, SleepView};
use crate::app::App;
use crate::core::identity::permissions::{MEMORY_MANAGE, MEMORY_VIEW};
use crate::platform::http::{ok, ApiResult, Body, Caller};

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/sleep", get(view).put(set))
        .route("/api/v1/sleep/run", post(run_now))
        .route("/api/v1/sleep/runs", get(runs))
        .route("/api/v1/sleep/runs/{id}", get(run))
        .route("/api/v1/sleep/runs/{id}/rollback", post(roll_back))
}

async fn view(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<SleepView> {
    caller.permitted(MEMORY_VIEW)?;
    Ok(ok(app.sleep.view()))
}

#[derive(Debug, Default, Deserialize)]
struct SetBody {
    /// "HH:MM" on the station's clock.
    time: Option<String>,
}

async fn set(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<SetBody>) -> ApiResult<SleepView> {
    caller.permitted(MEMORY_MANAGE)?;
    if let Some(time) = body.time {
        app.sleep.set_time(&time, caller.actor())?;
    }
    Ok(ok(app.sleep.view()))
}

#[derive(Debug, Serialize)]
struct Started {
    id: String,
}

async fn run_now(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Started> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    Ok(ok(Started { id: app.sleep.run_now(principal)? }))
}

#[derive(Debug, Default, Deserialize)]
struct RunsQuery {
    limit: Option<i64>,
}

async fn runs(State(app): State<Arc<App>>, caller: Caller, Query(query): Query<RunsQuery>) -> ApiResult<Vec<Run>> {
    caller.permitted(MEMORY_VIEW)?;
    Ok(ok(app.sleep.runs(query.limit.unwrap_or(30))?))
}

async fn run(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<RunDetail> {
    caller.permitted(MEMORY_VIEW)?;
    Ok(ok(app.sleep.run(&id)?))
}

async fn roll_back(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Run> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    Ok(ok(app.sleep.roll_back(&id, principal)?))
}
