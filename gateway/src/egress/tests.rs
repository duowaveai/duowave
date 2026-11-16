//! The decisions where they apply: reqwest through `proxy_for`, an app's program through the entry. The published
//! table is one for the whole process: these tests take turns (`TURN`).

use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;
use crate::config::Env;
use crate::locale::country::NONE;
use crate::settings::definitions::{HOME_COUNTRY, PACKAGE_SETTINGS};

static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The published table back to none when a test ends, however it ends: other tests' HTTP calls go directly.
struct Fresh;

impl Drop for Fresh {
    fn drop(&mut self) {
        *TABLE.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

fn egress(pairs: &[(&str, &str)]) -> (Arc<Egress>, Arc<SettingsService>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let catalog = dir.path().join("catalog");
    std::fs::create_dir_all(&catalog).unwrap();
    let mut env: Vec<(&str, &str)> = pairs.to_vec();
    let catalog_text = catalog.to_string_lossy().to_string();
    env.push(("PACKAGES_CATALOG_DIR", catalog_text.as_str()));
    let mut config = Config::for_tests(&Env::from_pairs(env.iter().copied()));
    config.plugins.catalog_dir = catalog;
    let db = Arc::new(Db::memory());
    let events = EventBus::new();
    let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
    let secrets = Arc::new(SecretsService::new(db.clone(), None).unwrap());
    (Egress::new(Arc::new(config), db, settings.clone(), secrets, events), settings, dir)
}

/// A provider of «Мозг» in the database, as the panel adds one.
fn provider(egress: &Egress, id: &str, preset: &str, base_url: Option<&str>) {
    egress
        .db
        .with(|c| {
            c.execute(
                "INSERT INTO llm_providers (id, preset, kind, title, base_url, created_at, updated_at)
                 VALUES (?1, ?2, 'openai', ?1, ?3, 'now', 'now')",
                rusqlite::params![id, preset, base_url],
            )
        })
        .unwrap();
}

/// A connection's entry stand-in: a CONNECT proxy that writes down the hosts it was asked for and answers 200 without
/// going anywhere.
async fn stand_in(asked: Arc<Mutex<Vec<String>>>) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { return };
            let asked = asked.clone();
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 4096];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buffer[..read]).to_string();
                let target = head.split_whitespace().nth(1).unwrap_or_default().to_string();
                asked.lock().unwrap().push(target);
                let body = b"through";
                let answer = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(answer.as_bytes()).await;
                let _ = stream.write_all(body).await;
            });
        }
    });
    port
}

fn letter(port: u16, up: bool) -> Value {
    json!({ "items": [{
        "id": "nl", "name": "Мой VPN", "country": "NL", "url": format!("http://127.0.0.1:{port}"),
        "state": if up { "up" } else { "down" }, "current": "🇳🇱 Амстердам",
    }]})
}

/// The answer to «Где ваш дом?» as the app «VPN» keeps it: its own setting `homeCountry`.
fn home(settings: &SettingsService, answer: &str) {
    let mut stored = settings.get(&PACKAGE_SETTINGS, None).unwrap_or_default();
    stored.entry("own-server".into()).or_default().insert("homeCountry".into(), json!(answer));
    settings.set(&PACKAGE_SETTINGS, &stored, None).unwrap();
}

fn decided(egress: &Egress, id: &str) -> Decision {
    egress.table().row(id).map(|row| row.decision.clone()).unwrap_or_else(|| panic!("no row {id}"))
}

#[tokio::test]
async fn the_model_goes_through_the_connection_the_home_directly_and_a_down_server_refuses_here() {
    let _turn = TURN.lock().await;
    let _fresh = Fresh;
    let (egress, settings, _dir) = egress(&[]);
    provider(&egress, "openai", "openai", None);
    home(&settings, "RU");
    egress.rebuild();
    assert_eq!(decided(&egress, "provider:openai"), Decision::Direct, "no connection reported: as before the app");

    let asked = Arc::new(Mutex::new(Vec::new()));
    let port = stand_in(asked.clone()).await;
    egress.report("own-server", &letter(port, true)).unwrap();
    assert_eq!(decided(&egress, "provider:openai"), Decision::Via { connection: "nl".into() });
    assert_eq!(decided(&egress, registry::ALICE), Decision::Direct, "Alice works from Russia");

    let openai = url::Url::parse("https://api.openai.com/v1/chat/completions").unwrap();
    assert_eq!(proxy_for(&openai).map(|url| url.port()), Some(Some(port)), "reqwest goes to the connection's entry");
    for local in ["http://127.0.0.1:8080/x", "http://192.168.1.20/", "http://station.local/"] {
        assert_eq!(proxy_for(&url::Url::parse(local).unwrap()), None, "{local}: home — directly");
    }

    // A plain call through a client built as the gateway builds every client: through the stand-in.
    let client = crate::utils::http::client_builder().build().unwrap();
    let answer = client.get("http://api.openai.com/v1/models").send().await.unwrap().text().await.unwrap();
    assert_eq!(answer, "through");
    assert_eq!(asked.lock().unwrap().as_slice(), ["http://api.openai.com/v1/models"]);

    // The server stops answering: the call fails on this machine, nothing reaches the stand-in or the internet.
    egress.report("own-server", &letter(port, false)).unwrap();
    assert_eq!(decided(&egress, "provider:openai"), Decision::Down { connection: "nl".into() });
    let refusal = proxy_for(&openai).unwrap();
    assert_eq!(refusal.host_str(), Some("127.0.0.1"));
    assert_ne!(refusal.port(), Some(port));
    let failed = client.get("http://api.openai.com/v1/models").send().await;
    assert!(failed.is_err() || failed.unwrap().status() == reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(asked.lock().unwrap().len(), 1, "not one connection out while the server is down");
}

#[tokio::test]
async fn all_through_the_server_alice_too_and_off_is_as_before() {
    let _turn = TURN.lock().await;
    let _fresh = Fresh;
    let (egress, settings, _dir) = egress(&[]);
    home(&settings, "DE");
    let asked = Arc::new(Mutex::new(Vec::new()));
    let port = stand_in(asked).await;
    egress.report("own-server", &letter(port, true)).unwrap();
    // A home in Germany: Alice wants a Russian address, and the only connection is in the Netherlands.
    assert!(matches!(decided(&egress, registry::ALICE), Decision::Need { wait: false, .. }));
    egress.set_mode(Mode::All, None).unwrap();
    assert_eq!(decided(&egress, registry::ALICE), Decision::Via { connection: "nl".into() });
    assert!(matches!(alice(), Hop::Via(_)));
    // The server down: Alice goes directly (decision 8).
    egress.report("own-server", &letter(port, false)).unwrap();
    assert_eq!(alice(), Hop::Direct);
    egress.set_mode(Mode::Off, None).unwrap();
    egress.report("own-server", &letter(port, true)).unwrap();
    assert_eq!(decided(&egress, registry::ALICE), Decision::Direct);
    assert_eq!(decided(&egress, registry::OTHER), Decision::Direct);
    home(&settings, NONE);
}

/// The switch, the pick and «напрямую …» as the owner's sample has them (16.11): off and on again comes back to the
/// same pick; a pick's ticks go when it changes; «Только выбранные» lets only the ticked through.
#[tokio::test]
async fn the_switch_keeps_the_pick_and_a_new_pick_clears_the_old_ticks() {
    let _turn = TURN.lock().await;
    let _fresh = Fresh;
    let (egress, settings, _dir) = egress(&[]);
    home(&settings, "RU");
    let port = stand_in(Arc::new(Mutex::new(Vec::new()))).await;
    egress.report("own-server", &letter(port, true)).unwrap();
    // A gateway from before the switch: «Где не работает» reads as on, all, home directly.
    let table = egress.table();
    assert_eq!((table.mode, table.pick, table.home_direct), (Mode::Blocked, Pick::All, true));
    // «Все, кроме выбранных»: the weather ticked goes directly, the rest through the server.
    egress.set_home_direct(false, None).unwrap();
    egress.set_pick(Pick::Except, None).unwrap();
    egress.set_row(registry::OTHER, Some(Some(Route::Direct)), None, None).unwrap();
    assert_eq!(egress.table().mode, Mode::All);
    assert_eq!(decided(&egress, registry::ALICE), Decision::Via { connection: "nl".into() });
    assert_eq!(decided(&egress, registry::OTHER), Decision::Direct);
    // Off and on: the same pick, the same tick.
    egress.set_on(false, None).unwrap();
    assert_eq!(egress.table().mode, Mode::Off);
    assert_eq!(decided(&egress, registry::ALICE), Decision::Direct);
    egress.set_on(true, None).unwrap();
    let table = egress.table();
    assert_eq!((table.mode, table.pick, table.home_direct), (Mode::All, Pick::Except, false));
    assert_eq!(decided(&egress, registry::OTHER), Decision::Direct);
    // «Только выбранные»: the old tick («напрямую») goes; the ticked now go through the chosen location.
    egress.set_pick(Pick::Only, None).unwrap();
    assert_eq!(egress.table().mode, Mode::Only);
    assert_eq!(egress.table().row(registry::OTHER).map(|row| row.route.clone()), Some(Route::Auto));
    assert_eq!(decided(&egress, registry::ALICE), Decision::Direct);
    egress.set_row(registry::ALICE, Some(Some(Route::Server)), None, None).unwrap();
    assert_eq!(decided(&egress, registry::ALICE), Decision::Via { connection: "nl".into() });
    // «Если VPN не отвечает» for all: the server down, Alice waits too.
    egress.set_on_down_all(OnDownAll::Wait, None).unwrap();
    egress.report("own-server", &letter(port, false)).unwrap();
    assert_eq!(decided(&egress, registry::ALICE), Decision::Down { connection: "nl".into() });
    egress.set_pick(Pick::All, None).unwrap();
    assert_eq!(egress.table().row(registry::ALICE).map(|row| row.route.clone()), Some(Route::Auto));
    home(&settings, NONE);
}

#[tokio::test]
async fn an_apps_program_gets_its_own_row_through_the_entry() {
    let _turn = TURN.lock().await;
    let _fresh = Fresh;
    let (egress, settings, _dir) = egress(&[]);
    home(&settings, "RU");
    let asked = Arc::new(Mutex::new(Vec::new()));
    let port = stand_in(asked.clone()).await;
    egress.report("own-server", &letter(port, true)).unwrap();
    egress.set_row(registry::OTHER, Some(Some(Route::Via("nl".into()))), None, None).unwrap();
    egress.rebuild();
    let table = egress.table();
    let row = table.row(registry::OTHER).unwrap();
    assert_eq!(table.hop(&row.decision), Hop::Via(url::Url::parse(&format!("http://127.0.0.1:{port}")).unwrap()));
    assert!(egress.has_own_rules());
    egress.reset_rows(None).unwrap();
    assert!(!egress.has_own_rules());
    // An unknown app's token: refused, whoever it says it is.
    assert!(matches!(egress.entry_hop("maps", "not-a-token", "tiles.example.com"), Hop::Refuse(_)));
    assert!(matches!(egress.entry_hop(app_entry::DOWN, "nl", "api.openai.com"), Hop::Refuse(_)));
}

#[tokio::test]
async fn kept_connections_wait_after_a_restart_until_the_app_speaks() {
    let _turn = TURN.lock().await;
    let _fresh = Fresh;
    let (egress, settings, _dir) = egress(&[]);
    provider(&egress, "claude", "claude", None);
    home(&settings, "RU");
    let asked = Arc::new(Mutex::new(Vec::new()));
    let port = stand_in(asked).await;
    egress.report("own-server", &letter(port, true)).unwrap();
    assert_eq!(decided(&egress, "provider:claude"), Decision::Via { connection: "nl".into() });
    // The gateway starts again: the connection is known, does not answer yet — the model waits, it does not go out.
    let kept = settings.get(&EGRESS_CONNECTIONS, None).unwrap();
    assert_eq!(kept.len(), 1);
    let again = Egress::new(
        egress.config.clone(),
        egress.db.clone(),
        settings.clone(),
        egress.secrets.clone(),
        EventBus::new(),
    );
    again.rebuild();
    assert_eq!(decided(&again, "provider:claude"), Decision::Down { connection: "nl".into() });
}

#[tokio::test]
async fn the_country_is_the_apps_and_an_old_answer_moves_into_it_once() {
    let _turn = TURN.lock().await;
    let _fresh = Fresh;
    let (egress, settings, _dir) = egress(&[]);
    // Gateway 0.13.0 asked the household: the answer is there, the app has none of its own.
    settings.set(&HOME_COUNTRY, &"DE".to_string(), None).unwrap();
    egress.rebuild();
    assert_eq!(egress.table().home, None, "no app gives the network: nobody asked");
    assert_eq!(settings.get(&HOME_COUNTRY, None).as_deref(), Some("DE"), "kept until the app is there");

    let asked = Arc::new(Mutex::new(Vec::new()));
    let port = stand_in(asked).await;
    egress.report("own-server", &letter(port, true)).unwrap();
    assert_eq!(egress.table().home.as_deref(), Some("DE"));
    let stored = settings.get(&PACKAGE_SETTINGS, None).unwrap();
    assert_eq!(stored["own-server"]["homeCountry"], json!("DE"), "moved into the app's own setting");
    assert_eq!(settings.get(&HOME_COUNTRY, None), None, "and gone from the household's");

    // The window answers «Не скажу»: no country, nothing decided by it.
    home(&settings, NONE);
    egress.rebuild();
    assert_eq!(egress.table().home, None);
    assert!(egress.table().rows.iter().any(|row| row.why == Why::NoCountry));
    home(&settings, "ru");
    egress.rebuild();
    assert_eq!(egress.table().home.as_deref(), Some("RU"));
}
