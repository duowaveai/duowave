//! Station-like audio for tests: Ogg/Opus, 16 kHz mono, one 20 ms packet per page, and the stream restarted at
//! SpotterEnd with a new OpusHead under the same serial number, as the station does. Synthetic sounds: a voiced,
//! speech-like tone at a given level and deterministic room noise. For the unit tests here and the end-to-end tests
//! in `tests/`; the numbers are computed as the TS test support computed them.
//!
//! The pages and packets are small and well-formed by construction, so these helpers panic rather than return
//! errors: a failure here is a broken test, never input from outside.

use super::audio::opus::{Application, Encoder};
use super::audio::pcm::{js_round, js_to_i16};

pub use super::audio::ogg_page::{ogg_page, raw_ogg_page};

const RATE: u32 = 16000;
/// 20 ms.
const FRAME: usize = 320;

/// An OpusHead packet: mono, 16 kHz, no output gain, mapping family 0.
pub fn opus_head(pre_skip: u16) -> Vec<u8> {
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1);
    head.push(1);
    head.extend_from_slice(&pre_skip.to_le_bytes());
    head.extend_from_slice(&RATE.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes());
    head.push(0);
    head
}

fn opus_tags() -> Vec<u8> {
    let vendor = b"duowave-test";
    let mut tags = Vec::with_capacity(8 + 4 + vendor.len() + 4);
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes());
    tags
}

/// One chain: OpusHead page (BOS), OpusTags page, then a page per 20 ms Opus packet (a last partial frame is left
/// out).
pub fn ogg_opus_chain(samples: &[i16], serial: u32) -> Vec<u8> {
    let mut encoder = Encoder::new(RATE, 1, Application::Voip).expect("a 16 kHz mono encoder");
    let mut bytes = ogg_page(serial, 0, 0, 0x02, &[&opus_head(0)]).expect("a head page");
    bytes.extend(ogg_page(serial, 1, 0, 0, &[&opus_tags()]).expect("a tags page"));
    let mut granule = 0;
    for (index, frame) in samples.as_chunks::<FRAME>().0.iter().enumerate() {
        let packet = encoder.encode(frame, FRAME).expect("an Opus packet");
        // Granule positions count 48 kHz samples.
        granule += (FRAME * 3) as i64;
        bytes.extend(ogg_page(serial, index as u32 + 2, granule, 0, &[&packet]).expect("a packet page"));
    }
    bytes
}

/// The station's voice request audio: the wake word buffer, then the command as a restarted chain.
#[derive(Debug, Clone)]
pub struct StationAudio {
    pub bytes: Vec<u8>,
    /// Where the command's chain starts: the station sends SpotterEnd here.
    pub spotter_end_at: usize,
}

pub fn station_audio(wake_word_buffer: &[i16], command: &[i16]) -> StationAudio {
    station_audio_with_serial(wake_word_buffer, command, 0x5f0e_a7a6)
}

pub fn station_audio_with_serial(wake_word_buffer: &[i16], command: &[i16], serial: u32) -> StationAudio {
    let mut bytes = ogg_opus_chain(wake_word_buffer, serial);
    let spotter_end_at = bytes.len();
    bytes.extend(ogg_opus_chain(command, serial));
    StationAudio { bytes, spotter_end_at }
}

/// Splits Ogg bytes into pages, e.g. to send them at the pace of the audio as the station does. A cut page at the
/// end is left out.
pub fn ogg_pages(bytes: &[u8]) -> Vec<&[u8]> {
    let mut pages = Vec::new();
    let mut offset = 0;
    while offset + 27 <= bytes.len() {
        let segments = usize::from(bytes[offset + 26]);
        let Some(lacing) = bytes.get(offset + 27..offset + 27 + segments) else { break };
        let length = 27 + segments + lacing.iter().map(|&size| usize::from(size)).sum::<usize>();
        let Some(page) = bytes.get(offset..offset + length) else { break };
        pages.push(page);
        offset += length;
    }
    pages
}

/// A voiced, speech-like tone (160 Hz plus harmonics, syllable-rate wobble) at the given RMS level.
pub fn voice(ms: f64, rms_dbfs: f64) -> Vec<i16> {
    voice_at_pitch(ms, rms_dbfs, 160.0)
}

/// A voiced, speech-like tone at the given pitch and RMS level.
pub fn voice_at_pitch(ms: f64, rms_dbfs: f64, pitch: f64) -> Vec<i16> {
    use std::f64::consts::PI;
    let count = js_round(ms * f64::from(RATE) / 1000.0) as usize;
    // A Float32Array in the TS: each value rounded to f32.
    let raw: Vec<f32> = (0..count)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let wobble = 0.75 + 0.25 * (2.0 * PI * 4.0 * t).sin();
            (wobble
                * ((2.0 * PI * pitch * t).sin()
                    + 0.5 * (4.0 * PI * pitch * t).sin()
                    + 0.25 * (6.0 * PI * pitch * t).sin())) as f32
        })
        .collect();
    let rms = (raw.iter().map(|&value| f64::from(value) * f64::from(value)).sum::<f64>() / count as f64).sqrt();
    let scale = (10f64.powf(rms_dbfs / 20.0) / rms) * 32767.0;
    raw.iter().map(|&value| js_to_i16(js_round(f64::from(value) * scale))).collect()
}

/// Room noise at -80 dBFS RMS (deterministic).
pub fn silence(ms: f64) -> Vec<i16> {
    silence_at(ms, -80.0)
}

/// Room noise at the given RMS level (deterministic: the same samples as the TS test support gave).
pub fn silence_at(ms: f64, rms_dbfs: f64) -> Vec<i16> {
    let count = js_round(ms * f64::from(RATE) / 1000.0) as usize;
    let amplitude = 10f64.powf(rms_dbfs / 20.0) * 32767.0 * 3f64.sqrt();
    let mut seed = 12345f64;
    (0..count)
        .map(|_| {
            // `(seed * 1103515245 + 12345) & 0x7fffffff` in JavaScript: a double product, then ToInt32.
            let product = seed * 1_103_515_245.0 + 12345.0;
            seed = f64::from(((product as i64) as i32) & 0x7fff_ffff);
            js_to_i16(js_round(((seed / f64::from(0x7fff_ffff)) * 2.0 - 1.0) * amplitude))
        })
        .collect()
}

pub fn concat(parts: &[&[i16]]) -> Vec<i16> {
    parts.concat()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::audio::pcm::{dbfs, levels};

    #[test]
    fn makes_sounds_at_the_asked_level() {
        assert_eq!(voice(1000.0, -20.0).len(), 16000);
        assert!((dbfs(levels(&voice(500.0, -30.0)).rms) + 30.0).abs() < 0.1);
        assert!((dbfs(levels(&silence_at(1000.0, -50.0)).rms) + 50.0).abs() < 1.0);
        assert_eq!(silence(10.0), silence(10.0));
        let audio = station_audio(&voice(400.0, -50.0), &silence(200.0));
        let pages = ogg_pages(&audio.bytes);
        // Head, tags and 20 packets; head, tags and 10 packets.
        assert_eq!(pages.len(), 22 + 12);
        assert_eq!(pages[..22].iter().map(|page| page.len()).sum::<usize>(), audio.spotter_end_at);
    }
}
