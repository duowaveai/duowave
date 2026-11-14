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

/// A Node on this machine for the test: the gateway's own is downloaded at the first need, here one is given instead
/// (`DUOWAVE_NODE`, as the Docker image does). Without one the test says so and passes.
fn some_node() -> Option<String> {
    // As PATH names it, not where the link leads: npm lies beside that name (Homebrew keeps it out of Node's cellar).
    let output = std::process::Command::new("sh").args(["-c", "command -v node"]).output();
    let path = output.ok().filter(|output| output.status.success())?;
    Some(String::from_utf8_lossy(&path.stdout).trim().to_string()).filter(|path| !path.is_empty())
}

/// An app of `runtime: node` (docs/plans/feed.md, stage 2): the example «Привет на Node» from docs/plugins/apps — put
/// in place, its dependencies put from its lock (npm ci), started on the gateway's Node under the same supervisor,
/// called, reaching the Host API with its token, started again after a fall, and removed with its dependencies.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runs_an_app_on_node_calls_it_starts_it_again_after_a_fall_and_removes_it_whole() {
    let Some(node) = some_node() else {
        eprintln!("no node on this machine: the test of runtime node is skipped");
        return;
    };
    let gateway = Gateway::start(&[("DUOWAVE_NODE", &node)]).await;
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/plugins/apps/hello-node");
    let dir = gateway.dir.path().join("packages/hello-node/1.0.0");
    std::fs::create_dir_all(&dir).unwrap();
    for file in ["manifest.json", "server.mjs", "package.json", "package-lock.json"] {
        std::fs::copy(example.join(file), dir.join(file)).unwrap();
    }
    let setup = message(
        gateway.post("/api/v1/auth/setup").json(&json!({"password": "node-admin-password"})),
        StatusCode::CREATED,
    );
    let admin = setup.await["token"].as_str().unwrap().to_string();
    let (name, value) = bearer(&admin);
    let (code, enabled) =
        send(gateway.post("/api/v1/packages/hello-node/enable").header(name.clone(), value.clone())).await;
    assert_eq!(code, StatusCode::OK, "{enabled}");
    assert_eq!((&enabled["message"]["kind"], &enabled["message"]["state"]), (&json!("app"), &json!("loaded")));
    assert!(dir.join("node_modules/.duowave-dependencies").is_file(), "npm ci ran from the lock");

    let hello = |word: &'static str| {
        let (name, value) = (name.clone(), value.clone());
        let gateway = &gateway;
        async move {
            send(
                gateway
                    .post("/api/v1/packages/hello-node/tools/hello_node/try")
                    .header(name, value)
                    .json(&json!({ "arguments": { "name": word } })),
            )
            .await
        }
    };
    let (code, answer) = hello("Аня").await;
    assert_eq!(code, StatusCode::OK, "{answer}");
    let content: Value = serde_json::from_str(answer["message"]["content"].as_str().unwrap()).unwrap();
    assert!(content["text"].as_str().unwrap().starts_with("Привет, Аня! Это Node v"), "{content}");
    assert_eq!(content["stored"], json!(true), "its token reaches the Host API: {content}");
    let files = content["files"].as_str().unwrap_or_default();
    assert!(files.ends_with("plugin-files/hello-node"), "its own folder on the bus: {content}");
    let first_pid = content["pid"].as_u64().unwrap();

    // A fall: started again.
    let _ = send(
        gateway
            .post("/api/v1/packages/hello-node/tools/hello_crash/try")
            .header(name.clone(), value.clone())
            .json(&json!({ "arguments": {} })),
    )
    .await;
    let mut again = None;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let (code, answer) = hello("снова").await;
        if code == StatusCode::OK {
            again = Some(answer);
            break;
        }
    }
    let again = again.expect("started again after its fall");
    let content: Value = serde_json::from_str(again["message"]["content"].as_str().unwrap()).unwrap();
    assert_ne!(content["pid"].as_u64().unwrap(), first_pid, "a new process");

    // The app as an MCP server for one's own Claude Code: its tools and its folder's files.
    let rpc = |body: Value| {
        let (name, value) = (name.clone(), value.clone());
        let gateway = &gateway;
        async move {
            let answer = gateway.post("/mcp/apps/hello-node").header(name, value).json(&body).send().await.unwrap();
            assert_eq!(answer.status(), StatusCode::OK);
            answer.json::<Value>().await.unwrap()
        }
    };
    let listed = rpc(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).await;
    let tools: Vec<&str> =
        listed["result"]["tools"].as_array().unwrap().iter().map(|tool| tool["name"].as_str().unwrap()).collect();
    assert!(
        tools.contains(&"hello_node") && tools.contains(&"write_file") && tools.contains(&"list_files"),
        "{tools:?}"
    );
    let call = |name: &str, arguments: Value| json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
    let wrote = rpc(call("write_file", json!({ "path": "notes/plan.md", "text": "план серии" }))).await;
    assert_eq!(wrote["result"]["isError"], json!(false), "{wrote}");
    let read = rpc(call("read_file", json!({ "path": "notes/plan.md" }))).await;
    assert_eq!(read["result"]["content"][0]["text"], json!("план серии"));
    let outside = rpc(call("read_file", json!({ "path": "../hello-node/../x" }))).await;
    assert_eq!(outside["result"]["isError"], json!(true), "nothing outside its folder");
    let greeted = rpc(call("hello_node", json!({ "name": "Код" }))).await;
    assert!(greeted["result"]["content"][0]["text"].as_str().unwrap().contains("Привет, Код!"), "{greeted}");

    // Removed: its folder goes with the dependencies npm put.
    let (code, removed) = send(gateway.delete("/api/v1/packages/hello-node").header(name, value)).await;
    assert_eq!(code, StatusCode::OK, "{removed}");
    assert!(!gateway.dir.path().join("packages/hello-node").exists(), "no folder, no node_modules");
    gateway.stop().await;
}

/// Media of an app's folder to its window (docs/plans/feed.md, «Один движок показа»): the panel asks the media engine
/// for a key as the app, for the person looking (`POST /api/v1/media/links`), and the window plays `/media/v/<key>`
/// with seeking. A key opens that one file; a page, a dot file or a made-up key get nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gives_an_apps_window_its_folders_video_with_seeking_by_a_key() {
    let gateway = Gateway::start(&[]).await;
    install_app(gateway.dir.path(), "1.0.0", None);
    let folder = gateway.dir.path().join("plugin-files/echo-app/episodes");
    std::fs::create_dir_all(&folder).unwrap();
    // An MP4 by its bytes: the `ftyp` box first, then anything.
    let mut video = b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom".to_vec();
    video.extend((0..3976u32).map(|n| (n % 251) as u8));
    std::fs::write(folder.join("01.mp4"), &video).unwrap();
    std::fs::write(folder.join("page.html"), "<script>alert(1)</script>").unwrap();
    std::fs::write(gateway.dir.path().join("plugin-files/echo-app/.secret"), "x").unwrap();
    let setup = message(
        gateway.post("/api/v1/auth/setup").json(&json!({"password": "media-admin-password"})),
        StatusCode::CREATED,
    );
    let admin = setup.await["token"].as_str().unwrap().to_string();
    let (name, value) = bearer(&admin);
    let (code, enabled) =
        send(gateway.post("/api/v1/packages/echo-app/enable").header(name.clone(), value.clone())).await;
    assert_eq!(code, StatusCode::OK, "{enabled}");
    let asked = json!({
        "app": "echo-app",
        "addresses": [
            "files/app/echo-app/episodes/01.mp4",
            "files/app/echo-app/episodes/page.html",
            "files/app/echo-app/.secret",
        ],
    });
    assert_eq!(
        gateway.post("/api/v1/media/links").json(&asked).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED,
        "keys only with the panel's key"
    );
    let answer =
        message(gateway.post("/api/v1/media/links").header(name.clone(), value.clone()).json(&asked), StatusCode::OK)
            .await;
    let links = answer["links"].as_array().unwrap();
    let path = links[0]["url"].as_str().unwrap().to_string();
    assert!(path.starts_with("/media/v/"), "{answer}");
    assert!(
        links[1]["error"].is_string() && links[2]["error"].is_string(),
        "no key for a page or a dot file: {answer}"
    );

    // The whole file and a piece of it: seeking.
    let whole = gateway.get(&path).send().await.unwrap();
    assert_eq!(whole.status(), StatusCode::OK);
    assert_eq!(whole.headers()["content-type"], "video/mp4");
    assert_eq!(whole.headers()["accept-ranges"], "bytes");
    assert_eq!(whole.bytes().await.unwrap().len(), video.len());
    let piece = gateway.get(&path).header("range", "bytes=1000-1099").send().await.unwrap();
    assert_eq!(piece.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(piece.headers()["content-range"], format!("bytes 1000-1099/{}", video.len()));
    assert_eq!(piece.bytes().await.unwrap().as_ref(), &video[1000..1100]);

    // Nothing else: a made-up key, the old pass of stage 3.
    for refused in ["/media/v/0123456789abcdef0123456789abcdef", "/app-media/0123456789abcdef/episodes/01.mp4"] {
        assert_eq!(gateway.get(refused).send().await.unwrap().status(), StatusCode::NOT_FOUND, "{refused}");
    }
    gateway.stop().await;
}

/// A voice server on this machine, as the computer's own (tts-server): one model «piper» with «denis» and «ruslan»;
/// every line comes back as a second of quiet WAV.
async fn voice_server() -> String {
    use axum::routing::{get, post};
    let wav = duowave::speech::audio::pcm::encode_wav(&vec![0i16; 22050], 22050);
    let app = axum::Router::new()
        .route(
            "/v1/models",
            get(|| async { axum::Json(json!({ "data": [{ "id": "piper", "voices": ["denis", "ruslan"] }] })) }),
        )
        .route(
            "/v1/audio/speech",
            post(move |axum::Json(body): axum::Json<Value>| {
                let wav = wav.clone();
                async move {
                    assert_eq!((&body["model"], &body["response_format"]), (&json!("piper"), &json!("wav")), "{body}");
                    ([("content-type", "audio/wav")], wav)
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}/v1")
}

/// The model and the voice for apps (docs/plans/feed.md, stage 4): «Привет на Node» writes a paragraph with the
/// household's model (`system/brain` → `write`; here the echo model) and voices it with a voice of the computer into
/// its own folder (`system/voice` → `synth`); the journal keeps which model wrote it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_writes_a_paragraph_with_the_model_and_voices_it_into_its_folder() {
    let Some(node) = some_node() else {
        eprintln!("no node on this machine: the test of runtime node is skipped");
        return;
    };
    let voices = voice_server().await;
    let gateway =
        Gateway::start(&[("DUOWAVE_NODE", &node), ("DIALOG_PROVIDER", "echo"), ("TTS_OWN_URL", &voices)]).await;
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/plugins/apps/hello-node");
    let dir = gateway.dir.path().join("packages/hello-node/1.0.0");
    std::fs::create_dir_all(&dir).unwrap();
    for file in ["manifest.json", "server.mjs", "package.json", "package-lock.json"] {
        std::fs::copy(example.join(file), dir.join(file)).unwrap();
    }
    let setup = message(
        gateway.post("/api/v1/auth/setup").json(&json!({"password": "story-admin-password"})),
        StatusCode::CREATED,
    );
    let admin = setup.await["token"].as_str().unwrap().to_string();
    let (name, value) = bearer(&admin);
    let (code, enabled) =
        send(gateway.post("/api/v1/packages/hello-node/enable").header(name.clone(), value.clone())).await;
    assert_eq!(code, StatusCode::OK, "{enabled}");
    // The owner's «да» to the links its manifest asks for: the model and the voice (the panel's question, in advance).
    for to in ["system/brain", "system/voice"] {
        let (code, linked) = send(
            gateway
                .put("/api/v1/bus/links")
                .header(name.clone(), value.clone())
                .json(&json!({ "from": "app/hello-node", "to": to, "level": "call" })),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{linked}");
    }
    let (code, answer) = send(
        gateway
            .post("/api/v1/packages/hello-node/tools/hello_story/try")
            .header(name.clone(), value.clone())
            .json(&json!({ "arguments": { "about": "ветер" } })),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{answer}");
    let content: Value = serde_json::from_str(answer["message"]["content"].as_str().unwrap()).unwrap();
    assert!(content["text"].as_str().unwrap().contains("что такое ветер"), "the echo model wrote it: {content}");
    assert_eq!(content["model"]["cloud"], json!(false), "{content}");
    assert_eq!(
        (&content["file"], &content["voice"], &content["publish"]),
        (&json!("voice/story.wav"), &json!("piper/denis"), &json!(true))
    );
    assert!((content["seconds"].as_f64().unwrap() - 1.0).abs() < 0.01, "{content}");
    let wav = std::fs::read(gateway.dir.path().join("plugin-files/hello-node/voice/story.wav")).unwrap();
    assert!(wav.starts_with(b"RIFF"), "a WAV in its own folder");
    let journal = message(
        gateway.get("/api/v1/workshop/drafts/hello-node/journal").header(name.clone(), value.clone()),
        StatusCode::OK,
    )
    .await;
    let lines: Vec<String> =
        journal["lines"].as_array().unwrap().iter().map(|line| line["message"].as_str().unwrap().to_string()).collect();
    assert!(lines.iter().any(|line| line.contains("модель") && line.contains("дома")), "{lines:?}");
    gateway.stop().await;
}

/// «Поделиться» and the window's own recording (docs/plans/feed.md, stage 5): what the person sent and where goes to
/// the app's journal; «В Видео» is a copy into the household's folder, never over a file there; a recording from the
/// window lands in the app's folder as WAV.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_apps_window_shares_a_file_into_videos_and_records_a_voice_into_its_folder() {
    let gateway = Gateway::start(&[]).await;
    install_app(gateway.dir.path(), "1.0.0", None);
    let folder = gateway.dir.path().join("plugin-files/echo-app/episodes");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("01.mp4"), b"not really a video").unwrap();
    let setup = message(
        gateway.post("/api/v1/auth/setup").json(&json!({"password": "share-admin-password"})),
        StatusCode::CREATED,
    );
    let admin = setup.await["token"].as_str().unwrap().to_string();
    let (name, value) = bearer(&admin);
    let (code, _) = send(gateway.post("/api/v1/packages/echo-app/enable").header(name.clone(), value.clone())).await;
    assert_eq!(code, StatusCode::OK);
    let share = |body: Value| {
        let (name, value) = (name.clone(), value.clone());
        let gateway = &gateway;
        async move { send(gateway.post("/api/v1/packages/echo-app/share").header(name, value).json(&body)).await }
    };
    let (code, first) = share(json!({ "file": "episodes/01.mp4", "where": "videos" })).await;
    assert_eq!(code, StatusCode::OK, "{first}");
    assert_eq!(
        first["message"]["saved"],
        json!("files/home/Videos/01.mp4"),
        "the house's «Видео» — Videos on the disk"
    );
    let (_, second) = share(json!({ "file": "episodes/01.mp4", "where": "videos" })).await;
    assert_eq!(second["message"]["saved"], json!("files/home/Videos/01-2.mp4"), "never over a file there");
    let videos = gateway.dir.path().join("files/home/Videos");
    assert_eq!(std::fs::read(videos.join("01-2.mp4")).unwrap(), b"not really a video");
    let (code, _) = share(json!({ "where": "telegram-web" })).await;
    assert_eq!(code, StatusCode::OK, "a text to a platform's page, by its id in the publishers' list");
    for refused in [
        json!({ "file": "../../secret", "where": "videos" }),
        json!({ "file": "episodes/02.mp4", "where": "videos" }),
        json!({ "file": "episodes/01.mp4", "where": "evil.example" }),
    ] {
        let (code, _) = share(refused.clone()).await;
        assert!(code == StatusCode::BAD_REQUEST || code == StatusCode::NOT_FOUND, "{refused}: {code}");
    }

    // The window's recording: WAV only, into its folder.
    let wav = duowave::speech::audio::pcm::encode_wav(&vec![0i16; 48_000], 24_000);
    let recorded = message(
        gateway.post("/api/v1/packages/echo-app/recordings").header(name.clone(), value.clone()).body(wav),
        StatusCode::CREATED,
    )
    .await;
    let file = recorded["file"].as_str().unwrap();
    assert!(file.starts_with("recordings/") && file.ends_with(".wav"), "{recorded}");
    assert_eq!(recorded["seconds"], json!(2.0));
    assert!(gateway.dir.path().join("plugin-files/echo-app").join(file).is_file());
    let (code, _) = send(
        gateway.post("/api/v1/packages/echo-app/recordings").header(name.clone(), value.clone()).body("not a wav"),
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);

    let journal =
        message(gateway.get("/api/v1/workshop/drafts/echo-app/journal").header(name, value), StatusCode::OK).await;
    let lines: Vec<String> =
        journal["lines"].as_array().unwrap().iter().map(|line| line["message"].as_str().unwrap().to_string()).collect();
    assert!(lines.iter().any(|line| line.contains("отправлено: episodes/01.mp4 → в «Видео»")), "{lines:?}");
    assert!(lines.iter().any(|line| line.contains("отправлено: текст → Telegram")), "{lines:?}");
    assert!(lines.iter().any(|line| line.contains("запись своим голосом")), "{lines:?}");
    gateway.stop().await;
}

/// «Поделиться» from any screen of the panel (capabilities/routes.rs): a person shares a file of the house by its
/// address — the list of where it can go is the gateway's, what went where is in the bus journal under the person's
/// name, a file the person may not see is not there; the watch page is for an app's clip only. And «Записать своим
/// голосом» into a folder of the house, written as the person.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_shares_a_house_file_from_any_screen_and_records_a_voice_into_a_folder() {
    let gateway = Gateway::start(&[]).await;
    let videos = gateway.dir.path().join("files/home/Videos");
    std::fs::create_dir_all(&videos).unwrap();
    std::fs::write(videos.join("Море.mp4"), b"not really a video").unwrap();
    let setup = message(
        gateway.post("/api/v1/auth/setup").json(&json!({"password": "share-person-password"})),
        StatusCode::CREATED,
    );
    let admin = setup.await["token"].as_str().unwrap().to_string();
    let (name, value) = bearer(&admin);
    let listed = message(gateway.get("/api/v1/publishers").header(name.clone(), value.clone()), StatusCode::OK).await;
    let ids: Vec<&str> =
        listed["publishers"].as_array().unwrap().iter().filter_map(|card| card["id"].as_str()).collect();
    assert!(ids.contains(&"watch") && ids.contains(&"tiktok") && ids.contains(&"telegram-web"), "{listed}");
    let post = |path: &'static str, body: Value| {
        let (name, value) = (name.clone(), value.clone());
        let gateway = &gateway;
        async move { send(gateway.post(path).header(name, value).json(&body)).await }
    };
    let video = "files/home/Videos/Море.mp4";
    let (code, noted) = post("/api/v1/publishers/shared", json!({ "where": "tiktok", "file": video })).await;
    assert_eq!(code, StatusCode::OK, "{noted}");
    let (code, refused) = post("/api/v1/publishers/watch/send", json!({ "file": video })).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{refused}");
    assert!(refused.to_string().contains("для роликов приложений"), "{refused}");
    for (path, body) in [
        ("/api/v1/publishers/shared", json!({ "where": "download", "file": "files/home/Videos/../../secret" })),
        ("/api/v1/publishers/shared", json!({ "where": "download", "file": "files/home/Videos/Нет.mp4" })),
        ("/api/v1/publishers/shared", json!({ "where": "videos", "file": video })),
        ("/api/v1/publishers/shared", json!({ "where": "download", "file": "/etc/passwd" })),
        ("/api/v1/publishers/tiktok/send", json!({ "file": video })),
    ] {
        let (code, answer) = post(path, body.clone()).await;
        assert!(
            [StatusCode::BAD_REQUEST, StatusCode::FORBIDDEN, StatusCode::NOT_FOUND].contains(&code),
            "{body}: {code} {answer}"
        );
    }
    let journal =
        message(gateway.get("/api/v1/bus/journal?limit=50").header(name.clone(), value.clone()), StatusCode::OK).await;
    let shared = journal["lines"].as_array().unwrap().iter().any(|line| {
        line["kind"] == "share"
            && line["from"].as_str().is_some_and(|from| from.starts_with("person/"))
            && line["note"].as_str().is_some_and(|note| note.contains("Море.mp4 → ТикТок"))
    });
    assert!(shared, "the journal under the person's name: {journal}");

    let wav = duowave::speech::audio::pcm::encode_wav(&vec![0i16; 16_000], 16_000);
    let recorded = message(
        gateway
            .post("/api/v1/recordings?folder=files/home/%D0%94%D0%BE%D0%BA%D1%83%D0%BC%D0%B5%D0%BD%D1%82%D1%8B")
            .header(name.clone(), value.clone())
            .body(wav),
        StatusCode::CREATED,
    )
    .await;
    let file = recorded["file"].as_str().unwrap();
    assert!(file.starts_with("files/home/Документы/voice-") && file.ends_with(".wav"), "{recorded}");
    assert_eq!(recorded["seconds"], json!(1.0));
    assert!(gateway.dir.path().join(file).is_file(), "{file}");
    let (code, _) =
        send(gateway.post("/api/v1/recordings?folder=files/app/echo-app").header(name, value).body("not a wav")).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    gateway.stop().await;
}

/// «Смотри и выбирай» (docs/plans/feed.md, stage 10): an app's video with its fork on a page of the gateway — anyone
/// with the link watches with seeking and votes once; the page runs nothing; the app's other files are not there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_watch_page_plays_an_apps_video_and_counts_one_vote_per_browser() {
    let gateway = Gateway::start(&[]).await;
    let folder = gateway.dir.path().join("plugin-files/echo-app/share");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("ep.mp4"), vec![7u8; 3000]).unwrap();
    let token = gateway
        .app
        .watch
        .create(duowave::plugins::watch::WatchPage {
            app: "echo-app".into(),
            file: "share/ep.mp4".into(),
            title: "Компас <серия 1>".into(),
            id: Some("kompas-01".into()),
            q: Some("ПЛЫВЁМ?".into()),
            a: Some("К ОСТРОВУ".into()),
            b: Some("НАЗАД".into()),
            votes: [0, 0],
            created_ms: chrono::Utc::now().timestamp_millis(),
        })
        .unwrap();
    let page = gateway.get(&format!("/watch/{token}")).send().await.unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.headers()["content-security-policy"].to_str().unwrap().contains("default-src 'none'"));
    let html = page.text().await.unwrap();
    assert!(html.contains("Компас &lt;серия 1&gt;") && html.contains("① К ОСТРОВУ"), "escaped, with the fork");
    let piece = gateway.get(&format!("/watch/{token}/video")).header("range", "bytes=10-19").send().await.unwrap();
    assert_eq!(piece.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(piece.bytes().await.unwrap().len(), 10);

    // A vote: the page remembers this browser's choice; a second vote changes nothing.
    let client =
        duowave::utils::http::client_builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let voted =
        client.post(format!("{}/watch/{token}/vote", gateway.base)).form(&[("choice", "b")]).send().await.unwrap();
    assert_eq!(voted.status(), StatusCode::SEE_OTHER);
    let cookie = voted.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    let again = client
        .post(format!("{}/watch/{token}/vote", gateway.base))
        .header("cookie", &cookie)
        .form(&[("choice", "a")])
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::SEE_OTHER);
    assert_eq!(gateway.app.watch.get(&token).unwrap().votes, [0, 1], "one vote per browser");
    let after = client.get(format!("{}/watch/{token}", gateway.base)).header("cookie", &cookie).send().await.unwrap();
    assert!(after.text().await.unwrap().contains("Ваш выбор — ②. Пока так: ① 0% · ② 100% (1)"));
    for wrong in ["/watch/0123/video", "/watch/00000000000000000000000000000000"] {
        assert_eq!(gateway.get(wrong).send().await.unwrap().status(), StatusCode::NOT_FOUND, "{wrong}");
    }
    gateway.stop().await;
}
