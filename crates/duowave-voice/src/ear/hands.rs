//! The ears' hands (docs/plans/audio-thinking-contract.md, «Hands»; audio-interface-3.md, «The door to Thinking»,
//! point 3): what each kind of ear declares it can do. The spec is the door's (`door::HandSpec`); the declaration is
//! here, beside the ear's commands (command.rs), because it says what those commands reach.

use crate::core::capabilities::CAPABILITY_NAMES;
use crate::door::HandSpec;

/// A station's hands: what acts on it with commands in the answer today (`Capability.station`), a group each.
pub fn station() -> Vec<HandSpec> {
    CAPABILITY_NAMES
        .iter()
        .map(|name| (name, name.info()))
        .filter(|(_, info)| info.station)
        .map(|(name, info)| HandSpec::group(name.as_str(), info.title))
        .collect()
}

/// The computer's hands: none yet. Later its own — the sound, the player's keys, focus mode, notifications.
pub fn computer() -> Vec<HandSpec> {
    Vec::new()
}

/// A command as the door's hand: its name and the command itself as the arguments.
pub fn hand_of(command: &super::Command) -> (String, serde_json::Value) {
    (command.name().to_string(), serde_json::to_value(command).unwrap_or(serde_json::Value::Null))
}

/// The command a hand's arguments carry; None — not a command of this ear.
pub fn command_of(args: &serde_json::Value) -> Option<super::Command> {
    serde_json::from_value(args.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_station_declares_what_its_commands_reach_and_the_computer_nothing_yet() {
        let groups = crate::door::hands::groups(&station());
        for group in ["volume", "playback", "timers", "alarms"] {
            assert!(groups.iter().any(|name| name == group), "the station has no «{group}» in {groups:?}");
        }
        assert!(!groups.iter().any(|name| name == "chat"), "a talk is not a hand");
        assert!(computer().is_empty());
    }

    #[test]
    fn a_command_goes_through_the_door_and_comes_back_whole() {
        use crate::ear::{Command, MediaItem, TimerSpec};
        let media = MediaItem {
            id: "m".into(),
            title: "Радио".into(),
            url: "https://example.org/r".into(),
            format: "MP3".into(),
            codec: None,
            r#type: "radio".into(),
            subtitle: None,
            image_url: None,
            duration_ms: None,
        };
        for command in [
            Command::SetVolume { level: 4.0, whisper: false },
            Command::StopPlayer,
            Command::DoNotDisturb(true),
            Command::TimerSet(TimerSpec { id: "t".into(), start: 1, seconds: 240 }),
            Command::Play { item: media, play_id: "p".into(), offset_ms: 0 },
            Command::LightAnimation(vec![1, 2, 3]),
        ] {
            let (hand, args) = hand_of(&command);
            assert_eq!(hand, command.name());
            assert_eq!(command_of(&args), Some(command));
        }
        assert_eq!(command_of(&serde_json::json!({"fly": {}})), None);
    }
}
