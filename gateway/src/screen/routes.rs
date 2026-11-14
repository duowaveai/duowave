//! `/api/v1/screen`: the windows of the computer's screen for their own pages (panel/src/screen). A window's page reads
//! what it shows, and its buttons — «Оставить», pause, the cross — and the end of its video come back here. Only from
//! this computer: the windows are its own.
use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::core::identity::permissions::STATION_VIEW;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Zone};

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/screen", get(view)).route("/api/v1/screen/{id}", post(act))
}

async fn view(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(json!({ "available": app.screen.available(), "windows": app.screen.windows() })))
}

#[derive(Deserialize)]
struct ActBody {
    /// keep, temporary, close, pause, resume, ended.
    action: String,
}

async fn act(
    State(app): State<Arc<App>>,
    caller: Caller,
    UrlPath(id): UrlPath<String>,
    Body(body): Body<ActBody>,
) -> ApiResult<Value> {
    caller.permitted(STATION_VIEW)?;
    caller.zone_at_least(Zone::Local)?;
    let screen = &app.screen;
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
