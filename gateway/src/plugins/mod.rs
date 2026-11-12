//! Plugins: what the gateway learns without being rebuilt (docs/plans/plugins.md, stage 2). A plugin is a package in
//! PACKAGES_DIR — a manifest and an ES module — that the admin switches on; packages of the other kinds (apps,
//! characters, themes) share the folder, the format and the list. Its code runs in an engine behind
//! `PluginRuntime` (QuickJS now), in a context of its own with its memory budget and time limits, loaded at its
//! first call and unloaded after a while without one. What it may do is the Host API v1, every call checked
//! against the rights in its manifest; what it gives the gateway it gives through the core's own extension points:
//! phrases to the router, a skill (`PluginSkill`), tools for «Мозг» (`ToolContribution`), sources for our player.
//! Packages come from the catalog (`catalog.rs`: one feed for every kind) through the installer (`install.rs`), or
//! are copied in by hand.

pub mod budget;
pub mod bundled;
pub mod catalog;
pub mod connector;
pub mod controller;
pub mod files;
pub mod history;
pub mod host_api;
pub mod install;
pub mod manifest;
pub mod mcp_runtime;
pub mod media;
pub mod phrases;
pub mod quickjs;
pub mod registry;
pub mod routes;
pub mod runtime;
pub mod schedule;
pub mod skill;
#[cfg(feature = "wasm")]
pub mod wasm;
pub mod workshop;

pub use catalog::{CatalogDeps, PackageCatalog};
pub use controller::{PluginHost, PluginsDeps};
pub use routes::routes;
