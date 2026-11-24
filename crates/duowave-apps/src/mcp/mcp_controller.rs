//! MCP-серверы для пульта. Видит всякий, кто видит подключения (секреты — только «задан · …a1b2»); добавляет,
//! меняет, включает инструменты и пробует их человек с правом подключений и только из дома: сервер — это
//! программа на этом компьютере или доступ к чужим данным.

use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, patch, post, put};
use axum::Router;
use serde::Deserialize;
use serde_json::Value;

use super::mcp_import::{present, Json};
use super::mcp_service::{
    ImportDone, LoginDone, LoginStart, McpServerInput, McpServerView, McpView, ToolChange, TryResult,
};
use crate::core::identity::permissions::{CONNECTORS_MANAGE, CONNECTORS_VIEW};
use crate::core::identity::{Principal, PrincipalKind};
use crate::mcp::McpService;
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Uses, Zone};

pub fn routes<S: crate::mcp::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/mcp", get(view))
        .route("/api/v1/mcp/servers", post(add))
        .route("/api/v1/mcp/import", post(import))
        .route("/api/v1/mcp/servers/{id}", patch(update).delete(remove))
        .route("/api/v1/mcp/servers/{id}/tools/{name}", put(set_tool))
        .route("/api/v1/mcp/servers/{id}/instructions", post(approve))
        .route("/api/v1/mcp/servers/{id}/reconnect", post(reconnect))
        .route("/api/v1/mcp/servers/{id}/tools/{name}/try", post(try_tool))
        .route("/api/v1/mcp/servers/{id}/login", post(login))
        .route("/api/v1/mcp/oauth/callback", get(callback))
        .route("/api/v1/mcp/oauth/complete", post(complete))
}

/// Servers are added by people, never by a script's key: a program on this computer is someone's decision.
fn person(principal: &Principal) -> Result<(), ApiError> {
    if principal.kind != PrincipalKind::User {
        return Err(ApiError::forbidden(crate::tr!("Это для людей, не для ключей.")));
    }
    Ok(())
}

/// From home, with the right of connectors, a person.
fn manager(caller: &Caller) -> Result<(), ApiError> {
    caller.zone_at_least(Zone::Home)?;
    person(caller.permitted(CONNECTORS_MANAGE)?)
}

pub async fn view(Uses(mcp): Uses<Arc<McpService>>, caller: Caller) -> ApiResult<McpView> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(mcp.view()))
}

pub async fn add(
    Uses(mcp): Uses<Arc<McpService>>,
    caller: Caller,
    Body(input): Body<McpServerInput>,
) -> ApiResult<McpServerView> {
    manager(&caller)?;
    Ok(created(mcp.add(&input, caller.actor())?))
}

#[derive(Debug, Default, Deserialize)]
pub struct ImportBody {
    #[serde(default, deserialize_with = "present")]
    json: Option<Json>,
}

async fn import(
    Uses(mcp): Uses<Arc<McpService>>,
    caller: Caller,
    Body(body): Body<ImportBody>,
) -> ApiResult<ImportDone> {
    manager(&caller)?;
    Ok(created(mcp.import_json(body.json.as_ref(), caller.actor())?))
}

async fn update(
    Uses(mcp): Uses<Arc<McpService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(input): Body<McpServerInput>,
) -> ApiResult<McpServerView> {
    manager(&caller)?;
    Ok(ok(mcp.update(&id, &input, caller.actor())?))
}

async fn remove(
    Uses(mcp): Uses<Arc<McpService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    manager(&caller)?;
    mcp.remove(&id, caller.actor()).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn set_tool(
    Uses(mcp): Uses<Arc<McpService>>,
    caller: Caller,
    Path((id, name)): Path<(String, String)>,
    Body(change): Body<ToolChange>,
) -> ApiResult<McpServerView> {
    manager(&caller)?;
    Ok(ok(mcp.set_tool(&id, &name, &change, caller.actor())?))
}

async fn approve(Uses(mcp): Uses<Arc<McpService>>, caller: Caller, Path(id): Path<String>) -> ApiResult<McpServerView> {
    manager(&caller)?;
    Ok(ok(mcp.approve_instructions(&id, caller.actor())?))
}

async fn reconnect(
    Uses(mcp): Uses<Arc<McpService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<McpServerView> {
    caller.permitted(CONNECTORS_MANAGE)?;
    Ok(ok(mcp.reconnect(&id).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct TryBody {
    #[serde(default)]
    arguments: Option<Value>,
}

pub async fn try_tool(
    Uses(mcp): Uses<Arc<McpService>>,
    caller: Caller,
    Path((id, name)): Path<(String, String)>,
    Body(body): Body<TryBody>,
) -> ApiResult<TryResult> {
    manager(&caller)?;
    Ok(ok(mcp.try_tool(&id, &name, body.arguments.as_ref()).await?))
}

/// «Войти» on a server in the cloud: the page of the service for the owner's browser.
async fn login(Uses(mcp): Uses<Arc<McpService>>, caller: Caller, Path(id): Path<String>) -> ApiResult<LoginStart> {
    manager(&caller)?;
    Ok(ok(mcp.login(&id).await?))
}

/// The service sends the browser back here after the sign-in (to localhost: on this computer it lands by itself).
/// No sign-in to the panel is asked — the state of a sign-in begun from the panel is the check; a page in words
/// answers, never the service's own error text.
async fn callback(
    Uses(mcp): Uses<Arc<McpService>>,
    caller: Caller,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Response, ApiError> {
    caller.zone_at_least(Zone::Home)?;
    let (status, text) = match mcp.complete_login(&query, None).await {
        Ok(done) => (
            StatusCode::OK,
            crate::tr!(
                "Вход в «{title}» выполнен. Это окно можно закрыть: сервер уже подключается.",
                title = done.title
            ),
        ),
        Err(error) => (StatusCode::BAD_REQUEST, error.message),
    };
    Ok(crate::oauth::page::page(status, &text))
}

#[derive(Debug, Default, Deserialize)]
pub struct CompleteBody {
    #[serde(default, deserialize_with = "present")]
    url: Option<Json>,
}

/// From a phone: the address the browser could not open after the sign-in, pasted into the panel.
async fn complete(
    Uses(mcp): Uses<Arc<McpService>>,
    caller: Caller,
    Body(body): Body<CompleteBody>,
) -> ApiResult<LoginDone> {
    person(caller.permitted(CONNECTORS_MANAGE)?)?;
    let text = body.url.as_ref().and_then(Json::as_str).map(str::trim).unwrap_or_default();
    let url =
        url::Url::parse(text).map_err(|_| ApiError::bad_request(crate::tr!("Это не адрес страницы после входа.")))?;
    let query: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    Ok(ok(mcp.complete_login(&query, caller.actor()).await?))
}
