//! What an ear says of itself in a request (the Station's device state): its volume, what its player plays, its timers
//! and alarms. The skills read it to decide; the Station's adapter reads it off the wire (src/gateway/protocol).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayerState {
    Idle,
    Playing,
    Stopped,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlarmDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

/// An alarm the station holds, as its alarm capability lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StationAlarm {
    pub id: String,
    pub hours: u32,
    pub minutes: u32,
    pub seconds: u32,
    /// A one-time alarm's day; repeating alarms have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<AlarmDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_seconds: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_timestamp: Option<i64>,
    /// It rings now.
    pub ringing: bool,
    /// A one-time alarm that has rung: the station keeps listing it.
    pub rung: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerView {
    pub state: PlayerState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_ms: Option<i64>,
}

/// A timer as the station's alarm capability lists it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StationTimer {
    pub id: String,
    /// Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<i64>,
    /// Seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<i64>,
    /// Seconds left when the state was taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<i64>,
    #[serde(default)]
    pub paused: bool,
}

/// What the station reports about itself in a request: volume, its player, the timers and alarms it holds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound_level: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound_muted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound_max_level: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player: Option<PlayerView>,
    /// Timers the alarm capability lists: running, paused or ringing (the state does not say which rings).
    #[serde(default)]
    pub timer_ids: Vec<String>,
    /// The same timers with what the station says of each: when it started, how long, what is left, paused —
    /// for the panel's widget (docs/plans/desk.md, stage 2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timers: Vec<StationTimer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alarms: Option<Vec<StationAlarm>>,
}
