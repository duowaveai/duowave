use super::opus_pcm_decoder::SPEECH_SAMPLE_RATE;

/// Pitch from 70 to 400 Hz: the autocorrelation lags looked at.
const MIN_LAG: usize = (SPEECH_SAMPLE_RATE / 400) as usize;
const MAX_LAG: usize = (SPEECH_SAMPLE_RATE as usize).div_ceil(70);
/// A window this periodic has a pitch.
const PERIODIC: f64 = 0.7;
/// And is this far over the room's floor, so the hum of a fridge in a pause does not count.
const ABOVE_FLOOR_DB: f64 = 10.0;
/// One-pole low-pass at ~850 Hz: the pitch and the first harmonics, not the hiss a whisper is made of.
const LOW_PASS: f64 = 0.33;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Voicing {
    /// Windows with a pitch.
    pub windows: usize,
    /// The most of them in a row: a vowel lasts several, music behind a whisper gives one here and there.
    pub longest_run: usize,
}

/// Windows of voiced speech: periodic (a pitch of 70–400 Hz) and well above the floor of the audio given. A whisper
/// has none, the vocal folds do not sound; speech from across the room is as quiet but keeps its pitch. On the
/// recordings of 31.10–02.11, in windows of 32 ms: whispers at most 2 in a row (also under the radio), distant and
/// normal speech 4–7 in a row.
pub fn voicing(pcm: &[i16], window_samples: usize) -> Voicing {
    if window_samples == 0 {
        return Voicing::default();
    }
    let mut x = vec![0f64; window_samples];
    let windows: Vec<(f64, f64)> = pcm
        .chunks_exact(window_samples)
        .map(|window| {
            let mean = window.iter().map(|&s| f64::from(s)).sum::<f64>() / window_samples as f64;
            let mut energy = 0.0;
            for (i, &sample) in window.iter().enumerate() {
                x[i] = (f64::from(sample) - mean) / 32768.0;
                energy += x[i] * x[i];
            }
            for i in 1..window_samples {
                x[i] = x[i - 1] + LOW_PASS * (x[i] - x[i - 1]);
            }
            (10.0 * (energy / window_samples as f64 + 1e-12).log10(), periodicity(&x))
        })
        .collect();
    if windows.is_empty() {
        return Voicing::default();
    }
    let mut levels: Vec<f64> = windows.iter().map(|(db, _)| *db).collect();
    levels.sort_by(f64::total_cmp);
    let floor = levels[(windows.len() as f64 * 0.1).floor() as usize];
    let mut count = 0;
    let mut run = 0;
    let mut longest_run = 0;
    for (db, periodicity) in windows {
        let voiced = periodicity >= PERIODIC && db >= floor + ABOVE_FLOOR_DB;
        if voiced {
            count += 1;
        }
        run = if voiced { run + 1 } else { 0 };
        longest_run = longest_run.max(run);
    }
    Voicing { windows: count, longest_run }
}

/// The highest normalized autocorrelation over the pitch lags.
fn periodicity(x: &[f64]) -> f64 {
    let mut best: f64 = 0.0;
    for lag in (MIN_LAG..=MAX_LAG).take_while(|&lag| lag < x.len()) {
        let (mut xy, mut xx, mut yy) = (0.0, 0.0, 0.0);
        for i in 0..x.len() - lag {
            xy += x[i] * x[i + lag];
            xx += x[i] * x[i];
            yy += x[i + lag] * x[i + lag];
        }
        if xx > 0.0 && yy > 0.0 {
            best = best.max(xy / (xx * yy).sqrt());
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 16000.0;
    const WINDOW: usize = 512;

    /// Deterministic noise in [-1, 1], with JavaScript's floating-point arithmetic.
    fn noise(seed: f64) -> impl FnMut() -> f64 {
        let mut state = seed;
        move || {
            state = (state * 1_103_515_245.0 + 12345.0) % 2_147_483_648.0;
            state / 1_073_741_824.0 - 1.0
        }
    }

    #[derive(Clone, Copy)]
    enum Kind {
        Voiced,
        Whisper,
        Silence,
    }

    /// Quiet room noise with a stretch of `kind` in the middle, at -50 dBFS or so.
    fn recording(kind: Kind) -> Vec<i16> {
        let mut room = noise(1.0);
        let mut hiss = noise(2.0);
        (0..(RATE as usize) * 2)
            .map(|i| {
                let t = i as f64 / RATE;
                let mut sample = room() * 3.0;
                if (0.6..1.2).contains(&t) {
                    match kind {
                        // A pitch of 140 Hz with harmonics, like a vowel.
                        Kind::Voiced => {
                            sample += 80.0 * (2.0 * std::f64::consts::PI * 140.0 * t).sin()
                                + 40.0 * (2.0 * std::f64::consts::PI * 280.0 * t).sin()
                                + 20.0 * (2.0 * std::f64::consts::PI * 420.0 * t).sin();
                        }
                        Kind::Whisper => sample += 120.0 * hiss(),
                        Kind::Silence => {}
                    }
                }
                crate::speech::audio::pcm::js_round(sample) as i16
            })
            .collect()
    }

    #[test]
    fn finds_the_pitch_of_quiet_voiced_speech_window_after_window() {
        let found = voicing(&recording(Kind::Voiced), WINDOW);
        assert!(found.windows >= 10, "{found:?}");
        assert!(found.longest_run >= 10, "{found:?}");
    }

    #[test]
    fn finds_none_in_a_whisper_or_in_the_room_alone() {
        assert_eq!(voicing(&recording(Kind::Whisper), WINDOW), Voicing { windows: 0, longest_run: 0 });
        assert_eq!(voicing(&recording(Kind::Silence), WINDOW), Voicing { windows: 0, longest_run: 0 });
    }

    #[test]
    fn takes_no_audio_for_no_voice() {
        assert_eq!(voicing(&[0i16; 100], WINDOW), Voicing { windows: 0, longest_run: 0 });
    }
}
