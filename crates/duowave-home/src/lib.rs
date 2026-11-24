//! DuoWave's floor «Дом» (docs/ARCHITECTURE.md, «Этажи»; docs/plans/modular-gateway.md): where the gateway stands and
//! its stations — a computer's or the station's own, a move between them (`placement`) —, the helpers and the link
//! between computers (`helpers`), the home's devices, its centre and its people's words (`home`), the home server a
//! station leans on (`home_server`), the way out to the internet (`egress`), the updates (`updates`) and the health
//! page (`health_status`). It stands on every floor below; the gateway (`duowave`) passes its modules on under their
//! own names.

// The floors below under their own names, as the gateway passes them: `crate::core`, `crate::plugins` read as they did.
pub use duowave_apps::{apps, bus, capabilities, connectors, mcp, midi, plugins, screen};
pub use duowave_base::{config, core, locale, operations, platform, secrets, settings, storage, utils};
pub use duowave_base::{route_needs, tr, ApiError, VERSION};
pub use duowave_brain::{characters, dialog, memory, oauth, router, skills, sleep, talk, tasks};
#[cfg(feature = "yandex-station")]
pub use duowave_ears::activator;
pub use duowave_ears::{gateway, live, requests, stations};
pub use duowave_rights::{access, rights};
pub use duowave_voice::{ear, models, speech, voice_lab};

pub mod egress;
pub mod health_status;
pub mod helpers;
pub mod home;
pub mod home_server;
pub mod placement;
pub mod updates;
