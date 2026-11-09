use std::sync::Arc;

use anyhow::Result;

use super::energy_vad::EnergyVadEngine;
use crate::config::{VadConfig, VadEngine as VadKind};

/// Per-request VAD state. Windows must be fed in order; each is `window_samples` floats in -1..1 at 16 kHz.
pub trait VadStream: Send {
    fn probability(&mut self, window: &[f32]) -> Result<f64>;
}

pub trait VadEngine: Send + Sync {
    fn name(&self) -> &'static str;
    fn window_samples(&self) -> usize;
    fn create_stream(&self) -> Box<dyn VadStream>;
}

/// Why the configured engine is not the one that runs.
#[derive(Debug)]
pub enum VadLoadError {
    /// VAD_ENGINE=silero in a build without ONNX Runtime (the `silero` feature): the station's, for one.
    NotBuilt,
    Failed(anyhow::Error),
}

impl std::fmt::Display for VadLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VadLoadError::NotBuilt => f.write_str("this build has no ONNX Runtime (the silero feature)"),
            VadLoadError::Failed(error) => write!(f, "{error:#}"),
        }
    }
}

pub fn create_vad_engine(config: &VadConfig) -> Result<Arc<dyn VadEngine>, VadLoadError> {
    match config.engine {
        VadKind::Energy => Ok(Arc::new(EnergyVadEngine::new(config.energy_threshold_dbfs))),
        VadKind::Silero => silero(config),
    }
}

#[cfg(feature = "silero")]
fn silero(config: &VadConfig) -> Result<Arc<dyn VadEngine>, VadLoadError> {
    let path = std::path::absolute(&config.model_path).unwrap_or_else(|_| config.model_path.clone());
    match super::silero_vad::SileroVadEngine::load(&path) {
        Ok(engine) => Ok(Arc::new(engine)),
        Err(error) => Err(VadLoadError::Failed(error)),
    }
}

#[cfg(not(feature = "silero"))]
fn silero(_config: &VadConfig) -> Result<Arc<dyn VadEngine>, VadLoadError> {
    Err(VadLoadError::NotBuilt)
}
