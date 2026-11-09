//! Own voices end to end (test/voice-lab.e2e-spec.ts): engines, takes, an imprint, choosing it — a real gateway on a
//! free port with a temporary DATA_DIR, a Mac that only pretends (its memory as the test sets it), an engine whose
//! Python is a script speaking as clone-server/imprint.py does, and a voice server already up. Audio is synthetic.
//!
//! The scenarios that need a signed-in admin sign in as the Node tests did (POST /api/v1/auth/setup). Choosing a
//! voice goes through the models, speech's voice and /health, linked in App (src/links).
//!
//! test/voice-lab.e2e-spec.ts, the Node original, is removed from the tree: git tag `node-gateway-last`.

#[cfg(all(feature = "pc", unix))]
mod pc {
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use axum::routing::get;
    use axum::{Json, Router};
    use duowave::app::App;
    use duowave::config::{Config, Env};
    use duowave::speech::audio::pcm::encode_wav;
    use duowave::voice_lab::platform::{CommandResult, Probe};
    use futures::future::BoxFuture;
    use reqwest::{Method, StatusCode};
    use serde_json::{json, Value};
    use tokio::sync::oneshot;

    /// The imprint job as the engine's Python would run it (clone-server/imprint.py), without a model: a script in
    /// the runtime's env/bin/python that speaks the same JSON lines and leaves the same voice folder.
    const FAKE_PYTHON: &str = r#"#!/bin/sh
take=""; out=""; name=""; engine=""; device=""; samples=""; field=""
for arg in "$@"; do
  case "$arg" in
    --*) field="$arg"; continue ;;
  esac
  case "$field" in
    --takes) [ -z "$take" ] && take="$arg" ;;
    --out) out="$arg" ;;
    --name) name="$arg" ;;
    --engine) engine="$arg" ;;
    --device) device="$arg" ;;
    --samples) samples="$arg" ;;
  esac
done
first=$(printf '%s' "$samples" | sed 's/^\["\([^"]*\)".*$/\1/')
echo '{"stage":"select","percent":12}'
echo '{"memoryMb":900}'
echo '{"stage":"imprint","percent":55}'
mkdir -p "$out/samples"
cp "$take" "$out/ref.wav"
cp "$take" "$out/samples/1.wav"
printf 'imprint' > "$out/voice.pt"
printf '{"id":"%s","name":"%s","engine":"%s","device":"%s","createdAt":"2026-11-06T01:00:00+00:00","reference":{"take":1,"start":1,"end":7,"seconds":6,"snrDb":30,"words":"слова образца"},"samples":[{"n":1,"text":"%s","seconds":3.2}],"rtf":1.2,"peakMb":2100}' "$(basename "$out")" "$name" "$engine" "$device" "$first" > "$out/meta.json"
echo "{\"stage\":\"done\",\"percent\":100,\"peakMb\":2100,\"rtf\":1.2,\"refSeconds\":6,\"device\":\"$device\"}"
"#;

    /// What the Mac's memory looks like to the sampling: 16 GB free and calm, until a test says otherwise.
    struct Memory {
        free_mb: AtomicU64,
        pressure_level: AtomicU64,
    }

    /// An Apple Silicon Mac, as detection and the memory sampling would see it.
    struct Mac {
        memory: Arc<Memory>,
    }

    impl Probe for Mac {
        fn platform(&self) -> String {
            "darwin".into()
        }
        fn arch(&self) -> String {
            "arm64".into()
        }
        fn env(&self, _name: &str) -> Option<String> {
            None
        }
        fn run(&self, command: &str, args: &[&str], _timeout_ms: u64) -> BoxFuture<'static, CommandResult> {
            let answer = |stdout: String| CommandResult { code: Some(0), stdout };
            let result = if command == "sysctl" && args.get(1) == Some(&"machdep.cpu.brand_string") {
                answer("Apple M4 Max\n".into())
            } else if command == "sysctl" {
                // page, free, speculative, purgeable, reusable, file-backed, pressure level, swap-ins.
                let free = self.memory.free_mb.load(Ordering::SeqCst) * 64;
                let level = self.memory.pressure_level.load(Ordering::SeqCst);
                answer(format!("16384\n{free}\n0\n0\n0\n0\n{level}\n0"))
            } else if command == "uv" || command == "git" {
                answer("ok".into())
            } else {
                CommandResult { code: None, stdout: String::new() }
            };
            Box::pin(async move { result })
        }
        fn exists(&self, _path: &Path) -> bool {
            false
        }
        fn read(&self, _path: &Path) -> Option<String> {
            None
        }
        fn total_mb(&self) -> u64 {
            36_864
        }
        fn memory_bytes(&self) -> (u64, u64) {
            (16_384 * 1_048_576, 36_864 * 1_048_576)
        }
        fn cpus(&self) -> (usize, Option<String>) {
            (14, None)
        }
        fn free_disk_mb(&self, _path: &Path) -> u64 {
            80_000
        }
        fn home(&self) -> std::path::PathBuf {
            "/Users/owner".into()
        }
    }

    const RATE: u32 = 16_000;

    /// A voiced, speech-like tone (fundamental plus harmonics, syllable-rate wobble) at the given RMS level.
    fn voice(ms: u32, rms_dbfs: f64) -> Vec<i16> {
        let count = (ms * RATE / 1000) as usize;
        let tau = std::f64::consts::TAU;
        let raw: Vec<f64> = (0..count)
            .map(|i| {
                let t = i as f64 / RATE as f64;
                let wobble = 0.75 + 0.25 * (tau * 4.0 * t).sin();
                wobble * ((tau * 160.0 * t).sin() + 0.5 * (tau * 320.0 * t).sin() + 0.25 * (tau * 480.0 * t).sin())
            })
            .collect();
        let rms = (raw.iter().map(|v| v * v).sum::<f64>() / count as f64).sqrt();
        let scale = 10f64.powf(rms_dbfs / 20.0) / rms * 32767.0;
        raw.iter().map(|v| (v * scale).round() as i16).collect()
    }

    /// Room noise at the given RMS level (deterministic).
    fn silence(ms: u32, rms_dbfs: f64) -> Vec<i16> {
        let amplitude = 10f64.powf(rms_dbfs / 20.0) * 32767.0 * 3f64.sqrt();
        let mut seed: u64 = 12345;
        (0..(ms * RATE / 1000))
            .map(|_| {
                seed = (seed * 1_103_515_245 + 12345) & 0x7fff_ffff;
                (((seed as f64 / 0x7fff_ffff as f64) * 2.0 - 1.0) * amplitude).round() as i16
            })
            .collect()
    }

    /// 20 s of reading: phrases and pauses.
    fn take() -> Vec<u8> {
        let mut pcm = Vec::new();
        for _ in 0..8 {
            pcm.extend(voice(2000, -20.0));
            pcm.extend(silence(500, -60.0));
        }
        encode_wav(&pcm, RATE)
    }

    fn executable(path: &Path, script: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The voice's server: already up (the gateway takes it over), speaking WAV.
    async fn clone_server() -> (String, tokio::task::JoinHandle<()>) {
        let health = json!({
            "server": "clone-server",
            "engine": "omnivoice",
            "device": "mps",
            "pid": 999_999,
            "instance": "stub",
            "voices": [],
            "memoryMb": 2_600,
            "peakMb": 3_000,
        });
        let wav = encode_wav(&voice(300, -20.0), 24_000);
        let router = Router::new().route("/health", get(move || async move { Json(health) })).fallback(move || {
            let wav = wav.clone();
            async move { ([(axum::http::header::CONTENT_TYPE, "audio/wav")], wav) }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        (
            url,
            tokio::spawn(async move {
                let _ = axum::serve(listener, router).await;
            }),
        )
    }

    /// A gateway of its own for each test.
    struct Gateway {
        base: String,
        clone_url: String,
        memory: Arc<Memory>,
        http: reqwest::Client,
        token: Option<String>,
        shutdown: Option<oneshot::Sender<()>>,
        clone: tokio::task::JoinHandle<()>,
        _app: Arc<App>,
        _dir: tempfile::TempDir,
    }

    impl Drop for Gateway {
        fn drop(&mut self) {
            if let Some(shutdown) = self.shutdown.take() {
                let _ = shutdown.send(());
            }
            self.clone.abort();
        }
    }

    async fn gateway() -> Gateway {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        executable(&root.join("runtime/omnivoice/env/bin/python"), FAKE_PYTHON);
        std::fs::write(
            root.join("runtime/omnivoice/ready.json"),
            r#"{"engine":"omnivoice","device":"mps","torch":"2.8.0","source":"bench","installedAt":"2026-11-06T00:00:00Z"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("clone-server")).unwrap();
        let (clone_url, clone) = clone_server().await;
        // Nothing answers for the other engine: a closed port of this computer.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let text = |sub: &str| root.join(sub).to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("DATA_DIR", text("data")),
            ("STATE_DIR", text("state")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("VOICE_LAB_RUNTIME_DIR", text("runtime")),
            ("VOICE_LAB_BENCH_DIR", String::new()),
            ("VOICE_LAB_SERVER_URLS", format!("omnivoice={clone_url};cosyvoice3=http://127.0.0.1:{closed}/v1")),
            // Our voice server for Silero and Piper (the env's route); recognition on this computer.
            ("TTS_OWN_URL", clone_url.clone()),
            ("ASR_PROVIDER", "openai".into()),
            ("ASR_BASE_URL", "http://127.0.0.1:9/v1".into()),
        ]);
        let mut config = Config::load(&env).expect("the test's configuration");
        config.voice_lab.server_dir = root.join("clone-server");
        // The model shop's open registry is HuggingFace: here a closed port, the shop serves what the database has.
        config.models.piper_registry = format!("http://127.0.0.1:{closed}/registry");
        let app = App::build(config).unwrap();
        let memory = Arc::new(Memory { free_mb: AtomicU64::new(16_384), pressure_level: AtomicU64::new(1) });
        app.voice_lab.use_probe(Arc::new(Mac { memory: memory.clone() }));
        app.voice_lab.start();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (shutdown, stopped) = oneshot::channel::<()>();
        tokio::spawn(duowave::app::serve(app.clone(), listener, async move {
            let _ = stopped.await;
        }));
        Gateway {
            base,
            clone_url,
            memory,
            http: duowave::utils::http::client(),
            token: None,
            shutdown: Some(shutdown),
            clone,
            _app: app,
            _dir: dir,
        }
    }

    /// An answer: its status, and the envelope's message (or the error's body).
    struct Answer {
        status: StatusCode,
        body: Value,
        content_type: String,
    }

    impl Answer {
        fn message(&self) -> &Value {
            &self.body["message"]
        }
    }

    impl Gateway {
        async fn call(&self, method: Method, path: &str, body: Option<Value>) -> Answer {
            let mut request = self.http.request(method, format!("{}{path}", self.base));
            if let Some(token) = &self.token {
                request = request.bearer_auth(token);
            }
            if let Some(body) = body {
                request = request.json(&body);
            }
            answer(request.send().await.unwrap()).await
        }

        async fn upload(&self, path: &str, wav: Vec<u8>) -> Answer {
            let mut request =
                self.http.post(format!("{}{path}", self.base)).header("Content-Type", "audio/wav").body(wav);
            if let Some(token) = &self.token {
                request = request.bearer_auth(token);
            }
            answer(request.send().await.unwrap()).await
        }

        /// The first admin, as the panel's first start makes one.
        async fn signed_in(mut self) -> Gateway {
            let setup =
                self.call(Method::POST, "/api/v1/auth/setup", Some(json!({"password": "e2e-admin-password"}))).await;
            assert_eq!(setup.status, StatusCode::CREATED, "{}", setup.body);
            self.token = Some(setup.message()["token"].as_str().unwrap().to_string());
            self
        }

        async fn lab(&self) -> Value {
            let view = self.call(Method::GET, "/api/v1/voice-lab", None).await;
            assert_eq!(view.status, StatusCode::OK, "{}", view.body);
            view.message().clone()
        }

        async fn finished_job(&self) -> Value {
            for _ in 0..100 {
                let job = self.lab().await["job"].clone();
                if job["state"] != "running" {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("the imprint did not finish");
        }

        async fn imprint(&self, name: &str) -> String {
            assert_eq!(
                self.upload("/api/v1/voice-lab/draft/takes?passage=1", take()).await.status,
                StatusCode::CREATED
            );
            let job = self
                .call(Method::POST, "/api/v1/voice-lab/imprint", Some(json!({"engine": "omnivoice", "name": name})))
                .await;
            assert_eq!(job.status, StatusCode::CREATED, "{}", job.body);
            let job = job.message();
            assert_eq!(
                (&job["kind"], &job["engine"], &job["state"]),
                (&json!("imprint"), &json!("omnivoice"), &json!("running"))
            );
            let done = self.finished_job().await;
            assert_eq!((&done["state"], &done["peakMb"]), (&json!("done"), &json!(2100)), "{done}");
            done["result"]["voiceId"].as_str().unwrap().to_string()
        }
    }

    async fn answer(response: reqwest::Response) -> Answer {
        let status = response.status();
        let content_type =
            response.headers().get("content-type").and_then(|value| value.to_str().ok()).unwrap_or("").to_string();
        let bytes = response.bytes().await.unwrap();
        Answer { status, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null), content_type }
    }

    #[tokio::test]
    async fn sees_this_computer_and_its_engines() {
        let gw = gateway().await.signed_in().await;
        let view = gw.lab().await;
        assert_eq!(view["available"], true);
        assert_eq!(view["platform"]["summary"], "Мак, Apple M4 Max, 36 ГБ памяти");
        assert_eq!((&view["platform"]["os"], &view["platform"]["gpu"]), (&json!("darwin"), &json!("apple")));
        let engines: Vec<Value> = view["engines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|engine| json!([engine["id"], engine["status"], engine["device"]]))
            .collect();
        assert_eq!(
            engines,
            vec![
                json!(["omnivoice", "ready", "mps"]),
                json!(["cosyvoice3", "missing", "cpu"]),
                json!(["voxcpm", "missing", "mps"])
            ]
        );
        assert_eq!(view["passages"].as_array().unwrap().len(), 5);
        assert_eq!(
            view["memory"],
            json!({"totalMb": 36_864, "poolMb": 16_384, "reserveMb": 2_212, "pressure": "normal"})
        );
        // The voice's server says what it holds: its next start is judged by its peak, not by the engine's guess.
        let server = &view["servers"]["omnivoice"];
        assert_eq!(
            (&server["up"], &server["memoryMb"], &server["peakMb"]),
            (&json!(true), &json!(2_600), &json!(3_000))
        );
        assert_eq!(view["engines"][0]["needMb"], json!({"imprint": 3_500, "serve": 3_450}));
    }

    #[tokio::test]
    async fn records_a_take_plays_it_back_refuses_what_is_not_a_wav() {
        let gw = gateway().await.signed_in().await;
        let added = gw.upload("/api/v1/voice-lab/draft/takes?passage=2", take()).await;
        assert_eq!(added.status, StatusCode::CREATED);
        let added = added.message();
        assert_eq!((&added["n"], &added["passage"], &added["verdict"]), (&json!(1), &json!(2), &json!("good")));
        let audio = gw.call(Method::GET, "/api/v1/voice-lab/draft/takes/1", None).await;
        assert_eq!(audio.status, StatusCode::OK);
        assert!(audio.content_type.starts_with("audio/wav"), "{}", audio.content_type);
        assert_eq!(
            gw.upload("/api/v1/voice-lab/draft/takes", b"ID3...".to_vec()).await.status,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(gw.call(Method::DELETE, "/api/v1/voice-lab/draft", None).await.status, StatusCode::OK);
        assert_eq!(gw.lab().await["draft"], json!({"takes": [], "seconds": 0}));
    }

    #[tokio::test]
    async fn takes_an_imprint_the_takes_go_with_the_voice_its_sample_plays_it_can_be_renamed_and_deleted() {
        let gw = gateway().await.signed_in().await;
        let id = gw.imprint("Тестовый").await;
        let view = gw.lab().await;
        assert_eq!(view["draft"]["takes"], json!([]));
        let voices = view["voices"].as_array().unwrap();
        assert_eq!(voices.len(), 1);
        for (field, value) in [
            ("id", json!(id)),
            ("name", json!("Тестовый")),
            ("engine", json!("omnivoice")),
            ("takes", json!(1)),
            ("chosen", json!(false)),
            ("refSeconds", json!(6)),
        ] {
            assert_eq!(voices[0][field], value, "{field}");
        }
        let sample = gw.call(Method::GET, &format!("/api/v1/voice-lab/voices/{id}/samples/1"), None).await;
        assert_eq!(sample.status, StatusCode::OK);
        let missing = gw.call(Method::GET, &format!("/api/v1/voice-lab/voices/{id}/samples/7"), None).await;
        assert_eq!(missing.status, StatusCode::NOT_FOUND);
        let renamed = gw
            .call(Method::PATCH, &format!("/api/v1/voice-lab/voices/{id}"), Some(json!({"name": "Второй голос"})))
            .await;
        assert_eq!((renamed.status, &renamed.message()["name"]), (StatusCode::OK, &json!("Второй голос")));
        assert_eq!(
            gw.call(Method::DELETE, &format!("/api/v1/voice-lab/voices/{id}"), None).await.status,
            StatusCode::OK
        );
        assert_eq!(gw.lab().await["voices"], json!([]));
    }

    #[tokio::test]
    async fn chooses_an_own_voice_through_the_models_the_gateway_speaks_through_its_server_and_keeps_it_from_deletion()
    {
        let gw = gateway().await.signed_in().await;
        let id = gw.imprint("Мой голос").await;
        let card = format!("own:{id}");
        let find = |models: &Value| {
            models["tts"].as_array().unwrap().iter().find(|entry| entry["id"] == json!(card)).cloned().unwrap()
        };
        let before = gw.call(Method::GET, "/api/v1/models", None).await;
        assert_eq!(before.status, StatusCode::OK);
        let entry = find(before.message());
        for (field, value) in [
            ("engine", json!("omnivoice")),
            ("group", json!("own")),
            ("title", json!("Мой голос")),
            ("badge", json!("свой")),
            ("installed", json!(true)),
            ("active", json!(false)),
        ] {
            assert_eq!(entry[field], value, "{field}");
        }
        for (field, value) in
            [("id", json!(id)), ("samples", json!(1)), ("engineTitle", json!("OmniVoice")), ("serving", json!(true))]
        {
            assert_eq!(entry["own"][field], value, "own.{field}");
        }
        let activated = gw.call(Method::POST, "/api/v1/models/tts/activate", Some(json!({"modelId": card}))).await;
        assert_eq!((activated.status, activated.message()), (StatusCode::CREATED, &json!({"needsRestart": false})));
        let after = gw.call(Method::GET, "/api/v1/models", None).await;
        assert_eq!(after.message()["ttsActive"], json!({"modelId": card, "voiceId": card}));
        assert_eq!(after.message()["ttsUp"], true);
        assert_eq!(find(after.message())["active"], true);
        let health = gw.call(Method::GET, "/health", None).await;
        let tts = health.message()["tts"].as_str().unwrap_or_default().to_string();
        assert!(tts.contains(&format!("omnivoice/{id} via {}", gw.clone_url)), "{tts}");
        assert_eq!(gw.lab().await["voices"][0]["chosen"], true);
        let refused = gw.call(Method::DELETE, &format!("/api/v1/voice-lab/voices/{id}"), None).await;
        assert_eq!(refused.status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn only_warns_when_memory_is_tight_and_starts_on_the_owners_vse_ravno_refuses_while_the_mac_is_short() {
        let gw = gateway().await.signed_in().await;
        // Measured once (2.1 GB), the imprint needs 2.4 GB with its margin; its ceiling is half again above.
        gw.imprint("Мой голос").await;
        assert_eq!(gw.upload("/api/v1/voice-lab/draft/takes", take()).await.status, StatusCode::CREATED);
        gw.memory.free_mb.store(4_000, Ordering::SeqCst);
        let imprint = |anyway: bool| {
            let body =
                if anyway { json!({"engine": "omnivoice", "anyway": true}) } else { json!({"engine": "omnivoice"}) };
            gw.call(Method::POST, "/api/v1/voice-lab/imprint", Some(body))
        };
        let warned = imprint(false).await;
        assert_eq!(warned.status, StatusCode::PRECONDITION_REQUIRED);
        assert_eq!(
            warned.body["message"],
            "Памяти впритык: свободно 3,9 ГБ, нужно около 2,4 ГБ, а 2,2 ГБ держу для колонки — она может подтормаживать."
        );
        gw.memory.pressure_level.store(2, Ordering::SeqCst);
        let refused = imprint(true).await;
        assert_eq!(refused.status, StatusCode::CONFLICT);
        assert!(refused.body["message"].as_str().unwrap().contains("не хватает памяти"));
        gw.memory.pressure_level.store(1, Ordering::SeqCst);
        assert_eq!(imprint(true).await.status, StatusCode::CREATED);
        let job = gw.finished_job().await;
        assert_eq!((&job["state"], &job["limitMb"]), (&json!("done"), &json!(3_623)));
        gw.memory.free_mb.store(16_384, Ordering::SeqCst);
        // No limit for the owner to set any more.
        let memory = gw.call(Method::PUT, "/api/v1/voice-lab/memory", Some(json!({"mode": "cap", "mb": 2048}))).await;
        assert_eq!(memory.status, StatusCode::NOT_FOUND);
        let stopped = gw.call(Method::POST, "/api/v1/voice-lab/job/stop", None).await;
        assert_eq!((stopped.status, stopped.message()), (StatusCode::CREATED, &json!({"stopped": false})));
    }

    #[tokio::test]
    async fn lets_everyone_who_sees_the_station_look_and_only_managers_change() {
        let gw = gateway().await;
        assert_eq!(gw.call(Method::GET, "/api/v1/voice-lab", None).await.status, StatusCode::UNAUTHORIZED);
        let install = gw.call(Method::POST, "/api/v1/voice-lab/engines/cosyvoice3/install", None).await;
        assert_eq!(install.status, StatusCode::UNAUTHORIZED);
        assert_eq!(install.body["message"], "Нужно войти.");
    }

    /// Every route of the panel's contract is there (closed to strangers, not missing), and the owner's old memory
    /// limit is gone.
    #[tokio::test]
    async fn answers_every_route_of_the_panels_contract_and_not_the_memory_limit_that_is_gone() {
        let gw = gateway().await;
        for (method, path) in [
            (Method::GET, "/api/v1/voice-lab"),
            (Method::POST, "/api/v1/voice-lab/platform/recheck"),
            (Method::POST, "/api/v1/voice-lab/engines/omnivoice/install"),
            (Method::GET, "/api/v1/voice-lab/draft/takes/1"),
            (Method::DELETE, "/api/v1/voice-lab/draft/takes/1"),
            (Method::DELETE, "/api/v1/voice-lab/draft"),
            (Method::POST, "/api/v1/voice-lab/imprint"),
            (Method::POST, "/api/v1/voice-lab/job/stop"),
            (Method::GET, "/api/v1/voice-lab/voices/v-1/samples/1"),
            (Method::PATCH, "/api/v1/voice-lab/voices/v-1"),
            (Method::DELETE, "/api/v1/voice-lab/voices/v-1"),
        ] {
            let answer = gw.call(method.clone(), path, None).await;
            assert_eq!(answer.status, StatusCode::UNAUTHORIZED, "{method} {path}: {}", answer.body);
        }
        // A stranger's upload is refused before it is read: a short one, so the answer is not lost to a reset.
        let upload = gw.upload("/api/v1/voice-lab/draft/takes?passage=1", b"RIFF".to_vec()).await;
        assert_eq!(upload.status, StatusCode::UNAUTHORIZED);
        let memory = gw.call(Method::PUT, "/api/v1/voice-lab/memory", Some(json!({"mode": "cap", "mb": 2048}))).await;
        assert_eq!(memory.status, StatusCode::NOT_FOUND);
        assert_eq!(memory.body["message"], "Cannot PUT /api/v1/voice-lab/memory");
    }
}

/// The station's build: the voice lab is not part of it, and says so in the envelope.
#[cfg(not(feature = "pc"))]
mod station {
    use duowave::config::{Config, Env};
    use reqwest::StatusCode;

    #[tokio::test]
    async fn says_the_voice_lab_is_not_part_of_this_build() {
        let app = duowave::app::App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(duowave::app::serve(app, listener, std::future::pending()));
        let response = duowave::utils::http::client().get(format!("{base}/api/v1/voice-lab")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["code"], 501);
        assert!(body["message"].as_str().unwrap().contains("нет в этой сборке"), "{body}");
    }
}
