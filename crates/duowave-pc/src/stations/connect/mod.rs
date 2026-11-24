//! Connecting a station from the panel — the computer's part, added to «Уши»'s `stations::connect` under its name:
//! finding it (mDNS, discovery.rs), reading its config over SSH and pointing its voice at the gateway or giving it
//! back (station_connect.rs, over station_shell.rs).

pub use duowave_ears::stations::connect::*;

pub mod discovery;
pub mod station_connect;
pub mod station_shell;
