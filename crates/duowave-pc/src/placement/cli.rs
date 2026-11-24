//! `duowave station …` on the computer: «Дом»'s commands with the computer's `setup` — DuoWave onto the station over
//! SSH, in one go (setup.rs).

pub use duowave_home::placement::cli::*;

pub fn command(args: &[String]) -> i32 {
    duowave_home::placement::cli::command_with(args, Some(super::setup::command))
}
