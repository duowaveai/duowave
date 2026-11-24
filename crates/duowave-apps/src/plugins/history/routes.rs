//! The history's API (docs/plans/package-history.md, «API»): the admin's, as the workshop is.
//!
//! - `GET /api/v1/history` — the room it takes, where it is kept; `PUT …/settings`, `POST …/clean`.
//! - `GET /api/v1/packages/{id}/history` — the feed; `…/history/compare`, `…/history/{record}` and its files.
//! - `GET|POST /api/v1/packages/{id}/restore` — what a step back of the installed app would do, and the step back.
//! - `POST /api/v1/workshop/drafts/{id}/restore` — the draft back to a step.
//! - `/api/v1/workshop/drafts/{id}/merge` — a merge: start, its state, the answer to the agent's question, «Отменить».
use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::routing::{get, post, put};
use axum::Router;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::gc::Place;
use super::merge::MergeReport;
use super::restore::{RestorePlan, RestoreReport};
use super::{Author, Comparison, Feed, History, What, Who};
use crate::characters::CharactersService;
use crate::dialog::DialogService;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Uses};
use crate::plugins::routes::admin;
use crate::plugins::workshop::agent::{self, ChatStep};
use crate::plugins::workshop::{DraftView, Workshop};
use crate::settings::definitions::PACKAGE_HISTORY;
use crate::settings::SettingsService;

pub fn routes<S: crate::plugins::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/history", get(room))
        .route("/api/v1/history/settings", put(set_settings))
        .route("/api/v1/history/clean", post(clean))
        .route("/api/v1/packages/{id}/history", get(feed))
        .route("/api/v1/packages/{id}/history/compare", get(compare))
        .route("/api/v1/packages/{id}/history/{record}", get(record))
        .route("/api/v1/packages/{id}/history/{record}/files/{*path}", get(file))
        .route("/api/v1/packages/{id}/restore", get(restore_plan).post(restore))
        .route("/api/v1/workshop/drafts/{id}/restore", post(restore_draft))
        .route("/api/v1/workshop/drafts/{id}/merge", get(merge_state).post(merge).delete(cancel_merge))
        .route("/api/v1/workshop/drafts/{id}/merge/answer", post(answer))
}

fn history_of(history: &Option<Arc<crate::plugins::history::History>>) -> Result<Arc<History>, ApiError> {
    history
        .clone()
        .ok_or_else(|| ApiError::conflict(crate::tr!("Истории приложений здесь нет: её папка не создаётся.")))
}

fn by_of(actor: Option<&crate::core::events::Actor>) -> Author {
    Author::of(actor, Who::Hand)
}

pub async fn room(
    Uses(history): Uses<Option<Arc<crate::plugins::history::History>>>,
    caller: Caller,
) -> ApiResult<super::gc::Room> {
    admin(&caller)?;
    Ok(ok(history_of(&history)?.room()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsBody {
    place: Option<String>,
    budget_mb: Option<u64>,
    days: Option<u64>,
    steps: Option<u64>,
}

async fn set_settings(
    Uses(history_service): Uses<Option<Arc<crate::plugins::history::History>>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    caller: Caller,
    Body(body): Body<SettingsBody>,
) -> ApiResult<super::gc::Room> {
    let actor = admin(&caller)?;
    let history = history_of(&history_service)?;
    let mut stored = settings.get(&PACKAGE_HISTORY, None).unwrap_or_else(|| json!({}));
    if let Some(place) = body.place {
        let place = Place::parse(&place).ok_or_else(|| ApiError::field("place", crate::tr!("here, home или both")))?;
        if place != Place::Here && !history.on_station() {
            return Err(ApiError::field("place", crate::tr!("компьютер хранит историю у себя")));
        }
        stored["place"] = json!(place);
    }
    for (key, value) in [("budgetMb", body.budget_mb), ("days", body.days), ("steps", body.steps)] {
        if let Some(value) = value {
            if value == 0 {
                return Err(ApiError::field(key, crate::tr!("больше нуля")));
            }
            stored[key] = json!(value);
        }
    }
    settings.set(&PACKAGE_HISTORY, &stored, actor)?;
    Ok(ok(history.room()))
}

async fn clean(
    Uses(history_service): Uses<Option<Arc<crate::plugins::history::History>>>,
    caller: Caller,
) -> ApiResult<super::gc::Cleaned> {
    admin(&caller)?;
    let history = history_of(&history_service)?;
    let cleaned = tokio::task::spawn_blocking(move || history.clean()).await.map_err(ApiError::internal)?;
    Ok(ok(cleaned))
}

#[derive(Debug, Deserialize)]
pub struct FeedQuery {
    pub before: Option<String>,
    pub limit: Option<usize>,
}

pub async fn feed(
    Uses(history_service): Uses<Option<Arc<crate::plugins::history::History>>>,
    caller: Caller,
    Path(id): Path<String>,
    Query(query): Query<FeedQuery>,
) -> ApiResult<Feed> {
    admin(&caller)?;
    let history = history_of(&history_service)?;
    // On the station that sent its history to the computer: the records it cleaned come back for the feed.
    if history.on_station() {
        let missing: Vec<String> =
            history.store().log(&id).into_iter().filter(|record| !history.store().has(record)).collect();
        if !missing.is_empty() {
            history.fetch(&missing).await;
        }
    }
    Ok(ok(history.feed(&id, query.before.as_deref(), query.limit.unwrap_or(50).clamp(1, 200))))
}

#[derive(Debug, Deserialize)]
pub struct CompareQuery {
    from: Option<String>,
    to: Option<String>,
}

async fn compare(
    Uses(history_service): Uses<Option<Arc<crate::plugins::history::History>>>,
    caller: Caller,
    Path(id): Path<String>,
    Query(query): Query<CompareQuery>,
) -> ApiResult<Comparison> {
    admin(&caller)?;
    let history = history_of(&history_service)?;
    let resolve = |record: Option<String>| -> Result<Option<String>, ApiError> {
        match record.filter(|record| !record.is_empty()) {
            Some(record) => history
                .resolve(&id, &record)
                .map(Some)
                .ok_or_else(|| ApiError::not_found(crate::tr!("Такой записи нет."))),
            None => Ok(None),
        }
    };
    let (from, to) = (resolve(query.from)?, resolve(query.to)?);
    for record in from.iter().chain(to.iter()) {
        history.ensure_local(record).await;
    }
    let comparison = history.compare(&id, from.as_deref(), to.as_deref()).map_err(ApiError::bad_request)?;
    Ok(ok(comparison))
}

async fn record(
    Uses(history_service): Uses<Option<Arc<crate::plugins::history::History>>>,
    caller: Caller,
    Path((id, record)): Path<(String, String)>,
) -> ApiResult<Value> {
    admin(&caller)?;
    let history = history_of(&history_service)?;
    let record = history.resolve(&id, &record).ok_or_else(|| ApiError::not_found(crate::tr!("Такой записи нет.")))?;
    history.ensure_local(&record).await;
    let view = history.record_view(&id, &record).ok_or_else(|| ApiError::not_found(crate::tr!("Такой записи нет.")))?;
    let data = view
        .record
        .data
        .as_deref()
        .and_then(|data| super::data::DataParts::read(history.store(), data))
        .map(|snapshot| snapshot.summary());
    Ok(ok(json!({ "record": view, "files": history.files_of(&record), "data": data })))
}

async fn file(
    Uses(history_service): Uses<Option<Arc<crate::plugins::history::History>>>,
    caller: Caller,
    Path((id, record, path)): Path<(String, String, String)>,
) -> ApiResult<Value> {
    admin(&caller)?;
    let history = history_of(&history_service)?;
    let record = history.resolve(&id, &record).ok_or_else(|| ApiError::not_found(crate::tr!("Такой записи нет.")))?;
    history.ensure_local(&record).await;
    let tree = history
        .record(&record)
        .map(|record| record.tree)
        .ok_or_else(|| ApiError::not_found(crate::tr!("Записи нет.")))?;
    let bytes = history
        .file_at(&tree, &path)
        .ok_or_else(|| ApiError::not_found(crate::tr!("Нет файла {path}.", path = path)))?;
    let content = String::from_utf8(bytes).map_err(|_| ApiError::bad_request(crate::tr!("Файл не текстовый.")))?;
    Ok(ok(json!({ "path": path, "content": content })))
}

#[derive(Debug, Deserialize)]
pub struct RestoreQuery {
    record: String,
    what: Option<String>,
}

fn what_of(text: Option<&str>) -> Result<What, ApiError> {
    match text {
        None => Ok(What::Code),
        Some(text) => What::parse(text).ok_or_else(|| ApiError::field("what", crate::tr!("code, data или code+data"))),
    }
}

async fn restore_plan(
    Uses(history_service): Uses<Option<Arc<crate::plugins::history::History>>>,
    caller: Caller,
    Path(id): Path<String>,
    Query(query): Query<RestoreQuery>,
) -> ApiResult<RestorePlan> {
    admin(&caller)?;
    let history = history_of(&history_service)?;
    let record =
        history.resolve(&id, &query.record).ok_or_else(|| ApiError::not_found(crate::tr!("Такой записи нет.")))?;
    history.ensure_local(&record).await;
    Ok(ok(history.restore_plan(&id, &record, what_of(query.what.as_deref())?)?))
}

#[derive(Debug, Deserialize)]
pub struct RestoreBody {
    pub record: String,
    pub what: Option<String>,
    #[serde(default)]
    pub approve: bool,
}

pub async fn restore(
    Uses(history_service): Uses<Option<Arc<crate::plugins::history::History>>>,
    Uses(characters): Uses<Arc<CharactersService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<RestoreBody>,
) -> ApiResult<RestoreReport> {
    let actor = admin(&caller)?;
    let history = history_of(&history_service)?;
    let record =
        history.resolve(&id, &body.record).ok_or_else(|| ApiError::not_found(crate::tr!("Такой записи нет.")))?;
    history.ensure_local(&record).await;
    let what = what_of(body.what.as_deref())?;
    let report =
        history.restore_installed(&id, &record, what, body.approve, by_of(actor.as_ref()), actor.clone()).await?;
    if report.restored {
        characters.sync();
    }
    Ok(ok(report))
}

#[derive(Debug, Deserialize)]
pub struct DraftRestoreBody {
    pub record: String,
}

pub async fn restore_draft(
    Uses(history_service): Uses<Option<Arc<crate::plugins::history::History>>>,
    Uses(workshop): Uses<Arc<Workshop>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<DraftRestoreBody>,
) -> ApiResult<DraftView> {
    let actor = admin(&caller)?;
    if let Some(history) = &history_service {
        if let Some(record) = history.resolve(&id, &body.record) {
            history.ensure_local(&record).await;
        }
    }
    Ok(ok(workshop.restore_step(&id, &body.record, by_of(actor.as_ref()))?))
}

/// The agent's work on a draft in the background, as the panel's chat starts it: the plaque and the log follow it.
pub fn start_agent(
    dialog: &Arc<DialogService>,
    workshop: &Arc<Workshop>,
    caller: &Caller,
    id: &str,
    message: String,
) -> Result<(), ApiError> {
    if !dialog.enabled() {
        return Err(ApiError::conflict(crate::tr!(
            "Модель не выбрана: добавьте провайдера в «Персонаж» → «Мышление»."
        )));
    }
    let principal = caller.principal().map(|principal| principal.id.clone()).unwrap_or_default();
    let key = format!("workshop:{principal}:{id}");
    let told: Arc<dyn Fn(ChatStep) + Send + Sync> = Arc::new(|_| {});
    let mut phrases = agent::chat(dialog, workshop.clone(), &key, id, &message, told, CancellationToken::new());
    tokio::spawn(async move { while phrases.next().await.is_some() {} });
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct MergeBody {
    /// A record; or `upstream` — the newer version of the app a copy was made from.
    record: String,
    #[serde(default)]
    agent: bool,
    /// What the owner said about it.
    asked: Option<String>,
}

async fn merge(
    Uses(history_service): Uses<Option<Arc<crate::plugins::history::History>>>,
    Uses(workshop): Uses<Arc<Workshop>>,
    Uses(dialog): Uses<Arc<DialogService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<MergeBody>,
) -> ApiResult<MergeReport> {
    let actor = admin(&caller)?;
    let history = history_of(&history_service)?;
    let record = if body.record == "upstream" {
        history
            .upstream_record(&id)
            .ok_or_else(|| ApiError::conflict(crate::tr!("Новой версии исходного приложения нет.")))?
    } else {
        history
            .resolve(&id, &body.record)
            .or_else(|| {
                // A record of another app (a copy's source): by its whole id.
                super::store::valid_id(&body.record)
                    .then(|| body.record.clone())
                    .filter(|r| history.record(r).is_some())
            })
            .ok_or_else(|| ApiError::not_found(crate::tr!("Такой записи нет.")))?
    };
    history.ensure_local(&record).await;
    let asked = body.asked.map(|asked| asked.trim().chars().take(600).collect::<String>()).filter(|a| !a.is_empty());
    let report = workshop.merge(&id, &record, body.agent, by_of(actor.as_ref()), asked).await?;
    if let Some(merge) = report.merge.as_ref() {
        start_agent(&dialog, &workshop, &caller, &id, workshop.merge_brief(merge))?;
    }
    Ok(ok(report))
}

async fn merge_state(
    Uses(history_service): Uses<Option<Arc<crate::plugins::history::History>>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    admin(&caller)?;
    let history = history_of(&history_service)?;
    Ok(ok(json!({ "merge": super::merge::state(&history, &id) })))
}

#[derive(Debug, Deserialize)]
pub struct AnswerBody {
    choice: String,
}

async fn answer(
    Uses(workshop): Uses<Arc<Workshop>>,
    Uses(dialog): Uses<Arc<DialogService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<AnswerBody>,
) -> ApiResult<Value> {
    admin(&caller)?;
    if body.choice.trim().is_empty() {
        return Err(ApiError::field("choice", crate::tr!("ответ на вопрос")));
    }
    let message = workshop.answer_merge(&id, &body.choice)?;
    start_agent(&dialog, &workshop, &caller, &id, message)?;
    Ok(ok(json!({ "answered": true })))
}

async fn cancel_merge(
    Uses(workshop): Uses<Arc<Workshop>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<DraftView> {
    admin(&caller)?;
    workshop.cancel_merge(&id)?;
    Ok(ok(workshop.draft(&id)?))
}
