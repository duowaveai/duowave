//! The do-not-disturb mode (set_do_not_disturb_directive, from the wiki: field numbers assumed, not yet verified).
//!
//! The gateway also remembers the mode as people set it by voice (`note`, from the router: every «не беспокоить» passes
//! there, whoever serves it), so that apps keep quiet in it: speech without a request (`system/voice` → `say`,
//! links/plugins.rs) is not said while it is on. The station tells nothing of the mode itself, so a mode switched off
//! elsewhere (its button, Yandex's app) is not seen: the remembered «on» lapses after `LAPSE`.

use std::sync::atomic::{AtomicU64, Ordering};

use futures::future::BoxFuture;

use crate::ear::Command;
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::skill::{ExecutionContext, Outcome, OwnSkill};

/// How long a remembered «on» holds without a word: a night and a little more.
const LAPSE_MS: u64 = 12 * 3_600_000;

/// When the mode was switched on by voice, in ms since the epoch; 0 — off.
static ON_SINCE: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |at| at.as_millis() as u64)
}

/// The mode was set by voice: on or off.
pub fn note(on: bool) {
    ON_SINCE.store(if on { now_ms().max(1) } else { 0 }, Ordering::SeqCst);
}

/// Whether the mode is on as far as the gateway knows: set by voice and not lapsed.
pub fn is_on() -> bool {
    on_at(ON_SINCE.load(Ordering::SeqCst), now_ms())
}

fn on_at(since: u64, now: u64) -> bool {
    since != 0 && now.saturating_sub(since) < LAPSE_MS
}

pub struct DoNotDisturbSkill;

impl OwnSkill for DoNotDisturbSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::DoNotDisturb
    }

    fn verified(&self, _intent: &Intent) -> bool {
        false
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        _context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            match intent.action.as_str() {
                "on" => Ok(Outcome::said("Включаю режим «Не беспокоить».", vec![Command::DoNotDisturb(true)])),
                "off" => Ok(Outcome::said("Выключаю режим «Не беспокоить».", vec![Command::DoNotDisturb(false)])),
                other => anyhow::bail!("do_not_disturb: unknown action {other}"),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_remembered_mode_lapses() {
        assert!(!on_at(0, 1_000));
        assert!(on_at(1_000, 1_000 + 3_600_000));
        assert!(!on_at(1_000, 1_000 + LAPSE_MS));
    }
}
