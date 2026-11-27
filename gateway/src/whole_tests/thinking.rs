//! `OurThinking` on a whole gateway (docs/plans/thinking.md, M0): a scripted ear sends the door's phrases and reads the
//! events back. For the same talks the talk layer's stand-in (`TalkThinking`) and ours give the same events; the
//! contract's rules hold for both; the setting `implementation` switches them from the next phrase.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;

use crate::app::App;
use crate::config::{Config, Env};
use crate::talk::thinking::TalkThinking;
use duowave_brain::door::{
    After, Budget, By, Context, Event, Kind, Opened, Outcome, Phrase, Place, Thinking, To, What, Who, V,
};

fn env(dir: &std::path::Path, provider: &str) -> Env {
    let text = |sub: &str| dir.join(sub).to_string_lossy().into_owned();
    Env::from_pairs([
        ("STATE_DIR", String::new()),
        ("DIALOG_PROVIDER", provider.into()),
        ("PACKAGES_DIR", text("packages")),
        ("DATA_DIR", text("data")),
        ("VOICE_LAB_DIR", text("voice-lab")),
        ("TTS_MODELS_DIR", text("voices")),
    ])
}

fn app(dir: &std::path::Path, provider: &str) -> Arc<App> {
    App::build(Config::for_tests(&env(dir, provider))).unwrap()
}

fn talk_of(app: &Arc<App>) -> Arc<dyn Thinking> {
    use crate::platform::http::Provides;
    let parts: crate::talk::Parts = app.provide();
    Arc::new(TalkThinking::new(parts, app.personas.clone(), crate::links::thinking::people(&app.access)))
}

/// A phrase of the computer's ear.
pub fn phrase(turn: &str, text: &str, opened: Opened, context: Context) -> Phrase {
    Phrase {
        v: V,
        turn: turn.into(),
        text: text.into(),
        lang: None,
        alts: Vec::new(),
        who: Who::default(),
        to: To { agent: None, by: By::Button, name: Some("app".into()) },
        place: Place { device: "computer".into(), room: None, kind: Kind::Computer, screen: true, speaker: true },
        hands: Vec::new(),
        opened,
        context,
        budget: Budget { voice: true, first_word_ms: Some(1500), limit_ms: None },
        heard: None,
        at: None,
    }
}

/// The scripted ear: a phrase through the door, its events to `end` (or the stream's end).
pub async fn talk(thinking: &dyn Thinking, phrase: Phrase) -> Vec<Event> {
    let events = thinking.phrase(phrase);
    tokio::time::timeout(Duration::from_secs(10), events.collect::<Vec<_>>()).await.expect("the answer ends")
}

/// What an answer is, without its times and its request's id: the events' kinds and words.
fn shape(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .map(|event| match &event.what {
            What::Begun { by, .. } => format!("begun {by}"),
            What::Say { text } => format!("say {text}"),
            What::Wait { why, name, .. } => format!("wait {why:?} {}", name.clone().unwrap_or_default()),
            What::Ask { question, expects, .. } => format!("ask {question} {expects:?}"),
            What::Hands { hand, .. } => format!("hands {hand}"),
            What::Work { title, state, .. } => format!("work {title:?} {state:?}"),
            What::Handoff { .. } => "handoff".into(),
            What::Follow { capability, ttl_ms } => format!("follow {capability} {ttl_ms}"),
            What::End { outcome, reason, said, .. } => format!("end {outcome:?} {reason:?} {said}"),
            What::Unknown => "unknown".into(),
        })
        .collect()
}

/// The contract's rules for any answer: every event carries the phrase's turn and one speaker, the last one is `end`,
/// once, and `begun` comes first when it comes.
fn holds_the_contract(turn: &str, events: &[Event]) {
    assert!(!events.is_empty(), "an answer says something");
    assert!(events.iter().all(|event| event.turn == turn), "every event carries its turn: {events:?}");
    let speakers: std::collections::HashSet<_> = events.iter().map(|event| event.speaker.clone()).collect();
    assert_eq!(speakers.len(), 1, "one agent speaks a turn: {events:?}");
    let ends = events.iter().filter(|event| matches!(event.what, What::End { .. })).count();
    assert_eq!(ends, 1, "one end: {events:?}");
    assert!(matches!(events.last().unwrap().what, What::End { .. }), "the end is the last: {events:?}");
    if let Some(begun) = events.iter().position(|event| matches!(event.what, What::Begun { .. })) {
        assert_eq!(begun, 0, "begun comes first: {events:?}");
    }
}

/// The talks of the stand-in's and the channel's checks: a question to the model, the works' words, a phrase the
/// closed window drops, a word that closes the window, a reply after a work's line.
fn talks() -> Vec<Phrase> {
    let window = |after| Context { after: Some(after), ..Context::default() };
    vec![
        phrase("t-1", "Привет, как дела?", Opened::Button, Context::default()),
        phrase("t-2", "как там виджет", Opened::Button, Context::default()),
        phrase("t-3", "а у соседей ремонт", Opened::Window, window(After::Work)),
        phrase("t-4", "хватит", Opened::Window, window(After::Ask)),
        phrase("t-5", "что ты умеешь", Opened::Text, Context::default()),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ours_gives_the_same_events_as_the_stand_in_for_the_same_talks() {
    let (one, two) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (theirs_app, ours_app) = (app(one.path(), "echo"), app(two.path(), "echo"));
    let (theirs, ours) = (talk_of(&theirs_app), ours_app.thinking.clone());
    for said in talks() {
        let turn = said.turn.clone();
        let (a, b) = (talk(&*theirs, said.clone()).await, talk(&*ours, said).await);
        holds_the_contract(&turn, &a);
        holds_the_contract(&turn, &b);
        assert_eq!(shape(&a), shape(&b), "turn {turn}");
    }
    // The brain's answer is one request of the journal, as the stand-in's.
    let first = talk(&*ours, phrase("t-6", "Расскажи сказку", Opened::Button, Context::default())).await;
    let What::Begun { request, by } = &first[0].what else { panic!("begun first: {first:?}") };
    assert_eq!(by, "home", "the echo model is at home");
    let view = ours_app
        .requests
        .active_views()
        .into_iter()
        .chain(ours_app.requests.recent_views(10))
        .find(|view| &view.id == request);
    let view = view.expect("the turn's request is in the journal");
    assert_eq!(view.device, "computer");
    assert_eq!(view.text.as_deref(), Some("Расскажи сказку"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_model_both_say_so_and_fail_with_no_model() {
    let (one, two) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (theirs_app, ours_app) = (app(one.path(), "none"), app(two.path(), "none"));
    let said = phrase("t-1", "Привет", Opened::Button, Context::default());
    let a = talk(&*talk_of(&theirs_app), said.clone()).await;
    let b = talk(&*ours_app.thinking.clone(), said).await;
    holds_the_contract("t-1", &b);
    assert_eq!(shape(&a), shape(&b));
    assert!(
        matches!(
            b.last().unwrap().what,
            What::End { outcome: Outcome::Failed, reason: Some(duowave_brain::door::Failure::NoModel), .. }
        ),
        "{b:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_setting_sends_the_next_phrase_to_the_stand_in_and_back() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "echo");
    let door = crate::links::thinking::door(&app, talk_of(&app));
    let first = talk(&*door, phrase("t-1", "Привет", Opened::Button, Context::default())).await;
    assert_eq!(
        app.thinking.settings().implementation(crate::thinking::settings::Where::of(Kind::Computer, true)),
        crate::thinking::settings::Implementation::Ours
    );
    holds_the_contract("t-1", &first);
    let mut settings = crate::thinking::Settings::default();
    settings.implementation = crate::thinking::settings::Varies::One(crate::thinking::settings::Implementation::Talk);
    app.settings.set(&crate::thinking::THINKING, &settings, None).unwrap();
    let second = talk(&*door, phrase("t-2", "Привет", Opened::Button, Context::default())).await;
    holds_the_contract("t-2", &second);
    assert_eq!(shape(&first), shape(&second), "the same answer from the stand-in");
    app.settings.forget(&crate::thinking::THINKING).unwrap();
    let third = talk(&*door, phrase("t-3", "Привет", Opened::Button, Context::default())).await;
    assert_eq!(shape(&first), shape(&third));
}

/// A model that writes nothing for half a minute, unless it is stopped.
struct Slow;

impl crate::dialog::llm::LlmClient for Slow {
    fn description(&self) -> String {
        "slow".into()
    }

    fn tools(&self) -> bool {
        false
    }

    fn stream(
        &self,
        _request: crate::dialog::llm::LlmRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> crate::dialog::llm::LlmStream {
        futures::stream::once(async move {
            tokio::select! {
                _ = cancel.cancelled() => Err(anyhow::anyhow!("stopped")),
                _ = tokio::time::sleep(Duration::from_secs(30)) => Ok(crate::dialog::llm::LlmEvent::Text("поздно".into())),
            }
        })
        .boxed()
    }
}

/// Our Thinking with a model of the test's own.
fn ours_with(app: &Arc<App>, client: Arc<dyn crate::dialog::llm::LlmClient>) -> Arc<crate::thinking::OurThinking> {
    use crate::platform::http::Provides;
    let parts =
        crate::talk::Parts { dialog: crate::dialog::dialog_service::tests::with_client(client), ..app.provide() };
    crate::links::thinking::ours(
        parts,
        app.personas.clone(),
        &app.access,
        app.settings.clone(),
        app.requests.clone(),
        None,
        None,
        None,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_cancels_the_turn_and_its_request_ends_cancelled() {
    use duowave_brain::door::{News, StopWhy, Told};
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "echo");
    let ours = ours_with(&app, Arc::new(Slow));
    let mut events = ours.phrase(phrase("t-1", "Подумай подольше", Opened::Button, Context::default()));
    let What::Begun { request, .. } = events.next().await.unwrap().what else { panic!("begun first") };
    assert_eq!(ours.going(), 1);
    ours.tell(News::new(Told::Stop { turn: "t-1".into(), why: StopWhy::Word, heard: None }));
    let rest = tokio::time::timeout(Duration::from_secs(5), events.collect::<Vec<_>>()).await.expect("it ends at once");
    assert!(rest.iter().all(|event| !matches!(event.what, What::Say { .. })), "nothing is said after a stop: {rest:?}");
    let view = app.requests.recent_views(10).into_iter().find(|view| view.id == request).expect("the request ended");
    assert_eq!(view.status, crate::core::requests::RequestStatus::Cancelled);
    assert_eq!(ours.going(), 0);
}

/// A model that answers in words with its usage: what the line's round must show.
fn counted(app: &Arc<App>) -> (Arc<crate::thinking::OurThinking>, Arc<crate::dialog::dialog_service::tests::Scripted>) {
    use crate::dialog::llm::{AssistantTurn, LlmEvent, LlmUsage};
    use crate::platform::http::Provides;
    let (llm, dialog) = crate::dialog::dialog_service::tests::scripted(Box::new(|_, _| {
        let usage = LlmUsage { input: 3000, output: 40, cache_read: Some(2500), cache_write: None };
        vec![
            Ok(LlmEvent::Text("Сейчас солнечно.".into())),
            Ok(LlmEvent::Turn(AssistantTurn {
                content: "Сейчас солнечно.".into(),
                usage: Some(usage),
                ..AssistantTurn::default()
            })),
        ]
    }));
    let parts = crate::talk::Parts { dialog, ..app.provide() };
    let ours = crate::links::thinking::ours(
        parts,
        app.personas.clone(),
        &app.access,
        app.settings.clone(),
        app.requests.clone(),
        None,
        None,
        None,
    );
    (ours, llm)
}

/// The line on the journal's row of a request.
fn line_of(app: &Arc<App>, request: &str) -> crate::thinking::line::Line {
    let views = app.requests.history(&crate::requests::service::HistoryQuery::default()).unwrap();
    let view = views.into_iter().find(|view| view.id == request).expect("the request is in the journal");
    serde_json::from_value(view.thinking.expect("its line")).expect("a line")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_writes_its_line_with_the_model_s_tokens_and_what_came_next() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "echo");
    let (ours, _llm) = counted(&app);
    let first = talk(&*ours, phrase("t-1", "Какая погода?", Opened::Button, Context::default())).await;
    let What::Begun { request, .. } = &first[0].what else { panic!("begun first") };
    let line = line_of(&app, request);
    assert_eq!(line.v, crate::thinking::line::V);
    assert_eq!(line.settings, crate::thinking::Settings::default().mark());
    assert_eq!((line.variants.thinking.as_str(), line.variants.card.as_str()), ("ours", "all"));
    assert_eq!((line.kind.as_str(), line.voice, line.end.as_str()), ("computer", true, "answered"));
    assert_eq!(line.rounds.len(), 1);
    let round = &line.rounds[0];
    assert_eq!((round.input, round.out, round.cache_read), (3000, 40, 2500), "the provider's own count");
    assert!(line.card.as_ref().is_some_and(|card| card.tokens.stable > 0), "the card's parts: {line:?}");
    assert!(line.first_word_ms.is_some() && line.budget_ms == Some(1500));
    assert!(line.next.is_none(), "what came next is not known yet");
    // The same words again soon: a repeat, written onto the earlier line.
    talk(&*ours, phrase("t-2", "какая погода", Opened::Button, Context::default())).await;
    let earlier = line_of(&app, request);
    assert_eq!(earlier.next.as_ref().map(|next| next.what.as_str()), Some("repeated"), "{earlier:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cut_answer_gives_next_cut_with_how_much_was_heard() {
    use duowave_brain::door::{News, Played, StopWhy, Told};
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "echo");
    let ours = ours_with(&app, Arc::new(Slow));
    let mut events = ours.phrase(phrase("t-1", "Расскажи длинную сказку", Opened::Button, Context::default()));
    let What::Begun { request, .. } = events.next().await.unwrap().what else { panic!("begun first") };
    ours.tell(News::new(Told::Stop {
        turn: "t-1".into(),
        why: StopWhy::Word,
        heard: Some(Played { chars: Some(42), ms: None }),
    }));
    let _ = tokio::time::timeout(Duration::from_secs(5), events.collect::<Vec<_>>()).await;
    // The line is written once the turn's task ends.
    let line = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let views = app.requests.history(&crate::requests::service::HistoryQuery::default()).unwrap();
            if let Some(line) = views.into_iter().find(|view| view.id == request).and_then(|view| view.thinking) {
                return line;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the line is written");
    let line: crate::thinking::line::Line = serde_json::from_value(line).unwrap();
    let next = line.next.expect("the cut is what came next");
    assert_eq!((next.what.as_str(), next.heard_chars), ("cut", Some(42)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_numbers_come_from_the_lines_of_the_journal() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "echo");
    let (ours, _llm) = counted(&app);
    for (turn, text) in [("t-1", "Привет"), ("t-2", "Как дела?"), ("t-3", "Что нового?")] {
        talk(&*ours, phrase(turn, text, Opened::Button, Context::default())).await;
    }
    let stats = ours.stats(&crate::thinking::stats::Query::default());
    assert_eq!((stats.turns, stats.brain), (3, 3));
    assert_eq!(stats.rounds, Some(1));
    assert_eq!(stats.cache_share, Some((2500.0f64 / 5500.0 * 1000.0).round() / 1000.0));
    assert!(stats.first_word_ms.is_some());
    let typed = ours.stats(&crate::thinking::stats::Query { mode: Some("text".into()), ..Default::default() });
    assert_eq!(typed.turns, 0, "every one was spoken");
}

/// A model that calls the tool a phrase asks for in its first round, then answers in words.
fn calling(app: &Arc<App>) -> (Arc<crate::thinking::OurThinking>, Arc<crate::dialog::dialog_service::tests::Scripted>) {
    use crate::dialog::llm::{AssistantTurn, LlmEvent, LlmMessage, ToolCall};
    use crate::platform::http::Provides;
    let (llm, dialog) = crate::dialog::dialog_service::tests::scripted(Box::new(|_, request| {
        let first = matches!(request.messages.last(), Some(LlmMessage::User(_)));
        let asked = request
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                LlmMessage::User(text) => Some(text.to_lowercase()),
                _ => None,
            })
            .unwrap_or_default();
        // The weather is the app's when «Погода» is installed: its tool takes the place of ours.
        let weather =
            if request.tools.iter().any(|tool| tool.name == "weather_now") { "weather_now" } else { "get_weather" };
        let tool = [("погод", weather), ("календар", "calendar_events"), ("таймер", "set_timer")]
            .into_iter()
            .find(|(word, _)| asked.contains(word))
            .map(|(_, tool)| tool);
        match (first, tool) {
            (true, Some(tool)) => vec![Ok(LlmEvent::Turn(AssistantTurn {
                tool_calls: vec![ToolCall {
                    id: "c-1".into(),
                    name: tool.into(),
                    input: Default::default(),
                    error: None,
                }],
                ..AssistantTurn::default()
            }))],
            _ => crate::dialog::dialog_service::tests::words("Готово."),
        }
    }));
    let parts = crate::talk::Parts { dialog, ..app.provide() };
    let ours = crate::links::thinking::ours(
        parts,
        app.personas.clone(),
        &app.access,
        app.settings.clone(),
        app.requests.clone(),
        None,
        None,
        None,
    );
    (ours, llm)
}

/// A phrase of the kitchen's station, by its word.
fn kitchen(turn: &str, text: &str) -> Phrase {
    Phrase {
        to: To { agent: None, by: By::Word, name: Some("Алиса".into()) },
        place: Place {
            device: "kitchen".into(),
            room: Some("kitchen".into()),
            kind: Kind::Station,
            screen: false,
            speaker: true,
        },
        hands: vec!["volume".into(), "playback".into(), "timers".into()],
        lang: Some("ru".into()),
        ..phrase(turn, text, Opened::Word, Context::default())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_card_in_shadow_hits_misses_or_is_not_needed_and_the_model_is_offered_the_same() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "echo");
    // The model takes the phrases: the station's quick rules would answer the weather by our skill.
    no_quick_rules(&app);
    let (ours, llm) = calling(&app);
    let mut shadows = Vec::new();
    for (turn, text) in [("t-1", "Какая погода?"), ("t-2", "Что у меня в календаре?"), ("t-3", "Привет")]
    {
        let events = talk(&*ours, kitchen(turn, text)).await;
        let What::Begun { request, .. } = &events[0].what else { panic!("begun first: {events:?}") };
        let line = line_of(&app, request);
        let shadow = line.shadow.iter().find(|shadow| shadow.part == "card").cloned().expect("the card in shadow");
        assert_eq!(line.variants.card, "all", "the live card is every tool");
        shadows.push((text, shadow));
    }
    let hits: Vec<_> = shadows.iter().map(|(text, shadow)| (*text, shadow.hit.as_str())).collect();
    assert_eq!(hits, [("Какая погода?", "hit"), ("Что у меня в календаре?", "miss"), ("Привет", "none")]);
    let (_, weather) = &shadows[0];
    let on_card = |name: &str| weather.tools.iter().any(|tool| tool == name);
    assert!((on_card("get_weather") || on_card("weather_now")) && on_card("open"), "{weather:?}");
    // What the model is offered is every tool of the station's turn, as before: no `open`, the commands there.
    let requests = llm.requests.lock().unwrap();
    let offered: Vec<String> = requests[0].tools.iter().map(|tool| tool.name.clone()).collect();
    assert!(!offered.contains(&"open".to_string()), "{offered:?}");
    let weather_offered = offered.iter().any(|name| name == "get_weather" || name == "weather_now");
    assert!(offered.contains(&"set_volume".to_string()) && weather_offered, "{offered:?}");
    assert!(weather.tokens < crate::thinking::card::tools_tokens(&requests[0].tools), "the card is smaller");
}

/// The home's devices as the audio side would tell them: the kitchen's and the bedroom's stations.
struct Rooms;

impl duowave_brain::door::Devices for Rooms {
    fn devices(&self) -> Vec<duowave_brain::door::Device> {
        let station = |device: &str, room: &str| duowave_brain::door::Device {
            device: device.into(),
            room: Some(room.into()),
            kind: Kind::Station,
            screen: false,
            speaker: true,
            hands: ["volume", "playback", "timers", "calls"]
                .iter()
                .map(|group| duowave_brain::door::HandSpec::group(group, group))
                .collect(),
            busy: false,
        };
        let mut kitchen = station("kitchen", "кухня");
        // Hands named one by one: they answer `hands_done`.
        kitchen.hands.push(duowave_brain::door::HandSpec {
            hand: "lamp.toggle".into(),
            group: "lights".into(),
            title: "Лампа".into(),
            args: serde_json::json!({"type": "object", "properties": {"on": {"type": "boolean"}}}),
            risky: false,
            about: Some("Turn the kitchen lamp on or off".into()),
        });
        kitchen.hands.push(duowave_brain::door::HandSpec {
            hand: "door.open".into(),
            group: "door".into(),
            title: "Дверь".into(),
            args: serde_json::json!({"type": "object", "properties": {}}),
            risky: true,
            about: Some("Open the front door".into()),
        });
        vec![kitchen, station("bedroom", "спальня")]
    }
}

/// Ours with the rooms' devices and a model of the test's own.
fn ours_in_rooms(app: &Arc<App>, dialog: Arc<crate::dialog::DialogService>) -> Arc<crate::thinking::OurThinking> {
    use crate::platform::http::Provides;
    let parts = crate::talk::Parts { dialog, ..app.provide() };
    Arc::new(crate::thinking::OurThinking::new(crate::thinking::Deps {
        parts,
        personas: app.personas.clone(),
        people: crate::links::thinking::people(&app.access),
        settings: app.settings.clone(),
        devices: Some(Arc::new(Rooms)),
        lines: Some(app.requests.clone()),
        titles: None,
        characters: None,
    }))
}

/// The scripted ear of a station: a phrase through the door, each hand done as `done` says, to the end.
async fn ear(
    thinking: &crate::thinking::OurThinking,
    phrase: Phrase,
    done: impl Fn(&str, &str, &serde_json::Value) -> Option<(bool, Option<String>)>,
) -> Vec<Event> {
    use duowave_brain::door::{News, Told};
    let mut events = thinking.phrase(phrase);
    let mut heard = Vec::new();
    loop {
        let next = tokio::time::timeout(Duration::from_secs(10), events.next()).await.expect("the answer goes on");
        let Some(event) = next else { break };
        if let What::Hands { call, device, hand, args } = &event.what {
            // A station sends no `hands_done`: its commands go into its answer.
            if let Some((ok, error)) = done(device, hand, args) {
                thinking.tell(News::new(Told::HandsDone { call: call.clone(), ok, note: None, error }));
            }
        }
        let end = matches!(event.what, What::End { .. });
        heard.push(event);
        if end {
            break;
        }
    }
    heard
}

fn hands_of(events: &[Event]) -> Vec<(String, String, serde_json::Value)> {
    events
        .iter()
        .filter_map(|event| match &event.what {
            What::Hands { device, hand, args, .. } => Some((device.clone(), hand.clone(), args.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_quick_rule_goes_as_a_hand_to_the_device_that_heard_or_the_room_it_names() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "echo");
    let (_llm, dialog) = crate::dialog::dialog_service::tests::scripted(Box::new(|_, _| {
        crate::dialog::dialog_service::tests::words("Не знаю.")
    }));
    let ours = ours_in_rooms(&app, dialog);
    // The station's ear sends no `hands_done`: its commands go into its answer.
    let louder = ear(&ours, kitchen("t-1", "громче"), |_, _, _| None).await;
    let hands = hands_of(&louder);
    assert!(!hands.is_empty(), "{louder:?}");
    assert!(hands.iter().all(|(device, ..)| device == "kitchen"), "{hands:?}");
    for (_, hand, args) in &hands {
        let command = crate::ear::hands::command_of(args).expect("the station's own command");
        assert_eq!(hand, command.name());
    }
    assert!(matches!(louder.last().unwrap().what, What::End { outcome: Outcome::Answered, .. }));
    let there = ear(&ours, kitchen("t-2", "в спальне выключи музыку"), |_, _, _| None).await;
    let hands = hands_of(&there);
    assert!(!hands.is_empty(), "{there:?}");
    assert!(hands.iter().all(|(device, ..)| device == "bedroom"), "{hands:?}");
}

/// A model that calls `tool` once, then answers with `words`; its requests are kept.
type Once = (Arc<crate::dialog::dialog_service::tests::Scripted>, Arc<crate::dialog::DialogService>);

fn once(tool: &'static str, words: &'static str) -> Once {
    use crate::dialog::llm::{AssistantTurn, LlmEvent, LlmMessage, ToolCall};
    crate::dialog::dialog_service::tests::scripted(Box::new(move |_, request| match request.messages.last() {
        Some(LlmMessage::User(_)) if request.tools.iter().any(|offered| offered.name == tool) => {
            vec![Ok(LlmEvent::Turn(AssistantTurn {
                tool_calls: vec![ToolCall {
                    id: "c-1".into(),
                    name: tool.into(),
                    input: Default::default(),
                    error: None,
                }],
                ..AssistantTurn::default()
            }))]
        }
        _ => crate::dialog::dialog_service::tests::words(words),
    }))
}

/// The fast route without the quick rules: the model takes the phrase.
fn no_quick_rules(app: &Arc<App>) {
    let mut settings = crate::thinking::Settings::default();
    settings.fast.steps.retain(|step| step != "quick");
    app.settings.set(&crate::thinking::THINKING, &settings, None).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hand_s_error_reaches_the_model_in_words() {
    use crate::dialog::llm::LlmMessage;
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "echo");
    no_quick_rules(&app);
    let (llm, dialog) = once("lamp_toggle", "Лампа не ответила.");
    let ours = ours_in_rooms(&app, dialog);
    let mut lamp = kitchen("t-1", "включи лампу");
    lamp.hands.push("lights".into());
    let events = ear(&ours, lamp, |device, hand, _| {
        assert_eq!((device, hand), ("kitchen", "lamp.toggle"));
        Some((false, Some("лампа не в сети".into())))
    })
    .await;
    assert_eq!(hands_of(&events).len(), 1, "{events:?}");
    assert!(
        events.iter().any(|event| matches!(&event.what, What::Say { text } if text == "Лампа не ответила.")),
        "{events:?}"
    );
    let requests = llm.requests.lock().unwrap();
    let results: Vec<String> = requests
        .iter()
        .flat_map(|request| request.messages.iter())
        .filter_map(|message| match message {
            LlmMessage::Tool(results) => Some(results.iter().map(|result| result.content.clone()).collect::<Vec<_>>()),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(results.iter().any(|result| result.contains("лампа не в сети")), "{results:?}");
    let offered: Vec<String> = requests[0].tools.iter().map(|tool| tool.name.clone()).collect();
    assert!(offered.contains(&"set_volume".to_string()), "the place's hands are tools: {offered:?}");
    assert!(!offered.contains(&"bluetooth".to_string()), "only the hands the place has: {offered:?}");
    let lamp = requests[0].tools.iter().find(|tool| tool.name == "lamp_toggle").expect("the named hand");
    assert_eq!(lamp.description, "Turn the kitchen lamp on or off", "its English line for the model");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_a_device_with_its_own_assistant_gets_a_handoff() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "none");
    let ours = app.thinking.clone();
    let without = talk(&*ours, kitchen("t-1", "расскажи анекдот")).await;
    assert!(!without.iter().any(|event| matches!(event.what, What::Handoff { .. })), "{without:?}");
    let mut with_alice = kitchen("t-2", "расскажи анекдот");
    with_alice.hands.push("assistant".into());
    let with = talk(&*ours, with_alice).await;
    // No model may answer: the assistant takes it as a conversation (the journal's capability).
    assert!(
        with.iter().any(|event| matches!(&event.what, What::Handoff { capability: Some(capability), text: None }
            if capability == "chat")),
        "{with:?}"
    );
    assert!(matches!(with.last().unwrap().what, What::End { outcome: Outcome::HandedOff, .. }));
}

/// A skill's question on a station, in the order the station's chain has it: «listen after» before its words, its
/// dialog followed — the reply in the window, with no wake word, is the skill's again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_skill_s_question_listens_before_its_words_and_its_dialog_takes_the_reply() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "echo");
    let (_llm, dialog) = crate::dialog::dialog_service::tests::scripted(Box::new(|_, _| {
        crate::dialog::dialog_service::tests::words("Не знаю.")
    }));
    let ours = ours_in_rooms(&app, dialog);
    let asked = ear(&ours, kitchen("t-1", "поставь таймер на чай"), |_, _, _| None).await;
    let listens = asked.iter().position(
        |event| matches!(&event.what, What::Ask { question, .. } if question == crate::talk::thinking::STATION_LISTENS),
    );
    let says = asked.iter().position(|event| matches!(event.what, What::Say { .. }));
    assert!(matches!((listens, says), (Some(listens), Some(says)) if listens < says), "{asked:?}");
    assert!(
        asked.iter().any(|event| matches!(&event.what, What::Follow { capability, ttl_ms }
            if capability == "timers" && *ttl_ms > 0)),
        "{asked:?}"
    );
    assert!(matches!(asked.last().unwrap().what, What::End { outcome: Outcome::Asked, .. }), "{asked:?}");
    let mut reply = kitchen("t-2", "на две минуты");
    reply.opened = Opened::Window;
    reply.to.by = By::Window;
    let set = ear(&ours, reply, |_, _, _| None).await;
    assert!(!hands_of(&set).is_empty(), "the timer's own dialog took the reply: {set:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_risky_hand_is_asked_first_and_runs_after_yes() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "echo");
    no_quick_rules(&app);
    let ours = ours_in_rooms(&app, once("door_open", "Открыть входную дверь?").1);
    let mut open = kitchen("t-1", "открой дверь");
    open.hands.push("door".into());
    let asked = ear(&ours, open.clone(), |_, _, _| panic!("not before «да»")).await;
    assert!(hands_of(&asked).is_empty(), "{asked:?}");
    assert!(asked.iter().any(|event| matches!(&event.what, What::Say { text } if text == "Открыть входную дверь?")));
    let mut yes = open;
    yes.turn = "t-2".into();
    yes.text = "да".into();
    yes.opened = Opened::Window;
    yes.context = Context { answer_to: Some("question".into()), after: Some(After::Ask), ..Context::default() };
    let opened = ear(&ours, yes, |device, hand, _| {
        assert_eq!((device, hand), ("kitchen", "door.open"));
        Some((true, None))
    })
    .await;
    assert_eq!(hands_of(&opened).len(), 1, "{opened:?}");
    assert!(opened.iter().any(|event| matches!(&event.what, What::Say { text } if text == "Готово.")), "{opened:?}");
}

/// The card live for typed phrases, the model at home too (D7 off for the test).
fn card_for_text(app: &Arc<App>, on: bool) {
    let mut settings = crate::thinking::Settings::default();
    let variant =
        if on { crate::thinking::settings::CardVariant::Card } else { crate::thinking::settings::CardVariant::All };
    settings.card.variant = crate::thinking::settings::Varies::By(std::collections::BTreeMap::from([
        ("default".to_string(), crate::thinking::settings::CardVariant::All),
        ("text".to_string(), variant),
    ]));
    settings.card.local_all = false;
    app.settings.set(&crate::thinking::THINKING, &settings, None).unwrap();
}

/// A model that opens `group` in its first round, calls `tool` once it is offered, then answers in words.
fn opening(group: &'static str, tool: &'static str) -> Once {
    use crate::dialog::llm::{AssistantTurn, LlmEvent, LlmMessage, ToolCall};
    crate::dialog::dialog_service::tests::scripted(Box::new(move |_, request| {
        let offered = |name: &str| request.tools.iter().any(|offered| offered.name == name);
        let called = |name: &str| {
            request.messages.iter().any(|message| {
                matches!(message, LlmMessage::Assistant(turn) if turn.tool_calls.iter().any(|call| call.name == name))
            })
        };
        let call = |name: &str, input: serde_json::Value| {
            vec![Ok(LlmEvent::Turn(AssistantTurn {
                tool_calls: vec![ToolCall {
                    id: name.into(),
                    name: name.into(),
                    input: input.as_object().cloned().unwrap(),
                    error: None,
                }],
                ..AssistantTurn::default()
            }))]
        };
        if offered(tool) && !called(tool) {
            call(tool, serde_json::json!({"text": "чайник", "in_seconds": 60}))
        } else if offered("open") && !called("open") {
            call("open", serde_json::json!({"groups": [group]}))
        } else {
            crate::dialog::dialog_service::tests::words("Готово.")
        }
    }))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_model_opens_a_group_and_calls_its_tool_in_the_next_round() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "echo");
    card_for_text(&app, true);
    let (llm, dialog) = opening("reminders", "set_reminder");
    let ours = ours_in_rooms(&app, dialog);
    let mut typed = phrase("t-1", "Не дай мне забыть про чайник через минуту", Opened::Text, Context::default());
    typed.budget.voice = false;
    let events = talk(&*ours, typed).await;
    let What::Begun { request, .. } = &events[0].what else { panic!("begun first: {events:?}") };
    let requests = llm.requests.lock().unwrap();
    let names = |index: usize| requests[index].tools.iter().map(|tool| tool.name.clone()).collect::<Vec<_>>();
    assert!(
        names(0).contains(&"open".to_string()) && !names(0).contains(&"set_reminder".to_string()),
        "{:?}",
        names(0)
    );
    assert!(names(1).contains(&"set_reminder".to_string()), "opened for the next round: {:?}", names(1));
    assert!(requests[0].system.contains("- reminders:"), "the catalog is on the card");
    let line = line_of(&app, request);
    assert_eq!(line.variants.card, "card");
    assert_eq!(line.rounds[0].opened, ["reminders"]);
    assert!(line.rounds.iter().any(|round| round.calls.contains(&"set_reminder".to_string())), "{line:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_panel_s_chat_answers_on_the_card_and_without_it_from_the_next_phrase() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "echo");
    card_for_text(&app, true);
    let (llm, dialog) = once("never_called", "Привет!");
    let ours = ours_in_rooms(&app, dialog);
    let principal = app.access.household.speaker(None);
    let typed = |text: &str| crate::talk::Turn {
        text: text.into(),
        origin: crate::talk::Origin::computer(crate::talk::Activator::Text),
        heard: None,
        voice: false,
        fresh: false,
        tools: crate::skills::tools::ToolsKind::Conversation,
    };
    let mut first = crate::thinking::panel::talk(&ours, &principal, &typed("Привет"));
    let mut seen = Vec::new();
    while let Some(event) = first.events.recv().await {
        seen.push(event);
    }
    assert!(matches!(seen.first(), Some(crate::talk::TalkEvent::Start { .. })), "{seen:?}");
    assert!(
        seen.iter().any(|event| matches!(event, crate::talk::TalkEvent::Phrase { text } if text == "Привет!")),
        "{seen:?}"
    );
    assert!(matches!(seen.last(), Some(crate::talk::TalkEvent::Done { .. })), "{seen:?}");
    card_for_text(&app, false);
    let mut second = crate::thinking::panel::talk(&ours, &principal, &typed("Как дела?"));
    while second.events.recv().await.is_some() {}
    let requests = llm.requests.lock().unwrap();
    let has_open = |index: usize| requests[index].tools.iter().any(|tool| tool.name == "open");
    assert!(has_open(0), "on the card");
    assert!(!has_open(requests.len() - 1), "every tool again from the next phrase");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn through_use_the_opened_tools_come_as_text_and_the_offer_stays_the_same() {
    use crate::dialog::llm::{AssistantTurn, LlmEvent, LlmMessage, ToolCall};
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "echo");
    card_for_text(&app, true);
    let mut settings = crate::thinking::Settings::of(&app.settings);
    settings.delivery = crate::thinking::settings::Varies::One(crate::thinking::settings::Delivery::Use);
    app.settings.set(&crate::thinking::THINKING, &settings, None).unwrap();
    let (llm, dialog) = crate::dialog::dialog_service::tests::scripted(Box::new(|_, request| {
        let called = |name: &str| {
            request.messages.iter().any(|message| {
                matches!(message, LlmMessage::Assistant(turn) if turn.tool_calls.iter().any(|call| call.name == name))
            })
        };
        let call = |name: &str, input: serde_json::Value| {
            vec![Ok(LlmEvent::Turn(AssistantTurn {
                tool_calls: vec![ToolCall {
                    id: name.into(),
                    name: name.into(),
                    input: input.as_object().cloned().unwrap(),
                    error: None,
                }],
                ..AssistantTurn::default()
            }))]
        };
        if !called("open") {
            call("open", serde_json::json!({"groups": ["reminders"]}))
        } else if !called("use") {
            call("use", serde_json::json!({"tool": "set_reminder", "args": {"text": "чайник", "in_seconds": 60}}))
        } else {
            crate::dialog::dialog_service::tests::words("Готово.")
        }
    }));
    let ours = ours_in_rooms(&app, dialog);
    let mut typed = phrase("t-1", "Не дай мне забыть про чайник", Opened::Text, Context::default());
    typed.budget.voice = false;
    let events = talk(&*ours, typed).await;
    let What::Begun { request, .. } = &events[0].what else { panic!("begun first: {events:?}") };
    let requests = llm.requests.lock().unwrap();
    let names = |index: usize| requests[index].tools.iter().map(|tool| tool.name.clone()).collect::<Vec<_>>();
    assert!(names(0).contains(&"use".to_string()), "{:?}", names(0));
    assert_eq!(names(0), names(1), "the offer stays the same: the cache holds");
    let opened = requests[1]
        .messages
        .iter()
        .find_map(|message| match message {
            LlmMessage::Tool(results) => Some(results[0].content.clone()),
            _ => None,
        })
        .unwrap_or_default();
    assert!(opened.contains("Call them through use") && opened.contains("set_reminder"), "{opened}");
    assert_eq!(line_of(&app, request).variants.delivery, "use");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_word_s_agent_answers_its_talk_and_its_window_then_the_home_s_character() {
    use crate::platform::http::Provides;
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "echo");
    let (llm, dialog) = once("never_called", "Поставил.");
    let parts = crate::talk::Parts { dialog, ..app.provide() };
    let characters: crate::thinking::agent::Characters = Arc::new(|id| {
        (id == "butler:otto").then(|| ("Отто".to_string(), "Ты дворецкий Отто, говоришь коротко.".to_string()))
    });
    let ours = Arc::new(crate::thinking::OurThinking::new(crate::thinking::Deps {
        parts,
        personas: app.personas.clone(),
        people: crate::links::thinking::people(&app.access),
        settings: app.settings.clone(),
        devices: None,
        lines: Some(app.requests.clone()),
        titles: None,
        characters: Some(characters),
    }));
    let mut to_otto = phrase("t-1", "Отто, поставь таймер на чай", Opened::Word, Context::default());
    to_otto.to = To { agent: Some("butler:otto".into()), by: By::Word, name: Some("Отто".into()) };
    let first = talk(&*ours, to_otto).await;
    assert!(first.iter().all(|event| event.speaker.as_deref() == Some("butler:otto")), "{first:?}");
    let What::Begun { request, .. } = &first[0].what else { panic!("begun first") };
    let line = line_of(&app, request);
    assert_eq!((line.agent.by.as_str(), line.agent.speaker.as_deref()), ("to", Some("butler:otto")));
    assert!(
        line.sense.iter().any(|sensed| sensed.kind == "work"),
        "«Чутьё»'s answers are in the line: {:?}",
        line.sense
    );
    {
        let requests = llm.requests.lock().unwrap();
        assert!(requests[0].system.contains("Ты дворецкий Отто"), "Otto's persona: {}", requests[0].system);
    }
    let reply = phrase("t-2", "и ещё одну минуту", Opened::Window, Context::default());
    let second = talk(&*ours, reply).await;
    assert!(
        second.iter().all(|event| event.speaker.as_deref() == Some("butler:otto")),
        "the window is Otto's: {second:?}"
    );
    let plain = phrase("t-3", "который час", Opened::Button, Context::default());
    let third = talk(&*ours, plain).await;
    assert!(third.iter().all(|event| event.speaker.as_deref() != Some("butler:otto")), "the home's again: {third:?}");
    let requests = llm.requests.lock().unwrap();
    assert!(!requests.last().unwrap().system.contains("Ты дворецкий Отто"));
}

/// A live provider whose session answers each reply as its script says; it keeps what it was given.
struct ScriptedLive {
    replies: std::sync::Mutex<std::collections::VecDeque<Vec<anyhow::Result<crate::thinking::live::LiveEvent>>>>,
    opened: std::sync::Mutex<Vec<(String, Vec<String>)>>,
    added: Arc<std::sync::Mutex<Vec<String>>>,
    results: Arc<std::sync::Mutex<Vec<String>>>,
}

struct ScriptedSession {
    live: Arc<ScriptedLive>,
}

impl crate::thinking::live::LiveSession for ScriptedSession {
    fn send(&self, _text: &str) {}

    fn result(&self, _call: &str, content: &str) {
        self.live.results.lock().unwrap().push(content.to_string());
    }

    fn add_tools(&self, tools: &[crate::dialog::llm::LlmTool]) -> bool {
        self.live.added.lock().unwrap().extend(tools.iter().map(|tool| tool.name.clone()));
        true
    }

    fn events(&self) -> futures::stream::BoxStream<'static, anyhow::Result<crate::thinking::live::LiveEvent>> {
        let reply = self.live.replies.lock().unwrap().pop_front().unwrap_or_default();
        futures::stream::iter(reply).boxed()
    }
}

struct Provider(Arc<ScriptedLive>);

impl crate::thinking::live::LiveProvider for Provider {
    fn open(
        &self,
        instructions: &str,
        tools: &[crate::dialog::llm::LlmTool],
    ) -> anyhow::Result<Arc<dyn crate::thinking::live::LiveSession>> {
        self.0
            .opened
            .lock()
            .unwrap()
            .push((instructions.to_string(), tools.iter().map(|tool| tool.name.clone()).collect()));
        Ok(Arc::new(ScriptedSession { live: self.0.clone() }))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_session_opens_with_the_card_its_calls_run_here_and_a_drop_goes_on_in_the_pipeline() {
    use crate::dialog::llm::{LlmMessage, ToolCall};
    use crate::thinking::live::LiveEvent;
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "echo");
    let (llm, dialog) = once("never_called", "Ответ конвейера.");
    let ours = ours_in_rooms(&app, dialog);
    let call = |name: &str, input: serde_json::Value| {
        Ok(LiveEvent::Call(ToolCall {
            id: name.into(),
            name: name.into(),
            input: input.as_object().cloned().unwrap(),
            error: None,
        }))
    };
    let live = Arc::new(ScriptedLive {
        replies: std::sync::Mutex::new(std::collections::VecDeque::from([
            vec![
                call("open", serde_json::json!({"groups": ["reminders"]})),
                call("set_timer", serde_json::json!({"seconds": 240})),
                Ok(LiveEvent::Text("Готово.".into())),
                Ok(LiveEvent::Done),
            ],
            vec![Err(anyhow::anyhow!("the network dropped"))],
        ])),
        opened: std::sync::Mutex::new(Vec::new()),
        added: Arc::new(std::sync::Mutex::new(Vec::new())),
        results: Arc::new(std::sync::Mutex::new(Vec::new())),
    });
    let thinking = crate::thinking::live::LiveThinking::new(ours, Arc::new(Provider(live.clone())));
    let first = talk(&thinking, kitchen("t-1", "Не дай мне забыть про чайник и поставь таймер")).await;
    {
        let opened = live.opened.lock().unwrap();
        assert_eq!(opened.len(), 1, "one session");
        assert!(opened[0].0.contains("Tools come in groups"), "the card is the session's instructions");
        assert!(
            opened[0].1.contains(&"open".to_string()) && opened[0].1.contains(&"recall".to_string()),
            "{:?}",
            opened[0].1
        );
    }
    assert!(
        live.added.lock().unwrap().contains(&"set_reminder".to_string()),
        "the opened group's tools join the session"
    );
    assert!(
        first.iter().any(|event| matches!(&event.what, What::Hands { device, .. } if device == "kitchen")),
        "the timer as hands: {first:?}"
    );
    assert!(first.iter().any(|event| matches!(&event.what, What::Say { text } if text == "Готово.")));
    // The network drops: the pipeline goes on, with the live exchange in its history.
    let second = talk(&thinking, kitchen("t-2", "а что я просил?")).await;
    assert!(
        second.iter().any(|event| matches!(&event.what, What::Say { text } if text == "Ответ конвейера.")),
        "{second:?}"
    );
    let requests = llm.requests.lock().unwrap();
    let history: Vec<String> = requests
        .last()
        .unwrap()
        .messages
        .iter()
        .filter_map(|message| match message {
            LlmMessage::User(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(history.iter().any(|text| text.contains("Не дай мне забыть")), "the same history: {history:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_every_built_in_app_the_catalog_and_the_card_s_tools_stay_within_their_caps() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "echo");
    card_for_text(&app, true);
    let (llm, dialog) = once("never_called", "Да.");
    let ours = ours_in_rooms(&app, dialog);
    let mut typed = phrase("t-1", "Какая погода и что нового?", Opened::Text, Context::default());
    typed.budget.voice = false;
    talk(&*ours, typed).await;
    let requests = llm.requests.lock().unwrap();
    let request = &requests[0];
    let caps = crate::thinking::Settings::default().card.caps;
    let catalog = request.system.split("The groups:").nth(1).unwrap_or_default();
    let catalog = catalog.split("\n\n").next().unwrap_or_default();
    assert!(!catalog.trim().is_empty(), "the catalog is on the card: {}", request.system);
    assert!(
        crate::dialog::estimate_tokens(catalog) <= caps.catalog,
        "{} tokens: {catalog}",
        crate::dialog::estimate_tokens(catalog)
    );
    let works = ["work_start", "work_add", "work_do", "app_card"];
    let tools: Vec<_> = request.tools.iter().filter(|tool| !works.contains(&tool.name.as_str())).cloned().collect();
    assert!(
        crate::thinking::card::tools_tokens(&tools) <= caps.tools,
        "{:?}",
        tools.iter().map(|tool| &tool.name).collect::<Vec<_>>()
    );
}
