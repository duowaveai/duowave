//! Tests that need the whole gateway (`App`) of modules whose floors are crates of their own (crates/,
//! docs/plans/modular-gateway.md): they live in the program, which has it. A file for each module's tests.

mod access_audit;
mod access_cli;
mod access_controller;
mod ask_controller;
mod brain_controller;
mod caller;
mod characters;
pub(crate) mod characters_contributes;
mod connectors_controller;
#[cfg(feature = "yandex-station")]
mod debug_controller;
mod ear_mode;
mod helpers_home_voice;
pub(crate) mod helpers_link;
mod helpers_models;
mod helpers_now;
mod history_routes;
pub(crate) mod home_character;
mod home_join_routes;
mod home_server_messenger;
mod home_status;
pub(crate) mod mcp_controller;
mod memory_controller;
mod oauth_controller;
mod persona_controller;
pub(crate) mod placement_routes;
mod plugins_routes;
#[cfg(feature = "yandex-station")]
mod session_registry;
#[cfg(feature = "yandex-station")]
mod station_session;
mod stations_routes;
mod talk_layer;
mod talk_listen;
mod talk_ready;
mod talk_ready_gateway;
mod talk_rules_gateway;
mod workshop;
mod workshop_agent;
mod workshop_routes;
