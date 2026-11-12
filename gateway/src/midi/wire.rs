//! MIDI as the gateway passes it (docs/plans/midi.md): channel messages only — a note, its release, a control (the
//! sustain pedal, a knob), a change of sound, the pitch wheel, pressure. SysEx goes neither way: it changes a
//! keyboard's settings and even its firmware. Clock and active sensing are dropped too: a keyboard sends them many
//! times a second, and they are no one's business here.
//!
//! The channel of notes carries them as short JSON frames: `{"t":"on","c":1,"n":60,"v":96}` — `c` the MIDI channel
//! 1–16, `n` the note 0–127, `v` its force or a value.

use serde_json::{json, Map, Value};

/// A channel message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    On {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    Off {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    /// Pressure on one key (polyphonic aftertouch).
    Touch {
        channel: u8,
        note: u8,
        value: u8,
    },
    /// A control: 64 — the sustain pedal, 123 — all notes off, knobs and sliders.
    Control {
        channel: u8,
        control: u8,
        value: u8,
    },
    Program {
        channel: u8,
        program: u8,
    },
    /// Pressure on the whole channel.
    Pressure {
        channel: u8,
        value: u8,
    },
    /// The pitch wheel, −8192…8191, 0 in the middle.
    Bend {
        channel: u8,
        value: i16,
    },
}

impl Message {
    /// One whole channel message, as a keyboard sends it; anything else — SysEx, clock, a lone data byte — is none.
    /// A note with force 0 is its release, as keyboards often send it.
    pub fn parse(bytes: &[u8]) -> Option<Message> {
        let (&status, data) = bytes.split_first()?;
        if !(0x80..0xF0).contains(&status) || data.iter().any(|byte| *byte >= 0x80) {
            return None;
        }
        let channel = (status & 0x0F) + 1;
        let one = |i: usize| data.get(i).copied();
        Some(match status & 0xF0 {
            0x80 => Message::Off { channel, note: one(0)?, velocity: one(1)? },
            0x90 => match (one(0)?, one(1)?) {
                (note, 0) => Message::Off { channel, note, velocity: 0 },
                (note, velocity) => Message::On { channel, note, velocity },
            },
            0xA0 => Message::Touch { channel, note: one(0)?, value: one(1)? },
            0xB0 => Message::Control { channel, control: one(0)?, value: one(1)? },
            0xC0 => Message::Program { channel, program: one(0)? },
            0xD0 => Message::Pressure { channel, value: one(0)? },
            _ => {
                let raw = (one(0)? as i16) | ((one(1)? as i16) << 7);
                Message::Bend { channel, value: raw - 8192 }
            }
        })
    }

    pub fn channel(&self) -> u8 {
        match *self {
            Message::On { channel, .. }
            | Message::Off { channel, .. }
            | Message::Touch { channel, .. }
            | Message::Control { channel, .. }
            | Message::Program { channel, .. }
            | Message::Pressure { channel, .. }
            | Message::Bend { channel, .. } => channel,
        }
    }

    /// The bytes a keyboard takes.
    pub fn bytes(&self) -> Vec<u8> {
        let status = |kind: u8, channel: u8| kind | (channel.clamp(1, 16) - 1);
        match *self {
            Message::On { channel, note, velocity } => vec![status(0x90, channel), note, velocity],
            Message::Off { channel, note, velocity } => vec![status(0x80, channel), note, velocity],
            Message::Touch { channel, note, value } => vec![status(0xA0, channel), note, value],
            Message::Control { channel, control, value } => vec![status(0xB0, channel), control, value],
            Message::Program { channel, program } => vec![status(0xC0, channel), program],
            Message::Pressure { channel, value } => vec![status(0xD0, channel), value],
            Message::Bend { channel, value } => {
                let raw = (value.clamp(-8192, 8191) + 8192) as u16;
                vec![status(0xE0, channel), (raw & 0x7F) as u8, (raw >> 7) as u8]
            }
        }
    }

    /// The frame's fields of this message, without the device, the time and the number.
    pub fn fields(&self) -> Map<String, Value> {
        let value = match *self {
            Message::On { channel, note, velocity } => json!({ "t": "on", "c": channel, "n": note, "v": velocity }),
            Message::Off { channel, note, velocity } => json!({ "t": "off", "c": channel, "n": note, "v": velocity }),
            Message::Touch { channel, note, value } => json!({ "t": "touch", "c": channel, "n": note, "v": value }),
            Message::Control { channel, control, value } => {
                json!({ "t": "cc", "c": channel, "k": control, "v": value })
            }
            Message::Program { channel, program } => json!({ "t": "program", "c": channel, "p": program }),
            Message::Pressure { channel, value } => json!({ "t": "pressure", "c": channel, "v": value }),
            Message::Bend { channel, value } => json!({ "t": "bend", "c": channel, "v": value }),
        };
        match value {
            Value::Object(map) => map,
            _ => Map::new(),
        }
    }

    /// A message from a frame of the panel; none when the frame is not one (`listen`, `play`…). A value out of its
    /// range is an error, never clamped into some other note.
    pub fn from_frame(frame: &Value) -> Option<Result<Message, String>> {
        let kind = frame.get("t").and_then(Value::as_str)?;
        if !matches!(kind, "on" | "off" | "touch" | "cc" | "program" | "pressure" | "bend") {
            return None;
        }
        Some(Self::read(kind, frame))
    }

    fn read(kind: &str, frame: &Value) -> Result<Message, String> {
        let field = |key: &str, max: i64| -> Result<i64, String> {
            frame
                .get(key)
                .and_then(Value::as_i64)
                .filter(|value| (0..=max).contains(value))
                .ok_or_else(|| format!("{kind}: «{key}» — целое от 0 до {max}"))
        };
        let channel = match frame.get("c") {
            None => 1,
            Some(_) => frame
                .get("c")
                .and_then(Value::as_i64)
                .filter(|channel| (1..=16).contains(channel))
                .ok_or_else(|| format!("{kind}: канал «c» — от 1 до 16"))? as u8,
        };
        let byte = |key: &str| field(key, 127).map(|value| value as u8);
        Ok(match kind {
            "on" => match byte("v").unwrap_or(100) {
                0 => Message::Off { channel, note: byte("n")?, velocity: 0 },
                velocity => Message::On { channel, note: byte("n")?, velocity },
            },
            "off" => Message::Off { channel, note: byte("n")?, velocity: byte("v").unwrap_or(0) },
            "touch" => Message::Touch { channel, note: byte("n")?, value: byte("v")? },
            "cc" => Message::Control { channel, control: byte("k")?, value: byte("v")? },
            "program" => Message::Program { channel, program: byte("p")? },
            "pressure" => Message::Pressure { channel, value: byte("v")? },
            _ => {
                let value = frame
                    .get("v")
                    .and_then(Value::as_i64)
                    .filter(|value| (-8192..=8191).contains(value))
                    .ok_or_else(|| "bend: «v» — от −8192 до 8191".to_string())?;
                Message::Bend { channel, value: value as i16 }
            }
        })
    }
}

/// A note of a melody for the keyboard (`play`): the note, its force, when from the start and how long, in ms; the
/// channel, 1 when not said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub note: u8,
    pub velocity: u8,
    pub at_ms: u64,
    pub length_ms: u64,
    pub channel: u8,
}

/// A melody at most this long, in notes and in time (docs/plans/midi.md, «Лимиты»).
pub const MELODY_NOTES: usize = 2000;
pub const MELODY_MS: u64 = 10 * 60 * 1000;

/// `[[нота, сила, начало мс, длина мс, канал?], …]`, checked whole: sorted by the start, within the limits.
pub fn melody(value: &Value) -> Result<Vec<Step>, String> {
    let notes = value.as_array().ok_or_else(|| "notes: список [нота, сила, начало мс, длина мс]".to_string())?;
    if notes.len() > MELODY_NOTES {
        return Err(format!("мелодия длиннее {MELODY_NOTES} нот"));
    }
    let mut steps = Vec::with_capacity(notes.len());
    for (i, note) in notes.iter().enumerate() {
        let parts = note.as_array().filter(|parts| (4..=5).contains(&parts.len()));
        let number = |k: usize| parts.and_then(|parts| parts.get(k)).and_then(Value::as_u64);
        let wrong = || format!("нота {}: [нота 0–127, сила 1–127, начало мс, длина мс, канал 1–16?]", i + 1);
        let (Some(note), Some(velocity), Some(at_ms), Some(length_ms)) = (number(0), number(1), number(2), number(3))
        else {
            return Err(wrong());
        };
        let channel = match parts.and_then(|parts| parts.get(4)) {
            None => 1,
            Some(_) => number(4).filter(|channel| (1..=16).contains(channel)).ok_or_else(wrong)?,
        };
        if note > 127 || !(1..=127).contains(&velocity) {
            return Err(wrong());
        }
        if at_ms + length_ms > MELODY_MS {
            return Err("мелодия длиннее 10 минут".into());
        }
        steps.push(Step { note: note as u8, velocity: velocity as u8, at_ms, length_ms, channel: channel as u8 });
    }
    steps.sort_by_key(|step| step.at_ms);
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_whole_channel_messages_pass() {
        assert_eq!(Message::parse(&[0x90, 60, 96]), Some(Message::On { channel: 1, note: 60, velocity: 96 }));
        assert_eq!(Message::parse(&[0x9F, 60, 0]), Some(Message::Off { channel: 16, note: 60, velocity: 0 }));
        assert_eq!(Message::parse(&[0xB0, 64, 127]), Some(Message::Control { channel: 1, control: 64, value: 127 }));
        assert_eq!(Message::parse(&[0xE0, 0, 64]), Some(Message::Bend { channel: 1, value: 0 }));
        assert_eq!(Message::parse(&[0xE0, 0x7F, 0x7F]), Some(Message::Bend { channel: 1, value: 8191 }));
        assert_eq!(Message::parse(&[0xC2, 5]), Some(Message::Program { channel: 3, program: 5 }));
        // SysEx, clock, active sensing, a lone data byte, a cut message, a data byte with its high bit.
        for bytes in [&[0xF0, 0x7E, 0x7F, 0xF7][..], &[0xF8], &[0xFE], &[60, 96], &[0x90, 60], &[0x90, 0x80, 1], &[]] {
            assert_eq!(Message::parse(bytes), None, "{bytes:?}");
        }
    }

    #[test]
    fn bytes_and_frames_go_both_ways() {
        let messages = [
            Message::On { channel: 2, note: 61, velocity: 90 },
            Message::Off { channel: 2, note: 61, velocity: 0 },
            Message::Touch { channel: 1, note: 60, value: 3 },
            Message::Control { channel: 1, control: 64, value: 0 },
            Message::Program { channel: 10, program: 0 },
            Message::Pressure { channel: 1, value: 7 },
            Message::Bend { channel: 1, value: -8192 },
        ];
        for message in messages {
            assert_eq!(Message::parse(&message.bytes()), Some(message));
            let frame = Value::Object(message.fields());
            assert_eq!(Message::from_frame(&frame), Some(Ok(message)));
        }
    }

    #[test]
    fn a_frame_out_of_range_is_refused_not_clamped() {
        assert!(Message::from_frame(&json!({ "t": "on", "n": 128 })).unwrap().is_err());
        assert!(Message::from_frame(&json!({ "t": "on", "n": 60, "c": 17 })).unwrap().is_err());
        assert!(Message::from_frame(&json!({ "t": "bend", "v": 9000 })).unwrap().is_err());
        assert!(Message::from_frame(&json!({ "t": "sysex", "data": [240] })).is_none());
        assert_eq!(
            Message::from_frame(&json!({ "t": "on", "n": 60 })),
            Some(Ok(Message::On { channel: 1, note: 60, velocity: 100 }))
        );
    }

    #[test]
    fn a_melody_is_checked_whole() {
        let steps = melody(&json!([[62, 90, 500, 400], [60, 90, 0, 400, 2]])).unwrap();
        assert_eq!(steps[0], Step { note: 60, velocity: 90, at_ms: 0, length_ms: 400, channel: 2 });
        assert_eq!(steps[1].note, 62);
        assert!(melody(&json!([[60, 0, 0, 400]])).is_err(), "force 0");
        assert!(melody(&json!([[60, 90, MELODY_MS, 1]])).is_err(), "too long");
        assert!(melody(&json!([[60, 90]])).is_err());
        let many: Vec<Value> = (0..=MELODY_NOTES).map(|i| json!([60, 90, i, 10])).collect();
        assert!(melody(&Value::Array(many)).is_err());
    }
}
