//! The gateway's recognizer over the local engine: the same `SpeechRecognizer` as the HTTP one, so the pipeline, the
//! keep-warm and the station's home server use it unchanged.

use std::sync::Arc;

use anyhow::Result;
use futures::future::{BoxFuture, FutureExt};
use tokio_util::sync::CancellationToken;

use super::supervisor::LocalAsr;
use crate::speech::asr::speech_recognizer::SpeechRecognizer;

pub struct LocalRecognizer {
    asr: Arc<LocalAsr>,
}

impl LocalRecognizer {
    pub fn new(asr: Arc<LocalAsr>) -> LocalRecognizer {
        LocalRecognizer { asr }
    }
}

impl SpeechRecognizer for LocalRecognizer {
    fn description(&self) -> String {
        let status = self.asr.status();
        let model = status.serving.or(status.model).unwrap_or_else(|| "no model yet".into());
        match status.backend {
            Some(backend) => format!("{model} on this computer ({backend})"),
            None => format!("{model} on this computer"),
        }
    }

    fn model(&self) -> String {
        self.asr.model()
    }

    fn recognize<'a>(
        &'a self,
        pcm: &'a [i16],
        sample_rate: u32,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<String>> {
        async move { self.asr.recognize(pcm, sample_rate, cancel).await }.boxed()
    }

    fn probe(&self) -> BoxFuture<'_, String> {
        let status = self.asr.status();
        let answer = if self.asr.ready() {
            "ready".to_string()
        } else {
            format!("not ready yet ({})", serde_json::to_value(status.phase).unwrap_or_default())
        };
        futures::future::ready(answer).boxed()
    }

    fn refresh(&self) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }

    fn forget(&self) {}
}
