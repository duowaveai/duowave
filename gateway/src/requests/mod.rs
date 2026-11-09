//! The requests going now and the journal of finished ones (src/requests): sessions start requests through the
//! `RequestStarter`, the panel follows them live (src/live) and pages the journal.

pub mod service;

pub use service::{routes, HistoryQuery, ModeSource, RequestsService, StoredMode, DEFAULT_KEEP_DAYS, RECENT};
