//! Voice activity detection: Silero VAD on ONNX Runtime (the `silero` feature, PC builds), or the energy VAD.

pub mod energy_vad;
#[cfg(feature = "silero")]
pub mod silero_vad;
pub mod vad_engine;
