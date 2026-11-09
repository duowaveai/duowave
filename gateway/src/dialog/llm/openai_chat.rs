//! Any OpenAI-compatible `POST /chat/completions` with `stream: true`: OpenAI, OpenRouter, Groq, Ollama, LM Studio,
//! vLLM. No temperature is sent: some models accept only the default. Tools are functions; their calls stream in
//! pieces by index (or, from providers that send no index, by a new id) and come out once the stream ends.

use std::collections::BTreeMap;

use anyhow::{anyhow, bail};
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::describe_http_error as describe;
use super::llm_client::{
    describe_models, AssistantTurn, LlmClient, LlmEvent, LlmMessage, LlmRequest, LlmSettings, LlmStream, TokensField,
    ToolCall, ToolChoice,
};
use super::sse::Lines;
use crate::dialog::generator::{generate, Yielder};
use crate::utils::http::client;

/// Where an OpenAI-compatible model is asked when LLM_BASE_URL names no other service.
pub const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

/// OpenAI's own API wants max_completion_tokens (its reasoning models refuse max_tokens); the compatible ones
/// max_tokens.
pub fn tokens_field_for(base_url: &str) -> TokensField {
    match url::Url::parse(base_url) {
        Ok(url) if url.host_str() == Some("api.openai.com") && url.port().is_none() => TokensField::MaxCompletionTokens,
        _ => TokensField::MaxTokens,
    }
}

fn tokens_field_name(field: TokensField) -> &'static str {
    match field {
        TokensField::MaxTokens => "max_tokens",
        TokensField::MaxCompletionTokens => "max_completion_tokens",
    }
}

pub struct OpenAiChatLlm {
    settings: LlmSettings,
    base_url: String,
    description: String,
}

impl OpenAiChatLlm {
    pub fn new(settings: LlmSettings) -> OpenAiChatLlm {
        let base_url = settings.base_url.as_deref().unwrap_or(OPENAI_BASE_URL).trim_end_matches('/').to_string();
        let models = if !settings.model.is_empty() || !settings.models.fast.is_empty() {
            describe_models(&settings.models)
        } else {
            "(LLM_MODEL not set)".into()
        };
        let description = format!("{models} via {base_url}/chat/completions");
        OpenAiChatLlm { settings, base_url, description }
    }
}

impl LlmClient for OpenAiChatLlm {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> bool {
        true
    }

    fn stream(&self, request: LlmRequest, cancel: CancellationToken) -> LlmStream {
        let settings = self.settings.clone();
        let url = format!("{}/chat/completions", self.base_url);
        generate(move |y| async move {
            if let Err(error) = run(&settings, &url, &request, &cancel, &y).await {
                y.send(Err(error)).await;
            }
        })
    }
}

/// What a provider added to its turn that must go back with it: DeepSeek's reasoning, Gemini's per-call signatures.
fn to_chat_messages(message: &LlmMessage) -> Vec<Value> {
    match message {
        LlmMessage::Tool(results) => results
            .iter()
            .map(|result| {
                // The format has no error flag: the text says it.
                let content = if result.is_error && !starts_with_error(&result.content) {
                    format!("error: {}", result.content)
                } else {
                    result.content.clone()
                };
                json!({"role": "tool", "tool_call_id": result.id, "content": content})
            })
            .collect(),
        LlmMessage::Assistant(turn) if !turn.tool_calls.is_empty() => {
            let native = turn.native.as_ref();
            let extras = native.and_then(|native| native.get("extras")).and_then(Value::as_object);
            let calls: Vec<Value> = turn
                .tool_calls
                .iter()
                .map(|call| {
                    let arguments = serde_json::to_string(&call.input).unwrap_or_else(|_| "{}".into());
                    let mut piece = json!({
                        "id": call.id,
                        "type": "function",
                        "function": {"name": call.name, "arguments": arguments},
                    });
                    if let Some(extra) = extras.and_then(|extras| extras.get(&call.id)) {
                        piece["extra_content"] = extra.clone();
                    }
                    piece
                })
                .collect();
            let content = if turn.content.is_empty() { Value::Null } else { json!(turn.content) };
            let mut message = json!({"role": "assistant", "content": content});
            if let Some(reasoning) =
                native.and_then(|native| native.get("reasoning")).and_then(Value::as_str).filter(|r| !r.is_empty())
            {
                message["reasoning_content"] = json!(reasoning);
            }
            message["tool_calls"] = Value::Array(calls);
            vec![message]
        }
        LlmMessage::Assistant(turn) => vec![json!({"role": "assistant", "content": turn.content})],
        LlmMessage::User(text) => vec![json!({"role": "user", "content": text})],
    }
}

fn starts_with_error(text: &str) -> bool {
    text.get(..5).is_some_and(|head| head.eq_ignore_ascii_case("error"))
}

/// The first `max` characters.
fn head(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// The call's input, or why it is not one: a broken call fails alone, not the whole answer.
fn parse_arguments(text: &str) -> (Map<String, Value>, Option<String>) {
    if text.trim().is_empty() {
        return (Map::new(), None);
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(input)) => (input, None),
        Ok(_) => (Map::new(), Some(format!("the arguments are not a JSON object: {}", head(text, 80)))),
        Err(_) => (Map::new(), Some(format!("the arguments are not valid JSON: {}", head(text, 80)))),
    }
}

fn aborted() -> anyhow::Error {
    anyhow!("This operation was aborted")
}

#[derive(Debug, Default)]
struct PendingCall {
    id: String,
    name: String,
    arguments: String,
    extra: Option<Value>,
}

async fn run(
    settings: &LlmSettings,
    url: &str,
    request: &LlmRequest,
    cancel: &CancellationToken,
    y: &Yielder<anyhow::Result<LlmEvent>>,
) -> anyhow::Result<()> {
    let tools: Vec<Value> = request
        .tools
        .iter()
        .map(|tool| {
            json!({"type": "function", "function": {"name": tool.name, "description": tool.description, "parameters": tool.parameters}})
        })
        .collect();
    let system = match request.system_tail.as_deref().filter(|tail| !tail.is_empty()) {
        Some(tail) => format!("{}\n\n{tail}", request.system),
        None => request.system.clone(),
    };
    let mut messages = vec![json!({"role": "system", "content": system})];
    messages.extend(request.messages.iter().flat_map(to_chat_messages));
    let model = request.model.clone().filter(|model| !model.is_empty()).unwrap_or_else(|| settings.model.clone());
    let mut body = Map::new();
    body.insert("model".into(), json!(model));
    body.insert("stream".into(), json!(true));
    body.insert(tokens_field_name(settings.tokens_field).into(), json!(settings.max_tokens));
    body.insert("messages".into(), Value::Array(messages));
    if !tools.is_empty() {
        body.insert("tools".into(), Value::Array(tools));
        if request.tool_choice == ToolChoice::None {
            body.insert("tool_choice".into(), json!("none"));
        }
    }

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Some(key) = settings.api_key.as_deref().filter(|key| !key.is_empty()) {
        let mut bearer = HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| anyhow!("the API key is not a header value"))?;
        bearer.set_sensitive(true);
        headers.insert(AUTHORIZATION, bearer);
    }
    for (name, value) in &settings.headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| anyhow!("not a header name: {name}"))?;
        let value = HeaderValue::from_str(value).map_err(|_| anyhow!("not a value for the header {name}"))?;
        headers.insert(name, value);
    }

    let send = client().post(url).headers(headers).body(Value::Object(body).to_string()).send();
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
        bail!("HTTP {status}: {}", head(&text, 300));
    }

    // Server-sent events: `data: {json}` lines, then `data: [DONE]`.
    let mut chunks = response.bytes_stream();
    let mut lines = Lines::default();
    let mut pending: BTreeMap<i64, PendingCall> = BTreeMap::new();
    let mut current: i64 = -1;
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut finished = false;
    while !finished {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(aborted()),
            chunk = chunks.next() => chunk,
        };
        let got = match chunk {
            Some(Ok(bytes)) => lines.push(&bytes),
            Some(Err(error)) => bail!("the answer broke off: {}", describe(&error)),
            None => {
                finished = true;
                lines.finish().into_iter().collect()
            }
        };
        for line in got {
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                finished = true;
                break;
            }
            let chunk: Value =
                serde_json::from_str(data).map_err(|error| anyhow!("a streamed chunk is not JSON: {error}"))?;
            let delta = chunk.get("choices").and_then(|choices| choices.get(0)).and_then(|choice| choice.get("delta"));
            let Some(delta) = delta else { continue };
            if let Some(content) = delta.get("content").and_then(Value::as_str).filter(|content| !content.is_empty()) {
                text.push_str(content);
                y.send(Ok(LlmEvent::Text(content.to_string()))).await;
            }
            if let Some(more) = delta.get("reasoning_content").and_then(Value::as_str) {
                reasoning.push_str(more);
            }
            for piece in delta.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                let id = piece.get("id").and_then(Value::as_str).filter(|id| !id.is_empty());
                // Without an index a new id starts a new call; pieces with neither continue the current one.
                if let Some(index) = piece.get("index").and_then(Value::as_i64) {
                    current = index;
                } else if id.is_some()
                    && (current < 0
                        || pending
                            .get(&current)
                            .is_some_and(|call| !call.id.is_empty() && Some(call.id.as_str()) != id))
                {
                    current += 1;
                } else if current < 0 {
                    current = 0;
                }
                let call = pending.entry(current).or_default();
                if let Some(id) = id {
                    call.id = id.to_string();
                }
                let function = piece.get("function");
                let part = |name: &str| function.and_then(|f| f.get(name)).and_then(Value::as_str).unwrap_or("");
                call.name.push_str(part("name"));
                call.arguments.push_str(part("arguments"));
                // Gemini's thought signature rides on the call and must come back with it.
                if let Some(extra) = piece.get("extra_content") {
                    call.extra = Some(extra.clone());
                }
            }
        }
    }

    let mut calls = Vec::new();
    let mut extras = Map::new();
    for (index, piece) in pending {
        let id = if piece.id.is_empty() { format!("call_{index}") } else { piece.id };
        let (input, error) = parse_arguments(&piece.arguments);
        if let Some(extra) = piece.extra {
            extras.insert(id.clone(), extra);
        }
        calls.push(ToolCall { id, name: piece.name, input, error });
    }
    for call in &calls {
        y.send(Ok(LlmEvent::Tool(call.clone()))).await;
    }
    let mut native = Map::new();
    if !reasoning.is_empty() {
        native.insert("reasoning".into(), json!(reasoning));
    }
    if !extras.is_empty() {
        native.insert("extras".into(), Value::Object(extras));
    }
    let turn = AssistantTurn {
        content: text,
        tool_calls: calls,
        native: (!native.is_empty()).then_some(Value::Object(native)),
        usage: None,
    };
    y.send(Ok(LlmEvent::Turn(turn))).await;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::{DialogProvider, ModelPair};
    use crate::dialog::llm::{LlmTool, ToolResult};
    use axum::extract::State;
    use axum::http::HeaderMap as Headers;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Stub {
        requests: Arc<Mutex<Vec<(Value, Headers)>>>,
        /// The chunks to stream for the next request, as `data:` lines.
        chunks: Arc<Mutex<Vec<Value>>>,
    }

    async fn chat(State(stub): State<Stub>, headers: Headers, body: String) -> axum::response::Response {
        stub.requests.lock().unwrap().push((serde_json::from_str(&body).unwrap(), headers));
        let mut out = String::new();
        for chunk in stub.chunks.lock().unwrap().iter() {
            out.push_str(&format!("data: {chunk}\n\n"));
        }
        out.push_str("data: [DONE]\n\n");
        ([(CONTENT_TYPE, "text/event-stream")], out).into_response()
    }

    async fn stub() -> (Stub, String) {
        let stub = Stub::default();
        let app = axum::Router::new().route("/v1/chat/completions", post(chat)).with_state(stub.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        (stub, url)
    }

    pub(crate) fn settings(url: &str) -> LlmSettings {
        LlmSettings {
            provider: DialogProvider::OpenAi,
            model: "m".into(),
            models: ModelPair { fast: "m".into(), smart: "m".into() },
            base_url: Some(url.into()),
            api_key: None,
            max_tokens: 300,
            effort: "low".into(),
            explicit: false,
            headers: BTreeMap::new(),
            tokens_field: TokensField::MaxTokens,
        }
    }

    async fn events(llm: &OpenAiChatLlm, request: LlmRequest) -> Vec<LlmEvent> {
        llm.stream(request, CancellationToken::new()).map(|event| event.unwrap()).collect().await
    }

    fn delta(value: Value) -> Value {
        json!({"choices": [{"delta": value}]})
    }

    fn tool() -> LlmTool {
        LlmTool {
            name: "search".into(),
            description: "Search.".into(),
            parameters: json!({"type": "object", "properties": {}}),
        }
    }

    fn question() -> Vec<LlmMessage> {
        vec![LlmMessage::User("q".into())]
    }

    fn input(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[tokio::test]
    async fn keeps_parallel_calls_apart_when_the_provider_sends_no_index_by_their_ids() {
        let (server, url) = stub().await;
        *server.chunks.lock().unwrap() = vec![
            delta(json!({"tool_calls": [{"id": "a", "function": {"name": "search", "arguments": "{\"q\":"}}]})),
            delta(json!({"tool_calls": [{"function": {"arguments": "\"кофе\"}"}}]})),
            delta(json!({"tool_calls": [{"id": "b", "function": {"name": "search", "arguments": "{\"q\":\"чай\"}"}}]})),
        ];
        let llm = OpenAiChatLlm::new(settings(&url));
        let request =
            LlmRequest { system: "s".into(), messages: question(), tools: vec![tool()], ..LlmRequest::default() };
        let calls: Vec<ToolCall> = events(&llm, request)
            .await
            .into_iter()
            .filter_map(|event| if let LlmEvent::Tool(call) = event { Some(call) } else { None })
            .collect();
        assert_eq!(
            calls,
            vec![
                ToolCall { id: "a".into(), name: "search".into(), input: input(json!({"q": "кофе"})), error: None },
                ToolCall { id: "b".into(), name: "search".into(), input: input(json!({"q": "чай"})), error: None },
            ]
        );
    }

    #[tokio::test]
    async fn gives_back_deepseeks_reasoning_and_geminis_per_call_signature_with_the_turn_they_came_in() {
        let (server, url) = stub().await;
        *server.chunks.lock().unwrap() = vec![
            delta(json!({"reasoning_content": "Нужно поискать."})),
            delta(json!({"tool_calls": [{"index": 0, "id": "c1", "function": {"name": "search", "arguments": "{}"},
                "extra_content": {"google": {"thought_signature": "sig"}}}]})),
        ];
        let llm = OpenAiChatLlm::new(settings(&url));
        let request =
            LlmRequest { system: "s".into(), messages: question(), tools: vec![tool()], ..LlmRequest::default() };
        let turn = events(&llm, request)
            .await
            .into_iter()
            .find_map(|event| if let LlmEvent::Turn(turn) = event { Some(turn) } else { None })
            .expect("a turn");
        *server.chunks.lock().unwrap() = vec![delta(json!({"content": "Готово."}))];
        let next = LlmRequest {
            system: "s".into(),
            system_tail: Some("Сейчас 10:00.".into()),
            messages: vec![
                LlmMessage::User("q".into()),
                LlmMessage::Assistant(turn),
                LlmMessage::Tool(vec![ToolResult {
                    id: "c1".into(), content: "нашлось".into(), is_error: false
                }]),
            ],
            tools: vec![tool()],
            tool_choice: ToolChoice::None,
            ..LlmRequest::default()
        };
        events(&llm, next).await;
        let (sent, _) = server.requests.lock().unwrap().last().cloned().unwrap();
        assert_eq!(sent["tool_choice"], json!("none"));
        assert_eq!(sent["messages"][0], json!({"role": "system", "content": "s\n\nСейчас 10:00."}));
        assert_eq!(
            sent["messages"][2],
            json!({
                "role": "assistant",
                "content": null,
                "reasoning_content": "Нужно поискать.",
                "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": {"name": "search", "arguments": "{}"},
                    "extra_content": {"google": {"thought_signature": "sig"}},
                }],
            })
        );
        assert_eq!(sent["messages"][3], json!({"role": "tool", "tool_call_id": "c1", "content": "нашлось"}));
    }

    #[tokio::test]
    async fn names_the_output_limit_as_the_service_wants_it_and_sends_the_headers_a_provider_needs() {
        let (server, url) = stub().await;
        *server.chunks.lock().unwrap() = vec![delta(json!({"content": "Да."}))];
        let mut config = settings(&url);
        config.tokens_field = TokensField::MaxCompletionTokens;
        config.headers.insert("OpenAI-Project".into(), "folder".into());
        config.api_key = Some("sk-local".into());
        let llm = OpenAiChatLlm::new(config);
        events(&llm, LlmRequest { system: "s".into(), messages: question(), ..LlmRequest::default() }).await;
        let (sent, headers) = server.requests.lock().unwrap().last().cloned().unwrap();
        assert_eq!(sent["max_completion_tokens"], json!(300));
        assert!(sent.get("max_tokens").is_none());
        assert!(sent.get("temperature").is_none());
        assert!(sent.get("tools").is_none());
        assert_eq!(headers["openai-project"], "folder");
        assert_eq!(headers["authorization"], "Bearer sk-local");
        assert_eq!(tokens_field_for("https://api.openai.com/v1"), TokensField::MaxCompletionTokens);
        assert_eq!(tokens_field_for("https://openrouter.ai/api/v1"), TokensField::MaxTokens);
        assert_eq!(tokens_field_for("not a url"), TokensField::MaxTokens);
    }

    #[tokio::test]
    async fn fails_a_broken_call_alone_and_says_why_the_service_refused() {
        assert_eq!(parse_arguments(" "), (Map::new(), None));
        assert!(parse_arguments("[1]").1.unwrap().starts_with("the arguments are not a JSON object"));
        assert!(parse_arguments("{\"a\":").1.unwrap().starts_with("the arguments are not valid JSON"));
        let failing = axum::Router::new().route(
            "/v1/chat/completions",
            post(|| async { (axum::http::StatusCode::UNAUTHORIZED, r#"{"error":"bad key"}"#) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, failing).await });
        let llm = OpenAiChatLlm::new(settings(&url));
        assert!(llm.description().ends_with("/v1/chat/completions"), "{}", llm.description());
        let out: Vec<_> = llm
            .stream(LlmRequest { messages: question(), ..LlmRequest::default() }, CancellationToken::new())
            .collect()
            .await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].as_ref().unwrap_err().to_string(), r#"HTTP 401: {"error":"bad key"}"#);
        let refused = OpenAiChatLlm::new(settings("http://127.0.0.1:9/v1"));
        let out: Vec<_> = refused
            .stream(LlmRequest { messages: question(), ..LlmRequest::default() }, CancellationToken::new())
            .collect()
            .await;
        assert!(out[0]
            .as_ref()
            .unwrap_err()
            .to_string()
            .starts_with("cannot reach http://127.0.0.1:9/v1/chat/completions"));
    }
}
