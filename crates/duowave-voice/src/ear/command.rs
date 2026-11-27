//! What the brain asks an ear to do: its abilities — the volume, the player, what it plays, bluetooth, the end of a
//! skill's dialog, timers and alarms, «не беспокоить», the display. The skills and the apps decide which; the ear's
//! adapter says it in the ear's own words: the Station's are Yandex's commands (src/gateway/commands.rs).

use serde::{Deserialize, Serialize};

use super::media::MediaItem;

/// One thing an ear is asked to do in an answer, or pushed to it outside one. As JSON it goes through the door to
/// Thinking as a hand's arguments (ear/hands.rs), externally tagged by its name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    /// The volume, 0–10. `whisper`: lower for this answer only, as on a whispered question.
    SetVolume {
        level: f64,
        whisper: bool,
    },
    Louder,
    Quieter,
    Mute,
    Unmute,
    /// «Стоп»: the player pauses and its queue is cleared.
    StopPlayer,
    PausePlayer,
    ContinuePlayer,
    NextTrack,
    PreviousTrack,
    /// The ear's player plays `item` now from `offset_ms`, under the id our player gave it (`play_id`): what the ear
    /// tells of it (started, stopped, finished, failed) comes back under that id, and when it is not a radio the ear
    /// asks for what follows it.
    Play {
        item: MediaItem,
        play_id: String,
        offset_ms: u64,
    },
    /// The ear comes back to the player for `play_id` (a new link, a new queue, a start after speech): the player
    /// answers what to play then (the Station: a callback of ours it sends back).
    AskAgain {
        play_id: String,
    },
    StartBluetooth,
    StopBluetooth,
    /// A skill's dialog (Yandex's «Навыки») is over.
    EndDialog {
        dialog_id: String,
    },
    TimerSet(TimerSpec),
    /// The ringing timer stops.
    TimerStop {
        id: String,
    },
    TimerCancel {
        id: String,
    },
    TimerPause {
        id: String,
    },
    TimerResume {
        id: String,
    },
    AlarmAdd(AlarmSpec),
    AlarmCancel {
        id: String,
    },
    /// The ringing alarm stops.
    AlarmStop,
    /// «Отложи будильник»: the ringing stops and the same alarm comes again as a one-time one, under a new id.
    AlarmSnooze(AlarmSpec),
    /// «Не беспокоить» on or off.
    DoNotDisturb(bool),
    /// The temperature on the ear's display, as Yandex shows it with the weather; an ear that cannot draw it does
    /// nothing.
    ShowTemperature(f64),
    /// An animation of the ear's lights as it was captured from the ear itself (the LED skill's `.b64` files).
    LightAnimation(Vec<u8>),
}

impl Command {
    /// What a `Play` plays: the item, the player's id for it and where it starts.
    pub fn played(&self) -> Option<(&MediaItem, &str, u64)> {
        match self {
            Command::Play { item, play_id, offset_ms } => Some((item, play_id, *offset_ms)),
            _ => None,
        }
    }

    /// Its name, for the journal and the tests: `set_volume`, `play`, `timer_set`…
    pub fn name(&self) -> &'static str {
        match self {
            Command::SetVolume { .. } => "set_volume",
            Command::Louder => "louder",
            Command::Quieter => "quieter",
            Command::Mute => "mute",
            Command::Unmute => "unmute",
            Command::StopPlayer => "stop_player",
            Command::PausePlayer => "pause_player",
            Command::ContinuePlayer => "continue_player",
            Command::NextTrack => "next_track",
            Command::PreviousTrack => "previous_track",
            Command::Play { .. } => "play",
            Command::AskAgain { .. } => "ask_again",
            Command::StartBluetooth => "start_bluetooth",
            Command::StopBluetooth => "stop_bluetooth",
            Command::EndDialog { .. } => "end_dialog",
            Command::TimerSet(_) => "timer_set",
            Command::TimerStop { .. } => "timer_stop",
            Command::TimerCancel { .. } => "timer_cancel",
            Command::TimerPause { .. } => "timer_pause",
            Command::TimerResume { .. } => "timer_resume",
            Command::AlarmAdd(_) => "alarm_add",
            Command::AlarmCancel { .. } => "alarm_cancel",
            Command::AlarmStop => "alarm_stop",
            Command::AlarmSnooze(_) => "alarm_snooze",
            Command::DoNotDisturb(_) => "do_not_disturb",
            Command::ShowTemperature(_) => "show_temperature",
            Command::LightAnimation(_) => "light_animation",
        }
    }
}

/// An id in the shape Yandex gives timers: four groups of eight hex digits.
pub fn new_timer_id() -> String {
    (0..4).map(|_| format!("{:08x}", rand::random::<u32>())).collect::<Vec<_>>().join("-")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerSpec {
    pub id: String,
    /// Unix seconds.
    pub start: u64,
    pub seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarDate {
    pub year: u32,
    pub month: u32,
    pub day: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlarmSpec {
    pub id: String,
    pub hours: u32,
    pub minutes: u32,
    pub seconds: u32,
    pub timezone: String,
    pub offset_seconds: i32,
    /// A one-time alarm on this date; weekdays (1 Monday … 7 Sunday) for a repeating one (not yet seen from Yandex).
    pub date: Option<CalendarDate>,
    pub weekdays: Vec<i32>,
    /// Unix seconds; Yandex leaves it out of a new alarm and copies the original's into a snoozed one.
    pub edit_timestamp: Option<u64>,
}
