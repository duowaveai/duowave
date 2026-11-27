//! The switch's tests: scripted Thinkings stand behind it (the door's `script`), a phrase goes through it, and the news
//! comes back to the one whose turn, call or notice it is. `OurThinking` on a whole gateway is tested in the program
//! (gateway/src/whole_tests/thinking.rs).

use std::sync::Arc;

use futures::StreamExt;
use serde_json::json;

use super::settings::THINKING;
use super::switch::Switch;
use crate::core::events::EventBus;
use crate::door::script::{Script, Step};
use crate::door::{
    self, Budget, Context, Expects, Hint, Kind, News, Notice, NoticeWhy, Opened, Outcome, Outside, Phrase, Place,
    StopWhy, Thinking, To, Told, Unasked, Urgency, What, Who, V,
};
use crate::settings::SettingsService;
use crate::storage::Db;

fn phrase(turn: &str, kind: Kind, voice: bool) -> Phrase {
    Phrase {
        v: V,
        turn: turn.into(),
        text: "привет".into(),
        lang: None,
        alts: Vec::new(),
        who: Who::default(),
        to: To::default(),
        place: Place {
            device: if kind == Kind::Computer { "computer".into() } else { "kitchen".into() },
            room: None,
            kind,
            screen: false,
            speaker: true,
        },
        hands: Vec::new(),
        opened: if voice { Opened::Button } else { Opened::Text },
        context: Context::default(),
        budget: Budget { voice, first_word_ms: Some(1500), limit_ms: None },
        heard: None,
        at: None,
    }
}

fn settings() -> Arc<SettingsService> {
    Arc::new(SettingsService::new(Arc::new(Db::memory()), EventBus::new()))
}

async fn said(events: door::Events) -> Vec<String> {
    events
        .filter_map(|event| async move {
            match event.what {
                What::Say { text } => Some(text),
                _ => None,
            }
        })
        .collect()
        .await
}

fn set(settings: &SettingsService, value: serde_json::Value) {
    let parsed: super::Settings = serde_json::from_value(value).unwrap();
    settings.set(&THINKING, &parsed, None).unwrap();
}

#[tokio::test]
async fn the_setting_picks_the_thinking_per_kind_of_device_from_the_next_phrase() {
    let (talk, ours) = (Arc::new(Script::saying("talk")), Arc::new(Script::saying("ours")));
    let store = settings();
    let switch = Switch::new(talk.clone(), ours.clone(), store.clone());
    assert_eq!(said(switch.phrase(phrase("t-1", Kind::Computer, true))).await, ["ours"], "ours by default");
    let station = said(switch.phrase(phrase("t-0", Kind::Station, true))).await;
    assert_eq!(station, ["talk"], "the station keeps its own chain until its checks by hand");
    assert!(!super::Settings::of(&store).takes_station());
    set(&store, json!({"implementation": {"default": "ours", "computer": "talk"}}));
    assert_eq!(said(switch.phrase(phrase("t-2", Kind::Computer, true))).await, ["talk"]);
    assert_eq!(said(switch.phrase(phrase("t-3", Kind::Station, true))).await, ["ours"]);
    set(&store, json!({"implementation": {"default": "ours", "text": "talk"}}));
    assert_eq!(said(switch.phrase(phrase("t-4", Kind::Computer, false))).await, ["talk"], "typed");
    assert_eq!(said(switch.phrase(phrase("t-5", Kind::Computer, true))).await, ["ours"], "spoken");
    assert_eq!((talk.phrases().len(), ours.phrases().len()), (3, 3));
    set(&store, json!({"implementation": {"station": "ours"}}));
    assert!(super::Settings::of(&store).takes_station(), "the station behind ours, by the setting");
}

#[tokio::test(start_paused = true)]
async fn a_stop_and_a_hand_s_result_reach_the_thinking_whose_turn_and_call_they_are() {
    let talk = Arc::new(Script::saying("talk"));
    let ours = Arc::new(Script::answering(|_| {
        vec![
            Step::Event(What::Hands {
                call: "c-1".into(),
                device: "kitchen".into(),
                hand: "sound.set".into(),
                args: json!({"level": 3}),
            }),
            Step::Pause(std::time::Duration::from_secs(5)),
            Step::say("Готово."),
            Step::end(Outcome::Answered),
        ]
    }));
    let switch = Switch::new(talk.clone(), ours.clone(), settings());
    let mut events = switch.phrase(phrase("t-1", Kind::Computer, true));
    assert!(matches!(events.next().await.unwrap().what, What::Hands { .. }));
    switch.tell(News::new(Told::HandsDone { call: "c-1".into(), ok: true, note: None, error: None }));
    switch.tell(News::new(Told::Stop { turn: "t-1".into(), why: StopWhy::Word, heard: None }));
    assert!(events.next().await.is_none(), "the stopped turn says nothing more");
    assert_eq!(ours.news().len(), 2, "both reached ours");
    assert!(talk.news().is_empty(), "none reached the other");
    // News nobody owns reaches every Thinking.
    switch.tell(News::new(Told::NoAnswer { question: "question".into() }));
    assert_eq!((ours.news().len(), talk.news().len()), (3, 1));
}

fn notice(id: &str) -> Outside {
    Outside::new(Unasked::Notice(Notice {
        notice: id.into(),
        text: "Курс валют: готово.".into(),
        speaker: None,
        why: NoticeWhy::WorkReady,
        work: Some("w-1".into()),
        question: None,
        urgency: Urgency::Now,
        to: Some(Hint { device: Some("computer".into()), ..Hint::default() }),
        until: None,
        expects: Expects::Free,
    }))
}

#[tokio::test]
async fn a_notice_is_said_once_by_the_thinking_that_stands_for_its_device_now() {
    let (talk, ours) = (Arc::new(Script::saying("talk")), Arc::new(Script::saying("ours")));
    let store = settings();
    let switch = Switch::new(talk.clone(), ours.clone(), store.clone());
    let mut heard = switch.notices();
    talk.notice(notice("work:1"));
    ours.notice(notice("work:1"));
    let first = heard.next().await.unwrap();
    assert_eq!(first, notice("work:1"), "ours stands for the computer by default");
    // Its delivery goes to the one that said it.
    switch.tell(News::new(Told::Delivered {
        notice: Some("work:1".into()),
        outcome: door::Delivery::Said,
        device: "computer".into(),
        heard: None,
    }));
    assert_eq!((ours.news().len(), talk.news().len()), (1, 0));
    set(&store, json!({"implementation": "talk"}));
    ours.notice(notice("work:2"));
    talk.notice(notice("work:2"));
    let next = heard.next().await.unwrap();
    assert_eq!(next, notice("work:2"));
    let more = futures::FutureExt::now_or_never(heard.next());
    assert!(more.is_none(), "the other's notice of the same work is not said twice");
}

#[tokio::test]
async fn a_phrase_said_to_a_second_agent_goes_to_its_thinking_and_the_rest_to_ours() {
    let (talk, ours) = (Arc::new(Script::saying("talk")), Arc::new(Script::saying("ours")));
    let hermes = Arc::new(Script::saying("hermes"));
    let switch = Switch::new(talk, ours.clone(), settings());
    switch.add_agent("agent:hermes", hermes.clone());
    let mut to_hermes = phrase("t-1", Kind::Computer, true);
    to_hermes.to = To { agent: Some("agent:hermes".into()), by: door::By::Word, name: Some("Гермес".into()) };
    assert_eq!(said(switch.phrase(to_hermes)).await, ["hermes"]);
    assert_eq!(said(switch.phrase(phrase("t-2", Kind::Computer, true))).await, ["ours"]);
    switch.remove_agent("agent:hermes");
    let mut again = phrase("t-3", Kind::Computer, true);
    again.to.agent = Some("agent:hermes".into());
    assert_eq!(said(switch.phrase(again)).await, ["ours"], "an agent that left: ours answers");
    assert_eq!(hermes.phrases().len(), 1);
}

#[test]
fn a_work_s_question_tells_what_it_expects() {
    use crate::talk::asked::Kind as Asked;
    let choice =
        |options: &[&str]| Expects::Choice { options: options.iter().map(|option| option.to_string()).collect() };
    assert_eq!(super::expects_of(Some(Asked::Ready)), choice(&["desk", "try"]), "«На стол или примерить?»");
    assert_eq!(super::expects_of(Some(Asked::Wait)), choice(&["wait", "stop"]), "«подождать или остановить?»");
    assert_eq!(super::expects_of(Some(Asked::Unfinished)), Expects::YesNo, "«Продолжить?»");
    assert_eq!(super::expects_of(Some(Asked::Talk)), Expects::Free);
    assert_eq!(super::expects_of(None), Expects::Free);
    // Work systems (docs/plans/work-systems.md): a system's question of rights and «Делаю?» of a plan are yes or no;
    // a limit's question expects what it offers too.
    assert_eq!(super::expects_of(Some(Asked::Permission)), Expects::YesNo, "«Claude Code спрашивает: …»");
    assert_eq!(super::expects_of(Some(Asked::Plan)), Expects::YesNo, "«План для …: … Делаю?»");
    let mut limit = crate::talk::works::Question::new("Лимит кончился.".into(), 0, Asked::Wait);
    for kind in ["switch", "hand"] {
        limit.offers.push(crate::talk::works::Offer { kind: kind.into(), id: "x".into(), title: "x".into() });
    }
    assert_eq!(super::expects_of_question(Some(&limit)), choice(&["wait", "stop", "switch", "hand"]));
    assert_eq!(super::expects_of_question(None), Expects::Free);
}

/// A foreign agent that answers every phrase with its words, and hears its stops.
struct Hermes(std::sync::Mutex<Vec<String>>);

impl super::foreign::ForeignAgent for Hermes {
    fn ask(&self, phrase: &Phrase) -> futures::future::BoxFuture<'static, anyhow::Result<String>> {
        let text = format!("Гермес: {}", phrase.text);
        Box::pin(async move { Ok(text) })
    }

    fn stop(&self, turn: &str) {
        self.0.lock().unwrap().push(turn.to_string());
    }
}

#[tokio::test]
async fn a_foreign_agent_answers_in_its_voice_and_its_notices_carry_its_as() {
    let (talk, ours) = (Arc::new(Script::saying("talk")), Arc::new(Script::saying("ours")));
    let switch = Switch::new(talk, ours, settings());
    let hermes = Arc::new(Hermes(std::sync::Mutex::new(Vec::new())));
    let foreign = Arc::new(super::foreign::Foreign { id: "agent:hermes".into(), agent: hermes.clone() });
    switch.add_agent("agent:hermes", foreign);
    let mut to_hermes = phrase("t-1", Kind::Computer, true);
    to_hermes.to = To { agent: Some("agent:hermes".into()), by: door::By::Word, name: Some("Гермес".into()) };
    let events: Vec<_> = switch.phrase(to_hermes).collect().await;
    assert!(events.iter().all(|event| event.speaker.as_deref() == Some("agent:hermes")), "{events:?}");
    assert!(matches!(&events[0].what, What::Say { text } if text == "Гермес: привет"));
    // Its notices, from a scripted second agent, carry its `as`.
    let noisy = Arc::new(Script::saying("—"));
    switch.add_agent("agent:noisy", noisy.clone());
    let mut heard = switch.notices();
    noisy.notice(notice("n-1"));
    let said = heard.next().await.unwrap();
    let Unasked::Notice(said) = said.what else { panic!("a notice") };
    assert_eq!(said.speaker.as_deref(), Some("agent:noisy"));
}

#[tokio::test(start_paused = true)]
async fn a_new_phrase_on_the_device_stops_the_turn_going_on_there() {
    let slow =
        Arc::new(Script::answering(|_| vec![Step::Pause(std::time::Duration::from_secs(30)), Step::say("поздно")]));
    let ours = Arc::new(Script::saying("ours"));
    let switch = Switch::new(Arc::new(Script::saying("talk")), ours, settings());
    switch.add_agent("agent:slow", slow.clone());
    let mut to_slow = phrase("t-1", Kind::Computer, true);
    to_slow.to.agent = Some("agent:slow".into());
    let _going = switch.phrase(to_slow);
    let _next: Vec<_> = switch.phrase(phrase("t-2", Kind::Computer, true)).collect().await;
    let stops: Vec<_> = slow.news().into_iter().filter(|news| matches!(news.what, Told::Stop { .. })).collect();
    assert_eq!(stops.len(), 1, "the slow agent's turn is stopped by the new phrase on its device");
}
