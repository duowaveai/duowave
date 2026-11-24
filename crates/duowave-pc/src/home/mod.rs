//! The home — the computer's part, added to «Дом»'s `home` under its name: «Поиск дома» (announce.rs), the home as this
//! computer sees it when it is a Турбо (devices.rs, ear.rs), and the computer's answer to what the home asks of it
//! (computer.rs).

pub use duowave_home::home::*;

pub mod announce;
pub mod computer;
pub mod devices;
pub mod ear;
