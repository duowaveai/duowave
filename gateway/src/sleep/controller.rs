//! The panel's sleep: `/api/v1/sleep/…` — its hour, «Запустить сон сейчас», the nights and «Откатить ночь». Its only
//! part so far is the memory's reflection, so looking takes `memory.view`, and setting the hour, starting a night and
//! rolling one back take `memory.manage`.

use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use super::{Run, RunDetail, SleepView};
use crate::core::identity::permissions::{MEMORY_MANAGE, MEMORY_VIEW};
use crate::platform::http::{ok, ApiResult, Body, Caller, Uses};
use crate::sleep::SleepService;

pub fn routes<S: crate::sleep::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/sleep", get(view).put(set))
        .route("/api/v1/sleep/run", post(run_now))
        .route("/api/v1/sleep/runs", get(runs))
        .route("/api/v1/sleep/runs/{id}", get(run))
        .route("/api/v1/sleep/runs/{id}/rollback", post(roll_back))
}

async fn view(Uses(sleep): Uses<Arc<SleepService>>, caller: Caller) -> ApiResult<SleepView> {
    caller.permitted(MEMORY_VIEW)?;
    Ok(ok(sleep.view()))
}

#[derive(Debug, Default, Deserialize)]
struct SetBody {
    /// "HH:MM" on the station's clock.
    time: Option<String>,
}

async fn set(Uses(sleep): Uses<Arc<SleepService>>, caller: Caller, Body(body): Body<SetBody>) -> ApiResult<SleepView> {
    caller.permitted(MEMORY_MANAGE)?;
    if let Some(time) = body.time {
        sleep.set_time(&time, caller.actor())?;
    }
    Ok(ok(sleep.view()))
}

#[derive(Debug, Serialize)]
struct Started {
    id: String,
}

async fn run_now(Uses(sleep): Uses<Arc<SleepService>>, caller: Caller) -> ApiResult<Started> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    Ok(ok(Started { id: sleep.run_now(principal)? }))
}

#[derive(Debug, Default, Deserialize)]
struct RunsQuery {
    limit: Option<i64>,
}

async fn runs(
    Uses(sleep): Uses<Arc<SleepService>>,
    caller: Caller,
    Query(query): Query<RunsQuery>,
) -> ApiResult<Vec<Run>> {
    caller.permitted(MEMORY_VIEW)?;
    Ok(ok(sleep.runs(query.limit.unwrap_or(30))?))
}

async fn run(Uses(sleep): Uses<Arc<SleepService>>, caller: Caller, Path(id): Path<String>) -> ApiResult<RunDetail> {
    caller.permitted(MEMORY_VIEW)?;
    Ok(ok(sleep.run(&id)?))
}

async fn roll_back(Uses(sleep): Uses<Arc<SleepService>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Run> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    Ok(ok(sleep.roll_back(&id, principal)?))
}
