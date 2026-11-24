//! The tests of `helpers/clients.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::Request;
use serde_json::{json, Map};

use crate::app::App;
use crate::config::{Config, Env};
use crate::helpers::clients::*;
use crate::helpers::contract::{Abilities, Ability, HelperIdentity};
use crate::platform::http::{Caller, LinkOrigin};

fn computer() -> Arc<App> {
    App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap()
}

/// A request's caller with these headers, and a home's link origin when given.
async fn caller(app: &Arc<App>, headers: &[(&str, &str)], link: Option<LinkOrigin>) -> Caller {
    let mut request = Request::builder().uri("/home/v1/status");
    for (name, value) in headers {
        request = request.header(*name, axum::http::HeaderValue::from_bytes(value.as_bytes()).unwrap());
    }
    let mut request = request.body(()).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new("192.168.1.20".parse().unwrap(), 5000)));
    if let Some(link) = link {
        request.extensions_mut().insert(link);
    }
    let (mut parts, _) = request.into_parts();
    Caller::from_request_parts(&mut parts, app).await.unwrap()
}

fn confirmed(app: &App, home: &str, gateway: &str, own_home: bool) -> (String, String) {
    let (id, key) = app.pc.clients.pair(home, gateway, own_home, None).unwrap();
    assert!(app.pc.clients.confirm(&id, Names { gateway: "Колонка".into(), home: "Дача".into() }));
    (id, key)
}

#[tokio::test]
async fn clients_are_remembered_by_key_with_the_names_they_give() {
    let app = computer();
    let input =
        Map::from_iter([("name".into(), json!("Колонка: домашний сервер")), ("grants".into(), json!(["home.models"]))]);
    let created = app.access.keys.create(&input, None).unwrap();
    let bearer = format!("Bearer {}", created.secret);
    // A 0.11 station names nothing: it shows by its key's name.
    let station = caller(&app, &[("authorization", &bearer)], None).await;
    app.pc.clients.saw(&station);
    let station = caller(
        &app,
        &[
            ("authorization", &bearer),
            ("x-duowave-gateway", "gw-1"),
            ("x-duowave-gateway-name", "  Кух\u{202E}ня\t  "),
            ("x-duowave-home", "home-1"),
        ],
        None,
    )
    .await;
    app.pc.clients.saw(&station);
    app.pc.clients.served(&station, Ability::Hearing, Instant::now());
    app.pc.clients.served(&station, Ability::Hearing, Instant::now());
    let id = format!("key:{}", created.key.id);
    let placed = [("station-a".to_string(), Some(created.key.id.clone()))];
    let views = app.pc.clients.view(&placed);
    let view = views.iter().find(|view| view.id == id).expect("the key client");
    assert_eq!(
        (view.via, view.name.as_str(), view.station.as_deref()),
        ("key", "Колонка: домашний сервер", Some("station-a"))
    );
    assert_eq!(view.gateways.len(), 1);
    assert_eq!(
        (view.gateways[0].id.as_str(), view.gateways[0].name.as_str()),
        ("gw-1", "Кухня"),
        "bidi and control characters out"
    );
    assert_eq!(view.home.as_ref().map(|home| home.id.as_str()), Some("home-1"));
    assert_eq!(view.today[&Ability::Hearing].count, 2);
    assert_eq!(view.gives, Ability::V1.to_vec(), "the owner made that key: everything");
    // A person is no client.
    let token = app.access.auth.setup("admin-password-of-tests", "", "127.0.0.1").await.unwrap().unwrap().token;
    let person = caller(&app, &[("authorization", &format!("Bearer {token}"))], None).await;
    app.pc.clients.saw(&person);
    assert_eq!(app.pc.clients.view(&[]).len(), 1);
    // At most 16 gateways a client.
    for at in 0..20 {
        let gateway = format!("gw-{at}");
        let more = caller(&app, &[("authorization", &bearer), ("x-duowave-gateway", &gateway)], None).await;
        app.pc.clients.saw(&more);
    }
    assert_eq!(app.pc.clients.view(&[])[0].gateways.len(), GATEWAYS);
}

#[tokio::test]
async fn another_home_gets_the_muscles_and_its_own_home_everything() {
    let app = computer();
    let (other, other_key) = confirmed(&app, "home-other", "gw-1", false);
    assert_eq!(app.pc.clients.given(&other), MUSCLES);
    assert!(!app.pc.clients.gives(&other, Ability::Messages) && app.pc.clients.gives(&other, Ability::Voice));
    let (own, own_key) = confirmed(&app, "home-own", "gw-2", true);
    assert_eq!(app.pc.clients.given(&own), given_at_pairing(true));
    // The computer's devices and folders (contract 3): its own home's by default, another home's only when given.
    assert!(app.pc.clients.gives(&own, Ability::Devices) && app.pc.clients.gives(&own, Ability::Files));
    assert!(!app.pc.clients.gives(&other, Ability::Devices) && !app.pc.clients.gives(&other, Ability::Files));
    // The owner gives another home more.
    app.pc.clients.set_gives(&other, vec![Ability::Hearing, Ability::Messages], None).unwrap();
    assert!(app.pc.clients.gives(&other, Ability::Messages) && !app.pc.clients.gives(&other, Ability::Voice));
    assert_eq!(
        app.pc.clients.set_gives("link:000000000000", vec![], None).unwrap_err().status,
        axum::http::StatusCode::NOT_FOUND
    );
    // The same gateway of the own home pairing again: its old pairing and key replaced.
    let (again, again_key) = confirmed(&app, "home-own", "gw-2", true);
    assert!(app.pc.clients.check(&own_key).unwrap().is_none() && app.pc.clients.check(&again_key).unwrap().is_some());
    assert!(!app.pc.clients.alive(&own) && app.pc.clients.alive(&again));
    // Another home's claim of the same ids replaces nothing: it proved no home.
    let (_, _) = confirmed(&app, "home-other", "gw-1", false);
    assert!(app.pc.clients.check(&other_key).unwrap().is_some());
    // A gateway that claimed this home without its key, then pairs again proving it: its claim is replaced.
    let (claim, claim_key) = confirmed(&app, "home-own", "gw-3", false);
    let (proven, _) = confirmed(&app, "home-own", "gw-3", true);
    assert!(
        app.pc.clients.check(&claim_key).unwrap().is_none()
            && !app.pc.clients.alive(&claim)
            && app.pc.clients.alive(&proven)
    );
    // Removed: its key opens nothing.
    app.pc.clients.remove(&other, None).unwrap();
    assert!(app.pc.clients.check(&other_key).unwrap().is_none());
    assert_eq!(app.pc.clients.given(&other), Abilities::NONE);
}

#[tokio::test]
async fn last_seen_is_written_at_close_and_rarely_otherwise() {
    let app = computer();
    let (id, _) = confirmed(&app, "home-1", "gw-1", false);
    let stored = || app.pc.clients.record_of(&id).unwrap().gateways["gw-1"].last_seen.clone();
    let at_pairing = stored();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let origin = LinkOrigin {
        client: id.clone(),
        own_home: false,
        home: HelperIdentity { id: "home-1".into(), name: "Дача".into() },
        gateway: HelperIdentity { id: "gw-1".into(), name: "Колонка".into() },
    };
    let link = caller(&app, &[], Some(origin.clone())).await;
    for _ in 0..100 {
        app.pc.clients.saw(&link);
    }
    assert_eq!(stored(), at_pairing, "a hundred requests write nothing");
    app.pc.clients.online(&id, &origin.gateway, true);
    app.pc.clients.online(&id, &origin.gateway, false);
    assert!(stored() > at_pairing, "the link's close writes when it was last seen");
    // A link client's gateways are kept to sixteen, in memory as in the record, however many ids its links bring.
    for at in 0..40 {
        let gateway = HelperIdentity { id: format!("gw-new-{at}"), name: String::new() };
        app.pc.clients.online(&id, &gateway, true);
        app.pc.clients.online(&id, &gateway, false);
    }
    assert!(locked(&app.pc.clients.memory).seen[&id].len() <= GATEWAYS);
    assert_eq!(app.pc.clients.record_of(&id).unwrap().gateways.len(), GATEWAYS);
    assert_eq!(app.pc.clients.view(&[])[0].gateways.len(), GATEWAYS);
}

/// A record that does not read is never taken for «nobody» (review of stage 7), the pins' rule: a right key is
/// neither refused nor let in — the link server says «again» —, a wrong one is refused as ever, a live link is kept
/// at the next look, a home is given nothing meanwhile, and nothing is written over the record — a new pairing
/// waits, so do the owner's changes and the last-seen times —; read again, all is as it was.
#[tokio::test]
async fn a_record_that_does_not_read_refuses_no_right_key_cuts_no_link_and_is_never_written_over() {
    let app = computer();
    let (id, key) = confirmed(&app, "home-1", "gw-1", false);
    let unreadable = |broken: bool| {
        let flag = if broken { "'unreadable'" } else { "0" };
        let sql = format!("UPDATE settings SET is_default = {flag} WHERE key = 'helper_clients'");
        app.db.with(|c| c.execute(&sql, [])).unwrap();
    };
    // Its row no longer reads (an I/O error of a small disk, say); writing still works.
    unreadable(true);
    assert!(app.pc.clients.check(&key).is_err(), "a right key: again, never refused");
    let wrong = format!("dwl1_{}_{}", "0".repeat(12), "a".repeat(64));
    assert_eq!(app.pc.clients.check(&wrong).unwrap(), None, "a wrong one: refused as ever");
    assert!(app.pc.clients.alive(&id), "its live links are kept");
    assert_eq!(app.pc.clients.given(&id), Abilities::NONE, "given nothing meanwhile");
    assert!(app.pc.clients.pair("home-2", "gw-2", false, None).is_err(), "a new pairing waits");
    assert!(app.pc.clients.set_gives(&id, vec![Ability::Messages], None).is_err());
    assert!(app.pc.clients.remove(&id, None).is_err());
    let gateway = HelperIdentity { id: "gw-1".into(), name: "Колонка".into() };
    app.pc.clients.online(&id, &gateway, true);
    app.pc.clients.online(&id, &gateway, false);
    assert!(locked(&app.pc.clients.memory).dirty, "the last-seen times wait for a record that reads");

    unreadable(false);
    assert_eq!(app.pc.clients.check(&key).unwrap().map(|client| client.id), Some(id.clone()));
    assert_eq!(app.pc.clients.given(&id), MUSCLES);
    assert_eq!(app.pc.clients.view(&[]).iter().map(|view| view.id.as_str()).collect::<Vec<_>>(), [id.as_str()]);
    app.pc.clients.persist_seen();
    assert!(!locked(&app.pc.clients.memory).dirty);
}
