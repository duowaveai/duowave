//! A device's hands (the contract's «Hands»): one thing a device can do — set the sound, start a timer, play the
//! radio. An ear declares the hands it has (the station's from what it does today, the computer none yet); Thinking
//! chooses who acts and sends `hands`; the audio side runs the hand on that device's ear and replies `hands_done`. The
//! hands of a place replace the old `Capability.station` and `ToolsKind`: a talk is offered what can be done where it
//! happens, whatever the device.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One hand, as `devices()` lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandSpec {
    /// `sound.set`, `timers.start`; a group's whole set of hands is named by the group alone.
    pub hand: String,
    /// `sound`, `timers`.
    pub group: String,
    pub title: String,
    /// A JSON schema of the arguments.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub args: Value,
    /// Thinking asks the person first.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub risky: bool,
    /// What the hand does, in English, for the model's tool description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
}

impl HandSpec {
    /// A group as one hand: what an ear declares before it names its hands one by one.
    pub fn group(group: &str, title: &str) -> HandSpec {
        HandSpec {
            hand: group.to_string(),
            group: group.to_string(),
            title: title.to_string(),
            args: Value::Null,
            risky: false,
            about: None,
        }
    }
}

/// The group of a device that has an assistant of its own (Alice on a station whose mode lets Yandex in): Thinking
/// may hand a phrase to it (`handoff`), decided by the data, not by the kind of the device.
pub const ASSISTANT: &str = "assistant";

/// The groups of the hands of a place: what a phrase's `hands` names.
pub fn groups(hands: &[HandSpec]) -> Vec<String> {
    let mut groups: Vec<String> = Vec::new();
    for hand in hands {
        if !groups.contains(&hand.group) {
            groups.push(hand.group.clone());
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_is_named_once_whatever_its_hands() {
        let hands = [
            HandSpec::group("sound", "Громкость"),
            HandSpec { hand: "timers.start".into(), ..HandSpec::group("timers", "Таймеры") },
            HandSpec { hand: "timers.stop".into(), ..HandSpec::group("timers", "Таймеры") },
        ];
        assert_eq!(groups(&hands), ["sound", "timers"]);
        assert_eq!(
            serde_json::to_value(&hands[0]).unwrap(),
            serde_json::json!({"hand": "sound", "group": "sound", "title": "Громкость"})
        );
    }
}
