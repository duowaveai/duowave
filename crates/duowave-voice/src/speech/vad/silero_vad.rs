use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use ort::session::Session;
use ort::value::Tensor;

use super::vad_engine::{VadEngine, VadStream};
use crate::speech::audio::opus_pcm_decoder::SPEECH_SAMPLE_RATE;

/// 32 ms at 16 kHz: the only window size Silero VAD v5+ accepts at this rate.
const WINDOW: usize = 512;
/// The model expects the last 64 samples of the previous window in front of each window.
const CONTEXT: usize = 64;
const STATE: usize = 2 * 128;

/// Silero VAD (MIT, assets/silero_vad.onnx) on ONNX Runtime. One session is shared by all requests; the recurrent
/// state and the context samples are per request.
pub struct SileroVadEngine {
    session: Arc<Mutex<Session>>,
}

impl SileroVadEngine {
    pub fn load(model_path: &Path) -> Result<SileroVadEngine> {
        let session = (|| -> ort::Result<Session> {
            let mut builder = Session::builder()?.with_intra_threads(1)?.with_inter_threads(1)?;
            builder.commit_from_file(model_path)
        })()
        .map_err(|error| anyhow!("{error}"))
        .with_context(|| format!("{}", model_path.display()))?;
        Ok(SileroVadEngine { session: Arc::new(Mutex::new(session)) })
    }
}

impl VadEngine for SileroVadEngine {
    fn name(&self) -> &'static str {
        "silero"
    }

    fn window_samples(&self) -> usize {
        WINDOW
    }

    fn create_stream(&self) -> Box<dyn VadStream> {
        Box::new(SileroVadStream {
            session: self.session.clone(),
            state: vec![0.0; STATE],
            input: vec![0.0; CONTEXT + WINDOW],
        })
    }
}

struct SileroVadStream {
    session: Arc<Mutex<Session>>,
    state: Vec<f32>,
    input: Vec<f32>,
}

impl VadStream for SileroVadStream {
    fn probability(&mut self, window: &[f32]) -> Result<f64> {
        let Some(window) = window.get(..WINDOW) else { bail!("silero takes windows of {WINDOW} samples") };
        self.input[CONTEXT..].copy_from_slice(window);
        let run = || -> ort::Result<(Vec<f32>, f32)> {
            let input = Tensor::from_array(([1usize, CONTEXT + WINDOW], self.input.clone()))?;
            let state = Tensor::from_array(([2usize, 1, 128], self.state.clone()))?;
            let sample_rate = Tensor::from_array(((), vec![i64::from(SPEECH_SAMPLE_RATE)]))?;
            let mut session = self.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let outputs = session.run(ort::inputs!["input" => input, "state" => state, "sr" => sample_rate])?;
            let (_, state) = outputs["stateN"].try_extract_tensor::<f32>()?;
            let (_, output) = outputs["output"].try_extract_tensor::<f32>()?;
            Ok((state.to_vec(), output.first().copied().unwrap_or(0.0)))
        };
        let (state, probability) = run().map_err(|error| anyhow!("silero: {error}"))?;
        if state.len() != STATE {
            bail!("silero: a state of {} values, not {STATE}", state.len());
        }
        self.state = state;
        self.input.copy_within(WINDOW.., 0);
        Ok(f64::from(probability))
    }
}
