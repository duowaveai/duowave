//! The panel's «Память» window (docs/plans/memory.md, «Управление памятью»): `/api/v1/memory/…`. Who may look needs
//! `memory.view`; who may change, switch or erase needs `memory.manage`. The pause takes only `memory.view`: it stops
//! writing and wipes nothing, and the voice can set it too.

use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use super::commands::PauseSpan;
use super::model::EntityKind;
use super::reflect::{ReflectionChange, ReflectionView};
use super::service::{EntityCard, EntityItem, FactView, MemoryState, Overview};
use super::store::{Change, Entity};
use crate::core::identity::permissions::{MEMORY_MANAGE, MEMORY_VIEW};
use crate::memory::reflect::Reflection;
use crate::memory::MemoryService;
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Uses};

pub fn routes<S: crate::memory::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/memory", get(overview))
        .route("/api/v1/memory/state", axum::routing::put(set_state))
        .route("/api/v1/memory/pause", post(pause).delete(resume))
        .route("/api/v1/memory/entities", get(entities).post(add_entity))
        .route("/api/v1/memory/entities/{id}", get(entity).patch(update_entity).delete(delete_entity))
        .route("/api/v1/memory/entities/{id}/merge", post(merge_entity))
        .route("/api/v1/memory/facts", post(add_fact))
        .route("/api/v1/memory/facts/{id}", axum::routing::patch(update_fact).delete(erase_fact))
        .route("/api/v1/memory/facts/{id}/close", post(close_fact))
        .route("/api/v1/memory/facts/{id}/reopen", post(reopen_fact))
        .route("/api/v1/memory/changes", get(changes))
        .route("/api/v1/memory/changes/{id}/undo", post(undo))
        .route("/api/v1/memory/reflection", get(reflection).put(set_reflection))
}

/// What a change answers with when there is nothing else to show.
#[derive(Debug, Serialize)]
pub struct Done {
    pub done: bool,
}

const DONE: Done = Done { done: true };

pub async fn overview(Uses(memory): Uses<Arc<MemoryService>>, caller: Caller) -> ApiResult<Overview> {
    let principal = caller.permitted(MEMORY_VIEW)?;
    Ok(ok(memory.overview(&memory.viewer_for(principal))?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateBody {
    pub enabled: Option<bool>,
    pub in_passing: Option<bool>,
}

pub async fn set_state(
    Uses(memory): Uses<Arc<MemoryService>>,
    caller: Caller,
    Body(body): Body<StateBody>,
) -> ApiResult<MemoryState> {
    caller.permitted(MEMORY_MANAGE)?;
    if let Some(on) = body.enabled {
        memory.set_enabled(on, caller.actor())?;
    }
    if let Some(on) = body.in_passing {
        memory.set_in_passing(on, caller.actor())?;
    }
    Ok(ok(memory.state()))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PauseBody {
    /// hour | morning | manual
    pub span: Option<String>,
    /// The browser's time zone, for «до утра».
    pub timezone: Option<String>,
}

pub async fn pause(
    Uses(memory): Uses<Arc<MemoryService>>,
    caller: Caller,
    Body(body): Body<PauseBody>,
) -> ApiResult<MemoryState> {
    caller.permitted(MEMORY_VIEW)?;
    let span = match body.span.as_deref() {
        Some("hour") => PauseSpan::Hour,
        Some("morning") | None => PauseSpan::Morning,
        Some("manual") => PauseSpan::UntilResumed,
        Some(other) => return Err(ApiError::bad_request(crate::tr!("Нет такой паузы: {name}.", name = other))),
    };
    let (state, _) = memory.pause(span, body.timezone.as_deref(), caller.actor())?;
    Ok(ok(state))
}

async fn resume(Uses(memory): Uses<Arc<MemoryService>>, caller: Caller) -> ApiResult<MemoryState> {
    caller.permitted(MEMORY_VIEW)?;
    Ok(ok(memory.resume(caller.actor())?))
}

pub async fn entities(Uses(memory): Uses<Arc<MemoryService>>, caller: Caller) -> ApiResult<Vec<EntityItem>> {
    let principal = caller.permitted(MEMORY_VIEW)?;
    Ok(ok(memory.entity_list(&memory.viewer_for(principal))?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewEntityBody {
    kind: Option<String>,
    name: String,
}

async fn add_entity(
    Uses(memory): Uses<Arc<MemoryService>>,
    caller: Caller,
    Body(body): Body<NewEntityBody>,
) -> ApiResult<Entity> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    let kind = match body.kind.as_deref() {
        None => EntityKind::Person,
        Some(kind) => EntityKind::parse(kind)
            .ok_or_else(|| ApiError::bad_request(crate::tr!("Нет такого вида: {kind}.", kind = kind)))?,
    };
    Ok(created(memory.add_entity(kind, &body.name, principal)?))
}

pub async fn entity(
    Uses(memory): Uses<Arc<MemoryService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<EntityCard> {
    let principal = caller.permitted(MEMORY_VIEW)?;
    Ok(ok(memory.entity_card(&id, &memory.viewer_for(principal))?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntityBody {
    name: Option<String>,
    aliases: Option<Vec<String>>,
}

async fn update_entity(
    Uses(memory): Uses<Arc<MemoryService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<EntityBody>,
) -> ApiResult<Entity> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    Ok(ok(memory.update_entity(&id, body.name.as_deref(), body.aliases.as_deref(), principal)?))
}

async fn delete_entity(
    Uses(memory): Uses<Arc<MemoryService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Done> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    memory.delete_entity(&id, principal)?;
    Ok(ok(DONE))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeBody {
    into: String,
}

async fn merge_entity(
    Uses(memory): Uses<Arc<MemoryService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<MergeBody>,
) -> ApiResult<Entity> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    Ok(ok(memory.merge_entities(&id, &body.into, principal)?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewFactBody {
    subject_id: String,
    text: String,
    #[serde(default)]
    pinned: bool,
}

async fn add_fact(
    Uses(memory): Uses<Arc<MemoryService>>,
    caller: Caller,
    Body(body): Body<NewFactBody>,
) -> ApiResult<FactView> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    Ok(created(memory.add_fact(&body.subject_id, &body.text, body.pinned, principal)?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactBody {
    text: Option<String>,
    pinned: Option<bool>,
}

async fn update_fact(
    Uses(memory): Uses<Arc<MemoryService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<FactBody>,
) -> ApiResult<FactView> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    Ok(ok(memory.update_fact(&id, body.text.as_deref(), body.pinned, principal)?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseBody {
    reason: Option<String>,
}

pub async fn close_fact(
    Uses(memory): Uses<Arc<MemoryService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<CloseBody>,
) -> ApiResult<Done> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    memory.close_fact(&id, body.reason.as_deref(), principal)?;
    Ok(ok(DONE))
}

async fn reopen_fact(
    Uses(memory): Uses<Arc<MemoryService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Done> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    memory.reopen_fact(&id, principal)?;
    Ok(ok(DONE))
}

pub async fn erase_fact(
    Uses(memory): Uses<Arc<MemoryService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Done> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    memory.erase_fact(&id, principal)?;
    Ok(ok(DONE))
}

#[derive(Debug, Default, Deserialize)]
pub struct ChangesQuery {
    pub limit: Option<i64>,
}

pub async fn changes(
    Uses(memory): Uses<Arc<MemoryService>>,
    caller: Caller,
    Query(query): Query<ChangesQuery>,
) -> ApiResult<Vec<Change>> {
    caller.permitted(MEMORY_VIEW)?;
    Ok(ok(memory.changes(query.limit.unwrap_or(50))?))
}

pub async fn undo(Uses(memory): Uses<Arc<MemoryService>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Done> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    memory.undo(&id, principal)?;
    Ok(ok(DONE))
}

/// «Сон и рефлексия» in the memory's settings: the reflection's switches and which model thinks at night.
async fn reflection(Uses(reflection): Uses<Arc<Reflection>>, caller: Caller) -> ApiResult<ReflectionView> {
    caller.permitted(MEMORY_VIEW)?;
    Ok(ok(reflection.view()))
}

async fn set_reflection(
    Uses(reflection): Uses<Arc<Reflection>>,
    caller: Caller,
    Body(body): Body<ReflectionChange>,
) -> ApiResult<ReflectionView> {
    caller.permitted(MEMORY_MANAGE)?;
    Ok(ok(reflection.change(body, caller.actor())?))
}
