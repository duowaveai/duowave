//! OpenAI's `POST /responses` with `stream: true`: what the ChatGPT plan takes through «Sign in with ChatGPT»
//! (src/oauth, docs/plans/provider-sign-in.md). The plan's rules: `store: false`, so every request carries the whole
//! conversation in `input` (no `previous_response_id`); no temperature and no output limit; the instructions in
//! `instructions` (a system message is refused); functions grouped in a namespace, and a call comes back — and goes
//! back — with it. The plan's limit can fail a response after it began (`response.failed`): the station says so in
//! words, and the provider's card shows it.

use anyhow::{anyhow, bail};
use futures::StreamExt;
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::describe_http_error as describe;
use super::llm_client::{
    describe_models, AssistantTurn, LlmClient, LlmEvent, LlmMessage, LlmRequest, LlmSettings, LlmStream, LlmUsage,
    SpokenFailure, ToolCall, ToolChoice,
};
use super::openai_chat::{aborted, head, parse_arguments, request_headers, starts_with_error, OPENAI_BASE_URL};
use super::sse::{Lines, SseDecoder};
use crate::dialog::generator::{generate, Yielder};
use crate::utils::http::client;

/// The namespace our functions travel in: the plan takes functions only grouped.
pub const NAMESPACE: &str = "duowave";

/// Said when the plan's limit is reached.
pub const LIMIT_PHRASE: &str = "Лимит подписки ChatGPT исчерпан. Выберите в пульте другую модель или подождите.";
/// Said when ChatGPT does not take the sign-in any more.
pub const SIGN_IN_PHRASE: &str = "ChatGPT не принял вход. Войдите снова в пульте, в «Мозге».";

pub struct OpenAiResponsesLlm {
    settings: LlmSettings,
    base_url: String,
    description: String,
}

impl OpenAiResponsesLlm {
    pub fn new(settings: LlmSettings) -> OpenAiResponsesLlm {
        let base_url = settings.base_url.as_deref().unwrap_or(OPENAI_BASE_URL).trim_end_matches('/').to_string();
        let description = format!("{} via {base_url}/responses", describe_models(&settings.models));
        OpenAiResponsesLlm { settings, base_url, description }
    }
}

impl LlmClient for OpenAiResponsesLlm {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> bool {
        true
    }

    fn stream(&self, request: LlmRequest, cancel: CancellationToken) -> LlmStream {
        let settings = self.settings.clone();
        let url = format!("{}/responses", self.base_url);
        generate(move |y| async move {
            if let Err(error) = run(&settings, &url, &request, &cancel, &y).await {
                y.send(Err(error)).await;
            }
        })
    }
}

/// A turn as input items: the user's text, the model's text and calls (with their namespace), the calls' results.
fn to_input(message: &LlmMessage) -> Vec<Value> {
    match message {
        LlmMessage::User(text) => vec![json!({"role": "user", "content": text})],
        LlmMessage::Assistant(turn) => {
            let mut items = Vec::new();
            if !turn.content.is_empty() {
                items.push(json!({"role": "assistant", "content": turn.content}));
            }
            for call in &turn.tool_calls {
                let arguments = serde_json::to_string(&call.input).unwrap_or_else(|_| "{}".into());
                items.push(json!({
                    "type": "function_call",
                    "call_id": call.id,
                    "name": call.name,
                    "namespace": NAMESPACE,
                    "arguments": arguments,
                }));
            }
            items
        }
        LlmMessage::Tool(results) => results
            .iter()
            .map(|result| {
                // The format has no error flag: the text says it.
                let output = if result.is_error && !starts_with_error(&result.content) {
                    format!("error: {}", result.content)
                } else {
                    result.content.clone()
                };
                json!({"type": "function_call_output", "call_id": result.id, "output": output})
            })
            .collect(),
    }
}

/// The request's body by the plan's rules.
fn body_of(settings: &LlmSettings, request: &LlmRequest) -> Value {
    let instructions = match request.system_tail.as_deref().filter(|tail| !tail.is_empty()) {
        Some(tail) => format!("{}\n\n{tail}", request.system),
        None => request.system.clone(),
    };
    let model = request.model.clone().filter(|model| !model.is_empty()).unwrap_or_else(|| settings.model.clone());
    let mut body = Map::new();
    body.insert("model".into(), json!(model));
    body.insert("instructions".into(), json!(instructions));
    body.insert("input".into(), Value::Array(request.messages.iter().flat_map(to_input).collect()));
    if !request.tools.is_empty() {
        let functions: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                json!({"type": "function", "name": tool.name, "description": tool.description, "parameters": tool.parameters})
            })
            .collect();
        body.insert(
            "tools".into(),
            json!([{
                "type": "namespace",
                "name": NAMESPACE,
                "description": "DuoWave: the station's commands, lookups and the connected services.",
                "tools": functions,
            }]),
        );
        if request.tool_choice == ToolChoice::None {
            body.insert("tool_choice".into(), json!("none"));
        }
    }
    body.insert("store".into(), json!(false));
    body.insert("stream".into(), json!(true));
    Value::Object(body)
}

/// A failure of the service as the owner can act on it: the plan's limit and a sign-in it does not take are said in
/// words; the rest is a plain error with the code (the service's own text, cut, goes to the log only).
fn failure(settings: &LlmSettings, status: Option<u16>, error: &Value) -> anyhow::Error {
    let code = error.get("code").and_then(Value::as_str).filter(|code| !code.is_empty()).unwrap_or("unknown_error");
    let message = error.get("message").and_then(Value::as_str).unwrap_or_default();
    let status = status.map(|status| format!("HTTP {status}: ")).unwrap_or_default();
    let detail = if message.is_empty() {
        format!("ChatGPT: {status}{code}")
    } else {
        format!("ChatGPT: {status}{code}: {}", head(message, 200))
    };
    match code {
        "subscription_sharing_usage_limit_exceeded" => {
            if let Some(access) = &settings.access {
                access.0.limited();
            }
            anyhow::Error::new(SpokenFailure { phrase: LIMIT_PHRASE.into(), detail })
        }
        "subscription_sharing_invalid_user" => {
            anyhow::Error::new(SpokenFailure { phrase: SIGN_IN_PHRASE.into(), detail })
        }
        _ => anyhow!(detail),
    }
}

/// The error of an answer that is not a stream: `{"error": {...}}`, or `{"detail": "..."}` before one opens.
fn refused(settings: &LlmSettings, status: u16, text: &str) -> anyhow::Error {
    let body: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let error = match body.get("error") {
        Some(error @ Value::Object(_)) => error.clone(),
        _ => {
            json!({"code": format!("http_{status}"), "message": body.get("detail").and_then(Value::as_str).unwrap_or(text)})
        }
    };
    if status == 401 && error.get("code").and_then(Value::as_str).is_none_or(|code| code.starts_with("http_")) {
        return anyhow::Error::new(SpokenFailure {
            phrase: SIGN_IN_PHRASE.into(),
            detail: format!("ChatGPT: HTTP 401: {}", head(text, 200)),
        });
    }
    failure(settings, Some(status), &error)
}

fn usage_of(usage: &Value) -> Option<LlmUsage> {
    let count = |name: &str| usage.get(name).and_then(Value::as_u64);
    Some(LlmUsage {
        input: count("input_tokens")?,
        output: count("output_tokens").unwrap_or(0),
        cache_read: usage
            .get("input_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_u64),
        cache_write: None,
    })
}

async fn run(
    settings: &LlmSettings,
    url: &str,
    request: &LlmRequest,
    cancel: &CancellationToken,
    y: &Yielder<anyhow::Result<LlmEvent>>,
) -> anyhow::Result<()> {
    let body = body_of(settings, request);
    let headers = tokio::select! {
        _ = cancel.cancelled() => return Err(aborted()),
        headers = request_headers(settings) => headers?,
    };
    let send = client().post(url).headers(headers).body(body.to_string()).send();
    let response = tokio::select! {
        _ = cancel.cancelled() => return Err(aborted()),
        response = send => response.map_err(|error| anyhow!("cannot reach {url}: {}", describe(&error)))?,
    };
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let text = tokio::select! {
            _ = cancel.cancelled() => return Err(aborted()),
            text = response.text() => text.unwrap_or_default(),
        };
        return Err(refused(settings, status, &text));
    }

    // Server-sent events, each a JSON object with its `type`; the answer is whole at `response.completed`.
    let mut chunks = response.bytes_stream();
    let mut lines = Lines::default();
    let mut decoder = SseDecoder::default();
    let mut text = String::new();
    let mut calls: Vec<ToolCall> = Vec::new();
    let mut usage = None;
    let mut ended = false;
    let mut done = false;
    while !done && !ended {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(aborted()),
            chunk = chunks.next() => chunk,
        };
        let got: Vec<String> = match chunk {
            Some(Ok(bytes)) => lines.push(&bytes),
            Some(Err(error)) => bail!("the answer broke off: {}", describe(&error)),
            None => {
                ended = true;
                // A last event without its blank line still counts.
                lines.finish().into_iter().chain([String::new()]).collect()
            }
        };
        for line in got {
            let Some(event) = decoder.line(&line) else { continue };
            if event.data.is_empty() || event.data == "[DONE]" {
                continue;
            }
            let data: Value =
                serde_json::from_str(&event.data).map_err(|error| anyhow!("a streamed event is not JSON: {error}"))?;
            let kind = data.get("type").and_then(Value::as_str).or(event.event.as_deref()).unwrap_or_default();
            match kind {
                "response.output_text.delta" => {
                    if let Some(delta) = data.get("delta").and_then(Value::as_str).filter(|delta| !delta.is_empty()) {
                        text.push_str(delta);
                        y.send(Ok(LlmEvent::Text(delta.to_string()))).await;
                    }
                }
                "response.output_item.done" => {
                    let item = &data["item"];
                    if item.get("type").and_then(Value::as_str) == Some("function_call") {
                        let field = |name: &str| item.get(name).and_then(Value::as_str).unwrap_or_default();
                        let id = Some(field("call_id")).filter(|id| !id.is_empty()).unwrap_or(field("id"));
                        let (input, error) = parse_arguments(field("arguments"));
                        calls.push(ToolCall { id: id.to_string(), name: field("name").to_string(), input, error });
                    }
                }
                "response.completed" => {
                    usage = data.get("response").and_then(|response| response.get("usage")).and_then(usage_of);
                    done = true;
                }
                "response.incomplete" => {
                    let reason = data.pointer("/response/incomplete_details/reason").and_then(Value::as_str);
                    tracing::warn!(target: "dialog", "ChatGPT ended the answer early: {}", reason.unwrap_or("no reason"));
                    done = true;
                }
                "response.failed" => {
                    let error = data.pointer("/response/error").cloned().unwrap_or(Value::Null);
                    return Err(failure(settings, None, &error));
                }
                "error" => {
                    let error = match data.get("error") {
                        Some(error @ Value::Object(_)) => error.clone(),
                        _ => data.clone(),
                    };
                    return Err(failure(settings, None, &error));
                }
                _ => {}
            }
        }
    }
    if !done {
        bail!("the answer broke off before it was complete");
    }
    if let Some(access) = &settings.access {
        access.0.ok();
    }
    for call in &calls {
        y.send(Ok(LlmEvent::Tool(call.clone()))).await;
    }
    y.send(Ok(LlmEvent::Turn(AssistantTurn { content: text, tool_calls: calls, native: None, usage }))).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::extract::State;
    use axum::http::{HeaderMap as Headers, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::post;
    use futures::future::BoxFuture;
    use reqwest::header::CONTENT_TYPE;

    use super::*;
    use crate::config::{DialogProvider, ModelPair};
    use crate::dialog::llm::{Access, AccessSource, LlmApi, LlmTool, ToolResult};

    #[derive(Clone, Default)]
    struct Stub {
        requests: Arc<Mutex<Vec<(Value, Headers)>>>,
        /// What the next answer streams, or (status, body) for a refusal.
        events: Arc<Mutex<Vec<Value>>>,
        refusal: Arc<Mutex<Option<(u16, String)>>>,
    }

    async fn responses(State(stub): State<Stub>, headers: Headers, body: String) -> axum::response::Response {
        stub.requests.lock().unwrap().push((serde_json::from_str(&body).unwrap(), headers));
        if let Some((status, body)) = stub.refusal.lock().unwrap().clone() {
            return (StatusCode::from_u16(status).unwrap(), body).into_response();
        }
        let mut out = String::new();
        for event in stub.events.lock().unwrap().iter() {
            out.push_str(&format!("event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap()));
        }
        ([(CONTENT_TYPE, "text/event-stream")], out).into_response()
    }

    async fn stub() -> (Stub, String) {
        let stub = Stub::default();
        let app = axum::Router::new().route("/v1/responses", post(responses)).with_state(stub.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        (stub, url)
    }

    /// A sign-in's access: a fixed token, and what the client told it.
    #[derive(Default)]
    struct Signed {
        asked: AtomicUsize,
        limited: AtomicUsize,
        ok: AtomicUsize,
    }

    impl AccessSource for Signed {
        fn bearer(&self) -> BoxFuture<'_, anyhow::Result<String>> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok("plan-access-token".to_string()) })
        }
        fn ok(&self) {
            self.ok.fetch_add(1, Ordering::SeqCst);
        }
        fn limited(&self) {
            self.limited.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn settings(url: &str, access: Arc<Signed>) -> LlmSettings {
        LlmSettings {
            provider: DialogProvider::OpenAi,
            model: "gpt-plan".into(),
            models: ModelPair { fast: "gpt-plan".into(), smart: "gpt-plan".into() },
            base_url: Some(url.into()),
            api_key: None,
            max_tokens: 300,
            effort: "low".into(),
            explicit: true,
            headers: BTreeMap::new(),
            tokens_field: Default::default(),
            access: Some(Access(access)),
            api: LlmApi::Responses,
        }
    }

    fn tool() -> LlmTool {
        LlmTool {
            name: "set_volume".into(),
            description: "Громкость".into(),
            parameters: json!({"type": "object", "properties": {"level": {"type": "integer"}}}),
        }
    }

    async fn events(llm: &OpenAiResponsesLlm, request: LlmRequest) -> anyhow::Result<Vec<LlmEvent>> {
        llm.stream(request, CancellationToken::new()).collect::<Vec<_>>().await.into_iter().collect()
    }

    #[tokio::test]
    async fn asks_by_the_plans_rules_and_streams_the_text_then_the_namespaced_call() {
        let (stub, url) = stub().await;
        *stub.events.lock().unwrap() = vec![
            json!({"type": "response.created", "response": {"id": "resp_1"}}),
            json!({"type": "response.output_text.delta", "delta": "Сейчас "}),
            json!({"type": "response.output_text.delta", "delta": "сделаю."}),
            json!({"type": "response.output_item.done", "item": {"type": "function_call", "id": "fc_1", "call_id": "call_7", "namespace": "duowave", "name": "set_volume", "arguments": "{\"level\":5}"}}),
            json!({"type": "response.completed", "response": {"usage": {"input_tokens": 120, "output_tokens": 9, "input_tokens_details": {"cached_tokens": 100}}}}),
        ];
        let access = Arc::new(Signed::default());
        let llm = OpenAiResponsesLlm::new(settings(&url, access.clone()));
        assert!(llm.description().ends_with("/v1/responses"), "{}", llm.description());
        let earlier = AssistantTurn {
            content: "Смотрю.".into(),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "set_volume".into(),
                input: Map::new(),
                error: None,
            }],
            native: None,
            usage: None,
        };
        let request = LlmRequest {
            system: "Ты Алиса.".into(),
            system_tail: Some("Сейчас 10:00.".into()),
            messages: vec![
                LlmMessage::User("Громче".into()),
                LlmMessage::Assistant(earlier),
                LlmMessage::Tool(vec![ToolResult {
                    id: "call_1".into(),
                    content: "нет колонки".into(),
                    is_error: true,
                }]),
            ],
            model: Some("gpt-plan-mini".into()),
            tools: vec![tool()],
            tool_choice: ToolChoice::Auto,
        };
        let got = events(&llm, request).await.unwrap();
        assert_eq!(got[0], LlmEvent::Text("Сейчас ".into()));
        assert_eq!(got[1], LlmEvent::Text("сделаю.".into()));
        let mut level = Map::new();
        level.insert("level".into(), json!(5));
        let call = ToolCall { id: "call_7".into(), name: "set_volume".into(), input: level, error: None };
        assert_eq!(got[2], LlmEvent::Tool(call.clone()));
        let LlmEvent::Turn(turn) = &got[3] else { panic!("{got:?}") };
        assert_eq!((turn.content.as_str(), turn.tool_calls.clone()), ("Сейчас сделаю.", vec![call]));
        assert_eq!(turn.usage, Some(LlmUsage { input: 120, output: 9, cache_read: Some(100), cache_write: None }));
        assert_eq!((access.asked.load(Ordering::SeqCst), access.ok.load(Ordering::SeqCst)), (1, 1));

        let (body, headers) = stub.requests.lock().unwrap()[0].clone();
        assert_eq!(headers["authorization"], "Bearer plan-access-token");
        assert_eq!(body["model"], json!("gpt-plan-mini"));
        assert_eq!(body["instructions"], json!("Ты Алиса.\n\nСейчас 10:00."));
        assert_eq!((body["store"].clone(), body["stream"].clone()), (json!(false), json!(true)));
        for absent in ["max_output_tokens", "temperature", "previous_response_id", "tool_choice"] {
            assert!(body.get(absent).is_none(), "{absent}: {body}");
        }
        assert_eq!(
            body["input"],
            json!([
                {"role": "user", "content": "Громче"},
                {"role": "assistant", "content": "Смотрю."},
                {"type": "function_call", "call_id": "call_1", "name": "set_volume", "namespace": "duowave", "arguments": "{}"},
                {"type": "function_call_output", "call_id": "call_1", "output": "error: нет колонки"},
            ])
        );
        assert_eq!(body["tools"][0]["type"], json!("namespace"));
        assert_eq!(body["tools"][0]["name"], json!("duowave"));
        assert_eq!(body["tools"][0]["tools"][0]["name"], json!("set_volume"));
        assert_eq!(body["tools"][0]["tools"][0]["type"], json!("function"));
    }

    #[tokio::test]
    async fn the_plans_limit_is_said_in_words_and_noted_for_the_card() {
        let (stub, url) = stub().await;
        *stub.events.lock().unwrap() = vec![
            json!({"type": "response.created", "response": {"id": "resp_2"}}),
            json!({"type": "response.failed", "response": {"error": {"code": "subscription_sharing_usage_limit_exceeded", "message": "Usage limit reached"}}}),
        ];
        let access = Arc::new(Signed::default());
        let llm = OpenAiResponsesLlm::new(settings(&url, access.clone()));
        let request = LlmRequest { messages: vec![LlmMessage::User("Привет".into())], ..LlmRequest::default() };
        let error = events(&llm, request.clone()).await.unwrap_err();
        let spoken = error.downcast_ref::<SpokenFailure>().expect("said in words");
        assert_eq!(spoken.phrase, LIMIT_PHRASE);
        assert!(spoken.detail.contains("subscription_sharing_usage_limit_exceeded"), "{}", spoken.detail);
        assert_eq!((access.limited.load(Ordering::SeqCst), access.ok.load(Ordering::SeqCst)), (1, 0));

        // Before a stream opens: the same code in the body of a 429, and a 401 that asks for a new sign-in.
        *stub.refusal.lock().unwrap() =
            Some((429, json!({"error": {"code": "subscription_sharing_usage_limit_exceeded"}}).to_string()));
        let error = events(&llm, request.clone()).await.unwrap_err();
        assert_eq!(error.downcast_ref::<SpokenFailure>().map(|f| f.phrase.as_str()), Some(LIMIT_PHRASE));
        *stub.refusal.lock().unwrap() = Some((401, r#"{"detail":"Unauthorized"}"#.into()));
        let error = events(&llm, request.clone()).await.unwrap_err();
        assert_eq!(error.downcast_ref::<SpokenFailure>().map(|f| f.phrase.as_str()), Some(SIGN_IN_PHRASE));
        *stub.refusal.lock().unwrap() = Some((
            400,
            json!({"error": {"code": "subscription_sharing_unsupported_capability", "param": "tools"}}).to_string(),
        ));
        let error = events(&llm, request).await.unwrap_err();
        assert!(error.downcast_ref::<SpokenFailure>().is_none());
        assert!(error.to_string().contains("subscription_sharing_unsupported_capability"), "{error}");
    }

    #[tokio::test]
    async fn an_answer_cut_before_completed_fails_and_the_last_round_asks_for_words() {
        let (stub, url) = stub().await;
        *stub.events.lock().unwrap() = vec![json!({"type": "response.output_text.delta", "delta": "Обры"})];
        let llm = OpenAiResponsesLlm::new(settings(&url, Arc::new(Signed::default())));
        let request = LlmRequest {
            messages: vec![LlmMessage::User("Привет".into())],
            tools: vec![tool()],
            tool_choice: ToolChoice::None,
            ..LlmRequest::default()
        };
        let error = events(&llm, request).await.unwrap_err();
        assert!(error.to_string().contains("broke off"), "{error}");
        assert_eq!(stub.requests.lock().unwrap()[0].0["tool_choice"], json!("none"));
    }
}
