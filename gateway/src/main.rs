//! `duowave`: the voice gateway.
//!
//!   duowave                   serve (the default)
//!   duowave serve             the same
//!   duowave health            whether the gateway on PORT answers /health (exit 0) or not (exit 1)
//!   duowave update [--check]  look for a new version in the feed and install it (restart the gateway after)
//!   duowave version           print the version
//!   duowave station …         the gateway on the station itself: install, point the voice, status, remove
//!                             (`duowave station help`)
//!   duowave plugin …          the workshop in a terminal: new (from a template or OpenAPI), test, install
//!
//! The developer's commands (src/tools):
//!
//!   duowave replay <file.in.ogg> [--no-asr] [--wav out.wav]   a recorded request through the speech pipeline
//!   duowave journal [--all]                                   the session journal live, one line per event
//!   duowave animations [journal dir] [content dir]            light animations from the relay journals
//!   duowave call-sim [--url …] [--play voice.wav] [--record heard.wav] [--answer-after 1500] [--call "Мама"]
//!                                                             a station for our calls (PC build)
//!   duowave livekit-token --name "Мама" [--days 30]           a LiveKit room access for an app (PC build)

/// The station's memory (Lite 2: ~120 MB free and no swap). musl's own allocator (mallocng) never gave back what a
/// burst left broken up: the panel's lists, a dozen requests at once from the computer, took the gateway from 15 to
/// 57 MB in an hour (16.11.2026), and the apps' budget is what is left. mimalloc, told to purge freed pages at once and
/// not to commit its arenas ahead, stayed at ~19 MB under the same requests in a container with the station's build
/// and data. Only in the station's build for the station: a PC keeps its system's allocator.
#[cfg(all(feature = "station", target_env = "musl"))]
#[global_allocator]
static MEMORY: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// mimalloc v3's options by number (include/mimalloc.h, `mi_option_e`): the crate names only the older ones.
#[cfg(all(feature = "station", target_env = "musl"))]
mod station_memory {
    const ARENA_EAGER_COMMIT: libmimalloc_sys::mi_option_t = 4;
    const PURGE_DELAY: libmimalloc_sys::mi_option_t = 15;

    /// Freed pages go back to the system at once (the default waits 10 ms, ×10 for arenas), and an arena is committed
    /// as it is used. What was there before, for the log: MIMALLOC_* in the environment shows here.
    pub fn tune() -> (i64, i64) {
        // SAFETY: plain option setters of the allocator, before any thread of ours.
        unsafe {
            let before = (
                libmimalloc_sys::mi_option_get(PURGE_DELAY) as i64,
                libmimalloc_sys::mi_option_get(ARENA_EAGER_COMMIT) as i64,
            );
            libmimalloc_sys::mi_option_set(PURGE_DELAY, 0);
            libmimalloc_sys::mi_option_set(ARENA_EAGER_COMMIT, 0);
            before
        }
    }
}

fn main() -> anyhow::Result<()> {
    #[cfg(all(feature = "station", target_env = "musl"))]
    let memory_was = station_memory::tune();
    #[cfg(not(all(feature = "station", target_env = "musl")))]
    let memory_was = ();
    // A recognition worker of this program's own gateway (src/speech/asr/local): nothing else starts in it.
    #[cfg(feature = "local-asr")]
    if duowave::speech::asr::local::is_worker_invocation() {
        std::process::exit(duowave::speech::asr::local::run_worker());
    }
    let command = std::env::args().nth(1).unwrap_or_else(|| "serve".into());
    match command.as_str() {
        "version" | "--version" | "-V" => {
            println!("duowave {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "serve" => serve(memory_was),
        "health" => health(),
        "update" => update(std::env::args().any(|arg| arg == "--check")),
        "station" => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            std::process::exit(duowave::placement::cli::command(&args));
        }
        "plugin" => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            std::process::exit(duowave::plugins::workshop::cli::command(&args));
        }
        tool if duowave::tools::COMMANDS.contains(&tool) => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            std::process::exit(duowave::tools::command(tool, &args));
        }
        other => {
            eprintln!(
                "duowave: unknown command {other:?} (serve, health, update, version, station, plugin, {})",
                duowave::tools::COMMANDS.join(", ")
            );
            std::process::exit(2);
        }
    }
}

#[cfg(all(feature = "station", target_env = "musl"))]
type MemoryWas = (i64, i64);
#[cfg(not(all(feature = "station", target_env = "musl")))]
type MemoryWas = ();

fn serve(memory_was: MemoryWas) -> anyhow::Result<()> {
    duowave::init_logging(None);
    #[cfg(all(feature = "station", target_env = "musl"))]
    tracing::info!(
        "memory: mimalloc, freed pages back at once (purge delay {} → 0 ms, arena eager commit {} → 0)",
        memory_was.0,
        memory_was.1
    );
    #[cfg(not(all(feature = "station", target_env = "musl")))]
    let () = memory_was;
    // One process-wide TLS provider: ring, the same on the station and on a PC.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let env = duowave::config::Env::from_process();
    duowave::config::data_dir::prepare_data_dir(&env)?;
    let config = duowave::config::Config::load(&env)?;
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(duowave::app::run(config))
}

fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread().enable_all().build()?)
}

/// For Docker's HEALTHCHECK and service managers: the gateway on this machine's PORT answers /health.
fn health() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = duowave::config::Config::load(&duowave::config::Env::from_process())?;
    let url = format!("http://127.0.0.1:{}/health", config.app.port);
    let answered = runtime()?.block_on(async {
        duowave::utils::http::client()
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map(|response| response.status().is_success())
            .unwrap_or(false)
    });
    if !answered {
        eprintln!("duowave: no answer from {url}");
        std::process::exit(1);
    }
    Ok(())
}

/// `duowave update`: the same check and install as the panel's «Обновить», from the console.
fn update(check_only: bool) -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = duowave::config::Config::load(&duowave::config::Env::from_process())?;
    let updates =
        std::sync::Arc::new(duowave::updates::UpdatesService::new(&config, duowave::core::events::EventBus::new()));
    match runtime()?.block_on(updates.update_from_console(check_only)) {
        Ok(message) => {
            println!("{message}");
            Ok(())
        }
        Err(problem) => {
            eprintln!("duowave update: {problem}");
            std::process::exit(1);
        }
    }
}
