use anyhow::Result;

use super::vad_engine::{VadEngine, VadStream};
use crate::speech::audio::pcm::{dbfs, levels_f32};

/// Level-based VAD: a fallback where ONNX Runtime is not there (the station's build) and a deterministic engine for
/// tests. The window's RMS level (after gain) maps to a probability: 0.5 at the threshold, ~0.95 six dB above.
#[derive(Debug, Clone)]
pub struct EnergyVadEngine {
    threshold_dbfs: f64,
}

impl EnergyVadEngine {
    pub fn new(threshold_dbfs: f64) -> EnergyVadEngine {
        EnergyVadEngine { threshold_dbfs }
    }
}

impl VadEngine for EnergyVadEngine {
    fn name(&self) -> &'static str {
        "energy"
    }

    fn window_samples(&self) -> usize {
        512
    }

    fn create_stream(&self) -> Box<dyn VadStream> {
        Box::new(EnergyVadStream { threshold_dbfs: self.threshold_dbfs })
    }
}

struct EnergyVadStream {
    threshold_dbfs: f64,
}

impl VadStream for EnergyVadStream {
    fn probability(&mut self, window: &[f32]) -> Result<f64> {
        Ok(1.0 / (1.0 + (-(dbfs(levels_f32(window).rms) - self.threshold_dbfs) / 2.0).exp()))
    }
}
