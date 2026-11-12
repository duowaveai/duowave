//! Recognition: an OpenAI-compatible /audio/transcriptions, or the computer's own engine (local/), and the command
//! out of what it heard.

#[cfg(feature = "pc")]
pub mod local;
pub mod openai_transcriber;
pub mod speech_recognizer;
pub mod transcript;
