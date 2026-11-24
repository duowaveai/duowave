//! The tests of `helpers/home_voice.rs` that need the whole gateway (`App`): its crate does not have it, the program
//! does.

use std::sync::Arc;

use crate::app::App;
use crate::config::{Config, Env};
use crate::helpers::home_voice::*;
use crate::helpers::tests::{stand, KEY};
use crate::helpers::{AddInput, HelperKind};
use crate::platform::http::Provides;
use crate::settings::definitions::PERSONA;

fn gateway(dir: &std::path::Path, role: &str) -> Arc<App> {
    let data = dir.to_string_lossy().into_owned();
    let env = Env::from_pairs([
        ("STATE_DIR", ""),
        ("DATA_DIR", data.as_str()),
        ("PLUGINS_BUNDLED", "false"),
        ("DUOWAVE_ROLE", role),
    ]);
    App::build(Config::for_tests(&env)).unwrap()
}

/// The window offers the voice of the helper that gives it — not only the old «computer» — and names it as its
/// last status did; without a helper it offers nothing; a computer never leans on its helpers for its voice.
#[tokio::test]
async fn the_voice_window_offers_the_helper_that_gives_the_voice() {
    let status = serde_json::json!({
        "version": "0.12.0", "peerApi": 2,
        "contract": { "version": 1, "helper": { "id": "srv", "name": "Мой сервер" }, "abilities": [
            { "id": "voice", "ready": true, "details": { "voice": "silero/baya" } },
            { "id": "hearing", "ready": true }
        ] }
    });
    let server = stand(status).await;
    let dir = tempfile::tempdir().unwrap();
    let station = gateway(dir.path(), "station");
    let alone = current(&station.provide());
    assert!(!alone.offered && !alone.up && alone.voice.is_none(), "no helper, no voice to offer");
    let own = AddInput { url: server.url.clone(), key: KEY.into(), kind: Some(HelperKind::Own), ..AddInput::default() };
    station.helpers.add(own, None).await.unwrap();
    assert!(station.helpers.entry("home-computer").is_none(), "one's own server, not the computer");
    let view = current(&station.provide());
    assert_eq!((view.offered, view.up, view.reached, view.voice.as_deref()), (true, true, true, Some("Baya")));

    let other = tempfile::tempdir().unwrap();
    let computer = gateway(other.path(), "pc");
    computer.voice.set_home(computer.helpers.clone());
    assert!(!computer.voice.has_home() && !offered(&computer.provide()), "its own voice window, as before");
}

/// Item 70: the home's character chosen with its voice, the voice up on the Турбо later — the station switches to
/// the computer's voice by itself, once; the owner's own «Алиса» after that stays.
#[tokio::test]
async fn the_station_switches_to_the_computers_voice_once_the_characters_voice_is_up_and_keeps_the_owners_alice() {
    let contract = |voice: &str| {
        serde_json::json!({
            "version": "0.16.0", "peerApi": 3,
            "contract": { "version": 1, "helper": { "id": "srv", "name": "Турбо" }, "abilities": [
                { "id": "voice", "ready": true, "details": { "voice": voice } }
            ] }
        })
    };
    let server = stand(contract("piper/ru_RU-dmitri-medium")).await;
    let dir = tempfile::tempdir().unwrap();
    let station = gateway(dir.path(), "station");
    let own = AddInput { url: server.url.clone(), key: KEY.into(), kind: Some(HelperKind::Own), ..AddInput::default() };
    station.helpers.add(own, None).await.unwrap();
    let id = station.helpers.list()[0].id.clone();
    station.voice.use_yandex();
    let demo = "p-0a1b2c3d4e5f:demo";
    let persona = crate::settings::definitions::Persona::plain(Some(demo.into()), "Демо-персонаж".into());
    station.settings.set(&PERSONA, &persona, None).unwrap();
    follow(&station.provide(), Some(demo), Some("p-0a1b2c3d4e5f"));

    // The model still downloads: the Турбо speaks a fallback voice — nothing switches.
    assert!(!follow_voice(&station.provide()));
    assert_eq!(current(&station.provide()).chosen, "alice");

    // The character's own voice came up there: the station speaks the computer's voice, by itself.
    *server.status.lock().unwrap() = contract("own:c-p-0a1b2c3d4e5f-demo-omnivoice/c-p-0a1b2c3d4e5f-demo-omnivoice");
    station.helpers.check(&id).await.unwrap();
    assert!(follow_voice(&station.provide()));
    assert_eq!(current(&station.provide()).chosen, "computer");
    assert!(!follow_voice(&station.provide()), "once");

    // The owner picks Alice by hand: hers stays, even when the same character is chosen again (its manner).
    chosen_by_hand(&station.provide());
    station.voice.use_yandex();
    character_chosen(&station.provide(), Some(demo));
    assert!(!follow_voice(&station.provide()));
    assert_eq!(current(&station.provide()).chosen, "alice");
}

/// The computer is on the line, but its voice is not ready (none installed): the window says so, not «не на связи».
#[tokio::test]
async fn a_computer_on_the_line_without_a_voice_is_reached_not_up() {
    let status = serde_json::json!({
        "version": "0.16.0", "peerApi": 3,
        "contract": { "version": 1, "helper": { "id": "mac", "name": "Mac" }, "abilities": [
            { "id": "voice", "ready": false },
            { "id": "hearing", "ready": true }
        ] }
    });
    let server = stand(status).await;
    let dir = tempfile::tempdir().unwrap();
    let station = gateway(dir.path(), "station");
    let own = AddInput { url: server.url.clone(), key: KEY.into(), kind: Some(HelperKind::Own), ..AddInput::default() };
    station.helpers.add(own, None).await.unwrap();
    let view = current(&station.provide());
    assert!(view.offered, "the computer would give the voice");
    assert!(!view.up && view.reached, "on the line, its voice not ready: {view:?}");
}

#[test]
fn names_a_voice_by_its_name_not_by_its_id() {
    assert_eq!(voice_name("silero/baya").as_deref(), Some("Baya"));
    assert_eq!(voice_name("piper/ru_RU-irina-medium").as_deref(), Some("Irina"));
    assert_eq!(voice_name("silero/ксения").as_deref(), Some("Ксения"));
    assert_eq!(voice_name("omnivoice/3f2a9c1e-77b0-4c1d-9a55-0c1d2e3f4a5b"), None, "a voice lab's id");
    assert_eq!(voice_name("cosyvoice3/v42"), None);
}
