//! The voice that answers (speech's VoiceService), the model shop (src/models) and the owner's own voices
//! (src/voice_lab), as models.module.ts and voice-lab.module.ts linked them: the shop chooses through the voice, an own
//! voice's route goes from the voice lab to the voice through the shop, the voice lab asks the voice which voice is
//! chosen, and a stored choice resolves through the shop's catalog when the gateway is built.
//!
//! The `.ts` files named here are the Node gateway's, removed from the tree: git tag `node-gateway-last`.

use std::sync::Arc;

use futures::future::{BoxFuture, FutureExt};
use serde_json::{Map, Value};

use crate::config::TtsProvider;
use crate::core::events::Actor;
use crate::models::models_service::{EngineState, LabJob};
use crate::models::{
    ModelsService, OwnRoute, OwnVoiceCard, OwnVoices, PreparedVoice, RestoredVoice, Voice, VoiceIds, VoicePair, Whose,
};
use crate::platform::http::ApiError;
use crate::settings::definitions::VoiceRecipe;
use crate::speech::tts::own_speech::SpeechRoute;
use crate::speech::voice_service::{self, VoiceCatalog, VoiceChoice};
use crate::speech::VoiceService;
use crate::voice_lab::clone_engines::spec;
use crate::voice_lab::jobs::JobRunState;
use crate::voice_lab::{OwnSpeech, VoiceLabService};

/// An own voice's route as the shop carries it: speech's SpeechRoute inside the shop's `OwnRoute`.
pub fn own_route(route: Arc<SpeechRoute>) -> OwnRoute {
    route
}

/// The route back out of the shop's `OwnRoute`; anything else is no route (the env's server speaks).
pub fn speech_route(route: OwnRoute) -> Option<Arc<SpeechRoute>> {
    route.downcast::<SpeechRoute>().ok()
}

/// The shop's `Voice` over speech's VoiceService: whose voice speaks, and which of ours.
pub struct ShopVoice(pub Arc<VoiceService>);

impl Voice for ShopVoice {
    fn has_own_speech(&self) -> bool {
        self.0.own_speech().is_some()
    }

    fn chosen_provider(&self) -> TtsProvider {
        self.0.chosen_provider()
    }

    fn whose(&self, yandex_allowed: bool) -> Whose {
        match self.0.whose(yandex_allowed) {
            voice_service::Whose::Yandex => Whose::Yandex,
            voice_service::Whose::Own => Whose::Own,
        }
    }

    fn own_choice(&self) -> Option<VoicePair> {
        self.0.own_choice().map(|pair| VoicePair { model: pair.model, voice: pair.voice })
    }

    fn use_yandex(&self) {
        self.0.use_yandex();
    }

    fn use_own(&self, own: VoicePair, ids: VoiceIds, route: Option<OwnRoute>) -> Result<(), ApiError> {
        // Only without TTS_OWN_URL, which the shop checks first: the Node gateway's Error, a 500.
        self.0
            .use_own(&own.model, &own.voice, Some((ids.model_id, ids.voice_id)), route.and_then(speech_route))
            .map_err(ApiError::internal)
    }

    fn restore(&self, provider: Option<TtsProvider>, own: Option<VoicePair>, route: Option<OwnRoute>) {
        let own = own.map(|pair| {
            (voice_service::VoicePair { model: pair.model, voice: pair.voice }, route.and_then(speech_route))
        });
        self.0.restore(provider, own);
    }
}

/// The shop's `OwnVoices` over the voice lab: the cards, a chosen voice's server and route, their memory given back.
pub struct LabVoices(pub Arc<VoiceLabService>);

impl OwnVoices for LabVoices {
    fn voices_for_cards(&self) -> BoxFuture<'_, Vec<OwnVoiceCard>> {
        async move {
            self.0
                .voices_for_cards()
                .await
                .into_iter()
                .map(|card| {
                    let engine = spec(card.voice.engine);
                    OwnVoiceCard {
                        id: card.voice.id,
                        name: card.voice.name,
                        engine: card.voice.engine.as_str().into(),
                        engine_title: engine.title.into(),
                        engine_license: engine.license.into(),
                        created_at: card.voice.created_at,
                        ref_seconds: card.voice.ref_seconds,
                        samples: card.voice.samples.len(),
                        ready: card.ready,
                        serving: card.serving,
                        reimprint: (card.voice.has_ref && card.voice.words.is_some()) || card.voice.takes > 0,
                        origin: card.voice.origin,
                    }
                })
                .collect()
        }
        .boxed()
    }

    fn prepare(&self, id: String, anyway: bool) -> BoxFuture<'_, Result<PreparedVoice, ApiError>> {
        async move {
            let prepared = self.0.prepare(&id, anyway).await?;
            Ok(PreparedVoice {
                engine: prepared.engine.as_str().into(),
                route: own_route(prepared.route),
                name: prepared.name,
            })
        }
        .boxed()
    }

    fn restore(&self, id: &str) -> Option<RestoredVoice> {
        let restored = self.0.restore(id)?;
        Some(RestoredVoice { engine: restored.engine.as_str().into(), route: own_route(restored.route) })
    }

    fn release(&self) {
        let lab = self.0.clone();
        tokio::spawn(async move { lab.release().await });
    }

    fn engines(&self) -> BoxFuture<'_, Vec<EngineState>> {
        async move {
            let mut engines = Vec::new();
            for card in self.0.engine_cards().await {
                let (download_mb, disk_mb) = self.0.engine_size(card.id).await;
                engines.push(EngineState {
                    id: card.id.as_str().into(),
                    title: card.title.into(),
                    status: serde_json::to_value(card.status)
                        .ok()
                        .and_then(|status| status.as_str().map(str::to_string))
                        .unwrap_or_default(),
                    reason: card.reason,
                    download_mb,
                    disk_mb,
                });
            }
            engines
        }
        .boxed()
    }

    fn install_engine(&self, engine: String, actor: Option<Actor>) -> BoxFuture<'_, Result<(), ApiError>> {
        async move { self.0.install(&engine, actor).await.map(|_| ()) }.boxed()
    }

    fn imprint(&self, recipe: VoiceRecipe, anyway: bool, actor: Option<Actor>) -> BoxFuture<'_, Result<(), ApiError>> {
        async move {
            let mut meta = Map::new();
            for (key, value) in [
                ("origin", "package"),
                ("package", recipe.package.as_str()),
                ("packageVersion", recipe.version.as_str()),
                ("character", recipe.character.as_str()),
                ("taken", "reference"),
            ] {
                meta.insert(key.into(), Value::String(value.into()));
            }
            let reference = std::path::PathBuf::from(&recipe.reference);
            self.0
                .imprint_reference(
                    &recipe.voice,
                    &recipe.engine,
                    &reference,
                    &recipe.text,
                    &recipe.name,
                    meta,
                    anyway,
                    actor,
                )
                .await
                .map(|_| ())
        }
        .boxed()
    }

    fn job(&self) -> Option<LabJob> {
        self.0.job().map(|job| LabJob {
            kind: job.kind.as_str().into(),
            engine: job.engine.clone(),
            running: job.state == JobRunState::Running,
            percent: job.percent,
            failed: job.error.filter(|_| job.state != JobRunState::Done),
        })
    }

    fn voice_engine(&self, id: &str) -> Option<String> {
        self.0.store.voice(id).map(|voice| voice.engine.as_str().to_string())
    }
}

/// The voice lab's `OwnSpeech` over the voice: the pair our engine is asked for now.
pub struct LabSpeech(pub Arc<VoiceService>);

impl OwnSpeech for LabSpeech {
    fn own_choice(&self) -> Option<(String, String)> {
        self.0.own_choice().map(|pair| (pair.model, pair.voice))
    }
}

/// Speech's `VoiceCatalog` over the shop: a stored choice resolves against the catalog in the database and the voice
/// lab's own voices (an own voice's server starts in the background). The shop's own start resolves it again once
/// it has read the registry.
impl VoiceCatalog for ModelsService {
    fn resolve_stored(&self, stored: &Map<String, Value>) -> Option<VoiceChoice> {
        let (chosen, route) = self.stored_voice(stored)?;
        Some(VoiceChoice {
            model_id: chosen.model_id,
            voice_id: chosen.voice_id,
            engine: chosen.engine,
            voice: chosen.voice,
            route: route.and_then(speech_route),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::config::{Config, Env};
    use crate::core::events::EventBus;
    use crate::models::{VoiceEntry, VoiceModelRecord};
    use crate::settings::definitions::{VOICE_CHOICE, VOICE_PROVIDER};

    fn voice(pairs: &[(&str, &str)]) -> Arc<VoiceService> {
        let config = Config::for_tests(&Env::from_pairs(pairs.iter().copied()));
        Arc::new(VoiceService::new(&config.gateway, EventBus::new()))
    }

    fn irina() -> VoiceModelRecord {
        VoiceModelRecord {
            id: "piper-ru_RU-irina-medium".into(),
            engine: "piper".into(),
            title: "Ирина".into(),
            description: "Piper".into(),
            badge: None,
            license: None,
            locale: Some("ru_RU".into()),
            quality: Some("medium".into()),
            size: None,
            url: None,
            file: Some("ru_RU-irina-medium.onnx".into()),
            source: "registry".into(),
            voices: vec![VoiceEntry { name: "ru_RU-irina-medium".into(), short: Some("irina".into()) }],
        }
    }

    #[test]
    fn carries_speechs_route_through_the_shop_and_nothing_else() {
        let route = Arc::new(SpeechRoute::limited("http://127.0.0.1:9/v1", 30_000, 1, None));
        let back = speech_route(own_route(route.clone())).expect("the same route");
        assert!(Arc::ptr_eq(&back, &route), "one route an engine: one queue");
        let other: OwnRoute = Arc::new("not a route");
        assert!(speech_route(other).is_none());
    }

    #[tokio::test]
    async fn the_shop_chooses_through_the_voice_and_an_own_voice_speaks_through_its_route() {
        let speech = voice(&[("TTS_OWN_URL", "http://127.0.0.1:8091/v1")]);
        let shop = ShopVoice(speech.clone());
        assert!(shop.has_own_speech());
        assert_eq!(shop.whose(false), Whose::Own, "«Свой» has our voice stand in for Alice's");
        assert_eq!(shop.whose(true), Whose::Yandex);
        let route = Arc::new(SpeechRoute::limited("http://127.0.0.1:8093/v1", 30_000, 1, None));
        let ids = VoiceIds { model_id: "own:v-1".into(), voice_id: "own:v-1".into() };
        shop.use_own(VoicePair { model: "omnivoice".into(), voice: "v-1".into() }, ids, Some(own_route(route)))
            .unwrap();
        assert_eq!(shop.own_choice(), Some(VoicePair { model: "omnivoice".into(), voice: "v-1".into() }));
        assert_eq!(shop.chosen_provider(), TtsProvider::Mixed);
        assert_eq!(speech.own_speech().unwrap().base_url(), "http://127.0.0.1:8093/v1", "its own server");
        assert_eq!(LabSpeech(speech.clone()).own_choice(), Some(("omnivoice".into(), "v-1".into())));
        shop.restore(None, Some(VoicePair { model: "piper".into(), voice: "irina".into() }), None);
        assert_eq!(speech.own_speech().unwrap().base_url(), "http://127.0.0.1:8091/v1", "the env's server again");
        shop.use_yandex();
        assert_eq!(shop.whose(true), Whose::Yandex);

        let without = ShopVoice(voice(&[]));
        assert!(!without.has_own_speech());
        let refused = without.use_own(
            VoicePair { model: "piper".into(), voice: "irina".into() },
            VoiceIds { model_id: "x".into(), voice_id: "x/irina".into() },
            None,
        );
        assert_eq!(refused.unwrap_err().status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// The gateway built over a database whose settings keep a catalog voice: it speaks from the build on, before
    /// the shop's start.
    #[tokio::test]
    async fn the_gateway_speaks_the_stored_voice_from_its_build_through_the_shops_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::from_pairs([
            ("STATE_DIR", dir.path().to_str().unwrap()),
            ("TTS_OWN_URL", "http://127.0.0.1:8091/v1"),
            ("TTS_MODELS_DIR", dir.path().join("voices").to_str().unwrap()),
        ]);
        {
            let first = App::build(Config::for_tests(&env)).unwrap();
            assert_eq!(first.voice.chosen_provider(), TtsProvider::Yandex);
            first.models.catalog().save_voice_models(&[irina()]).unwrap();
            let choice: Map<String, Value> = serde_json::from_value(serde_json::json!({
                "modelId": "piper-ru_RU-irina-medium",
                "voiceId": "piper-ru_RU-irina-medium/ru_RU-irina-medium",
            }))
            .unwrap();
            first.settings.set(&VOICE_CHOICE, &choice, None).unwrap();
            first.settings.set(&VOICE_PROVIDER, &"mixed".to_string(), None).unwrap();
        }
        let app = App::build(Config::for_tests(&env)).unwrap();
        assert_eq!(app.voice.chosen_provider(), TtsProvider::Mixed);
        assert_eq!(
            app.voice.own_choice(),
            Some(voice_service::VoicePair { model: "piper".into(), voice: "ru_RU-irina-medium".into() })
        );
        assert!(app.voice.describe(true).contains("piper/ru_RU-irina-medium via http://127.0.0.1:8091/v1"));
        // The shop's own resolution is the same voice.
        let stored = app.settings.get(&VOICE_CHOICE, None).unwrap();
        let (chosen, route) = app.models.stored_voice(&stored).unwrap();
        assert_eq!(chosen.engine, "piper");
        assert!(route.is_none(), "a catalog voice speaks through the env's server");
        let gone: Map<String, Value> = serde_json::from_value(serde_json::json!({"modelId": "own:v-gone"})).unwrap();
        assert!(app.models.resolve_stored(&gone).is_none(), "an own voice the lab no longer has");
    }
}
