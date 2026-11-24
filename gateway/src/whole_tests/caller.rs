//! The tests of `platform/http/caller.rs` that need the whole gateway (`App`): its crate does not have it, the program
//! does.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body as HttpBody;
use axum::extract::ConnectInfo;
use axum::extract::FromRequestParts;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use crate::app::App;
use crate::config::{Config, Env};
use crate::platform::http::caller::*;
use crate::platform::http::peer::{HELPER_PERSON, HELPER_ROLE};
use crate::platform::http::Zone;

const PAIR: &str = "0123456789abcdef0123456789abcdef";

/// The station's gateway (the computer's build playing it) with the pair's key in its .env.
fn station() -> Arc<App> {
    let env = Env::from_pairs([("STATE_DIR", ""), ("DUOWAVE_ROLE", "station"), ("PEER_KEY", PAIR)]);
    App::build(Config::for_tests(&env)).unwrap()
}

/// A GET from the home network with these headers.
async fn get(app: &Arc<App>, uri: &str, headers: &[(&str, &str)]) -> (StatusCode, Value) {
    let mut request = Request::builder().uri(uri);
    for (name, value) in headers {
        request = request.header(*name, axum::http::HeaderValue::from_bytes(value.as_bytes()).unwrap());
    }
    let mut request = request.body(HttpBody::empty()).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new("192.168.1.20".parse().unwrap(), 5000)));
    let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_link_caller_is_remote_whatever_socket_it_came_by() {
    let app = station();
    let origin = LinkOrigin {
        client: "link:0123456789ab".into(),
        own_home: false,
        home: crate::core::helpers::contract::HelperIdentity { id: "h1".into(), name: "Дача".into() },
        gateway: crate::core::helpers::contract::HelperIdentity { id: "g1".into(), name: "Колонка".into() },
    };
    let mut request = Request::builder().uri("/home/v1/status").body(HttpBody::empty()).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new("127.0.0.1".parse().unwrap(), 5000)));
    request.extensions_mut().insert(origin);
    let (mut parts, _) = request.into_parts();
    let caller = Caller::from_request_parts(&mut parts, &app).await.unwrap();
    assert_eq!(caller.zone, Zone::Remote, "a loopback socket does not make a home's request this computer's");
    assert!(caller.zone_at_least(Zone::Home).is_err());
    assert_eq!(caller.link().map(|link| link.client.as_str()), Some("link:0123456789ab"));
    // Its own principal: the computer's help only, named by its home and gateway; PANEL_REMOTE does not stop it.
    let principal = caller.signed_in().unwrap();
    assert_eq!((principal.id.as_str(), principal.name.as_str()), ("link-link:0123456789ab", "Дача · Колонка"));
    assert!(caller.permitted("home.models").is_ok());
    assert_eq!(caller.permitted("station.view").unwrap_err().status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_helper_has_the_rights_of_the_role_the_pass_names_and_a_members_without_one() {
    let app = station();
    let pair = format!("Bearer {PAIR}");
    let me = |role: Option<&'static str>| {
        let mut headers = vec![("authorization", pair.as_str()), (HELPER_PERSON, "Мама")];
        headers.extend(role.map(|role| (HELPER_ROLE, role)));
        headers
    };

    // The computer's own call: no role — a member, never an admin.
    let (status, body) = get(&app, "/api/v1/auth/me", &[("authorization", pair.as_str())]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["message"]["role"], "member");
    assert_eq!(body["message"]["name"], "Турбо");
    let (_, body) = get(&app, "/api/v1/auth/me", &me(Some("admin"))).await;
    assert_eq!(body["message"]["role"], "admin");
    assert_eq!(body["message"]["name"], "Турбо (Мама)", "the name as it came, in UTF-8");
    let (_, body) = get(&app, "/api/v1/auth/me", &me(Some("owner"))).await;
    assert_eq!(body["message"]["role"], "guest", "an unknown role is the least");

    // A member at the computer sees what a member sees here: not the journal, the people, the history.
    for uri in ["/api/v1/requests", "/api/v1/users", "/api/v1/audit"] {
        let (status, body) = get(&app, uri, &me(Some("member"))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
        assert_eq!(get(&app, uri, &me(None)).await.0, StatusCode::FORBIDDEN, "{uri} without a role");
        let (status, body) = get(&app, uri, &me(Some("admin"))).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    }
    assert_eq!(get(&app, "/api/v1/availability", &me(Some("member"))).await.0, StatusCode::OK);
    assert_eq!(get(&app, "/api/v1/availability", &me(Some("guest"))).await.0, StatusCode::FORBIDDEN);

    // The person's own id names them only when this home knows it (docs/plans/home-and-devices.md, stage 2): their
    // desk and panel memory are theirs everywhere; an id the home does not know is the computer's shared row.
    let input: serde_json::Map<String, Value> =
        serde_json::from_value(serde_json::json!({ "name": "Аня", "role": "member" })).unwrap();
    let anya = app.access.users.create(&input, None).await.unwrap();
    let named = |id: &'static str| {
        vec![
            ("authorization", pair.as_str()),
            (HELPER_ROLE, "member"),
            (crate::platform::http::peer::HELPER_PERSON_ID, id),
        ]
    };
    let anya_id: &'static str = Box::leak(anya.id.clone().into_boxed_str());
    let (_, body) = get(&app, "/api/v1/auth/me", &named(anya_id)).await;
    assert_eq!(body["message"]["id"], anya.id, "a person of the home");
    let (_, body) = get(&app, "/api/v1/auth/me", &named("someone-else")).await;
    assert_eq!(body["message"]["id"], "helper", "a stranger's id is no one's");

    // The role counts only beside the pair's key.
    let alone = [(HELPER_ROLE, "admin"), (HELPER_PERSON, "Мама")];
    assert_eq!(get(&app, "/api/v1/requests", &alone).await.0, StatusCode::UNAUTHORIZED);
    let wrong = [("authorization", "Bearer not-the-pairs-key-0123456789"), (HELPER_ROLE, "admin")];
    assert_ne!(get(&app, "/api/v1/requests", &wrong).await.0, StatusCode::OK);
}
