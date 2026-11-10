//! The panel's model shop (src/models): the catalog of voices and recognition models, downloads and activation; the
//! owner's own voices come from the voice lab. Local models are for a PC: the shop is in the `pc` build. The
//! catalog's tables are in every build — the persona reads its character cards there — and the station's build
//! answers /api/v1/models that its models are not in it (501).

pub mod catalog_db;
pub mod home_voice;
#[cfg(feature = "pc")]
pub mod model_files;
#[cfg(feature = "pc")]
pub mod model_servers;
#[cfg(feature = "pc")]
pub mod models_controller;
#[cfg(feature = "pc")]
pub mod models_service;

pub use catalog_db::{AsrModelRecord, CatalogDb, CharacterRecord, VoiceEntry, VoiceModelRecord};
#[cfg(feature = "pc")]
pub use model_servers::{server_env, AsrStatus, ModelServers, ServerName, TtsLoaded, TtsStatus};
#[cfg(feature = "pc")]
pub use models_controller::router;
#[cfg(feature = "pc")]
pub use models_service::{
    ChosenVoice, ModelsLinks, ModelsService, OwnRoute, OwnVoiceCard, OwnVoices, PreparedVoice, RestoredVoice, Voice,
    VoiceIds, VoicePair, Whose,
};

#[cfg(not(feature = "pc"))]
mod station {
    use std::sync::Arc;

    use axum::http::StatusCode;
    use axum::routing::any;
    use axum::Router;

    use crate::app::App;
    use crate::platform::http::ApiError;

    pub const NOT_IN_THIS_BUILD: &str =
        "Свои модели — только в сборке шлюза для компьютера: в этой сборке (для колонки) их нет.";

    /// `/api/v1/models` and everything under it: 501 in the envelope — but the computer's voice for the station's
    /// «Голос» (home_voice.rs).
    pub fn router() -> Router<Arc<App>> {
        Router::new()
            .route("/api/v1/models", any(not_in_this_build))
            .route("/api/v1/models/{*rest}", any(not_in_this_build))
            .merge(super::home_voice::routes())
    }

    async fn not_in_this_build() -> ApiError {
        ApiError::new(StatusCode::NOT_IMPLEMENTED, NOT_IN_THIS_BUILD)
    }
}

#[cfg(not(feature = "pc"))]
pub use station::router;

#[cfg(all(test, not(feature = "pc")))]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::app::{router, App};
    use crate::config::{Config, Env};

    #[tokio::test]
    async fn the_station_build_says_its_models_are_not_in_it() {
        let app = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
        for (method, uri) in [("GET", "/api/v1/models"), ("POST", "/api/v1/models/tts/silero/install")] {
            let request = Request::builder().method(method).uri(uri).body(Body::empty()).unwrap();
            let response = router(app.clone()).oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED, "{method} {uri}");
            let body: serde_json::Value =
                serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
            assert_eq!(body["code"], 501);
            assert_eq!(body["message"], super::station::NOT_IN_THIS_BUILD);
            assert_eq!(body["path"], uri);
        }
        // The station's «Голос» while the computer gives it its voice is this build's (home_voice.rs).
        let request = Request::builder().uri("/api/v1/models/home-voice").body(Body::empty()).unwrap();
        let response = router(app.clone()).oneshot(request).await.unwrap();
        assert_ne!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }
}
