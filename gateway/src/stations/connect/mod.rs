//! Connecting a station from the panel: finding it (mDNS), reading its config over SSH, pointing its voice at the
//! gateway and giving it back. The scripts for doing it by hand (`/station/connect.sh`) are served by every build;
//! the SSH, the mDNS discovery and the panel's wizard only by the PC build (`pc`).

pub mod quasar_config;
pub mod station_scripts;

#[cfg(feature = "pc")]
pub mod discovery;
#[cfg(feature = "pc")]
pub mod station_connect;
#[cfg(feature = "pc")]
pub mod station_shell;
