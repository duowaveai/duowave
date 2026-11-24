//! The tests of `mcp/mcp_controller.rs` that need the whole gateway (`App`): its crate does not have it, the program
//! does.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::Router;

use crate::platform::http::{Body, Caller, Uses};

use axum::extract::ConnectInfo;
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use serde_json::json;
use tower::ServiceExt;

use crate::app::App;
use crate::config::{Config, Env};
use crate::core::identity::principal::{api_key_principal, user_principal};
use crate::core::identity::Role;
use crate::mcp::fixtures;
use crate::mcp::mcp_controller::*;
use crate::mcp::mcp_service::tests::{until, READY};
use crate::mcp::mcp_service::ServerState;
use axum::response::IntoResponse;

// The Node e2e signed people in over HTTP; access is ported on its own (the Rust stub signs nobody in), so the
// handlers here are called as the guard would let these people through.

pub(crate) fn app(dir: &std::path::Path) -> Arc<App> {
    let env = Env::from_pairs([("STATE_DIR", dir.to_str().unwrap())]);
    App::build(Config::for_tests(&env)).unwrap()
}

pub(crate) fn admin(app: &Arc<App>) -> Caller {
    Caller::local(app.clone(), Some(user_principal("u1", "admin", Role::Admin, None)))
}

pub(crate) fn member(app: &Arc<App>) -> Caller {
    Caller::local(app.clone(), Some(user_principal("u2", "Аня", Role::Member, None)))
}

pub(crate) fn body<T: DeserializeOwned>(value: serde_json::Value) -> Body<T> {
    Body(serde_json::from_value(value).unwrap())
}

pub(crate) async fn answer(response: impl IntoResponse) -> (StatusCode, serde_json::Value) {
    let response = response.into_response();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
}

pub(crate) fn ready(app: &Arc<App>, id: &str) -> bool {
    app.mcp.states().iter().any(|server| server.id == id && server.state == ServerState::Ready)
}

#[tokio::test]
async fn lets_the_household_see_the_servers_but_not_add_change_or_try_them() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path());
    let (router, _) = fixtures::legacy_http();
    let base = fixtures::spawn(router).await;
    let given = json!({ "transport": "http", "title": "Fixture", "url": format!("{base}/mcp") });
    let (status, _) = answer(add(Uses::of(&app), admin(&app), body(given)).await).await;
    assert_eq!(status, StatusCode::CREATED);
    until("the fixture is up", || ready(&app, "fixture"), READY).await;

    assert_eq!(answer(view(Uses::of(&app), member(&app)).await).await.0, StatusCode::OK);
    let program = body(json!({ "transport": "stdio", "command": "echo" }));
    assert_eq!(answer(add(Uses::of(&app), member(&app), program).await).await.0, StatusCode::FORBIDDEN);
    let tool = Path(("fixture".to_string(), "echo".to_string()));
    let on = body(json!({ "on": true }));
    assert_eq!(answer(set_tool(Uses::of(&app), member(&app), tool, on).await).await.0, StatusCode::FORBIDDEN);
    let tool = Path(("fixture".to_string(), "echo".to_string()));
    assert_eq!(
        answer(try_tool(Uses::of(&app), member(&app), tool, body(json!({}))).await).await.0,
        StatusCode::FORBIDDEN
    );
    // A key may hold every right and still adds nothing: a program is someone's decision.
    let key = Caller::local(app.clone(), Some(api_key_principal("k1", "script", &["*".into()])));
    let program = body(json!({ "transport": "stdio", "command": "echo" }));
    let refused = add(Uses::of(&app), key, program).await.err().unwrap();
    assert_eq!(refused.message, "Это для людей, не для ключей.");

    let tool = Path(("fixture".to_string(), "echo".to_string()));
    let tried =
        answer(try_tool(Uses::of(&app), admin(&app), tool, body(json!({ "arguments": { "text": "да" } }))).await).await;
    assert_eq!(tried.0, StatusCode::OK);
    assert_eq!(tried.1["message"]["content"], json!("echo: да"));
    assert_eq!(tried.1["message"]["isError"], json!(false));
    app.shutdown().await;
}

pub(crate) async fn request(router: &Router, method: &str, uri: &str, from: [u8; 4]) -> (StatusCode, String) {
    send(router, method, uri, from, None, String::new()).await
}

/// A request as it comes over the network from `from`, signed with the token when there is one.
pub(crate) async fn send(
    router: &Router,
    method: &str,
    uri: &str,
    from: [u8; 4],
    token: Option<&str>,
    body: String,
) -> (StatusCode, String) {
    let mut request = axum::http::Request::builder().method(method).uri(uri);
    if !body.is_empty() {
        request = request.header("content-type", "application/json");
    }
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let mut request = request.body(axum::body::Body::from(body)).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::from((from, 5000))));
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn reads_no_bigger_body_than_express_did_before_it_asks_who_calls() {
    let dir = tempfile::tempdir().unwrap();
    let router = crate::app::router(app(dir.path()));
    // An object of many keys in a loose field, from someone who has not signed in: refused at the size, as
    // Express refused it, before anything parses it (axum alone would read up to 2 MB).
    let keys: Vec<String> = (0..20_000).map(|n| format!("\"{n:06}\":0")).collect();
    let big = format!("{{\"question\":{{{}}}}}", keys.join(","));
    assert!(big.len() > crate::mcp::BODY_LIMIT && big.len() < 2 * 1024 * 1024);
    for uri in ["/api/v1/brain/ask", "/api/v1/mcp/servers", "/api/v1/mcp/import", "/api/v1/mcp/oauth/complete"] {
        let (status, text) = send(&router, "POST", uri, [127, 0, 0, 1], None, big.clone()).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{uri}: {text}");
    }
    let (status, text) = send(&router, "PATCH", "/api/v1/mcp/servers/x", [127, 0, 0, 1], None, big.clone()).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{text}");
    let small = json!({ "question": "Ты здесь?" }).to_string();
    let (status, text) = send(&router, "POST", "/api/v1/brain/ask", [127, 0, 0, 1], None, small).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "a small one is read: {text}");
}

#[tokio::test]
async fn answers_the_panel_after_sign_in_only_and_the_services_return_with_a_page() {
    let dir = tempfile::tempdir().unwrap();
    let router = crate::app::router(app(dir.path()));
    let (status, text) = request(&router, "GET", "/api/v1/mcp", [127, 0, 0, 1]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(text.contains("Нужно войти."), "{text}");
    assert_eq!(request(&router, "DELETE", "/api/v1/mcp/servers/x", [127, 0, 0, 1]).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(request(&router, "POST", "/api/v1/brain/ask", [127, 0, 0, 1]).await.0, StatusCode::UNAUTHORIZED);
    let (status, page) = request(&router, "GET", "/api/v1/mcp/oauth/callback?state=x&code=y", [127, 0, 0, 1]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(page.starts_with("<!doctype html>") && page.contains("Этот вход устарел или начат не здесь"), "{page}");
    let (status, text) = request(&router, "GET", "/api/v1/mcp/oauth/callback?state=x", [8, 8, 8, 8]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(text.contains("Только из домашней сети."), "{text}");
}
