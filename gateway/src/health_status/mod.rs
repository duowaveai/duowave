//! The gateway's health: `/health`, for the deploy's checks and the panel's dashboard.

pub mod controller;
pub mod service;

pub use controller::routes;
pub use service::{HealthSources, HealthStatus, HealthStatusService, PublicHealth, StationHealth};
