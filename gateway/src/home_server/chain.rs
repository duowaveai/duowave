//! Recognition on the station with a home server: the computer's while it is up, else the cloud's set up in the
//! station's panel or .env (src/speech/cloud_speech.rs), else none — and with none the gateway leaves the request to Yandex, as it does without recognition.
//! A failed recognition at the computer goes on to the cloud in the same request.

use std::sync::Arc;

use anyhow::Result;
use futures::future::BoxFuture;
use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use super::HomeServerService;
use crate::speech::asr::speech_recognizer::SpeechRecognizer;
use crate::speech::cloud_speech::CloudHearing;

/// The station's recognition: the home server's, then the cloud's as the panel (or the .env) sets it up — read on
/// every request. One at a local address is no cloud on the station — the build's default (127.0.0.1:8090) is a
/// computer's server, and there is none on the station: without the computer the request goes to Yandex rather than
/// to nothing (live, 07.11).
pub fn station_recognizer(cloud: Arc<dyn CloudHearing>, home: Arc<HomeServerService>) -> HomeFirst {
    HomeFirst::new(home, cloud)
}

pub struct HomeFirst {
    home: Arc<HomeServerService>,
    cloud: Arc<dyn CloudHearing>,
}

impl HomeFirst {
    pub fn new(home: Arc<HomeServerService>, cloud: Arc<dyn CloudHearing>) -> HomeFirst {
        HomeFirst { home, cloud }
    }

    fn cloud(&self) -> Option<Arc<dyn SpeechRecognizer>> {
        self.cloud.recognizer()
    }

    /// The one that answers now: the computer's, else the cloud's.
    fn current(&self) -> Option<Arc<dyn SpeechRecognizer>> {
        self.home.recognizer().or_else(|| self.cloud())
    }
}

impl SpeechRecognizer for HomeFirst {
    fn description(&self) -> String {
        match (self.home.recognizer(), &self.cloud()) {
            (Some(home), _) => format!("home server: {}", home.description()),
            (None, Some(cloud)) => format!("{} (the home server is away)", cloud.description()),
            (None, None) => "off: the home server is away and no cloud is set up".into(),
        }
    }

    fn model(&self) -> String {
        self.current().map(|recognizer| recognizer.model()).unwrap_or_else(|| "off".into())
    }

    /// At home while the computer recognises; the cloud's otherwise.
    fn at_home(&self) -> Option<bool> {
        Some(self.home.recognizer().is_some())
    }

    fn recognize<'a>(
        &'a self,
        pcm: &'a [i16],
        sample_rate: u32,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<String>> {
        async move {
            let cloud = self.cloud();
            if let Some(home) = self.home.recognizer() {
                match home.recognize(pcm, sample_rate, cancel).await {
                    Ok(text) => return Ok(text),
                    Err(error) if cloud.is_some() && !cancel.is_cancelled() => {
                        tracing::warn!(target: "speech", "home server recognition failed, the cloud instead: {error:#}");
                    }
                    Err(error) => return Err(error),
                }
            }
            match &cloud {
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
        self.home.recognizer().is_some() || self.cloud().is_some()
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

    /// The station's cloud as its .env sets it up, the panel setting nothing.
    fn cloud_of(url: &str) -> Arc<dyn CloudHearing> {
        let config = Config::for_tests(&Env::from_pairs([("ASR_BASE_URL", url)]));
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let secrets = Arc::new(SecretsService::new(db, None).unwrap());
        Arc::new(crate::speech::cloud_speech::CloudSpeech::new(settings, secrets, config.speech.asr, None, true))
    }

    #[test]
    fn on_the_station_a_local_recognizer_is_none_and_without_the_computer_yandex_hears() {
        let chain = station_recognizer(cloud_of("http://127.0.0.1:8090/v1"), home());
        assert!(!chain.available(), "no computer, no cloud: the request goes to Yandex");
        let chain = station_recognizer(cloud_of("https://api.groq.com/openai/v1"), home());
        assert!(chain.available(), "a cloud's recognizer is the way down");
        assert_eq!(chain.at_home(), Some(false), "the cloud hears: no interim requests");
    }
}
