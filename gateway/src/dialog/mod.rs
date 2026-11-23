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
    speech_cut_note, with_error_phrase, AnsweredBy, DialogContext, DialogService, Filler, Inbox, ModelChoice,
    PhraseStream, ToolBox, ToolFilter, ToolOutcome, ToolRun, UsageSink, ERROR_PHRASE, LOOKUP_FILLER, SPEECH_MAX_TOKENS,
};
pub use llm::brain::{BrainProfile, BrainSource, BrainStatus, ModelSlot};
pub use persona_service::PersonaService;
pub use phrase_splitter::PhraseSplitter;
pub use system_prompt::Channel;

crate::route_needs!(
    Arc<brain::BrainService>,
    Arc<crate::secrets::SecretsService>,
    Arc<crate::oauth::OAuthService>,
    Arc<PersonaService>
);

/// The panel's routes of the dialog: `/api/v1/brain/…` (the providers and the choice) and `/api/v1/persona`.
pub fn routes<S: RouteNeeds>() -> Router<S> {
    Router::new().merge(brain::brain_controller::routes()).merge(persona_controller::routes())
}
