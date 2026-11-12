//! Our own synthesis: an OpenAI-compatible /audio/speech client, and the text made sayable for it (numbers in words,
//! Latin in Russian letters).

#[cfg(feature = "pc")]
pub mod managed;
pub mod own_speech;
pub mod say_latin;
pub mod say_numbers;
