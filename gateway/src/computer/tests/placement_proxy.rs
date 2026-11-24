//! The tests of `placement/proxy.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body as HttpBody;
use axum::extract::ConnectInfo;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Map, Value};
use tower::ServiceExt;

use crate::access::auth_service::LoginResult;
use crate::app::App;
use crate::config::{Config, Env};
use crate::core::availability::Placement;
use crate::core::identity::principal::{user_principal, Principal, PrincipalKind};
use crate::core::identity::roles::Role;
use crate::helpers::link::plain_path;
use crate::placement::proxy::*;
use crate::platform::http::peer::{helper_role, HELPER_PERSON, HELPER_ROLE};
use crate::settings::definitions::{SettingsHome, StationPlacement};

#[test]
fn passes_the_panels_api_its_health_plugins_previews_and_media_only() {
    assert!(passable("api/v1/brain"));
    assert!(passable("health"));
    assert!(passable("panel-plugins/radio/1.2.0/panel.js"));
    assert!(passable("panel-preview/t0k3n/panel.js"));
    assert!(passable("media/radio/cover.png"));
    assert!(!passable("uni.ws"));
    assert!(!passable("debug/sessions"));
    assert!(!passable("home/v1/llm/chat/completions"));
    assert!(!passable("panel/index.html"));
    // Without the session: the station's public files only.
    assert!(public_file("panel-plugins/radio/1.2.0/panel.css") && public_file("panel-preview/t0k3n/panel.js"));
    assert!(!public_file("api/v1/packages/panel") && !public_file("media/radio/cover.png"));
    assert!(public_file("media/v/0123abcd"), "the media engine's key is the pass");
    // A plugin's call is a reading; its settings, its other routes are not.
    assert!(plugin_call("api/v1/packages/shopping/call/list"));
    for rest in [
        "api/v1/packages/shopping/settings",
        "api/v1/packages/shopping/call",
        "api/v1/packages/shopping/call/list/more",
        "api/v1/packages/call/list",
        "api/v1/workshop/drafts/d1/call/list",
    ] {
        assert!(!plugin_call(rest), "{rest}");
    }
}

#[test]
fn a_phrase_said_to_the_home_is_not_a_change() {
    for rest in ["api/v1/listen", "api/v1/talk", "api/v1/talk/speech"] {
        assert!(speaking(rest), "{rest}");
    }
    for rest in ["api/v1/talk/input", "api/v1/talk/now", "api/v1/listen/model", "api/v1/brain"] {
        assert!(!speaking(rest), "{rest}");
    }
}

#[test]
fn a_path_that_would_lead_out_of_the_whitelist_does_not_pass() {
    for path in [
        "api/v1/brain",
        "health",
        "api/v1/packages/calendar",
        "panel-plugins/radio/1.2.0/panel.js",
        "media/radio/обложка дня.png",
    ] {
        assert!(plain_path(path), "{path}");
    }
    for path in [
        "api/v1/../../debug/sessions",
        "api/v1/%2e%2e/%2E%2E/debug/sessions",
        "api/v1/.%2e/mcp/plugins",
        "api/v1/./brain",
        "api/v1//brain",
        "api/v1/brain/",
        "api/v1/..\\..\\debug",
        "api/v1/x%2F..%2F..%2Fdebug",
        "api/v1/brain?debug",
        "api/v1/brain#debug",
        "api/v1/.\t./debug",
        "api/v1/.. ",
        "api/v1/ ",
    ] {
        assert!(!plain_path(path), "{path}");
    }
    // What it stands against: the address of the request reads encoded dots as a step up.
    let url = reqwest::Url::parse("http://192.0.2.23:8790/api/v1/%2e%2e/%2E%2E/debug/sessions").unwrap();
    assert_eq!(url.path(), "/debug/sessions");
}

#[test]
fn the_station_gives_a_person_their_own_role_a_key_a_guests_and_each_role_its_own_copies() {
    assert_eq!(helper_role(&user_principal("u1", "Аня", Role::Member, None)), Role::Member);
    assert_eq!(helper_role(&user_principal("u2", "Мама", Role::Admin, None)), Role::Admin);
    let key = Principal {
        kind: PrincipalKind::ApiKey,
        id: "k1".into(),
        name: "Скрипт".into(),
        role: None,
        grants: vec!["*".into()],
        denies: Vec::new(),
    };
    assert_eq!(helper_role(&key), Role::Guest, "a key, even with every right");
    let name = |role| copy_name(role, "api/v1/requests", Some("limit=4"));
    assert_ne!(name(Role::Admin), name(Role::Member));
    assert!(name(Role::Member).starts_with("member-"));
}

#[test]
fn of_a_stations_copies_the_newest_stay() {
    let dir = tempfile::tempdir().unwrap();
    let now = std::time::SystemTime::now();
    for index in 0..5u64 {
        let path = dir.path().join(format!("member-{index}.json"));
        std::fs::write(&path, b"{}").unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(now - Duration::from_secs(100 - index)).unwrap();
    }
    std::fs::write(dir.path().join("member-9.tmp"), b"").unwrap();
    keep_newest(dir.path(), 3);
    let mut left: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["member-2.json", "member-3.json", "member-4.json", "member-9.tmp"]);
}

#[test]
fn a_stations_copies_are_its_own_folder_and_go_whole_never_more() {
    let state = tempfile::tempdir().unwrap();
    let kitchen = copies_of(Some(state.path()), "h:kitchen").unwrap();
    assert_eq!(kitchen, state.path().join("station-copies").join("h_kitchen"));
    for key in ["", ".", "..", "..."] {
        assert_eq!(copies_of(Some(state.path()), key), None, "{key:?}");
    }
    std::fs::create_dir_all(&kitchen).unwrap();
    std::fs::write(kitchen.join("member-1.json"), b"{}").unwrap();
    forget_copies(Some(state.path()), "..");
    assert!(kitchen.exists() && state.path().exists(), "a key of dots removes nothing");
    forget_copies(Some(state.path()), "h:kitchen");
    assert!(!kitchen.exists() && state.path().join("station-copies").exists());
}

const PAIR: &str = "0123456789abcdef0123456789abcdef";

/// A pretend station gateway: it answers with the headers it was asked with, and notes what it was asked.
async fn pretend_station(asked: Arc<Mutex<Vec<String>>>) -> u16 {
    pretend_station_on(asked, 0).await
}

/// The same on this port (0: any free one).
async fn pretend_station_on(asked: Arc<Mutex<Vec<String>>>, port: u16) -> u16 {
    let big = || async { axum::Json(json!({ "text": "a".repeat(COPY_LIMIT) })) };
    let echo = move |request: axum::extract::Request| {
        let asked = asked.clone();
        async move {
            let header = |name: &str| {
                let value = request.headers().get(name).map(|value| value.as_bytes().to_vec());
                String::from_utf8(value.unwrap_or_default()).unwrap_or_default()
            };
            asked.lock().unwrap().push(format!("{} {}", request.method(), request.uri().path()));
            axum::Json(json!({ "message": {
                "role": header(HELPER_ROLE),
                "person": header(HELPER_PERSON),
                "key": header("authorization"),
                "language": header("accept-language"),
            }}))
        }
    };
    let router = Router::new().route("/api/v1/big", axum::routing::get(big)).fallback(echo);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    port
}

fn kitchen(port: u16, settings_home: SettingsHome) -> StationPlacement {
    StationPlacement {
        placement: Placement::Station,
        address: "127.0.0.1".into(),
        port,
        device: Some("h:kitchen".into()),
        settings_home,
        auto_update: false,
        managed: false,
        home_key_id: None,
        moved_at: None,
    }
}

/// A request to the computer's gateway as it comes from `from`, signed with `token` (none: an empty one).
async fn send(app: &Arc<App>, from: &str, method: &str, uri: &str, token: &str) -> (StatusCode, HeaderMap, Value) {
    send_with(app, from, method, uri, token, &[]).await
}

/// The same with headers of its own.
async fn send_with(
    app: &Arc<App>,
    from: &str,
    method: &str,
    uri: &str,
    token: &str,
    extra: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Value) {
    let body = if method == "GET" || method == "HEAD" { String::new() } else { "{}".into() };
    // The panel always says its language: the station answers its words in it (test 19.11, remark 12).
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("accept-language", "ru");
    if !token.is_empty() {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    let mut request = request.body(HttpBody::from(body)).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(from.parse().unwrap(), 5000)));
    let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// Rehearsal 16.11: an away station was named by its device id («Колонка «h:…» не на связи»).
#[tokio::test]
async fn an_away_station_is_named_as_people_know_it() {
    let state = tempfile::tempdir().unwrap();
    let env = Env::from_pairs([
        ("STATE_DIR", state.path().to_string_lossy().into_owned()),
        ("PANEL_REMOTE", "true".to_string()),
    ]);
    let app = App::build(Config::for_tests(&env)).unwrap();
    let admin = app.access.auth.setup("admin-password-1", "Мама", "test").await.unwrap().unwrap().token;
    // A port nobody listens on: the station is away.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    app.secrets.set(&crate::placement::peer::station_secret("h:kitchen"), Some(PAIR)).unwrap();
    app.pc.station_gateway.watch("h:kitchen").set_record(Some(&kitchen(port, SettingsHome::Station)), None).unwrap();
    let brain = "/station/h:kitchen/api/v1/brain";

    let (status, _, body) = send(&app, "192.168.1.20", "GET", brain, &admin).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["message"], away("127.0.0.1"), "no name in the list: its address, never «h:kitchen»");

    app.db
        .with(|c| {
            c.execute(
                "INSERT INTO stations (device, name, status, first_seen, last_seen) VALUES ('h:kitchen', 'Кухня', 'approved', 'then', 'then')",
                [],
            )
        })
        .unwrap();
    let (_, _, body) = send(&app, "192.168.1.20", "GET", brain, &admin).await;
    assert_eq!(body["message"], away("Кухня"));
}

/// The first steps, 24.11: the hub on the station rebooted after its move, the pass kept «Применить» of the invite
/// code in the queue and answered with its own words — the step took them for the hub's brain and fell, and a
/// provider «added» that way had no id: `providers/undefined/signin`. A request that waits for the station gets
/// neither a queued change nor a copy: an «away» answer, marked, and the panel asks again.
#[tokio::test]
async fn a_request_that_waits_for_the_station_is_neither_queued_nor_answered_with_a_copy() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let port = pretend_station(asked.clone()).await;
    let state = tempfile::tempdir().unwrap();
    let env = Env::from_pairs([
        ("STATE_DIR", state.path().to_string_lossy().into_owned()),
        ("PANEL_REMOTE", "true".to_string()),
    ]);
    let app = App::build(Config::for_tests(&env)).unwrap();
    let admin = app.access.auth.setup("admin-password-1", "Мама", "test").await.unwrap().unwrap().token;
    app.secrets.set(&crate::placement::peer::station_secret("h:kitchen"), Some(PAIR)).unwrap();
    let watch = app.pc.station_gateway.watch("h:kitchen");
    watch.set_record(Some(&kitchen(port, SettingsHome::Station)), None).unwrap();
    let (home, brain, invite) =
        ("192.168.1.20", "/station/h:kitchen/api/v1/brain", "/station/h:kitchen/api/v1/brain/invite");
    let wait = [(WAIT_FOR, "1")];

    // On the station: a copy of the brain is kept.
    let (status, _, _) = send_with(&app, home, "GET", brain, &admin, &wait).await;
    assert_eq!(status, StatusCode::OK);

    // It reboots: nobody listens on its port.
    let gone = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    watch.set_record(Some(&kitchen(gone, SettingsHome::Station)), None).unwrap();
    let (status, headers, body) = send_with(&app, home, "GET", brain, &admin, &wait).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "no copy for the one who waits: {body}");
    assert!(headers.contains_key(AWAY) && !headers.contains_key(COPY_AT));
    let (status, headers, _) = send_with(&app, home, "PUT", invite, &admin, &wait).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(headers.contains_key(AWAY));
    assert!(crate::home::queue::list(&app.db).is_empty(), "nothing waits in the queue");

    // Without the mark — as before: the copy, and the change kept for the station.
    let (status, headers, _) = send(&app, home, "GET", brain, &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key(COPY_AT) && !headers.contains_key(AWAY));
    let (status, _, body) = send(&app, home, "PUT", invite, &admin).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["queued"], true);
    assert_eq!(crate::home::queue::list(&app.db).len(), 1);
}

/// A tester's Mini 2, 22.11: its Wi-Fi dropped many times a day, and every request of the panel waited for it and
/// wrote a line. Now the first miss marks the station away — since when goes with the copy —, the requests after it
/// answer at once without trying it, and an answer to a try ends the outage.
#[tokio::test]
async fn an_away_station_is_not_tried_on_every_request() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let port = pretend_station(asked.clone()).await;
    let state = tempfile::tempdir().unwrap();
    let env = Env::from_pairs([
        ("STATE_DIR", state.path().to_string_lossy().into_owned()),
        ("PANEL_REMOTE", "true".to_string()),
    ]);
    let app = App::build(Config::for_tests(&env)).unwrap();
    let admin = app.access.auth.setup("admin-password-1", "Мама", "test").await.unwrap().unwrap().token;
    app.secrets.set(&crate::placement::peer::station_secret("h:kitchen"), Some(PAIR)).unwrap();
    let watch = app.pc.station_gateway.watch("h:kitchen");
    watch.set_record(Some(&kitchen(port, SettingsHome::Station)), None).unwrap();
    let (home, journal) = ("192.168.1.20", "/station/h:kitchen/api/v1/requests?limit=4");

    let (status, headers, _) = send(&app, home, "GET", journal, &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!headers.contains_key(COPY_AT) && watch.away_since().is_none());

    // The station goes: nobody listens on its port. The first request misses, marks it and gets the copy.
    let gone = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    watch.set_record(Some(&kitchen(gone, SettingsHome::Station)), None).unwrap();
    let (status, headers, _) = send(&app, home, "GET", journal, &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key(COPY_AT));
    let since = watch.away_since().expect("the station is away");
    assert_eq!(headers.get(AWAY_SINCE).and_then(|value| value.to_str().ok()), Some(since.to_string().as_str()));
    assert!(!watch.live().reachable);

    // It comes back on its port, but was tried a moment ago: the next requests get the copy at once, unasked.
    let back = Arc::new(Mutex::new(Vec::new()));
    pretend_station_on(back.clone(), gone).await;
    let (status, headers, _) = send(&app, home, "GET", journal, &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key(COPY_AT));
    assert!(back.lock().unwrap().is_empty(), "an away station is not tried on every request");
    assert_eq!(watch.away_since(), Some(since), "the outage began once");

    // A look got an answer: the outage is over, the pass goes to the station again.
    watch.answered();
    let (status, headers, _) = send(&app, home, "GET", journal, &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!headers.contains_key(COPY_AT));
    assert_eq!(back.lock().unwrap().len(), 1);
    assert!(watch.away_since().is_none() && watch.live().reachable);
}

#[tokio::test]
async fn the_pass_carries_the_persons_role_stays_at_home_and_keeps_each_roles_copies() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let port = pretend_station(asked.clone()).await;
    let state = tempfile::tempdir().unwrap();
    let env = Env::from_pairs([
        ("STATE_DIR", state.path().to_string_lossy().into_owned()),
        ("PANEL_REMOTE", "true".to_string()),
    ]);
    let app = App::build(Config::for_tests(&env)).unwrap();
    let admin = app.access.auth.setup("admin-password-1", "Мама", "test").await.unwrap().unwrap().token;
    let person = json!({"name": "Аня", "role": "member", "password": "member-password-1"});
    app.access.users.create(person.as_object().unwrap(), None).await.unwrap();
    let LoginResult::Ok(member) = app.access.auth.login(Some("Аня"), "member-password-1", "test").await.unwrap()
    else {
        panic!("the member signs in")
    };
    let member = member.token;
    let script = app
        .access
        .keys
        .create(&Map::from_iter([("name".into(), json!("Скрипт")), ("grants".into(), json!(["station.view"]))]), None)
        .unwrap()
        .secret;
    app.secrets.set(&crate::placement::peer::station_secret("h:kitchen"), Some(PAIR)).unwrap();
    let watch = app.pc.station_gateway.watch("h:kitchen");
    watch.set_record(Some(&kitchen(port, SettingsHome::Station)), None).unwrap();
    let home = "192.168.1.20";
    let journal = "/station/h:kitchen/api/v1/requests?limit=4";

    // Each person reaches the station with their own role: a member stays a member there.
    let (status, _, body) = send(&app, home, "GET", journal, &member).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["message"],
        json!({"role": "member", "person": "Аня", "key": format!("Bearer {PAIR}"), "language": "ru"})
    );
    let (_, _, body) = send(&app, home, "GET", journal, &admin).await;
    assert_eq!((body["message"]["role"].as_str(), body["message"]["person"].as_str()), (Some("admin"), Some("Мама")));
    let (_, _, body) = send(&app, home, "GET", "/station/h:kitchen/api/v1/station", &script).await;
    assert_eq!(body["message"]["role"], "guest", "a key: a guest's rights there");

    // From outside the home the pass is closed, even to the admin with the panel open outside.
    let (status, _, body) = send(&app, "8.8.8.8", "GET", journal, &admin).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::FORBIDDEN, Some("Только из домашней сети.")));

    // A path that would step out of the whitelist, and a station nobody linked, go nowhere.
    let before = asked.lock().unwrap().len();
    for uri in [
        "/station/h:kitchen/api/v1/..%2F..%2Fdebug%2Fsessions",
        "/station/h:kitchen/api/v1/%252e%252e/%252e%252e/debug/sessions",
        "/station/h:kitchen/api/v1/../../debug/sessions",
    ] {
        assert_eq!(send(&app, home, "GET", uri, &admin).await.0, StatusCode::NOT_FOUND, "{uri}");
    }
    let (status, _, body) = send(&app, home, "GET", "/station/h:nobody/api/v1/brain", &admin).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["message"], "Такой колонки у этого компьютера нет.");
    assert_eq!(asked.lock().unwrap().len(), before);

    let call = "/station/h:kitchen/api/v1/packages/calendar/call/today";
    let (status, _, body) = send(&app, home, "POST", call, &member).await;
    assert_eq!((status, body["message"]["role"].as_str()), (StatusCode::OK, Some("member")), "{body}");
    assert_eq!(send(&app, home, "POST", call, &admin).await.0, StatusCode::OK);
    let pause = "/station/h:kitchen/api/v1/station/player/pause";
    assert_eq!(send(&app, home, "POST", pause, &admin).await.0, StatusCode::OK);
    assert_eq!(send(&app, home, "POST", pause, &member).await.0, StatusCode::FORBIDDEN, "a change: the admin's");
    // The home's settings live at the station, never here: a change in its window goes to it as any other.
    for (method, rest) in [("PUT", "api/v1/brain"), ("PUT", "api/v1/packages/calendar/settings")] {
        let (status, _, body) = send(&app, home, method, &format!("/station/h:kitchen/{rest}"), &admin).await;
        assert_eq!(status, StatusCode::OK, "{rest}: {body}");
    }
    for rest in ["api/v1/station/mode", "api/v1/brain/ask", "api/v1/connectors/weather/check"] {
        let (status, _, body) = send(&app, home, "POST", &format!("/station/h:kitchen/{rest}"), &admin).await;
        assert_eq!((status, body["message"]["role"].as_str()), (StatusCode::OK, Some("admin")), "{rest}");
    }

    // The station's plugins' panel parts and the workshop's previews come without a key (`import()`, `<link>`):
    // they pass without the session — from home, on the whitelist — and go as anyone's: no key, nobody named.
    for uri in
        ["/station/h:kitchen/panel-plugins/radio/1.3.0/panel.js", "/station/h:kitchen/panel-preview/t0k3n/panel.css"]
    {
        let (status, headers, body) = send(&app, home, "GET", uri, "").await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        assert_eq!(body["message"], json!({"role": "", "person": "", "key": "", "language": "ru"}), "{uri}");
        // The app's window (a frame of no origin) imports it across origins: the allowance passes too.
        assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*", "{uri}");
        assert_eq!(send(&app, home, "HEAD", uri, "").await.0, StatusCode::OK, "{uri}");
        assert_eq!(send(&app, "8.8.8.8", "GET", uri, "").await.0, StatusCode::FORBIDDEN, "{uri}: from outside");
        assert_eq!(send(&app, home, "POST", uri, "").await.0, StatusCode::UNAUTHORIZED, "{uri}: only a reading");
    }
    let before = asked.lock().unwrap().len();
    for uri in ["/station/h:kitchen/panel-plugins/../api/v1/requests", "/station/h:kitchen/panel-preview/%2e%2e/debug"]
    {
        assert_eq!(send(&app, home, "GET", uri, "").await.0, StatusCode::NOT_FOUND, "{uri}");
    }
    assert_eq!(send(&app, home, "GET", journal, "").await.0, StatusCode::UNAUTHORIZED, "the rest asks who");
    assert_eq!(asked.lock().unwrap().len(), before);
    let (_, headers, _) = send(&app, home, "GET", journal, &admin).await;
    assert!(!headers.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN), "the API is not opened across origins");

    // A big answer is not copied.
    let (status, _, _) = send(&app, home, "GET", "/station/h:kitchen/api/v1/big", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!copy_path(&app.config, "h:kitchen", Role::Admin, "api/v1/big", None).unwrap().exists());

    // The station away: each role gets its own copy, nobody another's.
    watch.set_record(Some(&kitchen(9, SettingsHome::Station)), None).unwrap();
    let (status, headers, body) = send(&app, home, "GET", journal, &member).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(headers.contains_key(COPY_AT));
    assert_eq!(body["message"]["role"], "member");
    let (_, _, body) = send(&app, home, "GET", journal, &admin).await;
    assert_eq!(body["message"]["role"], "admin");
    let (status, _, _) = send(&app, home, "GET", journal, &script).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "a key never got a copy of the journal");

    // The station's record removed: what it showed through the pass goes with it.
    let copies = copies_dir(&app.config, "h:kitchen").unwrap();
    assert!(copies.is_dir());
    watch.set_record(None, None).unwrap();
    assert!(!copies.exists(), "the copies go with the record");
}
