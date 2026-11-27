//! The place's usual tools (docs/plans/thinking.md, §4): the tools called most on each device over the last days
//! (`thinking.card.usual_days`, 14), from the turns' lines. They go on the card's stable part, so they are counted once
//! a night, as a part of the sleep (`thinking_usual`), and kept in a small setting: the card changes at most once a
//! day, and the provider's cache holds. Example: the kitchen's station gets `set_timer`, `get_weather` and
//! `play_radio`; the computer gets `work_start`.

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use futures::FutureExt;
use serde::{Deserialize, Serialize};

use crate::core::identity::principal::Principal;
use crate::core::requests::{LineRow, RequestLines};
use crate::core::settings::{SettingDefinition, SettingScope};
use crate::settings::SettingsService;
use crate::sleep::{Night, PartReport, PartState, SleepPart};

use super::line::Line;
use super::settings::Settings;

const DAY_MS: u64 = 86_400_000;

/// What the nights counted: by device, each tool's calls.
pub const USUAL: SettingDefinition<Usual> = SettingDefinition {
    key: "thinking_usual",
    scope: SettingScope::Global,
    description: "Привычные инструменты мест",
    sensitive: false,
    parse: |value| serde_json::from_value(value.clone()).ok(),
};

/// The count of a night.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usual {
    /// When it was counted, ms since the epoch.
    pub at: u64,
    /// A device → a tool → its calls.
    pub devices: BTreeMap<String, BTreeMap<String, usize>>,
    /// The count before this one: what «Откатить ночь» brings back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Box<Usual>>,
}

/// Tools never counted as usual: they are on every card, or they only open others.
const NOT_USUAL: &[&str] = &["recall", "open", "use"];

impl Usual {
    /// The count of these lines.
    pub fn count(rows: &[LineRow], at: u64) -> Usual {
        let mut devices: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
        for row in rows {
            let Ok(line) = serde_json::from_value::<Line>(row.line.clone()) else { continue };
            for call in line.rounds.iter().flat_map(|round| round.calls.iter()) {
                if NOT_USUAL.contains(&call.as_str()) {
                    continue;
                }
                *devices.entry(row.device.clone()).or_default().entry(call.clone()).or_default() += 1;
            }
        }
        Usual { at, devices, before: None }
    }

    /// The device's usual tools, the most called first, at most `n`.
    pub fn top(&self, device: &str, n: usize) -> Vec<String> {
        let Some(counts) = self.devices.get(device) else { return Vec::new() };
        let mut tools: Vec<(&String, &usize)> = counts.iter().collect();
        tools.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        tools.into_iter().take(n).map(|(tool, _)| tool.clone()).collect()
    }

    /// How often each tool was called on the device.
    pub fn counts(&self, device: &str) -> BTreeMap<String, usize> {
        self.devices.get(device).cloned().unwrap_or_default()
    }

    /// The count as the home keeps it.
    pub fn of(settings: &SettingsService) -> Usual {
        settings.get(&USUAL, None).unwrap_or_default()
    }
}

/// The sleep's part that counts the places' usual tools.
pub struct UsualPart {
    pub lines: Arc<dyn RequestLines>,
    pub settings: Arc<SettingsService>,
}

impl UsualPart {
    fn rows(&self) -> Vec<LineRow> {
        let days = Settings::of(&self.settings).card.usual_days.max(1) as u64;
        let since = crate::utils::time::now_ms().saturating_sub(days * DAY_MS);
        self.lines.lines(since, None, 50_000)
    }
}

impl SleepPart for UsualPart {
    fn id(&self) -> &'static str {
        "thinking_usual"
    }

    fn title(&self) -> String {
        crate::tr!("Привычные инструменты")
    }

    fn enabled(&self) -> bool {
        true
    }

    fn work(&self) -> Result<(), String> {
        if self.rows().is_empty() {
            return Err(crate::tr!("Ходов «Мышления» за эти дни нет."));
        }
        Ok(())
    }

    fn run<'a>(&'a self, _night: &'a Night) -> BoxFuture<'a, PartReport> {
        async move {
            let before = Usual::of(&self.settings);
            let mut counted = Usual::count(&self.rows(), crate::utils::time::now_ms());
            counted.before = Some(Box::new(Usual { before: None, ..before }));
            let devices = counted.devices.len();
            let lines: Vec<String> = counted
                .devices
                .keys()
                .map(|device| format!("{device}: {}", counted.top(device, 3).join(", ")))
                .collect();
            match self.settings.set(&USUAL, &counted, None) {
                Ok(()) => PartReport {
                    lines,
                    ..PartReport::new(
                        self,
                        PartState::Done,
                        crate::tr!("Привычные инструменты посчитаны: мест {count}.", count = devices.to_string()),
                    )
                },
                Err(error) => PartReport::new(self, PartState::Failed, error.message.clone()),
            }
        }
        .boxed()
    }

    fn traces(&self, _run_id: &str) -> usize {
        0
    }

    fn details(&self, _run_id: &str) -> serde_json::Value {
        let usual = Usual::of(&self.settings);
        serde_json::json!({
            "at": usual.at,
            "devices": usual.devices.keys().map(|device| (device.clone(), usual.top(device, 3))).collect::<BTreeMap<_, _>>(),
        })
    }

    fn roll_back(&self, _run_id: &str, actor: &Principal) -> Result<usize, String> {
        let usual = Usual::of(&self.settings);
        let Some(before) = usual.before else { return Ok(0) };
        self.settings
            .set(&USUAL, &*before, Some(crate::core::events::Actor::from(actor)))
            .map_err(|error| error.message.clone())?;
        Ok(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(device: &str, calls: &[&str]) -> LineRow {
        LineRow {
            id: "r".into(),
            device: device.into(),
            at: 0,
            line: json!({"settings": "s", "variants": {"thinking": "ours", "card": "all", "rounds": "today", "delivery": "append"},
                         "kind": "station", "voice": true, "agent": {"by": "word"}, "end": "answered", "ms": 1,
                         "rounds": [{"ms": 1, "in": 1, "out": 1, "calls": calls}]}),
        }
    }

    #[test]
    fn the_most_called_tools_of_each_place_without_recall() {
        let rows = [
            row("kitchen", &["set_timer", "recall"]),
            row("kitchen", &["get_weather"]),
            row("kitchen", &["set_timer"]),
            row("kitchen", &["play_radio", "set_timer"]),
            row("computer", &["work_start"]),
        ];
        let usual = Usual::count(&rows, 1);
        assert_eq!(usual.top("kitchen", 3), ["set_timer", "get_weather", "play_radio"]);
        assert_eq!(usual.top("kitchen", 1), ["set_timer"]);
        assert_eq!(usual.top("computer", 3), ["work_start"]);
        assert!(usual.top("hall", 3).is_empty());
        assert_eq!(usual.counts("kitchen").get("recall"), None);
    }
}
