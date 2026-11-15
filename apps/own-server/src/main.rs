//! duowave-own-server — the «VPN» app's program (docs/plans/own-server.md, stage 1; docs/plugins/apps/own-server.md).
//!
//! The people's own servers — subscriptions and links of eight protocols — become local entrances (`mixed`, HTTP and
//! SOCKS on `127.0.0.1`), one per connection; the gateway decides who goes through which (its module `egress`). One
//! file, two processes: this one (the control: the app protocol, the settings, the checks, the letters to the
//! gateway) and the engine — shoes (github.com/cfal/shoes, MIT), started from this same file with
//! `DUOWAVE_OWN_SERVER_ROLE=engine`, since shoes gives the outside only its command line.

mod api;
mod app;
mod config;
mod country;
mod engine;
mod gateway;
mod i18n;
mod links;
mod net;
mod store;
mod subscription;

use std::path::PathBuf;
use std::time::Duration;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

fn main() {
    if std::env::var(engine::ROLE_VAR).as_deref() == Ok("engine") {
        engine::guard_orphan();
        shoes::cli::main();
        return;
    }
    let port: u16 = env("PORT").parse().unwrap_or(0);
    let data = std::env::var_os("DUOWAVE_APP_DATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    if port == 0 {
        eprintln!("own-server: PORT is not set: this program is started by the DuoWave gateway");
        std::process::exit(2);
    }
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    runtime.block_on(async move {
        let gateway = gateway::Gateway::new(env("DUOWAVE_URL"), env("DUOWAVE_APP_TOKEN"));
        let app = app::App::new(data, gateway);
        app.start();
        let server = tokio::spawn(api::serve(app.clone(), port, env("DUOWAVE_APP_TOKEN")));
        app.rebuild().await;
        for id in app.subscriptions() {
            let app = app.clone();
            tokio::spawn(async move { app.read_subscription(&id).await });
        }
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM");
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
            result = server => {
                eprintln!("own-server: the HTTP server stopped: {result:?}");
            }
        }
        // The gateway learns the entrances are gone (half a second at most), the engine goes, then this process: the
        // supervisor's SIGKILL comes 3 s after its SIGTERM.
        let _ = app.gateway.connections(Vec::new(), Duration::from_millis(500)).await;
        app.engine.shutdown().await;
    });
}
