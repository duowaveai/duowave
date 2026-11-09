//! Tiny MCP servers for the tests, written by hand against the wire (no SDK), so the gateway's client is tested
//! against the protocol and not against itself:
//!
//! - the kind most real servers are today: the 2025 `initialize` handshake, `server/discover` unknown — over stdio,
//!   Streamable HTTP and the old HTTP+SSE transport;
//! - a server of the 2026-07-28 protocol, found only through `server/discover`;
//! - a service in the cloud in miniature: an MCP server that wants a bearer token, its protected-resource metadata,
//!   and an authorization server with dynamic registration, PKCE and a code.
//!
//! The stdio servers are this test program itself, run again with a fixture's name (`program`): the tests need no
//! Node and no other binary.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// The old-protocol server's tools: words, a page of other people's text, a write, a failure, a slow call, a crash,
/// a tool that changes its own description (list_changed), what the process sees (its variables, its folder) — and
/// a second page of the list.
pub struct Legacy {
    echo: Mutex<String>,
}

/// What a server does with one message: its reply, what it says after, how long it takes, whether it exits.
#[derive(Default)]
pub struct Handled {
    pub reply: Option<Value>,
    pub after: Vec<Value>,
    pub delay: Option<Duration>,
    pub exit: bool,
}

fn text(value: &str, is_error: bool) -> Value {
    let mut result = json!({ "content": [{ "type": "text", "text": value }] });
    if is_error {
        result["isError"] = json!(true);
    }
    result
}

fn result(message: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": message["id"], "result": result })
}

fn failure(message: &Value, code: i64, text: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": message["id"], "error": { "code": code, "message": text } })
}

impl Default for Legacy {
    fn default() -> Self {
        Legacy { echo: Mutex::new("Repeats what it is given.".into()) }
    }
}

impl Legacy {
    fn tools(&self) -> Vec<Value> {
        let echo = self.echo.lock().unwrap().clone();
        vec![
            json!({ "name": "echo", "description": echo, "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } }, "annotations": { "readOnlyHint": true } }),
            json!({
                "name": "lookup",
                "description": "Finds a thing on the web.",
                "inputSchema": { "$schema": "https://json-schema.org/draft/2020-12/schema", "type": "object", "properties": { "q": { "$ref": "#/$defs/query" } }, "$defs": { "query": { "type": "string" } } },
                "annotations": { "readOnlyHint": true, "openWorldHint": true },
            }),
            json!({ "name": "add_note", "description": "Adds a note.", "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } }, "annotations": { "destructiveHint": false } }),
            json!({ "name": "fail", "description": "Always fails.", "inputSchema": { "type": "object" } }),
            json!({ "name": "slow", "description": "Takes three seconds.", "inputSchema": { "type": "object" }, "annotations": { "readOnlyHint": true } }),
            json!({ "name": "crash", "description": "Exits.", "inputSchema": { "type": "object" } }),
            json!({ "name": "mutate", "description": "Changes the description of echo.", "inputSchema": { "type": "object" } }),
            json!({ "name": "env", "description": "Lists its variables.", "inputSchema": { "type": "object" }, "annotations": { "readOnlyHint": true } }),
            json!({ "name": "where", "description": "Tells its folder and pid.", "inputSchema": { "type": "object" }, "annotations": { "readOnlyHint": true } }),
        ]
    }

    pub fn handle(&self, message: &Value) -> Handled {
        if message.get("id").is_none() {
            return Handled::default(); // notifications: initialized, cancelled
        }
        let method = message["method"].as_str().unwrap_or_default();
        let params = &message["params"];
        let reply = |value| Handled { reply: Some(result(message, value)), ..Default::default() };
        match method {
            "initialize" => reply(json!({
                "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": { "tools": { "listChanged": true } },
                "serverInfo": { "name": "fixture", "version": "1.0.0" },
                "instructions": "Fixture: use echo to repeat.",
            })),
            "ping" => reply(json!({})),
            "tools/list" => {
                let all = self.tools();
                if params["cursor"] == json!("page-2") {
                    reply(json!({ "tools": all[2..] }))
                } else {
                    reply(json!({ "tools": all[..2], "nextCursor": "page-2" }))
                }
            }
            "tools/call" => self.call(message, params["name"].as_str().unwrap_or_default(), &params["arguments"]),
            other => Handled {
                reply: Some(failure(message, -32601, &format!("Method not found: {other}"))),
                ..Default::default()
            },
        }
    }

    fn call(&self, message: &Value, name: &str, arguments: &Value) -> Handled {
        let reply = |value| Handled { reply: Some(result(message, value)), ..Default::default() };
        let argument = |key: &str| arguments[key].as_str().unwrap_or_default().to_string();
        match name {
            "echo" => reply(text(&format!("echo: {}", argument("text")), false)),
            "lookup" => reply(text(&format!("Нашлось: {}.", argument("q")), false)),
            "add_note" => reply(text("saved", false)),
            "fail" => reply(text("boom", true)),
            "slow" => Handled { delay: Some(Duration::from_secs(3)), ..reply(text("slow done", false)) },
            "crash" => Handled { exit: true, ..Default::default() },
            "mutate" => {
                *self.echo.lock().unwrap() = "Repeats what it is given — and now does more.".into();
                Handled {
                    after: vec![json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" })],
                    ..reply(text("mutated", false))
                }
            }
            "env" => {
                let mut keys: Vec<String> = std::env::vars().map(|(key, _)| key).collect();
                keys.sort();
                let var = std::env::var("FIXTURE_VAR").ok();
                reply(text(&json!({ "keys": keys, "FIXTURE_VAR": var }).to_string(), false))
            }
            "where" => {
                let cwd = std::env::current_dir().map(|dir| dir.display().to_string()).unwrap_or_default();
                reply(text(&json!({ "cwd": cwd, "pid": std::process::id() }).to_string(), false))
            }
            other => Handled {
                reply: Some(failure(message, -32602, &format!("unknown tool {other}"))),
                ..Default::default()
            },
        }
    }
}

/// A server of the 2026-07-28 protocol: no handshake, `server/discover` says who it is; `initialize` it does not know.
pub fn modern(message: &Value) -> Handled {
    if message.get("id").is_none() {
        return Handled::default();
    }
    let reply = |value| Handled { reply: Some(result(message, value)), ..Default::default() };
    match message["method"].as_str().unwrap_or_default() {
        "server/discover" => reply(json!({
            "resultType": "complete",
            "supportedVersions": ["2026-07-28"],
            "capabilities": { "tools": {} },
            "instructions": "Modern fixture: say hello.",
            "ttlMs": 0,
            "cacheScope": "private",
            "_meta": { "io.modelcontextprotocol/serverInfo": { "name": "modern-fixture", "version": "2.0.0" } },
        })),
        "ping" => reply(json!({})),
        "tools/list" => reply(json!({
            "resultType": "complete",
            "tools": [{ "name": "hello", "description": "Says hello.", "inputSchema": { "type": "object" }, "annotations": { "readOnlyHint": true } }],
        })),
        "tools/call" if message["params"]["name"] == json!("hello") => {
            let mut answer = text("hello from the 2026 server", false);
            answer["resultType"] = json!("complete");
            reply(answer)
        }
        other => Handled {
            reply: Some(failure(message, -32601, &format!("Method not found: {other}"))),
            ..Default::default()
        },
    }
}

// Programs: this test binary run again as a server over stdio.

/// The test that is a fixture program: `program(...)` runs the binary with its name.
pub const LEGACY_PROGRAM: &str = "mcp::fixtures::stdio_legacy";
pub const MODERN_PROGRAM: &str = "mcp::fixtures::stdio_modern";
/// An old server of the kind that ends when anything but `initialize` comes first (the probe).
pub const STRICT_PROGRAM: &str = "mcp::fixtures::stdio_strict";

/// The command and arguments that run a fixture program.
#[cfg(feature = "pc")]
pub fn program(name: &str) -> (String, Vec<String>) {
    let exe = std::env::current_exe().expect("the test binary").display().to_string();
    (exe, [name, "--exact", "--ignored", "--nocapture", "-q"].map(String::from).to_vec())
}

/// Whether this process was started as the named fixture (and not by a run of the ignored tests).
fn started_as(name: &str) -> bool {
    std::env::args().any(|arg| arg == name)
}

fn serve_stdio(handle: impl Fn(&Value) -> Handled) {
    let _ = std::fs::write("fixture.pid", std::process::id().to_string());
    eprintln!("fixture: started");
    let send = |message: &Value| {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{message}");
        let _ = out.flush();
    };
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else { continue };
        let handled = handle(&message);
        if handled.exit {
            std::process::exit(1);
        }
        if let Some(delay) = handled.delay {
            std::thread::sleep(delay);
        }
        if let Some(reply) = &handled.reply {
            send(reply);
        }
        for message in &handled.after {
            std::thread::sleep(Duration::from_millis(10));
            send(message);
        }
    }
}

#[test]
#[ignore = "a fixture: the MCP tests run it as a program"]
fn stdio_legacy() {
    if started_as(LEGACY_PROGRAM) {
        let fixture = Legacy::default();
        serve_stdio(|message| fixture.handle(message));
    }
}

#[test]
#[ignore = "a fixture: the MCP tests run it as a program"]
fn stdio_modern() {
    if started_as(MODERN_PROGRAM) {
        serve_stdio(modern);
    }
}

#[test]
#[ignore = "a fixture: the MCP tests run it as a program"]
fn stdio_strict() {
    if started_as(STRICT_PROGRAM) {
        let fixture = Legacy::default();
        let greeted = std::sync::atomic::AtomicBool::new(false);
        serve_stdio(|message| {
            if message["method"] == json!("initialize") {
                greeted.store(true, std::sync::atomic::Ordering::SeqCst);
            } else if !greeted.load(std::sync::atomic::Ordering::SeqCst) {
                return Handled { exit: true, ..Default::default() };
            }
            fixture.handle(message)
        });
    }
}

// Servers by address.

/// Serves a router on a free port of this computer; its base address.
pub async fn spawn(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a free port");
    let base = format!("http://{}", listener.local_addr().expect("an address"));
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    base
}

fn json_body(value: &Value) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], value.to_string()).into_response()
}

fn event_stream(messages: &[Value]) -> Response {
    let body: String = messages.iter().map(|message| format!("event: message\ndata: {message}\n\n")).collect();
    ([(header::CONTENT_TYPE, "text/event-stream")], body).into_response()
}

/// What an HTTP fixture saw of its requests.
#[derive(Default)]
pub struct Seen {
    pub headers: Mutex<Vec<HeaderMap>>,
}

impl Seen {
    /// The values a header had, request after request.
    pub fn values(&self, name: &str) -> Vec<String> {
        self.headers
            .lock()
            .unwrap()
            .iter()
            .filter_map(|headers| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string))
            .collect()
    }
}

struct LegacyHttp {
    fixture: Legacy,
    sessions: Mutex<HashSet<String>>,
    seen: Arc<Seen>,
}

/// The old-protocol server over Streamable HTTP at `/mcp`, with sessions as the 2025 SDKs keep them: a request
/// without a known session is refused with 400 (what such a server answers to `server/discover`).
pub fn legacy_http() -> (Router, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    let state =
        Arc::new(LegacyHttp { fixture: Legacy::default(), sessions: Mutex::new(HashSet::new()), seen: seen.clone() });
    let router = Router::new().route("/mcp", any(legacy_http_handler)).with_state(state);
    (router, seen)
}

async fn legacy_http_handler(
    State(state): State<Arc<LegacyHttp>>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    state.seen.headers.lock().unwrap().push(headers.clone());
    match method {
        Method::POST => {}
        Method::DELETE => return StatusCode::OK.into_response(),
        _ => return StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
    let Ok(message) = serde_json::from_slice::<Value>(&body) else { return StatusCode::BAD_REQUEST.into_response() };
    if message["method"] == json!("initialize") {
        let session = crate::utils::ids::uuid();
        state.sessions.lock().unwrap().insert(session.clone());
        let handled = state.fixture.handle(&message);
        let mut response = json_body(&handled.reply.unwrap_or(Value::Null));
        response.headers_mut().insert("mcp-session-id", HeaderValue::from_str(&session).unwrap());
        return response;
    }
    let known = headers
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|session| state.sessions.lock().unwrap().contains(session));
    if !known {
        let refusal = json!({ "jsonrpc": "2.0", "error": { "code": -32000, "message": "Bad Request: No valid session ID provided" }, "id": null });
        return (StatusCode::BAD_REQUEST, Json(refusal)).into_response();
    }
    if message.get("id").is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    let handled = state.fixture.handle(&message);
    if let Some(delay) = handled.delay {
        tokio::time::sleep(delay).await;
    }
    let Some(reply) = handled.reply else { return StatusCode::INTERNAL_SERVER_ERROR.into_response() };
    if handled.after.is_empty() {
        json_body(&reply)
    } else {
        // The server's news first, then the answer, on the request's own stream.
        let mut messages = handled.after;
        messages.push(reply);
        event_stream(&messages)
    }
}

/// The 2026-07-28 server over Streamable HTTP at `/mcp`: no sessions.
pub fn modern_http() -> Router {
    Router::new().route(
        "/mcp",
        any(|method: Method, body: Bytes| async move {
            if method != Method::POST {
                return StatusCode::METHOD_NOT_ALLOWED.into_response();
            }
            let Ok(message) = serde_json::from_slice::<Value>(&body) else {
                return StatusCode::BAD_REQUEST.into_response();
            };
            match modern(&message).reply {
                Some(reply) => json_body(&reply),
                None => StatusCode::ACCEPTED.into_response(),
            }
        }),
    )
}

struct SseServer {
    fixture: Legacy,
    sessions: Mutex<HashMap<String, mpsc::UnboundedSender<String>>>,
    seen: Arc<Seen>,
}

/// The old-protocol server over the old HTTP+SSE transport: GET `/sse` is the stream (its first event names where
/// to post), POST `/messages?sessionId=…` takes the messages. POST `/sse` is not allowed, as on such servers.
pub fn legacy_sse() -> (Router, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    let state =
        Arc::new(SseServer { fixture: Legacy::default(), sessions: Mutex::new(HashMap::new()), seen: seen.clone() });
    let router = Router::new().route("/sse", get(sse_stream)).route("/messages", post(sse_message)).with_state(state);
    (router, seen)
}

async fn sse_stream(State(state): State<Arc<SseServer>>, headers: HeaderMap) -> Response {
    state.seen.headers.lock().unwrap().push(headers);
    let session = crate::utils::ids::uuid();
    let (sender, receiver) = mpsc::unbounded_channel::<String>();
    let _ = sender.send(format!("event: endpoint\ndata: /messages?sessionId={session}\n\n"));
    state.sessions.lock().unwrap().insert(session, sender);
    let body = UnboundedReceiverStream::new(receiver).map(Ok::<_, Infallible>);
    ([(header::CONTENT_TYPE, "text/event-stream")], axum::body::Body::from_stream(body)).into_response()
}

async fn sse_message(
    State(state): State<Arc<SseServer>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    state.seen.headers.lock().unwrap().push(headers);
    let sender = query.get("sessionId").and_then(|id| state.sessions.lock().unwrap().get(id).cloned());
    let Some(sender) = sender else { return StatusCode::NOT_FOUND.into_response() };
    let Ok(message) = serde_json::from_slice::<Value>(&body) else { return StatusCode::BAD_REQUEST.into_response() };
    let handled = state.fixture.handle(&message);
    for message in handled.reply.iter().chain(&handled.after) {
        let _ = sender.send(format!("event: message\ndata: {message}\n\n"));
    }
    StatusCode::ACCEPTED.into_response()
}

/// A service in the cloud in miniature, at its base address: an MCP server at `/mcp` that wants `Bearer token-1`,
/// its protected-resource metadata, and an authorization server with dynamic registration, PKCE and a code.
pub struct Cloud {
    pub base: String,
    /// The access tokens it gave out.
    pub issued: Arc<Mutex<Vec<String>>>,
}

/// The cloud's base address and the tokens it gave out, for its handlers.
type CloudState = Arc<(String, Arc<Mutex<Vec<String>>>)>;

pub async fn cloud() -> Cloud {
    cloud_exchanging_after(Duration::ZERO).await
}

/// The same service, slow to exchange a code for tokens: a sign-in's return takes long enough to be left half-way.
pub async fn cloud_exchanging_after(pause: Duration) -> Cloud {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a free port");
    let base = format!("http://{}", listener.local_addr().expect("an address"));
    let issued = Arc::new(Mutex::new(Vec::new()));
    let state: CloudState = Arc::new((base.clone(), issued.clone()));
    let protected = |State(state): State<CloudState>| async move {
        Json(json!({ "resource": format!("{}/mcp", state.0), "authorization_servers": [state.0] }))
    };
    let metadata = |State(state): State<CloudState>| async move {
        let base = &state.0;
        Json(json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "registration_endpoint": format!("{base}/register"),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"],
        }))
    };
    let router = Router::new()
        .route("/.well-known/oauth-protected-resource", get(protected))
        .route("/.well-known/oauth-protected-resource/{*rest}", get(protected))
        .route("/.well-known/oauth-authorization-server", get(metadata))
        .route("/.well-known/openid-configuration", get(metadata))
        .route(
            "/register",
            post(|Json(mut body): Json<Value>| async move {
                body["client_id"] = json!("client-1");
                (StatusCode::CREATED, Json(body))
            }),
        )
        .route(
            "/authorize",
            get(|State(state): State<CloudState>, Query(query): Query<HashMap<String, String>>| async move {
                // The owner signed in: back to the redirect address with the code and the state.
                let Some(mut back) = query.get("redirect_uri").and_then(|uri| url::Url::parse(uri).ok()) else {
                    return StatusCode::BAD_REQUEST.into_response();
                };
                back.query_pairs_mut()
                    .append_pair("code", "code-1")
                    .append_pair("state", query.get("state").map(String::as_str).unwrap_or_default())
                    .append_pair("iss", &state.0);
                (StatusCode::FOUND, [(header::LOCATION, back.to_string())]).into_response()
            }),
        )
        .route(
            "/token",
            post(move |State(state): State<CloudState>, body: Bytes| async move {
                tokio::time::sleep(pause).await;
                let form: HashMap<String, String> = url::form_urlencoded::parse(&body).into_owned().collect();
                if form.get("code").map(String::as_str) != Some("code-1") || !form.contains_key("code_verifier") {
                    return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_grant" }))).into_response();
                }
                state.1.lock().unwrap().push("token-1".into());
                Json(json!({ "access_token": "token-1", "token_type": "Bearer", "expires_in": 3600, "refresh_token": "refresh-1" }))
                    .into_response()
            }),
        )
        .route(
            "/mcp",
            any(|State(state): State<CloudState>, method: Method, headers: HeaderMap, body: Bytes| async move {
                if headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) != Some("Bearer token-1") {
                    let challenge = format!("Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"", state.0);
                    return (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, challenge)], Json(json!({ "error": "unauthorized" })))
                        .into_response();
                }
                if method != Method::POST {
                    return StatusCode::METHOD_NOT_ALLOWED.into_response();
                }
                let Ok(message) = serde_json::from_slice::<Value>(&body) else { return StatusCode::BAD_REQUEST.into_response() };
                if message.get("id").is_none() {
                    return StatusCode::ACCEPTED.into_response();
                }
                let answer = match message["method"].as_str().unwrap_or_default() {
                    "initialize" => result(
                        &message,
                        json!({ "protocolVersion": message["params"]["protocolVersion"], "capabilities": { "tools": {} }, "serverInfo": { "name": "cloud", "version": "1" } }),
                    ),
                    "tools/list" => result(
                        &message,
                        json!({ "tools": [{ "name": "tasks", "description": "Lists tasks.", "inputSchema": { "type": "object" }, "annotations": { "readOnlyHint": true } }] }),
                    ),
                    _ => failure(&message, -32601, "Method not found"),
                };
                json_body(&answer)
            }),
        )
        .with_state(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Cloud { base, issued }
}
