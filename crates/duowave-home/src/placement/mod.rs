//! Where the gateway runs (docs/plans/station-placement.md): on the computer, on the station, or on the station with
//! the computer as its home server. What works in each (availability.rs), the gateway's state record (state.rs), the
//! station's own files and commands (station_fs.rs, cli.rs), the station's build inside the computer's
//! (payload.rs), the pair's key (peer.rs), the settings that travel between the two (transfer.rs, backup.rs). On the
//! computer (crates/duowave-pc, under this module's name) the moves between placements, its watch over the station's
//! gateway, the helper's pass to it, the relay of its live channel to the computer's panels and of the computer's
//! Telegram bot's messages to it; what the placement asks of them — computer.rs.

pub mod availability;
pub mod backup;
pub mod carry;
pub mod cli;
pub mod computer;
pub mod first_code;
pub mod payload;
pub mod peer;
pub mod routes;
pub mod service;
pub mod station_fs;
pub mod transfer;

pub use availability::{AvailabilityService, HelperSource, PlacementSource};
pub use routes::router;
pub use service::PlacementService;

crate::route_needs!(
    std::sync::Arc<PlacementService>,
    std::sync::Arc<crate::config::Config>,
    std::sync::Arc<crate::storage::Db>,
    std::sync::Arc<crate::access::AccessService>,
    std::sync::Arc<AvailabilityService>,
    std::sync::Arc<crate::core::identity::home::Identity>,
    std::sync::Arc<crate::home::join::Joins>,
    std::sync::Arc<crate::plugins::files::PluginFiles>,
    std::sync::Arc<crate::secrets::SecretsService>,
    crate::core::events::EventBus,
);
