//! Commands for the station, the way Yandex sends them (relay journal, docs/research/original-alice.md) or the
//! way the Yandex Station Extended wiki describes them. `VERIFIED_COMMANDS` says which: a command seen in our
//! journal works on our station; one known only from the wiki is tried only when its route has no Yandex to fall
//! back to.

use serde_json::{json, Map, Value};

use super::js;
use crate::gateway::protocol::protocol_types::{Directive, DirectiveType};

fn action(name: &str, payload: Option<Value>, analytics_type: Option<&str>) -> Directive {
    Directive {
        r#type: DirectiveType::ClientAction,
        name: name.to_string(),
        analytics_type: analytics_type.map(str::to_string),
        payload: payload.map(object),
        typed_payload: None,
        is_led_silent: Some(true),
        ignore_answer: None,
    }
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// Commands whose format we saw Yandex send to our station, or that it carried out as we sent them (live checks).
pub const VERIFIED_COMMANDS: &[&str] = &[
    "sound_set_level",
    "player_pause",
    "clear_queue",
    "tts_play_placeholder",
    "audio_play",
    "timer_set_directive",
    "timer_stop_directive",
    "timer_cancel_directive",
    "timer_pause_directive",
    "timer_resume_directive",
    "alarm_add_directive",
    "alarm_cancel_directive",
    "alarm_stop_directive",
    "sound_mute",
    "sound_unmute",
    "start_bluetooth",
    "stop_bluetooth",
    "end_dialog_session",
    "close_dialog",
];

pub fn all_verified(directives: &[Directive]) -> bool {
    directives.iter().all(|directive| VERIFIED_COMMANDS.contains(&directive.name.as_str()))
}

// ---------------------------------------------------------------- volume (0–10 on the station)

/// `is_whisper`: the station lowers its volume for this answer only, as on a whispered question.
pub fn set_volume(level: f64, whisper: bool) -> Directive {
    let level = js::round(level).clamp(0.0, 10.0);
    let mut payload = json!({ "new_level": js::number_value(level) });
    if whisper {
        payload["is_whisper"] = Value::Bool(true);
    }
    action("sound_set_level", Some(payload), None)
}

pub fn louder() -> Directive {
    action("sound_louder", None, None)
}

pub fn quieter() -> Directive {
    action("sound_quiter", None, None)
}

pub fn mute() -> Directive {
    action("sound_mute", None, None)
}

pub fn unmute() -> Directive {
    action("sound_unmute", None, None)
}

// ---------------------------------------------------------------- player

/// "Стоп", "пауза": Yandex sends both, `clear_queue` alone while radio plays.
pub fn stop_player() -> Vec<Directive> {
    vec![pause_player(), action("clear_queue", Some(json!({})), None)]
}

pub fn pause_player() -> Directive {
    action("player_pause", Some(json!({ "smooth": false })), None)
}

pub fn continue_player() -> Directive {
    action("player_continue", None, None)
}

pub fn next_track() -> Directive {
    action("player_next_track", None, None)
}

pub fn previous_track() -> Directive {
    action("player_previous_track", None, None)
}

// ---------------------------------------------------------------- bluetooth

pub fn start_bluetooth() -> Directive {
    action("start_bluetooth", None, None)
}

pub fn stop_bluetooth() -> Directive {
    action("stop_bluetooth", None, None)
}

// ---------------------------------------------------------------- dialogs

pub fn end_dialog(dialog_id: &str) -> Vec<Directive> {
    vec![
        action("end_dialog_session", Some(json!({ "dialog_id": dialog_id })), None),
        action("close_dialog", Some(json!({ "dialog_id": dialog_id })), None),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_commands_as_yandex_sends_them() {
        let level = set_volume(12.4, false);
        assert_eq!(level.name, "sound_set_level");
        assert_eq!(Value::Object(level.payload.unwrap()), json!({ "new_level": 10 }));
        assert_eq!(
            Value::Object(set_volume(3.5, true).payload.unwrap()),
            json!({ "new_level": 4, "is_whisper": true })
        );
        let stop = stop_player();
        assert_eq!(stop.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["player_pause", "clear_queue"]);
        assert_eq!(stop[1].payload, Some(Map::new()));
        assert_eq!(louder().payload, None);
        assert_eq!(louder().is_led_silent, Some(true));
        assert!(all_verified(&stop));
        assert!(!all_verified(&[louder()]));
    }
}
