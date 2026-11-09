//! The VAD engine and the recognizer, built from config, and one SpeechPipeline per voice request.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::asr::speech_recognizer::{create_recognizer, SpeechRecognizer};
use super::asr::transcript::{read_transcript, Transcript};
use super::audio::opus_pcm_decoder::SPEECH_SAMPLE_RATE;
use super::audio::pcm::js_round;
use super::speech_pipeline::{SpeechPipeline, SpeechPipelineOptions, UtteranceAudio};
use super::vad::energy_vad::EnergyVadEngine;
use super::vad::vad_engine::{create_vad_engine, VadEngine, VadLoadError};
use crate::config::{SpeechConfig, VadEngine as VadKind};
use crate::core::events::{EventBus, GatewayEvent};
use crate::utils::time::now_ms;

/// A recognition server at home (ours, asr-server/) is kept warm. Live 06.11: after a day and a half without a
/// request the first recognition took the server 12.7 s, past ASR_TIMEOUT_MS, and the request failed; the next ones
/// took 50–150 ms. So it hears half a second of silence at the start and after each quarter of an hour without a
/// request. A cloud is never asked: it bills every request.
const KEEP_WARM_IDLE_MS: u64 = 15 * 60_000;
const WARM_CLIP_SAMPLES: usize = SPEECH_SAMPLE_RATE as usize / 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recognition {
    /// What the recognizer returned.
    pub raw: String,
    /// The command: wake word and hallucinations removed.
    pub text: String,
    /// The command follows a repeated wake word: the audio holds more than it («Алиса, стоп. Алиса, включи кино»).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub restarted: bool,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SpeechStatus {
    pub vad: String,
    pub asr: String,
}

pub struct SpeechService {
    config: SpeechConfig,
    recognizer: Option<Arc<dyn SpeechRecognizer>>,
    engine: Arc<dyn VadEngine>,
    vad_problem: Option<String>,
    /// When the recognizer was last asked (ms since the epoch), by a request or the warm-up.
    asked_at: Arc<AtomicU64>,
    /// Ends the warm-up when the service goes.
    stop: CancellationToken,
}

impl Drop for SpeechService {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// The server is at home — loopback, a private address, a .local name — and costs nothing to ask.
fn at_home(base_url: &str) -> bool {
    let Ok(url) = url::Url::parse(base_url) else { return false };
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(name)) => {
            name == "localhost" || name.ends_with(".localhost") || name.ends_with(".local")
        }
        None => false,
    }
}

impl SpeechService {
    /// The VAD engine and the recognizer from config. A VAD that cannot load gives way to the energy VAD: a gateway
    /// that cannot start is worse than a cruder VAD; health and the log show the reason.
    pub fn new(config: SpeechConfig) -> SpeechService {
        let recognizer = create_recognizer(&config.asr);
        SpeechService::with_recognizer(config, recognizer)
    }

    /// With a recognizer of one's own (a test's), or none.
    pub fn with_recognizer(config: SpeechConfig, recognizer: Option<Arc<dyn SpeechRecognizer>>) -> SpeechService {
        let name = match config.vad.engine {
            VadKind::Silero => "silero",
            VadKind::Energy => "energy",
        };
        let (engine, vad_problem) = match create_vad_engine(&config.vad) {
            Ok(engine) => (engine, None),
            Err(error) => {
                let problem = match &error {
                    VadLoadError::NotBuilt => format!("{name} is not in this build: {error}"),
                    VadLoadError::Failed(_) => format!("{name} failed to load: {error}"),
                };
                match error {
                    VadLoadError::NotBuilt => {
                        tracing::warn!(target: "speech", "VAD {problem}; using the energy VAD instead")
                    }
                    VadLoadError::Failed(_) => {
                        tracing::error!(target: "speech", "VAD {problem}; using the energy VAD instead")
                    }
                }
                let energy: Arc<dyn VadEngine> = Arc::new(EnergyVadEngine::new(config.vad.energy_threshold_dbfs));
                (energy, Some(problem))
            }
        };
        let recognition =
            recognizer.as_ref().map_or_else(|| "off (ASR_PROVIDER=none)".to_string(), |asr| asr.description());
        tracing::info!(target: "speech", "VAD: {}; recognition: {recognition}", engine.name());
        SpeechService {
            config,
            recognizer,
            engine,
            vad_problem,
            asked_at: Arc::new(AtomicU64::new(0)),
            stop: CancellationToken::new(),
        }
    }

    /// The gateway's side of the start: a recognition server restarted from the panel may serve another model (it
    /// is asked at the next look), and the startup log says whether the server is there. Needs the runtime.
    pub fn start(&self, events: &EventBus) {
        let Some(recognizer) = self.recognizer.clone() else { return };
        let listener = recognizer.clone();
        events.on("speech", move |event| {
            if let GatewayEvent::ModelServerRestarted { which, .. } = &*event {
                if which == "asr" {
                    listener.forget();
                }
            }
        });
        let asked = self.config.asr.model.clone();
        let warm = recognizer.clone();
        tokio::spawn(async move {
            let state = recognizer.probe().await;
            let serving = recognizer.model();
            let other =
                if serving == asked { String::new() } else { format!(", serving {serving} (ASR_MODEL names {asked})") };
            tracing::info!(target: "speech", "recognition service {state}{other}");
        });
        if at_home(&self.config.asr.base_url) {
            tokio::spawn(keep_warm(warm, self.asked_at.clone(), self.stop.clone()));
        }
    }

    pub fn config(&self) -> &SpeechConfig {
        &self.config
    }

    pub fn recognition_enabled(&self) -> bool {
        self.recognizer.as_ref().is_some_and(|recognizer| recognizer.available())
    }

    /// The pipeline of one voice request. Unknown activation types wait for SpotterEnd too: holding audio costs
    /// latency, skipping the guard costs a wrong end.
    pub fn create_pipeline(&self, activation_type: Option<&str>) -> SpeechPipeline {
        SpeechPipeline::new(
            self.engine.as_ref(),
            SpeechPipelineOptions {
                endpoint: self.config.endpoint.clone(),
                gain: self.config.gain.clone(),
                expect_wake_word: activation_type.is_none_or(|kind| kind.is_empty() || kind == "spotter"),
            },
        )
    }

    pub async fn recognize(
        &self,
        audio: &UtteranceAudio,
        extra_wake_words: &[String],
        cancel: &CancellationToken,
    ) -> Result<Recognition> {
        let Some(recognizer) = &self.recognizer else { bail!("recognition is off (ASR_PROVIDER=none)") };
        let started = Instant::now();
        self.asked_at.store(now_ms(), Ordering::Relaxed);
        let raw = recognizer.recognize(&audio.pcm, SPEECH_SAMPLE_RATE, cancel).await?;
        let wake_words: Vec<String> = self.config.asr.wake_words.iter().chain(extra_wake_words).cloned().collect();
        let Transcript { text, restarted } = read_transcript(&raw, &wake_words);
        Ok(Recognition { raw, text, restarted, latency_ms: js_round(started.elapsed().as_secs_f64() * 1000.0) as u64 })
    }

    /// The model that recognises, as its server says: the journal and the dashboard name it. A look asks the server
    /// again when that is due (a minute after its last answer, at once after its restart). None: recognition is off.
    pub fn ear(&self) -> Option<String> {
        let recognizer = self.recognizer.as_ref()?;
        drop(recognizer.refresh());
        Some(recognizer.model())
    }

    pub fn status(&self) -> SpeechStatus {
        if let Some(recognizer) = &self.recognizer {
            drop(recognizer.refresh());
        }
        SpeechStatus {
            vad: self.vad_status(),
            asr: self.recognizer.as_ref().map_or_else(|| "off".to_string(), |asr| asr.description()),
        }
    }

    /// The VAD that runs, and why it is not the configured one when it is not: «energy (silero is not in this
    /// build: …)».
    pub fn vad_status(&self) -> String {
        match &self.vad_problem {
            Some(problem) => format!("{} ({problem})", self.engine.name()),
            None => self.engine.name().to_string(),
        }
    }
}

/// Half a second of silence whenever the recognizer has not been asked for a quarter of an hour, the first at once.
/// A warm-up past ASR_TIMEOUT_MS still loads the model: the server goes on with it after the gateway stops waiting.
async fn keep_warm(recognizer: Arc<dyn SpeechRecognizer>, asked_at: Arc<AtomicU64>, stop: CancellationToken) {
    let silence = vec![0i16; WARM_CLIP_SAMPLES];
    let mut first = true;
    loop {
        let idle = now_ms().saturating_sub(asked_at.load(Ordering::Relaxed));
        let wait = if idle >= KEEP_WARM_IDLE_MS {
            asked_at.store(now_ms(), Ordering::Relaxed);
            let started = Instant::now();
            let result = recognizer.recognize(&silence, SPEECH_SAMPLE_RATE, &stop).await;
            if stop.is_cancelled() {
                return;
            }
            let ms = started.elapsed().as_millis();
            match result {
                Ok(_) if first => tracing::info!(target: "speech", "recognition warmed up in {ms} ms"),
                Ok(_) => tracing::debug!(target: "speech", "recognition kept warm in {ms} ms"),
                Err(error) if first => {
                    tracing::warn!(target: "speech", "recognition warm-up failed after {ms} ms: {error}")
                }
                Err(error) => tracing::debug!(target: "speech", "recognition keep-warm failed after {ms} ms: {error}"),
            }
            first = false;
            KEEP_WARM_IDLE_MS
        } else {
            KEEP_WARM_IDLE_MS - idle
        };
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures::future::{BoxFuture, FutureExt};

    use super::*;
    use crate::config::{Config, Env};
    use crate::speech::speech_pipeline::SpeechEvent;
    use crate::speech::test_audio::{concat, silence, station_audio, voice};

    /// A recognizer that hears what it is told to.
    struct Heard {
        text: String,
        forgotten: Mutex<u32>,
    }

    impl SpeechRecognizer for Heard {
        fn description(&self) -> String {
            "heard via test".into()
        }

        fn model(&self) -> String {
            "test".into()
        }

        fn recognize<'a>(
            &'a self,
            _pcm: &'a [i16],
            _sample_rate: u32,
            _cancel: &'a CancellationToken,
        ) -> BoxFuture<'a, Result<String>> {
            futures::future::ready(Ok(self.text.clone())).boxed()
        }

        fn probe(&self) -> BoxFuture<'_, String> {
            futures::future::ready("reachable".to_string()).boxed()
        }

        fn refresh(&self) -> BoxFuture<'static, ()> {
            futures::future::ready(()).boxed()
        }

        fn forget(&self) {
            *self.forgotten.lock().unwrap() += 1;
        }
    }

    fn config(pairs: &[(&str, &str)]) -> SpeechConfig {
        Config::load(&Env::from_pairs(pairs.iter().copied())).unwrap().speech
    }

    #[tokio::test]
    async fn recognises_a_request_and_takes_the_wake_word_out() {
        let heard = Arc::new(Heard {
            text: "Слушай, Алиса, включи свет.".into(), forgotten: Mutex::new(0)
        });
        let speech = SpeechService::with_recognizer(config(&[("VAD_ENGINE", "energy")]), Some(heard.clone()));
        let events = EventBus::new();
        speech.start(&events);
        assert_eq!(speech.status(), SpeechStatus { vad: "energy".into(), asr: "heard via test".into() });
        assert_eq!(speech.ear().as_deref(), Some("test"));

        let mut pipeline = speech.create_pipeline(Some("spotter"));
        let audio = station_audio(
            &concat(&[&silence(1200.0), &voice(800.0, -50.0)]),
            &concat(&[&voice(800.0, -50.0), &silence(1500.0)]),
        );
        pipeline.push(&audio.bytes[..audio.spotter_end_at]);
        pipeline.mark_spotter_end();
        pipeline.push(&audio.bytes[audio.spotter_end_at..]);
        assert!(pipeline.flush().iter().any(|event| matches!(event, SpeechEvent::End { .. })));
        let utterance = pipeline.utterance_audio().unwrap();
        let recognition = speech.recognize(&utterance, &["яндекс".into()], &CancellationToken::new()).await.unwrap();
        assert_eq!(recognition.raw, "Слушай, Алиса, включи свет.");
        assert_eq!(recognition.text, "Включи свет.");
        assert!(!recognition.restarted);

        // A recognition server restarted from the panel: its model is asked again.
        events.emit(GatewayEvent::ModelServerRestarted { which: "asr".into(), actor: None });
        for _ in 0..100 {
            if *heard.forgotten.lock().unwrap() > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(*heard.forgotten.lock().unwrap(), 1);
    }

    /// Counts what it is asked; silence is heard as nothing.
    struct Counting(std::sync::atomic::AtomicUsize);

    impl SpeechRecognizer for Counting {
        fn description(&self) -> String {
            "counting".into()
        }
        fn model(&self) -> String {
            "test".into()
        }
        fn recognize<'a>(
            &'a self,
            pcm: &'a [i16],
            _sample_rate: u32,
            _cancel: &'a CancellationToken,
        ) -> BoxFuture<'a, Result<String>> {
            assert!(pcm.iter().all(|sample| *sample == 0), "the warm-up sends silence");
            self.0.fetch_add(1, Ordering::SeqCst);
            futures::future::ready(Ok(String::new())).boxed()
        }
        fn probe(&self) -> BoxFuture<'_, String> {
            futures::future::ready("reachable".to_string()).boxed()
        }
        fn refresh(&self) -> BoxFuture<'static, ()> {
            futures::future::ready(()).boxed()
        }
        fn forget(&self) {}
    }

    async fn asked(counting: &Counting, at_least: usize) -> usize {
        for _ in 0..200 {
            if counting.0.load(Ordering::SeqCst) >= at_least {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        counting.0.load(Ordering::SeqCst)
    }

    #[tokio::test]
    async fn warms_a_recognizer_at_home_at_the_start_and_never_a_cloud() {
        let home = Arc::new(Counting(Default::default()));
        let speech = SpeechService::with_recognizer(
            config(&[("VAD_ENGINE", "energy"), ("ASR_BASE_URL", "http://127.0.0.1:8090/v1")]),
            Some(home.clone()),
        );
        speech.start(&EventBus::new());
        assert_eq!(asked(&home, 1).await, 1, "half a second of silence at the start");
        // Asked just now: the next warm-up is a quarter of an hour away.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(home.0.load(Ordering::SeqCst), 1);
        drop(speech);

        let cloud = Arc::new(Counting(Default::default()));
        let speech = SpeechService::with_recognizer(
            config(&[("VAD_ENGINE", "energy"), ("ASR_BASE_URL", "https://api.openai.com/v1")]),
            Some(cloud.clone()),
        );
        speech.start(&EventBus::new());
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(cloud.0.load(Ordering::SeqCst), 0, "a cloud bills every request");
    }

    #[test]
    fn knows_a_server_at_home() {
        for url in [
            "http://127.0.0.1:8090/v1",
            "http://localhost:8090/v1",
            "http://192.168.1.5:8090/v1",
            "http://mac.local:8090/v1",
            "http://[::1]:8090/v1",
        ] {
            assert!(at_home(url), "{url}");
        }
        for url in ["https://api.openai.com/v1", "https://api.groq.com/openai/v1", "http://8.8.8.8/v1", "not a url"] {
            assert!(!at_home(url), "{url}");
        }
    }

    #[test]
    fn falls_back_to_the_energy_vad_and_says_why() {
        let speech = SpeechService::with_recognizer(
            config(&[("VAD_ENGINE", "silero"), ("VAD_MODEL_PATH", "/nowhere/silero_vad.onnx")]),
            None,
        );
        let status = speech.status();
        assert!(status.vad.starts_with("energy (silero "), "{}", status.vad);
        assert_eq!(status.asr, "off");
        assert!(!speech.recognition_enabled());
        assert_eq!(speech.ear(), None);
        // Without a wake word the command starts at once.
        assert_eq!(speech.create_pipeline(Some("button")).command_start_ms(), Some(0.0));
        assert_eq!(speech.create_pipeline(None).command_start_ms(), None);
    }
}
