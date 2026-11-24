use std::sync::Arc;

use anyhow::Result;
use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use super::openai_transcriber::OpenAiTranscriber;
use crate::config::AsrConfig;

pub trait SpeechRecognizer: Send + Sync {
    /// Provider, the model that serves and the address, for logs and health.
    fn description(&self) -> String;
    /// The model that recognises: the one the server says it serves, else the one asked for.
    fn model(&self) -> String;
    fn recognize<'a>(
        &'a self,
        pcm: &'a [i16],
        sample_rate: u32,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<String>>;
    /// Recognizes with each word's time and the language, where the engine can tell them (the hearing for apps,
    /// `system/hearing` → `recognize`); one that cannot answers the text alone, without words.
    fn recognize_heard<'a>(
        &'a self,
        pcm: &'a [i16],
        sample_rate: u32,
        language: Option<&'a str>,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<super::Heard>> {
        Box::pin(async move {
            let text = self.recognize(pcm, sample_rate, cancel).await?;
            Ok(super::Heard { text, language: language.map(str::to_string), words: Vec::new() })
        })
    }
    /// Quick reachability check for the startup log, which also learns the model that serves; never fails.
    fn probe(&self) -> BoxFuture<'_, String>;
    /// Asks the server again which model it serves, when that is due; never fails. The asking goes on when the
    /// future is dropped: await it only to know it is done.
    fn refresh(&self) -> BoxFuture<'static, ()>;
    /// The server restarted, perhaps with another model: the next refresh asks it.
    fn forget(&self);
    /// Whether it recognises now: the station's chain does not while its home server is away and no cloud is set
    /// up — the request then goes to Yandex, as with recognition off.
    fn available(&self) -> bool {
        true
    }
    /// Whether it recognises at home now (true) or in a cloud (false); None — read it from the configured address.
    fn at_home(&self) -> Option<bool> {
        None
    }
}

/// None when recognition is off (ASR_PROVIDER=none).
pub fn create_recognizer(config: &AsrConfig) -> Option<Arc<dyn SpeechRecognizer>> {
    if !config.enabled {
        return None;
    }
    Some(Arc::new(OpenAiTranscriber::new(config.clone())))
}
