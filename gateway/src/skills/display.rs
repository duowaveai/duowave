//! The station's display: 37 lights driven by draw_animation_directive with frames inline (station-protocol.proto,
//! DrawAnimationPayload). Four seven-segment digits, left to right, take lights 8·digit + 1 … 8·digit + 7 for
//! segments a–g (top, top right, bottom right, bottom, bottom left, top left, middle); 34 and 35 are the colon.
//! So Yandex drew «07:08» and «07:19» for its alarms and «3» for the weather (relay journal, 02.11).

use std::io::Write as _;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use bytes::Bytes;
use flate2::write::GzEncoder;
use flate2::Compression;
use prost::Message;

use super::js;
use crate::gateway::protocol::pb;
use crate::gateway::protocol::protocol_types::{Directive, DirectiveType};

pub const LIGHTS: usize = 37;
/// Lit with the temperature in Yandex's weather, right of the number; it pulses after its alarm time too.
const MARK: usize = 33;
/// The mark's column: right of every digit, so it comes on first.
const MARK_COLUMN: u32 = u32::MAX;
/// Inline frames: gzip, then base64.
const GZIP_BASE64: u32 = 2;

fn segments(char: char) -> Option<&'static str> {
    Some(match char {
        '0' => "abcdef",
        '1' => "bc",
        '2' => "abdeg",
        '3' => "abcdg",
        '4' => "bcfg",
        '5' => "acdfg",
        '6' => "acdefg",
        '7' => "abc",
        '8' => "abcdefg",
        '9' => "abcdfg",
        '-' => "g",
        _ => return None,
    })
}

/// A light and its column, left to right: a digit's left segments, its horizontal ones, its right ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lit {
    pub light: usize,
    pub column: u32,
}

/// The lights of a short text (digits and a minus) right-aligned on the four digits; None when it does not fit.
pub fn lights_of(text: &str) -> Option<Vec<Lit>> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() > 4 || chars.iter().any(|char| segments(*char).is_none()) {
        return None;
    }
    let mut lit = Vec::new();
    for (index, char) in chars.iter().enumerate() {
        let digit = 4 - chars.len() + index;
        for segment in segments(*char).unwrap_or_default().chars() {
            let offset = "abcdefg".find(segment).unwrap_or(0);
            let column = match segment {
                'e' | 'f' => 0,
                'a' | 'd' | 'g' => 1,
                _ => 2,
            };
            lit.push(Lit { light: 8 * digit + 1 + offset, column: 3 * digit as u32 + column });
        }
    }
    Some(lit)
}

const FRAME_MS: u16 = 20;
/// Steps from dark to full: 9 frames of 20 ms.
const RAMP: i64 = 9;
/// Columns light up right to left, one every 4 frames.
const STAGGER: i64 = 4;
const HOLD_MS: u16 = 3340;

/// Frames as Yandex's weather has them: a dark frame; the lights come on column by column from the right, each over
/// 180 ms; all of them held 3.3 s; then out in the reverse order.
fn frames(lit: &[Lit]) -> Vec<u8> {
    let mut columns: Vec<u32> = Vec::new();
    for entry in lit {
        if !columns.contains(&entry.column) {
            columns.push(entry.column);
        }
    }
    columns.sort_by(|a, b| b.cmp(a));
    let start = |column: u32| 1 + STAGGER * columns.iter().position(|c| *c == column).unwrap_or(0) as i64;
    let full = start(*columns.last().unwrap_or(&0)) + RAMP - 1;
    let frame = |index: i64, duration_ms: u16| {
        let mut bytes = vec![0u8; LIGHTS + 2];
        for entry in lit {
            let step = index - start(entry.column) + 1;
            if step > 0 {
                bytes[entry.light] = js::round(255.0 * step as f64 / RAMP as f64).min(255.0) as u8;
            }
        }
        bytes[LIGHTS..].copy_from_slice(&duration_ms.to_be_bytes());
        bytes
    };
    let rising: Vec<Vec<u8>> = (1..full).map(|index| frame(index, FRAME_MS)).collect();
    let mut all = frame(0, FRAME_MS);
    for bytes in &rising {
        all.extend_from_slice(bytes);
    }
    all.extend(frame(full, HOLD_MS));
    for bytes in rising.iter().rev() {
        all.extend_from_slice(bytes);
    }
    all
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    // Writing to memory does not fail.
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_default()
}

fn draw_animation(data: &[u8]) -> Directive {
    let name = "draw_animation_directive";
    let payload = pb::DrawAnimationPayload {
        animations: vec![pb::Animation {
            frames: Some(pb::InlineAnimation { format: GZIP_BASE64, data: STANDARD.encode(gzip(data)) }),
            file: None,
        }],
        policy: 0,
        speaking: 1,
        name: name.into(),
    };
    Directive {
        r#type: DirectiveType::ClientAction,
        name: name.into(),
        analytics_type: Some(name.into()),
        payload: None,
        typed_payload: Some(Bytes::from(payload.encode_to_vec())),
        is_led_silent: Some(true),
        ignore_answer: None,
    }
}

/// The temperature on the display, as Yandex shows it with the weather: «3», «-12» and its mark.
pub fn show_temperature(celsius: f64) -> Option<Directive> {
    let rounded = js::round(celsius);
    let mut lit = lights_of(&js::number_text(rounded))?;
    lit.push(Lit { light: MARK, column: MARK_COLUMN });
    Some(draw_animation(&frames(&lit)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    /// How Yandex draws «+3» for its weather, as seen in a relay journal: the lights come on in three steps of
    /// 20 ms frames, hold 3.34 s and go out the same way. Runs of [lights on, ms per frame, frames].
    const YANDEX_PLUS_3: &[(&[usize], u16, usize)] = &[
        (&[], 20, 1),
        (&[33], 20, 4),
        (&[26, 27, 33], 20, 4),
        (&[25, 26, 27, 28, 31, 33], 20, 8),
        (&[25, 26, 27, 28, 31, 33], 3340, 1),
        (&[25, 26, 27, 28, 31, 33], 20, 8),
        (&[26, 27, 33], 20, 4),
        (&[33], 20, 4),
    ];

    fn expand(runs: &[(&[usize], u16, usize)]) -> Vec<(Vec<usize>, u16)> {
        runs.iter().flat_map(|(lit, ms, count)| std::iter::repeat_n((lit.to_vec(), *ms), *count)).collect()
    }

    /// Per frame: the lights on and the duration.
    fn decoded_frames(base64: &str) -> Vec<(Vec<usize>, u16)> {
        let mut data = Vec::new();
        flate2::read::GzDecoder::new(&STANDARD.decode(base64).unwrap()[..]).read_to_end(&mut data).unwrap();
        data.chunks(LIGHTS + 2)
            .map(|frame| {
                let lit = frame[..LIGHTS].iter().enumerate().filter(|(_, v)| **v != 0).map(|(i, _)| i).collect();
                (lit, u16::from_be_bytes([frame[LIGHTS], frame[LIGHTS + 1]]))
            })
            .collect()
    }

    fn sorted_lights(text: &str) -> Vec<usize> {
        let mut lights: Vec<usize> = lights_of(text).unwrap().iter().map(|entry| entry.light).collect();
        lights.sort();
        lights
    }

    #[test]
    fn lights_the_digits_as_yandex_drew_its_alarm_times_07_08_and_07_19() {
        assert_eq!(
            sorted_lights("0708"),
            [1, 2, 3, 4, 5, 6, 9, 10, 11, 17, 18, 19, 20, 21, 22, 25, 26, 27, 28, 29, 30, 31]
        );
        assert_eq!(sorted_lights("0719"), [1, 2, 3, 4, 5, 6, 9, 10, 11, 18, 19, 25, 26, 27, 28, 30, 31]);
        assert_eq!(sorted_lights("3"), [25, 26, 27, 28, 31]);
        assert_eq!(sorted_lights("-12"), [15, 18, 19, 25, 26, 28, 29, 31]);
        assert_eq!(lights_of("12345"), None);
    }

    #[test]
    fn shows_the_temperature_frame_by_frame_as_yandex_does() {
        let directive = show_temperature(3.4).unwrap();
        let payload = pb::DrawAnimationPayload::decode(directive.typed_payload.unwrap()).unwrap();
        assert_eq!(payload.speaking, 1);
        assert_eq!(payload.name, "draw_animation_directive");
        let frames = payload.animations[0].frames.as_ref().unwrap();
        assert_eq!(frames.format, 2);
        assert_eq!(decoded_frames(&frames.data), expand(YANDEX_PLUS_3));
    }

    #[test]
    fn shows_what_fits_and_nothing_else() {
        assert!(show_temperature(-0.3).is_some());
        assert!(show_temperature(-27.0).is_some());
        assert!(show_temperature(12345.0).is_none());
    }
}
