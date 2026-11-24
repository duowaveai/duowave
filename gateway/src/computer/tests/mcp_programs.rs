//! The computer's part of whole_tests/mcp_controller.rs: what only a computer has, tested with the whole gateway built.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;

use crate::app::App;
use crate::mcp::fixtures;
use crate::mcp::mcp_controller::*;
use crate::mcp::mcp_service::tests::{until, READY};
use crate::platform::http::Uses;
use crate::whole_tests::mcp_controller::*;

fn alive(pid: i32) -> bool {
    crate::mcp::programs::alive(pid)
}

async fn add_program(app: &Arc<App>) -> serde_json::Value {
    let (command, args) = fixtures::program(crate::computer::tests::mcp_fixtures::LEGACY_PROGRAM);
    let given = json!({
        "transport": "stdio",
        "title": "Fixture",
        "command": command,
        "args": args,
        "env": { "FIXTURE_VAR": "program-secret-4242" },
    });
    let (status, added) = answer(add(Uses::of(&app), admin(app), body(given)).await).await;
    assert_eq!(status, StatusCode::CREATED);
    added["message"].clone()
}

#[tokio::test]
async fn adds_a_program_with_a_sealed_variable_shows_it_among_the_connectors_never_shows_the_value() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path());
    let added = add_program(&app).await;
    assert_eq!(added["id"], json!("fixture"));
    assert_eq!(added["transport"], json!("stdio"));
    assert_eq!(added["secrets"], json!([{ "kind": "env", "name": "FIXTURE_VAR", "set": true, "hint": "…4242" }]));
    until("the fixture is up", || ready(&app, "fixture"), READY).await;
    let connector = app.mcp.states().into_iter().find(|server| server.id == "fixture").unwrap();
    assert_eq!((connector.title.as_str(), connector.cloud, connector.tools), ("Fixture", true, 0));
    let (status, shown) = answer(view(Uses::of(&app), admin(&app)).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(shown["message"]["servers"][0]["state"], json!("ready"));
    assert!(!shown.to_string().contains("program-secret"));
    assert!(!serde_json::to_string(&app.mcp.states()).unwrap().contains("program-secret"));
    // Among the connectors as the gateway lists them, over its own routes; the value on none of them.
    let router = crate::app::router(app.clone());
    let token = sign_in(&app).await;
    let (status, listed) = send(&router, "GET", "/api/v1/connectors", [127, 0, 0, 1], Some(&token), "".into()).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let listed: serde_json::Value = serde_json::from_str(&listed).unwrap();
    let entry = listed["message"]["connectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["manifest"]["id"] == json!("mcp:fixture"))
        .cloned()
        .expect("the server among the connectors");
    let manifest = &entry["manifest"];
    assert_eq!(
        (&manifest["kind"], &manifest["title"], &manifest["cloud"]),
        (&json!("mcp"), &json!("Fixture"), &json!(true))
    );
    assert_eq!(entry["status"], json!({ "state": "ready", "detail": "инструментов включено: 0" }));
    for route in ["/api/v1/mcp", "/api/v1/connectors", "/health"] {
        let (status, text) = send(&router, "GET", route, [127, 0, 0, 1], Some(&token), "".into()).await;
        assert_eq!(status, StatusCode::OK, "{route}: {text}");
        assert!(!text.contains("program-secret"), "{route}: {text}");
    }
    let pid: i32 = std::fs::read_to_string(dir.path().join("mcp").join("fixture").join("fixture.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(pid));
    app.shutdown().await;
}

#[tokio::test]
async fn stops_the_program_when_the_gateway_stops() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path());
    add_program(&app).await;
    until("the fixture is up", || ready(&app, "fixture"), READY).await;
    let pid: i32 = std::fs::read_to_string(dir.path().join("mcp").join("fixture").join("fixture.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(pid));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(crate::app::serve(app.clone(), listener, async move {
        let _ = stopped.await;
    }));
    let _ = stop.send(());
    serving.await.unwrap().unwrap();
    until("the program outlived the gateway", || !alive(pid), Duration::from_secs(5)).await;
}

/// The first admin, signed in as the panel's wizard makes one; the token.
async fn sign_in(app: &Arc<App>) -> String {
    app.access.auth.setup("admin-password-1", "admin", "setup").await.unwrap().unwrap().token
}
