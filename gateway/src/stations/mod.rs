//! The stations the gateway serves: their approval and records, their presence, their look, and connecting them
//! from the panel.

pub mod connect;
pub mod firmware;
pub mod looks;
pub mod presence;
#[cfg(feature = "pc")]
pub mod pretend;
pub mod routes;
pub mod service;

pub use routes::router;

pub use presence::PresenceService;
pub use service::{Admission, StationEntry, StationStatus, StationView, StationsService, UNKNOWN_DEVICE};

#[cfg(feature = "pc")]
crate::route_needs!(
    std::sync::Arc<StationsService>,
    std::sync::Arc<crate::config::Config>,
    std::sync::Arc<connect::station_connect::StationConnectService>,
    std::sync::Arc<crate::core::identity::home::Identity>,
);
#[cfg(not(feature = "pc"))]
crate::route_needs!(std::sync::Arc<StationsService>, std::sync::Arc<crate::config::Config>);
