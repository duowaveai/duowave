//! An MCP server for the station's conversation, over Streamable HTTP at `/mcp`, written by hand against the wire as
//! the 2025 SDKs serve it (sessions; a request without one refused with 400): a tool that reads (from the web) and
//! one that writes. The TS e2e ran its fixture as a Node program; this one needs nothing beside the test.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use serde_json::{json, Value};

use super::{lock, serve};

fn text(value: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": value }] })
}

fn result(message: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": message["id"], "result": result })
}

fn handle(message: &Value) -> Value {
    let params = &message["params"];
    match message["method"].as_str().unwrap_or_default() {
        "initialize" => result(
            message,
            json!({
                "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "fixture", "version": "1.0.0" },
                "instructions": "Fixture: lookup finds things, add_note writes them down.",
            }),
        ),
        "ping" => result(message, json!({})),
        "tools/list" => result(
            message,
            json!({ "tools": [
                {
                    "name": "lookup",
                    "description": "Finds a thing on the web.",
                    "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } },
                    "annotations": { "readOnlyHint": true, "openWorldHint": true },
                },
                {
                    "name": "add_note",
                    "description": "Adds a note.",
                    "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } },
                    "annotations": { "destructiveHint": false },
                },
            ] }),
        ),
        "tools/call" => match params["name"].as_str().unwrap_or_default() {
            "lookup" => {
                result(message, text(&format!("Нашлось: {}.", params["arguments"]["q"].as_str().unwrap_or(""))))
            }
            "add_note" => result(message, text("saved")),
            other => {
                json!({ "jsonrpc": "2.0", "id": message["id"], "error": { "code": -32602, "message": format!("Unknown tool: {other}") } })
            }
        },
        other => {
            json!({ "jsonrpc": "2.0", "id": message["id"], "error": { "code": -32601, "message": format!("Method not found: {other}") } })
        }
    }
}

#[derive(Default)]
struct Sessions(Mutex<HashSet<String>>);

async fn mcp(State(sessions): State<Arc<Sessions>>, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    match method {
        Method::POST => {}
        Method::DELETE => return StatusCode::OK.into_response(),
        _ => return StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
    let Ok(message) = serde_json::from_slice::<Value>(&body) else { return StatusCode::BAD_REQUEST.into_response() };
    let json_body = |value: &Value| ([(header::CONTENT_TYPE, "application/json")], value.to_string()).into_response();
    if message["method"] == json!("initialize") {
        let session = format!("session-{}", lock(&sessions.0).len() + 1);
        lock(&sessions.0).insert(session.clone());
        let mut response = json_body(&handle(&message));
        response.headers_mut().insert("mcp-session-id", HeaderValue::from_str(&session).unwrap());
        return response;
    }
    let known = headers
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|session| lock(&sessions.0).contains(session));
    if !known {
        let refusal = json!({ "jsonrpc": "2.0", "error": { "code": -32000, "message": "Bad Request: No valid session ID provided" }, "id": null });
        return (StatusCode::BAD_REQUEST, Json(refusal)).into_response();
    }
    if message.get("id").is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    json_body(&handle(&message))
}

/// The server's URL: `http://127.0.0.1:<port>/mcp`.
pub async fn start() -> String {
    let router = Router::new().route("/mcp", any(mcp)).with_state(Arc::new(Sessions::default()));
    format!("{}/mcp", serve(router).await)
}
