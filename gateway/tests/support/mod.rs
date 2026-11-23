//! Starting a gateway for an end-to-end test: a real server on a free port of 127.0.0.1 (`app::serve`), its own
//! DATA_DIR in a temporary folder, reqwest as the client. Every test in tests/ shares it: `mod support;`.
//!
//! The environment is only what the test gives (no process variables, no .env files), on top of a quiet base: no
//! models, no recorder, the energy VAD. The gateway stops when the `Gateway` is dropped; `restart` starts it again on
//! its folder, as the program starts again on a package of settings. A package a gateway takes asks it to start again
//! (`placement.restart()`, whose default execs the program — here the test binary): a test gateway only counts it
//! (`Gateway::restarts`), and a test restarts it itself.

#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
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
    // A computer's link port: a free one (src/helpers/link_serve.rs), never PORT + 1.
    ("LINK_PORT", "0"),
];

pub struct Gateway {
    pub app: Arc<App>,
    /// `http://127.0.0.1:<port>`.
    pub base: String,
    pub port: u16,
    pub dir: tempfile::TempDir,
    pub client: reqwest::Client,
    /// The times the gateway asked to start again (a package of settings taken): counted, never done. A test may set
    /// its own restarter over it.
    pub restarts: Arc<AtomicUsize>,
    /// The test's own variables, for a restart.
    vars: Vec<(String, String)>,
    /// Where it listens: 127.0.0.1, or every address of this machine (`start_open`).
    host: &'static str,
    server: Server,
}

/// The server going: told to stop when it is dropped.
struct Server {
    stop: Option<oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
}

impl Server {
    /// Stops it and waits: open streams end, the services beside it stop (`app::serve`).
    async fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

impl Gateway {
    /// A gateway with these variables over the base, serving at once.
    pub async fn start(vars: &[(&str, &str)]) -> Gateway {
        let vars = vars.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect();
        Gateway::serve(tempfile::tempdir().expect("a temporary folder"), vars, "127.0.0.1").await
    }

    /// A gateway that listens on every address of this machine, still asked at 127.0.0.1: a computer a station leans
    /// on tells the station its own address on the home network (src/placement/watch.rs, `keep_links`), and the
    /// station dials that address from then on. A gateway on 127.0.0.1 only would be away for it.
    pub async fn start_open(vars: &[(&str, &str)]) -> Gateway {
        let vars = vars.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect();
        Gateway::serve(tempfile::tempdir().expect("a temporary folder"), vars, "0.0.0.0").await
    }

    /// The same gateway started again on its folder — its database, its secrets, a package of settings left for its
    /// start — with its variables, on a new free port; its count of restarts starts again. A restarter the test set on
    /// the app before is gone with it.
    pub async fn restart(mut self) -> Gateway {
        self.server.stop().await;
        self.app.requests.shutdown();
        let Gateway { dir, vars, host, .. } = self;
        Gateway::serve(dir, vars, host).await
    }

    async fn serve(dir: tempfile::TempDir, vars: Vec<(String, String)>, host: &'static str) -> Gateway {
        // The port first: the gateway says it to others (an ear tells its centre where it listens), never the
        // default one a gateway of this computer may be listening on.
        let listener = TcpListener::bind((host, 0)).await.expect("a free port");
        let port = listener.local_addr().expect("a bound address").port();
        let mut env = Env::from_pairs(BASE.iter().copied())
            .with("DATA_DIR", &dir.path().to_string_lossy())
            .with("PORT", &port.to_string());
        for (key, value) in &vars {
            env = env.with(key, value);
        }
        prepare_data_dir(&env).expect("DATA_DIR ready");
        let config = Config::load(&env).unwrap_or_else(|error| panic!("{error}"));
        let app = App::build(config).expect("the gateway builds");
        let restarts = Arc::new(AtomicUsize::new(0));
        let counted = restarts.clone();
        app.placement.set_restarter(Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        }));
        // As the program does: what runs before the first request (the stations' listeners, the catalog, …).
        app.start().await.expect("the gateway starts");
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
            restarts,
            vars,
            host,
            server: Server { stop: Some(stop), handle: Some(server) },
        }
    }

    /// The port the link listens on (a computer that helps), once bound.
    #[cfg(feature = "pc")]
    pub fn link_port(&self) -> Option<u16> {
        self.app.link.port()
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
        self.server.stop().await;
        self.app.requests.shutdown();
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
