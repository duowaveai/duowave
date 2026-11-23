//! `/health`: open to all — the deploy's checks call it unsigned — but the details (routes, engines, addresses of our
//! servers) only for this computer and the home network, or for someone signed in.

use std::sync::Arc;

use axum::routing::get;
use axum::Router;
use serde_json::Value;

use super::service::PublicHealth;
use crate::health_status::HealthStatusService;
use crate::platform::http::{ok, ApiResult, Caller, Uses, Zone};

pub fn routes<S: crate::health_status::RouteNeeds>() -> Router<S> {
    Router::new().route("/health", get(health))
}

async fn health(Uses(health): Uses<Arc<HealthStatusService>>, caller: Caller) -> ApiResult<Value> {
    let status = health.status();
    if caller.principal().is_some() || matches!(caller.zone, Zone::Local | Zone::Home) {
        return Ok(ok(serde_json::to_value(status)?));
    }
    Ok(ok(serde_json::to_value(PublicHealth { status: status.status })?))
}
