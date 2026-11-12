use super::pcm::{dbfs, from_db, js_round, levels};
use crate::config::{GainConfig, GainMode};

/// Peak level after gain, just under full scale.
const CEILING: f64 = 0.97;

/// Gain in front of VAD and recognition. After the station's VQE normal speech peaks around -40 dBFS and a whisper
/// is lower still, so both models need +30..+45 dB.
///
/// Auto mode picks the gain once per request from the wake word buffer: the wake word is the user's own voice at
/// the user's distance, so its loudest 32 ms window is brought to the target level. Whatever would clip gets less
/// gain (per VAD window, and once for the whole recognition audio).
#[derive(Debug, Clone)]
pub struct GainStage {
    config: GainConfig,
    gain_db: f64,
    reference_dbfs: Option<f64>,
    linear: f64,
}

impl GainStage {
    pub fn new(config: GainConfig) -> GainStage {
        let db = if config.mode == GainMode::Fixed { config.fixed_db } else { config.max_db };
        let mut stage = GainStage { config, gain_db: 0.0, reference_dbfs: None, linear: 1.0 };
        stage.set(db);
        stage
    }

    pub fn gain_db(&self) -> f64 {
        self.gain_db
    }

    /// Loudest window of the audio the auto gain was picked from, dBFS.
    pub fn reference_dbfs(&self) -> Option<f64> {
        self.reference_dbfs
    }

    pub fn calibrate(&mut self, samples: &[i16], window_samples: usize) {
        if self.config.mode != GainMode::Auto || samples.is_empty() {
            return;
        }
        let loudest = samples.chunks(window_samples.max(1)).map(|window| levels(window).rms).fold(0.0, f64::max);
        let reference = dbfs(loudest);
        self.reference_dbfs = Some(reference);
        self.set(self.config.max_db.min((self.config.target_dbfs - reference).max(0.0)));
    }

    /// A gain picked for an earlier request (auto mode): a request with no wake word to pick it from — the station
    /// listening on after a question — would keep the maximum, and room noise at +40 dB is louder than VAD's
    /// threshold, so its end of speech never came (live 12.11: every follow-up ran to VAD_MAX_SPEECH_MS).
    pub fn assume(&mut self, db: f64) {
        if self.config.mode != GainMode::Auto || !db.is_finite() {
            return;
        }
        self.set(db.clamp(0.0, self.config.max_db));
    }

    /// Linear gain for audio whose peak (0..1) is given: the request gain, lowered so the peak stays under the
    /// ceiling.
    pub fn limited(&self, peak: f64) -> f64 {
        self.linear.min(CEILING / peak.max(1e-9))
    }

    fn set(&mut self, db: f64) {
        self.gain_db = js_round(db * 10.0) / 10.0;
        self.linear = from_db(self.gain_db);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::test_audio::voice;

    fn auto() -> GainConfig {
        GainConfig { mode: GainMode::Auto, fixed_db: 0.0, max_db: 40.0, target_dbfs: -20.0, no_wake_word_db: 30.0 }
    }

    #[test]
    fn brings_the_loudest_window_of_the_wake_word_to_the_target_level() {
        let mut gain = GainStage::new(auto());
        gain.calibrate(&voice(800.0, -50.0), 512);
        // The loudest window of the wobbling tone is ~2.3 dB above its average level.
        assert!(gain.gain_db() > 26.5, "{}", gain.gain_db());
        assert!(gain.gain_db() < 29.0, "{}", gain.gain_db());
        assert!((gain.reference_dbfs().unwrap() - (-20.0 - gain.gain_db())).abs() < 0.5);
    }

    #[test]
    fn caps_the_gain_for_near_silence_and_never_attenuates_loud_speech() {
        let mut quiet = GainStage::new(auto());
        quiet.calibrate(&voice(800.0, -90.0), 512);
        assert_eq!(quiet.gain_db(), 40.0);
        let mut loud = GainStage::new(auto());
        loud.calibrate(&voice(800.0, -10.0), 512);
        assert_eq!(loud.gain_db(), 0.0);
    }

    #[test]
    fn keeps_peaks_under_full_scale_and_ignores_calibration_in_fixed_mode() {
        let mut fixed = GainStage::new(GainConfig { mode: GainMode::Fixed, fixed_db: 30.0, ..auto() });
        fixed.calibrate(&voice(800.0, -50.0), 512);
        assert_eq!(fixed.gain_db(), 30.0);
        assert!((fixed.limited(0.001) - 10f64.powf(30.0 / 20.0)).abs() < 0.0005);
        assert!(0.5 * fixed.limited(0.5) <= 0.97);
    }

    #[test]
    fn takes_an_earlier_gain_within_its_limits_in_auto_mode_only() {
        let mut gain = GainStage::new(auto());
        assert_eq!(gain.gain_db(), 40.0, "the maximum until anything is known");
        gain.assume(27.34);
        assert_eq!((gain.gain_db(), gain.reference_dbfs()), (27.3, None), "assumed, not measured");
        gain.assume(55.0);
        assert_eq!(gain.gain_db(), 40.0);
        gain.assume(f64::NAN);
        assert_eq!(gain.gain_db(), 40.0);
        let mut fixed = GainStage::new(GainConfig { mode: GainMode::Fixed, fixed_db: 30.0, ..auto() });
        fixed.assume(12.0);
        assert_eq!(fixed.gain_db(), 30.0);
    }
}
