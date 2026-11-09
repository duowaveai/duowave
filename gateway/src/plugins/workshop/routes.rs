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

use super::agent::{self, ChatStep};
use super::{CheckReport, DraftView, InstallPlan, InstallReport, PhraseReport, RunReport};
use crate::app::App;
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller};
use crate::plugins::routes::admin;
use crate::settings::definitions::{BrainPick, WORKSHOP_AGENT};

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
    admin(&caller)?;
    Ok(ok(app.workshop.write(&id, &path, &body.content)?))
}

async fn delete_file(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, path)): Path<(String, String)>,
) -> ApiResult<DraftView> {
    admin(&caller)?;
    Ok(ok(app.workshop.delete_file(&id, &path)?))
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
}

async fn run(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<RunBody>,
) -> ApiResult<RunReport> {
    admin(&caller)?;
    Ok(ok(app.workshop.run(&id, &body.function, body.input.unwrap_or_else(|| json!({}))).await?))
}

#[derive(Debug, Deserialize)]
pub struct PhraseBody {
    text: String,
}

async fn phrase(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<PhraseBody>,
) -> ApiResult<PhraseReport> {
    admin(&caller)?;
    Ok(ok(app.workshop.try_phrase(&id, &body.text).await?))
}

async fn journal(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<Value> {
    admin(&caller)?;
    Ok(ok(json!({ "lines": app.workshop.journal(&id) })))
}

#[derive(Debug, Default, Deserialize)]
pub struct InstallBody {
    /// The admin's «ок» to the first install or new rights, given in the panel with them on the screen.
    #[serde(default)]
    approve: bool,
}

async fn install(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<InstallBody>,
) -> ApiResult<InstallReport> {
    let actor = admin(&caller)?;
    let report = app.workshop.install(&id, body.approve, actor).await?;
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
                return Err(ApiError::field("model", "выберите модель"));
            }
            if !app.brain.view().providers.iter().any(|provider| provider.id == pick.provider) {
                return Err(ApiError::field("provider", "такого провайдера нет в «Мозге»"));
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
        return Err(ApiError::bad_request("Напишите, что сделать."));
    }
    if !app.dialog.enabled() {
        return Err(ApiError::conflict("Модель не выбрана: добавьте провайдера на вкладке «Мозг»."));
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
    let cancel = CancellationToken::new();
    let answering = cancel.clone();
    let mut phrases = agent::chat(&app.dialog, app.workshop.clone(), &key, &id, &message, told, answering.clone());
    let secrets = app.secrets.clone();
    tokio::spawn(async move {
        loop {
            let next = tokio::select! {
                _ = answering.cancelled() => return,
                next = phrases.next() => next,
            };
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
    let guard = cancel.drop_guard();
    let stream = UnboundedReceiverStream::new(receiver).map(move |event| {
        let _ = &guard;
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
            Body(PhraseBody { text: "Скажи привет Маша".into() }),
        )
        .await
        .unwrap()
        .1;
        assert_eq!(tried.speech.as_deref(), Some("Привет, маша! Это 1-й раз."));

        // The first install waits for the admin's «ок»; then it is on.
        let asked = install(State(app.clone()), admin(), Path("hello".into()), Body(InstallBody { approve: false }))
            .await
            .unwrap()
            .1;
        assert!(!asked.installed && asked.needs_approval.as_ref().is_some_and(|approval| approval.first));
        let done = install(State(app.clone()), admin(), Path("hello".into()), Body(InstallBody { approve: true }))
            .await
            .unwrap()
            .1;
        assert!(done.installed, "{}", done.message);
        assert_eq!(done.version.as_deref(), Some("0.1.0"));
        assert!(app.plugins.is_active("hello"));
        // The same version again is the next patch; nothing new asked of the admin.
        let again = install(State(app.clone()), admin(), Path("hello".into()), Body(InstallBody { approve: false }))
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

        // The plan of the first install: every right is new, nothing installed yet.
        assert!(plan(State(app.clone()), member(), Path("hello".into())).await.is_err());
        let first = plan(State(app.clone()), admin(), Path("hello".into())).await.unwrap().1;
        assert!(first.first && first.from.is_none(), "{first:?}");
        assert_eq!(first.to, "0.1.0");
        assert!(!first.added.is_empty() && first.removed.is_empty() && first.kept.is_empty(), "{first:?}");
        assert!(!first.phrases.is_empty(), "the template's phrases: {first:?}");
        let done = install(State(app.clone()), admin(), Path("hello".into()), Body(InstallBody { approve: true }))
            .await
            .unwrap()
            .1;
        assert!(done.installed, "{}", done.message);
        // The plan of the next one: the same rights kept, the next patch, nothing installed by asking.
        let next = plan(State(app.clone()), admin(), Path("hello".into())).await.unwrap().1;
        assert_eq!((next.first, next.from.as_deref(), next.to.as_str()), (false, Some("0.1.0"), "0.1.1"));
        assert!(next.added.is_empty() && next.removed.is_empty() && !next.kept.is_empty(), "{next:?}");
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
}
