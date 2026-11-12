//! Speech: the station's Ogg/Opus in, the end of the command and its text out (gain, VAD, the endpointer,
//! recognition), and our own synthesis back (an OpenAI-compatible /audio/speech).

pub mod asr;
pub mod audio;
pub mod endpoint;
pub mod interim_recognition;
pub mod listen;
pub mod speech_pipeline;
pub mod speech_service;
pub mod test_audio;
pub mod tts;
pub mod vad;
pub mod voice_service;
pub mod wake_word_gains;

pub use speech_service::{Recognition, SpeechService, SpeechStatus};
pub use voice_service::VoiceService;
