//! The computer's whole-gateway tests (moved from src/whole_tests): what only a computer has, tested with the whole
//! gateway built.

mod calls_controller;
mod characters_voices;
pub(crate) mod helpers_builtin;
mod helpers_clients;
mod helpers_link_serve;
mod helpers_link_tls;
mod helpers_local_models;
mod helpers_now_computer;
mod helpers_routes;
mod home_announce;
mod home_character_computer;
mod home_server_serve;
pub(crate) mod mcp_fixtures;
mod mcp_programs;
mod models_controller;
mod placement_computer;
mod placement_proxy;
mod placement_relay;
mod voice_lab_controller;
mod voice_lab_service;
pub(crate) mod voice_lab_testing;
