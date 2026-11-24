//! The voice gateway for Yandex Station: speaks the station's uniproxy protocol and routes speech to our own ASR,
//! LLM and TTS — or relays it to Yandex. The module tree mirrors the Node gateway's `src/` (docs/plans/rust-gateway.md).
//! That TS original is removed from the tree and kept in git history at the tag `node-gateway-last`: a comment citing
//! a `.ts` file, a Nest module, a `test/*.e2e-spec.ts` or `src/voice-lab` means that tag (PORTING.md).

// The floors below are crates of their own (crates/, docs/plans/modular-gateway.md): their modules are passed on here
// under their own names, so `crate::config` in the gateway and `duowave::config` outside read as before.
pub use duowave_apps::{apps, bus, connectors, midi, plugins, screen};
pub use duowave_base::{config, core, locale, operations, secrets, storage, utils};
pub use duowave_base::{init_logging, log_file, route_needs, tr, VERSION};
pub use duowave_brain::{dialog, memory, oauth, router, skills, sleep, talk, tasks};
#[cfg(feature = "yandex-station")]
pub use duowave_ears::activator;
pub use duowave_ears::{gateway, live, requests};
pub use duowave_home::{egress, health_status, updates};
pub use duowave_rights::{access, rights};
pub use duowave_voice::ear;

// The floor «Только компьютер» (crates/duowave-pc): its own modules and its parts of the floors' ones, wired into the
// gateway in one place (src/computer). The station's build has computer/none.rs there: the floors' own modules,
// nothing of a computer's.
#[cfg_attr(not(feature = "pc"), path = "computer/none.rs")]
pub mod computer;
pub use computer::{
    capabilities, characters, helpers, home, home_server, mcp, models, placement, platform, settings, speech, stations,
    voice_lab,
};

pub mod app;
pub mod links;
pub mod panel;
pub mod tools;

pub use platform::http::ApiError;

#[cfg(test)]
mod whole_tests;
