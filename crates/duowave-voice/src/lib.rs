//! DuoWave's floor «Голос» (docs/ARCHITECTURE.md, «Этажи»; docs/plans/modular-gateway.md): hearing and speaking —
//! recognition, synthesis and the audio (`speech`), the voice lab where the household makes its own voices
//! (`voice_lab`), the shop of voice and recognition models (`models`), and the ear contract — what an ear can do and
//! say, whatever it is: a Station, the computer's microphone, a box of our own (`ear`). It stands on the base floor
//! alone; the gateway (`duowave`) passes its modules on under their own names.

// The base floor's modules under their own names, as the gateway passes them: `crate::core` reads as it did.
pub use duowave_base::{config, core, locale, operations, platform, secrets, settings, storage, utils};
pub use duowave_base::{route_needs, tr, ApiError, VERSION};

pub mod ear;
pub mod models;
pub mod speech;
pub mod voice_lab;
