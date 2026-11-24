//! Plain HTTP the station sends to the uniproxy host. Answers keep the station's format (no envelope).

use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;

pub fn router<S: crate::gateway::RouteNeeds>() -> Router<S> {
    // The connectivity check the station makes against the host from its uniProxyUrl.
    Router::new().route("/generate_204", get(|| async { StatusCode::NO_CONTENT }))
}
