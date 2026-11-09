//! The stations the gateway serves: their approval and records, their presence, their look, and connecting them
//! from the panel.

pub mod connect;
pub mod firmware;
pub mod looks;
pub mod presence;
pub mod routes;
pub mod service;

pub use routes::router;

pub use presence::PresenceService;
pub use service::{Admission, StationEntry, StationStatus, StationView, StationsService, UNKNOWN_DEVICE};
