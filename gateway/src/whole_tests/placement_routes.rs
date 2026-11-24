//! The tests of `placement/routes.rs` that need the whole gateway (`App`): its crate does not have it, the program
//! does.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body as HttpBody;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Map, Value};
use tower::ServiceExt;

use crate::app::App;
use crate::config::{Config, Env};
use crate::placement::peer::{HELPER_PERSON, HELPER_ROLE};
use crate::placement::transfer::{self};

pub(crate) const PAIR: &str = "0123456789abcdef0123456789abcdef";

/// The station's gateway (the computer's build playing it); its .env with these.
pub(crate) fn station(vars: &[(&str, &str)]) -> Arc<App> {
    let base = [("STATE_DIR", ""), ("DUOWAVE_ROLE", "station")];
    let env = Env::from_pairs(base.into_iter().chain(vars.iter().copied()));
    App::build(Config::for_tests(&env)).unwrap()
}

/// A request from the home network with these headers.
pub(crate) async fn send(
    app: &Arc<App>,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri).header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, axum::http::HeaderValue::from_bytes(value.as_bytes()).unwrap());
    }
    let mut request = request.body(HttpBody::from(body.to_string())).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new("192.168.1.20".parse().unwrap(), 5000)));
    let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// Stage 5: a second station with DuoWave of its own joins the home as the centre's ear by the code it says
/// itself — typed on the computer that is the home's Turbo; it then follows the centre (`/api/v1/home/ear`).
// The ear's voice goes to the centre through the Station's adapter (gateway/uniproxy/ear_bridge.rs).

#[tokio::test]
async fn the_station_gives_its_pair_key_only_to_its_admin_signed_in_with_the_password() {
    let app = station(&[]);
    let admin = app.access.auth.setup("admin-password-1", "Мама", "test").await.unwrap().unwrap().token;
    let admin = format!("Bearer {admin}");
    let pair_uri = "/api/v1/placement/pair";
    let (status, body) = send(&app, "POST", pair_uri, &[("authorization", admin.as_str())], json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let key = body["message"]["key"].as_str().unwrap().to_string();
    assert_eq!(app.placement.peer_key(), Some(key.as_str()), "made here and kept");

    // A key with every right is not a person; the pair's key itself (the computer's pass) does not take it either.
    let grants = Map::from_iter([("name".into(), json!("Скрипт")), ("grants".into(), json!(["*"]))]);
    let script = format!("Bearer {}", app.access.keys.create(&grants, None).unwrap().secret);
    let (status, _) = send(&app, "POST", pair_uri, &[("authorization", script.as_str())], json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let pair = format!("Bearer {key}");
    let through = [("authorization", pair.as_str()), (HELPER_PERSON, "Мама"), (HELPER_ROLE, "admin")];
    assert_eq!(send(&app, "POST", pair_uri, &through, json!({})).await.0, StatusCode::FORBIDDEN);
}

/// A package that needs a newer schema than this gateway knows is refused here, before anything is kept and
/// before the gateway starts again — as every build's route since it came refuses one, in these words: a station of
/// an earlier build so refuses a routine sending of schema 2 that reaches it all the same (transfer.rs), and the
/// computer knows the refusal by them (watch.rs).
#[tokio::test]
async fn a_package_that_needs_a_newer_schema_is_refused_before_anything_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().to_string_lossy().into_owned();
    let app = station(&[("PEER_KEY", PAIR), ("STATE_DIR", state.as_str())]);
    assert_eq!(app.config.skills.state_dir.as_deref(), Some(dir.path()), "a package would be kept here");
    let pair = format!("Bearer {PAIR}");
    let mut bundle = transfer::export(&app.db, &app.secrets, "pc").unwrap();
    bundle.schema = transfer::SCHEMA + 1;
    let sealed = transfer::seal(&bundle, PAIR);
    let (status, body) = send(&app, "POST", "/api/v1/transfer", &[("authorization", pair.as_str())], sealed).await;
    let refused = Some("Набор настроек не для этого шлюза: обновите его.");
    assert_eq!((status, body["message"].as_str()), (StatusCode::BAD_REQUEST, refused), "{body}");
    assert!(!transfer::incoming_file(dir.path()).exists(), "nothing kept for the next start");
    assert_eq!(transfer::synced_at(&app.db, "received"), None, "nor noted as received");
}
