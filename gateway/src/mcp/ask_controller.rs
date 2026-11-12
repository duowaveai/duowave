//! «Спросить мозг»: the panel's test chat. The same model and the same MCP tools as the station's conversation (the
//! servers this person may use, not only those open to the voice), without the station: a question in text, the
//! answer as it is written, every tool call with what it returned. Its own history per person.
//!
//! What the chat needs of the dialog is `AskDialog`: the gateway links its dialog to the MCP service at the build
//! (`McpService::attach_dialog`, src/links). Without one no model answers here, as with no model chosen.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use futures::stream::BoxStream;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::CancellationToken;

use super::mcp_import::{present, Json};
use super::mcp_service::{Cancelled, McpOffer, OfferOptions, ToolOutcome};
use crate::app::App;
use crate::core::connectors::ContributedOffer;
use crate::core::identity::permissions::MCP_USE;
use crate::core::identity::Principal;
use crate::dialog::llm::{LlmTool, ToolCall};
use crate::platform::http::{ApiError, Body, Caller};

/// How much of a tool's answer the chat shows: enough to see what the model read.
const SHOWN: usize = 600;
/// Rounds of tool calls before the model must answer in words.
pub const ROUNDS: usize = 3;

/// What the check chat needs of the dialog (src/dialog).
pub trait AskDialog: Send + Sync {
    /// Whether a model answers (DIALOG_PROVIDER, or a provider chosen in the panel).
    fn enabled(&self) -> bool;
    /// The fast profile's model and its provider's title, for the chat's first event.
    fn fast_model(&self) -> AskModel;
    /// Forgets a conversation's history.
    fn forget(&self, key: &str);
    /// The answer in phrases as the fast profile writes them, with the toolbox's tools when there are any;
    /// `cancel` stops it (the panel closed the chat).
    fn answer(
        &self,
        key: &str,
        question: &str,
        toolbox: Option<AskToolbox>,
        cancel: CancellationToken,
    ) -> BoxStream<'static, anyhow::Result<String>>;
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AskModel {
    pub model: String,
    pub provider: String,
}

/// One event of the test chat's stream.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum AskEvent {
    Start {
        model: String,
        provider: String,
        tools: usize,
    },
    Text {
        text: String,
        ms: u64,
    },
    Tool {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        server: Option<String>,
        input: Map<String, Value>,
    },
    Result {
        name: String,
        content: String,
        is_error: bool,
        outside: bool,
        ms: u64,
    },
    Done {
        ms: u64,
    },
    Error {
        message: String,
    },
}

/// The chat's tools as the dialog's toolbox takes them: the MCP offer and the connectors' tools (the plugins'),
/// every call and its result told to the chat.
pub struct AskToolbox {
    offer: Option<Arc<McpOffer>>,
    contributed: Option<Arc<ContributedOffer>>,
    principal: Principal,
    tools: Vec<LlmTool>,
    events: mpsc::UnboundedSender<AskEvent>,
    started: Instant,
}

impl AskToolbox {
    /// None when there is no tool to offer.
    fn new(
        offer: Option<Arc<McpOffer>>,
        contributions: Vec<crate::core::connectors::ToolContribution>,
        principal: Principal,
        events: mpsc::UnboundedSender<AskEvent>,
        started: Instant,
    ) -> Option<AskToolbox> {
        let mut tools: Vec<LlmTool> = offer.as_ref().map(|offer| offer.tools.clone()).unwrap_or_default();
        let contributed = ContributedOffer::new(contributions, &tools).map(Arc::new);
        tools.extend(contributed.iter().flat_map(|contributed| contributed.tools.iter().cloned()));
        (!tools.is_empty()).then_some(AskToolbox { offer, contributed, principal, tools, events, started })
    }

    pub fn tools(&self) -> &[LlmTool] {
        &self.tools
    }

    pub fn rounds(&self) -> usize {
        ROUNDS
    }

    /// How to use these tools: for the stable part of the instructions.
    pub fn instructions(&self) -> &str {
        self.offer.as_ref().map(|offer| offer.instructions.as_str()).unwrap_or_default()
    }

    /// The tools still offered after a text from outside the house: those that change nothing.
    pub fn safe_after_taint(&self, tools: &[LlmTool]) -> Vec<LlmTool> {
        tools
            .iter()
            .filter(|tool| {
                self.offer.as_ref().is_some_and(|offer| offer.safe(&tool.name))
                    || self.contributed.as_ref().is_some_and(|contributed| contributed.safe(&tool.name))
            })
            .cloned()
            .collect()
    }

    pub async fn run(&self, call: &ToolCall, cancel: &CancellationToken) -> Result<ToolOutcome, Cancelled> {
        let contributed = self.contributed.as_ref().filter(|contributed| contributed.owns(&call.name));
        let server = match contributed {
            Some(_) => Some("Приложения".to_string()),
            None => self.offer.as_ref().and_then(|offer| offer.server_of(&call.name)).map(|server| server.title),
        };
        let _ = self.events.send(AskEvent::Tool { name: call.name.clone(), server, input: call.input.clone() });
        let outcome = match (contributed, &self.offer) {
            (Some(contributed), _) => contributed.run(call, &self.principal, cancel.clone()).await,
            (None, Some(offer)) => offer.run(call, cancel).await?,
            (None, None) => ToolOutcome::error(format!("error: {} is not available now", call.name)),
        };
        let _ = self.events.send(AskEvent::Result {
            name: call.name.clone(),
            content: outcome.content.chars().take(SHOWN).collect(),
            is_error: outcome.is_error,
            outside: outcome.tainted,
            ms: self.started.elapsed().as_millis() as u64,
        });
        Ok(outcome)
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct AskBody {
    #[serde(default, deserialize_with = "present")]
    question: Option<Json>,
    #[serde(default, deserialize_with = "present")]
    fresh: Option<Json>,
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/brain/ask", post(ask))
}

async fn ask(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<AskBody>) -> Result<Response, ApiError> {
    let principal = caller.permitted(MCP_USE)?.clone();
    let question: String = body
        .question
        .as_ref()
        .and_then(Json::as_str)
        .map(|q| q.trim().chars().take(1000).collect())
        .unwrap_or_default();
    if question.is_empty() {
        return Err(ApiError::bad_request("Напишите вопрос."));
    }
    let dialog = app
        .mcp
        .dialog()
        .filter(|dialog| dialog.enabled())
        .ok_or_else(|| ApiError::conflict("Модель не выбрана: добавьте провайдера в «Персонаж» → «Мозг»."))?;
    let key = format!("panel:{}", principal.id);
    if body.fresh.as_ref().and_then(Json::as_bool) == Some(true) {
        dialog.forget(&key);
    }
    let offer = app.mcp.offer(&principal, OfferOptions { voice: false, capability_allowed: None }).map(Arc::new);
    let contributions = app.connectors.tools(&principal);
    let secrets = app.secrets.clone();
    let started = Instant::now();
    let ms = move || started.elapsed().as_millis() as u64;
    let (events, receiver) = mpsc::unbounded_channel();
    let fast = dialog.fast_model();
    let toolbox = AskToolbox::new(offer, contributions, principal.clone(), events.clone(), started);
    let _ = events.send(AskEvent::Start {
        model: fast.model,
        provider: fast.provider,
        tools: toolbox.as_ref().map(|toolbox| toolbox.tools().len()).unwrap_or(0),
    });
    // The panel closing the chat drops the body, and that stops the answer.
    let cancel = CancellationToken::new();
    let answering = cancel.clone();
    tokio::spawn(async move {
        let mut phrases = dialog.answer(&key, &question, toolbox, answering.clone());
        loop {
            let next = tokio::select! {
                _ = answering.cancelled() => return,
                next = phrases.next() => next,
            };
            match next {
                Some(Ok(text)) => {
                    let _ = events.send(AskEvent::Text { text, ms: ms() });
                }
                Some(Err(error)) => {
                    if !answering.is_cancelled() {
                        let message = secrets.redact(&format!("{error:#}")).chars().take(300).collect();
                        let _ = events.send(AskEvent::Error { message });
                    }
                    return;
                }
                None => {
                    let _ = events.send(AskEvent::Done { ms: ms() });
                    return;
                }
            }
        }
    });
    let guard = cancel.drop_guard();
    let body = UnboundedReceiverStream::new(receiver).map(move |event| {
        let _ = &guard;
        let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
        Ok::<_, Infallible>(format!("data: {data}\n\n"))
    });
    let mut response = (StatusCode::OK, axum::body::Body::from_stream(body)).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, no-transform"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use http_body_util::BodyExt;
    use serde_json::json;

    use super::*;
    use crate::config::{Config, Env};
    use crate::core::identity::principal::user_principal;
    use crate::core::identity::Role;
    use crate::mcp::fixtures;
    use crate::mcp::mcp_service::tests::{input, until, READY};
    use crate::mcp::mcp_service::{ServerState, ToolChange};

    /// The dialog as the chat sees it: a model that says its phrases, after one tool call when it has one.
    struct Model {
        phrases: Vec<String>,
        call: Option<ToolCall>,
        forgotten: Mutex<Vec<String>>,
    }

    impl AskDialog for Model {
        fn enabled(&self) -> bool {
            true
        }

        fn fast_model(&self) -> AskModel {
            AskModel { model: "home-small".into(), provider: "Дома".into() }
        }

        fn forget(&self, key: &str) {
            self.forgotten.lock().unwrap().push(key.to_string());
        }

        fn answer(
            &self,
            _key: &str,
            _question: &str,
            toolbox: Option<AskToolbox>,
            cancel: CancellationToken,
        ) -> BoxStream<'static, anyhow::Result<String>> {
            let phrases = self.phrases.clone();
            let call = self.call.clone();
            futures::stream::once(async move {
                let mut said = Vec::new();
                if let (Some(toolbox), Some(call)) = (toolbox, call) {
                    assert_eq!(toolbox.rounds(), ROUNDS);
                    assert!(toolbox.instructions().starts_with("Подключения."));
                    // Echo and the memory's `recall` change nothing; `remember` and `forget` do.
                    assert_eq!(toolbox.safe_after_taint(toolbox.tools()).len(), 2, "echo and recall change nothing");
                    if let Ok(outcome) = toolbox.run(&call, &cancel).await {
                        said.push(Ok(format!("Сервер сказал: {}", outcome.content)));
                    }
                }
                said.extend(phrases.into_iter().map(Ok));
                futures::stream::iter(said)
            })
            .flatten()
            .boxed()
        }
    }

    fn app() -> (tempfile::TempDir, Arc<App>) {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::from_pairs([("STATE_DIR", dir.path().to_str().unwrap())]);
        let app = App::build(Config::for_tests(&env)).unwrap();
        (dir, app)
    }

    fn member(app: &Arc<App>) -> Caller {
        Caller::local(app.clone(), Some(user_principal("u2", "Аня", Role::Member, None)))
    }

    fn question(value: Value) -> Body<AskBody> {
        Body(serde_json::from_value(value).unwrap())
    }

    async fn events(response: Response) -> Vec<Value> {
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/event-stream; charset=utf-8");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8_lossy(&bytes)
            .split("\n\n")
            .filter_map(|chunk| chunk.strip_prefix("data: "))
            .map(|data| serde_json::from_str(data).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn answers_the_check_chat_as_a_stream_of_events() {
        let (_dir, app) = app();
        let model = Arc::new(Model { phrases: vec!["Да.".into()], call: None, forgotten: Mutex::new(Vec::new()) });
        app.mcp.attach_dialog(model.clone());
        let response =
            ask(State(app.clone()), member(&app), question(json!({ "question": "Ты здесь?", "fresh": true }))).await;
        let events = events(response.unwrap()).await;
        let types: Vec<&str> = events.iter().map(|event| event["type"].as_str().unwrap()).collect();
        assert_eq!(types, ["start", "text", "done"]);
        // No connectors here: the tools are the memory's own — remember, forget, recall (src/memory/tools.rs).
        assert_eq!(events[0], json!({ "type": "start", "model": "home-small", "provider": "Дома", "tools": 3 }));
        assert_eq!(events[1]["text"], json!("Да."));
        assert_eq!(model.forgotten.lock().unwrap().as_slice(), ["panel:u2"], "its own history per person");
    }

    #[tokio::test]
    async fn shows_every_tool_call_with_what_it_returned() {
        let (_dir, app) = app();
        let (router, _) = fixtures::legacy_http();
        let base = fixtures::spawn(router).await;
        let server = app
            .mcp
            .add(&input(json!({ "transport": "http", "title": "Fixture", "url": format!("{base}/mcp") })), None)
            .unwrap();
        let ready = || app.mcp.states().iter().any(|s| s.id == server.id && s.state == ServerState::Ready);
        until("the fixture is up", ready, READY).await;
        let on: ToolChange = serde_json::from_value(json!({ "on": true })).unwrap();
        app.mcp.set_tool(&server.id, "echo", &on, None).unwrap();
        let call = ToolCall {
            id: "c1".into(),
            name: "mcp_fixture__echo".into(),
            input: json!({ "text": "да" }).as_object().cloned().unwrap(),
            error: None,
        };
        app.mcp.attach_dialog(Arc::new(Model {
            phrases: vec!["Готово.".into()],
            call: Some(call),
            forgotten: Mutex::new(Vec::new()),
        }));
        let events = events(
            ask(State(app.clone()), member(&app), question(json!({ "question": "Повтори «да»" }))).await.unwrap(),
        )
        .await;
        let types: Vec<&str> = events.iter().map(|event| event["type"].as_str().unwrap()).collect();
        assert_eq!(types, ["start", "tool", "result", "text", "text", "done"]);
        assert_eq!(events[0]["tools"], json!(4), "the fixture's echo and the memory's three");
        assert_eq!(
            events[1],
            json!({ "type": "tool", "name": "mcp_fixture__echo", "server": "Fixture", "input": { "text": "да" } })
        );
        assert_eq!(events[2]["content"], json!("echo: да"));
        assert_eq!((events[2]["isError"].clone(), events[2]["outside"].clone()), (json!(false), json!(true)));
        assert_eq!(events[3]["text"], json!("Сервер сказал: echo: да"));
        app.shutdown().await;
    }

    #[tokio::test]
    async fn wants_a_question_and_a_model() {
        let (_dir, app) = app();
        let empty = ask(State(app.clone()), member(&app), question(json!({ "question": "  " }))).await.err().unwrap();
        assert_eq!((empty.status, empty.message.as_str()), (StatusCode::BAD_REQUEST, "Напишите вопрос."));
        let none =
            ask(State(app.clone()), member(&app), question(json!({ "question": "Ты здесь?" }))).await.err().unwrap();
        assert_eq!(none.status, StatusCode::CONFLICT);
        assert_eq!(none.message, "Модель не выбрана: добавьте провайдера в «Персонаж» → «Мозг».");
        let guest = Caller::local(app.clone(), Some(user_principal("g", "Гость", Role::Guest, None)));
        let refused = ask(State(app.clone()), guest, question(json!({ "question": "Ты здесь?" }))).await.err().unwrap();
        assert_eq!(refused.status, StatusCode::FORBIDDEN, "the servers' tools are for the household");
    }
}
