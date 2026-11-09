//! Pairing mode on and off (start_bluetooth, stop_bluetooth: from the wiki, confirmed live on 01.11: a phone paired
//! and played).

use futures::future::BoxFuture;

use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::directives as commands;
use crate::skills::skill::{ExecutionContext, Outcome, OwnSkill};

pub struct BluetoothSkill;

impl OwnSkill for BluetoothSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Bluetooth
    }

    fn verified(&self, _intent: &Intent) -> bool {
        true
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        _context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            match intent.action.as_str() {
                "on" => Ok(Outcome::said("Включаю блютуз.", vec![commands::start_bluetooth()])),
                "off" => Ok(Outcome::said("Выключаю блютуз.", vec![commands::stop_bluetooth()])),
                other => anyhow::bail!("bluetooth: unknown action {other}"),
            }
        })
    }
}
