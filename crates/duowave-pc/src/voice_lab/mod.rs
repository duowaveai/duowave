//! The owner's own cloned voices (src/voice-lab, clone-server/): engines installed by one button, takes recorded in
//! the panel, imprints taken from them, and the servers that speak in them — the computer's, added to «Голос»'s
//! `voice_lab` (which keeps the station's 501 answers) under its name.
//!
//! src/voice-lab is the Node original, removed from the tree: git tag `node-gateway-last`.

pub use duowave_voice::voice_lab::*;

pub mod clone_engines;
pub mod clone_servers;
pub mod controller;
pub mod engine_installer;
pub mod jobs;
pub mod js;
pub mod memory_policy;
pub mod passages;
pub mod platform;
pub mod service;
pub mod system_memory;
pub mod voice_store;

pub use controller::router;
pub use service::{OwnSpeech, OwnVoiceCard, PreparedVoice, RestoredVoice, VoiceLabService, VoiceLabView};

crate::route_needs!(std::sync::Arc<VoiceLabService>);
