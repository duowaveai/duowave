//! How long an audio file plays, from its headers: the station's player needs it in audio_play (duration_ms), as
//! Yandex gives it for its tracks. MP3 (a Xing/Info or VBRI header, else the first frame's bitrate), Ogg (Opus or
//! Vorbis: the last page's granule), WAV and FLAC; other formats, or what does not parse, give None.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use crate::skills::js;

const HEAD_BYTES: u64 = 64 * 1024;
const TAIL_BYTES: u64 = 64 * 1024;

type Key = (std::path::PathBuf, u64, Option<SystemTime>);

static KNOWN: LazyLock<Mutex<HashMap<Key, Option<u64>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Milliseconds, or None; kept per file until it changes.
pub fn audio_duration_ms(file: &Path) -> Option<u64> {
    let meta = std::fs::metadata(file).ok()?;
    let key: Key = (file.to_path_buf(), meta.len(), meta.modified().ok());
    if let Some(known) = KNOWN.lock().unwrap().get(&key) {
        return *known;
    }
    let duration = measure(file, meta.len());
    KNOWN.lock().unwrap().insert(key, duration);
    duration
}

fn read(file: &Path, position: u64, length: u64) -> Option<Vec<u8>> {
    let mut handle = File::open(file).ok()?;
    handle.seek(SeekFrom::Start(position)).ok()?;
    let mut buffer = Vec::with_capacity(length as usize);
    handle.take(length).read_to_end(&mut buffer).ok()?;
    Some(buffer)
}

fn measure(file: &Path, size: u64) -> Option<u64> {
    let head = read(file, 0, size.min(HEAD_BYTES))?;
    if head.starts_with(b"OggS") {
        let tail = read(file, size.saturating_sub(TAIL_BYTES), size.min(TAIL_BYTES))?;
        return ogg_ms(&head, &tail);
    }
    if head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WAVE") {
        return wav_ms(&head);
    }
    if head.starts_with(b"fLaC") {
        return flac_ms(&head);
    }
    mp3_ms(&head, size)
}

fn u16_le(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_le(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u32_be(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_le(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

fn ms(value: f64) -> Option<u64> {
    let rounded = js::round(value);
    (rounded.is_finite() && rounded >= 0.0).then_some(rounded as u64)
}

// ---------------------------------------------------------------- MP3

/// kbps by index: MPEG-1 and MPEG-2/2.5 Layer III.
const BITRATES_1: [u32; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
const BITRATES_2: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];

fn sample_rate(version: u8, index: u8) -> Option<u32> {
    let rates: [u32; 3] = match version {
        3 => [44100, 48000, 32000],
        2 => [22050, 24000, 16000],
        0 => [11025, 12000, 8000],
        _ => return None,
    };
    rates.get(index as usize).copied()
}

fn mp3_ms(head: &[u8], size: u64) -> Option<u64> {
    let mut start = 0usize;
    // An ID3v2 tag: its size is four 7-bit bytes.
    if head.starts_with(b"ID3") && head.len() >= 10 {
        let tag =
            (u32::from(head[6]) << 21) | (u32::from(head[7]) << 14) | (u32::from(head[8]) << 7) | u32::from(head[9]);
        start = 10 + tag as usize;
    }
    let mut at = start;
    while at + 4 <= head.len() {
        if head[at] != 0xff || head[at + 1] & 0xe0 != 0xe0 {
            at += 1;
            continue;
        }
        let version = (head[at + 1] >> 3) & 3; // 3 MPEG-1, 2 MPEG-2, 0 MPEG-2.5
        let layer = (head[at + 1] >> 1) & 3; // 1 Layer III
        let bitrate_index = head[at + 2] >> 4;
        let rate_index = (head[at + 2] >> 2) & 3;
        if version == 1 || layer != 1 || bitrate_index == 0 || bitrate_index == 15 || rate_index == 3 {
            at += 1;
            continue;
        }
        let rate = f64::from(sample_rate(version, rate_index)?);
        let samples_per_frame = if version == 3 { 1152.0 } else { 576.0 };
        let mono = head[at + 3] >> 6 == 3;
        // The Xing/Info header sits after the side information; VBRI 32 bytes after the frame header.
        let side = match (version == 3, mono) {
            (true, true) => 17,
            (true, false) => 32,
            (false, true) => 9,
            (false, false) => 17,
        };
        let xing = at + 4 + side;
        let tag = head.get(xing..xing + 4);
        if (tag == Some(b"Xing") || tag == Some(b"Info")) && head.get(xing + 7).is_some_and(|flags| flags & 1 == 1) {
            return ms(f64::from(u32_be(head, xing + 8)?) * samples_per_frame * 1000.0 / rate);
        }
        if head.get(at + 36..at + 40) == Some(b"VBRI") {
            return ms(f64::from(u32_be(head, at + 36 + 14)?) * samples_per_frame * 1000.0 / rate);
        }
        let kbps = if version == 3 { BITRATES_1 } else { BITRATES_2 }[bitrate_index as usize];
        return ms((size as f64 - at as f64) * 8.0 / f64::from(kbps));
    }
    None
}

// ---------------------------------------------------------------- Ogg

fn ogg_ms(head: &[u8], tail: &[u8]) -> Option<u64> {
    // The last page's header, searched back from where a whole header still fits.
    let from = tail.len().checked_sub(27)?;
    let last = (0..=from).rev().find(|at| tail.get(*at..*at + 4) == Some(b"OggS"))?;
    let granule = u64_le(tail, last + 6)?;
    // The first packet, after the first page's header and its segment table.
    let packet = 27 + usize::from(*head.get(26)?);
    if head.get(packet..packet + 8) == Some(b"OpusHead") {
        let pre_skip = u64::from(u16_le(head, packet + 10)?);
        return ms(granule.saturating_sub(pre_skip) as f64 / 48.0);
    }
    if head.get(packet + 1..packet + 7) == Some(b"vorbis") {
        let rate = u32_le(head, packet + 12)?;
        return if rate == 0 { None } else { ms(granule as f64 * 1000.0 / f64::from(rate)) };
    }
    None
}

// ---------------------------------------------------------------- WAV, FLAC

fn wav_ms(head: &[u8]) -> Option<u64> {
    let mut byte_rate: Option<u32> = None;
    let mut at = 12usize;
    while at + 8 <= head.len() {
        let id = &head[at..at + 4];
        let length = u32_le(head, at + 4)? as usize;
        if id == b"fmt " {
            byte_rate = Some(u32_le(head, at + 16)?);
        }
        if id == b"data" {
            return byte_rate.filter(|rate| *rate != 0).and_then(|rate| ms(length as f64 * 1000.0 / f64::from(rate)));
        }
        at += 8 + length + (length & 1);
    }
    None
}

fn flac_ms(head: &[u8]) -> Option<u64> {
    // STREAMINFO, the first metadata block: sample rate in 20 bits, total samples in 36.
    let info = 8;
    if head.len() < info + 18 {
        return None;
    }
    let rate =
        (u32::from(head[info + 10]) << 12) | (u32::from(head[info + 11]) << 4) | (u32::from(head[info + 12]) >> 4);
    let samples = f64::from(head[info + 13] & 0x0f) * 4_294_967_296.0 + f64::from(u32_be(head, info + 14)?);
    if rate == 0 {
        return None;
    }
    ms(samples * 1000.0 / f64::from(rate))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An Ogg page: its header (granule, segment table for one packet) and the packet.
    pub(crate) fn ogg_page(granule: u64, packet: &[u8]) -> Vec<u8> {
        let mut page = b"OggS".to_vec();
        page.push(0); // version
        page.push(0); // header type
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&[1, 0, 0, 0]); // serial
        page.extend_from_slice(&[0, 0, 0, 0]); // sequence
        page.extend_from_slice(&[0, 0, 0, 0]); // checksum: not read
        page.push(1);
        page.push(packet.len() as u8);
        page.extend_from_slice(packet);
        page
    }

    /// Three seconds of Ogg Opus as an encoder writes its pages: OpusHead (pre-skip 312), tags, the last granule.
    pub(crate) fn ogg_opus_three_seconds() -> Vec<u8> {
        let mut opus_head = b"OpusHead".to_vec();
        opus_head.extend_from_slice(&[1, 1]);
        opus_head.extend_from_slice(&312u16.to_le_bytes());
        opus_head.extend_from_slice(&16_000u32.to_le_bytes());
        opus_head.extend_from_slice(&[0, 0, 0]);
        let mut file = ogg_page(0, &opus_head);
        file.extend(ogg_page(0, b"OpusTags\0\0\0\0\0\0\0\0"));
        file.extend(ogg_page(48_000, &[0xf8; 40]));
        file.extend(ogg_page(48_000 * 3 + 312, &[0xf8; 40]));
        file
    }

    #[test]
    fn knows_how_long_our_files_play_from_their_headers() {
        let dir = tempfile::tempdir().unwrap();
        // CBR MP3: MPEG-1 Layer III, 128 kbps, 44,1 kHz; 100 frames of 417 bytes are 2,61 s.
        let mut frame = vec![0u8; 417];
        frame[..4].copy_from_slice(&0xfffb_9064u32.to_be_bytes());
        let mp3 = dir.path().join("Тест - Ровный.mp3");
        std::fs::write(&mp3, frame.repeat(100)).unwrap();
        // WAV: 16 kHz mono 16-bit, 2 s of silence.
        let mut wav = vec![0u8; 44 + 64_000];
        wav[0..4].copy_from_slice(b"RIFF");
        wav[4..8].copy_from_slice(&(36u32 + 64_000).to_le_bytes());
        wav[8..16].copy_from_slice(b"WAVEfmt ");
        wav[16..20].copy_from_slice(&16u32.to_le_bytes());
        wav[20..22].copy_from_slice(&1u16.to_le_bytes());
        wav[22..24].copy_from_slice(&1u16.to_le_bytes());
        wav[24..28].copy_from_slice(&16_000u32.to_le_bytes());
        wav[28..32].copy_from_slice(&32_000u32.to_le_bytes());
        wav[32..34].copy_from_slice(&2u16.to_le_bytes());
        wav[34..36].copy_from_slice(&16u16.to_le_bytes());
        wav[36..40].copy_from_slice(b"data");
        wav[40..44].copy_from_slice(&64_000u32.to_le_bytes());
        let silence = dir.path().join("Тест - Тишина.wav");
        std::fs::write(&silence, wav).unwrap();
        let ogg = dir.path().join("Тест - Ogg.ogg");
        std::fs::write(&ogg, ogg_opus_three_seconds()).unwrap();
        let id3 = dir.path().join("Кино - Звезда по имени Солнце.mp3");
        std::fs::write(&id3, "ID3").unwrap();

        assert_eq!(audio_duration_ms(&mp3), Some(js::round(100.0 * 417.0 * 8.0 / 128.0) as u64));
        assert_eq!(audio_duration_ms(&silence), Some(2000));
        assert!(audio_duration_ms(&ogg).is_some_and(|ms| ms.abs_diff(3000) < 50));
        assert_eq!(audio_duration_ms(&id3), None);
        assert_eq!(audio_duration_ms(&dir.path().join("missing.mp3")), None);
    }
}
