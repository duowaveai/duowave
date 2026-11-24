//! The tests of `mcp/ask_controller.rs` that need the whole gateway (`App`): its crate does not have it, the program
//! does.

use std::sync::{Arc, Mutex};

use axum::http::{header, StatusCode};
use axum::response::Response;
use futures::stream::BoxStream;
use futures::StreamExt;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::config::{Config, Env};
use crate::core::identity::principal::user_principal;
use crate::core::identity::Role;
use crate::dialog::llm::ToolCall;
use crate::mcp::ask_controller::*;
use crate::mcp::fixtures;
use crate::mcp::mcp_service::tests::{input, until, READY};
use crate::mcp::mcp_service::{ServerState, ToolChange};
use crate::platform::http::{Body, Caller, Uses};

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
    let response = ask(
        Uses::of(&app),
        Uses::of(&app),
        Uses::of(&app),
        member(&app),
        question(json!({ "question": "Ты здесь?", "fresh": true })),
    )
    .await;
    let events = events(response.unwrap()).await;
    let types: Vec<&str> = events.iter().map(|event| event["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["start", "text", "done"]);
    // No connectors here: the tools are the memory's own — remember, forget, recall (src/memory/tools.rs) — and
    // the works' — work_start, work_add, work_do (src/talk/work_tools.rs).
    assert_eq!(events[0], json!({ "type": "start", "model": "home-small", "provider": "Дома", "tools": 6 }));
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
        ask(
            Uses::of(&app),
            Uses::of(&app),
            Uses::of(&app),
            member(&app),
            question(json!({ "question": "Повтори «да»" })),
        )
        .await
        .unwrap(),
    )
    .await;
    let types: Vec<&str> = events.iter().map(|event| event["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["start", "tool", "result", "text", "text", "done"]);
    assert_eq!(events[0]["tools"], json!(7), "the fixture's echo, the memory's three and the works' three");
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
    let empty =
        ask(Uses::of(&app), Uses::of(&app), Uses::of(&app), member(&app), question(json!({ "question": "  " })))
            .await
            .err()
            .unwrap();
    assert_eq!((empty.status, empty.message.as_str()), (StatusCode::BAD_REQUEST, "Напишите вопрос."));
    let none =
        ask(Uses::of(&app), Uses::of(&app), Uses::of(&app), member(&app), question(json!({ "question": "Ты здесь?" })))
            .await
            .err()
            .unwrap();
    assert_eq!(none.status, StatusCode::CONFLICT);
    assert_eq!(none.message, "Модель не выбрана: добавьте провайдера в «Персонаж» → «Мышление».");
    let guest = Caller::local(app.clone(), Some(user_principal("g", "Гость", Role::Guest, None)));
    let refused =
        ask(Uses::of(&app), Uses::of(&app), Uses::of(&app), guest, question(json!({ "question": "Ты здесь?" })))
            .await
            .err()
            .unwrap();
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "the servers' tools are for the household");
}
