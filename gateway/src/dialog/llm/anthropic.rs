//! Claude over the Messages API, streamed, with raw HTTP: the request the Node gateway sent through
//! @anthropic-ai/sdk 1.3.0 (`client.beta.messages.stream`) and the message its stream helper put together.
//!
//! Thinking stays adaptive (Opus 5.5 cannot turn it off) and the effort, low by default, keeps the first words
//! quick. When a safety classifier declines, the API retries on the model Anthropic picks for that category
//! (`fallbacks: "default"`); Haiku has no fallback. Tools are client tools: their calls come back once the turn is
//! over, with the turn to continue from. The stable part of the instructions, with the tools before it, is marked
//! for the prompt cache when it is long; the time and the lookup's data come after the mark.
//!
//! As the SDK: `anthropic-version: 2026-10-21`, the key as `Authorization: Bearer`, one retry (connection errors,
//! 408, 409, 429, 5xx, or what `x-should-retry` says; after `retry-after(-ms)` or half a second), ten minutes for the
//! response to start. Not sent: the SDK's telemetry headers (`X-Stainless-*`, its User-Agent).

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{anyhow, bail};
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::describe_http_error as describe;
use super::llm_client::{
    describe_models, AssistantTurn, LlmClient, LlmEvent, LlmMessage, LlmRequest, LlmSettings, LlmStream, LlmUsage,
    ToolCall, ToolChoice,
};
use super::sse::{Lines, SseDecoder, SseEvent};
use crate::dialog::generator::{generate, Yielder};
use crate::utils::http::client;

/// Where Claude is asked when a panel provider names no other address (the environment's is not borrowed).
pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

/// The API version the SDK sent.
pub const ANTHROPIC_VERSION: &str = "2026-10-21";

/// The beta that lets the API answer a declined request with the fallback model.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// Below this many characters of tools and instructions the prefix is not marked for the cache: Anthropic caches
/// only long prefixes, and a write costs more than a plain read. MCP tools make the prefix long.
const CACHE_MIN_CHARS: usize = 6000;

/// A spoken answer cannot wait out a long backoff.
const MAX_RETRIES: u32 = 1;

/// The SDK's timeout: for the response to start, not for the whole stream.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(600);

pub struct AnthropicLlm {
    settings: LlmSettings,
    description: String,
}

impl AnthropicLlm {
    pub fn new(settings: LlmSettings) -> AnthropicLlm {
        let description = format!("{} via Anthropic, effort {}", describe_models(&settings.models), settings.effort);
        AnthropicLlm { settings, description }
    }
}

impl LlmClient for AnthropicLlm {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> bool {
        true
    }

    fn stream(&self, request: LlmRequest, cancel: CancellationToken) -> LlmStream {
        let settings = self.settings.clone();
        generate(move |y| async move {
            if let Err(error) = run(&settings, &request, &cancel, &y).await {
                y.send(Err(error)).await;
            }
        })
    }
}

async fn run(
    settings: &LlmSettings,
    request: &LlmRequest,
    cancel: &CancellationToken,
    y: &Yielder<anyhow::Result<LlmEvent>>,
) -> anyhow::Result<()> {
    let key = credentials(settings)?;
    let model = request.model.clone().filter(|model| !model.is_empty()).unwrap_or_else(|| settings.model.clone());
    let haiku = model.contains("haiku");
    let body = body_of(settings, request, &model, haiku);
    let headers = headers_of(settings, &key, haiku)?;
    let url = messages_url(settings.base_url.as_deref().filter(|url| !url.is_empty()).unwrap_or(ANTHROPIC_BASE_URL));
    let response = open(&url, headers, body.to_string(), cancel).await?;

    let mut message = Message::default();
    let mut text = String::new();
    let mut lines = Lines::default();
    let mut decoder = SseDecoder::default();
    let mut chunks = response.bytes_stream();
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(aborted()),
            chunk = chunks.next() => chunk,
        };
        let (got, ended) = match chunk {
            Some(Ok(bytes)) => (lines.push(&bytes), false),
            Some(Err(error)) => bail!("Anthropic stream broke: {}", describe(&error)),
            None => (lines.finish().into_iter().collect(), true),
        };
        for line in got {
            let Some(event) = decoder.line(&line) else { continue };
            if let Some(delta) = handle(&mut message, event)? {
                text.push_str(&delta);
                y.send(Ok(LlmEvent::Text(delta))).await;
            }
        }
        if ended {
            break;
        }
    }
    let turn = message.finish(text)?;
    for call in &turn.tool_calls {
        y.send(Ok(LlmEvent::Tool(call.clone()))).await;
    }
    y.send(Ok(LlmEvent::Turn(turn))).await;
    Ok(())
}

/// The key. From .env: LLM_API_KEY, else ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN (LlmSettings::from_dialog). A
/// panel provider uses its own key only: without one every request fails with a plain reason instead of quietly
/// spending the environment's key. (The SDK's `ant auth login` profiles are not read.)
fn credentials(settings: &LlmSettings) -> anyhow::Result<String> {
    match settings.api_key.as_deref().filter(|key| !key.is_empty()) {
        Some(key) => Ok(key.to_string()),
        None if settings.explicit => bail!("no API key: add it to the provider in the panel"),
        None => bail!("no API key: set LLM_API_KEY or ANTHROPIC_API_KEY"),
    }
}

/// `{base}/v1/messages`, joined as the SDK joined a path to its base URL.
fn messages_url(base: &str) -> String {
    if base.ends_with('/') {
        format!("{base}v1/messages")
    } else {
        format!("{base}/v1/messages")
    }
}

/// Characters as JavaScript counted them: the cache threshold is the Node gateway's.
fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn body_of(settings: &LlmSettings, request: &LlmRequest, model: &str, haiku: bool) -> Value {
    let tools: Vec<Value> = request
        .tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.parameters,
                "eager_input_streaming": true,
            })
        })
        .collect();
    let tools_chars =
        if request.tools.is_empty() { 0 } else { js_len(&serde_json::to_string(&request.tools).unwrap_or_default()) };
    let mut stable = json!({"type": "text", "text": request.system});
    if js_len(&request.system) + tools_chars >= CACHE_MIN_CHARS {
        stable["cache_control"] = json!({"type": "ephemeral"});
    }
    let mut system = vec![stable];
    if let Some(tail) = request.system_tail.as_deref().filter(|tail| !tail.is_empty()) {
        system.push(json!({"type": "text", "text": tail}));
    }
    let mut body = Map::new();
    body.insert("model".into(), json!(model));
    body.insert("max_tokens".into(), json!(settings.max_tokens));
    body.insert("system".into(), Value::Array(system));
    body.insert("messages".into(), Value::Array(request.messages.iter().map(to_param).collect()));
    body.insert("output_config".into(), json!({"effort": settings.effort}));
    if !tools.is_empty() {
        body.insert("tools".into(), Value::Array(tools));
        if request.tool_choice == ToolChoice::None {
            body.insert("tool_choice".into(), json!({"type": "none"}));
        }
    }
    if !haiku {
        body.insert("fallbacks".into(), json!("default"));
    }
    body.insert("stream".into(), json!(true));
    Value::Object(body)
}

/// Whether JavaScript took a value for true: the Node gateway sent `native` back when it was.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn to_param(message: &LlmMessage) -> Value {
    match message {
        LlmMessage::Tool(results) => {
            let content: Vec<Value> = results
                .iter()
                .map(|result| {
                    let mut block = json!({"type": "tool_result", "tool_use_id": result.id, "content": result.content});
                    if result.is_error {
                        block["is_error"] = json!(true);
                    }
                    block
                })
                .collect();
            json!({"role": "user", "content": content})
        }
        // Thinking blocks are valid only as the model wrote them: the turn goes back as it came.
        LlmMessage::Assistant(turn) if turn.native.as_ref().is_some_and(truthy) => {
            json!({"role": "assistant", "content": turn.native})
        }
        LlmMessage::Assistant(turn) if !turn.tool_calls.is_empty() => {
            let mut content = Vec::new();
            if !turn.content.is_empty() {
                content.push(json!({"type": "text", "text": turn.content}));
            }
            for call in &turn.tool_calls {
                content.push(json!({"type": "tool_use", "id": call.id, "name": call.name, "input": call.input}));
            }
            json!({"role": "assistant", "content": content})
        }
        LlmMessage::Assistant(turn) => json!({"role": "assistant", "content": turn.content}),
        LlmMessage::User(text) => json!({"role": "user", "content": text}),
    }
}

fn headers_of(settings: &LlmSettings, key: &str, haiku: bool) -> anyhow::Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert("anthropic-version", HeaderValue::from_static(ANTHROPIC_VERSION));
    let mut bearer =
        HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| anyhow!("the API key is not a header value"))?;
    bearer.set_sensitive(true);
    headers.insert(AUTHORIZATION, bearer);
    // The provider's own headers over those, as the SDK's defaultHeaders were.
    for (name, value) in &settings.headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| anyhow!("not a header name: {name}"))?;
        let value = HeaderValue::from_str(value).map_err(|_| anyhow!("not a value for the header {name}"))?;
        headers.insert(name, value);
    }
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if !haiku {
        headers.insert("anthropic-beta", HeaderValue::from_static(FALLBACK_BETA));
    }
    Ok(headers)
}

fn aborted() -> anyhow::Error {
    anyhow!("Request was aborted.")
}

/// The request until the response starts, retried once as the SDK did.
async fn open(
    url: &str,
    headers: HeaderMap,
    body: String,
    cancel: &CancellationToken,
) -> anyhow::Result<reqwest::Response> {
    let mut retries = MAX_RETRIES;
    loop {
        let send = client().post(url).headers(headers.clone()).body(body.clone()).send();
        let outcome = tokio::select! {
            _ = cancel.cancelled() => return Err(aborted()),
            outcome = tokio::time::timeout(RESPONSE_TIMEOUT, send) => outcome,
        };
        let asked_wait = match outcome {
            Ok(Ok(response)) if response.status().is_success() => return Ok(response),
            Ok(Ok(response)) => {
                if retries == 0 || !should_retry(&response) {
                    let status = response.status().as_u16();
                    let text = tokio::select! {
                        _ = cancel.cancelled() => return Err(aborted()),
                        text = response.text() => text.unwrap_or_else(|error| error.to_string()),
                    };
                    return Err(status_error(status, &text));
                }
                retry_after(response.headers())
            }
            Ok(Err(error)) if retries == 0 => bail!("cannot reach Anthropic: Connection error. ({})", describe(&error)),
            Err(_) if retries == 0 => bail!("cannot reach Anthropic: Request timed out."),
            Ok(Err(_)) | Err(_) => None,
        };
        let wait = asked_wait.unwrap_or_else(|| default_retry_delay(MAX_RETRIES - retries));
        retries -= 1;
        tokio::select! {
            _ = cancel.cancelled() => return Err(aborted()),
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

fn should_retry(response: &reqwest::Response) -> bool {
    match response.headers().get("x-should-retry").and_then(|value| value.to_str().ok()) {
        Some("true") => return true,
        Some("false") => return false,
        _ => {}
    }
    let status = response.status().as_u16();
    matches!(status, 408 | 409 | 429) || status >= 500
}

/// The wait the API asked for: `retry-after-ms`, or `retry-after` in seconds or as a date; None for none usable.
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).map(str::trim);
    // A zero `retry-after-ms` does not count, as for the SDK: `retry-after` decides then.
    let ms =
        header("retry-after-ms").and_then(|value| value.parse::<f64>().ok()).filter(|ms| *ms != 0.0).or_else(|| {
            let value = header("retry-after")?;
            match value.parse::<f64>() {
                Ok(seconds) => Some(seconds * 1000.0),
                Err(_) => chrono::DateTime::parse_from_rfc2822(value)
                    .ok()
                    .map(|at| (at.timestamp_millis() - chrono::Utc::now().timestamp_millis()) as f64),
            }
        })?;
    (ms > 0.0 && ms <= 2f64.powi(31) - 1.0).then(|| Duration::from_secs_f64(ms / 1000.0))
}

/// Half a second, doubling per retry up to eight, less up to a quarter of jitter.
fn default_retry_delay(retries_done: u32) -> Duration {
    let seconds = (0.5 * 2f64.powi(retries_done as i32)).min(8.0);
    let jitter = 1.0 - rand::random::<f64>() * 0.25;
    Duration::from_secs_f64(seconds * jitter)
}

/// The SDK's error message for a status: «401 {"type":"error",…}», its body's message when it has one.
fn api_message(status: u16, text: &str) -> String {
    let detail = match serde_json::from_str::<Value>(text) {
        Ok(json) => match json.get("message") {
            Some(Value::String(message)) if !message.is_empty() => message.clone(),
            Some(message) if truthy(message) => message.to_string(),
            _ => json.to_string(),
        },
        Err(_) if text.is_empty() => return format!("{status} status code (no body)"),
        Err(_) => text.to_string(),
    };
    format!("{status} {detail}")
}

/// Readable reasons for the log, as the Node gateway explained the SDK's typed errors.
fn status_error(status: u16, text: &str) -> anyhow::Error {
    let message = api_message(status, text);
    match status {
        401 => anyhow!("Anthropic rejected the API key"),
        404 => anyhow!("Anthropic does not know the model: {message}"),
        429 => anyhow!("Anthropic rate limit: {message}"),
        _ => anyhow!("Anthropic API {status}: {message}"),
    }
}

/// One server-sent event: the text it adds, if any. Events without a name and pings are skipped, as by the SDK.
fn handle(message: &mut Message, event: SseEvent) -> anyhow::Result<Option<String>> {
    match event.event.as_deref() {
        None | Some("ping") => Ok(None),
        Some("error") => bail!("Anthropic API ?: {}", event.data),
        Some(_) if event.data.is_empty() => Ok(None),
        Some(_) => {
            let data: Value = serde_json::from_str(&event.data)
                .map_err(|error| anyhow!("Anthropic sent an event that is not JSON: {error}"))?;
            message.apply(&data)
        }
    }
}

/// The message as the stream builds it: content blocks with their deltas applied, the stop reason, the usage.
#[derive(Debug, Default)]
struct Message {
    started: bool,
    stopped: bool,
    content: Vec<Value>,
    /// The partial JSON of tool inputs, by block.
    inputs: HashMap<usize, String>,
    /// A tool input that did not parse.
    unparsed: Option<String>,
    stop_reason: Option<String>,
    stop_details: Value,
    usage: Usage,
}

#[derive(Debug, Default)]
struct Usage {
    input: u64,
    output: u64,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
}

fn count(usage: Option<&Value>, name: &str) -> Option<u64> {
    usage.and_then(|usage| usage.get(name)).and_then(Value::as_u64)
}

/// The blocks whose input streams as JSON.
fn takes_input(kind: &str) -> bool {
    matches!(kind, "tool_use" | "server_tool_use" | "mcp_tool_use")
}

fn append(block: &mut Map<String, Value>, field: &str, piece: &str) {
    let mut text = block.get(field).and_then(Value::as_str).unwrap_or("").to_string();
    text.push_str(piece);
    block.insert(field.into(), Value::String(text));
}

impl Message {
    fn apply(&mut self, event: &Value) -> anyhow::Result<Option<String>> {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let index = event.get("index").and_then(Value::as_u64).map(|index| index as usize);
        match kind {
            "message_start" => {
                if self.started {
                    bail!("Unexpected event order, got message_start before receiving \"message_stop\"");
                }
                self.started = true;
                let message = event.get("message");
                self.content = message
                    .and_then(|message| message.get("content"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                self.stop_reason =
                    message.and_then(|message| message.get("stop_reason")).and_then(Value::as_str).map(str::to_string);
                let usage = message.and_then(|message| message.get("usage"));
                self.usage = Usage {
                    input: count(usage, "input_tokens").unwrap_or(0),
                    output: count(usage, "output_tokens").unwrap_or(0),
                    cache_read: count(usage, "cache_read_input_tokens"),
                    cache_write: count(usage, "cache_creation_input_tokens"),
                };
            }
            "content_block_start" if self.started => {
                self.content.push(event.get("content_block").cloned().unwrap_or(Value::Null));
            }
            "content_block_delta" => {
                let delta = event.get("delta").unwrap_or(&Value::Null);
                let delta_kind = delta.get("type").and_then(Value::as_str).unwrap_or("");
                if let (true, Some(index)) = (self.started, index) {
                    self.merge(index, delta_kind, delta);
                }
                if delta_kind == "text_delta" {
                    return Ok(Some(delta.get("text").and_then(Value::as_str).unwrap_or("").to_string()));
                }
            }
            "content_block_stop" if self.started => {
                if let Some(index) = index {
                    self.close(index);
                }
            }
            "message_delta" if self.started => {
                let delta = event.get("delta");
                self.stop_reason =
                    delta.and_then(|delta| delta.get("stop_reason")).and_then(Value::as_str).map(str::to_string);
                self.stop_details = delta.and_then(|delta| delta.get("stop_details")).cloned().unwrap_or(Value::Null);
                // The counters are whole-message totals, sent when they apply: overwritten, never added.
                let usage = event.get("usage");
                if let Some(output) = count(usage, "output_tokens") {
                    self.usage.output = output;
                }
                if let Some(input) = count(usage, "input_tokens") {
                    self.usage.input = input;
                }
                if let Some(written) = count(usage, "cache_creation_input_tokens") {
                    self.usage.cache_write = Some(written);
                }
                if let Some(read) = count(usage, "cache_read_input_tokens") {
                    self.usage.cache_read = Some(read);
                }
            }
            "message_stop" if self.started => self.stopped = true,
            _ => {}
        }
        Ok(None)
    }

    fn merge(&mut self, index: usize, delta_kind: &str, delta: &Value) {
        let Some(block) = self.content.get_mut(index).and_then(Value::as_object_mut) else { return };
        let kind = block.get("type").and_then(Value::as_str).unwrap_or("").to_string();
        let piece = |field: &str| delta.get(field).and_then(Value::as_str).unwrap_or("");
        match (delta_kind, kind.as_str()) {
            ("text_delta", "text") => append(block, "text", piece("text")),
            ("citations_delta", "text") => {
                let mut citations = block.get("citations").and_then(Value::as_array).cloned().unwrap_or_default();
                citations.push(delta.get("citation").cloned().unwrap_or(Value::Null));
                block.insert("citations".into(), Value::Array(citations));
            }
            ("input_json_delta", kind) if takes_input(kind) => {
                self.inputs.entry(index).or_default().push_str(piece("partial_json"));
            }
            ("thinking_delta", "thinking") => append(block, "thinking", piece("thinking")),
            ("signature_delta", "thinking") => {
                block.insert("signature".into(), delta.get("signature").cloned().unwrap_or(Value::Null));
            }
            ("compaction_delta", "compaction") => {
                block.insert("content".into(), delta.get("content").cloned().unwrap_or(Value::Null));
                if let Some(encrypted) = delta.get("encrypted_content") {
                    block.insert("encrypted_content".into(), encrypted.clone());
                }
            }
            _ => {}
        }
    }

    /// A finished block: a tool's streamed input becomes its `input` ({} when nothing streamed).
    fn close(&mut self, index: usize) {
        let Some(buffer) = self.inputs.remove(&index) else { return };
        let Some(block) = self.content.get_mut(index).and_then(Value::as_object_mut) else { return };
        let input = if buffer.is_empty() {
            Value::Object(Map::new())
        } else {
            serde_json::from_str(&buffer).unwrap_or_else(|error| {
                self.unparsed.get_or_insert(format!(
                    "Unable to parse tool parameter JSON from model. Please retry your request or adjust your prompt. \
                     Error: {error}. JSON: {buffer}"
                ));
                Value::Object(Map::new())
            })
        };
        block.insert("input".into(), input);
    }

    /// The turn, once the stream is over: its tool calls, its blocks as they came (`native`) and the usage.
    fn finish(self, text: String) -> anyhow::Result<AssistantTurn> {
        let out_of_tokens = self.stop_reason.as_deref() == Some("max_tokens");
        // A cut-off input fails as running out of tokens below (the SDK parsed partial JSON); a broken one fails here.
        if let (Some(problem), false) = (&self.unparsed, out_of_tokens) {
            bail!("{problem}");
        }
        if !self.started {
            bail!("request ended without sending any chunks");
        }
        if !self.stopped {
            bail!("stream ended without producing a Message with role=assistant");
        }
        if self.stop_reason.as_deref() == Some("refusal") {
            let category = self.stop_details.get("category").and_then(Value::as_str).unwrap_or("no category");
            bail!("Claude declined to answer ({category})");
        }
        let mut calls = Vec::new();
        for block in &self.content {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            // A cut-off turn may hold a truncated input: no command is run on it.
            if out_of_tokens {
                bail!("Claude ran out of tokens in a tool call");
            }
            calls.push(ToolCall {
                id: block.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                name: block.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                input: block.get("input").and_then(Value::as_object).cloned().unwrap_or_default(),
                error: None,
            });
        }
        let usage = LlmUsage {
            input: self.usage.input,
            output: self.usage.output,
            cache_read: self.usage.cache_read.filter(|n| *n > 0),
            cache_write: self.usage.cache_write.filter(|n| *n > 0),
        };
        Ok(AssistantTurn {
            content: text,
            tool_calls: calls,
            native: Some(Value::Array(self.content)),
            usage: Some(usage),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DialogProvider, ModelPair};
    use crate::dialog::llm::{LlmTool, ToolResult};
    use axum::extract::State;
    use axum::http::HeaderMap as Headers;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    type Recorded = Arc<Mutex<Vec<(Value, Headers)>>>;

    struct Stub {
        url: String,
        requests: Recorded,
        /// The events of the next answers; one text block «Привет.» when empty.
        script: Arc<Mutex<Vec<String>>>,
    }

    fn event(name: &str, data: Value) -> String {
        format!("event: {name}\ndata: {data}\n\n")
    }

    /// Anthropic's streaming Messages API, enough for the client: one text block «Привет.».
    fn hello() -> String {
        [
            event(
                "message_start",
                json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant",
                    "model": "claude-test", "content": [], "stop_reason": null, "stop_sequence": null,
                    "usage": {"input_tokens": 12, "output_tokens": 1, "cache_read_input_tokens": 2000}}}),
            ),
            event(
                "content_block_start",
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            ),
            event("ping", json!({"type": "ping"})),
            event(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Привет."}}),
            ),
            event("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
            event(
                "message_delta",
                json!({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                    "usage": {"output_tokens": 3}}),
            ),
            event("message_stop", json!({"type": "message_stop"})),
        ]
        .concat()
    }

    async fn messages(
        State((requests, script)): State<(Recorded, Arc<Mutex<Vec<String>>>)>,
        headers: Headers,
        body: String,
    ) -> axum::response::Response {
        requests.lock().unwrap().push((serde_json::from_str(&body).unwrap(), headers));
        let next = script.lock().unwrap().pop();
        ([(CONTENT_TYPE, "text/event-stream")], next.unwrap_or_else(hello)).into_response()
    }

    async fn stub() -> Stub {
        let requests: Recorded = Arc::default();
        let script: Arc<Mutex<Vec<String>>> = Arc::default();
        let app =
            axum::Router::new().route("/v1/messages", post(messages)).with_state((requests.clone(), script.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        Stub { url, requests, script }
    }

    fn settings(url: &str, explicit: bool, key: Option<&str>) -> LlmSettings {
        LlmSettings {
            provider: DialogProvider::Anthropic,
            model: "claude-haiku-test".into(),
            models: ModelPair { fast: "claude-haiku-test".into(), smart: "claude-haiku-test".into() },
            base_url: Some(url.into()),
            api_key: key.map(str::to_string),
            max_tokens: 300,
            effort: "low".into(),
            explicit,
            headers: BTreeMap::new(),
            tokens_field: Default::default(),
        }
    }

    fn ask(text: &str) -> Vec<LlmMessage> {
        vec![LlmMessage::User(text.into())]
    }

    async fn events(llm: &AnthropicLlm, request: LlmRequest) -> anyhow::Result<Vec<LlmEvent>> {
        llm.stream(request, CancellationToken::new()).collect::<Vec<_>>().await.into_iter().collect()
    }

    #[tokio::test]
    async fn never_borrows_the_environments_key_for_a_provider_from_the_panel() {
        let server = stub().await;
        let llm = AnthropicLlm::new(settings(&server.url, true, None));
        let request = LlmRequest { system: "s".into(), messages: ask("q"), ..LlmRequest::default() };
        let error = events(&llm, request).await.unwrap_err();
        assert!(error.to_string().contains("no API key"), "{error}");
        assert!(server.requests.lock().unwrap().is_empty(), "nothing is sent without the provider's own key");
    }

    #[tokio::test]
    async fn uses_the_panel_key_marks_a_long_stable_prefix_for_the_cache_and_puts_the_time_after_it() {
        let server = stub().await;
        let llm = AnthropicLlm::new(settings(&server.url, true, Some("panel-key")));
        let tools = vec![LlmTool {
            name: "search".into(),
            description: "Search.".into(),
            parameters: json!({"type": "object", "properties": {}}),
        }];
        let request = LlmRequest {
            system: "Характер. ".repeat(700),
            system_tail: Some("Сейчас 10:00.".into()),
            messages: ask("q"),
            tools,
            tool_choice: ToolChoice::None,
            ..LlmRequest::default()
        };
        let out = events(&llm, request).await.unwrap();
        let (body, headers) = server.requests.lock().unwrap().last().cloned().unwrap();
        assert_eq!(headers["authorization"], "Bearer panel-key");
        assert_eq!(headers["anthropic-version"], ANTHROPIC_VERSION);
        assert!(headers.get("anthropic-beta").is_none(), "Haiku has no fallback");
        assert!(!format!("{headers:?}").contains("environment-key"));
        assert_eq!(body["system"][0]["cache_control"], json!({"type": "ephemeral"}));
        assert_eq!(body["system"][1], json!({"type": "text", "text": "Сейчас 10:00."}));
        assert_eq!(body["tool_choice"], json!({"type": "none"}));
        assert_eq!(body["tools"][0]["input_schema"], json!({"type": "object", "properties": {}}));
        assert_eq!(body["tools"][0]["eager_input_streaming"], json!(true));
        assert_eq!(body["output_config"], json!({"effort": "low"}));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["max_tokens"], json!(300));
        assert!(body.get("fallbacks").is_none());
        assert_eq!(out[0], LlmEvent::Text("Привет.".into()));
        let Some(LlmEvent::Turn(turn)) = out.last() else { panic!("no turn: {out:?}") };
        assert_eq!(turn.usage, Some(LlmUsage { input: 12, output: 3, cache_read: Some(2000), cache_write: None }));
        assert_eq!(turn.content, "Привет.");
        assert_eq!(turn.native, Some(json!([{"type": "text", "text": "Привет."}])));
    }

    #[tokio::test]
    async fn does_not_mark_a_short_prompt_for_the_cache_a_write_would_cost_more_than_it_saves() {
        let server = stub().await;
        let llm = AnthropicLlm::new(settings(&server.url, true, Some("panel-key")));
        events(&llm, LlmRequest { system: "Коротко.".into(), messages: ask("q"), ..LlmRequest::default() })
            .await
            .unwrap();
        let (body, _) = server.requests.lock().unwrap().last().cloned().unwrap();
        assert_eq!(body["system"], json!([{"type": "text", "text": "Коротко."}]));
    }

    #[tokio::test]
    async fn gives_back_thinking_and_tool_calls_as_they_came_and_asks_the_fallback_for_a_big_model() {
        let server = stub().await;
        let turn_with_tool = [
            event(
                "message_start",
                json!({"type": "message_start", "message": {"content": [], "usage": {"input_tokens": 5, "output_tokens": 0}}}),
            ),
            event(
                "content_block_start",
                json!({"type": "content_block_start", "index": 0,
                    "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
            ),
            event(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Ищу."}}),
            ),
            event(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig"}}),
            ),
            event("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
            event(
                "content_block_start",
                json!({"type": "content_block_start", "index": 1,
                    "content_block": {"type": "tool_use", "id": "toolu_1", "name": "search", "input": {}}}),
            ),
            event(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 1,
                    "delta": {"type": "input_json_delta", "partial_json": "{\"q\": \"ко"}}),
            ),
            event(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 1,
                    "delta": {"type": "input_json_delta", "partial_json": "фе\"}"}}),
            ),
            event("content_block_stop", json!({"type": "content_block_stop", "index": 1})),
            event(
                "message_delta",
                json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"},
                    "usage": {"output_tokens": 9, "cache_creation_input_tokens": 7000}}),
            ),
            event("message_stop", json!({"type": "message_stop"})),
        ]
        .concat();
        server.script.lock().unwrap().push(turn_with_tool);
        let mut config = settings(&server.url, false, Some("env-key"));
        config.model = "claude-opus-test".into();
        config.headers.insert("X-Team".into(), "home".into());
        let llm = AnthropicLlm::new(config);
        let out = events(&llm, LlmRequest { system: "s".into(), messages: ask("Найди кофе"), ..LlmRequest::default() })
            .await
            .unwrap();
        let (body, headers) = server.requests.lock().unwrap().last().cloned().unwrap();
        assert_eq!(headers["anthropic-beta"], FALLBACK_BETA);
        assert_eq!(headers["x-team"], "home");
        assert_eq!(body["fallbacks"], json!("default"));
        assert_eq!(body["model"], json!("claude-opus-test"));
        let call = ToolCall {
            id: "toolu_1".into(),
            name: "search".into(),
            input: json!({"q": "кофе"}).as_object().cloned().unwrap(),
            error: None,
        };
        assert_eq!(out[0], LlmEvent::Tool(call.clone()));
        let LlmEvent::Turn(turn) = out[1].clone() else { panic!("{out:?}") };
        assert_eq!(turn.tool_calls, vec![call]);
        assert_eq!(turn.usage, Some(LlmUsage { input: 5, output: 9, cache_read: None, cache_write: Some(7000) }));
        let native = json!([
            {"type": "thinking", "thinking": "Ищу.", "signature": "sig"},
            {"type": "tool_use", "id": "toolu_1", "name": "search", "input": {"q": "кофе"}},
        ]);
        assert_eq!(turn.native, Some(native.clone()));

        // The next round sends the turn back unchanged, then the result.
        let next = LlmRequest {
            system: "s".into(),
            messages: vec![
                LlmMessage::User("Найди кофе".into()),
                LlmMessage::Assistant(turn),
                LlmMessage::Tool(vec![ToolResult { id: "toolu_1".into(), content: "нет".into(), is_error: true }]),
            ],
            ..LlmRequest::default()
        };
        events(&llm, next).await.unwrap();
        let (body, _) = server.requests.lock().unwrap().last().cloned().unwrap();
        assert_eq!(body["messages"][1], json!({"role": "assistant", "content": native}));
        assert_eq!(
            body["messages"][2],
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "нет", "is_error": true}]})
        );
    }

    #[tokio::test]
    async fn explains_failures_and_refusals() {
        assert_eq!(status_error(401, r#"{"type":"error"}"#).to_string(), "Anthropic rejected the API key");
        let unknown = status_error(404, r#"{"type":"error","error":{"type":"not_found_error","message":"model: x"}}"#)
            .to_string();
        assert!(unknown.starts_with("Anthropic does not know the model: 404 {"), "{unknown}");
        assert!(unknown.contains(r#""message":"model: x""#), "the body, compact: {unknown}");
        assert_eq!(status_error(429, r#"{"message":"slow down"}"#).to_string(), "Anthropic rate limit: 429 slow down");
        assert_eq!(status_error(400, "").to_string(), "Anthropic API 400: 400 status code (no body)");
        assert_eq!(status_error(400, "plain").to_string(), "Anthropic API 400: 400 plain");
        let mut refused = Message { started: true, stopped: true, ..Message::default() };
        refused.stop_reason = Some("refusal".into());
        refused.stop_details = json!({"category": "cyber"});
        assert_eq!(refused.finish(String::new()).unwrap_err().to_string(), "Claude declined to answer (cyber)");
        let mut cut = Message { started: true, stopped: true, ..Message::default() };
        cut.content = vec![json!({"type": "tool_use", "id": "t", "name": "n", "input": {}})];
        cut.stop_reason = Some("max_tokens".into());
        cut.unparsed = Some("Unable to parse".into());
        assert_eq!(cut.finish(String::new()).unwrap_err().to_string(), "Claude ran out of tokens in a tool call");
        assert!(Message::default().finish(String::new()).is_err());
    }

    #[tokio::test]
    async fn retries_once_after_what_the_api_asks_and_stops_when_cancelled() {
        let attempts = Arc::new(Mutex::new(0));
        let counted = attempts.clone();
        let app = axum::Router::new().route(
            "/v1/messages",
            post(move || {
                let counted = counted.clone();
                async move {
                    *counted.lock().unwrap() += 1;
                    (axum::http::StatusCode::SERVICE_UNAVAILABLE, [("retry-after-ms", "10")], r#"{"type":"error"}"#)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        let llm = AnthropicLlm::new(settings(&url, true, Some("k")));
        let error = events(&llm, LlmRequest { messages: ask("q"), ..LlmRequest::default() }).await.unwrap_err();
        assert_eq!(*attempts.lock().unwrap(), 2, "one retry");
        assert!(error.to_string().starts_with("Anthropic API 503: 503 "), "{error}");

        let cancel = CancellationToken::new();
        cancel.cancel();
        let out: Vec<_> =
            llm.stream(LlmRequest { messages: ask("q"), ..LlmRequest::default() }, cancel).collect().await;
        assert_eq!(out.len(), 1);
        assert!(out[0].as_ref().unwrap_err().to_string().contains("aborted"));
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("2"));
        assert_eq!(retry_after(&headers), Some(Duration::from_secs(2)));
        headers.insert("retry-after", HeaderValue::from_static("0"));
        assert_eq!(retry_after(&headers), None);
        assert!(
            default_retry_delay(0) <= Duration::from_millis(500)
                && default_retry_delay(0) >= Duration::from_millis(375)
        );
    }

    #[test]
    fn joins_the_path_to_the_base_as_the_sdk_did() {
        assert_eq!(messages_url("https://api.anthropic.com"), "https://api.anthropic.com/v1/messages");
        assert_eq!(messages_url("http://proxy:8080/"), "http://proxy:8080/v1/messages");
        let to_tool = to_param(&LlmMessage::Assistant(AssistantTurn {
            content: "Ищу.".into(),
            tool_calls: vec![ToolCall { id: "t".into(), name: "n".into(), input: Map::new(), error: None }],
            ..AssistantTurn::default()
        }));
        assert_eq!(
            to_tool,
            json!({"role": "assistant", "content": [{"type": "text", "text": "Ищу."}, {"type": "tool_use", "id": "t", "name": "n", "input": {}}]})
        );
    }
}
