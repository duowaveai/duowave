//! The workshop over HTTP: `/api/v1/workshop/…` for the panel's «Мастерская», and `/mcp/workshop` — the same tools as
//! an MCP server (streamable HTTP, JSON answers) for Claude Code. All of it is the admin's at home: a draft is code that
//! runs in the gateway once installed. The agent's chat streams as server-sent events, like «Спросить мозг».
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{DefaultBodyLimit, Path, State};
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
use crate::app::App;
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller};
use crate::plugins::history::{Author, Who};
use crate::plugins::routes::admin;
use crate::settings::definitions::{BrainPick, WORKSHOP_AGENT};
use crate::utils::time::now_ms;

/// A file of a draft is up to 1 MB, sent as JSON.
const BODY_LIMIT: usize = 2 * 1024 * 1024;
/// MCP's protocol version this server speaks first.
const MCP_PROTOCOL: &str = "2025-06-18";

pub fn routes() -> Router<Arc<App>> {
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

async fn list(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<WorkshopView> {
    admin(&caller)?;
    Ok(ok(WorkshopView { dir: app.workshop.dir().display().to_string(), drafts: app.workshop.drafts() }))
}

#[derive(Debug, Default, Deserialize)]
pub struct CreateBody {
    id: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    title: Option<String>,
    /// An OpenAPI document in JSON: a tool per operation.
    #[serde(default)]
    openapi: Option<String>,
}

async fn create(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<CreateBody>) -> ApiResult<DraftView> {
    admin(&caller)?;
    let kind = body.kind.as_deref().or(Some("plugin")).filter(|_| body.openapi.is_none());
    Ok(created(app.workshop.create(body.id.trim(), kind, body.title.as_deref(), body.openapi.as_deref())?))
}

/// «Открыть в мастерской»: the draft, or one from the installed package of one's own.
async fn open(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<DraftView> {
    admin(&caller)?;
    Ok(ok(app.workshop.open(&id)?))
}

async fn one(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<DraftView> {
    admin(&caller)?;
    Ok(ok(app.workshop.draft(&id)?))
}

async fn remove(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    admin(&caller)?;
    app.workshop.delete(&id)?;
    Ok(ok(json!({ "deleted": true })))
}

/// The draft's panel part to look at before installing (docs/plans/desk.md, stage 5): an address with a token.
async fn preview(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    admin(&caller)?;
    let (module, styles) = app.workshop.preview(&id)?;
    Ok(ok(json!({ "module": module, "styles": styles })))
}

/// The previewed file: JavaScript or CSS, only from this gateway's own panel (the panel's content policy).
async fn preview_file(
    State(app): State<Arc<App>>,
    Path((token, file)): Path<(String, String)>,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;
    let text = app.workshop.preview_file(&token, &file).ok_or_else(|| ApiError::not_found("Not Found"))?;
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
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, path)): Path<(String, String)>,
) -> ApiResult<Value> {
    admin(&caller)?;
    let content = app.workshop.read(&id, &path)?;
    Ok(ok(json!({ "path": path, "content": content })))
}

#[derive(Debug, Deserialize)]
pub struct FileBody {
    content: String,
}

async fn write(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, path)): Path<(String, String)>,
    Body(body): Body<FileBody>,
) -> ApiResult<DraftView> {
    let actor = admin(&caller)?;
    let view = app.workshop.write(&id, &path, &body.content)?;
    app.workshop.hand_edited(&id, Author::of(actor.as_ref(), Who::Hand));
    Ok(ok(view))
}

async fn delete_file(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, path)): Path<(String, String)>,
) -> ApiResult<DraftView> {
    let actor = admin(&caller)?;
    let view = app.workshop.delete_file(&id, &path)?;
    app.workshop.hand_edited(&id, Author::of(actor.as_ref(), Who::Hand));
    Ok(ok(view))
}

async fn check(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<CheckReport> {
    admin(&caller)?;
    Ok(ok(app.workshop.check(&id).await?))
}

#[derive(Debug, Deserialize)]
pub struct RunBody {
    function: String,
    #[serde(default)]
    input: Option<Value>,
    /// On the draft's example (`examples.json`), never the network: the desk's tile and «Примерка».
    #[serde(default)]
    example: bool,
    /// Plain settings over the defaults; a private one is refused.
    #[serde(default)]
    settings: Map<String, Value>,
}

async fn run(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<RunBody>,
) -> ApiResult<RunReport> {
    admin(&caller)?;
    let trying = Trying { example: body.example, settings: app.workshop.plain_settings(&id, &body.settings)? };
    Ok(ok(app.workshop.run(&id, &body.function, body.input.unwrap_or_else(|| json!({})), &trying).await?))
}

#[derive(Debug, Deserialize)]
pub struct PhraseBody {
    text: String,
    #[serde(default)]
    example: bool,
    #[serde(default)]
    settings: Map<String, Value>,
}

async fn phrase(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<PhraseBody>,
) -> ApiResult<PhraseReport> {
    admin(&caller)?;
    let trying = Trying { example: body.example, settings: app.workshop.plain_settings(&id, &body.settings)? };
    Ok(ok(app.workshop.try_phrase(&id, &body.text, &trying).await?))
}

async fn journal(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    admin(&caller)?;
    Ok(ok(json!({ "lines": app.workshop.journal(&id) })))
}

/// A step as the panel gets it: what a key or a token may have put into its text is hidden.
fn shown(app: &App, mut step: Step) -> Step {
    step.text = app.secrets.redact(&step.text);
    step
}

/// A draft's work with its title, for the plaque «Делаю виджет «…»».
fn job_view(app: &App, mut job: Job) -> Value {
    job.asked = app.secrets.redact(&job.asked);
    job.last = job.last.map(|step| shown(app, step));
    let title = app.workshop.draft(&job.draft).ok().and_then(|draft| draft.title);
    let mut view = serde_json::to_value(&job).unwrap_or_default();
    view["title"] = json!(title);
    view
}

/// What the agent does now (docs/plans/desk.md, «Делаю виджет»): the drafts it works on, whoever started it, and the
/// ones it finished in the last minute and a half.
/// `now` is this gateway's clock (ms since the epoch): the helper's panel reads a station's work through its pass and
/// counts how long a step goes on by the station's own clock, not the computer's.
async fn activity(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    admin(&caller)?;
    let jobs: Vec<Value> = app.workshop.activity().jobs().into_iter().map(|job| job_view(&app, job)).collect();
    Ok(ok(json!({ "jobs": jobs, "now": now_ms() })))
}

#[derive(Debug, Default, Deserialize)]
pub struct AfterQuery {
    /// Only the steps after this one (`seq`); none — all that is kept.
    #[serde(default)]
    after: u64,
}

/// A draft's work and its steps: what was asked, the calls and their results (each naming its call), what the agent
/// said, how it ended; `now` as in `activity`.
async fn activity_log(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<AfterQuery>,
) -> ApiResult<Value> {
    admin(&caller)?;
    let Some((job, steps)) = app.workshop.activity().log(&id, query.after) else {
        return Ok(ok(json!({ "job": null, "steps": [], "now": now_ms() })));
    };
    let steps: Vec<Step> = steps.into_iter().map(|step| shown(&app, step)).collect();
    Ok(ok(json!({ "job": job_view(&app, job), "steps": steps, "now": now_ms() })))
}

/// «Остановить»: the agent's work on the draft ends now, by voice or from the panel alike.
async fn stop(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    admin(&caller)?;
    Ok(ok(json!({ "stopped": app.workshop.activity().stop(&id) })))
}

#[derive(Debug, Default, Deserialize)]
pub struct MakeBody {
    /// «Сделай виджет: мои дела на сегодня»; with `draft` — new work on it («переделай: крупнее»).
    words: String,
    #[serde(default)]
    draft: Option<String>,
}

/// «Сделай…» from the panel (docs/plans/voice-creation.md): a draft of the request's face, its card, the agent in the
/// background — the same road as the station's. Words that are not a request: 422.
async fn make(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<MakeBody>) -> ApiResult<super::Made> {
    let actor = admin(&caller)?;
    let who = actor.map(|actor| actor.name);
    Ok(created(app.workshop.make(&body.words, body.draft.as_deref(), super::activity::By::Panel, who)?))
}

/// «В черновики»: the draft waits in the workshop, off the desk (docs/plans/voice-creation.md).
async fn park(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<DraftView> {
    admin(&caller)?;
    Ok(ok(app.workshop.park(&id)?))
}

#[derive(Debug, Default, Deserialize)]
pub struct InstallBody {
    /// The admin's «ок» to the first install or new rights, given in the panel with them on the screen.
    #[serde(default)]
    approve: bool,
    /// The values given in «Примерка», plain and private: written before the plugin first starts.
    #[serde(default)]
    settings: Map<String, Value>,
}

async fn install(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<InstallBody>,
) -> ApiResult<InstallReport> {
    let actor = admin(&caller)?;
    let report = app.workshop.install(&id, body.approve, &body.settings, actor).await?;
    if report.installed {
        app.characters.sync();
    }
    Ok(ok(report))
}

/// What «Поставить» would do: shown before the admin says «да», whether or not it asks for new rights.
async fn plan(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<InstallPlan> {
    admin(&caller)?;
    Ok(ok(app.workshop.plan(&id).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct TrialBody {
    #[serde(default)]
    approve: bool,
    /// How long, minutes: 10 unless given, at most an hour.
    #[serde(default)]
    minutes: Option<u32>,
}

/// «Проба на колонке»: the draft runs on the gateway for a while, then the version before comes back by itself.
async fn trial(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<TrialBody>,
) -> ApiResult<InstallReport> {
    let actor = admin(&caller)?;
    Ok(ok(app.workshop.start_trial(&id, body.minutes.unwrap_or(10), body.approve, actor).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct EndTrialQuery {
    /// Keep the trial's version as an install instead of taking it back.
    #[serde(default)]
    keep: bool,
}

async fn end_trial(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<EndTrialQuery>,
) -> ApiResult<Value> {
    let actor = admin(&caller)?;
    let message = app.workshop.end_trial(&id, query.keep, actor).await?;
    Ok(ok(json!({ "message": message, "draft": app.workshop.draft(&id).ok() })))
}

/// The agent's model: its own choice (null — the smart profile of «Мозг») and the model that answers now.
fn agent_view(app: &App) -> Value {
    let choice = app.settings.get(&WORKSHOP_AGENT, None);
    let context = agent::context(&app.workshop, "", choice.clone());
    json!({ "choice": choice, "answers": app.dialog.profile_for(&context), "smart": app.dialog.brain().profiles.smart })
}

async fn agent_model(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    admin(&caller)?;
    Ok(ok(agent_view(&app)))
}

#[derive(Debug, Default, Deserialize)]
pub struct AgentBody {
    /// A provider of «Мозг» and its model; null — back to the smart profile.
    #[serde(default)]
    choice: Option<BrainPick>,
}

/// The agent's own model; «Мозг» itself does not change.
async fn set_agent_model(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<AgentBody>) -> ApiResult<Value> {
    let actor = admin(&caller)?;
    match body.choice {
        None => app.settings.forget(&WORKSHOP_AGENT)?,
        Some(pick) => {
            let pick = BrainPick { provider: pick.provider.trim().to_string(), model: pick.model.trim().to_string() };
            if pick.model.is_empty() {
                return Err(ApiError::field("model", crate::tr!("выберите модель")));
            }
            if !app.brain.view().providers.iter().any(|provider| provider.id == pick.provider) {
                return Err(ApiError::field("provider", crate::tr!("такого провайдера нет в «Мозге»")));
            }
            app.settings.set(&WORKSHOP_AGENT, &pick, actor)?;
        }
    }
    Ok(ok(agent_view(&app)))
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
    State(app): State<Arc<App>>,
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
    if !app.dialog.enabled() {
        return Err(ApiError::conflict(crate::tr!("Модель не выбрана: добавьте провайдера в «Персонаж» → «Мозг».")));
    }
    let key = format!("workshop:{principal}:{id}");
    if body.fresh {
        app.dialog.forget(&key);
    }
    let started = Instant::now();
    let ms = move || started.elapsed().as_millis() as u64;
    let (events, receiver) = mpsc::unbounded_channel();
    let answers = app.dialog.profile_for(&agent::context(&app.workshop, &id, agent::model_of(&app.workshop)));
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
    let mut phrases =
        agent::chat(&app.dialog, app.workshop.clone(), &key, &id, &message, told, CancellationToken::new());
    let secrets = app.secrets.clone();
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
async fn mcp(State(app): State<Arc<App>>, caller: Caller, Body(request): Body<Value>) -> Result<Response, ApiError> {
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
            let (text, failed) = agent::run_tool(&app.workshop, &draft, name, &arguments).await;
            // Claude Code's saves are steps of the draft's history, as the panel's by hand.
            if !failed && matches!(name, "write_file" | "append_file" | "edit_file" | "delete_file") {
                app.workshop.hand_edited(&draft, Author { who: Who::Mcp, name: None, via: Some("mcp".into()) });
            }
            rpc_result(&id, json!({ "content": [{ "type": "text", "text": text }], "isError": failed }))
        }
        other => rpc_error(&id, -32601, &format!("method {other} is not here")),
    };
    let mut response = axum::Json(answer).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::core::identity::principal::user_principal;
    use crate::core::identity::Role;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn shows_the_admin_what_the_agent_does_on_a_draft_under_its_title_and_stops_it() {
        use crate::plugins::workshop::activity::By;

        let dir = tempfile::tempdir().unwrap();
        let text = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ]);
        let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
        let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
        let member = || Caller::local(app.clone(), Some(user_principal("u2", "Аня", Role::Member, None)));
        app.workshop.create("widget-a", Some("plugin"), Some("Курс крипты"), None).unwrap();
        let before = now_ms();
        let idle = activity(State(app.clone()), admin()).await.unwrap().1;
        assert_eq!(idle["jobs"], json!([]));
        // The gateway's clock comes with each answer: the helper's panel counts a station's work by the station's.
        assert!(idle["now"].as_u64().is_some_and(|now| now >= before && now <= now_ms()), "{idle}");
        assert!(activity(State(app.clone()), member()).await.is_err(), "the workshop is the admin's");
        let from = |after: u64| axum::extract::Query(AfterQuery { after });
        let none = activity_log(State(app.clone()), admin(), Path("widget-a".into()), from(0)).await.unwrap().1;
        assert!(none["job"].is_null() && none["now"].as_u64().is_some(), "{none}");

        let run = app.workshop.activity().begin(
            "widget-a",
            By::Voice,
            "Сделай виджет: курс биткоина",
            &CancellationToken::new(),
        );
        let file = json!({"path": "panel.js", "content": "…"});
        let call = app.workshop.activity().call(&run, "write_file", file.as_object().unwrap());
        let jobs = activity(State(app.clone()), admin()).await.unwrap().1;
        let job = &jobs["jobs"][0];
        assert_eq!(
            (job["title"].clone(), job["by"].clone(), job["busy"].clone(), job["last"]["tool"].clone()),
            (json!("Курс крипты"), json!("voice"), json!(true), json!("write_file"))
        );
        app.workshop.activity().answer(&run, call, "write_file", "Записано.", false);
        let log = activity_log(State(app.clone()), admin(), Path("widget-a".into()), from(0)).await.unwrap().1;
        assert!(log["now"].as_u64().is_some_and(|now| now >= before), "{log}");
        let steps = log["steps"].as_array().unwrap().clone();
        assert_eq!(
            steps.iter().map(|step| step["kind"].clone()).collect::<Vec<_>>(),
            vec![json!("asked"), json!("tool"), json!("result")]
        );
        assert_eq!((steps[1]["seq"].clone(), steps[2]["call"].clone()), (json!(call), json!(call)), "its call");

        // «Остановить»: the run's token is cancelled, the log ends with it.
        assert_eq!(stop(State(app.clone()), admin(), Path("widget-a".into())).await.unwrap().1["stopped"], json!(true));
        assert!(run.cancel.is_cancelled());
        let after = steps[2]["seq"].as_u64().unwrap();
        let log = activity_log(
            State(app.clone()),
            admin(),
            Path("widget-a".into()),
            axum::extract::Query(AfterQuery { after }),
        )
        .await
        .unwrap()
        .1;
        assert_eq!((log["job"]["busy"].clone(), log["steps"][0]["kind"].clone()), (json!(false), json!("stopped")));
        assert_eq!(
            stop(State(app.clone()), admin(), Path("widget-a".into())).await.unwrap().1["stopped"],
            json!(false)
        );
    }

    /// Stage 2 of docs/plans/voice-creation.md: a draft's card says who asked and where it is; the agent's work moves
    /// it (working, then ready with its last words); what it needs and the ready line come from its manifest; the
    /// panel hears which draft and what happened; «В черновики» parks it; a deleted draft takes its card along.
    #[tokio::test]
    async fn a_drafts_card_says_who_asked_and_where_it_is_and_the_panel_hears_which_draft() {
        use crate::core::events::GatewayEvent;
        use crate::plugins::workshop::activity::{By, StepKind};
        use crate::plugins::workshop::{DraftMeta, Face, Stage};

        let dir = tempfile::tempdir().unwrap();
        let text = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ]);
        let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
        let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
        let mut heard = app.events.subscribe();
        app.workshop.create("widget-1", Some("widget"), Some("Дела на сегодня"), None).unwrap();
        let mut manifest: Value =
            serde_json::from_str(&app.workshop.read("widget-1", "manifest.json").unwrap()).unwrap();
        manifest["settings"] = json!([
            { "key": "token", "type": "string", "title": "Ключ Todoist", "secret": true },
            { "key": "count", "type": "number", "title": "Показывать", "default": 5 }
        ]);
        app.workshop.write("widget-1", "manifest.json", &manifest.to_string()).unwrap();

        // No card yet: one read from what is known — the panel's, a widget by its card, waiting in the workshop.
        let view = one(State(app.clone()), admin(), Path("widget-1".into())).await.unwrap().1;
        assert_eq!(
            (view.meta.by, view.meta.stage, view.meta.kind, view.meta.kept),
            (By::Panel, Stage::Parked, Face::Widget, false)
        );
        let card = DraftMeta {
            by: By::Panel,
            who: Some("Админ".into()),
            asked: "мои дела на сегодня из Тудуиста".into(),
            kind: Face::Widget,
            stage: Stage::Working,
            updated: 0,
            note: None,
            kept: true,
        };
        app.workshop.keep_card("widget-1", card, Some("made")).unwrap();
        let run = app.workshop.activity().begin("widget-1", By::Panel, "мои дела", &CancellationToken::new());
        app.workshop.activity().said(&run, "Сделал виджет: дела на сегодня из Todoist.");
        app.workshop.activity().said(&run, "Ключ впишите в «Примерке».");
        app.workshop.activity().end(&run, StepKind::Done, "");

        let view = one(State(app.clone()), admin(), Path("widget-1".into())).await.unwrap().1;
        assert_eq!((view.meta.stage, view.meta.who.as_deref()), (Stage::Ready, Some("Админ")));
        assert_eq!(
            view.meta.note.as_deref(),
            Some("Сделал виджет: дела на сегодня из Todoist. Ключ впишите в «Примерке».")
        );
        assert_eq!(
            view.ready_line.as_deref(),
            Some("Готово: виджет «Дела на сегодня». Ему нужен ключ Todoist. Примерите — или сразу на стол?")
        );
        let needs = serde_json::to_value(&view.needs).unwrap();
        assert_eq!(
            needs[0],
            json!({ "key": "token", "title": "Ключ Todoist", "type": "string", "secret": true, "required": true })
        );
        assert_eq!((needs[1]["default"].clone(), needs[1]["required"].clone()), (json!(5), json!(false)));
        let listed = list(State(app.clone()), admin()).await.unwrap().1;
        assert_eq!(serde_json::to_value(&listed).unwrap()["drafts"][0]["meta"]["stage"], json!("ready"));

        // «В черновики».
        let parked = park(State(app.clone()), admin(), Path("widget-1".into())).await.unwrap().1;
        assert_eq!(parked.meta.stage, Stage::Parked);
        // The panel heard which draft and what happened to it, between the agent's steps.
        let mut changes = Vec::new();
        while let Ok(event) = heard.try_recv() {
            if let GatewayEvent::WorkshopChanged { draft, change } = event.as_ref() {
                // The card's changes; the agent's steps and the apps' history ride the same event.
                if change != "agent" && change != "history" {
                    changes.push(format!("{draft}:{change}"));
                }
            }
        }
        assert_eq!(changes, ["widget-1:made", "widget-1:ready", "widget-1:parked"]);

        app.workshop.delete("widget-1").unwrap();
        assert!(!app.workshop.dir().join(".meta/widget-1.json").exists(), "the card goes with the draft");
    }

    /// Stage 3: a run on the draft's example takes plain settings and refuses a private one; the widget's template
    /// carries an empty example and passes «Проверить».
    #[tokio::test]
    async fn runs_a_draft_on_its_example_with_plain_settings_and_never_a_private_one() {
        let dir = tempfile::tempdir().unwrap();
        let text = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ]);
        let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
        let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
        let made = app.workshop.create("widget-2", Some("widget"), Some("Отметки"), None).unwrap();
        assert!(made.files.iter().any(|file| file.path == "examples.json"), "the template carries an empty example");
        assert!(app.workshop.read("widget-2", "examples.json").is_ok(), "a root file of a draft");
        let checked = check(State(app.clone()), admin(), Path("widget-2".into())).await.unwrap().1;
        assert!(checked.ok && checked.warnings.is_empty(), "{checked:?}");
        let mut manifest: Value =
            serde_json::from_str(&app.workshop.read("widget-2", "manifest.json").unwrap()).unwrap();
        manifest["settings"] = json!([
            { "key": "token", "type": "string", "title": "Ключ", "secret": true },
            { "key": "caption", "type": "string", "title": "Подпись", "default": "отметок" }
        ]);
        app.workshop.write("widget-2", "manifest.json", &manifest.to_string()).unwrap();
        let body = |settings: Value| RunBody {
            function: "panelData".into(),
            input: None,
            example: true,
            settings: settings.as_object().unwrap().clone(),
        };
        let ran = run(State(app.clone()), admin(), Path("widget-2".into()), Body(body(json!({ "caption": "дел" }))))
            .await
            .unwrap()
            .1;
        assert!(ran.ok && ran.example, "{ran:?}");
        let Err(private) =
            run(State(app.clone()), admin(), Path("widget-2".into()), Body(body(json!({ "token": "x-1234" })))).await
        else {
            panic!("a private setting reached the sandbox")
        };
        assert!(private
            .errors
            .as_ref()
            .is_some_and(|errors| errors["token"][0].as_str().unwrap().starts_with("личное")));
        assert!(run(State(app.clone()), admin(), Path("widget-2".into()), Body(body(json!({ "nope": 1 }))))
            .await
            .is_err());
        assert!(run(State(app.clone()), admin(), Path("widget-2".into()), Body(body(json!({ "caption": 3 }))))
            .await
            .is_err());
    }

    /// Stage 4 of docs/plans/voice-creation.md: the values given in «Примерка» are checked against the draft's
    /// manifest and written before the plugin first starts — it works with its key from the first call. Nothing is
    /// written while the admin's «да» is awaited, nor when a value is wrong.
    #[tokio::test]
    async fn installs_a_draft_with_its_settings_and_the_plugin_starts_with_its_key() {
        use crate::plugins::runtime::CallKind;
        use crate::settings::definitions::PACKAGE_SETTINGS;

        let dir = tempfile::tempdir().unwrap();
        let text = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ]);
        let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
        let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
        app.workshop.create("todo", Some("plugin"), Some("Дела"), None).unwrap();
        let manifest = json!({
            "schema": 1, "kind": "plugin", "id": "todo", "version": "0.1.0", "title": "Дела", "api": "^2",
            "runs": ["pc", "station", "stationServer"], "permissions": [],
            "settings": [
                { "key": "token", "type": "string", "title": "Ключ Todoist", "secret": true },
                { "key": "count", "type": "number", "title": "Показывать", "default": 5 }
            ],
            "contributes": { "intents": [{ "phrases": ["мои дела"], "handler": "tasks" }] }
        });
        app.workshop.write("todo", "manifest.json", &manifest.to_string()).unwrap();
        let code =
            r#"export function tasks() { return `${host.settings.get("token")} ${host.settings.get("count")}`; }"#;
        app.workshop.write("todo", "backend.js", code).unwrap();
        let body =
            |approve: bool, settings: Value| InstallBody { approve, settings: settings.as_object().unwrap().clone() };
        let stored = || app.settings.get(&PACKAGE_SETTINGS, None).unwrap_or_default().get("todo").cloned();

        // The «да» is awaited: nothing written yet — the key lives in the page alone.
        let asked =
            install(State(app.clone()), admin(), Path("todo".into()), Body(body(false, json!({ "token": "td-0123" }))))
                .await
                .unwrap()
                .1;
        assert!(asked.needs_approval.is_some() && !asked.installed);
        assert!(!app.secrets.has("plugin:todo:token") && stored().is_none());
        // A wrong value, an unknown one: nothing installed, nothing written.
        for wrong in [json!({ "token": "td-0123", "count": "три" }), json!({ "token": "td-0123", "nope": 1 })] {
            assert!(install(State(app.clone()), admin(), Path("todo".into()), Body(body(true, wrong))).await.is_err());
        }
        assert!(app.plugins.installed("todo").is_none() && !app.secrets.has("plugin:todo:token"));

        let given = json!({ "token": "td-0123", "count": 3 });
        let done = install(State(app.clone()), admin(), Path("todo".into()), Body(body(true, given))).await.unwrap().1;
        assert!(done.installed, "{}", done.message);
        assert_eq!(app.plugins.call("todo", "tasks", json!({}), CallKind::Voice).await.unwrap(), json!("td-0123 3"));
        assert_eq!(
            stored(),
            Some(json!({ "count": 3 }).as_object().unwrap().clone()),
            "the key is not among the settings"
        );
        assert_eq!(app.secrets.get("plugin:todo:token").as_deref(), Some("td-0123"));
        let shown = app.plugins.view_one("todo").await.unwrap();
        assert_eq!(shown.settings[0].value, Some(json!(crate::plugins::manifest::SECRET_SHOWN)));
    }

    /// Stage 5: «сделай…» from the panel — the same road as the station's: a draft of the request's face under the
    /// panel's name, its card with who asked, the agent in the background; words that are not a request — 422.
    #[tokio::test]
    async fn makes_a_draft_from_the_panels_words_and_refuses_what_is_not_a_request() {
        use crate::plugins::workshop::activity::By;
        use crate::plugins::workshop::{Face, Stage};

        let dir = tempfile::tempdir().unwrap();
        let text = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("DIALOG_PROVIDER", "echo".into()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ]);
        let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
        let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
        let member = || Caller::local(app.clone(), Some(user_principal("u2", "Аня", Role::Member, None)));
        let words = |words: &str| Body(MakeBody { words: words.into(), draft: None });
        assert!(make(State(app.clone()), member(), words("Сделай виджет: погода")).await.is_err(), "the admin's");
        let Err(not) = make(State(app.clone()), admin(), words("Какая погода завтра?")).await else {
            panic!("not a request, made")
        };
        assert_eq!(not.status, StatusCode::UNPROCESSABLE_ENTITY);
        let made =
            make(State(app.clone()), admin(), words("Сделай виджет: мои дела на сегодня из Тудуиста")).await.unwrap().1;
        assert_eq!(
            (made.draft.as_str(), made.kind, made.title.as_str()),
            ("widget-1", Face::Widget, "Мои дела на сегодня из тудуиста")
        );
        let card = app.workshop.card("widget-1");
        assert_eq!(
            (card.by, card.who.as_deref(), card.asked.as_str()),
            (By::Panel, Some("Админ"), "Сделай виджет: мои дела на сегодня из Тудуиста")
        );
        // The agent finishes in the background.
        for _ in 0..200 {
            if app.workshop.card("widget-1").stage == Stage::Ready {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert_eq!(app.workshop.card("widget-1").stage, Stage::Ready);
        // «Переделай» on the same draft.
        let again = make(
            State(app.clone()),
            admin(),
            Body(MakeBody { words: "Переделай: крупнее".into(), draft: Some("widget-1".into()) }),
        )
        .await
        .unwrap()
        .1;
        assert_eq!(again.draft, "widget-1");
        assert_eq!(app.workshop.card("widget-1").asked, "Переделай: крупнее");
    }

    #[tokio::test]
    async fn writes_checks_and_installs_a_draft_the_admins_approval_first_and_serves_its_tools_over_mcp() {
        let dir = tempfile::tempdir().unwrap();
        let text = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ]);
        let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
        let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
        let member = || Caller::local(app.clone(), Some(user_principal("u2", "Аня", Role::Member, None)));

        let body =
            CreateBody {
                id: "hello".into(), kind: Some("plugin".into()), title: Some("Привет".into()), openapi: None
            };
        assert!(create(State(app.clone()), member(), Body(CreateBody { id: "x".into(), ..CreateBody::default() }))
            .await
            .is_err());
        let made = create(State(app.clone()), admin(), Body(body)).await.unwrap().1;
        assert!(made.files.iter().any(|file| file.path == "backend.js"));
        let checked = check(State(app.clone()), admin(), Path("hello".into())).await.unwrap().1;
        assert!(checked.ok, "{checked:?}");
        let tried = phrase(
            State(app.clone()),
            admin(),
            Path("hello".into()),
            Body(PhraseBody { text: "Скажи привет Маша".into(), example: false, settings: Map::new() }),
        )
        .await
        .unwrap()
        .1;
        assert_eq!(tried.speech.as_deref(), Some("Привет, маша! Это 1-й раз."));

        // The first install waits for the admin's «ок»; then it is on.
        let asked = install(
            State(app.clone()),
            admin(),
            Path("hello".into()),
            Body(InstallBody { approve: false, ..InstallBody::default() }),
        )
        .await
        .unwrap()
        .1;
        assert!(!asked.installed && asked.needs_approval.as_ref().is_some_and(|approval| approval.first));
        let done = install(
            State(app.clone()),
            admin(),
            Path("hello".into()),
            Body(InstallBody { approve: true, ..InstallBody::default() }),
        )
        .await
        .unwrap()
        .1;
        assert!(done.installed, "{}", done.message);
        assert_eq!(done.version.as_deref(), Some("0.1.0"));
        assert!(app.plugins.is_active("hello"));
        // The same version again is the next patch; nothing new asked of the admin.
        let again = install(
            State(app.clone()),
            admin(),
            Path("hello".into()),
            Body(InstallBody { approve: false, ..InstallBody::default() }),
        )
        .await
        .unwrap()
        .1;
        assert_eq!((again.installed, again.version.as_deref()), (true, Some("0.1.1")));
        assert!(app.workshop.journal("hello").iter().any(|line| line.message.contains("0.1.1")));
        // A new draft under the installed plugin's id would be its next version: refused with the way out.
        app.workshop.delete("hello").unwrap();
        let taken = app.workshop.create("hello", Some("plugin"), None, None).unwrap_err();
        assert!(
            taken.message.contains("id «hello» уже занят") && taken.message.contains("Открыть в мастерской"),
            "{}",
            taken.message
        );
        assert!(app.workshop.create("hello", None, None, None).is_ok(), "opening the own plugin itself still works");

        // The workshop as an MCP server.
        let listed = mcp(State(app.clone()), admin(), Body(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})))
            .await
            .unwrap();
        let listed: Value = serde_json::from_slice(&listed.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert!(listed["result"]["tools"].as_array().unwrap().iter().any(|tool| tool["name"] == "try_phrase"));
        let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "read_file", "arguments": {"draft": "hello", "path": "manifest.json"}}});
        let answer = mcp(State(app.clone()), admin(), Body(call)).await.unwrap();
        let answer: Value = serde_json::from_slice(&answer.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert!(answer["result"]["content"][0]["text"].as_str().unwrap().contains("\"0.1.1\""), "{answer}");
        let note =
            mcp(State(app.clone()), admin(), Body(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})))
                .await
                .unwrap();
        assert_eq!(note.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn shows_the_plan_before_an_install_tries_a_draft_for_a_while_and_keeps_the_agents_model_apart() {
        let dir = tempfile::tempdir().unwrap();
        let text = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ]);
        let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
        let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
        let member = || Caller::local(app.clone(), Some(user_principal("u2", "Аня", Role::Member, None)));
        app.workshop.create("hello", Some("plugin"), Some("Привет"), None).unwrap();

        // The plan of the first install: nothing installed yet; the template asks for no rights (Host API 2: what it
        // gives is no right).
        assert!(plan(State(app.clone()), member(), Path("hello".into())).await.is_err());
        let first = plan(State(app.clone()), admin(), Path("hello".into())).await.unwrap().1;
        assert!(first.first && first.from.is_none(), "{first:?}");
        assert_eq!(first.to, "0.1.0");
        assert!(first.added.is_empty() && first.removed.is_empty() && first.kept.is_empty(), "{first:?}");
        assert!(!first.phrases.is_empty(), "the template's phrases: {first:?}");
        let done = install(
            State(app.clone()),
            admin(),
            Path("hello".into()),
            Body(InstallBody { approve: true, ..InstallBody::default() }),
        )
        .await
        .unwrap()
        .1;
        assert!(done.installed, "{}", done.message);
        // The plan of the next one: the same rights kept, the next patch, nothing installed by asking.
        let next = plan(State(app.clone()), admin(), Path("hello".into())).await.unwrap().1;
        assert_eq!((next.first, next.from.as_deref(), next.to.as_str()), (false, Some("0.1.0"), "0.1.1"));
        assert!(next.added.is_empty() && next.removed.is_empty(), "{next:?}");
        assert!(next.enabled);
        assert_eq!(app.plugins.installed("hello").unwrap().version(), "0.1.0", "a plan installs nothing");

        // A trial puts the draft in for a while; taken back, the version before is there again.
        let body = || Body(TrialBody { approve: false, minutes: Some(5) });
        let tried = trial(State(app.clone()), admin(), Path("hello".into()), body()).await.unwrap().1;
        assert!(tried.installed && tried.message.contains("Проба"), "{}", tried.message);
        assert_eq!(app.plugins.installed("hello").unwrap().version(), "0.1.1");
        let running = app.workshop.draft("hello").unwrap().trial.expect("the draft shows its trial");
        assert_eq!((running.version.as_str(), running.previous.as_deref()), ("0.1.1", Some("0.1.0")));
        assert!(trial(State(app.clone()), admin(), Path("hello".into()), body()).await.is_err(), "one at a time");
        let query = |keep| axum::extract::Query(EndTrialQuery { keep });
        end_trial(State(app.clone()), admin(), Path("hello".into()), query(false)).await.unwrap();
        assert_eq!(app.plugins.installed("hello").unwrap().version(), "0.1.0");
        assert!(app.plugins.is_active("hello"));
        assert!(app.workshop.draft("hello").unwrap().trial.is_none());
        // Kept, the trial's version stays as an install.
        trial(State(app.clone()), admin(), Path("hello".into()), body()).await.unwrap();
        end_trial(State(app.clone()), admin(), Path("hello".into()), query(true)).await.unwrap();
        assert_eq!(app.plugins.installed("hello").unwrap().version(), "0.1.1");
        assert!(end_trial(State(app.clone()), admin(), Path("hello".into()), query(false)).await.is_err());

        // A trial of a plugin that was not installed: it asks the first install's approval, and goes after.
        app.workshop.create("bye", Some("plugin"), Some("Пока"), None).unwrap();
        let asked = trial(State(app.clone()), admin(), Path("bye".into()), body()).await.unwrap().1;
        assert!(!asked.installed && asked.needs_approval.is_some());
        let approved = Body(TrialBody { approve: true, minutes: None });
        assert!(trial(State(app.clone()), admin(), Path("bye".into()), approved).await.unwrap().1.installed);
        assert!(app.plugins.is_active("bye"));
        end_trial(State(app.clone()), admin(), Path("bye".into()), query(false)).await.unwrap();
        assert!(app.plugins.installed("bye").is_none());

        // The agent's own model: a provider of «Мозг» or none; «Мозг» itself stays as it was.
        let brain = app.settings.get(&crate::settings::definitions::BRAIN, None);
        let unknown = Some(BrainPick { provider: "nope".into(), model: "big".into() });
        assert!(set_agent_model(State(app.clone()), admin(), Body(AgentBody { choice: unknown.clone() }))
            .await
            .is_err());
        assert!(set_agent_model(State(app.clone()), member(), Body(AgentBody { choice: None })).await.is_err());
        app.settings.set(&WORKSHOP_AGENT, &unknown.clone().unwrap(), None).unwrap();
        assert_eq!(agent::context(&app.workshop, "hello", agent::model_of(&app.workshop)).model, unknown);
        let reset = set_agent_model(State(app.clone()), admin(), Body(AgentBody { choice: None })).await.unwrap().1;
        assert!(reset["choice"].is_null(), "{reset}");
        assert!(agent::model_of(&app.workshop).is_none());
        assert_eq!(app.settings.get(&crate::settings::definitions::BRAIN, None), brain);
    }

    #[tokio::test]
    async fn starts_a_widget_that_passes_the_check_installs_and_shows_its_preview_by_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let text = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ]);
        let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
        let admin = || Caller::local(app.clone(), Some(user_principal("u1", "Админ", Role::Admin, None)));
        // A title said in words, quotes and all, stays the card's title in the manifest.
        app.workshop.create("widget-wash", Some("widget"), Some("Стирка \"быстрая\""), None).unwrap();
        let report = app.workshop.check("widget-wash").await.unwrap();
        assert!(report.ok && report.warnings.is_empty(), "{report:?}");
        for function in ["panelData", "mark"] {
            assert!(report.exports.iter().any(|name| name == function), "{function}: {report:?}");
        }
        let manifest: Value =
            serde_json::from_str(&app.workshop.read("widget-wash", "manifest.json").unwrap()).unwrap();
        assert_eq!(manifest["contributes"]["panel"]["cards"][0]["title"], "Стирка \"быстрая\"");
        // A card the panel draws: its function's answer is a card of known parts.
        let shown = app.workshop.run("widget-wash", "panelData", json!({}), &Trying::example()).await.unwrap();
        assert!(shown.ok && crate::bus::cards::check(&shown.result).is_empty(), "{shown:?}");

        // A part the panel does not know, and a tap to a function the panel may not call: the check names both.
        app.workshop
            .write(
                "widget-wash",
                "backend.js",
                "export function panelData() { return { big: \"1\", gauge: 3, buttons: [{ text: \"Сброс\", tap: \"reset\" }] }; }\nexport function mark() { return panelData(); }\nexport function reset() { return panelData(); }\n",
            )
            .unwrap();
        let warned = app.workshop.check("widget-wash").await.unwrap();
        assert!(warned.warnings.iter().any(|warning| warning.contains("«gauge»")), "{warned:?}");
        assert!(warned.warnings.iter().any(|warning| warning.contains("reset")), "{warned:?}");
        app.workshop
            .write(
                "widget-wash",
                "backend.js",
                &crate::plugins::workshop::templates::widget("widget-wash", "Стирка")[1].1,
            )
            .unwrap();

        let done = install(
            State(app.clone()),
            admin(),
            Path("widget-wash".into()),
            Body(InstallBody { approve: true, ..InstallBody::default() }),
        )
        .await
        .unwrap()
        .1;
        assert!(done.installed, "{}", done.message);

        // Its reading writes nothing: no word to the panel, or every widget on it would read again each second for
        // as long as it is open (review: packages-data-loop). A person's «Отметить» writes, and is told.
        let mut heard = app.events.subscribe();
        for _ in 0..2 {
            let read = app.plugins.panel_call("widget-wash", "panelData", json!({})).await.unwrap();
            assert_eq!(read["big"], json!("0"), "{read}");
        }
        let marked = app.plugins.panel_call("widget-wash", "mark", json!({})).await.unwrap();
        assert_eq!(marked["big"], json!("1"), "{marked}");
        assert_eq!(app.plugins.panel_call("widget-wash", "panelData", json!({})).await.unwrap()["big"], json!("1"));
        let mut told = Vec::new();
        while let Ok(event) = heard.try_recv() {
            if let crate::core::events::GatewayEvent::PackageDataChanged { package } = event.as_ref() {
                told.push(package.clone());
            }
        }
        assert_eq!(told, ["widget-wash"], "the mark only");

        // A window of its own that only shows rows could be a card: the check says so.
        app.workshop
            .write(
                "widget-wash",
                "panel.js",
                "import { h } from \"preact\";\nexport default definePlugin({ id: \"widget-wash\", slots: { \"desk.widgets\": [{ id: \"rows\", Component: () => h(\"p\", null, \"строка\") }] } });\n",
            )
            .unwrap();
        let advised = app.workshop.check("widget-wash").await.unwrap();
        assert!(advised.warnings.iter().any(|warning| warning.contains("может быть карточкой")), "{advised:?}");

        // A panel part that does not parse fails the check here, not in the panel.
        app.workshop
            .write("widget-wash", "panel.js", "export default definePlugin({ id: \"widget-wash\", slots: { ")
            .unwrap();
        let broken = app.workshop.check("widget-wash").await.unwrap();
        assert!(!broken.ok && broken.code.as_deref().is_some_and(|code| code.starts_with("panel.js:")), "{broken:?}");
    }

    #[tokio::test]
    async fn serves_a_drafts_preview_to_the_apps_window_across_origins_and_nothing_without_its_token() {
        let dir = tempfile::tempdir().unwrap();
        let text = |sub: &str| dir.path().join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ]);
        let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
        app.workshop.create("waves", Some("plugin"), Some("Волны"), None).unwrap();
        app.workshop.write("waves", "panel.js", "export default {};\n").unwrap();
        let (module, _) = app.workshop.preview("waves").unwrap();
        let token = module.trim_start_matches("/panel-preview/").trim_end_matches("/panel.js").to_string();

        // The app's window is a sandboxed frame of no origin: its `import()` is across origins and needs the allowance.
        let response = preview_file(State(app.clone()), Path((token.clone(), "panel.js".into()))).await.unwrap();
        let headers = response.headers();
        assert_eq!(headers[axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(headers[axum::http::header::CONTENT_TYPE], "text/javascript; charset=utf-8");
        assert_eq!(headers[axum::http::header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"export default {};\n");

        // Only its two parts, only by a live token.
        assert!(preview_file(State(app.clone()), Path((token, "manifest.json".into()))).await.is_err());
        assert!(preview_file(State(app.clone()), Path(("0".repeat(32), "panel.js".into()))).await.is_err());
    }
}
