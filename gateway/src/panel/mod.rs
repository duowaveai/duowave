//! The «Стекло» control panel's files: the SPA at /panel. Its API is /api/v1 (src/access, src/gateway, src/models).

pub mod controller;

pub use controller::{panel_policy, routes};
