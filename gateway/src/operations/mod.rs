//! Running the gateway day to day: home networks, retention, the station watch, the machine's memory and disk.

pub mod networks;
pub mod retention;
pub mod station_watch;
pub mod system_memory;

pub use retention::RetentionService;
pub use station_watch::{ActiveSessions, StationWatch, TelegramSender, WatchStatus};
