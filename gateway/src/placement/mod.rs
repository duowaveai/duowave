//! Where the gateway runs (docs/plans/station-placement.md): on the computer, on the station, or on the station with
//! the computer as its home server. What works in each (availability.rs), the gateway's state record (state.rs), the
//! station's own files and commands (station_fs.rs, cli.rs), the station's build inside the computer's
//! (payload.rs), the pair's key (peer.rs), the settings that travel between the two (transfer.rs, backup.rs), and on
//! the computer the moves between placements (moves.rs) and its watch over the station's gateway (watch.rs).

pub mod availability;
pub mod backup;
pub mod cli;
#[cfg(feature = "pc")]
pub mod moves;
pub mod payload;
pub mod peer;
pub mod routes;
pub mod service;
pub mod station_fs;
pub mod transfer;
#[cfg(feature = "pc")]
pub mod watch;

pub use availability::{AvailabilityService, HomeServerSource, PlacementSource};
pub use routes::router;
pub use service::PlacementService;
