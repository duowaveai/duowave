//! What /health reads of the modules, as health-status.service.ts read them: the speech setup, the dialog and its
//! brain, whose voice speaks, the skills (asked in gateway mode only). The station sessions and the station's
//! presence come from the station link's own source.
//!
//! health-status.service.ts is the Node gateway's, removed from the tree: git tag `node-gateway-last`.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::dialog::DialogService;
use crate::health_status::HealthSources;
use crate::skills::SkillsService;
use crate::speech::{SpeechService, VoiceService};

pub struct ModuleHealth {
    pub speech: Arc<SpeechService>,
    pub dialog: Arc<DialogService>,
    pub voice: Arc<VoiceService>,
    pub skills: Arc<SkillsService>,
}

impl HealthSources for ModuleHealth {
    fn speech(&self) -> Option<Value> {
        serde_json::to_value(self.speech.status()).ok()
    }

    fn dialog(&self) -> Option<String> {
        Some(self.dialog.description())
    }

    fn brain(&self) -> Option<Value> {
        serde_json::to_value(self.dialog.brain()).ok()
    }

    fn tts(&self, yandex_allowed: bool) -> Option<String> {
        Some(self.voice.describe(yandex_allowed))
    }

    /// Who serves each capability, what is ours (and set up), and the counters since the start.
    fn skills(&self) -> Option<Value> {
        Some(json!({
            "routes": self.skills.router.routes().describe(),
            "implemented": self.skills.implemented(),
            "served": self.skills.stats.served_by_capability(),
            "unsupported": self.skills.stats.unsupported_by_capability(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::app::{router, App};
    use crate::config::{Config, Env};
    use crate::skills::capabilities::CapabilityName;
    use crate::skills::skill_stats::Executor;

    async fn health(app: &std::sync::Arc<App>) -> serde_json::Value {
        let request = Request::builder().uri("/health").body(Body::empty()).unwrap();
        let mut request = request;
        // From this computer: the details are shown.
        request.extensions_mut().insert(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 9))));
        let response = router(app.clone()).oneshot(request).await.unwrap();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["message"].clone()
    }

    #[tokio::test]
    async fn health_reads_the_speech_the_dialog_the_voice_and_the_skills() {
        let env = Env::from_pairs([
            ("STATE_DIR", ""),
            ("DIALOG_PROVIDER", "echo"),
            ("ASR_PROVIDER", "none"),
            ("VAD_ENGINE", "energy"),
            ("UNIPROXY_MODE", "gateway"),
            ("TTS_VOICE", "alice"),
        ]);
        let app = App::build(Config::for_tests(&env)).unwrap();
        app.skills.stats.count(CapabilityName::Weather, Executor::Own);
        let status = health(&app).await;
        assert_eq!(status["speech"], serde_json::to_value(app.speech.status()).unwrap());
        assert_eq!(status["speech"]["asr"], "off");
        assert_eq!(status["dialog"], app.dialog.description());
        assert_eq!(status["brain"]["provider"], "echo");
        assert_eq!(status["tts"], "yandex via wss://uniproxy.alice.yandex.net/uni.ws, voice alice");
        let skills = &status["skills"];
        assert_eq!(skills["routes"]["weather"], app.skills.router.routes().describe()[&CapabilityName::Weather]);
        assert_eq!(skills["implemented"]["weather"], true);
        assert_eq!(skills["served"]["weather"], serde_json::json!({"own": 1, "yandex": 0, "unsupported": 0}));
        assert_eq!(skills["unsupported"], serde_json::json!({}));
        // The station link's own source (src/gateway/uniproxy): no station connected.
        assert_eq!(status["activeSessions"], 0, "the station link's own source");
    }
}
