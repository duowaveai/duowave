//! Plugins: what the gateway learns without being rebuilt (docs/plans/plugins.md, stage 2). A plugin is a package in
//! PACKAGES_DIR — a manifest and an ES module — that the admin switches on; packages of the other kinds (apps,
//! characters, themes) share the folder, the format and the list. Its code runs in an engine behind
//! `PluginRuntime` (QuickJS now), in a context of its own with its memory budget and time limits, loaded at its
//! first call and unloaded after a while without one. What it may do is the Host API v1, every call checked
//! against the rights in its manifest; what it gives the gateway it gives through the core's own extension points:
//! phrases to the router, a skill (`PluginSkill`), tools for «Мозг» (`ToolContribution`), sources for our player.
//! Packages come from the catalog (`catalog.rs`: one feed for every kind) through the installer (`install.rs`), or
//! are copied in by hand.

pub mod agent_files;
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
mod quickjs_memory;
pub mod registry;
pub mod routes;
pub mod runtime;
pub mod schedule;
pub mod skill;
pub mod task_pump;
#[cfg(feature = "wasm")]
pub mod wasm;
pub mod watch;
pub mod workshop;

pub use catalog::{CatalogDeps, PackageCatalog};
pub use controller::{character_package, PluginHost, PluginsDeps};
pub use routes::routes;

crate::route_needs!(
    Option<std::sync::Arc<history::History>>,
    std::sync::Arc<workshop::Workshop>,
    std::sync::Arc<crate::dialog::DialogService>,
    std::sync::Arc<PackageCatalog>,
    std::sync::Arc<PluginHost>,
    std::sync::Arc<crate::bus::Bus>,
    std::sync::Arc<crate::characters::CharactersService>,
    std::sync::Arc<files::PluginFiles>,
    std::sync::Arc<schedule::Scheduler>,
    std::sync::Arc<watch::WatchPages>,
    std::sync::Arc<crate::secrets::SecretsService>,
    std::sync::Arc<crate::settings::SettingsService>,
    std::sync::Arc<crate::dialog::brain::BrainService>,
    std::sync::Arc<crate::capabilities::publish::Publishers>,
    std::sync::Arc<dyn routes::AgentSeats>
);
