//! Apps end to end (docs/plans/plugins.md, stage 9): a package of `kind: app` with the build of this platform — the
//! reference app `duowave-echo-app` — switched on by the admin; the supervisor starts its program, the gateway calls its
//! tool over localhost HTTP, the app reaches the Host API with its own token; a fall is answered with a new start; a
//! build whose hash is not the manifest's never starts. And a long task (src/tasks): the app «counts for a long time»,
//! the panel's API sees how far it got, cancels it, and a task of an app that fell or was switched off is interrupted.

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
            { "name": "echo_crash", "description": "Падает", "handler": "crash" },
            { "name": "echo_count", "description": "Долго считает", "handler": "count" }
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

/// The app's tasks as the panel's API gives them.
async fn tasks_of(gateway: &Gateway, admin: &str) -> Vec<Value> {
    let (name, value) = bearer(admin);
    let view = message(gateway.get("/api/v1/tasks?app=echo-app").header(name, value), StatusCode::OK).await;
    view["tasks"].as_array().unwrap().clone()
}

/// Starts the count through the app's tool; the task's id, or the gateway's refusal as the app passed it on.
async fn count(gateway: &Gateway, admin: &str, arguments: Value) -> Result<String, Value> {
    let (code, answer) = try_tool(gateway, admin, "echo_count", arguments).await;
    assert_eq!(code, StatusCode::OK, "{answer}");
    let content: Value = serde_json::from_str(answer["message"]["content"].as_str().unwrap()).unwrap();
    match content["task"].as_str() {
        Some(id) => Ok(id.to_string()),
        None => Err(content["refused"].clone()),
    }
}

/// Waits until the task is in `state`; the task then.
async fn task_in(gateway: &Gateway, admin: &str, id: &str, state: &str) -> Value {
    for _ in 0..100 {
        let found = tasks_of(gateway, admin).await.into_iter().find(|task| task["id"] == json!(id));
        if let Some(task) = found.filter(|task| task["state"] == json!(state)) {
            return task;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the task did not come to «{state}»: {:?}", tasks_of(gateway, admin).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_runs_a_long_task_the_panel_follows_and_cancels() {
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

    // The count answers at once with its task; the panel sees it go on, in percent and in words.
    let id = count(&gateway, &admin, json!({ "seconds": 2 })).await.expect("the task started");
    let running = task_in(&gateway, &admin, &id, "running").await;
    assert_eq!((&running["app"], &running["title"]), (&json!("echo-app"), &json!("Долго считаю: 2 с")));
    // One at a time: the gateway refuses a second, in words the app passes on.
    let refused = count(&gateway, &admin, json!({ "seconds": 2 })).await.unwrap_err();
    assert!(refused["error"].as_str().unwrap().contains("уже идёт задача «Долго считаю: 2 с»"), "{refused}");
    let mut seen = Vec::new();
    for _ in 0..80 {
        let task = tasks_of(&gateway, &admin).await.into_iter().find(|task| task["id"] == json!(id)).unwrap();
        if task["state"] != json!("running") {
            break;
        }
        seen.extend(task["percent"].as_u64());
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(seen.iter().any(|percent| (1..100).contains(percent)), "its progress was seen on the way: {seen:?}");
    let done = task_in(&gateway, &admin, &id, "done").await;
    assert_eq!((&done["percent"], &done["result"]), (&json!(100), &json!({ "counted": 4, "seconds": 2 })), "{done}");
    let one = message(gateway.get(&format!("/api/v1/tasks/{id}")).header(name.clone(), value.clone()), StatusCode::OK);
    let log: Vec<String> =
        one.await["log"].as_array().unwrap().iter().map(|step| step["text"].as_str().unwrap().to_string()).collect();
    assert_eq!((log.first().map(String::as_str), log.last().map(String::as_str)), (Some("Разминаюсь"), Some("Готово")));

    // «Отменить»: the app hears it with its next report and ends the task itself.
    let id = count(&gateway, &admin, json!({ "seconds": 120 })).await.expect("the slot is free again");
    let (code, asked) =
        send(gateway.post(&format!("/api/v1/tasks/{id}/cancel")).header(name.clone(), value.clone())).await;
    assert_eq!(code, StatusCode::OK, "{asked}");
    assert_eq!(asked["message"]["cancelRequested"], json!(true), "{asked}");
    let cancelled = task_in(&gateway, &admin, &id, "cancelled").await;
    assert_eq!(cancelled["cancelRequested"], json!(false), "{cancelled}");

    // A task that fails says why.
    let id = count(&gateway, &admin, json!({ "seconds": 2, "fail": true })).await.unwrap();
    let failed = task_in(&gateway, &admin, &id, "failed").await;
    assert_eq!(failed["error"], json!("Сбился со счёта на половине."));

    // The app fell at its task: the task is interrupted, with the reason; the app starts again and may count anew.
    let id = count(&gateway, &admin, json!({ "seconds": 120 })).await.unwrap();
    let _ = try_tool(&gateway, &admin, "echo_crash", json!({})).await;
    let cut = task_in(&gateway, &admin, &id, "interrupted").await;
    assert!(cut["error"].as_str().unwrap().starts_with("Приложение упало"), "{cut}");
    let mut again = None;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let (code, answer) = try_tool(&gateway, &admin, "echo_count", json!({ "seconds": 120 })).await;
        if code == StatusCode::OK {
            let content: Value = serde_json::from_str(answer["message"]["content"].as_str().unwrap()).unwrap();
            again = content["task"].as_str().map(str::to_string);
            break;
        }
    }
    let id = again.expect("started again after its fall, and counts");

    // Switched off at its task: interrupted too. And nobody without a key reads the tasks.
    let (code, _) = send(gateway.post("/api/v1/packages/echo-app/disable").header(name.clone(), value.clone())).await;
    assert_eq!(code, StatusCode::OK);
    let cut = task_in(&gateway, &admin, &id, "interrupted").await;
    assert_eq!(cut["error"], json!("Приложение остановлено."));
    assert_eq!(gateway.get("/api/v1/tasks").send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
    let no_app = gateway.post("/api/v1/apps/host/tasks").json(&json!({ "title": "чужая" })).send().await.unwrap();
    assert_eq!(no_app.status(), StatusCode::UNAUTHORIZED, "the Host API is the app's alone, by its token");
    gateway.stop().await;
}
