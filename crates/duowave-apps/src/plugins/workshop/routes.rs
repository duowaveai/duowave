//! The workshop over HTTP: `/api/v1/workshop/…` for the panel's «Мастерская», and `/mcp/workshop` — the same tools as
//! an MCP server (streamable HTTP, JSON answers) for Claude Code. All of it is the admin's at home: a draft is code that
//! runs in the gateway once installed. The agent's chat streams as server-sent events, like «Спросить мозг».
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{DefaultBodyLimit, Path};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::CancellationToken;

use super::activity::{Job, Step};
use super::agent::{self, ChatStep};
use super::{CheckReport, DraftView, InstallPlan, InstallReport, PhraseReport, RunReport, Trying};
use crate::characters::CharactersService;
use crate::dialog::brain::BrainService;
use crate::dialog::DialogService;
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Uses};
use crate::plugins::history::{Author, Who};
use crate::plugins::routes::admin;
use crate::plugins::workshop::Workshop;
use crate::secrets::SecretsService;
use crate::settings::definitions::{BrainPick, WORKSHOP_AGENT};
use crate::settings::SettingsService;
use crate::utils::time::now_ms;

/// A file of a draft is up to 1 MB, sent as JSON.
const BODY_LIMIT: usize = 2 * 1024 * 1024;
/// MCP's protocol version this server speaks first.
const MCP_PROTOCOL: &str = "2025-06-18";

pub fn routes<S: crate::plugins::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/workshop", get(list))
        .route("/api/v1/workshop/drafts", post(create))
        .route("/api/v1/workshop/drafts/{id}", get(one).delete(remove))
        .route("/api/v1/workshop/drafts/{id}/open", post(open))
        .route("/api/v1/workshop/drafts/{id}/files/{*path}", get(read).put(write).delete(delete_file))
        .route("/api/v1/workshop/drafts/{id}/check", post(check))
        .route("/api/v1/workshop/drafts/{id}/run", post(run))
        .route("/api/v1/workshop/drafts/{id}/phrase", post(phrase))
        .route("/api/v1/workshop/drafts/{id}/journal", get(journal))
        .route("/api/v1/workshop/drafts/{id}/install", get(plan).post(install))
        .route("/api/v1/workshop/drafts/{id}/trial", post(trial).delete(end_trial))
        .route("/api/v1/workshop/agent", get(agent_model).put(set_agent_model))
        .route("/api/v1/workshop/drafts/{id}/chat", post(chat))
        .route("/api/v1/workshop/activity", get(activity))
        .route("/api/v1/workshop/drafts/{id}/activity", get(activity_log))
        .route("/api/v1/workshop/drafts/{id}/stop", post(stop))
        .route("/api/v1/workshop/drafts/{id}/park", post(park))
        .route("/api/v1/workshop/make", post(make))
        .route("/api/v1/workshop/drafts/{id}/preview", post(preview))
        .route("/panel-preview/{token}/{file}", get(preview_file))
        .route("/mcp/workshop", post(mcp).get(mcp_get))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

#[derive(Debug, Serialize)]
pub struct WorkshopView {
    dir: String,
    drafts: Vec<DraftView>,
}

pub async fn list(Uses(workshop): Uses<Arc<Workshop>>, caller: Caller) -> ApiResult<WorkshopView> {
    admin(&caller)?;
    Ok(ok(WorkshopView { dir: workshop.dir().display().to_string(), drafts: workshop.drafts() }))
}

#[derive(Debug, Default, Deserialize)]
pub struct CreateBody {
    /// None: the workshop gives a random one.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// An OpenAPI document in JSON: a tool per operation.
    #[serde(default)]
    pub openapi: Option<String>,
}

pub async fn create(
    Uses(workshop): Uses<Arc<Workshop>>,
    caller: Caller,
    Body(body): Body<CreateBody>,
) -> ApiResult<DraftView> {
    admin(&caller)?;
    let kind = body.kind.as_deref().or(Some("plugin")).filter(|_| body.openapi.is_none());
    let id = workshop.new_draft_id(body.id.as_deref())?;
    Ok(created(workshop.create(&id, kind, body.title.as_deref(), body.openapi.as_deref())?))
}

/// «Открыть в мастерской»: the draft, or one from the installed package of one's own.
async fn open(Uses(workshop): Uses<Arc<Workshop>>, caller: Caller, Path(id): Path<String>) -> ApiResult<DraftView> {
    admin(&caller)?;
    Ok(ok(workshop.open(&id)?))
}

pub async fn one(Uses(workshop): Uses<Arc<Workshop>>, caller: Caller, Path(id): Path<String>) -> ApiResult<DraftView> {
    admin(&caller)?;
    Ok(ok(workshop.draft(&id)?))
}

async fn remove(Uses(workshop): Uses<Arc<Workshop>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    admin(&caller)?;
    workshop.delete(&id)?;
    Ok(ok(json!({ "deleted": true })))
}

/// The draft's panel part to look at before installing (docs/plans/desk.md, stage 5): an address with a token.
async fn preview(Uses(workshop): Uses<Arc<Workshop>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    admin(&caller)?;
    let (module, styles) = workshop.preview(&id)?;
    Ok(ok(json!({ "module": module, "styles": styles })))
}

/// The previewed file: JavaScript or CSS, only from this gateway's own panel (the panel's content policy).
pub async fn preview_file(
    Uses(workshop): Uses<Arc<Workshop>>,
    Path((token, file)): Path<(String, String)>,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;
    let text = workshop.preview_file(&token, &file).ok_or_else(|| ApiError::not_found("Not Found"))?;
    let kind = if file.ends_with(".css") { "text/css; charset=utf-8" } else { "text/javascript; charset=utf-8" };
    use axum::http::header;
    // An app's window is a sandboxed frame of no origin (docs/plans/bus.md, «Окошки»): it imports the draft's part
    // across origins, like an installed app's (plugins/routes.rs, `panel_file`). The address carries a short-lived
    // random token; the file is the draft's panel code, no secret in it.
    Ok((
        [
            (header::CONTENT_TYPE, kind),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        ],
        text,
    )
        .into_response())
}

async fn read(
    Uses(workshop): Uses<Arc<Workshop>>,
    caller: Caller,
    Path((id, path)): Path<(String, String)>,
) -> ApiResult<Value> {
    admin(&caller)?;
    let content = workshop.read(&id, &path)?;
    Ok(ok(json!({ "path": path, "content": content })))
}

#[derive(Debug, Deserialize)]
pub struct FileBody {
    content: String,
}

async fn write(
    Uses(workshop): Uses<Arc<Workshop>>,
    caller: Caller,
    Path((id, path)): Path<(String, String)>,
    Body(body): Body<FileBody>,
) -> ApiResult<DraftView> {
    let actor = admin(&caller)?;
    let view = workshop.write(&id, &path, &body.content)?;
    workshop.hand_edited(&id, Author::of(actor.as_ref(), Who::Hand));
    Ok(ok(view))
}

async fn delete_file(
    Uses(workshop): Uses<Arc<Workshop>>,
    caller: Caller,
    Path((id, path)): Path<(String, String)>,
) -> ApiResult<DraftView> {
    let actor = admin(&caller)?;
    let view = workshop.delete_file(&id, &path)?;
    workshop.hand_edited(&id, Author::of(actor.as_ref(), Who::Hand));
    Ok(ok(view))
}

pub async fn check(
    Uses(workshop): Uses<Arc<Workshop>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<CheckReport> {
    admin(&caller)?;
    Ok(ok(workshop.check(&id).await?))
}

#[derive(Debug, Deserialize)]
pub struct RunBody {
    pub function: String,
    #[serde(default)]
    pub input: Option<Value>,
    /// On the draft's example (`examples.json`), never the network: the desk's tile and «Примерка».
    #[serde(default)]
    pub example: bool,
    /// Plain settings over the defaults; a private one is refused.
    #[serde(default)]
    pub settings: Map<String, Value>,
}

pub async fn run(
    Uses(workshop): Uses<Arc<Workshop>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<RunBody>,
) -> ApiResult<RunReport> {
    admin(&caller)?;
    let trying = Trying { example: body.example, settings: workshop.plain_settings(&id, &body.settings)? };
    Ok(ok(workshop.run(&id, &body.function, body.input.unwrap_or_else(|| json!({})), &trying).await?))
}

#[derive(Debug, Deserialize)]
pub struct PhraseBody {
    pub text: String,
    #[serde(default)]
    pub example: bool,
    #[serde(default)]
    pub settings: Map<String, Value>,
}

pub async fn phrase(
    Uses(workshop): Uses<Arc<Workshop>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<PhraseBody>,
) -> ApiResult<PhraseReport> {
    admin(&caller)?;
    let trying = Trying { example: body.example, settings: workshop.plain_settings(&id, &body.settings)? };
    Ok(ok(workshop.try_phrase(&id, &body.text, &trying).await?))
}

async fn journal(Uses(workshop): Uses<Arc<Workshop>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    admin(&caller)?;
    Ok(ok(json!({ "lines": workshop.journal(&id) })))
}

/// A step as the panel gets it: what a key or a token may have put into its text is hidden.
fn shown(secrets: &SecretsService, mut step: Step) -> Step {
    step.text = secrets.redact(&step.text);
    step
}

/// A draft's work with its title, for the plaque «Делаю виджет «…»».
fn job_view(secrets: &SecretsService, workshop: &Workshop, mut job: Job) -> Value {
    job.asked = secrets.redact(&job.asked);
    job.last = job.last.map(|step| shown(secrets, step));
    let title = workshop.draft(&job.draft).ok().and_then(|draft| draft.title);
    let mut view = serde_json::to_value(&job).unwrap_or_default();
    view["title"] = json!(title);
    view
}

/// What the agent does now (docs/plans/desk.md, «Делаю виджет»): the drafts it works on, whoever started it, and the
/// ones it finished in the last minute and a half.
/// `now` is this gateway's clock (ms since the epoch): the helper's panel reads a station's work through its pass and
/// counts how long a step goes on by the station's own clock, not the computer's.
pub async fn activity(
    Uses(workshop): Uses<Arc<Workshop>>,
    Uses(secrets): Uses<Arc<SecretsService>>,
    caller: Caller,
) -> ApiResult<Value> {
    admin(&caller)?;
    let jobs: Vec<Value> =
        workshop.activity().jobs().into_iter().map(|job| job_view(&secrets, &workshop, job)).collect();
    Ok(ok(json!({ "jobs": jobs, "now": now_ms() })))
}

#[derive(Debug, Default, Deserialize)]
pub struct AfterQuery {
    /// Only the steps after this one (`seq`); none — all that is kept.
    #[serde(default)]
    pub after: u64,
}

/// A draft's work and its steps: what was asked, the calls and their results (each naming its call), what the agent
/// said, how it ended; `now` as in `activity`.
pub async fn activity_log(
    Uses(workshop): Uses<Arc<Workshop>>,
    Uses(secrets): Uses<Arc<SecretsService>>,
    caller: Caller,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<AfterQuery>,
) -> ApiResult<Value> {
    admin(&caller)?;
    let Some((job, steps)) = workshop.activity().log(&id, query.after) else {
        return Ok(ok(json!({ "job": null, "steps": [], "now": now_ms() })));
    };
    let steps: Vec<Step> = steps.into_iter().map(|step| shown(&secrets, step)).collect();
    Ok(ok(json!({ "job": job_view(&secrets, &workshop, job), "steps": steps, "now": now_ms() })))
}

/// «Остановить»: the agent's work on the draft ends now, by voice or from the panel alike.
pub async fn stop(Uses(workshop): Uses<Arc<Workshop>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    admin(&caller)?;
    Ok(ok(json!({ "stopped": workshop.stop(&id) })))
}

#[derive(Debug, Default, Deserialize)]
pub struct MakeBody {
    /// «Сделай виджет: мои дела на сегодня»; with `draft` — new work on it («переделай: крупнее»).
    pub words: String,
    #[serde(default)]
    pub draft: Option<String>,
}

/// «Сделай…» from the panel (docs/plans/voice-creation.md): a draft of the request's face, its card, the agent in the
/// background — the same road as the station's. Words that are not a request: 422.
pub async fn make(
    Uses(workshop): Uses<Arc<Workshop>>,
    caller: Caller,
    Body(body): Body<MakeBody>,
) -> ApiResult<super::Made> {
    let actor = admin(&caller)?;
    let who = actor.map(|actor| actor.name);
    Ok(created(workshop.make(&body.words, body.draft.as_deref(), super::activity::By::Panel, who)?))
}

/// «В черновики»: the draft waits in the workshop, off the desk (docs/plans/voice-creation.md).
pub async fn park(Uses(workshop): Uses<Arc<Workshop>>, caller: Caller, Path(id): Path<String>) -> ApiResult<DraftView> {
    admin(&caller)?;
    Ok(ok(workshop.park(&id)?))
}

#[derive(Debug, Default, Deserialize)]
pub struct InstallBody {
    /// The admin's «ок» to the first install or new rights, given in the panel with them on the screen.
    #[serde(default)]
    pub approve: bool,
    /// The values given in «Примерка», plain and private: written before the plugin first starts.
    #[serde(default)]
    pub settings: Map<String, Value>,
}

pub async fn install(
    Uses(workshop): Uses<Arc<Workshop>>,
    Uses(characters): Uses<Arc<CharactersService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<InstallBody>,
) -> ApiResult<InstallReport> {
    let actor = admin(&caller)?;
    let report = workshop.install(&id, body.approve, &body.settings, actor).await?;
    if report.installed {
        characters.sync();
    }
    Ok(ok(report))
}

/// What «Поставить» would do: shown before the admin says «да», whether or not it asks for new rights.
pub async fn plan(
    Uses(workshop): Uses<Arc<Workshop>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<InstallPlan> {
    admin(&caller)?;
    Ok(ok(workshop.plan(&id).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct TrialBody {
    #[serde(default)]
    pub approve: bool,
    /// How long, minutes: 10 unless given, at most an hour.
    #[serde(default)]
    pub minutes: Option<u32>,
}

/// «Проба на колонке»: the draft runs on the gateway for a while, then the version before comes back by itself.
pub async fn trial(
    Uses(workshop): Uses<Arc<Workshop>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<TrialBody>,
) -> ApiResult<InstallReport> {
    let actor = admin(&caller)?;
    Ok(ok(workshop.start_trial(&id, body.minutes.unwrap_or(10), body.approve, actor).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct EndTrialQuery {
    /// Keep the trial's version as an install instead of taking it back.
    #[serde(default)]
    pub keep: bool,
}

pub async fn end_trial(
    Uses(workshop): Uses<Arc<Workshop>>,
    caller: Caller,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<EndTrialQuery>,
) -> ApiResult<Value> {
    let actor = admin(&caller)?;
    let message = workshop.end_trial(&id, query.keep, actor).await?;
    Ok(ok(json!({ "message": message, "draft": workshop.draft(&id).ok() })))
}

/// The agent's model: its own choice (null — the smart profile of «Мозг») and the model that answers now.
fn agent_view(settings: &SettingsService, workshop: &Workshop, dialog: &DialogService) -> Value {
    let choice = settings.get(&WORKSHOP_AGENT, None);
    let context = agent::context(workshop, "", choice.clone());
    json!({ "choice": choice, "answers": dialog.profile_for(&context), "smart": dialog.brain().profiles.smart })
}

async fn agent_model(
    Uses(settings): Uses<Arc<SettingsService>>,
    Uses(workshop): Uses<Arc<Workshop>>,
    Uses(dialog): Uses<Arc<DialogService>>,
    caller: Caller,
) -> ApiResult<Value> {
    admin(&caller)?;
    Ok(ok(agent_view(&settings, &workshop, &dialog)))
}

#[derive(Debug, Default, Deserialize)]
pub struct AgentBody {
    /// A provider of «Мозг» and its model; null — back to the smart profile.
    #[serde(default)]
    pub choice: Option<BrainPick>,
}

/// The agent's own model; «Мозг» itself does not change.
pub async fn set_agent_model(
    Uses(settings): Uses<Arc<SettingsService>>,
    Uses(brain): Uses<Arc<BrainService>>,
    Uses(workshop): Uses<Arc<Workshop>>,
    Uses(dialog): Uses<Arc<DialogService>>,
    caller: Caller,
    Body(body): Body<AgentBody>,
) -> ApiResult<Value> {
    let actor = admin(&caller)?;
    match body.choice {
        None => settings.forget(&WORKSHOP_AGENT)?,
        Some(pick) => {
            let pick = BrainPick { provider: pick.provider.trim().to_string(), model: pick.model.trim().to_string() };
            if pick.model.is_empty() {
                return Err(ApiError::field("model", crate::tr!("выберите модель")));
            }
            if !brain.view().providers.iter().any(|provider| provider.id == pick.provider) {
                return Err(ApiError::field("provider", crate::tr!("такого провайдера нет в «Мышлении»")));
            }
            settings.set(&WORKSHOP_AGENT, &pick, actor)?;
        }
    }
    Ok(ok(agent_view(&settings, &workshop, &dialog)))
}

/// One event of the agent's chat.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum WorkshopEvent {
    Start { model: String, provider: String },
    Text { text: String, ms: u64 },
    Tool { name: String, input: Map<String, Value> },
    Result { name: String, content: String, is_error: bool, ms: u64 },
    Done { ms: u64 },
    Error { message: String },
}

#[derive(Debug, Default, Deserialize)]
pub struct ChatBody {
    message: String,
    /// Start the conversation over.
    #[serde(default)]
    fresh: bool,
}

/// The agent's answer as server-sent events: its words, every tool it calls and what came back.
async fn chat(
    Uses(dialog): Uses<Arc<DialogService>>,
    Uses(workshop): Uses<Arc<Workshop>>,
    Uses(secrets_service): Uses<Arc<SecretsService>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<ChatBody>,
) -> Result<Response, ApiError> {
    admin(&caller)?;
    let principal = caller.principal().map(|principal| principal.id.clone()).unwrap_or_default();
    let message: String = body.message.trim().chars().take(4000).collect();
    if message.is_empty() {
        return Err(ApiError::bad_request(crate::tr!("Напишите, что сделать.")));
    }
    if !dialog.enabled() {
        return Err(ApiError::conflict(crate::tr!(
            "Модель не выбрана: добавьте провайдера в «Персонаж» → «Мышление»."
        )));
    }
    let key = format!("workshop:{principal}:{id}");
    if body.fresh {
        dialog.forget(&key);
    }
    let started = Instant::now();
    let ms = move || started.elapsed().as_millis() as u64;
    let (events, receiver) = mpsc::unbounded_channel();
    let answers = dialog.profile_for(&agent::context(&workshop, &id, agent::model_of(&workshop)));
    let _ = events.send(WorkshopEvent::Start {
        model: answers.as_ref().map(|answers| answers.model.clone()).unwrap_or_default(),
        provider: answers.map(|answers| answers.title).unwrap_or_default(),
    });
    let told_events = events.clone();
    let told: Arc<dyn Fn(ChatStep) + Send + Sync> = Arc::new(move |step| {
        let event = match step {
            ChatStep::Tool { name, input } => WorkshopEvent::Tool { name, input },
            ChatStep::Result { name, content, failed } => {
                WorkshopEvent::Result { name, content, is_error: failed, ms: ms() }
            }
        };
        let _ = told_events.send(event);
    });
    // The work goes on when this panel closes (the plaque «Делаю виджет» and the workshop's log follow it):
    // «Остановить» ends it (activity.rs), not the connection.
    let mut phrases = agent::chat(&dialog, workshop.clone(), &key, &id, &message, told, CancellationToken::new());
    let secrets = secrets_service.clone();
    tokio::spawn(async move {
        loop {
            let next = phrases.next().await;
            match next {
                Some(Ok(text)) => {
                    let _ = events.send(WorkshopEvent::Text { text, ms: ms() });
                }
                Some(Err(error)) => {
                    let message = secrets.redact(&format!("{error:#}")).chars().take(300).collect();
                    let _ = events.send(WorkshopEvent::Error { message });
                    return;
                }
                None => {
                    let _ = events.send(WorkshopEvent::Done { ms: ms() });
                    return;
                }
            }
        }
    });
    let stream = UnboundedReceiverStream::new(receiver).map(move |event| {
        let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
        Ok::<_, Infallible>(format!("data: {data}\n\n"))
    });
    let mut response = (StatusCode::OK, axum::body::Body::from_stream(stream)).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, no-transform"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}

/// MCP over streamable HTTP without a stream: the server answers each POST with JSON.
async fn mcp_get() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, "MCP мастерской: только POST (JSON-RPC)").into_response()
}

fn rpc_result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The workshop as an MCP server: `initialize`, `tools/list`, `tools/call`, `ping`. The admin's key (a panel key with
/// the right to plugins) goes in `Authorization: Bearer`.
pub async fn mcp(
    Uses(workshop): Uses<Arc<Workshop>>,
    caller: Caller,
    Body(request): Body<Value>,
) -> Result<Response, ApiError> {
    admin(&caller)?;
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(Value::as_str).unwrap_or_default();
    // A notification has no id and gets no answer.
    if request.get("id").is_none() {
        return Ok(StatusCode::ACCEPTED.into_response());
    }
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    let answer = match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(MCP_PROTOCOL);
            rpc_result(
                &id,
                json!({
                    "protocolVersion": asked,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "duowave-workshop", "version": crate::VERSION },
                    "instructions": agent::skill_text(),
                }),
            )
        }
        "ping" => rpc_result(&id, json!({})),
        "tools/list" => {
            let tools: Vec<Value> = agent::tools(true)
                .into_iter()
                .map(|tool| json!({ "name": tool.name, "description": tool.description, "inputSchema": tool.parameters }))
                .collect();
            rpc_result(&id, json!({ "tools": tools }))
        }
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let arguments = params.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default();
            let draft = arguments.get("draft").and_then(Value::as_str).unwrap_or_default().to_string();
            let (text, failed) = agent::run_tool(&workshop, &draft, name, &arguments).await;
            // Claude Code's saves are steps of the draft's history, as the panel's by hand.
            if !failed && matches!(name, "write_file" | "append_file" | "edit_file" | "delete_file") {
                workshop.hand_edited(&draft, Author { who: Who::Mcp, name: None, via: Some("mcp".into()) });
            }
            rpc_result(&id, json!({ "content": [{ "type": "text", "text": text }], "isError": failed }))
        }
        other => rpc_error(&id, -32601, &format!("method {other} is not here")),
    };
    let mut response = axum::Json(answer).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}
