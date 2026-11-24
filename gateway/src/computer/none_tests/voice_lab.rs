//! The tests of `voice_lab/mod.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

use axum::body::Body;
use axum::http::StatusCode;
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::app::App;
use crate::config::{Config, Env};
use crate::voice_lab::station::*;

#[tokio::test]
async fn says_the_voice_lab_is_not_part_of_the_stations_build() {
    let app = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
    let router = router().with_state(app).layer(axum::middleware::from_fn(crate::platform::http::api::error_bodies));
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
