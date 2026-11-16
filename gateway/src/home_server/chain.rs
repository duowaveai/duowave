//! Recognition on the station with its helpers (docs/plans/home-and-helpers.md, «Слух»): the helpers that hear, in the
//! list's order — an error at one goes to the next in the same request —, then the cloud set up in the station's panel
//! or .env (src/speech/cloud_speech.rs) while the regulator lets the clouds out, else none — and with none the gateway leaves the request to Yandex, as it
//! does without recognition (the caller's rule, unchanged).

use std::sync::Arc;

use anyhow::Result;
use futures::future::BoxFuture;
use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use crate::helpers::contract::Ability;
use crate::helpers::{Giver, Helpers, Reach};
use crate::speech::asr::speech_recognizer::SpeechRecognizer;
use crate::speech::asr::Heard;
use crate::speech::cloud_speech::CloudHearing;

/// The station's recognition: the helpers', then a recognizer set up in the cloud. One at a local address is no cloud
/// on the station — the build's default (127.0.0.1:8090) is a computer's server, and there is none on the station:
/// without the helpers the request goes to Yandex rather than to nothing (live, 07.11).
pub fn hearing_chain(config: &crate::config::AsrConfig, helpers: Arc<Helpers>) -> HelpersFirst {
    let cloud = crate::speech::asr::speech_recognizer::create_recognizer(config)
        .filter(|_| !crate::operations::networks::is_local_url(Some(&config.base_url)));
    HelpersFirst::new(helpers, cloud)
}

/// The station's recognition with its cloud as the panel (or the .env) sets it up — read on every request
/// (docs/plans/speaker-alone.md).
pub fn station_chain(cloud: Arc<dyn CloudHearing>, helpers: Arc<Helpers>) -> HelpersFirst {
    HelpersFirst { helpers, cloud }
}

pub struct HelpersFirst {
    helpers: Arc<Helpers>,
    cloud: Arc<dyn CloudHearing>,
}

/// One that hears now: who it is, and its recognizer.
struct Ear {
    giver: Giver,
    recognizer: Arc<dyn SpeechRecognizer>,
}

impl HelpersFirst {
    pub fn new(helpers: Arc<Helpers>, cloud: Option<Arc<dyn SpeechRecognizer>>) -> HelpersFirst {
        HelpersFirst { helpers, cloud: Arc::new(cloud) }
    }

    /// The cloud, while the regulator lets the clouds out.
    fn cloud(&self) -> Option<Arc<dyn SpeechRecognizer>> {
        self.cloud.recognizer().filter(|_| self.helpers.lets(Reach::Foreign))
    }

    /// Who hears now, in order: this gateway's own parts first (stage 6), then the helpers.
    fn ears(&self) -> Vec<Ear> {
        self.helpers
            .pick_all(Ability::Hearing)
            .into_iter()
            .filter_map(|giver| {
                let recognizer = match &giver {
                    Giver::Itself => self.helpers.own_recognizer()?,
                    Giver::Helper(remote) => remote.recognizer(),
                };
                Some(Ear { giver, recognizer })
            })
            .collect()
    }

    /// The one that answers now: the first that hears, else the allowed cloud.
    fn current(&self) -> Option<Arc<dyn SpeechRecognizer>> {
        self.ears().into_iter().next().map(|ear| ear.recognizer).or_else(|| self.cloud())
    }
}

impl SpeechRecognizer for HelpersFirst {
    fn description(&self) -> String {
        match (self.ears().into_iter().next(), self.cloud()) {
            (Some(Ear { giver: Giver::Itself, recognizer }), _) => recognizer.description(),
            (Some(Ear { giver: Giver::Helper(remote), recognizer }), _) => {
                format!("helper {}: {}", remote.id(), recognizer.description())
            }
            (None, Some(cloud)) => format!("{} (no helper hears)", cloud.description()),
            (None, None) => "off".into(),
        }
    }

    fn model(&self) -> String {
        self.current().map(|recognizer| recognizer.model()).unwrap_or_else(|| "off".into())
    }

    /// At home while this gateway or a helper at home hears; a helper outside the home or the cloud — not.
    fn at_home(&self) -> Option<bool> {
        Some(match self.ears().into_iter().next().map(|ear| ear.giver) {
            Some(Giver::Itself) => true,
            Some(Giver::Helper(remote)) => remote.reach() == Reach::Home,
            None => false,
        })
    }

    fn recognize<'a>(
        &'a self,
        pcm: &'a [i16],
        sample_rate: u32,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<String>> {
        async move {
            let mut last = None;
            for ear in self.ears() {
                match ear.recognizer.recognize(pcm, sample_rate, cancel).await {
                    Ok(text) => return Ok(text),
                    Err(error) if cancel.is_cancelled() => return Err(error),
                    Err(error) => {
                        tracing::warn!(target: "speech", "a helper's recognition failed, the next way instead: {error:#}");
                        last = Some(error);
                    }
                }
            }
            match (self.cloud(), last) {
                (Some(cloud), _) => cloud.recognize(pcm, sample_rate, cancel).await,
                (None, Some(error)) => Err(error),
                (None, None) => anyhow::bail!("recognition: no helper hears and no cloud is allowed"),
            }
        }
        .boxed()
    }

    /// The same way down for the apps' hearing: each one's words with their times and the language.
    fn recognize_heard<'a>(
        &'a self,
        pcm: &'a [i16],
        sample_rate: u32,
        language: Option<&'a str>,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<Heard>> {
        async move {
            let mut last = None;
            for ear in self.ears() {
                match ear.recognizer.recognize_heard(pcm, sample_rate, language, cancel).await {
                    Ok(heard) => return Ok(heard),
                    Err(error) if cancel.is_cancelled() => return Err(error),
                    Err(error) => {
                        tracing::warn!(target: "speech", "a helper's recognition failed, the next way instead: {error:#}");
                        last = Some(error);
                    }
                }
            }
            match (self.cloud(), last) {
                (Some(cloud), _) => cloud.recognize_heard(pcm, sample_rate, language, cancel).await,
                (None, Some(error)) => Err(error),
                (None, None) => anyhow::bail!("recognition: no helper hears and no cloud is allowed"),
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
        !self.ears().is_empty() || self.cloud().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::helpers::tests::{contract, stand, station, ALL};
    use crate::helpers::{Away, HelperKind};

    /// A cloud recognizer stand-in: whatever it hears is «облако».
    struct Cloud;

    impl SpeechRecognizer for Cloud {
        fn description(&self) -> String {
            "cloud".into()
        }
        fn model(&self) -> String {
            "cloud".into()
        }
        fn recognize<'a>(&'a self, _: &'a [i16], _: u32, _: &'a CancellationToken) -> BoxFuture<'a, Result<String>> {
            async { Ok("облако".to_string()) }.boxed()
        }
        fn probe(&self) -> BoxFuture<'_, String> {
            async { "reachable".to_string() }.boxed()
        }
        fn refresh(&self) -> BoxFuture<'static, ()> {
            futures::future::ready(()).boxed()
        }
        fn forget(&self) {}
    }

    #[tokio::test]
    async fn an_error_at_one_helper_goes_to_the_next_in_the_same_request_then_an_allowed_cloud() {
        let (a, b) = (stand(contract("a", ALL)).await, stand(contract("b", ALL)).await);
        a.deaf.store(true, std::sync::atomic::Ordering::SeqCst);
        *b.heard.lock().unwrap() = "два".into();
        let station = station(&[]);
        station.put("home-a", &a.url, HelperKind::Home);
        station.put("home-b", &b.url, HelperKind::Home);
        station.helpers.check_all().await;
        let chain = HelpersFirst::new(station.helpers.clone(), Some(Arc::new(Cloud)));
        let cancel = CancellationToken::new();
        assert_eq!(chain.recognize(&[0; 1600], 16000, &cancel).await.unwrap(), "два", "the next one, same request");
        assert!(chain.description().starts_with("helper home-a:"), "{}", chain.description());
        assert_eq!(chain.at_home(), Some(true));

        b.deaf.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(chain.recognize(&[0; 1600], 16000, &cancel).await.unwrap(), "облако", "then the cloud");

        // Both away, the regulator keeps the clouds out: nothing hears — the request goes to Yandex.
        a.set_away(true);
        b.set_away(true);
        station.helpers.check_all().await;
        station.helpers.check_all().await;
        assert!(chain.available(), "the cloud still");
        assert_eq!(chain.at_home(), Some(false));
        station.helpers.set_away(Away::Home, None).unwrap();
        assert!(!chain.available());
        assert!(chain.recognize(&[0; 1600], 16000, &cancel).await.is_err());
        assert_eq!(chain.description(), "off");
    }

    #[tokio::test]
    async fn the_hearing_for_apps_keeps_the_words_and_the_language_through_the_chain() {
        let (a, b) = (stand(contract("a", ALL)).await, stand(contract("b", ALL)).await);
        *a.heard.lock().unwrap() = "раз".into();
        *b.heard.lock().unwrap() = "два".into();
        let station = station(&[]);
        station.put("home-a", &a.url, HelperKind::Home);
        station.put("home-b", &b.url, HelperKind::Home);
        station.helpers.check_all().await;
        let chain = hearing_chain(&station.config.speech.asr, station.helpers.clone());
        let cancel = CancellationToken::new();
        let heard = chain.recognize_heard(&[0; 1600], 16000, None, &cancel).await.unwrap();
        assert_eq!((heard.text.as_str(), heard.language.as_deref()), ("раз", Some("en")));
        assert_eq!(heard.words.len(), 1, "the words' times reach the apps");
        a.deaf.store(true, std::sync::atomic::Ordering::SeqCst);
        let heard = chain.recognize_heard(&[0; 1600], 16000, None, &cancel).await.unwrap();
        assert_eq!((heard.text.as_str(), heard.words[0].word.as_str()), ("два", "два"), "on its error the next one's");
    }

    /// What hears on this computer itself: the local engine's words and the language it heard.
    struct Engine;

    impl SpeechRecognizer for Engine {
        fn description(&self) -> String {
            "gigaam-v3 on this computer (metal)".into()
        }
        fn model(&self) -> String {
            "gigaam-v3".into()
        }
        fn recognize<'a>(&'a self, _: &'a [i16], _: u32, _: &'a CancellationToken) -> BoxFuture<'a, Result<String>> {
            async { Ok("включи свет".to_string()) }.boxed()
        }
        fn recognize_heard<'a>(
            &'a self,
            _: &'a [i16],
            _: u32,
            _: Option<&'a str>,
            _: &'a CancellationToken,
        ) -> BoxFuture<'a, Result<Heard>> {
            async {
                let words = vec![crate::speech::asr::HeardWord { word: "свет".into(), start: 0.2, end: 0.6 }];
                Ok(Heard { text: "включи свет".into(), language: Some("ru".into()), words })
            }
            .boxed()
        }
        fn probe(&self) -> BoxFuture<'_, String> {
            async { "ready".to_string() }.boxed()
        }
        fn refresh(&self) -> BoxFuture<'static, ()> {
            futures::future::ready(()).boxed()
        }
        fn forget(&self) {}
    }

    /// A computer's own parts as the helpers take them (the built-in helper, stage 6): its hearing, when it has one.
    struct Own(Option<Arc<dyn SpeechRecognizer>>);

    impl crate::helpers::OwnParts for Own {
        fn contract(&self) -> crate::helpers::contract::Contract {
            crate::helpers::contract::Contract::new(Default::default(), Vec::new())
        }
        fn gives_here(&self, ability: Ability) -> bool {
            ability == Ability::Hearing && self.0.is_some()
        }
        fn recognizer(&self) -> Option<Arc<dyn SpeechRecognizer>> {
            self.0.clone()
        }
        fn speech(&self) -> Option<Arc<crate::speech::tts::own_speech::OwnSpeech>> {
            None
        }
        fn models(&self) -> Vec<crate::helpers::OwnModel> {
            Vec::new()
        }
        fn is_me(&self, _: &str) -> bool {
            false
        }
        fn check(&self) -> BoxFuture<'_, ()> {
            futures::future::ready(()).boxed()
        }
    }

    /// A computer's helpers (not in the station role), its own parts these.
    fn computer(own: Own, asr: &crate::config::AsrConfig) -> HelpersFirst {
        use crate::core::build::Profile;
        use crate::core::events::EventBus;
        use crate::core::identity::home::{DeviceKind, Identity};
        let config = Arc::new(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")])));
        let db = Arc::new(crate::storage::Db::memory());
        let events = EventBus::new();
        let settings = Arc::new(crate::settings::SettingsService::new(db.clone(), events.clone()));
        let secrets = Arc::new(crate::secrets::SecretsService::new(db.clone(), None).unwrap());
        let profile = Profile::for_role(None);
        let kind = DeviceKind::of(profile);
        let identity = Arc::new(Identity::new(settings.clone(), secrets.clone(), db, kind, events.clone()));
        identity.ensure().unwrap();
        let helpers = Helpers::new(config, settings, secrets, events, profile, identity);
        helpers.set_own(Arc::new(own));
        hearing_chain(asr, helpers)
    }

    #[tokio::test]
    async fn on_a_computer_the_chain_says_what_its_own_recognizer_says_and_off_when_nothing_hears() {
        let none = Config::for_tests(&Env::from_pairs([("ASR_PROVIDER", "none")])).speech.asr;
        let chain = computer(Own(Some(Arc::new(Engine))), &none);
        assert_eq!(chain.description(), "gigaam-v3 on this computer (metal)", "its own words, verbatim");
        assert_eq!((chain.model(), chain.at_home(), chain.available()), ("gigaam-v3".to_string(), Some(true), true));
        let cancel = CancellationToken::new();
        assert_eq!(chain.recognize(&[0; 1600], 16000, &cancel).await.unwrap(), "включи свет");
        let heard = chain.recognize_heard(&[0; 1600], 16000, None, &cancel).await.unwrap();
        assert_eq!((heard.language.as_deref(), heard.words.len()), (Some("ru"), 1), "the engine's words and language");

        // Nothing of its own hears, no helper, no cloud: off, as with recognition off.
        let deaf = computer(Own(None), &none);
        assert_eq!((deaf.description(), deaf.available(), deaf.at_home()), ("off".to_string(), false, Some(false)));
        assert!(deaf.recognize(&[0; 1600], 16000, &cancel).await.is_err());
    }

    #[tokio::test]
    async fn on_the_station_a_local_recognizer_is_none_and_without_helpers_yandex_hears() {
        let station = station(&[]);
        let local = Config::for_tests(&Env::from_pairs([("ASR_BASE_URL", "http://127.0.0.1:8090/v1")])).speech.asr;
        let chain = hearing_chain(&local, station.helpers.clone());
        assert!(!chain.available(), "no helper, no cloud: the request goes to Yandex");
        assert_eq!(chain.description(), "off");
        let cloud =
            Config::for_tests(&Env::from_pairs([("ASR_BASE_URL", "https://api.groq.com/openai/v1")])).speech.asr;
        assert!(hearing_chain(&cloud, station.helpers.clone()).available(), "a cloud's recognizer is the way down");
    }
}
