//! The panel's screen «Выход в интернет» (panel/src/screens/egress.tsx):
//!
//! - `GET /api/v1/egress` → the simple mode, the home's country, the connections, every row as decided, the person's
//!   own addresses — whoever sees the apps;
//! - `PUT /api/v1/egress` `{mode?, preferred?}` — a button, the connection to go through first (`null` — the first
//!   that answers);
//! - `PUT /api/v1/egress/rows/{id}` `{route?, onDown?}` — a row's own route (`auto`, `direct`, a connection's id) and
//!   what it does when its server is down (`wait`, `direct`, `null` — the row's default);
//! - `DELETE /api/v1/egress/rows` — «Вернуть как было»;
//! - `PUT /api/v1/egress/custom` `{hosts}` — «Свои адреса», the whole list;
//! - `POST /api/v1/egress/seen` — the rows seen: «новое» goes.
//!
//! Changes — whoever may edit the settings.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post, put};
use axum::Router;
use serde_json::{json, Value};

use crate::app::App;
use crate::core::identity::permissions::{CONNECTORS_VIEW, SETTINGS_EDIT};
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller};

use super::registry::custom_host;
use super::rules::{Mode, OnDown, Route};
use super::Egress;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/egress", get(view).put(set))
        .route("/api/v1/egress/rows", axum::routing::delete(reset))
        .route("/api/v1/egress/rows/{id}", put(set_row))
        .route("/api/v1/egress/custom", put(set_custom))
        .route("/api/v1/egress/seen", post(seen))
}

/// The screen's data.
pub fn view_of(egress: &Egress) -> Value {
    let table = egress.table();
    let connections: Vec<Value> = table
        .connections
        .iter()
        .map(|one| {
            json!({
                "id": one.id, "name": one.name, "country": one.country, "current": one.current,
                "up": one.up(), "app": one.app,
            })
        })
        .collect();
    let rows: Vec<Value> = table
        .rows
        .iter()
        .map(|row| {
            json!({
                "id": row.goer.id,
                "name": row.goer.name,
                "section": row.goer.section,
                "hosts": row.goer.hosts,
                "keyed": row.goer.keyed,
                "viaEntry": row.goer.via_entry,
                "route": row.route.as_text(),
                "onDown": row.on_down,
                "onDownOwn": row.on_down_own,
                "decision": row.decision,
                "why": row.why,
                "reach": row.reach,
                "learned": row.learned,
                "new": row.new,
            })
        })
        .collect();
    json!({
        "mode": table.mode,
        "home": table.home,
        "preferred": table.preferred,
        "ownRules": egress.has_own_rules(),
        "app": egress.connections_app(),
        "anyReported": table.any_reported,
        "connections": connections,
        "rows": rows,
        "custom": egress.custom(),
        "reachUpdated": table.reach_updated,
    })
}

async fn view(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(view_of(&app.egress)))
}

async fn set(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<Value> {
    caller.permitted(SETTINGS_EDIT)?;
    if let Some(mode) = body.get("mode") {
        let mode = mode
            .as_str()
            .and_then(Mode::parse)
            .ok_or_else(|| ApiError::bad_request(crate::tr!("mode: off, blocked или all")))?;
        app.egress.set_mode(mode, caller.actor())?;
    }
    if let Some(preferred) = body.get("preferred") {
        let id = match preferred {
            Value::Null => None,
            Value::String(id) => match Route::parse(id) {
                Some(Route::Via(id)) => Some(id),
                _ => return Err(ApiError::bad_request(crate::tr!("preferred: id подключения или null"))),
            },
            _ => return Err(ApiError::bad_request(crate::tr!("preferred: id подключения или null"))),
        };
        app.egress.set_preferred(id.as_deref(), caller.actor())?;
    }
    Ok(ok(view_of(&app.egress)))
}

async fn set_row(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<Value> {
    caller.permitted(SETTINGS_EDIT)?;
    if id.is_empty() || id.len() > 200 {
        return Err(ApiError::bad_request(crate::tr!("нет такой строки")));
    }
    let route = match body.get("route") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(text)) => Some(Some(
            Route::parse(text)
                .ok_or_else(|| ApiError::bad_request(crate::tr!("route: auto, direct или id подключения")))?,
        )),
        Some(_) => return Err(ApiError::bad_request(crate::tr!("route: auto, direct или id подключения"))),
    };
    let on_down = match body.get("onDown") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(text)) => Some(Some(
            OnDown::parse(text).ok_or_else(|| ApiError::bad_request(crate::tr!("onDown: wait, direct или null")))?,
        )),
        Some(_) => return Err(ApiError::bad_request(crate::tr!("onDown: wait, direct или null"))),
    };
    if !app.egress.table().rows.iter().any(|row| row.goer.id == id) {
        return Err(ApiError::not_found(crate::tr!("нет такой строки")));
    }
    app.egress.set_row(&id, route, on_down, caller.actor())?;
    Ok(ok(view_of(&app.egress)))
}

async fn reset(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(SETTINGS_EDIT)?;
    app.egress.reset_rows(caller.actor())?;
    Ok(ok(view_of(&app.egress)))
}

async fn set_custom(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<Value> {
    caller.permitted(SETTINGS_EDIT)?;
    let list = body
        .get("hosts")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::bad_request(crate::tr!("hosts: список адресов")))?;
    if list.len() > 100 {
        return Err(ApiError::bad_request(crate::tr!("своих адресов — до 100")));
    }
    let mut hosts = Vec::new();
    for item in list {
        let text = item.as_str().unwrap_or_default();
        let host = custom_host(text).ok_or_else(|| {
            ApiError::bad_request(crate::tr!(
                "«{text}» — не адрес в интернете: имя вроде example.com или *.example.com",
                text = text
            ))
        })?;
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    app.egress.set_custom(hosts, caller.actor())?;
    Ok(ok(view_of(&app.egress)))
}

async fn seen(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_VIEW)?;
    app.egress.mark_seen()?;
    Ok(ok(json!({ "seen": true })))
}
