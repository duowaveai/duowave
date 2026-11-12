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

fn main() -> anyhow::Result<()> {
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
        "serve" => serve(),
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

fn serve() -> anyhow::Result<()> {
    duowave::init_logging(None);
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
    let updates = duowave::updates::UpdatesService::new(&config, duowave::core::events::EventBus::new());
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
