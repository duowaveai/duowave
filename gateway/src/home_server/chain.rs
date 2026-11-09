//! Recognition on the station with a home server: the computer's while it is up, else the cloud's set up in the
//! station's .env, else none — and with none the gateway leaves the request to Yandex, as it does without recognition.
//! A failed recognition at the computer goes on to the cloud in the same request.

use std::sync::Arc;

use anyhow::Result;
use futures::future::BoxFuture;
use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use super::HomeServerService;
use crate::speech::asr::speech_recognizer::SpeechRecognizer;

/// The station's recognition: the home server's, then a recognizer set up in the cloud. One at a local address is
/// no cloud on the station — the build's default (127.0.0.1:8090) is a computer's server, and there is none on the
/// station: without the computer the request goes to Yandex rather than to nothing (live, 07.11).
pub fn station_recognizer(config: &crate::config::AsrConfig, home: Arc<HomeServerService>) -> HomeFirst {
    let cloud = crate::speech::asr::speech_recognizer::create_recognizer(config)
        .filter(|_| !crate::operations::networks::is_local_url(Some(&config.base_url)));
    HomeFirst::new(home, cloud)
}

pub struct HomeFirst {
    home: Arc<HomeServerService>,
    cloud: Option<Arc<dyn SpeechRecognizer>>,
}

impl HomeFirst {
    pub fn new(home: Arc<HomeServerService>, cloud: Option<Arc<dyn SpeechRecognizer>>) -> HomeFirst {
        HomeFirst { home, cloud }
    }

    /// The one that answers now: the computer's, else the cloud's.
    fn current(&self) -> Option<Arc<dyn SpeechRecognizer>> {
        self.home.recognizer().or_else(|| self.cloud.clone())
    }
}

impl SpeechRecognizer for HomeFirst {
    fn description(&self) -> String {
        match (self.home.recognizer(), &self.cloud) {
            (Some(home), _) => format!("home server: {}", home.description()),
            (None, Some(cloud)) => format!("{} (the home server is away)", cloud.description()),
            (None, None) => "off: the home server is away and no cloud is set up".into(),
        }
    }

    fn model(&self) -> String {
        self.current().map(|recognizer| recognizer.model()).unwrap_or_else(|| "off".into())
    }

    fn recognize<'a>(
        &'a self,
        pcm: &'a [i16],
        sample_rate: u32,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<String>> {
        async move {
            if let Some(home) = self.home.recognizer() {
                match home.recognize(pcm, sample_rate, cancel).await {
                    Ok(text) => return Ok(text),
                    Err(error) if self.cloud.is_some() && !cancel.is_cancelled() => {
                        tracing::warn!(target: "speech", "home server recognition failed, the cloud instead: {error:#}");
                    }
                    Err(error) => return Err(error),
                }
            }
            match &self.cloud {
                Some(cloud) => cloud.recognize(pcm, sample_rate, cancel).await,
                None => anyhow::bail!("recognition: the home server is away and no cloud is set up"),
            }
        }
        .boxed()
    }

    fn probe(&self) -> BoxFuture<'_, String> {
        async move {
            match self.current() {
                Some(recognizer) => recognizer.probe().await,
                None => "away".into(),
            }
        }
        .boxed()
    }

    fn refresh(&self) -> BoxFuture<'static, ()> {
        match self.current() {
            Some(recognizer) => recognizer.refresh(),
            None => futures::future::ready(()).boxed(),
        }
    }

    fn forget(&self) {
        if let Some(recognizer) = self.current() {
            recognizer.forget();
        }
    }

    /// Whether anything recognises now: without, the request goes to Yandex as it does with recognition off.
    fn available(&self) -> bool {
        self.home.recognizer().is_some() || self.cloud.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::core::events::EventBus;
    use crate::secrets::SecretsService;
    use crate::settings::SettingsService;
    use crate::storage::Db;

    fn home() -> Arc<HomeServerService> {
        let config = Arc::new(Config::for_tests(&Env::default()));
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let secrets = Arc::new(SecretsService::new(db, None).unwrap());
        HomeServerService::new(config, settings, secrets, EventBus::new())
    }

    #[test]
    fn on_the_station_a_local_recognizer_is_none_and_without_the_computer_yandex_hears() {
        let local = Config::for_tests(&Env::from_pairs([("ASR_BASE_URL", "http://127.0.0.1:8090/v1")])).speech.asr;
        let chain = station_recognizer(&local, home());
        assert!(!chain.available(), "no computer, no cloud: the request goes to Yandex");
        let cloud =
            Config::for_tests(&Env::from_pairs([("ASR_BASE_URL", "https://api.groq.com/openai/v1")])).speech.asr;
        assert!(station_recognizer(&cloud, home()).available(), "a cloud's recognizer is the way down");
    }
}
