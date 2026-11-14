//! The owner's own cloned voices (src/voice-lab, clone-server/): engines installed by one button, takes recorded in
//! the panel, imprints taken from them, and the servers that speak in them. A PC's part, behind the `pc` feature: the
//! station's build has no Python engines beside it, and its `/api/v1/voice-lab` says so (501).
//!
//! src/voice-lab is the Node original, removed from the tree: git tag `node-gateway-last`.

#[cfg(feature = "pc")]
pub mod clone_engines;
#[cfg(feature = "pc")]
pub mod clone_servers;
#[cfg(feature = "pc")]
pub mod controller;
#[cfg(feature = "pc")]
pub mod engine_installer;
#[cfg(feature = "pc")]
pub mod jobs;
#[cfg(feature = "pc")]
pub mod js;
#[cfg(feature = "pc")]
pub mod memory_policy;
#[cfg(feature = "pc")]
pub mod passages;
#[cfg(feature = "pc")]
pub mod platform;
#[cfg(feature = "pc")]
pub mod service;
#[cfg(feature = "pc")]
pub mod system_memory;
#[cfg(feature = "pc")]
pub mod voice_store;

#[cfg(feature = "pc")]
pub use controller::router;
#[cfg(feature = "pc")]
pub use service::{OwnSpeech, OwnVoiceCard, PreparedVoice, RestoredVoice, VoiceLabService, VoiceLabView};

#[cfg(not(feature = "pc"))]
pub use station::router;

/// The station's build: no voice lab — every route under it answers 501 in the envelope.
#[cfg(not(feature = "pc"))]
mod station {
    use std::sync::Arc;

    use axum::http::StatusCode;
    use axum::routing::any;
    use axum::Router;

    use crate::app::App;
    use crate::platform::http::ApiError;

    pub fn not_in_this_build_text() -> String {
        crate::tr!("Голосовой лаборатории нет в этой сборке: свои голоса делаются в шлюзе на компьютере.")
    }

    pub fn router() -> Router<Arc<App>> {
        Router::new()
            .route("/api/v1/voice-lab", any(not_in_this_build))
            .route("/api/v1/voice-lab/{*rest}", any(not_in_this_build))
    }

    async fn not_in_this_build() -> ApiError {
        ApiError::new(StatusCode::NOT_IMPLEMENTED, not_in_this_build_text())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::config::{Config, Env};
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        #[tokio::test]
        async fn says_the_voice_lab_is_not_part_of_the_stations_build() {
            let app = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
            let router =
                router().with_state(app).layer(axum::middleware::from_fn(crate::platform::http::api::error_bodies));
            for (method, uri) in [("GET", "/api/v1/voice-lab"), ("POST", "/api/v1/voice-lab/draft/takes?passage=1")] {
                let request = axum::http::Request::builder().method(method).uri(uri).body(Body::empty()).unwrap();
                let response = router.clone().oneshot(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
                let body: serde_json::Value =
                    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
                assert_eq!(body["code"], 501);
                assert_eq!(body["message"], not_in_this_build_text());
            }
        }
    }
}
