//! `/api/v1/screen`: the windows of the computer's screen for their own pages (panel/src/screen). A window's page reads
//! what it shows, and its buttons — «Оставить», pause, the cross — and the end of its video come back here. Only from
//! this computer: the windows are its own.
use std::sync::Arc;

use axum::extract::Path as UrlPath;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::core::identity::permissions::STATION_VIEW;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Uses, Zone};
use crate::screen::Screen;

pub fn routes<S: crate::screen::RouteNeeds>() -> Router<S> {
    Router::new().route("/api/v1/screen", get(view)).route("/api/v1/screen/{id}", post(act))
}

async fn view(Uses(screen): Uses<Arc<Screen>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(json!({ "available": screen.available(), "windows": screen.windows() })))
}

#[derive(Deserialize)]
struct ActBody {
    /// keep, temporary, close, pause, resume, ended.
    action: String,
}

async fn act(
    Uses(screen_service): Uses<Arc<Screen>>,
    caller: Caller,
    UrlPath(id): UrlPath<String>,
    Body(body): Body<ActBody>,
) -> ApiResult<Value> {
    caller.permitted(STATION_VIEW)?;
    caller.zone_at_least(Zone::Local)?;
    let screen = &screen_service;
    if !screen.windows().iter().any(|window| window.id == id) {
        return Err(ApiError::not_found(crate::tr!("окна уже нет")));
    }
    let done = match body.action.as_str() {
        "keep" => screen.keep(&id, true).map(|_| ()),
        "temporary" => screen.keep(&id, false).map(|_| ()),
        "close" => screen.close(&id).map(|_| ()),
        "pause" => screen.pause(Some(&id)).map(|_| ()),
        "resume" => screen.resume(Some(&id)).map(|_| ()),
        "ended" => {
            screen.ended(&id);
            Ok(())
        }
        other => return Err(ApiError::bad_request(crate::tr!("Окно не умеет «{other}».", other = other))),
    };
    done.map_err(ApiError::bad_request)?;
    Ok(ok(json!({ "windows": screen.windows() })))
}
