//! The station's mode — Яндекс, Шлюз, Свой — the one source of truth for it (mode_service.rs).

pub mod mode_service;

pub use mode_service::{ModeService, ModeView, SessionKind};
