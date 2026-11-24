//! DuoWave's base floor (docs/ARCHITECTURE.md, «Этажи»; docs/plans/modular-gateway.md): what every other part
//! stands on — the configuration and `.env`, the database and its migrations, the secrets, outgoing HTTP, the API's
//! answers and who asks, the languages, the shared types and events, the settings, home networks and retention. It
//! knows nothing of the floors above it; the gateway (`duowave`) passes its modules on under their own names, so
//! `duowave::config` and `crate::config` there read as before.

pub mod config;
pub mod core;
pub mod locale;
pub mod operations;
pub mod platform;
pub mod secrets;
pub mod settings;
pub mod storage;
pub mod utils;

pub use platform::http::ApiError;

/// This build's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The gateway's log: to stderr (a terminal, a service manager's journal) or, for the desktop app, to a file.
/// `RUST_LOG` filters it (default `info`, ONNX Runtime at `warn`). Once per process; later calls do nothing.
/// The gateway's log file, when it writes one (the desktop app): the developer's report reads its end.
static LOG_FILE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Where the gateway's log goes, when it is a file.
pub fn log_file() -> Option<&'static std::path::Path> {
    LOG_FILE.get().map(std::path::PathBuf::as_path)
}

pub fn init_logging(file: Option<&std::path::Path>) {
    if let Some(path) = file {
        let _ = LOG_FILE.set(path.to_path_buf());
    }
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
