//! Volume, 0–10 on the station. "Louder" and "quieter" become sound_set_level from the level the station reports in
//! the request, so "громче на два" works and only the command seen in our journal is sent; without that level the
//! relative commands from the wiki go instead.

use futures::future::BoxFuture;

use crate::ear::Command;
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::js;
use crate::skills::skill::{ExecutionContext, Outcome, OwnSkill};

pub struct VolumeSkill;

impl OwnSkill for VolumeSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Volume
    }

    /// Every command of ours is confirmed live; sound_mute and sound_unmute on 01.11.
    fn verified(&self, _intent: &Intent) -> bool {
        true
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move { volume(intent, context) })
    }
}

fn volume(intent: &Intent, context: &ExecutionContext) -> anyhow::Result<Outcome> {
    let state = context.device_state.as_ref();
    let max = state.and_then(|state| state.sound_max_level).filter(|max| *max != 0.0 && !max.is_nan()).unwrap_or(10.0);
    let current = state.and_then(|state| state.sound_level);
    let steps = js::number_param(&intent.params, "steps").filter(|steps| *steps > 0.0).unwrap_or(1.0);
    Ok(match intent.action.as_str() {
        "louder" => Outcome::commands(vec![match current {
            None => Command::Louder,
            Some(current) => Command::SetVolume { level: max.min(current + steps), whisper: false },
        }]),
        "quieter" => Outcome::commands(vec![match current {
            None => Command::Quieter,
            Some(current) => Command::SetVolume { level: (current - steps).max(0.0), whisper: false },
        }]),
        "set" => {
            let said = js::to_number(intent.params.get("level"));
            if !said.is_finite() {
                return Ok(Outcome::speech("Не расслышала, какую громкость поставить."));
            }
            // «Громкость 60» is percent (live, 01.11: 60 and 70 set the maximum).
            let level = if said > max && said <= 100.0 { js::round(said * max / 100.0) } else { said };
            Outcome::commands(vec![Command::SetVolume { level: level.min(max).max(0.0), whisper: false }])
        }
        "max" => Outcome::commands(vec![Command::SetVolume { level: max, whisper: false }]),
        "mute" => Outcome::commands(vec![Command::Mute]),
        "unmute" => Outcome::commands(vec![Command::Unmute]),
        "status" => Outcome::speech(match current {
            None => "Не знаю, какая сейчас громкость.".to_string(),
            Some(current) => format!("Громкость {}.", js::number_text(current)),
        }),
        other => anyhow::bail!("volume: unknown action {other}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ear::DeviceState;
    use chrono::Utc;
    use serde_json::{json, Value};

    fn context(sound_level: Option<f64>) -> ExecutionContext {
        let mut context = ExecutionContext::new("h:test", Utc::now());
        context.device_state = sound_level.map(|level| DeviceState {
            sound_level: Some(level),
            sound_max_level: Some(10.0),
            ..DeviceState::default()
        });
        context
    }

    async fn run(action: &str, params: Value, sound_level: Option<f64>) -> Outcome {
        let intent = Intent {
            capability: CapabilityName::Volume,
            action: action.into(),
            params: params.as_object().cloned().unwrap(),
        };
        VolumeSkill.execute(&intent, &context(sound_level)).await.unwrap()
    }

    /// The level the ear is told (the Station's adapter rounds it and keeps it in 0–10: src/gateway/commands/basic.rs).
    fn level(outcome: &Outcome) -> f64 {
        match outcome.commands[0] {
            Command::SetVolume { level, .. } => level,
            ref other => panic!("not a volume: {other:?}"),
        }
    }

    #[tokio::test]
    async fn makes_louder_by_two_a_level_from_the_station_state_the_command_we_saw_yandex_send() {
        let outcome = run("louder", json!({"steps": 2}), Some(4.0)).await;
        assert_eq!(outcome.commands, vec![Command::SetVolume { level: 6.0, whisper: false }]);
    }

    #[tokio::test]
    async fn keeps_the_level_within_the_scale() {
        let outcome = run("quieter", json!({"steps": 3}), Some(1.0)).await;
        assert_eq!(level(&outcome), 0.0);
    }

    #[tokio::test]
    async fn takes_a_level_above_the_scale_for_percent() {
        for (said, level) in [(60, 6), (100, 10), (7, 7), (150, 10)] {
            assert_eq!(super::tests::level(&run("set", json!({"level": said}), Some(4.0)).await), level as f64);
        }
    }

    #[tokio::test]
    async fn falls_back_to_the_relative_command_without_the_station_state() {
        let outcome = run("louder", json!({}), None).await;
        assert_eq!(outcome.commands[0], Command::Louder);
    }
}
