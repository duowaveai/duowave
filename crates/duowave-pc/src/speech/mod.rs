//! Hearing and speaking with the computer's own engines, added to «Голос»'s `speech` under its name: recognition on
//! this computer (`asr::local`: transcribe.cpp in a worker process) and the voice servers it runs (`tts::managed`).

pub use duowave_voice::speech::*;

pub mod asr;
pub mod tts;
