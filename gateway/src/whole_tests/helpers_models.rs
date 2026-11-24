//! The tests of `helpers/models.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

use std::sync::Arc;

use crate::dialog::brain::helper_models::{ModelHelper, ModelHelpers};
use crate::dialog::brain::BrainService;
use crate::helpers::contract::Ability;
use crate::helpers::tests::{contract, stand, station, Station, ALL, KEY};
use crate::helpers::{Change, HelperKind, HelperState, Helpers, Reach};

/// «Мозг» over the station's stores, knowing these helpers.
fn brain_over(station: &Station, helpers: Arc<Helpers>) -> Arc<BrainService> {
    let brain = BrainService::new(
        station.config.dialog.clone(),
        station.db.clone(),
        station.settings.clone(),
        station.secrets.clone(),
        station.events.clone(),
    )
    .unwrap();
    brain.set_helpers(helpers);
    Arc::new(brain)
}

/// The helpers whose model has a row in «Мозг».
fn rows(brain: &BrainService) -> Vec<String> {
    brain.view().providers.into_iter().filter_map(|row| row.helper.map(|helper| helper.id)).collect()
}

fn ids(models: &[ModelHelper]) -> Vec<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

#[tokio::test]
async fn the_helpers_with_a_model_in_order_each_with_its_route_its_key_and_whether_it_is_on_the_line() {
    let modelless: Vec<(&str, bool)> = ALL.iter().copied().filter(|(id, _)| *id != "model").collect();
    let not_ready: Vec<(&str, bool)> = ALL.iter().map(|(id, ready)| (*id, *ready && *id != "model")).collect();
    let (a, b, c) = (
        stand(contract("a", ALL)).await,
        stand(contract("b", &modelless)).await,
        stand(contract("c", &not_ready)).await,
    );
    let station = station(&[]);
    station.put("home-a", &a.url, HelperKind::Home);
    station.put("home-b", &b.url, HelperKind::Home);
    station.put("home-c", &c.url, HelperKind::Home);
    let before = station.helpers.model_helpers();
    assert_eq!(ids(&before), ["home-a", "home-b", "home-c"], "not read yet: each may have one");
    assert!(before.iter().all(|model| !model.read && !model.up && model.state == HelperState::Unknown));

    station.helpers.check_all().await;
    let models = station.helpers.model_helpers();
    assert_eq!(ids(&models), ["home-a", "home-c"], "b declares none");
    let first = &models[0];
    assert_eq!(first.base_url, format!("{}/home/v1/llm", a.url));
    assert_eq!(first.key.as_deref(), Some(KEY));
    assert_eq!((first.name.as_str(), first.reach, first.kind), ("Турбо home-a", Reach::Home, HelperKind::Home));
    assert!(first.up && first.read && !first.linked && !first.builtin && first.provider.is_none());
    assert!(!models[1].up && models[1].read, "c declares its model, not ready yet");

    // Away, switched off for the model, switched off: still listed while it only is away.
    a.set_away(true);
    station.helpers.check_all().await;
    station.helpers.check_all().await;
    assert!(!station.helpers.model_helpers()[0].up);
    assert_eq!(station.helpers.model_helpers()[0].state, HelperState::Down);
    station.change("home-c", Change { without: Some(vec![Ability::Model]), ..Change::default() });
    assert_eq!(ids(&station.helpers.model_helpers()), ["home-a"]);
    station.change("home-a", Change { enabled: Some(false), ..Change::default() });
    assert!(station.helpers.model_helpers().is_empty());
}

#[tokio::test]
async fn the_regulator_is_said_not_applied_a_helper_kept_out_stays_listed() {
    let server = stand(contract("srv", ALL)).await;
    let station = station(&[]);
    // A stand-in at home, given as one's own server: its reach is mine.
    station.put("own-srv", &server.url, HelperKind::Own);
    station.helpers.check_all().await;
    station.helpers.set_away(crate::helpers::Away::Home, None).unwrap();
    let models = station.helpers.model_helpers();
    assert_eq!(ids(&models), ["own-srv"]);
    assert_eq!(models[0].reach, Reach::Mine);
    assert!(!ModelHelpers::lets(&*station.helpers, Reach::Mine) && ModelHelpers::lets(&*station.helpers, Reach::Home));
    assert!(!models[0].up, "not looked at while kept out");
}

/// One computer reached by two entries — its address at home and its tunnel, the same `helperId` (review of stage
/// 4): one model under the earlier entry, its row made and kept by the list, never by who is on the line — nothing
/// made for the later entry while the earlier one is away, nothing dropped or written when it is back —, asked by
/// the tunnel while it stands for the computer, out of the home for so long.
#[tokio::test]
async fn one_helper_by_two_entries_is_one_model_asked_by_the_road_that_stands_for_it() {
    use crate::config::ModelProfile;
    use crate::core::availability::State;
    use crate::core::events::GatewayEvent;
    let (lan, tunnel) = (stand(contract("pc", ALL)).await, stand(contract("pc", ALL)).await);
    let station = station(&[("DIALOG_PROVIDER", "none")]);
    station.put("home-a", &lan.url, HelperKind::Home);
    station.put("own-t", &tunnel.url, HelperKind::Own);
    station.helpers.check_all().await;
    let brain = brain_over(&station, station.helpers.clone());
    brain.keep_helper_models().unwrap();
    assert_eq!(rows(&brain), ["home-a"], "the later entry has no row of its own");
    let row = brain.view().providers.into_iter().find(|row| row.helper.is_some()).unwrap();
    let fast = serde_json::json!({"provider": row.id, "model": "home"});
    brain.choose(&serde_json::Map::from_iter([("fast".to_string(), fast)]), None).unwrap();
    let mut events = station.events.subscribe();

    // The computer leaves home: its address there away, the tunnel stands for it.
    lan.set_away(true);
    station.helpers.check_all().await;
    station.helpers.check_all().await;
    let models = station.helpers.model_helpers();
    assert_eq!(ids(&models), ["home-a"]);
    let road = (models[0].via.as_deref(), models[0].base_url.as_str(), models[0].reach, models[0].up);
    assert_eq!(road, (Some("own-t"), format!("{}/home/v1/llm", tunnel.url).as_str(), Reach::Mine, true));
    brain.keep_helper_models().unwrap();
    assert_eq!(rows(&brain), ["home-a"], "no row made for the road");
    let now = brain.now(ModelProfile::Fast);
    assert_eq!((now.state, now.helper.as_deref(), now.reach), (State::Ready, Some("home-a"), Some(Reach::Mine)));
    assert!(brain.view().providers.iter().any(|view| view.id == row.id && view.cloud), "out of the home for now");

    // Home again: its own road, and nothing written all along.
    lan.set_away(false);
    station.helpers.check_all().await;
    let models = station.helpers.model_helpers();
    assert_eq!((models[0].via.as_deref(), models[0].reach), (None, Reach::Home));
    brain.keep_helper_models().unwrap();
    assert_eq!(rows(&brain), ["home-a"]);
    assert_eq!(brain.view().providers.iter().find(|view| view.id == row.id).map(|view| view.cloud), Some(false));
    let mut written = 0;
    while let Ok(event) = events.try_recv() {
        written += usize::from(matches!(&*event, GatewayEvent::BrainChanged { change, .. } if change != "choice"));
    }
    assert_eq!(written, 0, "the row stood as it was");
}

/// A start has read no helper yet, and one not read only may have a model: no row is made for a maybe, and none is
/// dropped. A computer without a model of its own, away while the station starts again, never gets one; the
/// server's stays.
#[tokio::test]
async fn a_start_makes_and_drops_no_row_until_the_helpers_are_read() {
    let modelless: Vec<(&str, bool)> = ALL.iter().copied().filter(|(id, _)| *id != "model").collect();
    let (computer, server) = (stand(contract("pc", &modelless)).await, stand(contract("srv", ALL)).await);
    let env = [("HOME_SERVER_URL", computer.url.as_str()), ("HOME_SERVER_KEY", KEY), ("DIALOG_PROVIDER", "none")];
    let station = station(&env);
    let brain = brain_over(&station, station.helpers.clone());
    brain.keep_helper_models().unwrap();
    assert!(rows(&brain).is_empty(), "the computer is not read yet: nothing made for a maybe");
    station.put("own-x", &server.url, HelperKind::Own);
    station.helpers.check_all().await;
    brain.keep_helper_models().unwrap();
    assert_eq!(rows(&brain), ["own-x"], "the computer has none: the server's");

    // Started again while the computer is off: a new engine over the same stores, nobody read.
    computer.set_away(true);
    let profile = crate::core::build::Profile::for_role(station.config.placement.role.as_deref());
    let started = Helpers::new(
        station.config.clone(),
        station.settings.clone(),
        station.secrets.clone(),
        station.events.clone(),
        profile,
        station.identity.clone(),
    );
    let again = brain_over(&station, started.clone());
    again.keep_helper_models().unwrap();
    assert_eq!(rows(&again), ["own-x"], "kept at the start");
    started.check_all().await;
    again.keep_helper_models().unwrap();
    assert_eq!(rows(&again), ["own-x"], "the computer away is not read yet");
    computer.set_away(false);
    started.check_all().await;
    again.keep_helper_models().unwrap();
    assert_eq!(rows(&again), ["own-x"], "read at last, the computer has none");
    started.stop();
}
