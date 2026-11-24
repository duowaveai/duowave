//! The tests of `models/mod.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

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
        assert_eq!(body["message"], crate::models::station::not_in_this_build_text());
        assert_eq!(body["path"], uri);
    }
    // The station's «Голос» while the computer gives it its voice is this build's (home_voice.rs).
    let request = Request::builder().uri("/api/v1/models/home-voice").body(Body::empty()).unwrap();
    let response = router(app.clone()).oneshot(request).await.unwrap();
    assert_ne!(response.status(), StatusCode::NOT_IMPLEMENTED);
}
