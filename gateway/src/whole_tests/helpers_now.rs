//! The tests of `helpers/now.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

use crate::helpers::contract::Ability;
use crate::helpers::now::*;
use crate::helpers::Reach;

#[test]
fn a_cloud_is_named_by_its_host() {
    assert_eq!(cloud_name("https://api.openai.com/v1").as_deref(), Some("OpenAI"));
    assert_eq!(cloud_name("https://api.groq.com/openai/v1").as_deref(), Some("Groq"));
    assert_eq!(cloud_name("https://api.elevenlabs.io").as_deref(), Some("ElevenLabs"));
    assert_eq!(cloud_name("https://api.example.invalid/v1").as_deref(), Some("example.invalid"));
    assert_eq!(cloud_name("not an address"), None);
}

#[test]
fn a_server_elsewhere_at_home_is_named_by_its_host_and_one_on_this_machine_goes_unnamed() {
    assert_eq!(home_name("http://192.168.1.50:8091/v1").as_deref(), Some("192.168.1.50"));
    assert_eq!(home_name("http://tts:8091/v1").as_deref(), Some("tts"));
    assert_eq!(home_name("http://nas.local:8091").as_deref(), Some("nas.local"));
    for own in ["http://127.0.0.1:8091/v1", "http://localhost:8091", "http://LOCALHOST:8091", "http://[::1]:8091"] {
        assert_eq!(home_name(own), None, "{own}");
    }
    assert_eq!(home_name("not an address"), None);
}

#[test]
fn the_first_road_that_gives_is_in_use_and_the_first_ones_problem_says_why() {
    let away = |name: &str| Road {
        by: By::Helper,
        helper: Some(name.into()),
        name: Some(name.into()),
        reach: Some(Reach::Home),
        problem: Some(format!("{name} не на связи")),
    };
    let cloud = Road::of(By::Cloud, Some("OpenAI".into()), Some(Reach::Foreign), None);
    let row_ = row(Ability::Hearing, vec![away("A"), away("B"), cloud]);
    assert_eq!((row_.by, row_.first, row_.why.as_deref()), (By::Cloud, false, Some("A не на связи")));
    let first = row(Ability::Apps, vec![Road::of(By::Here, None, Some(Reach::Home), None)]);
    assert_eq!((first.by, first.first, first.why), (By::Here, true, None));
    let none = row(Ability::Messages, vec![away("A")]);
    assert_eq!((none.by, none.first, none.why.as_deref()), (By::None, false, Some("A не на связи")));
    assert_eq!(row(Ability::Messages, Vec::new()).by, By::None);
}

/// «Где сейчас» and the registry say a helper has no bot by one rule (`RemoteHelper::no_bot`): its contacts answer
/// so, or its contract of version 1 declares the messages not ready, read or not; its contacts only unread (just
/// added) are «не готов» on both.
#[tokio::test]
async fn where_now_and_the_registry_say_no_bot_by_one_rule() {
    use crate::core::build::Profile;
    use crate::core::events::EventBus;
    use crate::helpers::tests::{contract, stand, station, ALL, KEY};
    use crate::helpers::AddInput;
    use crate::placement::AvailabilityService;

    // A station's one helper, added (it answered, its contacts not read) and maybe looked at since: the problem of
    // its road of the messages in «Где сейчас», and the registry's reason.
    async fn said(status: serde_json::Value, look: bool) -> (Option<String>, String) {
        let helper = stand(status).await;
        let station = station(&[]);
        let registry = AvailabilityService::with(Profile { station: true }, false, false, EventBus::new());
        registry.set_helpers(station.helpers.clone());
        let add = AddInput { url: helper.url.clone(), key: KEY.into(), ..AddInput::default() };
        station.helpers.add(add, None).await.unwrap();
        if look {
            station.helpers.check_all().await;
        }
        let road = helper_roads(&station.helpers, Ability::Messages).remove(0);
        (road.problem, registry.get("messages").unwrap().reason)
    }
    let no_bot = |name: &str| {
        (Some(format!("{name}: не настроен бот Telegram")), "У Турбо не настроен бот Telegram".to_string())
    };

    let unread = (Some("Турбо a пока не готов".to_string()), "Турбо ещё не готово".to_string());
    assert_eq!(said(contract("a", ALL), false).await, unread, "its bot's contacts not read yet");
    let botless = [("hearing", true), ("voice", true), ("messages", false)];
    assert_eq!(said(contract("b", &botless), false).await, no_bot("Турбо b"), "its contract says so");
    assert_eq!(said(contract("a", ALL), true).await, no_bot("Турбо a"), "its contacts answer so (503)");
}
