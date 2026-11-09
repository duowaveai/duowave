//! Starting a gateway for an end-to-end test: a real server on a free port of 127.0.0.1 (`app::serve`), its own
//! DATA_DIR in a temporary folder, reqwest as the client. Every test in tests/ shares it: `mod support;`.
//!
//! The environment is only what the test gives (no process variables, no .env files), on top of a quiet base: no
//! models, no recorder, the energy VAD. The gateway stops when the `Gateway` is dropped.

#![allow(dead_code)]

use std::sync::Arc;

use duowave::app::{self, App};
use duowave::config::{data_dir::prepare_data_dir, Config, Env};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// What every test gateway starts with; a test's own variables go on top.
const BASE: &[(&str, &str)] = &[
    ("NODE_ENV", "test"),
    ("RECORDER_ENABLED", "false"),
    ("VOICE_LAB_BENCH_DIR", ""),
    ("CALLS_STATION_MIC_PORT", ""),
    ("VAD_ENGINE", "energy"),
    ("ASR_PROVIDER", "none"),
    ("DIALOG_PROVIDER", "none"),
    ("ALERT_MACOS_NOTIFY", "false"),
    // The built-in plugins answer the weather, the news, the music by themselves, and some reach the internet: a test
    // gets them when it asks (tests/plugins.rs), the built-in skills otherwise.
    ("PLUGINS_BUNDLED", "false"),
];

pub struct Gateway {
    pub app: Arc<App>,
    /// `http://127.0.0.1:<port>`.
    pub base: String,
    pub port: u16,
    pub dir: tempfile::TempDir,
    pub client: reqwest::Client,
    stop: Option<oneshot::Sender<()>>,
    server: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
}

impl Gateway {
    /// A gateway with these variables over the base, serving at once.
    pub async fn start(vars: &[(&str, &str)]) -> Gateway {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let mut env = Env::from_pairs(BASE.iter().copied()).with("DATA_DIR", &dir.path().to_string_lossy());
        for (key, value) in vars {
            env = env.with(key, value);
        }
        prepare_data_dir(&env).expect("DATA_DIR ready");
        let config = Config::load(&env).unwrap_or_else(|error| panic!("{error}"));
        let app = App::build(config).expect("the gateway builds");
        // As the program does: what runs before the first request (the stations' listeners, the catalog, …).
        app.start().await.expect("the gateway starts");
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let port = listener.local_addr().expect("a bound address").port();
        let (stop, stopped) = oneshot::channel::<()>();
        let server = tokio::spawn(app::serve(app.clone(), listener, async move {
            let _ = stopped.await;
        }));
        let client = duowave::utils::http::client_builder().no_proxy().build().expect("a client");
        Gateway {
            app,
            base: format!("http://127.0.0.1:{port}"),
            port,
            dir,
            client,
            stop: Some(stop),
            server: Some(server),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.client.request(method, self.url(path))
    }

    pub fn get(&self, path: &str) -> RequestBuilder {
        self.request(Method::GET, path)
    }

    pub fn post(&self, path: &str) -> RequestBuilder {
        self.request(Method::POST, path)
    }

    pub fn put(&self, path: &str) -> RequestBuilder {
        self.request(Method::PUT, path)
    }

    pub fn patch(&self, path: &str) -> RequestBuilder {
        self.request(Method::PATCH, path)
    }

    pub fn delete(&self, path: &str) -> RequestBuilder {
        self.request(Method::DELETE, path)
    }

    /// Stops the server and waits for it: open streams end, the requests going now are journaled.
    pub async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(server) = self.server.take() {
            let _ = server.await;
        }
        self.app.requests.shutdown();
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

/// `Authorization: Bearer <token>`.
pub fn bearer(token: &str) -> (reqwest::header::HeaderName, String) {
    (reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
}

/// A request sent: its status and its JSON body (Null when it has none or it is not JSON).
pub async fn send(request: RequestBuilder) -> (StatusCode, Value) {
    let response = request.send().await.expect("the gateway answers");
    let status = response.status();
    let body = response.bytes().await.unwrap_or_default();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The status alone.
pub async fn status(request: RequestBuilder) -> StatusCode {
    send(request).await.0
}

/// The envelope's `message` of a success, asserting the status first.
pub async fn message(request: RequestBuilder, expected: StatusCode) -> Value {
    let (status, body) = send(request).await;
    assert_eq!(status, expected, "{body}");
    body.get("message").cloned().unwrap_or(Value::Null)
}

/// Waits until `check` holds, up to `ms`; false when it never did.
pub async fn until(ms: u64, mut check: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(ms);
    while tokio::time::Instant::now() < deadline {
        if check() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    check()
}
