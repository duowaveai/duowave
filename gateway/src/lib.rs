//! The voice gateway for Yandex Station: speaks the station's uniproxy protocol and routes speech to our own ASR,
//! LLM and TTS — or relays it to Yandex. The module tree mirrors the Node gateway's `src/` (docs/plans/rust-gateway.md).
//! That TS original is removed from the tree and kept in git history at the tag `node-gateway-last`: a comment citing
//! a `.ts` file, a Nest module, a `test/*.e2e-spec.ts` or `src/voice-lab` means that tag (PORTING.md).

pub mod access;
pub mod app;
pub mod apps;
pub mod bus;
#[cfg(feature = "pc")]
pub mod calls;
pub mod characters;
pub mod config;
pub mod connectors;
pub mod core;
pub mod dialog;
pub mod gateway;
pub mod health_status;
pub mod home_server;
pub mod links;
pub mod live;
pub mod mcp;
pub mod memory;
pub mod midi;
pub mod models;
pub mod oauth;
pub mod operations;
pub mod panel;
pub mod placement;
pub mod platform;
pub mod plugins;
pub mod requests;
pub mod secrets;
pub mod settings;
pub mod skills;
pub mod sleep;
pub mod speech;
pub mod stations;
pub mod storage;
pub mod tools;
pub mod updates;
pub mod utils;
pub mod voice_lab;

pub use platform::http::ApiError;

/// This build's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The gateway's log: to stderr (a terminal, a service manager's journal) or, for the desktop app, to a file.
/// `RUST_LOG` filters it (default `info`, ONNX Runtime at `warn`). Once per process; later calls do nothing.
pub fn init_logging(file: Option<&std::path::Path>) {
    use tracing_subscriber::EnvFilter;
    // ONNX Runtime narrates every graph optimisation at info: only its warnings reach our log.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,ort=warn"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    let _ = match file.and_then(|path| std::fs::OpenOptions::new().create(true).append(true).open(path).ok()) {
        Some(file) => builder.with_ansi(false).with_writer(std::sync::Mutex::new(file)).try_init(),
        // Colours only on a terminal: a log redirected to a file (a service, `>> logs/duowave.log`) stays plain text.
        None => builder.with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr())).try_init(),
    };
    log_panics();
}

/// A panic goes to the gateway's log with its place, then to the default report: the panel's «подробности в журнале»
/// (a voice lab job, an own voice server that failed) point at the log, which for the desktop app is a file of its
/// own, not stderr. Set once.
fn log_panics() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let payload = info.payload();
            let what = payload
                .downcast_ref::<&str>()
                .map(|text| text.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "(no message)".into());
            let place = info.location().map(|at| format!(" at {}:{}", at.file(), at.line())).unwrap_or_default();
            tracing::error!("panic{place}: {what}");
            default(info);
        }));
    });
}
