//! The panel's model shop (src/models): the catalog of voices and recognition models, downloads and activation; the
//! owner's own voices come from the voice lab. Local models are for a PC: the shop is the computer's floor's
//! (crates/duowave-pc/src/models, under this module's name). Here, in every build: the catalog's tables — the persona
//! reads its character cards there — and the station's answer that its models are not in it (501).

pub mod catalog_db;

pub use catalog_db::{AsrModelRecord, CatalogDb, CharacterRecord, VoiceEntry, VoiceModelRecord};
pub mod station {
    use axum::http::StatusCode;
    use axum::routing::any;
    use axum::Router;

    use crate::platform::http::ApiError;

    pub fn not_in_this_build_text() -> String {
        crate::tr!("Свои модели — только в DuoWave для компьютера: в этой сборке (для колонки) их нет.")
    }

    /// `/api/v1/models` and everything under it: 501 in the envelope — but the computer's voice for the station's
    /// «Голос» (src/helpers/home_voice.rs, the gateway adds it).
    pub fn router<S: crate::platform::http::RouteState>() -> Router<S> {
        Router::new()
            .route("/api/v1/models", any(not_in_this_build))
            .route("/api/v1/models/{*rest}", any(not_in_this_build))
    }

    async fn not_in_this_build() -> ApiError {
        ApiError::new(StatusCode::NOT_IMPLEMENTED, not_in_this_build_text())
    }
}

pub use station::router;
