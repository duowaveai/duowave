//! The tests of `talk/rules.rs` that need the whole gateway (`App`): its crate does not have it, the program does.
//!
//! Step 4 of docs/plans/audio-interface.md on a whole gateway: the works' words of a station are the layer's without
//! the model (episode B), the open door takes a phrase without a template as a wish while «громче» stays the station's,
//! and «напиши навык…» is not a message.

use crate::config::{Config, Env};
use crate::skills::capabilities::CapabilityName;
use crate::skills::router::{Decision, ModelState, Resolution, RouteInput};
use crate::skills::skill::{ExecutionContext, Outcome};
use crate::utils::time::now_ms;

fn env(dir: &std::path::Path) -> Env {
    let text = |sub: &str| dir.join(sub).to_string_lossy().into_owned();
    Env::from_pairs([
        ("STATE_DIR", String::new()),
        ("DIALOG_PROVIDER", "echo".into()),
        ("PACKAGES_DIR", text("packages")),
        ("DATA_DIR", text("data")),
        ("VOICE_LAB_DIR", text("voice-lab")),
        ("TTS_MODELS_DIR", text("voices")),
    ])
}

/// What the router decides for a phrase of the kitchen's station, and what the station says when it is ours.
async fn say(
    app: &std::sync::Arc<crate::app::App>,
    text: &str,
) -> (Option<&'static str>, CapabilityName, Option<Outcome>) {
    let input = RouteInput {
        text: text.into(),
        device_tag: "kitchen".into(),
        wake_word: true,
        model: ModelState { enabled: true, tools: true, ..Default::default() },
        now: now_ms(),
        ..RouteInput::default()
    };
    match app.skills.router.decide(&input) {
        Decision::Intent { intent, resolution, rule, .. } => {
            let outcome = match (&resolution, rule) {
                (Resolution::Own(skill), Some("talk:works")) => {
                    let context =
                        ExecutionContext { text: text.into(), ..ExecutionContext::new("kitchen", chrono::Utc::now()) };
                    Some(skill.execute(&intent, &context).await.unwrap())
                }
                _ => None,
            };
            (rule, intent.capability, outcome)
        }
        other => panic!("not a rule's: {other:?}"),
    }
}

fn speech(outcome: &Option<Outcome>) -> String {
    outcome.as_ref().and_then(|outcome| outcome.speech.clone()).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_works_words_and_the_door_are_the_layers_and_the_stations_controls_stay_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::app::App::build(Config::for_tests(&env(dir.path()))).unwrap();

    // Nothing in work: the words of a thing are still the layer's, and it says so.
    let (rule, _, said) = say(&app, "Алиса, как там виджет?").await;
    assert_eq!((rule, speech(&said).as_str()), (Some("talk:works"), "Сейчас в работе ничего нет."));
    assert_eq!(say(&app, "Алиса, какая погода?").await.0.filter(|rule| *rule == "talk:works"), None);

    // The door: a phrase without a template becomes a wish.
    let (_, _, said) = say(&app, "Алиса, мастерская").await;
    assert_eq!(speech(&said), "Мастерская открыта. Что делаем?");
    assert!(said.as_ref().is_some_and(|outcome| outcome.listen), "the station listens for the answer");
    let (rule, capability, _) = say(&app, "Алиса, громче").await;
    assert_ne!(rule, Some("talk:works"), "the station's controls stay its own");
    assert_eq!(capability, CapabilityName::Volume);
    let (rule, _, said) = say(&app, "Алиса, часы с секундами").await;
    assert_eq!(rule, Some("talk:works"));
    assert_eq!(
        speech(&said),
        "Делаю: часы с секундами. Около минуты. Можно добавлять детали по ходу. Что-то ещё добавить?"
    );
    let work = app.talk.works().all(Some("kitchen"), None).remove(0);
    assert_eq!((work.brief.as_str(), work.device()), ("часы с секундами", "kitchen"));

    // Episode B: how it goes, from the work's progress, without the model.
    let (rule, _, said) = say(&app, "Алиса, как там виджет?").await;
    assert_eq!(rule, Some("talk:works"));
    assert!(speech(&said).starts_with("В работе "), "{}", speech(&said));
    let (_, _, said) = say(&app, "Алиса, выйти из мастерской").await;
    assert_eq!(speech(&said), "Мастерская закрыта.");
    assert!(!app.talk.now("kitchen", crate::memory::text::Lang::Ru).contains("Мастерская открыта."));

    // «Напиши навык…» is a thing to make, not a message to someone.
    let (rule, capability, _) = say(&app, "Алиса, напиши навык будильник для кота").await;
    assert_eq!((rule, capability), (Some("talk:works"), CapabilityName::Plugins));
    let (_, capability, _) = say(&app, "Алиса, напиши маме что я задержусь").await;
    assert_eq!(capability, CapabilityName::Calls);
}
