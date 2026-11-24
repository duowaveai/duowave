//! DuoWave's floor «Приложения» (docs/ARCHITECTURE.md, «Этажи»; docs/plans/modular-gateway.md): packages and their
//! apps — the manifest and the Host API, the plugins' engines, the catalog and the workshop (`plugins`), the apps' own
//! programs (`apps`) —, the cards, routes and providers of «Связи» (`bus`), capabilities (`capabilities`), the panel's
//! connections (`connectors`), MCP (`mcp`), the screen (`screen`) and MIDI keyboards (`midi`). It stands on every floor
//! below; the gateway (`duowave`) passes its modules on under their own names.

// The floors below under their own names, as the gateway passes them: `crate::core`, `crate::talk` read as they did.
pub use duowave_base::{config, core, locale, operations, platform, secrets, settings, storage, utils};
pub use duowave_base::{route_needs, tr, ApiError, VERSION};
pub use duowave_brain::{characters, dialog, memory, oauth, router, skills, sleep, talk, tasks};
#[cfg(feature = "yandex-station")]
pub use duowave_ears::activator;
pub use duowave_ears::{gateway, live, requests, stations};
pub use duowave_rights::{access, rights};
pub use duowave_voice::{ear, models, speech, voice_lab};

pub mod apps;
pub mod bus;
pub mod capabilities;
pub mod connectors;
pub mod mcp;
pub mod midi;
pub mod plugins;
pub mod screen;
