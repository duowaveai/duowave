//! The channel's tests: a fake port (frames in, what was played out), a scripted Thinking, a fake «Слух» that hears
//! the texts it is given, and a voice whose every phrase is a tenth of a second of sound.

use std::collections::VecDeque;
use std::sync::Mutex;

use futures::future::BoxFuture;

use super::*;
use crate::core::events::EventBus;
use crate::core::requests::{RequestKind, RequestStart, RequestView};
use crate::door::script::{Script, Step as Does};
use crate::door::{Expects, NoticeWhy, Outside, Urgency};
use crate::ear::port::fake::FakePort;
use crate::speech::test_audio;
use crate::storage::db::Db;

/// «Слух» that hears these texts, one a piece.
struct FakeHear(Mutex<VecDeque<String>>);

impl FakeHear {
    fn of(texts: &[&str]) -> Arc<FakeHear> {
        Arc::new(FakeHear(Mutex::new(texts.iter().map(|text| text.to_string()).collect())))
    }

    fn more(&self, text: &str) {
        self.0.lock().unwrap().push_back(text.to_string());
    }
}

impl Hear for FakeHear {
    fn ready(&self) -> bool {
        true
    }

    fn at_home(&self) -> bool {
        true
    }

    fn recognize(&self, _pcm: Vec<i16>) -> BoxFuture<'static, Result<String, String>> {
        let text = self.0.lock().unwrap().pop_front().unwrap_or_default();
        Box::pin(async move { Ok(text) })
    }
}

/// A voice: every phrase is 100 ms of quiet sound; how many were made.
#[derive(Default)]
struct FakeSpeak(Mutex<Vec<String>>);

impl Speak for FakeSpeak {
    fn ready(&self) -> bool {
        true
    }

    fn synthesize(&self, text: &str, _cancel: &CancellationToken) -> BoxFuture<'static, Option<(Vec<i16>, u32)>> {
        self.0.lock().unwrap().push(text.to_string());
        Box::pin(async { Some((vec![1; 1600], 16_000)) })
    }
}

/// The journal as a gateway keeps it, and the trips the ear offered to Thinking.
struct TestTrips {
    journal: Arc<crate::requests::RequestsService>,
    offered: Mutex<Vec<String>>,
}

struct OwnMode;

impl crate::requests::ModeSource for OwnMode {
    fn current(&self) -> crate::settings::definitions::PanelMode {
        crate::settings::definitions::PanelMode::Own
    }
}

impl Trips for TestTrips {
    fn begin(&self) -> Option<Arc<RequestHandle>> {
        let start = RequestStart {
            device: DEVICE.to_string(),
            session: None,
            kind: RequestKind::Voice,
            text: None,
            parent: None,
        };
        Some(self.journal.begin_in(start, "own"))
    }

    fn offer(&self, trip: Arc<RequestHandle>) {
        self.offered.lock().unwrap().push(trip.id());
    }
}

impl TestTrips {
    /// Every trip, going or ended, the newest first.
    fn all(&self) -> Vec<RequestView> {
        let mut all = self.journal.active_views();
        all.extend(self.journal.recent_views(50));
        all
    }
}

struct Rig {
    port: Arc<FakePort>,
    script: Arc<Script>,
    hear: Arc<FakeHear>,
    speak: Arc<FakeSpeak>,
    settings: Arc<SettingsService>,
    channel: Arc<VoiceChannel>,
    trips: Arc<TestTrips>,
}

fn rig(script: Script, heard: &[&str]) -> Rig {
    let port = Arc::new(FakePort::default());
    let script = Arc::new(script);
    let hear = FakeHear::of(heard);
    let speak = Arc::new(FakeSpeak::default());
    let settings = Arc::new(SettingsService::new(Arc::new(Db::memory()), EventBus::new()));
    // No quiet hours in a test: the clock of the day is not the test's.
    settings.set(&QUIET_HOURS, &QuietHours { from: 0, to: 0 }, None).unwrap();
    let trips = Arc::new(TestTrips {
        journal: crate::requests::RequestsService::new(
            Arc::new(Db::memory()),
            EventBus::new(),
            settings.clone(),
            Arc::new(OwnMode),
        ),
        offered: Mutex::new(Vec::new()),
    });
    let channel = VoiceChannel::start(Deps {
        port: port.clone(),
        thinking: script.clone(),
        hear: hear.clone(),
        speak: speak.clone(),
        settings: settings.clone(),
        events: EventBus::new(),
        others: Arc::new(|| vec!["Алиса".to_string()]),
        trips: trips.clone(),
    });
    Rig { port, script, hear, speak, settings, channel, trips }
}

fn silence(ms: u64) -> Vec<i16> {
    test_audio::silence(ms as f64)
}

fn voice(ms: u64) -> Vec<i16> {
    test_audio::voice(ms as f64, -20.0)
}

/// A phrase said after the tone: quiet, the speech, the pause that ends it.
fn said(ms: u64) -> Vec<i16> {
    [silence(400), voice(ms), silence(1400)].concat()
}

async fn until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..3000 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never: {what}");
}

/// The same, saying what the channel and the port were at when it never came.
async fn until_in(rig: &Rig, what: &str, done: impl Fn() -> bool) {
    for _ in 0..3000 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "never: {what}; the channel {:?}, capturing {:?}, played {}, said {:?}, phrases {}, news {:?}",
        rig.channel.view(),
        rig.port.capturing(),
        rig.port.played().len(),
        spoken(rig),
        rig.script.phrases().len(),
        rig.script.news()
    );
}

fn spoken(rig: &Rig) -> Vec<String> {
    rig.speak.0.lock().unwrap().clone()
}

#[tokio::test(start_paused = true)]
async fn a_press_hears_a_thought_and_the_answer_is_said_through_the_port() {
    let rig = rig(Script::saying("Сейчас пять часов."), &["который час"]);
    rig.channel.set_person("u-1", "Аня");
    rig.channel.press();
    until("the microphone opens", || rig.port.capturing() == Some(true)).await;
    assert!(rig.port.hear(&said(900)));
    until("the phrase reaches Thinking", || rig.script.phrases().len() == 1).await;
    let phrase = rig.script.phrases().remove(0);
    assert_eq!((phrase.text.as_str(), phrase.opened), ("который час", Opened::Button));
    assert_eq!((phrase.to.by, phrase.to.name.as_deref()), (By::Button, Some(APP)));
    assert_eq!(phrase.who.person.as_deref(), Some("u-1"), "the person on this computer's panel speaks");
    assert_eq!(phrase.place.kind, Kind::Computer);
    assert!(phrase.budget.voice);
    until("the answer is said", || spoken(&rig) == ["Сейчас пять часов."]).await;
    until("the channel is quiet again", || rig.channel.view().state == State::Quiet).await;
    assert!(
        rig.script
            .news()
            .iter()
            .any(|news| matches!(&news.what, Told::Said { turn, at: Some(_) } if *turn == phrase.turn)),
        "Thinking hears the answer was heard to its end"
    );
    assert_eq!(rig.port.capturing(), None, "the microphone is closed when the talk is over");
    assert!(rig.port.played().iter().any(|(samples, _)| samples.len() == 1600), "the phrase went out through the port");
}

#[tokio::test(start_paused = true)]
async fn a_question_opens_the_window_and_the_reply_goes_back_to_it() {
    let script = Script::answering(|phrase| {
        if phrase.context.answer_to.is_some() {
            vec![Does::say("Добавлю юань."), Does::end(Outcome::Answered)]
        } else {
            vec![Does::say("Что-то ещё добавить?"), Does::ask("question"), Does::end(Outcome::Asked)]
        }
    });
    let rig = rig(script, &["сделай виджет с курсом валют", "и ещё юань"]);
    rig.channel.press();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.port.hear(&said(1200));
    until("the window opens by itself", || rig.channel.view().state == State::Window).await;
    assert_eq!(rig.channel.view().why.as_deref(), Some("question"));
    rig.port.hear(&said(800));
    until("the reply reaches Thinking", || rig.script.phrases().len() == 2).await;
    let reply = rig.script.phrases().remove(1);
    assert_eq!(reply.opened, Opened::Window);
    assert_eq!((reply.context.answer_to.as_deref(), reply.context.after), (Some("question"), Some(After::Ask)));
    assert_eq!(reply.to.by, By::Window);
}

#[tokio::test(start_paused = true)]
async fn a_window_nobody_answers_closes_without_a_word() {
    let script =
        Script::answering(|_| vec![Does::say("Поставить на стол?"), Does::ask("question"), Does::end(Outcome::Asked)]);
    let rig = rig(script, &["сделай игру"]);
    rig.channel.press();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.port.hear(&said(900));
    until("the window opens", || rig.channel.view().state == State::Window).await;
    rig.port.hear(&silence(7_000));
    until("the window closes", || rig.channel.view().state == State::Quiet).await;
    assert!(rig.script.news().iter().any(|news| news.what == Told::NoAnswer { question: "question".into() }));
    assert_eq!(rig.script.phrases().len(), 1, "nothing more went to Thinking");
}

#[tokio::test(start_paused = true)]
async fn a_station_s_word_in_the_window_is_the_station_s() {
    let script =
        Script::answering(|_| vec![Does::say("Что-то ещё?"), Does::ask("question"), Does::end(Outcome::Asked)]);
    let rig = rig(script, &["сделай игру", "Алиса включи музыку"]);
    rig.channel.press();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.port.hear(&said(900));
    until("the window opens", || rig.channel.view().state == State::Window).await;
    rig.port.hear(&said(900));
    until("the window closes", || rig.channel.view().state == State::Quiet).await;
    assert_eq!(rig.script.phrases().len(), 1, "«Алиса, …» heard by itself is the station's");
}

#[tokio::test(start_paused = true)]
async fn a_press_while_speaking_cuts_the_answer_and_listens() {
    let script = Script::answering(|_| {
        vec![Does::say("Раз."), Does::say("Два."), Does::say("Три."), Does::end(Outcome::Answered)]
    });
    let rig = rig(script, &["посчитай", "хватит"]);
    *rig.port.speed.lock().unwrap() = 50.0;
    rig.channel.press();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.port.hear(&said(700));
    until_in(&rig, "it speaks", || rig.channel.view().state == State::Speaking).await;
    rig.channel.press();
    until("it listens again", || rig.channel.view().state == State::Listening).await;
    let stop = rig.script.news().into_iter().find_map(|news| match news.what {
        Told::Stop { why, .. } => Some(why),
        _ => None,
    });
    assert_eq!(stop, Some(StopWhy::Word), "Thinking hears that its answer was cut");
    assert!(!rig.script.news().iter().any(|news| matches!(news.what, Told::Said { .. })), "a cut answer is not said");
}

#[tokio::test(start_paused = true)]
async fn a_hanging_thought_waits_and_the_next_press_goes_on_with_it() {
    let rig = rig(Script::saying("Делаю."), &["сделай виджет чтобы", "показывал курс доллара"]);
    // The pause judged by «Чутьё» is the word's (and the window's): a thought the key began is the key's to end.
    rig.channel.woken(Woken { word: "Компьютер".into(), heard: Vec::new(), agent: None, before: Vec::new() });
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.port.hear(&[silence(400), voice(900), silence(4_200)].concat());
    until("«Жду.»", || spoken(&rig).iter().any(|line| line == "Жду.")).await;
    until("quiet, the thought parked", || rig.channel.view().state == State::Quiet).await;
    assert_eq!(rig.channel.view().parked.as_deref(), Some("сделай виджет чтобы"));
    assert!(rig.script.phrases().is_empty(), "nothing went to Thinking yet");
    rig.channel.press();
    until("the microphone opens again", || rig.port.capturing().is_some()).await;
    rig.port.hear(&said(1000));
    until("the whole thought reaches Thinking", || rig.script.phrases().len() == 1).await;
    assert_eq!(rig.script.phrases()[0].text, "сделай виджет чтобы показывал курс доллара");
}

#[tokio::test(start_paused = true)]
async fn the_key_let_go_after_holding_ends_the_thought_at_once() {
    let rig = rig(Script::saying("Хорошо."), &["запиши мысль"]);
    rig.channel.press_held();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.port.hear(&[silence(400), voice(1500)].concat());
    tokio::time::sleep(Duration::from_millis(100)).await;
    rig.channel.release(2_000);
    until("the phrase reaches Thinking without the silence", || rig.script.phrases().len() == 1).await;
}

/// 27.11, the trial of main: with the key down, a pause of two seconds sent half the thought.
#[tokio::test(start_paused = true)]
async fn a_held_key_keeps_the_thought_through_a_pause_until_it_is_let_go() {
    let rig = rig(Script::saying("Делаю."), &["сделай виджет чтобы", "показывал курс доллара"]);
    rig.channel.press_held();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.port.hear(&[silence(400), voice(1200), silence(2500), voice(1000), silence(600)].concat());
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(rig.script.phrases().is_empty(), "nothing goes while the key is down");
    rig.channel.release(6_000);
    until_in(&rig, "the whole thought reaches Thinking", || rig.script.phrases().len() == 1).await;
    let text = rig.script.phrases()[0].text.clone();
    assert!(text.contains("чтобы") && text.contains("доллара"), "{text}");
}

fn with_key(rig: &Rig, activation: KeyActivation, auto_end: bool) {
    let voice_settings = TalkVoice { activation, auto_end, ..TalkVoice::default() };
    rig.settings.set(&TALK_VOICE, &voice_settings, None).unwrap();
}

/// Handy's «hold or toggle», a tap, our end of speech off: the thought stays on through its pauses until the next press.
#[tokio::test(start_paused = true)]
async fn a_tap_of_the_key_keeps_listening_until_the_next_press() {
    let rig = rig(Script::saying("Делаю."), &["сделай виджет чтобы", "показывал курс доллара"]);
    with_key(&rig, KeyActivation::HoldOrToggle, false);
    rig.channel.press_held();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.channel.release(120);
    rig.port.hear(&[silence(400), voice(1200), silence(3500), voice(1000), silence(3500)].concat());
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(rig.script.phrases().is_empty(), "a tapped thought waits for the key");
    assert_eq!(rig.channel.view().state, State::Listening);
    rig.channel.press_held();
    until_in(&rig, "the next press ends it", || rig.script.phrases().len() == 1).await;
    assert_eq!(rig.script.phrases()[0].text, "сделай виджет чтобы показывал курс доллара");
}

/// Handy's «push to talk»: every release ends the thought; a tap with nothing said closes the microphone.
#[tokio::test(start_paused = true)]
async fn push_to_talk_ends_at_every_release() {
    let rig = rig(Script::saying("Хорошо."), &[]);
    with_key(&rig, KeyActivation::PushToTalk, true);
    rig.channel.press_held();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.port.hear(&silence(300));
    tokio::time::sleep(Duration::from_millis(50)).await;
    rig.channel.release(120);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(rig.channel.view().state, State::Quiet, "quiet at once");
    assert!(rig.script.phrases().is_empty());
}

/// Handy's «toggle»: letting the key go means nothing; the next press ends the thought.
#[tokio::test(start_paused = true)]
async fn toggle_ignores_the_release_and_ends_at_the_next_press() {
    let rig = rig(Script::saying("Хорошо."), &["запиши мысль"]);
    with_key(&rig, KeyActivation::Toggle, false);
    rig.channel.press_held();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.port.hear(&[silence(400), voice(1200), silence(3000)].concat());
    tokio::time::sleep(Duration::from_millis(200)).await;
    rig.channel.release(2_500);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(rig.script.phrases().is_empty(), "the release ends nothing");
    rig.channel.press_held();
    until_in(&rig, "the press ends it", || rig.script.phrases().len() == 1).await;
}

/// The default (the owner, 27.11): after a tap the thought ends when the person is done, as «Чутьё» judges it.
#[tokio::test(start_paused = true)]
async fn a_tap_ends_by_itself_when_the_person_is_done() {
    let rig = rig(Script::saying("Сейчас пять."), &["который час"]);
    rig.channel.press_held();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.channel.release(120);
    rig.port.hear(&said(900));
    until_in(&rig, "the phrase goes by itself", || rig.script.phrases().len() == 1).await;
}

/// The default: a tapped thought may be ended early by the next press, before its pause.
#[tokio::test(start_paused = true)]
async fn a_press_ends_a_tapped_thought_before_its_pause() {
    let rig = rig(Script::saying("Хорошо."), &["запиши мысль"]);
    rig.channel.press();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.port.hear(&[silence(400), voice(1200)].concat());
    tokio::time::sleep(Duration::from_millis(200)).await;
    rig.channel.press();
    until_in(&rig, "the press sends it", || rig.script.phrases().len() == 1).await;
}

/// 27.11, the owner: macOS asks about the microphone, the hand leaves the key for «Разрешить» — the thought waits for
/// the sound, and once it comes the person just speaks.
#[tokio::test(start_paused = true)]
async fn a_key_let_go_before_any_sound_keeps_listening_for_it() {
    let rig = rig(Script::saying("Сейчас пять."), &["который час"]);
    rig.channel.press_held();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.channel.release(1_500);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(rig.channel.view().state, State::Listening, "still listening: no sound came yet");
    rig.port.hear(&said(900));
    until_in(&rig, "the phrase goes once the sound came", || rig.script.phrases().len() == 1).await;
}

/// The sound came only after a wait (macOS's question answered): «слушаю» sounds again — now it hears.
#[tokio::test(start_paused = true)]
async fn the_first_sound_after_a_wait_brings_the_listening_tone_again() {
    let rig = rig(Script::saying("Сейчас пять."), &["который час"]);
    let listen = tones::listen();
    let tones_played = |rig: &Rig| rig.port.played().iter().filter(|(samples, _)| *samples == listen).count();
    rig.channel.press();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    assert_eq!(tones_played(&rig), 1, "«слушаю» at the press");
    tokio::time::sleep(Duration::from_secs(5)).await;
    rig.port.hear(&said(900));
    until_in(&rig, "the phrase goes", || rig.script.phrases().len() == 1).await;
    assert_eq!(tones_played(&rig), 2, "«слушаю» again once the sound came");
}

/// 27.11, the trial of main: with macOS's question about the microphone open, no frames came and «Слушаю» stayed on.
#[tokio::test(start_paused = true)]
async fn a_microphone_that_gives_nothing_stops_the_listening() {
    let rig = rig(Script::saying("Хорошо."), &[]);
    rig.channel.press();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    // Before its first sound the ear waits a minute (macOS's question is read, the hand goes to «Разрешить»).
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(rig.channel.view().state, State::Listening, "half a minute without sound: still listening");
    tokio::time::sleep(Duration::from_secs(31)).await;
    assert_eq!(rig.channel.view().state, State::Quiet, "a minute without sound: it stops");
    assert!(rig.script.phrases().is_empty());
}

fn ready_notice() -> Outside {
    Outside::new(Unasked::Notice(Notice {
        notice: "work:w-1".into(),
        text: "Курс валют: готово. На стол или примерить?".into(),
        speaker: None,
        why: NoticeWhy::WorkQuestion,
        work: Some("w-1".into()),
        question: Some("ready".into()),
        urgency: Urgency::Now,
        to: None,
        until: None,
        expects: Expects::Free,
    }))
}

#[tokio::test(start_paused = true)]
async fn the_computer_says_its_own_ready_and_listens_for_the_answer() {
    let rig = rig(Script::saying("На столе."), &["да"]);
    until("the channel listens to notices", || rig.channel.view().state == State::Quiet).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    rig.script.notice(ready_notice());
    until("the line is said", || spoken(&rig).iter().any(|line| line.starts_with("Курс валют"))).await;
    until("the window opens for the answer", || rig.channel.view().state == State::Window).await;
    assert_eq!(rig.channel.view().why.as_deref(), Some("ready"));
    assert!(rig.script.news().iter().any(|news| matches!(
        &news.what,
        Told::Delivered { notice: Some(notice), outcome: Delivery::Said, .. } if notice == "work:w-1"
    )));
    rig.port.hear(&said(500));
    until("«да» reaches Thinking as the answer", || rig.script.phrases().len() == 1).await;
    let answer = rig.script.phrases().remove(0);
    assert_eq!((answer.context.answer_to.as_deref(), answer.context.after), (Some("ready"), Some(After::Notice)));
}

#[tokio::test(start_paused = true)]
async fn in_a_call_the_ready_is_a_notification_and_thinking_hears_it_was_shown() {
    let rig = rig(Script::saying("—"), &[]);
    rig.port.set_busy(true);
    tokio::time::sleep(Duration::from_millis(50)).await;
    rig.script.notice(ready_notice());
    until("a notification", || !rig.port.notified().is_empty()).await;
    assert!(spoken(&rig).is_empty(), "the call is not spoken over");
    assert!(rig
        .script
        .news()
        .iter()
        .any(|news| matches!(&news.what, Told::Delivered { outcome: Delivery::Shown, .. })));
}

#[tokio::test(start_paused = true)]
async fn speaking_over_the_answer_cuts_it_once_the_echo_check_passed() {
    let script = Script::answering(|phrase| {
        if phrase.context.over.is_some() {
            vec![Does::end(Outcome::Dropped)]
        } else {
            vec![
                Does::say("Солнечная система — это Солнце и всё, что вращается вокруг него."),
                Does::end(Outcome::Answered),
            ]
        }
    });
    let rig = rig(script, &["расскажи про солнечную систему", "хватит"]);
    let voice_settings = TalkVoice { echo: Some(EchoCheck { ok: true, at: 1 }), ..TalkVoice::default() };
    rig.settings.set(&TALK_VOICE, &voice_settings, None).unwrap();
    // The answer plays for 10 s.
    *rig.port.speed.lock().unwrap() = 100.0;
    rig.channel.press();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.port.hear(&said(1200));
    until_in(&rig, "it speaks and listens over itself", || {
        rig.channel.view().state == State::Speaking && rig.port.capturing() == Some(true)
    })
    .await;
    rig.port.hear(&[silence(1_200), voice(800), silence(600)].concat());
    until("the phrase over the answer reaches Thinking", || rig.script.phrases().len() == 2).await;
    let over = rig.script.phrases().remove(1);
    assert_eq!(over.opened, Opened::Window);
    assert!(over.context.over.is_some(), "said over our speech");
    assert!(rig.script.news().iter().any(|news| matches!(news.what, Told::Stop { why: StopWhy::Over, .. })));
    let _ = &rig.hear;
}

#[tokio::test(start_paused = true)]
async fn its_own_echo_does_not_cut_the_answer() {
    let rig = rig(Script::saying("Солнце — звезда."), &["что такое солнце", "солнце звезда"]);
    let voice_settings = TalkVoice { echo: Some(EchoCheck { ok: true, at: 1 }), ..TalkVoice::default() };
    rig.settings.set(&TALK_VOICE, &voice_settings, None).unwrap();
    *rig.port.speed.lock().unwrap() = 100.0;
    rig.channel.press();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.port.hear(&said(900));
    until_in(&rig, "it speaks", || rig.channel.view().state == State::Speaking && rig.port.capturing() == Some(true))
        .await;
    rig.port.hear(&[silence(1_200), voice(800), silence(600)].concat());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!rig.script.news().iter().any(|news| matches!(news.what, Told::Stop { .. })), "the echo is ours");
    assert_eq!(rig.script.phrases().len(), 1);
    rig.hear.more("");
}

/// The echo check (К7) as the panel asks for it: the computer says its phrase, the microphone gives `sound` meanwhile.
async fn echo_check(rig: &Rig, sound: Vec<i16>) -> Result<Checked, String> {
    let channel = rig.channel.clone();
    let checking = tokio::spawn(async move { channel.check().await });
    until_in(rig, "the check listens while it speaks", || rig.port.capturing() == Some(true)).await;
    rig.port.hear(&sound);
    checking.await.unwrap()
}

#[tokio::test(start_paused = true)]
async fn the_echo_check_passes_when_nothing_of_the_computer_is_heard() {
    // 27.11: a microphone that cancels the echo fully gives «Слух» nothing to hear — that failed the check.
    let rig = rig(Script::saying("—"), &[""]);
    // The check's phrase plays for 2 s.
    *rig.port.speed.lock().unwrap() = 20.0;
    let checked = echo_check(&rig, silence(1_500)).await.expect("checked");
    assert!(checked.ok, "nothing heard is the echo cancelled");
    assert!(rig.settings.get(&TALK_VOICE, None).unwrap().echo.is_some_and(|echo| echo.ok));
}

#[tokio::test(start_paused = true)]
async fn the_echo_check_fails_when_the_computer_hears_its_own_phrase() {
    let rig = rig(Script::saying("—"), &["проверяю слышу ли я себя раз два три"]);
    *rig.port.speed.lock().unwrap() = 20.0;
    let checked = echo_check(&rig, voice(1_500)).await.expect("checked");
    assert!(!checked.ok, "its own words came back");
    assert!(rig.settings.get(&TALK_VOICE, None).unwrap().echo.is_some_and(|echo| !echo.ok));
}

#[tokio::test(start_paused = true)]
async fn an_echo_check_the_microphone_gave_no_sound_to_is_no_check() {
    let rig = rig(Script::saying("—"), &[""]);
    let checked = rig.channel.check().await;
    assert!(checked.is_err(), "no sound is no check: {checked:?}");
    assert!(rig.settings.get(&TALK_VOICE, None).unwrap_or_default().echo.is_none(), "nothing kept");
}

#[test]
fn what_the_computer_says_never_holds_a_station_s_word() {
    let others = vec!["Алиса".to_string()];
    assert_eq!(speakable("Скажите «Алиса, стоп».", &others), "Скажите «колонка, стоп».");
    assert_eq!(speakable("Готово.", &others), "Готово.");
}

#[test]
fn the_word_is_cut_off_the_thought_it_woke() {
    let woken = Woken {
        word: "Компьютер".into(), heard: vec!["компутер".into()], agent: None, before: Vec::new()
    };
    assert_eq!(strip_word("Компьютер, который час", Some(&woken)), "который час");
    assert_eq!(strip_word("компутер включи свет", Some(&woken)), "включи свет");
    assert_eq!(strip_word("который час", Some(&woken)), "который час");
}

// ── the trip (owner 27.11: the orbit follows any ear, not only a station) ────────────────────────────────────────

/// The phases of a trip's steps, in their order.
fn phases(trip: &RequestView) -> Vec<RequestPhase> {
    trip.steps.iter().map(|step| step.phase).collect()
}

#[tokio::test(start_paused = true)]
async fn a_question_on_the_computer_s_ear_is_one_trip_from_the_press_to_the_last_word() {
    let rig = rig(Script::saying("Солнце — звезда."), &["что такое солнце"]);
    rig.channel.press();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    let going = rig.trips.all();
    assert_eq!(going.len(), 1, "the press begins the trip");
    assert_eq!(going[0].phase, Some(RequestPhase::Listen));
    rig.port.hear(&said(900));
    until_in(&rig, "the answer is said and the trip ends", || {
        rig.trips.all().first().is_some_and(|trip| trip.status != RequestStatus::Active)
    })
    .await;
    let trip = rig.trips.all().remove(0);
    assert_eq!((trip.device.as_str(), trip.kind), (DEVICE, RequestKind::Voice));
    assert_eq!(trip.status, RequestStatus::Answered);
    assert_eq!(trip.text.as_deref(), Some("что такое солнце"));
    assert_eq!(trip.answer.as_deref(), Some("Солнце — звезда."));
    assert_eq!(phases(&trip), vec![RequestPhase::Listen, RequestPhase::Recognize, RequestPhase::Speak]);
    assert_eq!(*rig.trips.offered.lock().unwrap(), vec![trip.id.clone()], "Thinking here goes on in it");
}

#[tokio::test(start_paused = true)]
async fn a_press_nobody_speaks_after_ends_its_trip_silent_and_an_unanswered_window_makes_none() {
    let rig = rig(Script::saying("Сколько?"), &[]);
    rig.channel.press();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.port.hear(&silence(9_000));
    until_in(&rig, "the trip ends", || {
        rig.trips.all().first().is_some_and(|trip| trip.status != RequestStatus::Active)
    })
    .await;
    assert_eq!(rig.trips.all()[0].status, RequestStatus::Silent);
    assert!(rig.trips.offered.lock().unwrap().is_empty(), "nothing went to Thinking");
}

#[tokio::test(start_paused = true)]
async fn thinking_on_another_gateway_is_named_by_the_ear_s_trip() {
    // The hub of a Турбо thinks in a request of its own: the ear keeps the trip and names that one.
    let script = Script::answering(|_| {
        vec![
            Does::Event(What::Begun { request: "hub-7".into(), by: "cloud".into() }),
            Does::say("Сейчас восемь."),
            Does::end(Outcome::Answered),
        ]
    });
    let rig = rig(script, &["который час"]);
    rig.channel.press();
    until("the microphone opens", || rig.port.capturing().is_some()).await;
    rig.port.hear(&said(900));
    until_in(&rig, "the trip ends", || {
        rig.trips.all().first().is_some_and(|trip| trip.status != RequestStatus::Active)
    })
    .await;
    let trip = rig.trips.all().remove(0);
    assert_eq!(trip.thought_in.as_deref(), Some("hub-7"));
    assert_eq!(
        phases(&trip),
        vec![RequestPhase::Listen, RequestPhase::Recognize, RequestPhase::Think, RequestPhase::Speak]
    );
    let think = trip.steps.iter().find(|step| step.phase == RequestPhase::Think).unwrap();
    assert_eq!(think.by, Some(journal::Place::Cloud));
    assert_eq!(trip.status, RequestStatus::Answered);
}
