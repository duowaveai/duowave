//! PCM helpers: levels, WAV in and out, resampling.

use std::ops::RangeInclusive;

use anyhow::{bail, Result};

/// Rates a WAV's header may name. Any other is a damaged or bogus header: at 1 Hz a phrase would be hours of audio.
const WAV_RATES: RangeInclusive<u32> = 1_000..=384_000;

/// The most samples `try_resample` makes: five minutes at 48 kHz, longer than any phrase.
pub const MAX_RESAMPLED: usize = 48_000 * 300;

/// JavaScript's Math.round: the nearest integer, halves toward +∞ (`f64::round` takes them away from zero). The
/// gateway's numbers (gains, levels, positions in ms) round as they did in Node.
pub fn js_round(x: f64) -> f64 {
    let rounded = x.round();
    if rounded - x == -0.5 {
        rounded + 1.0
    } else {
        rounded
    }
}

/// An integral number stored in an Int16Array, as JavaScript does it: modulo 2^16, NaN and infinities as 0.
pub(crate) fn js_to_i16(x: f64) -> i16 {
    if x.is_finite() {
        (x.trunc() as i64) as i16
    } else {
        0
    }
}

/// Level of an amplitude (1.0 = full scale) in dBFS, floored at -200.
pub fn dbfs(amplitude: f64) -> f64 {
    20.0 * amplitude.max(1e-10).log10()
}

pub fn from_db(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LevelStats {
    /// Largest absolute sample, 0..1.
    pub peak: f64,
    /// Root mean square, 0..1.
    pub rms: f64,
}

/// Levels of 16-bit samples.
pub fn levels(samples: &[i16]) -> LevelStats {
    levels_of(samples.iter().map(|&sample| f64::from(sample) / 32768.0), samples.len())
}

/// Levels of float samples in -1..1.
pub fn levels_f32(samples: &[f32]) -> LevelStats {
    levels_of(samples.iter().map(|&sample| f64::from(sample)), samples.len())
}

fn levels_of(values: impl Iterator<Item = f64>, count: usize) -> LevelStats {
    let mut peak = 0.0;
    let mut energy = 0.0;
    for value in values {
        let magnitude = value.abs();
        if magnitude > peak {
            peak = magnitude;
        }
        energy += value * value;
    }
    LevelStats { peak, rms: if count > 0 { (energy / count as f64).sqrt() } else { 0.0 } }
}

pub fn to_int16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&sample| {
            let value = f64::from(sample).clamp(-1.0, 1.0);
            js_to_i16(if value < 0.0 { js_round(value * 32768.0) } else { js_round(value * 32767.0) })
        })
        .collect()
}

/// 16-bit PCM mono WAV, the format every recognition API accepts.
pub fn encode_wav(pcm: &[i16], sample_rate: u32) -> Vec<u8> {
    let data = (pcm.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + pcm.len() * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data.to_le_bytes());
    for sample in pcm {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

/// A decoded WAV: mono samples and their rate.
#[derive(Debug, Clone, PartialEq)]
pub struct Wav {
    pub pcm: Vec<i16>,
    pub sample_rate: u32,
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    bytes.get(offset..offset + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes.get(offset..offset + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Samples and rate of a 16-bit PCM WAV (mono, or stereo mixed down).
pub fn decode_wav(wav: &[u8]) -> Result<Wav> {
    if wav.get(0..4) != Some(b"RIFF") || wav.get(8..12) != Some(b"WAVE") {
        bail!("not a WAV file");
    }
    let mut offset = 12;
    let mut channels = 1usize;
    let mut sample_rate = 16000u32;
    let mut bits = 16u16;
    while offset + 8 <= wav.len() {
        let id = &wav[offset..offset + 4];
        let Some(size) = u32_at(wav, offset + 4) else { break };
        if id == b"fmt " {
            let (Some(count), Some(rate), Some(depth)) =
                (u16_at(wav, offset + 10), u32_at(wav, offset + 12), u16_at(wav, offset + 22))
            else {
                bail!("WAV with a cut fmt chunk");
            };
            channels = usize::from(count);
            sample_rate = rate;
            bits = depth;
        }
        if id == b"data" {
            if bits != 16 {
                bail!("WAV with {bits}-bit samples: only 16-bit is read");
            }
            if channels == 0 || !WAV_RATES.contains(&sample_rate) {
                bail!("WAV with {channels} channel(s) at {sample_rate} Hz");
            }
            // Streamed WAVs may say 0 or 0xffffffff here: the rest of the file is the data.
            let start = offset + 8;
            let end = if size == 0 || size == u32::MAX {
                wav.len()
            } else {
                wav.len().min(start.saturating_add(size as usize))
            };
            let bytes = &wav[start..end - (end - start) % 2];
            let samples: Vec<i16> = bytes.as_chunks::<2>().0.iter().map(|pair| i16::from_le_bytes(*pair)).collect();
            if channels == 1 {
                return Ok(Wav { pcm: samples, sample_rate });
            }
            let mono = (0..samples.len() / channels)
                .map(|i| {
                    let sum: f64 = (0..channels).map(|c| f64::from(samples[i * channels + c])).sum();
                    js_to_i16(js_round(sum / channels as f64))
                })
                .collect();
            return Ok(Wav { pcm: mono, sample_rate });
        }
        offset = offset.saturating_add(8 + size as usize + (size % 2) as usize);
    }
    bail!("WAV without data");
}

/// floor(len · to / from): the length resampling gives, computed in doubles as the TS did.
fn resampled_len(len: usize, from: u32, to: u32) -> f64 {
    ((len as f64 * f64::from(to)) / f64::from(from)).floor()
}

/// `resample` for a rate a file named: past MAX_RESAMPLED samples it fails instead of asking the allocator. A bogus
/// rate (a few Hz) asks for gigabytes, and a refused allocation aborts the whole gateway; Node's Int16Array threw a
/// RangeError, and only that phrase failed.
pub fn try_resample(pcm: &[i16], from: u32, to: u32) -> Result<Vec<i16>> {
    if from != to && from != 0 {
        let count = resampled_len(pcm.len(), from, to);
        if count > MAX_RESAMPLED as f64 {
            bail!(
                "resampling {} samples from {from} Hz to {to} Hz makes {count}: more than {MAX_RESAMPLED}",
                pcm.len()
            );
        }
    }
    Ok(resample(pcm, from, to))
}

/// Linear resampling: enough for speech going to Opus at 16 kHz. For rates the caller has checked; a rate a file
/// named goes through `try_resample`.
pub fn resample(pcm: &[i16], from: u32, to: u32) -> Vec<i16> {
    if from == to {
        return pcm.to_vec();
    }
    if from == 0 || pcm.is_empty() {
        return Vec::new();
    }
    let count = resampled_len(pcm.len(), from, to) as usize;
    (0..count)
        .map(|i| {
            let position = (i as f64 * f64::from(from)) / f64::from(to);
            let index = position.floor();
            let fraction = position - index;
            let index = (index as usize).min(pcm.len() - 1);
            let next = pcm[(index + 1).min(pcm.len() - 1)];
            js_to_i16(js_round(f64::from(pcm[index]) * (1.0 - fraction) + f64::from(next) * fraction))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_halves_up_as_javascript_does() {
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(-2.6), -3.0);
        assert_eq!(js_round(0.49999999999999994), 0.0);
        assert_eq!(js_to_i16(40000.0), -25536);
    }

    #[test]
    fn a_wav_comes_back_as_it_went() {
        let pcm: Vec<i16> = (0..100).map(|i| (i * 300 - 15000) as i16).collect();
        let wav = encode_wav(&pcm, 22050);
        assert_eq!(wav.len(), 44 + 200);
        assert_eq!(decode_wav(&wav).unwrap(), Wav { pcm: pcm.clone(), sample_rate: 22050 });
        assert!(decode_wav(b"RIFF").is_err());
        assert_eq!(resample(&pcm, 16000, 16000), pcm);
        assert_eq!(resample(&pcm, 16000, 48000).len(), 300);
        assert_eq!(resample(&pcm, 48000, 16000).len(), 33);
    }

    #[test]
    fn mixes_stereo_down_and_reads_a_streamed_size() {
        let mut wav = encode_wav(&[100, 300, -100, -301], 8000);
        wav[22] = 2; // channels
        wav[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode_wav(&wav).unwrap(), Wav { pcm: vec![200, -200], sample_rate: 8000 });
    }

    /// The voice lab quotes these when it refuses a take («Запись не принята: нужен WAV 16 бит (…)»).
    #[test]
    fn says_why_a_wav_is_not_read() {
        assert_eq!(decode_wav(b"ID3 not a wav").unwrap_err().to_string(), "not a WAV file");
        let mut eight = encode_wav(&[1, 2], 16_000);
        eight[34] = 8; // bits per sample
        assert_eq!(decode_wav(&eight).unwrap_err().to_string(), "WAV with 8-bit samples: only 16-bit is read");
        let mut stereo = encode_wav(&[100, 300, -5, -6], 48_000);
        stereo[22] = 2;
        assert_eq!(decode_wav(&stereo).unwrap().pcm, vec![200, -5], "halves round up, as Math.round");
    }

    #[test]
    fn resamples_linearly_holding_the_last_sample() {
        assert_eq!(resample(&[0, 300], 16_000, 32_000), vec![0, 150, 300, 300]);
        assert_eq!(resample(&vec![0; 16_000], 16_000, 24_000).len(), 24_000);
    }

    /// A WAV whose fmt chunk names `rate`, its samples left as they are.
    fn wav_saying(rate: u32) -> Vec<u8> {
        let mut wav = encode_wav(&[1, 2, 3, 4], 22_050);
        wav[24..28].copy_from_slice(&rate.to_le_bytes());
        wav
    }

    #[test]
    fn refuses_a_wav_whose_header_names_a_rate_no_audio_has() {
        for rate in [1, 999, 384_001, u32::MAX] {
            assert_eq!(
                decode_wav(&wav_saying(rate)).unwrap_err().to_string(),
                format!("WAV with 1 channel(s) at {rate} Hz")
            );
        }
        for rate in [1_000, 8_000, 384_000] {
            assert_eq!(decode_wav(&wav_saying(rate)).unwrap().sample_rate, rate);
        }
    }

    #[test]
    fn refuses_to_resample_into_more_than_five_minutes_at_48_khz() {
        // 10 s at 22050 Hz said to be 1 Hz: 21 GB at 48 kHz, refused before anything is allocated.
        let error = try_resample(&vec![0; 220_500], 1, 48_000).unwrap_err();
        assert_eq!(
            error.to_string(),
            "resampling 220500 samples from 1 Hz to 48000 Hz makes 10584000000: more than 14400000"
        );
        // A plausible rate, but a minute past five at 48 kHz.
        assert!(try_resample(&vec![0; 360_000], 1_000, 48_000).is_err());
        assert_eq!(try_resample(&[0, 300], 16_000, 32_000).unwrap(), vec![0, 150, 300, 300]);
        assert_eq!(try_resample(&vec![0; 22_050], 22_050, 48_000).unwrap().len(), 48_000);
    }
}
