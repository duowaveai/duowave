//! Recognition on this computer itself, without Python or a server: GigaAM, Parakeet and Whisper as GGUF through
//! transcribe.cpp (MIT), the way Handy does it (docs/plans/speech-servers.md). The model runs in a worker — this same
//! program started with `--asr-worker` — that the gateway talks to over the worker's stdin and stdout and that ends
//! with the gateway. A crash or a hang of native code (a GPU driver's too) takes down only the worker.
//!
//! - catalog.rs — the models and their pinned files;
//! - download.rs — a file fetched whole, resumed, checked by SHA-256;
//! - protocol.rs — the frames between the gateway and the worker;
//! - supervisor.rs — the gateway's side: the worker kept serving, replaced, moved to the processor;
//! - recognizer.rs — the gateway's `SpeechRecognizer` over it;
//! - worker.rs — the worker's side (feature `local-asr`: it alone links transcribe.cpp).
//!
//! Everything but the worker builds with `pc`; the gateway runs it only in a build with `local-asr`
//! ([`AVAILABLE`]), which every program that may start a worker checks for first ([`is_worker_invocation`]).

pub mod catalog;
pub mod download;
pub mod protocol;
pub mod recognizer;
pub mod supervisor;
#[cfg(feature = "local-asr")]
mod worker;

/// The first argument that makes this program a recognition worker.
pub const WORKER_FLAG: &str = "--asr-worker";
/// A worker's argument: the processor only, no GPU driver loaded at all (after a GPU failed).
pub const CPU_ONLY_FLAG: &str = "--cpu-only";

/// Whether this build can recognise by itself.
pub const AVAILABLE: bool = cfg!(feature = "local-asr");

/// Whether this process was started as a worker: checked first thing in `main`, before anything else starts.
pub fn is_worker_invocation() -> bool {
    std::env::args_os().nth(1).is_some_and(|arg| arg == WORKER_FLAG)
}

/// Serves as a worker until the gateway closes its stdin; the exit code.
#[cfg(feature = "local-asr")]
pub fn run_worker() -> i32 {
    worker::run()
}
