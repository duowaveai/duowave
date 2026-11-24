//! DuoWave's floor «Уши» (docs/ARCHITECTURE.md, «Этажи»; docs/plans/modular-gateway.md): the ears' adapters to the
//! ear contract — the Station's (Yandex's protocol, the uniproxy server and sessions, the relay to Yandex, the
//! Station's commands: `gateway`, behind the feature `yandex-station`) —, the stations and their connection
//! (`stations`), Alice's listener (`activator`), the panel's live channel (`live`) and the requests' journal
//! (`requests`). It stands on the base floor, «Права», «Голос» and «Мозг»; the gateway (`duowave`) passes its modules
//! on under their own names.

// The floors below under their own names, as the gateway passes them: `crate::core`, `crate::talk` read as they did.
pub use duowave_base::{config, core, locale, operations, platform, secrets, settings, storage, utils};
pub use duowave_base::{route_needs, tr, ApiError, VERSION};
pub use duowave_brain::{characters, dialog, memory, oauth, router, skills, sleep, talk, tasks};
pub use duowave_rights::{access, rights};
pub use duowave_voice::{ear, models, speech, voice_lab};

#[cfg(feature = "yandex-station")]
pub mod activator;
pub mod gateway;
pub mod live;
pub mod requests;
pub mod stations;
