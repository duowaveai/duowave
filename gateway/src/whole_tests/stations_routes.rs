//! The tests of `stations/routes.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

use std::net::SocketAddr;

use axum::body::Body as AxumBody;
use axum::extract::ConnectInfo;
use axum::http::{header, HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use crate::app::{router, App};
use crate::config::{Config, Env};

/// The whole stack on an in-memory gateway: the Host check, the envelope, the routes.
fn gateway() -> axum::Router {
    let config = Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]));
    router(App::build(config).expect("the app builds in memory"))
}

/// A request from this computer (loopback), so a guarded route is reached rather than refused by zone.
async fn call(method: &str, uri: &str, host: Option<&str>) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(host) = host {
        builder = builder.header(header::HOST, host);
    }
    let mut request = builder.body(AxumBody::empty()).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5000))));
    let response = gateway().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn serves_the_connect_script_as_plain_text_without_the_envelope() {
    let (status, headers, body) = call("GET", "/station/connect.sh", Some("192.168.1.50:3000")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(header::CONTENT_TYPE).unwrap(), "text/plain; charset=utf-8");
    assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    // The gateway address is the one the station dialed (the Host), and there is no envelope around it.
    assert!(body.contains("voice.sh point 'ws://192.168.1.50:3000/uni.ws' ''"), "{body}");
    assert!(!body.contains("\"code\""), "a script, not the JSON envelope");
}

#[tokio::test]
async fn serves_the_restore_script_back_to_the_firmwares_own_config() {
    let (status, _headers, body) = call("GET", "/station/restore.sh", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("voice.sh drop") && body.contains("reboot"), "{body}");
    let (status, _headers, body) = call("GET", "/station/uninstall.sh", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("voice.sh\" drop") && body.contains("rm -rf \"$DIR\""), "{body}");
}

#[tokio::test]
async fn the_stations_list_needs_signing_in() {
    // Loopback is allowed by zone, but nobody is signed in: 401, in the error envelope.
    let (status, _headers, body) = call("GET", "/api/v1/stations", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["code"], 401);
}
