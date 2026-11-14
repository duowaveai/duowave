//! The dialog: who answers the recognised question — the model and its providers (llm/), the brain the panel picks
//! (brain/), the persona, the history — and the answers as phrases for speech (dialog_service.rs).

pub mod brain;
pub mod dialog_history;
pub mod dialog_service;
pub mod generator;
pub mod llm;
pub mod persona_controller;
pub mod persona_service;
pub mod phrase_splitter;
pub mod system_prompt;

use std::sync::Arc;

use axum::Router;

pub use dialog_service::{
    with_error_phrase, AnsweredBy, DialogContext, DialogService, Filler, ModelChoice, PhraseStream, ToolBox,
    ToolFilter, ToolOutcome, ToolRun, ERROR_PHRASE, LOOKUP_FILLER,
};
pub use llm::brain::{BrainProfile, BrainSource, BrainStatus, ModelSlot};
pub use persona_service::PersonaService;
pub use phrase_splitter::PhraseSplitter;
pub use system_prompt::Channel;

use crate::app::App;

/// The panel's routes of the dialog: `/api/v1/brain/…` (the providers and the choice) and `/api/v1/persona`.
pub fn routes() -> Router<Arc<App>> {
    Router::new().merge(brain::brain_controller::routes()).merge(persona_controller::routes())
}
