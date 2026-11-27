//! The works in folders' routes (docs/plans/router-and-works.md, «API»). Everything is the admin's (`settings.edit`):
//! the journals hold the content of files.
//!
//! - `GET /api/v1/work-folders` → `{folders}`: each with its current branch and whether git is there. `POST` — a new
//!   folder `{name?, path, agent?, model?, maxUsd?, prepare?, checks?, allow?}` (a folder with git, none of the
//!   gateway's); `PUT /api/v1/work-folders/{id}` — the same without the path; `DELETE` — the folder goes (its works'
//!   branches stay in it).
//! - `POST /api/v1/work-folders/{id}/start {task, agent?, model?}` → `{said, work}`: «Новая работа» — a work of the
//!   kind `folder`, at once or in the queue.
//! - `GET /api/v1/harnesses` → `{harnesses}`: the agents — Claude Code built in — found, version, signed in, how it
//!   pays and why not; a card's agent with its card (what to install, how to sign in, its terms, whether it asks before
//!   commands). `PUT /api/v1/harnesses/claude-code {path?, auth?, key?, keyFrom?}` — where its program is, how it pays
//!   (`terminal`, `key`), the gateway's key for «по ключу API» (or a provider's of «Мозг», `keyFrom`);
//!   `PUT /api/v1/harnesses/{id} {path}` — where a card's agent's program is (empty — by its name).
//! - `GET /api/v1/works/{id}/runs` → `{work, runs}`: a work in a folder as the helper leads it — folder, agent, branch,
//!   copy, session and the command that opens it in a terminal — and its runs. `GET …/runs/{n}/log?after=&limit=&
//!   trouble=1&find=` → `{steps, next, going}`: the run's common steps from the place `after`, read on while it goes;
//!   `GET …/runs/{n}/raw` — the raw file of the run, as the agent printed it.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use serde::{Deserialize, Serialize};

use super::folders::{self, Asked, Guarded};
use super::harness::claude_code::{Auth, CLAUDE_CODE, KEY_SECRET};
use super::harness::Ready;
use super::runs;
use super::{FolderWorks, Record};
use crate::core::identity::permissions::SETTINGS_EDIT;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Uses};
use crate::secrets::SecretsService;
use crate::settings::definitions::{WorkFolder, HARNESS_CLAUDE_AUTH, HARNESS_CLAUDE_PATH, HARNESS_PATHS};
use crate::settings::SettingsService;
use crate::talk::folder::{FolderAsk, FOLDER_EXECUTOR};
use crate::talk::{Activator, Origin, TalkLayer};

pub fn routes<S: crate::works::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/work-folders", get(list_folders).post(add_folder))
        .route("/api/v1/work-folders/{id}", put(update_folder).delete(remove_folder))
        .route("/api/v1/work-folders/{id}/start", post(start))
        .route("/api/v1/harnesses", get(harnesses))
        .route("/api/v1/harnesses/claude-code", put(set_claude_code))
        .route("/api/v1/harnesses/{id}", put(set_agent_path))
        .route("/api/v1/works/{id}/runs", get(runs_of))
        .route("/api/v1/works/{id}/runs/{n}/log", get(log))
        .route("/api/v1/works/{id}/runs/{n}/raw", get(raw))
        .route(super::plugs::plugin::HOOKS_PATH, post(hooks))
        .route("/api/v1/work-systems/claude-code/plugin", get(plugin))
}

/// Two keys alike, compared in a time that does not tell how much of them matched.
fn same(given: &str, expected: &str) -> bool {
    given.len() == expected.len()
        && given.bytes().zip(expected.bytes()).fold(0u8, |differ, (a, b)| differ | (a ^ b)) == 0
}

/// Claude Code's hooks post here (works/plugs/plugin.rs, docs/plans/work-systems.md, stage 3): a session's event,
/// and the answer is the hook's decision. The system's key stands for a sign-in; a wrong one is refused.
async fn hooks(
    Uses(claude): Uses<Option<Arc<super::plugs::claude::ClaudeByHand>>>,
    Uses(secrets): Uses<Arc<SecretsService>>,
    headers: axum::http::HeaderMap,
    Body(input): Body<serde_json::Value>,
) -> Response {
    let Some(claude) = claude else { return axum::http::StatusCode::NOT_FOUND.into_response() };
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    let expected = secrets.get(super::plugs::plugin::KEY_SECRET).unwrap_or_default();
    if expected.is_empty() || !same(given, &expected) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    axum::Json(claude.hook(&input, super::plugs::claude::ASK_WAIT).await).into_response()
}

/// DuoWave's plugin for Claude Code, written now with this system's key and this gateway's port, and the commands
/// that install and remove it (the person runs them; DuoWave edits nothing of Claude Code's).
async fn plugin(
    Uses(works_service): Uses<Option<Arc<crate::works::FolderWorks>>>,
    Uses(secrets): Uses<Arc<SecretsService>>,
    Uses(config): Uses<Arc<crate::config::Config>>,
    caller: Caller,
) -> ApiResult<super::plugs::plugin::PluginView> {
    caller.permitted(SETTINGS_EDIT)?;
    let works = helper(&works_service)?;
    let key = match secrets.get(super::plugs::plugin::KEY_SECRET).filter(|key| !key.is_empty()) {
        Some(key) => key,
        None => {
            use base64::Engine;
            let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(crate::utils::ids::random_bytes(32));
            secrets.set(super::plugs::plugin::KEY_SECRET, Some(&key))?;
            key
        }
    };
    let dir = super::plugs::plugin::dir_of(&works.dir);
    let view = super::plugs::plugin::write(&dir, config.app.port, &key).map_err(ApiError::internal)?;
    Ok(ok(view))
}

/// A folder as the panel shows it: the setting, its branch now and whether git answers there.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FolderView {
    #[serde(flatten)]
    folder: WorkFolder,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    git: bool,
}

async fn view(folder: WorkFolder) -> FolderView {
    let path = std::path::PathBuf::from(&folder.path);
    let git = super::git::toplevel(&path).await.is_some();
    let branch = if git { super::git::current_branch(&path).await } else { None };
    FolderView { folder, branch, git }
}

#[derive(Debug, Serialize)]
struct Folders {
    folders: Vec<FolderView>,
}

fn helper(works: &Option<Arc<crate::works::FolderWorks>>) -> Result<Arc<FolderWorks>, ApiError> {
    works.clone().ok_or_else(|| ApiError::unavailable(crate::tr!("Работа в папках здесь недоступна.")))
}

async fn list_folders(Uses(settings): Uses<Arc<SettingsService>>, caller: Caller) -> ApiResult<Folders> {
    caller.permitted(SETTINGS_EDIT)?;
    let mut views = Vec::new();
    for folder in folders::list(&settings) {
        views.push(view(folder).await);
    }
    Ok(ok(Folders { folders: views }))
}

async fn add_folder(
    Uses(works_service): Uses<Option<Arc<crate::works::FolderWorks>>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    caller: Caller,
    Body(asked): Body<Asked>,
) -> ApiResult<FolderView> {
    caller.permitted(SETTINGS_EDIT)?;
    let works = helper(&works_service)?;
    let guarded = Guarded { data: works.guarded.clone(), home: super::home() };
    check_agent(&works, asked.agent.as_deref())?;
    let folder = folders::add(&settings, &guarded, asked, caller.actor()).await?;
    Ok(ok(view(folder).await))
}

async fn update_folder(
    Uses(works_service): Uses<Option<Arc<crate::works::FolderWorks>>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(asked): Body<Asked>,
) -> ApiResult<FolderView> {
    caller.permitted(SETTINGS_EDIT)?;
    let works = helper(&works_service)?;
    check_agent(&works, asked.agent.as_deref())?;
    let folder = folders::update(&settings, &id, asked, caller.actor())?;
    Ok(ok(view(folder).await))
}

/// An agent named for a folder must be one there is.
fn check_agent(works: &FolderWorks, agent: Option<&str>) -> Result<(), ApiError> {
    match agent.map(str::trim).filter(|agent| !agent.is_empty()) {
        Some(agent) if works.harness(agent).is_none() => {
            Err(ApiError::field("agent", crate::tr!("Такого агента нет: поставьте его карточку в MagicPlace.")))
        }
        _ => Ok(()),
    }
}

#[derive(Debug, Serialize)]
struct Done {
    ok: bool,
}

async fn remove_folder(
    Uses(settings): Uses<Arc<SettingsService>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Done> {
    caller.permitted(SETTINGS_EDIT)?;
    folders::remove(&settings, &id, caller.actor())?;
    Ok(ok(Done { ok: true }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartAsked {
    task: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    model: Option<String>,
    /// The panel's switch of the set (stage 5 of docs/plans/work-systems.md).
    #[serde(default)]
    set: Option<String>,
    /// How our system does it (stage 6): `review`, `plan`; none — at once.
    #[serde(default)]
    way: Option<String>,
}

#[derive(Debug, Serialize)]
struct Begun {
    said: String,
    work: crate::talk::works::Work,
}

async fn start(
    Uses(works_service): Uses<Option<Arc<crate::works::FolderWorks>>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    Uses(talk): Uses<Arc<TalkLayer>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(asked): Body<StartAsked>,
) -> ApiResult<Begun> {
    caller.permitted(SETTINGS_EDIT)?;
    let works = helper(&works_service)?;
    folders::get(&settings, &id).ok_or_else(|| ApiError::not_found(crate::tr!("Такой рабочей папки нет.")))?;
    if asked.task.trim().is_empty() {
        return Err(ApiError::field("task", crate::tr!("Напишите, что сделать.")));
    }
    check_agent(&works, asked.agent.as_deref())?;
    // Typed in the panel: no limit of the works by voice holds it.
    let origin = Origin::computer(Activator::Text).by(caller.principal().map(|principal| principal.name.clone()));
    let lang = talk.lang(crate::talk::COMPUTER);
    let set = asked.set.map(|set| set.trim().to_string()).filter(|set| !set.is_empty());
    if let Some(set) = &set {
        let sets = settings.get(&crate::router::sets::WORK_SETS, None).unwrap_or_default();
        crate::router::sets::find(&sets, set)
            .ok_or_else(|| ApiError::field("set", crate::tr!("Такого набора нет.")))?;
    }
    let way = super::ways::parse(asked.way.as_deref()).map(str::to_string);
    let ask = FolderAsk { folder: id, task: asked.task, agent: asked.agent, model: asked.model, set, way };
    let (said, work) = talk
        .start_folder(origin, ask)
        .map_err(|refusal| ApiError::conflict(refusal.for_person(crate::talk::pack::of(lang))))?;
    Ok(ok(Begun { said, work }))
}

/// An agent as the panel shows it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HarnessView {
    id: String,
    title: String,
    builtin: bool,
    ready: Ready,
    /// Claude Code: how it pays as chosen (none — not chosen yet), whether the gateway has a key, the program set.
    #[serde(skip_serializing_if = "Option::is_none")]
    auth: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key_set: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path_set: Option<String>,
    /// A card's agent: its card, and its description.
    #[serde(skip_serializing_if = "Option::is_none")]
    card: Option<crate::plugins::manifest::AgentCard>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Debug, Serialize)]
struct Harnesses {
    harnesses: Vec<HarnessView>,
}

async fn claude_view(
    secrets: &Arc<SecretsService>,
    settings: &Arc<SettingsService>,
    works: &FolderWorks,
) -> HarnessView {
    use super::harness::Harness;
    HarnessView {
        id: CLAUDE_CODE.into(),
        title: works.claude.title(),
        builtin: true,
        ready: works.claude.ready().await,
        auth: works.claude.auth_chosen().map(Auth::as_str),
        key_set: Some(secrets.has(KEY_SECRET)),
        path_set: settings.get(&HARNESS_CLAUDE_PATH, None).filter(|path| !path.is_empty()),
        card: None,
        description: None,
    }
}

async fn agent_view(settings: &Arc<SettingsService>, works: &FolderWorks, card: super::agents::Card) -> HarnessView {
    use super::harness::Harness;
    let paths = settings.get(&HARNESS_PATHS, None).unwrap_or_default();
    let ready = match works.agents.get(&card.id) {
        Some(agent) => agent.ready().await,
        None => Ready::not(super::harness::NotReady::Gone),
    };
    HarnessView {
        path_set: paths.get(&card.id).cloned(),
        id: card.id,
        title: card.title,
        builtin: false,
        ready,
        auth: None,
        key_set: None,
        card: Some(card.card),
        description: card.description,
    }
}

async fn harnesses(
    Uses(works_service): Uses<Option<Arc<crate::works::FolderWorks>>>,
    Uses(secrets): Uses<Arc<SecretsService>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    caller: Caller,
) -> ApiResult<Harnesses> {
    caller.permitted(SETTINGS_EDIT)?;
    let works = helper(&works_service)?;
    let mut views = vec![claude_view(&secrets, &settings, &works).await];
    for card in works.agents.cards() {
        views.push(agent_view(&settings, &works, card).await);
    }
    Ok(ok(Harnesses { harnesses: views }))
}

#[derive(Debug, Default, Deserialize)]
struct PathAsked {
    #[serde(default)]
    path: Option<String>,
}

/// Where a card's agent's program is: a path the panel sets, or by its name on the search path.
async fn set_agent_path(
    Uses(works_service): Uses<Option<Arc<crate::works::FolderWorks>>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(asked): Body<PathAsked>,
) -> ApiResult<HarnessView> {
    caller.permitted(SETTINGS_EDIT)?;
    let works = helper(&works_service)?;
    let card =
        works.agents.cards().into_iter().find(|card| card.id == id).ok_or_else(|| {
            ApiError::not_found(crate::tr!("Такого агента нет: поставьте его карточку в MagicPlace."))
        })?;
    let mut paths = settings.get(&HARNESS_PATHS, None).unwrap_or_default();
    match asked.path.as_deref().map(str::trim).filter(|path| !path.is_empty()) {
        Some(path) if super::harness::find_program(path).is_none() => {
            return Err(ApiError::field("path", crate::tr!("Здесь нет программы: укажите полный путь к ней.")));
        }
        Some(path) => {
            paths.insert(id.clone(), path.to_string());
        }
        None => {
            paths.remove(&id);
        }
    }
    settings.set(&HARNESS_PATHS, &paths, caller.actor())?;
    works.agents.forget(&id);
    Ok(ok(agent_view(&settings, &works, card).await))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeAsked {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    auth: Option<String>,
    #[serde(default)]
    key: Option<String>,
    /// A provider of «Мозг» whose key to take.
    #[serde(default)]
    key_from: Option<String>,
}

async fn set_claude_code(
    Uses(works_service): Uses<Option<Arc<crate::works::FolderWorks>>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    Uses(secrets): Uses<Arc<SecretsService>>,
    caller: Caller,
    Body(asked): Body<ClaudeAsked>,
) -> ApiResult<HarnessView> {
    caller.permitted(SETTINGS_EDIT)?;
    let works = helper(&works_service)?;
    if let Some(path) = asked.path {
        let path = path.trim().to_string();
        if path.is_empty() {
            settings.forget(&HARNESS_CLAUDE_PATH)?;
        } else if super::harness::find_program(&path).is_none() {
            return Err(ApiError::field("path", crate::tr!("Здесь нет программы: укажите полный путь к claude.")));
        } else {
            settings.set(&HARNESS_CLAUDE_PATH, &path, caller.actor())?;
        }
    }
    if let Some(auth) = asked.auth {
        let auth = Auth::parse(&auth).ok_or_else(|| ApiError::field("auth", crate::tr!("вход — terminal или key")))?;
        settings.set(&HARNESS_CLAUDE_AUTH, &auth.as_str().to_string(), caller.actor())?;
    }
    if let Some(provider) = asked.key_from {
        let key = secrets
            .get(&format!("llm:{}:key", provider.trim()))
            .ok_or_else(|| ApiError::field("keyFrom", crate::tr!("У этого провайдера нет ключа.")))?;
        secrets.set(KEY_SECRET, Some(&key))?;
    }
    if let Some(key) = asked.key {
        let key = key.trim();
        secrets.set(KEY_SECRET, (!key.is_empty()).then_some(key))?;
    }
    Ok(ok(claude_view(&secrets, &settings, &works).await))
}

/// A work in a folder for the panel: the record, the agent's name and the command that opens its session.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkView {
    #[serde(flatten)]
    record: Record,
    agent_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    resume_command: Option<String>,
    going: bool,
}

#[derive(Debug, Serialize)]
struct Runs {
    work: WorkView,
    runs: Vec<runs::Run>,
}

/// The helper's work of a talk's work, by the talk's id.
fn record_of(talk: &Arc<TalkLayer>, works: &FolderWorks, id: &str) -> Result<Record, ApiError> {
    let work = talk.works().get(id).ok_or_else(|| ApiError::not_found(crate::tr!("Такой работы нет.")))?;
    let target = work
        .target
        .filter(|_| work.executor == FOLDER_EXECUTOR)
        .ok_or_else(|| ApiError::not_found(crate::tr!("Это не работа в папке.")))?;
    works.record(&target).ok_or_else(|| ApiError::not_found(crate::tr!("Такой работы нет.")))
}

async fn runs_of(
    Uses(works_service): Uses<Option<Arc<crate::works::FolderWorks>>>,
    Uses(talk): Uses<Arc<TalkLayer>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Runs> {
    caller.permitted(SETTINGS_EDIT)?;
    let works = helper(&works_service)?;
    let record = record_of(&talk, &works, &id)?;
    let harness = works.harness(&record.agent);
    let agent_title = harness.as_ref().map(|harness| harness.title()).unwrap_or_else(|| record.agent.clone());
    let resume_command = match (&harness, &record.session) {
        (Some(harness), Some(session)) => harness.resume_command(session, &works.copy_of(&record.id)),
        _ => None,
    };
    let runs = runs::list(&works.db, &record.id)?;
    let going = works.going_of(&record.id).is_some();
    Ok(ok(Runs { work: WorkView { record, agent_title, resume_command, going }, runs }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Log {
    #[serde(flatten)]
    read: runs::Read,
    /// The run goes on: read on from `next`.
    going: bool,
}

async fn log(
    Uses(works_service): Uses<Option<Arc<crate::works::FolderWorks>>>,
    Uses(talk): Uses<Arc<TalkLayer>>,
    caller: Caller,
    Path((id, n)): Path<(String, u32)>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Log> {
    caller.permitted(SETTINGS_EDIT)?;
    let works = helper(&works_service)?;
    let record = record_of(&talk, &works, &id)?;
    let after = query.get("after").and_then(|after| after.parse().ok()).unwrap_or(0);
    let limit = query.get("limit").and_then(|limit| limit.parse().ok()).unwrap_or(runs::READ_MAX);
    let trouble = query.get("trouble").is_some_and(|trouble| trouble == "1" || trouble == "true");
    let find = query.get("find").map(String::as_str);
    let run = runs::list(&works.db, &record.id)?.into_iter().find(|run| run.n == n);
    let run = run.ok_or_else(|| ApiError::not_found(crate::tr!("Такого запуска нет.")))?;
    let read = runs::read(&works.dir, &record.id, n, after, limit, trouble, find);
    Ok(ok(Log { read, going: run.ended_at.is_none() }))
}

async fn raw(
    Uses(works_service): Uses<Option<Arc<crate::works::FolderWorks>>>,
    Uses(talk): Uses<Arc<TalkLayer>>,
    caller: Caller,
    Path((id, n)): Path<(String, u32)>,
) -> Result<Response, ApiError> {
    caller.permitted(SETTINGS_EDIT)?;
    let works = helper(&works_service)?;
    let record = record_of(&talk, &works, &id)?;
    let path = runs::raw_path(&works.dir, &record.id, n);
    let bytes = tokio::fs::read(&path).await.map_err(|_| ApiError::not_found(crate::tr!("Такого запуска нет.")))?;
    let name = format!("attachment; filename=\"duowave-{}-run-{n}.jsonl\"", record.id);
    Ok(([(header::CONTENT_TYPE, "application/x-ndjson".to_string()), (header::CONTENT_DISPOSITION, name)], bytes)
        .into_response())
}
