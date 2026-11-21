//! Running the gateway day to day: home networks, retention, the station watch, the machine's memory and disk.

pub mod device_state;
// The station's own way out, and its outages (Wi-Fi drops of a tester's Mini 2, 22.11).
pub mod network_watch;
pub mod networks;
pub mod retention;
pub mod station_watch;
pub mod system_memory;

pub use retention::RetentionService;
pub use station_watch::{ActiveSessions, StationWatch, TelegramSender, WatchStatus};
