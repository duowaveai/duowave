//! The do-not-disturb mode (set_do_not_disturb_directive, from the wiki: field numbers assumed, not yet verified).

use futures::future::BoxFuture;

use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::skill::{ExecutionContext, Outcome, OwnSkill};
use crate::skills::typed_directives::do_not_disturb;

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
                "on" => Ok(Outcome::said("Включаю режим «Не беспокоить».", vec![do_not_disturb(true)])),
                "off" => Ok(Outcome::said("Выключаю режим «Не беспокоить».", vec![do_not_disturb(false)])),
                other => anyhow::bail!("do_not_disturb: unknown action {other}"),
            }
        })
    }
}
