//! The stations the gateway serves: their approval and records, their presence, their look, and connecting them
//! from the panel.

pub mod connect;
pub mod firmware;
/// The stations' looks are the base floor's (core/stations/looks.rs): a gateway's default name reads them.
pub use crate::core::stations::looks;
pub mod presence;
pub mod pretend;
pub mod routes;
pub mod service;

pub use routes::router;

pub use presence::PresenceService;
pub use service::{Admission, StationEntry, StationStatus, StationView, StationsService, UNKNOWN_DEVICE};

crate::route_needs!(std::sync::Arc<StationsService>, std::sync::Arc<crate::config::Config>);
