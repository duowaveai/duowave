//! The tests of `home/join.rs` that need the whole gateway (`App`): its crate does not have it, the program does.
//!
//! The routes as a station centre serves them, from the home network.

use std::net::SocketAddr;

use axum::body::Body as HttpBody;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::app::App;
use crate::config::{Config, Env};

fn station() -> std::sync::Arc<App> {
    let env = Env::from_pairs([("STATE_DIR", ""), ("DUOWAVE_ROLE", "station")]);
    App::build(Config::for_tests(&env)).unwrap()
}

async fn send(
    app: &std::sync::Arc<App>,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Value,
    from: &str,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri).header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let mut request = request.body(HttpBody::from(body.to_string())).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(from.parse().unwrap(), 5000)));
    let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// A computer asks in from the Wi-Fi with no sign-in; the admin sees the code, a person who is not an admin and an
/// unsigned caller do not; the right code gives the pair's key — the one `placement/pair` gives the station's admin.
#[tokio::test]
async fn a_computer_joins_a_station_centre_by_the_code_only_the_admin_sees() {
    let app = station();
    let admin = app.access.auth.setup("admin-password-1", "Мама", "test").await.unwrap().unwrap().token;
    let (status, asked) =
        send(&app, "POST", "/api/v1/home/join", None, json!({ "kind": "turbo", "name": "Ноутбук" }), "192.168.1.20")
            .await;
    assert_eq!(status, StatusCode::CREATED, "{asked}");
    let (id, token) = (asked["message"]["id"].as_str().unwrap(), asked["message"]["token"].as_str().unwrap());
    assert!(asked["message"].get("code").is_none(), "the asker is never told the code: {asked}");

    let (status, _) = send(&app, "GET", "/api/v1/home/joins", None, Value::Null, "192.168.1.20").await;
    assert!(status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN, "{status}");
    let (status, list) = send(&app, "GET", "/api/v1/home/joins", Some(&admin), Value::Null, "192.168.1.21").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let code = list["message"][0]["code"].as_str().unwrap().to_string();
    assert_eq!(
        (list["message"][0]["name"].as_str(), list["message"][0]["kind"].as_str()),
        (Some("Ноутбук"), Some("turbo"))
    );

    let uri = format!("/api/v1/home/join/{id}/code");
    let (status, joined) =
        send(&app, "POST", &uri, None, json!({ "token": token, "code": code }), "192.168.1.20").await;
    assert_eq!(status, StatusCode::CREATED, "{joined}");
    assert_eq!(joined["message"]["grant"]["key"].as_str(), app.placement.peer_key(), "the pair's key");
}

/// «Разрешить» from the admin's panel: the asker's next look takes the key, once; «Это не я» closes another one.
#[tokio::test]
async fn the_admin_allows_or_turns_down_and_the_asker_reads_how_it_ended() {
    let app = station();
    let admin = app.access.auth.setup("admin-password-1", "Мама", "test").await.unwrap().unwrap().token;
    let ask = |name: &'static str| {
        let app = app.clone();
        async move {
            let body = json!({ "kind": "turbo", "name": name });
            let (_, asked) = send(&app, "POST", "/api/v1/home/join", None, body, "192.168.1.20").await;
            (
                asked["message"]["id"].as_str().unwrap().to_string(),
                asked["message"]["token"].as_str().unwrap().to_string(),
            )
        }
    };
    let (id, token) = ask("Ноутбук").await;
    let (status, _) =
        send(&app, "POST", &format!("/api/v1/home/join/{id}/approve"), Some(&admin), json!({}), "192.168.1.21").await;
    assert_eq!(status, StatusCode::CREATED);
    let look = format!("/api/v1/home/join/{id}?token={token}");
    let (_, first) = send(&app, "GET", &look, None, Value::Null, "192.168.1.20").await;
    assert_eq!(first["message"]["status"], "joined");
    assert_eq!(first["message"]["grant"]["key"].as_str(), app.placement.peer_key());
    let (_, second) = send(&app, "GET", &look, None, Value::Null, "192.168.1.20").await;
    assert!(second["message"].get("grant").is_none(), "once: {second}");

    let (other, other_token) = ask("Чужой").await;
    send(&app, "POST", &format!("/api/v1/home/join/{other}/deny"), Some(&admin), json!({}), "192.168.1.21").await;
    let (_, denied) =
        send(&app, "GET", &format!("/api/v1/home/join/{other}?token={other_token}"), None, Value::Null, "192.168.1.20")
            .await;
    assert_eq!(denied["message"]["status"], "denied");
    assert!(denied["message"].get("grant").is_none());
}
