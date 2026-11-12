//! The panel's «Память» window (docs/plans/memory.md, «Управление памятью»): `/api/v1/memory/…`. Who may look needs
//! `memory.view`; who may change, switch or erase needs `memory.manage`. The pause takes only `memory.view`: it stops
//! writing and wipes nothing, and the voice can set it too.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use super::commands::PauseSpan;
use super::model::EntityKind;
use super::reflect::{ReflectionChange, ReflectionView};
use super::service::{EntityCard, EntityItem, FactView, MemoryState, Overview};
use super::store::{Change, Entity};
use crate::app::App;
use crate::core::identity::permissions::{MEMORY_MANAGE, MEMORY_VIEW};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller};

pub fn routes() -> Router<Arc<App>> {
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
struct Done {
    done: bool,
}

const DONE: Done = Done { done: true };

async fn overview(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Overview> {
    let principal = caller.permitted(MEMORY_VIEW)?;
    Ok(ok(app.memory.overview(&app.memory.viewer_for(principal))?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StateBody {
    enabled: Option<bool>,
    in_passing: Option<bool>,
}

async fn set_state(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<StateBody>) -> ApiResult<MemoryState> {
    caller.permitted(MEMORY_MANAGE)?;
    if let Some(on) = body.enabled {
        app.memory.set_enabled(on, caller.actor())?;
    }
    if let Some(on) = body.in_passing {
        app.memory.set_in_passing(on, caller.actor())?;
    }
    Ok(ok(app.memory.state()))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PauseBody {
    /// hour | morning | manual
    span: Option<String>,
    /// The browser's time zone, for «до утра».
    timezone: Option<String>,
}

async fn pause(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<PauseBody>) -> ApiResult<MemoryState> {
    caller.permitted(MEMORY_VIEW)?;
    let span = match body.span.as_deref() {
        Some("hour") => PauseSpan::Hour,
        Some("morning") | None => PauseSpan::Morning,
        Some("manual") => PauseSpan::UntilResumed,
        Some(other) => return Err(ApiError::bad_request(format!("Нет такой паузы: {other}."))),
    };
    let (state, _) = app.memory.pause(span, body.timezone.as_deref(), caller.actor())?;
    Ok(ok(state))
}

async fn resume(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<MemoryState> {
    caller.permitted(MEMORY_VIEW)?;
    Ok(ok(app.memory.resume(caller.actor())?))
}

async fn entities(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Vec<EntityItem>> {
    let principal = caller.permitted(MEMORY_VIEW)?;
    Ok(ok(app.memory.entity_list(&app.memory.viewer_for(principal))?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewEntityBody {
    kind: Option<String>,
    name: String,
}

async fn add_entity(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<NewEntityBody>) -> ApiResult<Entity> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    let kind = match body.kind.as_deref() {
        None => EntityKind::Person,
        Some(kind) => {
            EntityKind::parse(kind).ok_or_else(|| ApiError::bad_request(format!("Нет такого вида: {kind}.")))?
        }
    };
    Ok(created(app.memory.add_entity(kind, &body.name, principal)?))
}

async fn entity(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<EntityCard> {
    let principal = caller.permitted(MEMORY_VIEW)?;
    Ok(ok(app.memory.entity_card(&id, &app.memory.viewer_for(principal))?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntityBody {
    name: Option<String>,
    aliases: Option<Vec<String>>,
}

async fn update_entity(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<EntityBody>,
) -> ApiResult<Entity> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    Ok(ok(app.memory.update_entity(&id, body.name.as_deref(), body.aliases.as_deref(), principal)?))
}

async fn delete_entity(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Done> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    app.memory.delete_entity(&id, principal)?;
    Ok(ok(DONE))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeBody {
    into: String,
}

async fn merge_entity(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<MergeBody>,
) -> ApiResult<Entity> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    Ok(ok(app.memory.merge_entities(&id, &body.into, principal)?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewFactBody {
    subject_id: String,
    text: String,
    #[serde(default)]
    pinned: bool,
}

async fn add_fact(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<NewFactBody>) -> ApiResult<FactView> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    Ok(created(app.memory.add_fact(&body.subject_id, &body.text, body.pinned, principal)?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactBody {
    text: Option<String>,
    pinned: Option<bool>,
}

async fn update_fact(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<FactBody>,
) -> ApiResult<FactView> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    Ok(ok(app.memory.update_fact(&id, body.text.as_deref(), body.pinned, principal)?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloseBody {
    reason: Option<String>,
}

async fn close_fact(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<CloseBody>,
) -> ApiResult<Done> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    app.memory.close_fact(&id, body.reason.as_deref(), principal)?;
    Ok(ok(DONE))
}

async fn reopen_fact(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Done> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    app.memory.reopen_fact(&id, principal)?;
    Ok(ok(DONE))
}

async fn erase_fact(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Done> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    app.memory.erase_fact(&id, principal)?;
    Ok(ok(DONE))
}

#[derive(Debug, Default, Deserialize)]
struct ChangesQuery {
    limit: Option<i64>,
}

async fn changes(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(query): Query<ChangesQuery>,
) -> ApiResult<Vec<Change>> {
    caller.permitted(MEMORY_VIEW)?;
    Ok(ok(app.memory.changes(query.limit.unwrap_or(50))?))
}

async fn undo(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Done> {
    let principal = caller.permitted(MEMORY_MANAGE)?;
    app.memory.undo(&id, principal)?;
    Ok(ok(DONE))
}

/// «Сон и рефлексия» in the memory's settings: the reflection's switches and which model thinks at night.
async fn reflection(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<ReflectionView> {
    caller.permitted(MEMORY_VIEW)?;
    Ok(ok(app.reflection.view()))
}

async fn set_reflection(
    State(app): State<Arc<App>>,
    caller: Caller,
    Body(body): Body<ReflectionChange>,
) -> ApiResult<ReflectionView> {
    caller.permitted(MEMORY_MANAGE)?;
    Ok(ok(app.reflection.change(body, caller.actor())?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::core::identity::principal::user_principal;
    use crate::core::identity::Role;
    use crate::memory::model::{Origin, Source};
    use crate::memory::Remember;
    use axum::http::StatusCode;

    #[tokio::test]
    async fn shows_memory_to_who_may_see_it_and_changes_it_only_with_the_right() {
        let app = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
        let member = || Caller::local(app.clone(), Some(user_principal("u2", "Аня", Role::Member, None)));
        let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));

        // The admin-only rights: a member sees nothing by default.
        let refused = overview(State(app.clone()), member()).await.err().unwrap();
        assert_eq!(refused.status, StatusCode::FORBIDDEN);

        app.memory
            .remember(
                Remember {
                    fact: "У Маши аллергия на орехи".into(), about: Some("Маша".into()), ..Remember::default()
                },
                Origin::station_voice(Some("kitchen"), Some("alice")),
                Source::Model,
                None,
            )
            .unwrap();
        let shown = overview(State(app.clone()), admin()).await.unwrap().1;
        assert_eq!(shown.counts.home, 1);
        assert_eq!(shown.recent.len(), 1);
        let masha = entities(State(app.clone()), admin())
            .await
            .unwrap()
            .1
            .into_iter()
            .find(|e| e.entity.name == "Маша")
            .unwrap();
        let card = entity(State(app.clone()), admin(), Path(masha.entity.id.clone())).await.unwrap().1;
        assert_eq!(card.current.len(), 1);

        // «Больше не так», then «Отменить» it.
        let fact = card.current[0].fact.id.clone();
        close_fact(State(app.clone()), admin(), Path(fact.clone()), Body(CloseBody::default())).await.unwrap();
        assert_eq!(overview(State(app.clone()), admin()).await.unwrap().1.counts.home, 0);
        let log = changes(State(app.clone()), admin(), Query(ChangesQuery { limit: Some(10) })).await.unwrap().1;
        let close = log.iter().find(|c| c.action == "close").unwrap();
        undo(State(app.clone()), admin(), Path(close.id.clone())).await.unwrap();
        assert_eq!(overview(State(app.clone()), admin()).await.unwrap().1.counts.home, 1);

        // Off by the switch: the panel's only, with the right.
        let off = set_state(State(app.clone()), admin(), Body(StateBody { enabled: Some(false), in_passing: None }))
            .await
            .unwrap();
        assert!(!off.1.enabled);
        let paused = pause(State(app.clone()), admin(), Body(PauseBody { span: Some("hour".into()), timezone: None }))
            .await
            .unwrap();
        assert!(paused.1.paused);
        let erased = erase_fact(State(app.clone()), admin(), Path(fact)).await.unwrap();
        assert!(erased.1.done);
        assert_eq!(overview(State(app.clone()), admin()).await.unwrap().1.counts.home, 0);
    }
}
