//! The tests of `home/status.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

use serde_json::json;

use crate::app::App;
use crate::core::availability::State as Way;
use crate::home::status::*;
use crate::operations::network_watch::Outages;
use crate::operations::system_memory::{Counters, Pressure};
use crate::settings::definitions::PanelMode;

fn good() -> Facts {
    Facts {
        on_station: true,
        voice: Some(true),
        brain: Way::Ready,
        brain_title: Some("Claude".into()),
        brain_reason: None,
        turbos: vec![TurboFact { name: "Ноутбук".into(), up: true, ..TurboFact::default() }],
        own_voice: true,
        voice_failed: None,
        mode: PanelMode::Gateway,
        wifi: None,
    }
}

fn states(rows: &[Row]) -> Vec<(&'static str, RowState, Option<&'static str>)> {
    rows.iter().map(|row| (row.id, row.state, row.action)).collect()
}

/// Everything in place: every row green, only the mode with its way to change.
#[test]
fn a_home_in_order_is_green_row_by_row() {
    let rows = rows(&good());
    assert!(rows.iter().all(|row| row.state == RowState::Ok), "{rows:?}");
    assert_eq!(
        rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        ["voice", "brain", "turbo", "characterVoice", "mode"]
    );
}

/// Each fault the test of 21.11 met as «говорит Алиса» is its own row with its own fix: the voice past DuoWave,
/// a key that stopped working, the Turbo behind a firewall, Alice's voice instead of the character's.
#[test]
fn each_fault_is_its_own_row_with_its_fix() {
    let mut facts = good();
    facts.voice = Some(false);
    facts.brain = Way::Off;
    facts.brain_reason = Some("ключ больше не работает.".into());
    facts.turbos = vec![TurboFact {
        name: "Ноутбук".into(),
        error: Some("порт закрыт".into()),
        id: Some("pc-1".into()),
        firewall: true,
        ..TurboFact::default()
    }];
    facts.own_voice = false;
    let rows = rows(&facts);
    assert_eq!(
        states(&rows)[..4],
        [
            ("voice", RowState::Bad, Some("pointLocal")),
            ("brain", RowState::Bad, Some("brain")),
            ("turbo", RowState::Bad, Some("firewall")),
            ("characterVoice", RowState::Warn, Some("voice")),
        ]
    );
    assert!(rows[1].text.contains("ключ больше не работает"), "{}", rows[1].text);
    assert!(rows[2].text.contains("Ноутбук") && rows[2].text.contains("порт закрыт"), "{}", rows[2].text);
    assert_eq!(rows[2].turbo.as_deref(), Some("pc-1"), "the firewall's button on that very computer");
}

/// The Turbo's voice refused by Windows (22.11, WinError 4551 from a Windows Turbo, every answer silent): the row
/// says so with the way on that computer; another failure — its words and the voice to choose. Alice speaks meanwhile.
#[test]
fn a_turbo_voice_windows_blocked_is_said_plainly_with_the_way_on_that_computer() {
    let mut facts = good();
    let why =
        "own speech: HTTP 500 {\"error\": \"[WinError 4551] An Application Control policy has blocked this file\"}";
    assert!(blocked_by_windows(why));
    facts.voice_failed = Some(VoiceFailed {
        blocked_by_windows: true,
        why: why.into(),
        turbo: Some("gw-win".into()),
        name: Some("Компьютер Windows".into()),
    });
    let voice = rows(&facts).into_iter().find(|row| row.id == "characterVoice").unwrap();
    assert_eq!(
        (voice.state, voice.action, voice.turbo.as_deref()),
        (RowState::Warn, Some("windowsVoice"), Some("gw-win"))
    );
    assert!(voice.text.contains("Smart App Control") && voice.text.contains("голос Алисы"), "{}", voice.text);

    facts.voice_failed =
        Some(VoiceFailed {
            blocked_by_windows: false, why: "Турбо ответило 500.".into(), turbo: None, name: None
        });
    let voice = rows(&facts).into_iter().find(|row| row.id == "characterVoice").unwrap();
    assert_eq!(voice.action, Some("voice"));
    assert!(voice.text.contains("Турбо ответило 500") && !voice.text.contains(".."), "{}", voice.text);
}

/// A home with several Turbos (22.11): all on the line — one green row with their names; one away while another
/// helps — a warning that says who helps; all away — a fault. The firewall's button only for a Turbo that said
/// its firewall keeps the station out, never beside a key or a silence (22.11: it stood by «не принимает ключ»).
#[test]
fn several_turbos_share_one_row_and_the_firewall_button_is_only_for_a_firewall() {
    let turbo = |name: &str, up: bool, error: Option<&str>, firewall: bool| TurboFact {
        name: name.into(),
        up,
        error: error.map(str::to_string),
        id: Some(format!("pc-{name}")),
        firewall,
    };
    let row_of = |turbos: Vec<TurboFact>| turbo_row(&turbos);

    let both = row_of(vec![turbo("Mac", true, None, false), turbo("Windows", true, None, false)]);
    assert_eq!((both.state, both.action), (RowState::Ok, None));
    assert!(both.text.contains("«Mac», «Windows»"), "{}", both.text);

    let one_away =
        row_of(vec![turbo("Mac", false, Some("не ответило вовремя"), false), turbo("Windows", true, None, false)]);
    assert_eq!((one_away.state, one_away.action, one_away.turbo.as_deref()), (RowState::Warn, None, None));
    assert!(
        one_away.text.contains("«Mac» не на связи") && one_away.text.contains("помогает «Windows»"),
        "{}",
        one_away.text
    );

    let closed =
        row_of(vec![turbo("Mac", true, None, false), turbo("Windows", false, Some("Сделайте сеть частной."), true)]);
    assert_eq!((closed.action, closed.turbo.as_deref()), (Some("firewall"), Some("pc-Windows")));

    let all_away = row_of(vec![turbo("Mac", false, Some("ключ"), false)]);
    assert_eq!((all_away.state, all_away.action), (RowState::Bad, None));
}

/// No Turbo is a warning, not a fault: the station works without one. No model in «Алиса» is a warning too —
/// Yandex answers there.
#[test]
fn no_turbo_and_no_model_in_alice_are_warnings() {
    let mut facts = good();
    facts.turbos.clear();
    facts.brain = Way::Off;
    facts.mode = PanelMode::Relay;
    let rows = rows(&facts);
    assert_eq!(
        states(&rows)[1..3],
        [("brain", RowState::Warn, Some("brain")), ("turbo", RowState::Warn, Some("turbo"))]
    );
}

/// The Turbo says why the station cannot reach it (Windows' firewall closed): with the pair's key only, and its
/// words then stand in the row of a Turbo that is not on the line.
/// A tester's Mini 2, 22.11: its Wi-Fi dropped for minutes many times a day. The station's row says how often and
/// how long in the last day, right after the voice, and what it sees of the cause.
#[test]
fn a_station_that_lost_its_wifi_says_how_often_how_long_and_why() {
    use crate::operations::network_watch::Outage;
    let hour = 3600 * 1000;
    let now = 30 * hour;
    let outages = Outages {
        // One going on for 2 minutes, one of 3 minutes an hour ago, one more than a day ago — not counted.
        offline_since: Some(now - 2 * 60_000),
        ended: [
            Outage { from: now - 26 * hour, to: now - 26 * hour + 60_000 },
            Outage { from: now - hour, to: now - hour + 3 * 60_000 },
        ]
        .into(),
    };
    assert_eq!(day_of(&outages, now), (2, 5));
    assert_eq!(day_of(&Outages::default(), now), (0, 0));

    let mut facts = good();
    facts.wifi = Some(WifiFact { drops: 2, minutes: 5, hint: Some("Сигнал слабый.".into()) });
    let rows = rows(&facts);
    assert_eq!(rows[1].id, "wifi");
    assert_eq!(rows[1].state, RowState::Warn);
    assert!(rows[1].text.contains("пропаданий: 2") && rows[1].text.contains("5 мин"), "{}", rows[1].text);
    assert!(rows[1].text.ends_with("Сигнал слабый."), "{}", rows[1].text);

    // The cause: AliceBridge stuck first, then a weak signal, then too little memory.
    let stuck = r#"{"transaction": {"phase": "rolling_back", "message": "Wi-Fi confirmation timed out"}}"#;
    let low = Counters { total_mb: 120, pool_mb: 9, pressure: Pressure::Critical, swap_in_bytes: None };
    assert!(wifi_hint(Some(stuck), Some(-80.0), Some(&low)).unwrap().contains("AliceBridge"));
    let rolling_back_only = r#"{"transaction": {"phase": "rolling_back", "message": "rolling back"}}"#;
    assert!(wifi_hint(Some(rolling_back_only), Some(-80.0), None).unwrap().contains("-80"));
    assert!(wifi_hint(None, Some(-60.0), Some(&low)).unwrap().contains("9 МБ"));
    assert_eq!(wifi_hint(None, Some(-60.0), None), None);
}

#[tokio::test]
async fn the_turbo_tells_the_centre_its_firewall_with_the_pair_key_only() {
    use axum::body::Body as HttpBody;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    let env = crate::config::Env::from_pairs([("STATE_DIR", ""), ("DUOWAVE_ROLE", "station")]);
    let app = App::build(crate::config::Config::for_tests(&env)).unwrap();
    let key = app.placement.pair_key().unwrap();
    let send = |key: Option<String>| {
        let app = app.clone();
        async move {
            let body = json!({ "closed": true, "advice": "Сеть Windows «Общедоступная»: сделайте её частной." });
            let mut request = Request::builder()
                .method("POST")
                .uri("/api/v1/home/turbo-network")
                .header("content-type", "application/json");
            if let Some(key) = key {
                request = request.header("authorization", format!("Bearer {key}"));
            }
            let mut request = request.body(HttpBody::from(body.to_string())).unwrap();
            request
                .extensions_mut()
                .insert(ConnectInfo(std::net::SocketAddr::new("192.168.1.20".parse().unwrap(), 5000)));
            crate::app::router(app).oneshot(request).await.unwrap().status()
        }
    };
    assert_eq!(send(None).await, StatusCode::FORBIDDEN);
    assert_eq!(send(Some(key)).await, StatusCode::OK);
    assert!(turbo_says(None).unwrap_or_default().contains("Общедоступная"), "a computer that did not name itself");
}

/// The route answers from a station centre with the rows and whether all are green.
#[tokio::test]
async fn the_route_answers_its_rows() {
    let env = crate::config::Env::from_pairs([("STATE_DIR", ""), ("DUOWAVE_ROLE", "station")]);
    let app = App::build(crate::config::Config::for_tests(&env)).unwrap();
    let rows = rows(&facts(&crate::platform::http::Provides::provide(&app)));
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0].id, "voice");
}
