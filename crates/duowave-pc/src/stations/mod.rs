//! The stations — the computer's part, added to «Уши»'s `stations` under its name: connecting one over SSH and finding
//! it (connect/), the pretend stations' answer and greeting (pretend.rs), the connect wizard's routes (routes.rs).

pub use duowave_ears::stations::*;

pub mod connect;
pub mod pretend;
pub mod routes;

pub use routes::router;

crate::route_needs!(
    std::sync::Arc<StationsService>,
    std::sync::Arc<crate::config::Config>,
    std::sync::Arc<connect::station_connect::StationConnectService>,
    std::sync::Arc<crate::core::identity::home::Identity>,
);
