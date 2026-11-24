//! The tests of `gateway/uniproxy/station_session.rs` that need the whole gateway (`App`): its crate does not have it,
//! the program does.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::Message;
use axum::http::HeaderMap;
use bytes::Bytes;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use crate::activator::{Starter, Wake, WakeOutcome};
use crate::app::App;
use crate::config::{Config, Env};
use crate::core::requests::RequestStatus;
use crate::gateway::protocol::aapi_codec::AapiCodec;
use crate::gateway::protocol::protocol_types::EventHeader;
use crate::gateway::uniproxy::spoken_answer::AnswerOutcome;
use crate::gateway::uniproxy::station_session::*;
use crate::gateway::uniproxy::uniproxy_server::{Handshake, Outgoing};
use crate::gateway::yandex::yandex_session::{StationCredentials, YandexSession, YandexSessionOptions};
use crate::skills::capabilities::CapabilityName;
use crate::utils::time::now_ms;

/// The panel's «Сказать» names the voice of one phrase: the character's or Alice's; anything else is no voice.
#[test]
fn a_typed_phrase_names_the_characters_voice_or_alices() {
    assert_eq!(SayVoice::parse("own"), Some(SayVoice::Own));
    assert_eq!(SayVoice::parse("character"), Some(SayVoice::Own));
    assert_eq!(SayVoice::parse("alice"), Some(SayVoice::Alice));
    assert_eq!(SayVoice::parse("yandex"), Some(SayVoice::Alice));
    assert_eq!(SayVoice::parse("loud"), None);
}

/// The review's deadlock: a sessions poll (`snapshot`) asked our Yandex session whether it was up while it held
/// the journal, and our Yandex session journals under its own state (`open`, `dispatch`, the heartbeat). Here
/// the two meet in the order that deadlocked: our Yandex session holds its state, the poll comes in and waits
/// for that state, then our Yandex session journals. The journal is a leaf lock now (SessionJournal): the poll
/// waits holding the session's state only, which journaling never takes, so both finish.
/// A session of an in-memory gateway, with no socket under it: what it sends stays in the queue.
fn session_of(app: &App) -> (Arc<StationSession>, mpsc::UnboundedReceiver<Outgoing>) {
    session_with(app, None)
}

/// The same, with who answers a word's request.
fn session_with(
    app: &App,
    characters: Option<Arc<dyn crate::activator::WordCharacter>>,
) -> (Arc<StationSession>, mpsc::UnboundedReceiver<Outgoing>) {
    let deps = Arc::new(SessionDeps {
        codec: Arc::new(AapiCodec::new()),
        config: Arc::new(app.config.gateway.clone()),
        speech: app.speech.clone(),
        dialog: app.dialog.clone(),
        skills: app.skills.clone(),
        voice: app.voice.clone(),
        modes: app.modes.clone(),
        household: app.access.household.clone(),
        events: app.events.clone(),
        requests: None,
        mcp: None,
        tools: None,
        rescue: None,
        talk: None,
        characters,
    });
    let handshake = Handshake { headers: HeaderMap::new(), remote_address: "192.168.1.50".into() };
    StationSession::new(&handshake, deps)
}

/// Nothing on disk: no database, no session journal folder (the latest events stay in memory).
fn memory_gateway() -> Arc<App> {
    App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", ""), ("RECORDER_ENABLED", "false")]))).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sessions_poll_never_deadlocks_with_our_yandex_session_journaling_under_its_state() {
    let app = memory_gateway();
    let (session, _outgoing) = session_of(&app);
    let link = session.link();
    // Never opened: nothing connects anywhere.
    let yandex = YandexSession::new(
        YandexSessionOptions {
            url: "ws://127.0.0.1:9/uni.ws".into(),
            tts: app.config.gateway.tts.clone(),
            ping_ms: 0,
            reconnect_ms: vec![1000],
        },
        StationCredentials { headers: Vec::new(), synchronize_state: Bytes::new() },
        session.deps.codec.clone(),
        link.clone(),
        link,
    );
    session.lock().yandex = Some(yandex.clone());

    let (done, finished) = std::sync::mpsc::channel();
    let (held, holding) = std::sync::mpsc::channel();
    let upstream = done.clone();
    std::thread::spawn(move || {
        let meanwhile = || {
            let _ = held.send(());
            // Time for the poll to come in and wait for our state.
            std::thread::sleep(Duration::from_millis(300));
        };
        yandex.note_under_state(meanwhile, "yandexConnected", "journaled under its state");
        let _ = upstream.send("our Yandex session journaled");
    });
    holding.recv().unwrap();
    let poll = session.clone();
    std::thread::spawn(move || {
        let snapshot = poll.snapshot();
        let _ = done.send(if snapshot.yandex_connected == Some(false) { "the poll answered" } else { "?" });
    });
    let mut ended = Vec::new();
    for _ in 0..2 {
        ended.push(finished.recv_timeout(Duration::from_secs(5)).expect("the poll and our Yandex session deadlocked"));
    }
    ended.sort();
    assert_eq!(ended, ["our Yandex session journaled", "the poll answered"]);
    assert!(session.recent_events(10).iter().any(|event| event.kind == "yandexConnected"));
}

/// An activator's word on the station (src/activator): asked once, the station's «directive» request takes the
/// sound before the word — the command starts where the word ended, the request is the word's and its text loses
/// the word; while that request listens, another word wakes nothing.
#[tokio::test]
async fn an_activators_word_wakes_the_station_and_its_request_starts_with_the_word() {
    let app = memory_gateway();
    let (session, _outgoing) = session_of(&app);
    let asked = Arc::new(AtomicU32::new(0));
    let starter: Starter = {
        let (session, asked) = (session.clone(), asked.clone());
        Arc::new(move || {
            asked.fetch_add(1, Ordering::SeqCst);
            let session = session.clone();
            Box::pin(async move {
                // The station answers as its button: a request of its own.
                let mut request = unknown_request();
                request.activation_type = Some("directive".into());
                let header = EventHeader { message_id: "woken-1".into(), ref_message_id: None, stream_id: Some(3) };
                let mut inner = session.lock();
                session.start_voice(&mut inner, header, request);
                Ok(())
            })
        })
    };
    let now = now_ms() as i64;
    // Two seconds of sound, the word ending 1.9 s into it.
    let wake = Wake {
        app: "words".into(),
        word: "otto".into(),
        title: "Отто".into(),
        heard: vec!["ото".into()],
        character: Some("butler:otto".into()),
        score: 0.99,
        at_ms: now - 100,
        audio: vec![0; 16 * 2000],
        audio_start_ms: now - 2000,
    };
    assert_eq!(session.wake_by_activator(wake.clone(), starter.clone()).await, WakeOutcome::Started);
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    {
        let mut inner = session.lock();
        let request = {
            let voice = inner.voice_by_stream(3).expect("the station's request");
            assert_eq!(voice.activation.as_ref().map(|by| by.title.as_str()), Some("Отто"));
            assert!(extra_wake_words(voice).contains(&"ото".to_string()));
            voice.speech.flush();
            // 1.5 s of sound before the word's end goes in front of the station's.
            assert_eq!(voice.speech.command_start_ms(), Some(1500.0));
            voice.request.clone()
        };
        assert!(inner.by_wake_word("woken-1", &request));
        assert_eq!(
            session.origin(inner.activation("woken-1").as_ref(), &request).character.as_deref(),
            Some("butler:otto")
        );
    }
    let again = session.wake_by_activator(wake, starter).await;
    assert_eq!(again, WakeOutcome::Skipped(crate::tr!("колонка уже слушает")));
    assert_eq!(asked.load(Ordering::SeqCst), 1, "a listening station is not asked again");
    assert!(session.recent_events(10).iter().any(|event| event.kind == "activatorWake"));
}

/// Who answers a word's request, as a test sees it: the calls; «answers already» after the first.
struct Called(std::sync::Mutex<Vec<(String, String)>>);

impl crate::activator::WordCharacter for Called {
    fn answer_as(&self, character: &str, word: &str) -> bool {
        let mut calls = self.0.lock().unwrap();
        calls.push((character.to_string(), word.to_string()));
        calls.len() == 1
    }
}

/// A word's request with words makes the word's character answer — today the house's from then on — and it is
/// noted; a request no word began calls nobody.
#[tokio::test]
async fn a_words_request_with_words_makes_its_character_answer() {
    let app = memory_gateway();
    let called = Arc::new(Called(std::sync::Mutex::new(Vec::new())));
    let (session, _outgoing) = session_with(&app, Some(called.clone()));
    let starter: Starter = {
        let session = session.clone();
        Arc::new(move || {
            let session = session.clone();
            Box::pin(async move {
                let mut request = unknown_request();
                request.activation_type = Some("directive".into());
                let header = EventHeader { message_id: "woken-1".into(), ref_message_id: None, stream_id: Some(3) };
                let mut inner = session.lock();
                session.start_voice(&mut inner, header, request);
                Ok(())
            })
        })
    };
    let now = now_ms() as i64;
    let wake = Wake {
        app: "words".into(),
        word: "otto".into(),
        title: "Отто".into(),
        heard: vec!["ото".into()],
        character: Some("butler:otto".into()),
        score: 0.99,
        at_ms: now - 100,
        audio: vec![0; 16 * 2000],
        audio_start_ms: now - 2000,
    };
    assert_eq!(session.wake_by_activator(wake, starter).await, WakeOutcome::Started);
    {
        let mut inner = session.lock();
        let request = inner.voice_by_stream(3).expect("the station's request").request.clone();
        session.handle_request(&mut inner, "woken-1", &request, "который час", false);
        session.handle_request(&mut inner, "spoken-2", &unknown_request(), "который час", false);
    }
    assert_eq!(*called.0.lock().unwrap(), vec![("butler:otto".to_string(), "Отто".to_string())]);
    assert!(session.recent_events(30).iter().any(|event| event.kind == "activatorCharacter"));
}

/// A word the station begins no request for is asked once more, then given up; a request of another kind meanwhile
/// is not the word's.
#[tokio::test(start_paused = true)]
async fn a_word_without_a_request_is_asked_once_more_then_given_up() {
    let app = memory_gateway();
    let (session, _outgoing) = session_of(&app);
    let asked = Arc::new(AtomicU32::new(0));
    let silent: Starter = {
        let asked = asked.clone();
        Arc::new(move || {
            asked.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        })
    };
    let wake = Wake {
        app: "words".into(),
        word: "otto".into(),
        title: "Отто".into(),
        heard: Vec::new(),
        character: None,
        score: 1.0,
        at_ms: now_ms() as i64,
        audio: Vec::new(),
        audio_start_ms: now_ms() as i64,
    };
    let outcome = session.wake_by_activator(wake.clone(), silent).await;
    assert_eq!(outcome, WakeOutcome::Failed(crate::tr!("колонка не начала слушать")));
    assert_eq!(asked.load(Ordering::SeqCst), 2);
    assert!(session.lock().pending_wake.is_none());

    // The station's own wake word comes while the word waits: that request is not the word's.
    let spotter: Starter = {
        let session = session.clone();
        Arc::new(move || {
            let session = session.clone();
            Box::pin(async move {
                let mut request = unknown_request();
                request.activation_type = Some("spotter".into());
                let header = EventHeader { message_id: "alice-1".into(), ref_message_id: None, stream_id: Some(5) };
                let mut inner = session.lock();
                session.start_voice(&mut inner, header, request);
                Ok(())
            })
        })
    };
    let outcome = session.wake_by_activator(wake, spotter).await;
    assert_eq!(outcome, WakeOutcome::Skipped(crate::tr!("колонка начала другой запрос")));
    assert!(session.lock().activation("alice-1").is_none());
}

/// A vgw_say callback that comes as its speech stops waiting (the 5 s ran out, and the speech has not yet taken
/// its entry back) still gets an empty response, as in the TS.
#[tokio::test]
async fn answers_a_callback_its_speech_no_longer_waits_for_with_an_empty_response() {
    let app = memory_gateway();
    let (session, mut outgoing) = session_of(&app);
    let (resolve, called_back) = oneshot::channel::<SayCallback>();
    drop(called_back);
    let mut request = unknown_request();
    request.event.payload = json!({ "id": "say-1" }).as_object().cloned();
    {
        let mut inner = session.lock();
        inner.pending_says.push(("say-1".into(), resolve));
        session.on_say_callback(&mut inner, "callback-1", &request);
        assert!(inner.pending_says.is_empty());
    }
    let Ok(Outgoing::Message(Message::Binary(frame))) = outgoing.try_recv() else {
        panic!("a response goes to the station")
    };
    let decoded = session.deps.codec.decode_server_frame(&frame).unwrap();
    let event = &decoded["event"];
    assert_eq!(event["header"]["refMessageId"], "callback-1");
    assert!(event["aliceResponse"].is_object(), "{decoded}");
    assert!(session.recent_events(10).iter().any(|event| event.kind == "callback"
        && event.summary.as_deref().is_some_and(|summary| summary.contains("no longer waits"))));
}

#[test]
fn reads_the_stations_request_to_continue_its_player() {
    // As the station sends it: field 1 the typed frame {53: {3: {1: 1}}}, field 2 the analytics with its purpose.
    let purpose = b"player_continue";
    let analytics = [&[0x10, 0x03, 0x1a, purpose.len() as u8][..], purpose].concat();
    let frame = [0xaa, 0x03, 0x04, 0x1a, 0x02, 0x08, 0x01];
    let raw = [&[0x0a, frame.len() as u8][..], &frame, &[0x12, analytics.len() as u8], &analytics].concat();
    let mut request = unknown_request();
    request.event.name = Some("@@mm_semantic_frame".into());
    request.event.payload_raw = Some(Bytes::from(raw));
    assert!(is_player_continue(&request));
    request.event.name = Some("@@mm_stack_engine_get_next".into());
    assert!(!is_player_continue(&request));
}

#[test]
fn names_the_capability_of_a_yandex_callback() {
    assert_eq!(callback_capability("calls_next_step"), CapabilityName::Calls);
    assert_eq!(callback_capability("music_thin_client_on_started"), CapabilityName::Music);
    assert_eq!(callback_capability("@@mm_stack_engine_get_next"), CapabilityName::Skills);
    assert!(player_callback("@@mm_stack_engine_get_next") && !player_callback("vgw_media_on_finished"));
}

#[test]
fn ends_a_request_by_its_answer() {
    let spoken = SayOutcome::Answer(AnswerOutcome::Spoken);
    assert_eq!(status_of(spoken, true, false), RequestStatus::Answered);
    assert_eq!(status_of(SayOutcome::Answer(AnswerOutcome::Silent), true, false), RequestStatus::Failed);
    assert_eq!(status_of(SayOutcome::Answer(AnswerOutcome::Empty), false, true), RequestStatus::Answered);
    assert_eq!(status_of(SayOutcome::Answer(AnswerOutcome::Empty), false, false), RequestStatus::Silent);
    assert_eq!(status_of(SayOutcome::Failed, false, false), RequestStatus::Failed);
}

#[test]
fn splits_long_texts_into_phrases() {
    assert_eq!(split_phrases("  Хорошо.  "), vec!["Хорошо."]);
    assert!(split_phrases("   ").is_empty());
    let long = "Первое длинное предложение для проверки разбиения на фразы. ".repeat(4);
    assert!(split_phrases(&long).len() > 1);
}
