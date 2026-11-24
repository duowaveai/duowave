//! The tests of `talk/ready.rs` that need the whole gateway (`App`): its crate does not have it, the program does.
//!
//! Step 5 of docs/plans/audio-interface.md on a whole gateway, with a station that answers every push: «Готово»
//! said where it was asked, «да» without the wake word puts the thing on the desk, a phrase the question cannot read
//! goes on as any other, a new version of the thing on the desk goes there at once and «верни как было» undoes it, a
//! line not said to its end is said once more after the next talk, a key nobody filled in leaves it on the desk
//! waiting for it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use futures::future::BoxFuture;

use crate::app::App;
use crate::config::{Config, Env};
use crate::core::events::GatewayEvent;
use crate::core::requests::{RequestKind, RequestStart, RequestStatus};
use crate::ear::TurnStation;
use crate::plugins::workshop::meta::Stage;
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::router::{Decision, ModelState, Resolution, RouteInput};
use crate::skills::skill::ExecutionContext;
use crate::talk::asked::Kind;
use crate::talk::works::{End, WorkState};
use crate::utils::time::now_ms;

/// A station that runs every push as the real one calls back, and keeps what it said.
struct Station {
    app: OnceLock<Weak<App>>,
    said: Arc<Mutex<Vec<String>>>,
    /// Whether a push is said to its end; false — cut by «стоп».
    to_the_end: Arc<AtomicBool>,
}

impl TurnStation for Station {
    fn ask(
        &self,
        device: &str,
        intent: Intent,
        _label: String,
        still: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> BoxFuture<'static, bool> {
        let (app, said, to_the_end) =
            (self.app.get().and_then(Weak::upgrade), self.said.clone(), self.to_the_end.clone());
        let device = device.to_string();
        Box::pin(async move {
            let (Some(app), true) = (app, still()) else { return false };
            let Some(skill) = app.skills.router.implementation(&intent) else { return false };
            let context = ExecutionContext::new(&device, chrono::Utc::now());
            let Ok(outcome) = skill.execute(&intent, &context).await else { return false };
            said.lock().unwrap().push(outcome.speech.unwrap_or_default());
            to_the_end.load(Ordering::SeqCst)
        })
    }
    fn hearing(&self, _device: &str) -> bool {
        false
    }
    fn ringing(&self, _device: &str) -> bool {
        false
    }
}

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

/// A phrase of the kitchen's station: the rule that took it and, when it is ours, what the station says.
async fn phrase(app: &Arc<App>, text: &str, wake_word: bool) -> (Option<&'static str>, CapabilityName, String) {
    let input = RouteInput {
        text: text.into(),
        device_tag: "kitchen".into(),
        wake_word,
        model: ModelState { enabled: true, tools: true, ..Default::default() },
        now: now_ms(),
        ..RouteInput::default()
    };
    match app.skills.router.decide(&input) {
        Decision::Intent { intent, resolution, rule, .. } => {
            let said = match (&resolution, rule) {
                (Resolution::Own(skill), Some(_)) if intent.capability == CapabilityName::Plugins => {
                    let context =
                        ExecutionContext { text: text.into(), ..ExecutionContext::new("kitchen", chrono::Utc::now()) };
                    skill.execute(&intent, &context).await.unwrap().speech.unwrap_or_default()
                }
                _ => String::new(),
            };
            (rule, intent.capability, said)
        }
        Decision::Model { .. } => (None, CapabilityName::Chat, String::new()),
        other => panic!("{other:?}"),
    }
}

async fn heard(station: &Station, count: usize) -> String {
    for _ in 0..400 {
        if let Some(said) = station.said.lock().unwrap().get(count - 1).cloned() {
            return said;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!(
        "the station said {} line(s), not {count}: {:?}",
        station.said.lock().unwrap().len(),
        station.said.lock().unwrap()
    );
}

async fn until(what: &str, check: impl Fn() -> bool) {
    for _ in 0..400 {
        if check() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("never: {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_is_said_where_it_was_asked_and_yes_puts_it_on_the_desk() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::build(Config::for_tests(&env(dir.path()))).unwrap();
    let station =
        Arc::new(Station { app: OnceLock::new(), said: Arc::default(), to_the_end: Arc::new(AtomicBool::new(true)) });
    let _ = station.app.set(Arc::downgrade(&app));
    app.talk.set_turns(station.clone());
    let mut events = app.events.subscribe();

    // The old template's words: a work without the model; the agent finishes, the gateway checks it.
    let (rule, _, said) = phrase(&app, "Алиса, сделай виджет часы с секундами", true).await;
    assert_eq!(
        (rule, said.as_str()),
        (
            Some("talk:works"),
            "Делаю: часы с секундами. Около минуты. Можно добавлять детали по ходу. Что-то ещё добавить?"
        )
    );
    let ready = heard(&station, 1).await;
    assert_eq!(ready, "Часы с секундами: готово. На стол или примерить?", "the gateway's line, not the agent's words");
    let work = app.talk.works().all(Some("kitchen"), None).remove(0);
    assert_eq!(work.state, WorkState::Ready);
    until("said to its end", || app.talk.works().get(&work.id).is_some_and(|work| work.said_at.is_some())).await;
    let draft = work.target.clone().unwrap();

    // «да» without the wake word: on the desk.
    let (rule, _, said) = phrase(&app, "да", false).await;
    assert_eq!((rule, said.as_str()), (Some("talk:works"), "На столе."));
    assert!(app.workshop.host().installed(&draft).is_some(), "installed by voice");
    until("decided", || {
        app.talk
            .works()
            .get(&work.id)
            .is_some_and(|work| work.result.as_ref().is_some_and(|result| result.end == Some(End::Desk)))
    })
    .await;
    assert_eq!(app.workshop.card(&draft).stage, Stage::Installed);

    // A phrase the question cannot read is not the question's: the weather answers it.
    app.talk.asked.ask("kitchen", &work.id, Kind::Ready, false);
    let (rule, capability, _) = phrase(&app, "какая погода", false).await;
    assert_ne!(rule, Some("talk:works"));
    assert_eq!(capability, CapabilityName::Weather);

    // A new version of the thing on the desk: there at once, «верни как было» undoes it.
    let (_, _, said) = phrase(&app, "Алиса, и еще сделай цифры крупнее", true).await;
    assert_eq!(said, "Поправляю «Часы с секундами»: версия 2. Скажу, когда будет готово.");
    let edited = heard(&station, 2).await;
    assert_eq!(edited, "Часы с секундами: сделай цифры крупнее. На столе.");
    assert_eq!(app.talk.recent_edit("kitchen").as_deref(), Some(draft.as_str()));
    assert_eq!(app.talk.recent_edit("hall"), None, "another device did not change it");
    if app.history.is_some() {
        let (rule, _, _) = phrase(&app, "Алиса, верни как было", true).await;
        assert_eq!(rule, Some("history:voice"));
    }

    // A line cut short is said once more after the device's next talk — once.
    station.to_the_end.store(false, Ordering::SeqCst);
    let id = work.id.clone();
    app.talk.works().update(&id, |work| work.said_at = None);
    app.talk.say_on_station(id.clone(), true);
    heard(&station, 3).await;
    let talk = || {
        let trip = app.requests.begin_in(
            RequestStart {
                device: "kitchen".into(),
                session: None,
                kind: RequestKind::Voice,
                text: Some("который час".into()),
                parent: None,
            },
            "own",
        );
        trip.finish(RequestStatus::Answered, None, None);
    };
    talk();
    heard(&station, 4).await;
    talk();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(station.said.lock().unwrap().len(), 4, "once more, not again and again");

    let mut changes = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let GatewayEvent::WorkChanged { work, change, .. } = event.as_ref() {
            changes.push(format!("{work}:{change}"));
        }
    }
    assert!(changes.contains(&format!("{}:ready", work.id)), "{changes:?}");
    assert!(changes.contains(&format!("{}:said", work.id)), "{changes:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_nobody_filled_in_leaves_it_on_the_desk_waiting_and_the_panel_opens_its_fields() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::build(Config::for_tests(&env(dir.path()))).unwrap();
    let station =
        Arc::new(Station { app: OnceLock::new(), said: Arc::default(), to_the_end: Arc::new(AtomicBool::new(true)) });
    let _ = station.app.set(Arc::downgrade(&app));
    app.talk.set_turns(station.clone());
    let mut events = app.events.subscribe();
    phrase(&app, "Алиса, сделай виджет дела на сегодня", true).await;
    heard(&station, 1).await;
    let work = app.talk.works().all(Some("kitchen"), None).remove(0);
    let draft = work.target.clone().unwrap();
    // The agent's manifest asks for a key.
    let mut manifest: serde_json::Value =
        serde_json::from_str(&app.workshop.read(&draft, "manifest.json").unwrap()).unwrap();
    manifest["settings"] =
        serde_json::json!([{ "key": "token", "type": "string", "title": "Ключ Todoist", "secret": true }]);
    app.workshop.write(&draft, "manifest.json", &manifest.to_string()).unwrap();

    app.talk.asked.ask("kitchen", &work.id, Kind::Ready, false);
    let (_, _, said) = phrase(&app, "на стол", false).await;
    assert_eq!(said, "На столе. Чтобы заработало, впишите ключ Todoist в пульте.");
    assert!(app.workshop.host().installed(&draft).is_some());
    let view = app.workshop.host().view_one(&draft).await.unwrap();
    assert_eq!(view.waits, ["Ключ Todoist"], "«ждёт ключ» on its tile");
    let mut opened = false;
    while let Ok(event) = events.try_recv() {
        if let GatewayEvent::WorkshopChanged { draft: changed, change } = event.as_ref() {
            opened |= changed == &draft && change == "open:needs";
        }
    }
    assert!(opened, "the panel opens the fields to fill in");
}
