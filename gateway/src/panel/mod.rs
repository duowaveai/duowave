//! The «Стекло» control panel's files: the SPA at /panel. Its API is /api/v1 (src/access, src/gateway, src/models);
//! the desk of each person is here (desk.rs), and what the panel remembers about them (state.rs).

pub mod controller;
pub mod desk;
pub mod state;

pub use controller::{panel_policy, routes};
