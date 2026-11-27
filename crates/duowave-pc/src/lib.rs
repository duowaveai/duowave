//! DuoWave's floor «Только компьютер» (docs/ARCHITECTURE.md, «Этажи»; docs/plans/modular-gateway.md): calls — the
//! call page over WebRTC, LiveKit, SIP, the Telegram bot (`calls`) — and the agents' works in work folders (`works`).
//! A computer's gateway has them, the station's has not: the gateway (`duowave`) takes this crate with its feature
//! `pc` and passes its modules on under their own names.

// A computer's floor only: without the feature the crate is empty, and a station's build of the workspace leaves it
// out.
#![cfg(feature = "pc")]

// The floors below under their own names, as the gateway passes them: `crate::core`, `crate::talk` read as they did.
pub use duowave_apps::{apps, bus, connectors, midi, plugins, screen};
pub use duowave_base::{config, core, locale, operations, secrets, storage, utils};
pub use duowave_base::{log_file, route_needs, tr, ApiError, VERSION};
pub use duowave_brain::{dialog, memory, oauth, router, skills, sleep, talk, tasks};
#[cfg(feature = "yandex-station")]
pub use duowave_ears::activator;
pub use duowave_ears::{gateway, live, requests};
pub use duowave_home::{egress, health_status, updates};
pub use duowave_rights::{access, rights};
pub use duowave_voice::ear;

pub mod calls;
pub mod works;

// The computer's parts of the floors below, under their modules' names: each adds to the lower module it is named
// after (`pub use duowave_voice::models::*` and its own), so `crate::models::ModelsService` reads as it did.
pub mod capabilities;
pub mod characters;
pub mod helpers;
pub mod home;
pub mod home_server;
pub mod mcp;
pub mod models;
pub mod permissions;
pub mod placement;
pub mod platform;
pub mod settings;
pub mod speech;
pub mod stations;
pub mod voice_lab;

/// What the computer's floor says at the program's start, before the configuration is read: that it is in this program
/// (`core::build::computer`), its runner of MCP programs (`mcp::mcp_connection`), its reader of a password typed with
/// no echo (`access::cli`). Again is the same.
pub fn install() {
    crate::core::build::set_computer();
    crate::mcp::mcp_connection::set_programs(std::sync::Arc::new(crate::mcp::programs::OnThisComputer));
    duowave_rights::access::cli::set_hidden(|question| {
        rpassword::prompt_password(question).map(|line| line.trim_end_matches(['\r', '\n']).to_string())
    });
}
