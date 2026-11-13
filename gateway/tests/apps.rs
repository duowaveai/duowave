//! Apps end to end (docs/plans/plugins.md, stage 9): a package of `kind: app` with the build of this platform — the
//! reference app `duowave-echo-app` — switched on by the admin; the supervisor starts its program, the gateway calls its
//! tool over localhost HTTP, the app reaches the Host API with its own token; a fall is answered with a new start; a
//! build whose hash is not the manifest's never starts.

mod support;

use std::path::Path;
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{json, Value};
use sha2::Digest;
use support::{bearer, message, send, Gateway};

/// The reference app as a package in the gateway's DATA_DIR, its hash in the manifest (or a wrong one).
fn install_app(data: &Path, version: &str, hash: Option<&str>) {
    let dir = data.join("packages/echo-app").join(version);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let program = std::fs::read(env!("CARGO_BIN_EXE_duowave-echo-app")).unwrap();
    std::fs::write(dir.join("bin/echo-app"), &program).unwrap();
    let sha = hash.map(str::to_string).unwrap_or_else(|| hex::encode(sha2::Sha256::digest(&program)));
    let manifest = json!({
        "schema": 1, "kind": "app", "runtime": "process", "id": "echo-app", "version": version,
        "title": "Эхо-приложение", "api": "^2", "runs": ["pc", "station", "stationServer"],
        "budget": { "memoryMb": 64 }, "permissions": [],
        "targets": { duowave::plugins::manifest::platform(): { "file": "bin/echo-app", "sha256": sha } },
        "health": { "path": "/health", "intervalS": 5 },
        "contributes": { "tools": [
            { "name": "echo_app", "description": "Повторяет вход", "handler": "echo" },
            { "name": "echo_crash", "description": "Падает", "handler": "crash" }
        ] }
    });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
}

async fn try_tool(gateway: &Gateway, admin: &str, tool: &str, arguments: Value) -> (StatusCode, Value) {
    let (name, value) = bearer(admin);
    send(
        gateway
            .post(&format!("/api/v1/packages/echo-app/tools/{tool}/try"))
            .header(name, value)
            .json(&json!({ "arguments": arguments })),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runs_an_app_under_the_supervisor_calls_it_and_starts_it_again_after_a_fall() {
    let gateway = Gateway::start(&[]).await;
    install_app(gateway.dir.path(), "1.0.0", None);
    let setup = message(
        gateway.post("/api/v1/auth/setup").json(&json!({"password": "apps-admin-password"})),
        StatusCode::CREATED,
    );
    let admin = setup.await["token"].as_str().unwrap().to_string();
    let (name, value) = bearer(&admin);
    let (code, enabled) =
        send(gateway.post("/api/v1/packages/echo-app/enable").header(name.clone(), value.clone())).await;
    assert_eq!(code, StatusCode::OK, "{enabled}");
    assert_eq!((&enabled["message"]["kind"], &enabled["message"]["state"]), (&json!("app"), &json!("loaded")));

    // Its tool, over HTTP; it stored its input through the Host API with its own token.
    let (code, answer) = try_tool(&gateway, &admin, "echo_app", json!({ "word": "привет" })).await;
    assert_eq!(code, StatusCode::OK, "{answer}");
    let content: Value = serde_json::from_str(answer["message"]["content"].as_str().unwrap()).unwrap();
    assert_eq!((&content["echo"]["word"], &content["stored"]), (&json!("привет"), &json!(true)), "{content}");
    let first_pid = content["pid"].as_u64().unwrap();
    let view =
        message(gateway.get("/api/v1/packages/echo-app").header(name.clone(), value.clone()), StatusCode::OK).await;
    assert!(view["storageUsed"].as_u64().unwrap() > 0, "{view}");
    assert!(view["memory"]["allocated"].as_u64().unwrap() > 0, "its memory is counted for real: {view}");

    // Its own sound reaches the station through our /media: its /stream/…, nothing else of its server.
    let sound = gateway.get("/media/apps/echo-app/song.mp3").send().await.unwrap();
    assert_eq!(sound.status(), StatusCode::OK);
    assert_eq!(sound.headers()["content-type"], "audio/mpeg");
    assert_eq!(sound.bytes().await.unwrap().as_ref(), &[0xFF, 0xFB, 0x90, 0x64, 0, 0, 0, 0]);
    for path in ["/media/apps/echo-app/.hidden", "/media/apps/nobody/song.mp3", "/media/apps/echo-app"] {
        assert_eq!(gateway.get(path).send().await.unwrap().status(), StatusCode::NOT_FOUND, "{path}");
    }

    // A fall: the supervisor starts it again.
    let _ = try_tool(&gateway, &admin, "echo_crash", json!({})).await;
    let mut again = None;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let (code, answer) = try_tool(&gateway, &admin, "echo_app", json!({ "word": "снова" })).await;
        if code == StatusCode::OK {
            again = Some(answer);
            break;
        }
    }
    let again = again.expect("started again after its fall");
    let content: Value = serde_json::from_str(again["message"]["content"].as_str().unwrap()).unwrap();
    assert_ne!(content["pid"].as_u64().unwrap(), first_pid, "a new process");
    let journal = message(
        gateway.get("/api/v1/workshop/drafts/echo-app/journal").header(name.clone(), value.clone()),
        StatusCode::OK,
    )
    .await;
    let lines: Vec<String> =
        journal["lines"].as_array().unwrap().iter().map(|line| line["message"].as_str().unwrap().to_string()).collect();
    assert!(lines.iter().any(|line| line.contains("новый запуск")), "{lines:?}");

    // A build whose hash is not the manifest's never starts.
    install_app(gateway.dir.path(), "1.1.0", Some(&"0".repeat(64)));
    let (code, refused) =
        send(gateway.post("/api/v1/packages/echo-app/reload").header(name.clone(), value.clone())).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{refused}");
    assert!(refused["message"].as_str().unwrap().contains("хеш не совпал"), "{refused}");
    let (code, _) = send(gateway.post("/api/v1/packages/echo-app/disable").header(name, value)).await;
    assert_eq!(code, StatusCode::OK);
    let gone = gateway.get("/media/apps/echo-app/song.mp3").send().await.unwrap();
    assert_eq!(gone.status(), StatusCode::NOT_FOUND, "an app that is off has no sound");
    gateway.stop().await;
}
