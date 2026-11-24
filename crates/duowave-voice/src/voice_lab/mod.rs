//! The owner's own cloned voices (src/voice-lab, clone-server/). The lab itself — engines installed by one button,
//! takes recorded in the panel, imprints taken from them, and the servers that speak in them — is the computer's floor's
//! (crates/duowave-pc/src/voice_lab, under this module's name): the station's build has no Python engines beside it.
//! Here: the station's answer, `/api/v1/voice-lab` is not in this build (501) — the computer's lab stands in for it.
//!
//! src/voice-lab is the Node original, removed from the tree: git tag `node-gateway-last`.

pub use station::router;

/// The station's build: no voice lab — every route under it answers 501 in the envelope.
pub mod station {
    use axum::http::StatusCode;
    use axum::routing::any;
    use axum::Router;

    use crate::platform::http::{ApiError, RouteState};

    pub fn not_in_this_build_text() -> String {
        crate::tr!("Голосовой лаборатории нет в этой сборке: свои голоса делаются в DuoWave на компьютере.")
    }

    pub fn router<S: RouteState>() -> Router<S> {
        Router::new()
            .route("/api/v1/voice-lab", any(not_in_this_build))
            .route("/api/v1/voice-lab/{*rest}", any(not_in_this_build))
    }

    async fn not_in_this_build() -> ApiError {
        ApiError::new(StatusCode::NOT_IMPLEMENTED, not_in_this_build_text())
    }
}
