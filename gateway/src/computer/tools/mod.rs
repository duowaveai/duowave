//! The computer's own commands beside `serve` (src/tools): `call-sim` and `livekit-token` (the calls'), `asr-check`
//! (recognition on this computer), `pretend` (the pretend stations' quasar and Yandex).

pub mod asr_check;
pub mod call_sim;
pub mod livekit_token;
#[cfg(feature = "yandex-station")]
pub mod pretend;

use crate::tools::{absolute, config, flag, option, runtime, Failure, Usage};

/// One of the computer's commands, with its arguments; None — not one of its.
pub fn run(name: &str, args: &[String]) -> Option<anyhow::Result<()>> {
    Some(match name {
        "call-sim" => call_sim::main(args),
        "livekit-token" => livekit_token::main(args),
        "asr-check" => asr_check::main(args),
        #[cfg(feature = "yandex-station")]
        "pretend" => pretend::main(args),
        _ => return None,
    })
}
