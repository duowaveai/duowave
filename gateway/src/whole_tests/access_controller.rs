//! The tests of `access/controller.rs` that need the whole gateway (`App`): its crate does not have it, the program
//! does.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body as HttpBody;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::access::controller::*;
use crate::app::App;
use crate::config::{Config, Env};

/// A request as it comes over the network from `from`.
async fn call(app: &Arc<App>, from: &str, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    send(app, from, method, uri, body.to_string(), None).await
}

/// The same, with a raw body and the token it is signed with, if any.
async fn send(
    app: &Arc<App>,
    from: &str,
    method: &str,
    uri: &str,
    body: String,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri).header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let mut request = request.body(HttpBody::from(body)).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(from.parse().unwrap(), 5000)));
    let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn memory_gateway() -> Arc<App> {
    App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap()
}

#[tokio::test]
async fn counts_the_guesses_of_the_setup_code_and_makes_their_source_wait() {
    let app = memory_gateway();
    let guess = json!({"password": "admin-password-1", "code": "1000-1000"});
    for _ in 0..5 {
        let (status, _) = call(&app, "192.168.1.20", "POST", "/api/v1/auth/setup", guess.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    let right = json!({"password": "admin-password-1", "code": app.access.auth.setup_code()});
    let (status, body) = call(&app, "192.168.1.20", "POST", "/api/v1/auth/setup", right.clone()).await;
    assert_eq!(
        (status, body["message"].as_str()),
        (StatusCode::TOO_MANY_REQUESTS, Some("Слишком много попыток — подождите 15 мин."))
    );
    let (status, _) = call(&app, "192.168.1.21", "POST", "/api/v1/auth/setup", right).await;
    assert_eq!(status, StatusCode::CREATED, "another device of the house is not the guesser");
}

#[tokio::test]
async fn reads_no_bigger_body_than_express_did() {
    let app = memory_gateway();
    let name = "Я".repeat(BODY_LIMIT);
    let body = json!({"name": name, "password": "wrong-password"}).to_string();
    let (status, _) = send(&app, "127.0.0.1", "POST", "/api/v1/auth/login", body, None).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let (status, _) = call(&app, "127.0.0.1", "POST", "/api/v1/auth/login", json!({"password": "x"})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "a small one is read");
}

#[tokio::test]
async fn a_sign_in_the_database_cannot_check_is_the_servers_error_not_sign_in_again() {
    let app = memory_gateway();
    let signed_in = app.access.auth.setup("admin-password-1", "admin", "setup").await.unwrap().unwrap();
    let token = Some(signed_in.token.as_str());
    app.db.with(|c| c.execute_batch("DROP TABLE user_settings; DROP TABLE users;")).unwrap();
    let (status, body) = send(&app, "127.0.0.1", "GET", "/api/v1/auth/me", String::new(), token).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some("Internal server error")));
    let (status, _) = send(&app, "127.0.0.1", "GET", "/api/v1/household", String::new(), token).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "a right is not checked either");
    // A public route takes it for nobody and answers.
    let (status, _) = send(&app, "8.8.8.8", "GET", "/health", String::new(), token).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn the_first_run_from_another_device_at_home_needs_the_code_from_the_log() {
    let app = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
    let phone = "192.168.1.20";
    let (_, state) = call(&app, phone, "GET", "/api/v1/auth/state", Value::Null).await;
    assert_eq!(
        state["message"],
        json!({"configured": false, "accounts": 0, "needsCode": true, "station": cfg!(feature = "station")})
    );
    let (status, body) = call(&app, phone, "POST", "/api/v1/auth/setup", json!({"password": "admin-password-1"})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["message"], "Нужен код настройки — он в журнале DuoWave (logs/duowave.log).");
    let (status, body) = call(&app, "8.8.8.8", "POST", "/api/v1/auth/setup", json!({"password": "x"})).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::FORBIDDEN, Some("Только из домашней сети.")));

    let code = app.access.auth.setup_code();
    let setup = json!({"password": "admin-password-1", "name": "  Мама  ", "code": code});
    let (status, body) = call(&app, phone, "POST", "/api/v1/auth/setup", setup).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["message"]["user"]["name"], "Мама");
    let (_, state) = call(&app, phone, "GET", "/api/v1/auth/state", Value::Null).await;
    assert_eq!(
        state["message"],
        json!({"configured": true, "accounts": 1, "needsCode": false, "station": cfg!(feature = "station")})
    );

    // From outside the home the panel answers only with PANEL_REMOTE; its first question is open.
    let (status, body) = call(&app, "8.8.8.8", "POST", "/api/v1/auth/login", json!({"password": "x"})).await;
    assert_eq!(
        (status, body["message"].as_str()),
        (StatusCode::FORBIDDEN, Some("Вход вне дома выключен (PANEL_REMOTE)."))
    );
    assert_eq!(call(&app, "8.8.8.8", "GET", "/api/v1/auth/state", Value::Null).await.0, StatusCode::OK);
}

#[tokio::test]
async fn opens_the_panel_from_outside_when_the_owner_allows_it() {
    let env = Env::from_pairs([("STATE_DIR", ""), ("PANEL_REMOTE", "true")]);
    let app = App::build(Config::for_tests(&env)).unwrap();
    let signed_in = app.access.auth.setup("admin-password-1", "admin", "setup").await.unwrap().unwrap();
    let mut request = Request::builder()
        .uri("/api/v1/auth/me")
        .header("authorization", format!("Bearer {}", signed_in.token))
        .body(HttpBody::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new("8.8.8.8".parse().unwrap(), 5000)));
    let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (status, _) =
        call(&app, "8.8.8.8", "POST", "/api/v1/auth/login", json!({"password": "admin-password-1"})).await;
    assert_eq!(status, StatusCode::OK, "the login answers from outside too");
}
