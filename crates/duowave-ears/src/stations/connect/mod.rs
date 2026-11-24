//! Connecting a station from the panel: the scripts for doing it by hand (`/station/connect.sh`), served by every
//! build, the station's config read (quasar_config.rs) and its neighbours. Finding it (mDNS), reading its config over
//! SSH, pointing its voice at the gateway and giving it back — the computer's floor (crates/duowave-pc, under this
//! module's name).

pub mod neighbours;
pub mod quasar_config;
pub mod station_scripts;
