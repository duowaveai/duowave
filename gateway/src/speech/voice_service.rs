//! Whose voice answers, decided in one place and read on every answer.

use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{bail, Result};
use serde::Serialize;
use serde_json::{Map, Value};

use super::tts::own_speech::{OwnSpeech, SpeechRoute};
use crate::config::{GatewayConfig, TtsConfig, TtsProvider};
use crate::core::events::{EventBus, GatewayEvent};
use crate::settings::definitions::{VOICE_CHOICE, VOICE_PROVIDER};
use crate::settings::SettingsService;

/// An own voice's card id: `own:<voice id>` (src/models).
const OWN_PREFIX: &str = "own:";

/// The pair our engine is asked for: (model, voice) as the voice server names them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VoicePair {
    pub model: String,
    pub voice: String,
}

/// Whose voice the panel shows: Alice's or ours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Whose {
    Yandex,
    Own,
}

/// A voice of the catalog, as a stored choice resolves to it: its catalog ids, the pair the voice server names, and
/// for an own voice its server's route.
#[derive(Clone)]
pub struct VoiceChoice {
    pub model_id: String,
    pub voice_id: String,
    /// The engine as the voice server names it: silero, piper, an own voice's engine.
    pub engine: String,
    pub voice: String,
    pub route: Option<Arc<SpeechRoute>>,
}

/// The model catalog and the voice lab (src/models, src/voice_lab — not ported yet): what a stored voice choice
/// names. The models module implements it.
pub trait VoiceCatalog: Send + Sync {
    /// A stored choice (`{modelId, voiceId}` by catalog ids, or the older `{model, voice}` pair) at the start: what
    /// it names, an own voice with its route (its server starts in the background, the gateway does not wait for
    /// it). None: the catalog has no such voice.
    fn resolve_stored(&self, stored: &Map<String, Value>) -> Option<VoiceChoice>;
}

struct Chosen {
    provider: TtsProvider,
    own_voice: Option<VoicePair>,
}

/// The provider a computer's gateway speaks with, as its start settles it (`restore_from_settings`): the stored choice
/// when it can speak — «ours» needs a voice server of ours (TTS_OWN_URL) — the env's `own` kept over a stored `mixed`;
/// else the env's. A move to the station reads the computer's so (src/placement/moves.rs).
pub fn provider_at_start(env: &TtsConfig, stored: Option<TtsProvider>) -> TtsProvider {
    let usable =
        stored.filter(|provider| matches!(provider, TtsProvider::Yandex | TtsProvider::None) || env.own.is_some());
    match usable {
        Some(TtsProvider::Mixed) if env.provider == TtsProvider::Own => TtsProvider::Own,
        Some(provider) => provider,
        None => env.provider,
    }
}

/// Our voice through the station's home server while it is up (src/home_server): the computer speaks with the voice
/// it has chosen, the voice lab's too.
pub trait HomeVoice: Send + Sync {
    fn speech(&self) -> Option<Arc<OwnSpeech>>;
}

/// Whose voice answers, decided in one place and read on every answer. The env (TTS_PROVIDER, TTS_OWN_*) gives the
/// start; the panel's choice (the model shop) overrides it at runtime — nothing writes into the config. Without
/// Yandex («Свой») Alice's voice cannot speak: our engine stands in when there is one.
pub struct VoiceService {
    config: TtsConfig,
    yandex_url: String,
    events: EventBus,
    /// Our engine, when TTS_OWN_URL is set; its voice pair changes at runtime.
    own_speech: Option<Arc<OwnSpeech>>,
    chosen: Mutex<Chosen>,
    /// The station's home server: our voice from the computer first, then the cloud's (TTS_OWN_URL), then Alice's.
    home: OnceLock<Arc<dyn HomeVoice>>,
}

impl VoiceService {
    pub fn new(config: &GatewayConfig, events: EventBus) -> VoiceService {
        let tts = config.tts.clone();
        let own_speech = tts.own.clone().map(|own| Arc::new(OwnSpeech::new(own)));
        let own_voice = tts.own.as_ref().map(|own| VoicePair { model: own.model.clone(), voice: own.voice.clone() });
        VoiceService {
            chosen: Mutex::new(Chosen { provider: tts.provider, own_voice }),
            config: tts,
            yandex_url: config.yandex_url.clone(),
            events,
            own_speech,
            home: OnceLock::new(),
        }
    }

    /// The station with a home server: its voice comes first while it is up.
    pub fn set_home(&self, home: Arc<dyn HomeVoice>) {
        let _ = self.home.set(home);
    }

    /// The engine that speaks our voice now: the home server's while it is up, else ours (TTS_OWN_URL).
    pub fn speaker(&self) -> Option<Arc<OwnSpeech>> {
        self.home.get().and_then(|home| home.speech()).or_else(|| self.own_speech.clone())
    }

    /// The engine a computer speaks with (the talk without a station, src/talk): our voice as chosen — `mixed` is ours
    /// too. Alice's voice comes through the station's session with Yandex and cannot speak there, so with hers chosen,
    /// or none, there is no engine: the caller says so in words.
    pub fn for_computer(&self) -> Option<Arc<OwnSpeech>> {
        match self.chosen_provider() {
            TtsProvider::Own | TtsProvider::Mixed => self.speaker(),
            TtsProvider::Yandex | TtsProvider::None => None,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Chosen> {
        self.chosen.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Our engine, when TTS_OWN_URL is set.
    pub fn own_speech(&self) -> Option<&Arc<OwnSpeech>> {
        self.own_speech.as_ref()
    }

    /// The provider as chosen (the env's, or the panel's).
    pub fn chosen_provider(&self) -> TtsProvider {
        self.lock().provider
    }

    /// The provider in force: Alice's voice needs Yandex, and «Свой» has none. On the station with a home server our
    /// voice goes down the way the plan says — the computer, the cloud, Alice's voice.
    pub fn provider(&self, yandex_allowed: bool) -> TtsProvider {
        let chosen = self.lock().provider;
        let ours = self.speaker().is_some();
        if self.home.get().is_some() && matches!(chosen, TtsProvider::Own | TtsProvider::Mixed) && !ours {
            return if yandex_allowed { TtsProvider::Yandex } else { TtsProvider::None };
        }
        if chosen == TtsProvider::Yandex && !yandex_allowed {
            return if ours { TtsProvider::Mixed } else { TtsProvider::None };
        }
        chosen
    }

    pub fn whose(&self, yandex_allowed: bool) -> Whose {
        if self.provider(yandex_allowed) == TtsProvider::Yandex {
            Whose::Yandex
        } else {
            Whose::Own
        }
    }

    /// The pair our engine is asked for.
    pub fn own_choice(&self) -> Option<VoicePair> {
        self.lock().own_voice.clone()
    }

    /// The env's own pair, before any panel choice.
    pub fn env_own_choice(&self) -> Option<VoicePair> {
        self.config.own.as_ref().map(|own| VoicePair { model: own.model.clone(), voice: own.voice.clone() })
    }

    /// The station leans on a home server for our voice (`set_home`).
    pub fn has_home(&self) -> bool {
        self.home.get().is_some()
    }

    /// The home server's voice from the next answer on — the computer's choice, its voice lab's too — while it is up;
    /// Alice's while it is not (`provider`). `own` stays `own`; otherwise ours speak in the computer's voice and
    /// Yandex's in its own (`mixed`). Only a station with a home server (src/home_server).
    pub fn use_home(&self) -> Result<TtsProvider> {
        if !self.has_home() {
            bail!("no home server: the computer does not give this gateway its voice");
        }
        let provider = {
            let mut chosen = self.lock();
            if chosen.provider != TtsProvider::Own {
                chosen.provider = TtsProvider::Mixed;
            }
            chosen.provider
        };
        self.events.emit(GatewayEvent::VoiceChanged {
            provider: provider.as_str().into(),
            model_id: None,
            voice_id: None,
            actor: None,
        });
        Ok(provider)
    }

    /// Alice's voice from the next answer on.
    pub fn use_yandex(&self) {
        self.lock().provider = TtsProvider::Yandex;
        self.events.emit(GatewayEvent::VoiceChanged {
            provider: TtsProvider::Yandex.as_str().into(),
            model_id: None,
            voice_id: None,
            actor: None,
        });
    }

    /// Our voice from the next answer on. `own` (ours for Yandex's answers too) stays `own`; otherwise ours speak in
    /// ours and Yandex's in its own (`mixed`). A cloned voice brings the route to its own server; without one the
    /// env's server (Silero, Piper) speaks. `ids`: the catalog's model and voice ids, for the event.
    pub fn use_own(
        &self,
        model: &str,
        voice: &str,
        ids: Option<(String, String)>,
        route: Option<Arc<SpeechRoute>>,
    ) -> Result<()> {
        let Some(own_speech) = &self.own_speech else {
            bail!("TTS_OWN_URL is not set: there is no voice server of ours");
        };
        let provider = {
            let mut chosen = self.lock();
            chosen.own_voice = Some(VoicePair { model: model.into(), voice: voice.into() });
            own_speech.choose(model, voice, route);
            if chosen.provider != TtsProvider::Own {
                chosen.provider = TtsProvider::Mixed;
            }
            chosen.provider
        };
        let (model_id, voice_id) = ids.map_or((None, None), |(model_id, voice_id)| (Some(model_id), Some(voice_id)));
        self.events.emit(GatewayEvent::VoiceChanged {
            provider: provider.as_str().into(),
            model_id,
            voice_id,
            actor: None,
        });
        Ok(())
    }

    /// At start: what the settings say, without an event (nothing changed).
    pub fn restore(&self, provider: Option<TtsProvider>, own: Option<(VoicePair, Option<Arc<SpeechRoute>>)>) {
        let mut chosen = self.lock();
        if let (Some((pair, route)), Some(own_speech)) = (own, &self.own_speech) {
            own_speech.choose(&pair.model, &pair.voice, route);
            chosen.own_voice = Some(pair);
        }
        if let Some(provider) = provider.filter(|provider| *provider != chosen.provider) {
            // The env's `own` is kept over a stored `mixed`: the panel only ever wrote yandex or mixed.
            chosen.provider = if provider == TtsProvider::Mixed && self.config.provider == TtsProvider::Own {
                TtsProvider::Own
            } else {
                provider
            };
            tracing::info!(target: "voice", "speaks {} (from the settings)", chosen.provider.as_str());
        }
    }

    /// At start: the voice the settings keep (VOICE_PROVIDER, VOICE_CHOICE), as the models module restored it in
    /// the TS. The stored choice resolves through the catalog; without one (it is not ported yet, or not in this
    /// build) the env's pair speaks. Returns the stored choice it resolved.
    pub fn restore_from_settings(
        &self,
        settings: &SettingsService,
        catalog: Option<&dyn VoiceCatalog>,
    ) -> Option<VoiceChoice> {
        let stored_value = settings.get(&VOICE_CHOICE, None);
        let stored = stored_value.as_ref().zip(catalog).and_then(|(value, catalog)| catalog.resolve_stored(value));
        let lost_own = stored_value
            .as_ref()
            .and_then(|value| value.get("modelId"))
            .and_then(Value::as_str)
            .and_then(|id| id.strip_prefix(OWN_PREFIX));
        if let (Some(own), None) = (lost_own, &stored) {
            tracing::error!(
                target: "voice",
                "own voice {own} is chosen but gone from the voice lab: the env's voice speaks until another is chosen"
            );
        }
        let provider = settings.get(&VOICE_PROVIDER, None).and_then(|provider| TtsProvider::parse(&provider));
        // A stored «ours» without a voice server of ours cannot speak: the env's provider stays. A station with a home
        // server speaks it with the computer's voice (a move with the computer as its server brings the computer's
        // choice, transfer::MOVE_SETS).
        let usable = provider.filter(|provider| {
            matches!(provider, TtsProvider::Yandex | TtsProvider::None) || self.own_speech.is_some() || self.has_home()
        });
        if let (Some(provider), None) = (provider, usable) {
            tracing::warn!(
                target: "voice",
                "stored tts provider {} needs TTS_OWN_URL; the env's {} stays",
                provider.as_str(),
                self.chosen_provider().as_str()
            );
        }
        let own = stored.as_ref().map(|choice| {
            (VoicePair { model: choice.engine.clone(), voice: choice.voice.clone() }, choice.route.clone())
        });
        self.restore(usable, own);
        if let Some(choice) = &stored {
            tracing::info!(target: "voice", "tts: {}/{} speaks (from the settings)", choice.model_id, choice.voice_id);
        }
        stored
    }

    /// For /health: who speaks and through what.
    pub fn describe(&self, yandex_allowed: bool) -> String {
        let provider = self.provider(yandex_allowed);
        let home = self.home.get().and_then(|home| home.speech());
        let own = match (self.own_choice(), &self.own_speech, home) {
            (_, _, Some(home)) => Some(format!("the home server's voice via {}", home.base_url())),
            (Some(pair), Some(own_speech), None) => {
                Some(format!("{}/{} via {}", pair.model, pair.voice, own_speech.base_url()))
            }
            _ => None,
        };
        match provider {
            TtsProvider::Yandex => match &self.config.voice {
                Some(voice) => format!("yandex via {}, voice {voice}", self.yandex_url),
                None => format!("yandex via {}", self.yandex_url),
            },
            TtsProvider::None => "off".into(),
            TtsProvider::Own | TtsProvider::Mixed => {
                let whose = if provider == TtsProvider::Own {
                    "own, Yandex's answers too"
                } else {
                    "mixed: ours own, Yandex's its own"
                };
                format!("{whose}: {}", own.as_deref().unwrap_or("TTS_OWN_URL not set"))
            }
        }
    }

    /// Yandex's synthesis markup is for Yandex's engine only.
    pub fn markup(&self, yandex_allowed: bool) -> bool {
        self.config.markup && self.provider(yandex_allowed) == TtsProvider::Yandex
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::storage::Db;

    fn gateway(pairs: &[(&str, &str)]) -> GatewayConfig {
        Config::load(&Env::from_pairs(pairs.iter().copied())).unwrap().gateway
    }

    fn settings() -> SettingsService {
        SettingsService::new(Arc::new(Db::memory()), EventBus::new())
    }

    struct Catalog;

    impl VoiceCatalog for Catalog {
        fn resolve_stored(&self, stored: &Map<String, Value>) -> Option<VoiceChoice> {
            (stored.get("modelId")?.as_str()? == "piper-ru_RU-irina-medium").then(|| VoiceChoice {
                model_id: "piper-ru_RU-irina-medium".into(),
                voice_id: "piper-ru_RU-irina-medium/irina".into(),
                engine: "piper".into(),
                voice: "irina".into(),
                route: None,
            })
        }
    }

    #[test]
    fn stands_in_for_alices_voice_without_yandex() {
        let own = VoiceService::new(&gateway(&[("TTS_OWN_URL", "http://127.0.0.1:8091/v1")]), EventBus::new());
        assert_eq!(own.provider(true), TtsProvider::Yandex);
        assert_eq!(own.provider(false), TtsProvider::Mixed);
        assert_eq!(own.whose(false), Whose::Own);
        assert_eq!(own.describe(false), "mixed: ours own, Yandex's its own: silero/xenia via http://127.0.0.1:8091/v1");
        let none = VoiceService::new(&gateway(&[("TTS_VOICE", "alice")]), EventBus::new());
        assert_eq!(none.provider(false), TtsProvider::None);
        assert_eq!(none.describe(false), "off");
        assert_eq!(none.describe(true), "yandex via wss://uniproxy.alice.yandex.net/uni.ws, voice alice");
        assert!(none.use_own("piper", "irina", None, None).is_err());
    }

    #[tokio::test]
    async fn chooses_our_voice_and_tells_whose_it_is() {
        let events = EventBus::new();
        let mut heard = events.subscribe();
        let voice = VoiceService::new(&gateway(&[("TTS_OWN_URL", "http://127.0.0.1:8091/v1")]), events);
        voice.use_own("piper", "irina", Some(("piper-x".into(), "piper-x/irina".into())), None).unwrap();
        assert_eq!(voice.chosen_provider(), TtsProvider::Mixed);
        assert_eq!(voice.own_choice(), Some(VoicePair { model: "piper".into(), voice: "irina".into() }));
        assert_eq!(voice.own_speech().unwrap().description(), "piper/irina via http://127.0.0.1:8091/v1/audio/speech");
        let event = heard.recv().await.unwrap();
        assert_eq!(
            *event,
            GatewayEvent::VoiceChanged {
                provider: "mixed".into(),
                model_id: Some("piper-x".into()),
                voice_id: Some("piper-x/irina".into()),
                actor: None
            }
        );
        voice.use_yandex();
        assert_eq!(voice.whose(true), Whose::Yandex);
    }

    #[test]
    fn restores_the_stored_voice_keeping_the_envs_own_over_a_stored_mixed() {
        let settings = settings();
        settings.set(&VOICE_PROVIDER, &"mixed".to_string(), None).unwrap();
        let choice: Map<String, Value> = serde_json::from_value(
            serde_json::json!({"modelId": "piper-ru_RU-irina-medium", "voiceId": "piper-ru_RU-irina-medium/irina"}),
        )
        .unwrap();
        settings.set(&VOICE_CHOICE, &choice, None).unwrap();
        let config = gateway(&[("TTS_OWN_URL", "http://127.0.0.1:8091/v1"), ("TTS_PROVIDER", "own")]);
        let voice = VoiceService::new(&config, EventBus::new());
        let restored = voice.restore_from_settings(&settings, Some(&Catalog)).unwrap();
        assert_eq!(restored.voice, "irina");
        assert_eq!(voice.chosen_provider(), TtsProvider::Own);
        assert_eq!(voice.own_choice(), Some(VoicePair { model: "piper".into(), voice: "irina".into() }));
        // Markup is for Yandex's engine only.
        assert!(!voice.markup(true));

        // Without the catalog only the provider comes back; without a voice server of ours, not even that.
        let plain = VoiceService::new(&gateway(&[("TTS_OWN_URL", "http://127.0.0.1:8091/v1")]), EventBus::new());
        assert!(plain.restore_from_settings(&settings, None).is_none());
        assert_eq!(plain.chosen_provider(), TtsProvider::Mixed);
        assert_eq!(plain.own_choice(), plain.env_own_choice());
        let alone = VoiceService::new(&gateway(&[]), EventBus::new());
        alone.restore_from_settings(&settings, None);
        assert_eq!(alone.chosen_provider(), TtsProvider::Yandex);
    }

    /// The computer's voice as the station has it: the station's own voice server is not set (its .env has no
    /// TTS_OWN_URL), its home server is.
    struct Home(Option<Arc<OwnSpeech>>);

    impl HomeVoice for Home {
        fn speech(&self) -> Option<Arc<OwnSpeech>> {
            self.0.clone()
        }
    }

    fn home_speech() -> Arc<OwnSpeech> {
        let config = gateway(&[("TTS_OWN_URL", "http://192.0.2.10:7780/home/v1/tts")]);
        Arc::new(OwnSpeech::new(config.tts.own.unwrap()))
    }

    /// Live, 10.11: the computer speaks «mixed» and moved the gateway to the station with itself as its server; the
    /// station answered in Alice's voice in «Шлюз» — its own provider was «yandex» and a stored «mixed» needed a voice
    /// server of its own. A stored «mixed» (the move brings it) speaks the computer's voice while it is up, Alice's when
    /// it is away; the station's «Голос» window switches it either way.
    #[tokio::test]
    async fn a_station_with_a_home_server_speaks_the_computers_voice_in_gateway_mode_as_the_move_set_it() {
        let settings = settings();
        settings.set(&VOICE_PROVIDER, &"mixed".to_string(), None).unwrap();
        let station = VoiceService::new(&gateway(&[]), EventBus::new());
        assert!(station.use_home().is_err(), "no home server: nothing to speak with");
        station.set_home(Arc::new(Home(Some(home_speech()))));
        station.restore_from_settings(&settings, None);
        assert_eq!(station.chosen_provider(), TtsProvider::Mixed);
        assert_eq!(station.provider(true), TtsProvider::Mixed, "«Шлюз»: our answers in the computer's voice");
        assert!(station.describe(true).contains("the home server's voice"), "{}", station.describe(true));

        let away = VoiceService::new(&gateway(&[]), EventBus::new());
        away.set_home(Arc::new(Home(None)));
        away.restore_from_settings(&settings, None);
        assert_eq!((away.chosen_provider(), away.provider(true)), (TtsProvider::Mixed, TtsProvider::Yandex));

        // «Алиса» and back in the station's «Голос» window.
        station.use_yandex();
        assert_eq!(station.provider(true), TtsProvider::Yandex);
        assert_eq!(station.use_home().unwrap(), TtsProvider::Mixed);
        assert_eq!(station.provider(true), TtsProvider::Mixed);
    }

    #[test]
    fn the_provider_a_computer_speaks_with_is_its_stored_choice_when_it_can_speak_it() {
        let own = gateway(&[("TTS_OWN_URL", "http://127.0.0.1:8091/v1")]).tts;
        let env_own = gateway(&[("TTS_OWN_URL", "http://127.0.0.1:8091/v1"), ("TTS_PROVIDER", "own")]).tts;
        let none = gateway(&[]).tts;
        assert_eq!(provider_at_start(&own, Some(TtsProvider::Mixed)), TtsProvider::Mixed);
        assert_eq!(provider_at_start(&own, None), TtsProvider::Yandex, "the env's");
        assert_eq!(provider_at_start(&env_own, Some(TtsProvider::Mixed)), TtsProvider::Own, "the env's own kept");
        assert_eq!(provider_at_start(&env_own, Some(TtsProvider::Yandex)), TtsProvider::Yandex);
        assert_eq!(provider_at_start(&none, Some(TtsProvider::Mixed)), TtsProvider::Yandex, "no voice server of ours");
    }
}
