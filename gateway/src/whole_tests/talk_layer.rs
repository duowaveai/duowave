//! The tests of `talk/layer.rs` that need the whole gateway (`App`): its crate does not have it, the program does.

use crate::config::{Config, Env};
use crate::plugins::workshop::activity::By;
use crate::plugins::workshop::meta::{DraftMeta, Face, Stage};
use crate::talk::works::WorkState;

fn env(dir: &std::path::Path) -> Env {
    let text = |sub: &str| dir.join(sub).to_string_lossy().into_owned();
    Env::from_pairs([
        ("STATE_DIR", text("state")),
        ("DIALOG_PROVIDER", "echo".into()),
        ("PACKAGES_DIR", text("packages")),
        ("DATA_DIR", text("data")),
        ("VOICE_LAB_DIR", text("voice-lab")),
        ("TTS_MODELS_DIR", text("voices")),
    ])
}

/// The clean-path check of 19.11: the block said «На столе: …» of every package that was on, and the brain told
/// the person a widget was on their desk while nobody saw it there. It names the apps that are on — no claim of
/// where their widgets stand — and the ones whose code runs on the Турбо, so a call that waits for it is no surprise.
#[test]
fn the_block_names_the_apps_that_are_on_and_which_run_on_the_turbo() {
    let layer = crate::talk::layer::TalkLayer::new(None, crate::core::events::EventBus::new());
    layer.set_desk(std::sync::Arc::new(|| {
        vec![
            crate::talk::layer::DeskApp { id: "weather".into(), title: "Погода".into(), turbo: false },
            crate::talk::layer::DeskApp { id: "youtube".into(), title: "YouTube".into(), turbo: true },
        ]
    }));
    layer.door.open("kitchen");
    let ru = layer.now("kitchen", crate::memory::text::Lang::Ru);
    assert!(ru.contains("- Включены приложения: Погода [weather], YouTube [youtube] — на Турбо."), "{ru}");
    assert!(!ru.contains("На столе"), "{ru}");
    let en = layer.now("kitchen", crate::memory::text::Lang::En);
    assert!(en.contains("- Apps switched on: Погода [weather], YouTube [youtube] — on the Turbo."), "{en}");
}

/// A run of the agent is a work whoever started it; a restart while it goes on interrupts the work and the card —
/// nothing restarts by itself.
#[tokio::test]
async fn follows_the_cards_and_a_restart_interrupts_the_work_going_on() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::app::App::build(Config::for_tests(&env(dir.path()))).unwrap();
    let mut heard = app.events.subscribe();
    app.workshop.create("golos-vidzhet-1", Some("widget"), Some("Испанский"), None).unwrap();
    let card = DraftMeta {
        by: By::Voice,
        who: Some("kitchen".into()),
        asked: "сделай виджет: испанские слова дня".into(),
        kind: Face::Widget,
        stage: Stage::Working,
        updated: 0,
        note: None,
        kept: true,
        work: None,
        unchanged: false,
    };
    app.workshop.keep_card("golos-vidzhet-1", card, Some("made")).unwrap();
    let work = app.talk.works().of_target("workshop", "golos-vidzhet-1").expect("a work for the run");
    assert_eq!((work.state, work.title.as_str(), work.device()), (WorkState::Working, "Испанский", "kitchen"));
    assert_eq!(work.origin.activator, crate::talk::Activator::Word);
    let mut changes = Vec::new();
    while let Ok(event) = heard.try_recv() {
        if let crate::core::events::GatewayEvent::WorkChanged { work, device, change } = event.as_ref() {
            changes.push(format!("{work}:{device}:{change}"));
        }
    }
    assert_eq!(changes, ["1:kitchen:started"]);
    let now = app.talk.now("kitchen", crate::memory::text::Lang::Ru);
    assert!(now.contains("- В работе [р1]: виджет «Испанский»"), "{now}");
    assert_eq!(app.talk.now("hall", crate::memory::text::Lang::Ru), "", "another device knows nothing of it");
    drop(app);

    let app = crate::app::App::build(Config::for_tests(&env(dir.path()))).unwrap();
    let work = app.talk.works().get(&work.id).unwrap();
    assert_eq!(work.state, WorkState::Interrupted);
    assert_eq!(app.workshop.card("golos-vidzhet-1").stage, Stage::Interrupted);
    let now = app.talk.now("kitchen", crate::memory::text::Lang::Ru);
    assert!(now.contains("- Прервано [р1]: виджет «Испанский»"), "{now}");
}

/// «На стол или примерить?» said and nobody answered in its time: nothing is done, the work waits («Ждёт вас»), and
/// the question is asked once more after the device's next talk — once, never twice. «Нет» is a «not now».
#[tokio::test(start_paused = true)]
async fn a_question_nobody_answered_waits_and_is_asked_once_more_once() {
    use crate::talk::asked::{Kind, ANSWER_FOR};
    use crate::talk::works::{NewWork, Outcome, Question, WorkResult};
    let layer = crate::talk::layer::TalkLayer::new(None, crate::core::events::EventBus::new());
    let ready = |title: &str| {
        let work = layer.works.create(NewWork {
            kind: crate::talk::layer::APP,
            face: None,
            executor: "workshop".into(),
            target: Some(title.to_lowercase()),
            title: title.into(),
            brief: "часы".into(),
            origin: crate::talk::Origin::station("kitchen", true),
            again: false,
        });
        let line = format!("{title}: готово. На стол или примерить?");
        layer
            .works
            .update(&work.id, |work| {
                work.state = WorkState::Ready;
                work.result = Some(WorkResult { line: line.clone(), end: None, ..WorkResult::default() });
                work.question = Some(Question::new(line, 0, Kind::Ready));
            })
            .unwrap()
    };
    let clock = ready("Часы");
    assert!(layer.waiting().iter().any(|work| work.id == clock.id), "ready with a question: it waits");
    assert_eq!(layer.remind("kitchen"), None, "not asked yet — nothing to remind of");
    layer.said_on(&clock.id, "kitchen");
    assert!(layer.asked.open("kitchen").is_some());
    tokio::time::sleep(ANSWER_FOR + std::time::Duration::from_millis(10)).await;
    tokio::task::yield_now().await;
    let question = layer.works.get(&clock.id).unwrap().question.unwrap();
    assert_eq!(question.outcome, Some(Outcome::Silent));
    assert!(layer.waiting().iter().any(|work| work.id == clock.id), "silent: it still waits");
    assert_eq!(layer.remind("hall").as_deref(), None, "another device was not asked");
    assert_eq!(layer.remind("kitchen").as_deref(), Some("Часы ещё ждёт: на стол или примерить?"));
    assert!(layer.asked.open("kitchen").is_some(), "the reminder asks again: the answer is taken");
    tokio::time::sleep(ANSWER_FOR + std::time::Duration::from_millis(10)).await;
    tokio::task::yield_now().await;
    assert_eq!(layer.remind("kitchen"), None, "never a second time");

    let weather = ready("Погода");
    layer.said_on(&weather.id, "kitchen");
    layer.ended_asking(&weather.id, Outcome::Declined);
    assert!(!layer.waiting().iter().any(|work| work.id == weather.id), "declined: it does not wait for the person");
}

/// The hand check of 19.11 on the layer: a run's end reaches every work going on its thing, «Остановить» ends a
/// work its executor is no longer at, a work whose run ended unheard is settled, and what became of a thing — or
/// its new version — answers the questions of its older works, so «Ждёт вас» counts only what really waits.
mod hand_check_1911 {
    use std::sync::Arc;

    use crate::core::events::EventBus;
    use crate::talk::asked::Kind;
    use crate::talk::executor::{fake::Shelf, Presence, Report};
    use crate::talk::layer::{TalkLayer, SETTLE_AFTER_MS};
    use crate::talk::origin::{Activator, Origin};
    use crate::talk::works::{End, NewWork, Question, Work, WorkResult, WorkState, APP};
    use crate::utils::time::now_ms;

    fn layer() -> (Arc<TalkLayer>, Arc<Shelf>) {
        let layer = TalkLayer::new(None, EventBus::new());
        let shelf = Shelf::with(&[("btc", "Bitcoin rate", Presence::Draft)]);
        layer.add_executor(shelf.clone());
        (layer, shelf)
    }

    /// A work of the thing `btc` as the panel's «Доделать» makes it: going until its run ends.
    fn going(layer: &TalkLayer) -> Work {
        layer.works.create(NewWork {
            kind: APP,
            face: None,
            executor: "workshop".into(),
            target: Some("btc".into()),
            title: "Bitcoin rate".into(),
            brief: "Доделай начатое и проверь, что всё работает.".into(),
            origin: Origin::computer(Activator::Text),
            again: true,
        })
    }

    /// A work of `btc` that ended «не доделано» and asks «Доделать?».
    fn unfinished(layer: &TalkLayer) -> Work {
        let work = going(layer);
        layer
            .works
            .update(&work.id, |work| {
                work.state = WorkState::Unfinished;
                let line = "Bitcoin rate: не доделано. Доделать?".to_string();
                work.result = Some(WorkResult { line: line.clone(), end: None, ..WorkResult::default() });
                work.question = Some(Question::new(line, 0, Kind::Unfinished));
            })
            .unwrap()
    }

    fn state(layer: &TalkLayer, work: &Work) -> WorkState {
        layer.works.get(&work.id).unwrap().state
    }

    #[tokio::test]
    async fn a_run_that_ends_ends_every_work_going_on_its_thing() {
        let (layer, _) = layer();
        let older = going(&layer);
        std::thread::sleep(std::time::Duration::from_millis(3));
        let newer = going(&layer);
        let ended = Report::Ended { target: "btc".into(), state: WorkState::Ready, line: None, title: None };
        layer.heard("workshop", ended);
        assert_eq!(state(&layer, &newer), WorkState::Ready, "the newest going takes the end");
        assert_eq!(state(&layer, &older), WorkState::Ready, "its twin ends with it, not «идёт» forever");
        assert_eq!(layer.works.going(), 0);

        // A newer record that ended no longer hides an older one still going: the next run's end finds it.
        let left = going(&layer);
        std::thread::sleep(std::time::Duration::from_millis(3));
        let done = going(&layer);
        layer.works.update(&done.id, |work| work.state = WorkState::Failed);
        let ended = Report::Ended { target: "btc".into(), state: WorkState::Unfinished, line: None, title: None };
        layer.heard("workshop", ended);
        assert_eq!(state(&layer, &left), WorkState::Unfinished, "the end reached the work going");
        assert_eq!(state(&layer, &done), WorkState::Failed, "the ended one is left as it was");
    }

    #[tokio::test]
    async fn stop_ends_a_work_its_executor_is_no_longer_at() {
        let (layer, _) = layer();
        let work = going(&layer);
        let label = format!("р{}", work.id);
        let said = layer.act(&Origin::computer(Activator::Text), "stop", Some(&label)).await.unwrap();
        assert!(!said.contains("уже не в работе"), "{said}");
        assert_eq!(state(&layer, &work), WorkState::Stopped);
        assert!(layer.works.get(&work.id).unwrap().ended_at.is_some());
        let again = layer.act(&Origin::computer(Activator::Text), "stop", Some(&label)).await.unwrap();
        assert!(again.contains("уже не в работе"), "stopped already: {again}");
    }

    #[tokio::test]
    async fn a_work_whose_run_ended_unheard_is_settled_and_one_at_work_is_not() {
        let (layer, shelf) = layer();
        let work = going(&layer);
        let later = now_ms() + SETTLE_AFTER_MS + 1000;
        layer.settle_at(later);
        assert_eq!(state(&layer, &work), WorkState::Working, "the executor cannot tell: the record is believed");
        shelf.at.lock().unwrap().insert("btc".into(), true);
        layer.settle_at(later);
        assert_eq!(state(&layer, &work), WorkState::Working, "at work on it: going");
        shelf.at.lock().unwrap().insert("btc".into(), false);
        layer.settle_at(now_ms());
        assert_eq!(state(&layer, &work), WorkState::Working, "just begun: its run may not show yet");
        layer.settle_at(later);
        assert_eq!(state(&layer, &work), WorkState::Interrupted, "at nothing on it for a minute: not «идёт»");
    }

    #[tokio::test]
    async fn a_new_version_answers_the_old_question_and_the_desk_answers_them_all() {
        let (layer, _) = layer();
        let old = unfinished(&layer);
        assert_eq!(layer.waiting().len(), 1, "«Доделать?» waits");
        std::thread::sleep(std::time::Duration::from_millis(3));
        let label = format!("р{}", old.id);
        layer.add(Origin::computer(Activator::Text), "Доделай начатое.", Some(&label), false).await.unwrap();
        assert!(layer.waiting().is_empty(), "«Доделать» began the new version: the old one no longer waits");
        assert_eq!(layer.works.going(), 1);

        // An older version asking, a newer one put on the desk: the desk answers both.
        let (layer, _) = self::layer();
        let older = unfinished(&layer);
        std::thread::sleep(std::time::Duration::from_millis(3));
        let newer = going(&layer);
        layer.works.update(&newer.id, |work| work.state = WorkState::Ready);
        layer.heard("workshop", Report::Decided { target: "btc".into(), end: End::Desk });
        assert!(layer.waiting().is_empty(), "nothing behind «Ждёт вас»");
        let older = layer.works.get(&older.id).unwrap();
        assert_eq!((older.state, older.result.and_then(|result| result.end)), (WorkState::Done, Some(End::Desk)));
    }
}
