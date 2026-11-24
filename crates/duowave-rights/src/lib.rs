//! DuoWave's floor «Права» (docs/ARCHITECTURE.md, «Этажи»; docs/plans/modular-gateway.md), right above the base:
//! the core of «Связи» — who may reach whom and how far, for everything in the system: apps, the gateway's services,
//! folders and files, devices, people (`rights`) — and people's own rights: signing in, roles, API keys (`access`).
//! It stands on the base floor alone; the gateway (`duowave`) passes both modules on under their own names.

// The base floor's modules under their own names, as the gateway passes them: `crate::core` reads as it did.
pub use duowave_base::{config, core, locale, operations, platform, secrets, settings, storage, utils};
pub use duowave_base::{route_needs, tr, ApiError, VERSION};

pub mod access;
pub mod rights;
