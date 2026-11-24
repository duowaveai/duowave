//! The panel's model shop (src/models): the catalog of voices and recognition models, downloads and activation, the
//! model servers — the computer's, added to «Голос»'s `models` (the catalog's tables, the station's 501) under its
//! name.

pub use duowave_voice::models::*;

pub mod model_files;
pub mod model_servers;
pub mod models_controller;
pub mod models_service;

pub use model_servers::{server_env, AsrStatus, ModelServers, ServerName, TtsLoaded, TtsStatus};
pub use models_controller::router;
pub use models_service::{
    ChosenVoice, ModelsLinks, ModelsService, OwnRoute, OwnVoiceCard, OwnVoices, PreparedVoice, RestoredVoice, Voice,
    VoiceIds, VoicePair, Whose,
};

crate::route_needs!(
    std::sync::Arc<ModelsService>,
    Option<std::sync::Arc<crate::speech::asr::local::supervisor::LocalAsr>>,
);
