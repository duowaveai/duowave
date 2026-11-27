//! DuoWave's floor «Мозг» (docs/ARCHITECTURE.md, «Этажи»; docs/plans/modular-gateway.md): the conversation and its
//! models (`dialog`), the router that hands a phrase to a skill or to the model (`router`), signing in at the model
//! providers (`oauth`), the talk layer with its works (`talk`), memory (`memory`), the characters (`characters`), the
//! built-in skills (`skills`), the tasks (`tasks`), the night's sleep (`sleep`) and Thinking behind the door
//! (`thinking`). It talks to an ear through the ear
//! contract, never to a Station. It stands on the base floor and «Голос»; the gateway (`duowave`) passes its modules
//! on under their own names.

// The floors below under their own names, as the gateway passes them: `crate::core`, `crate::speech` read as they did.
pub use duowave_base::{config, core, locale, operations, platform, secrets, settings, storage, utils};
pub use duowave_base::{route_needs, tr, ApiError, VERSION};
pub use duowave_door::door;
pub use duowave_sense::sense;
pub use duowave_voice::{ear, models, speech, voice_lab};

pub mod characters;
pub mod dialog;
pub mod memory;
pub mod oauth;
pub mod router;
pub mod skills;
pub mod sleep;
pub mod talk;
pub mod tasks;
/// Thinking behind the door to the audio side (docs/plans/thinking.md): `OurThinking`, its parts and settings.
pub mod thinking;
