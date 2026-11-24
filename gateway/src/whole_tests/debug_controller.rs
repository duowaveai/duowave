//! The tests of `gateway/debug/debug_controller.rs` that need the whole gateway (`App`): its crate does not have it,
//! the program does.

use std::net::SocketAddr;

use axum::body::Body as HttpBody;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::app::App;
use crate::config::{Config, Env};

/// Express read no JSON body over 100 KB, and stopped before any guard: nobody signed in makes the gateway read
/// and parse more than that.
#[tokio::test]
async fn reads_no_bigger_body_than_express_did_before_the_access_checks() {
    let env = Env::from_pairs([("STATE_DIR", ""), ("DEBUG_API_ENABLED", "true")]);
    let app = App::build(Config::for_tests(&env)).unwrap();
    let status = |path: &str, body: String| {
        let mut request = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(HttpBody::from(body))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5000))));
        let app = app.clone();
        async move { crate::app::router(app).oneshot(request).await.unwrap().status() }
    };
    let big = format!("[{}0]", "0,".repeat(60 * 1024));
    for path in ["/api/v1/station/mode", "/api/v1/station/routes", "/debug/sessions/latest/say"] {
        assert_eq!(status(path, big.clone()).await, StatusCode::PAYLOAD_TOO_LARGE, "{path}");
    }
    assert_eq!(status("/api/v1/station/mode", "{}".into()).await, StatusCode::UNAUTHORIZED, "a small one is read");
}
