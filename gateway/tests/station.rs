//! The station link in gateway mode end to end (test/app.e2e-spec.ts, its station cases): a station built on the
//! codec on our uniproxy; a fake Yandex for our own session with it; fake recognition, model and weather servers.
//! Each test starts its own gateway with its own fakes.
//!
//! test/app.e2e-spec.ts, the Node original, is removed from the tree: git tag `node-gateway-last`.

mod fakes;
mod support;

use std::path::Path;
use std::sync::{Arc, LazyLock};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use duowave::core::stations::presence::PresenceState;
use duowave::gateway::protocol::proto_object::bytes_value;
use duowave::gateway::protocol::protocol_types::{Directive, DirectiveType};
use duowave::speech::audio::ogg_opus_demuxer::{OggOpusDemuxer, OggOpusEvent};
use duowave::speech::test_audio::{concat, ogg_opus_chain, ogg_pages, silence, silence_at, station_audio, voice};
use fakes::gateway_yandex::{FakeAnswer, GatewayYandex};
use fakes::{
    at, codec, lock, server_frame, sleep, start_forecast, until, until_async, AsrStub, ChatStub, FakeConnection,
    StationClient,
};
use reqwest::StatusCode;
use serde_json::{json, Map, Value};
use support::{bearer, message, send, Gateway};
use tokio::sync::Semaphore;

/// What the station sends on its handshake: the gateway repeats it to Yandex and must never journal the token.
const STATION_HEADERS: &[(&str, &str)] =
    &[("user-agent", "WebSocket++/0.8.2"), ("x-uprx-oauth-token", "y0_test-secret"), ("x-uprx-uid", "123456")];

/// A few gateways at once: the timing of speech must not suffer from too many beside it.
static RUNNING: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(3));

struct Env {
    gateway: Gateway,
    token: String,
    asr: AsrStub,
    llm: ChatStub,
    yandex: GatewayYandex,
    _dir: tempfile::TempDir,
    _turn: tokio::sync::SemaphorePermit<'static>,
}

/// The gateway's own environment, as the TS test's beforeAll set it.
async fn env() -> Env {
    env_with(&[]).await
}

async fn env_with(extra: &[(&str, &str)]) -> Env {
    let turn = RUNNING.acquire().await.expect("the semaphore stays open");
    let forecast = start_forecast().await;
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("content").join("music");
    std::fs::create_dir_all(&music).unwrap();
    std::fs::write(music.join("Кино - Группа крови.mp3"), vec![1u8; 4096]).unwrap();
    std::fs::write(music.join("Кино - Кукушка.mp3"), vec![2u8; 4096]).unwrap();
    let asr = AsrStub::start().await;
    let llm = ChatStub::start().await;
    let yandex = GatewayYandex::start().await;
    let journal = dir.path().join("sessions").to_string_lossy().into_owned();
    let content = dir.path().join("content").to_string_lossy().into_owned();
    let yandex_url = yandex.url();
    let mut vars: Vec<(&str, &str)> = vec![
        ("RECORDER_ENABLED", "true"),
        ("RECORDER_DIR", &journal),
        // The defaults are for a new household (relay, no audio, no debug API); these tests need the gateway's own.
        ("UNIPROXY_MODE", "gateway"),
        ("RECORDER_AUDIO", "true"),
        ("DEBUG_API_ENABLED", "true"),
        // Level-based VAD keeps synthetic audio deterministic.
        ("VAD_ENGINE", "energy"),
        ("VAD_NO_SPEECH_MS", "1500"),
        ("ASR_PROVIDER", "openai"),
        ("ASR_BASE_URL", &asr.url),
        ("ASR_MODEL", "test-model"),
        ("ASR_PARTIAL_INTERVAL_MS", "50"),
        ("ASR_INTERIM_INTERVAL_MS", "300"),
        ("ASR_QUIET_END_MS", "600"),
        ("DIALOG_PROVIDER", "openai"),
        ("LLM_BASE_URL", &llm.url),
        ("LLM_MODEL", "test-llm"),
        ("DIALOG_HOLD_RESPONSE_MS", "300"),
        ("YANDEX_UNIPROXY_URL", &yandex_url),
        ("TTS_TIMEOUT_MS", "3000"),
        // A capability with its route off, for "not yet"; our own music and the player's commands ours alone.
        ("ROUTES", "do_not_disturb=off;music=own;playback=own"),
        ("CONTENT_DIR", &content),
        ("YANDEX_TIMEOUT_MS", "1500"),
        // The morning show goes to Yandex as the station's voice (music would, but here music is ours).
        ("YANDEX_VOICE", "morning_show"),
        ("WEATHER_LOCATION", "25.20,55.27"),
        ("WEATHER_FORECAST_URL", &forecast),
        // Several test stations: any is taken.
        ("STATION_PAIRING", "open"),
    ];
    vars.extend_from_slice(extra);
    let gateway = Gateway::start(&vars).await;
    // The Rust gateway seeds a new content folder with the shipped radio list; the TS test's folder had none, so the
    // radio was Yandex's. The list is read on every search: without it, it is again.
    let _ = std::fs::remove_file(dir.path().join("content").join("radio.json"));
    let setup = message(
        gateway.post("/api/v1/auth/setup").json(&json!({"password": "e2e-admin-password"})),
        StatusCode::CREATED,
    );
    let token = setup.await["token"].as_str().unwrap().to_string();
    Env { gateway, token, asr, llm, yandex, _dir: dir, _turn: turn }
}

/// Wake word buffer: room noise, then a "word" ending at SpotterEnd, at the station's quiet level.
fn wake_word() -> Vec<i16> {
    concat(&[&silence(1200.0), &voice(800.0, -50.0)])
}

fn is_final_asr(r#ref: &str) -> impl Fn(&Value) -> bool + '_ {
    move |m| at(m, "event.header.refMessageId") == r#ref && at(m, "event.asrResult.endOfUtt") == true
}

fn is_interim_asr(r#ref: &str) -> impl Fn(&Value) -> bool + '_ {
    move |m| {
        at(m, "event.header.refMessageId") == r#ref
            && !at(m, "event.asrResult").is_null()
            && at(m, "event.asrResult.endOfUtt") != true
    }
}

fn response_to(request_id: &str) -> impl Fn(&Value) -> bool + '_ {
    move |m| at(m, "event.aliceResponse.header.requestId") == request_id
}

fn speak_of(r#ref: &str) -> impl Fn(&Value) -> bool + '_ {
    move |m| !at(m, "event.ttsSpeak").is_null() && at(m, "event.header.refMessageId") == r#ref
}

fn close_of(stream_id: u64) -> impl Fn(&Value) -> bool {
    move |m| at(m, "streamControl.streamId") == stream_id && !at(m, "streamControl.close").is_null()
}

fn directive_names(message: &Value) -> Vec<String> {
    at(message, "event.aliceResponse.response.directives")
        .as_array()
        .map(|directives| directives.iter().map(|d| at(d, "name").as_str().unwrap_or("").to_string()).collect())
        .unwrap_or_default()
}

/// A result without recognition: nothing came on the wire (the TS's `recognition ?? []`).
fn heard_nothing(message: &Value) -> bool {
    let recognition = at(message, "event.asrResult.recognition");
    recognition.is_null() || recognition == &json!([])
}

/// The normalized text of a final or interim result; "" when it has no recognition.
fn normalized(message: &Value) -> String {
    at(message, "event.asrResult.recognition.0.normalized").as_str().unwrap_or("").to_string()
}

fn client_directive(name: &str, payload: Value) -> Directive {
    let mut directive = Directive::client(name, payload.as_object().cloned().unwrap_or_default());
    directive.r#type = DirectiveType::ClientAction;
    directive
}

impl Env {
    fn url(&self) -> String {
        format!("ws://127.0.0.1:{}/uni.ws", self.gateway.port)
    }

    async fn station(&self, headers: &[(&str, &str)]) -> StationClient {
        StationClient::connect(&self.url(), headers).await.expect("the station connects")
    }

    /// A station that synchronized, and our session with Yandex for it, once that is up.
    async fn connected_station(&self, sync_id: &str) -> (StationClient, Arc<FakeConnection>) {
        let before = self.yandex.server.count();
        let station = self.station(STATION_HEADERS).await;
        station.synchronize(sync_id).await;
        for _ in 0..100 {
            let upstream = self.yandex.server.connections().into_iter().skip(before).find(|c| !c.frames().is_empty());
            if let Some(upstream) = upstream {
                // Our session counts as up once Yandex's SynchronizeStateResponse is in.
                sleep(30).await;
                return (station, upstream);
            }
            sleep(20).await;
        }
        panic!("our session with Yandex did not open");
    }

    /// A spoken command from wake word to the end of speech, on a station that already synchronized.
    fn speak(
        &self,
        station: &StationClient,
        message_id: &str,
        stream_id: u32,
        request_id: &str,
        device_state: Option<Value>,
        environment: Option<Value>,
    ) {
        station.start_voice(message_id, stream_id, request_id, device_state, environment);
        let audio = station_audio(&wake_word(), &concat(&[&voice(700.0, -50.0), &silence(1200.0)]));
        station.send_audio(stream_id, &audio.bytes[..audio.spotter_end_at]);
        station.send(json!({ "streamControl": { "streamId": stream_id, "messageId": format!("{message_id}-spotter"), "spotterEnd": {} } }));
        station.send_audio(stream_id, &audio.bytes[audio.spotter_end_at..]);
    }

    async fn get(&self, path: &str) -> Value {
        let (name, value) = bearer(&self.token);
        message(self.gateway.get(path).header(name, value), StatusCode::OK).await
    }

    async fn post(&self, path: &str, body: Value, expected: StatusCode) -> Value {
        let (name, value) = bearer(&self.token);
        message(self.gateway.post(path).header(name, value).json(&body), expected).await
    }

    /// Every journal event of every session, through the debug API.
    async fn all_events(&self) -> Vec<Value> {
        let mut events = Vec::new();
        for session in self.get("/debug/sessions").await.as_array().unwrap() {
            let path = format!("/debug/sessions/{}/events?limit=500", session["id"].as_str().unwrap());
            events.extend(self.get(&path).await.as_array().unwrap().iter().cloned());
        }
        events
    }

    /// The voiceTimeline journal entry of a request.
    async fn voice_timeline(&self, request_id: &str) -> Value {
        for _ in 0..40 {
            let events = self.all_events().await;
            if let Some(timeline) =
                events.iter().find(|e| e["kind"] == "voiceTimeline" && e["data"]["requestId"] == request_id)
            {
                return timeline["data"].clone();
            }
            sleep(50).await;
        }
        panic!("no voiceTimeline for {request_id}");
    }

    /// The answerTimeline journal entry of the answer to a request message, once the answer has finished.
    async fn answer_timeline(&self, r#ref: &str) -> Value {
        for _ in 0..60 {
            let events = self.all_events().await;
            if let Some(timeline) = events.iter().find(|e| e["kind"] == "answerTimeline" && e["data"]["ref"] == r#ref) {
                return timeline["data"].clone();
            }
            sleep(50).await;
        }
        panic!("no answerTimeline for {ref}");
    }

    /// The route the journal noted for a text.
    async fn route(&self, text: &str) -> Value {
        for _ in 0..40 {
            let events = self.all_events().await;
            if let Some(route) = events.iter().find(|e| e["kind"] == "route" && e["data"]["text"] == text) {
                return route["data"].clone();
            }
            sleep(50).await;
        }
        panic!("no route for {text}");
    }
}

/// The command at the station's pace: 100 ms of audio every 100 ms (the first two pages are stream headers).
async fn send_in_real_time(station: &StationClient, stream_id: u32, command: &[u8], done: impl Fn() -> bool) {
    let pages = ogg_pages(command);
    for chunk in pages.chunks(5) {
        if done() {
            return;
        }
        station.send_audio(stream_id, &chunk.concat());
        sleep(100).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answers_the_station_connectivity_check_with_204() {
    let env = env().await;
    assert_eq!(send(env.gateway.get("/generate_204")).await.0, StatusCode::NO_CONTENT);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn takes_what_came_through_a_cloudflare_tunnel_for_outside_no_debug_api_no_station_or_calls_socket() {
    let env = env().await;
    let tunnel = [("cf-connecting-ip", "203.0.113.7"), ("cf-ray", "8f00000000000000-WAW")];
    let (name, value) = bearer(&env.token);
    let mut debug = env.gateway.get("/debug/sessions").header(name, value);
    for (header, value) in tunnel {
        debug = debug.header(header, value);
    }
    assert_eq!(send(debug).await.0, StatusCode::FORBIDDEN);
    let mut headers: Vec<(&str, &str)> = STATION_HEADERS.to_vec();
    headers.extend(tunnel);
    let refused = StationClient::connect(&env.url(), &headers).await.err();
    assert_eq!(refused, Some(403));
    if cfg!(feature = "pc") {
        let calls = format!("ws://127.0.0.1:{}/calls/ws", env.gateway.port);
        assert_eq!(StationClient::connect(&calls, &headers).await.err(), Some(403));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refuses_the_station_socket_to_a_browsers_page() {
    let env = env().await;
    // A page on the home network opening the gateway's socket says whose page it is; a station never does.
    let page = [STATION_HEADERS, &[("origin", "http://evil.example")]].concat();
    assert_eq!(StationClient::connect(&env.url(), &page).await.err(), Some(403));
    let station = StationClient::connect(&env.url(), STATION_HEADERS).await.expect("the station itself connects");
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn takes_the_station_socket_by_any_name_it_reaches_us_at_the_host_check_is_the_pages() {
    let env = env().await;
    // A router's domain nobody listed in ALLOWED_HOSTS. Node's 'upgrade' listener never went through Express's Host
    // check: the station connects by it, while the gateway's pages refuse it.
    let named = [STATION_HEADERS, &[("host", "voice-gw.fritz.box:3000")]].concat();
    let station = StationClient::connect(&env.url(), &named).await.expect("the station connects by that name");
    station.synchronize("sync-named").await;
    let (status, _) = send(env.gateway.get("/health").header("host", "voice-gw.fritz.box:3000")).await;
    assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reports_health_with_the_speech_setup() {
    let env = env().await;
    let health = message(env.gateway.get("/health"), StatusCode::OK).await;
    assert_eq!(health["status"], "ok");
    assert_eq!(
        health["speech"],
        json!({ "vad": "energy", "asr": format!("test-model via {}/audio/transcriptions", env.asr.url) })
    );
    assert_eq!(health["dialog"], format!("test-llm via {}/chat/completions", env.llm.url));
    assert_eq!(health["tts"], format!("yandex via {}", env.yandex.url()));
    let host = url::Url::parse(&env.llm.url).unwrap();
    let title = format!("{}:{}", host.host_str().unwrap(), host.port().unwrap());
    let profile = json!({ "provider": "env", "kind": "openai", "title": title, "model": "test-llm", "cloud": false });
    assert_eq!(
        health["brain"],
        json!({
            "provider": "openai",
            "models": { "fast": "test-llm", "smart": "test-llm" },
            "cloud": false,
            "profiles": { "fast": profile, "smart": profile },
            "source": "env",
        })
    );
    assert_eq!(health["activeSessions"], 0);
    assert_eq!(health["station"]["connected"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn synchronizes_state() {
    let env = env().await;
    let station = env.station(&[]).await;
    station.send(json!({ "event": { "header": { "messageId": "sync-1" },
        "synchronizeStateRequest": { "application": { "deviceModel": "plum", "timezone": "Asia/Dubai" } } } }));
    let sync = station.wait_for(|m| !at(m, "event.synchronizeStateResponse").is_null(), 4000).await;
    assert_eq!(at(&sync, "event.header.refMessageId"), "sync-1");
    assert!(!at(&sync, "event.synchronizeStateResponse.sessionId").as_str().unwrap().is_empty());
    let sessions = env.get("/debug/sessions").await;
    assert_eq!(sessions[0]["device"]["deviceModel"], "plum");
    assert_eq!(env.get("/api/v1/station/sessions").await[0]["wire"], "aapi");
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keeps_what_the_station_says_about_itself_and_lets_a_person_pick_only_what_that_leaves_open() {
    let env = env().await;
    let station = env.station(&[]).await;
    station.send(json!({ "event": { "header": { "messageId": "sync-look" }, "synchronizeStateRequest": {
        "application": { "deviceModel": "plum", "deviceColor": "blue", "timezone": "Asia/Dubai" } } } }));
    station.wait_for(|m| !at(m, "event.synchronizeStateResponse").is_null(), 4000).await;
    station.close();
    let mut entry = Value::Null;
    for _ in 0..40 {
        entry = env.get("/api/v1/stations").await[0].clone();
        if entry["deviceColor"] == "blue" {
            break;
        }
        sleep(25).await;
    }
    // The Lite 2 says its colour, not whether it has the clock: blue comes in both, so the clock is a person's choice.
    assert_eq!(
        (&entry["deviceModel"], &entry["deviceColor"], &entry["model"], &entry["color"]),
        (&json!("plum"), &json!("blue"), &Value::Null, &Value::Null)
    );
    let look = &entry["look"];
    assert_eq!(
        (&look["family"], &look["model"], &look["color"], &look["modelFrom"], &look["colorFrom"]),
        (&json!("lite2"), &json!("clock"), &json!("azure"), &json!("default"), &json!("station"))
    );

    let device = fakes::encode_uri(entry["device"].as_str().unwrap());
    let (name, value) = bearer(&env.token);
    let patch = |body: Value| {
        env.gateway.patch(&format!("/api/v1/stations/{device}")).header(name.clone(), value.clone()).json(&body)
    };
    let picked = message(patch(json!({ "model": "plain", "name": "Кухня" })), StatusCode::OK).await;
    assert_eq!((&picked["name"], &picked["model"]), (&json!("Кухня"), &json!("plain")));
    assert_eq!(
        (&picked["look"]["model"], &picked["look"]["clock"], &picked["look"]["modelFrom"]),
        (&json!("plain"), &json!(false), &json!("chosen"))
    );
    let (status, refused) = send(patch(json!({ "color": "pink" }))).await;
    assert_eq!((status, refused["message"].as_str()), (StatusCode::BAD_REQUEST, Some("Цвет колонка сообщила сама.")));
    assert_eq!(send(patch(json!({ "model": "big" }))).await.0, StatusCode::BAD_REQUEST);
    let reset = message(patch(json!({ "model": null })), StatusCode::OK).await;
    assert_eq!((&reset["look"]["model"], &reset["look"]["modelFrom"]), (&json!("clock"), &json!("default")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recognises_the_command_after_the_wake_word_and_answers_in_the_order_the_station_expects() {
    let env = env().await;
    let station = env.station(&[]).await;
    station.start_voice("voice-1", 1, "req-1", None, None);
    station.wait_for(|m| !at(m, "event.inputStartAck").is_null(), 4000).await;
    // Interim results keep the station listening while the user speaks.
    station.wait_for(is_interim_asr("voice-1"), 4000).await;

    let audio = station_audio(
        &wake_word(),
        &concat(&[&voice(200.0, -50.0), &silence(500.0), &voice(700.0, -55.0), &silence(1200.0)]),
    );
    station.send_audio(1, &audio.bytes[..audio.spotter_end_at]);
    station.send(json!({ "streamControl": { "streamId": 1, "messageId": "sc-1", "spotterEnd": {} } }));
    station.send_audio(1, &audio.bytes[audio.spotter_end_at..]);

    let last = station.wait_for(is_final_asr("voice-1"), 4000).await;
    assert_eq!(normalized(&last), "Привет.");
    station.wait_for(response_to("req-1"), 4000).await;

    let final_at = station.index(is_final_asr("voice-1")).unwrap();
    let close_at = station.index(close_of(1)).unwrap();
    let validation_at = station.index(|m| at(m, "event.spotterValidation.result") == "SPOTTING_OK").unwrap();
    let response_at = station.index(|m| !at(m, "event.aliceResponse").is_null()).unwrap();
    assert!(final_at < close_at && close_at < validation_at && validation_at < response_at);
    // No interim result after the final one.
    assert!(!station.received()[final_at + 1..].iter().any(|m| !at(m, "event.asrResult").is_null()));

    // The last call is the final recognition; interim ones may come before it.
    let transcriptions = env.asr.transcriptions();
    let last_call = transcriptions.last().unwrap();
    assert!(last_call.contains("multipart/form-data"));
    assert!(last_call.contains("name=\"model\"\r\n\r\ntest-model"));
    assert!(last_call.contains("RIFF"));

    let timeline = env.voice_timeline("req-1").await;
    assert_eq!(timeline["outcome"], "endOfUtterance");
    assert_eq!(
        (&timeline["recognition"]["raw"], &timeline["recognition"]["text"]),
        (&json!("Алиса, привет."), &json!("Привет."))
    );
    assert!(timeline.get("allAudio").is_none());
    assert_eq!(timeline["speech"]["commandStartSource"], "station");
    until(2000, "the utterance's file", || Path::new(timeline["utterance"]["file"].as_str().unwrap()).exists()).await;
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sends_the_text_recognised_so_far_in_interim_results_while_the_command_is_spoken() {
    let env = env().await;
    let station = env.station(&[]).await;
    station.start_voice("voice-4", 9, "req-4", None, None);
    let audio = station_audio(&wake_word(), &concat(&[&voice(1500.0, -50.0), &silence(1000.0)]));
    station.send_audio(9, &audio.bytes[..audio.spotter_end_at]);
    station.send(json!({ "streamControl": { "streamId": 9, "messageId": "sc-5", "spotterEnd": {} } }));
    send_in_real_time(&station, 9, &audio.bytes[audio.spotter_end_at..], || station.has(is_final_asr("voice-4"))).await;

    let last = station.wait_for(is_final_asr("voice-4"), 4000).await;
    assert_eq!(normalized(&last), "Привет.");
    let final_at = station.index(is_final_asr("voice-4")).unwrap();
    let interims: Vec<Value> =
        station.received()[..final_at].iter().filter(|m| is_interim_asr("voice-4")(m)).cloned().collect();
    // Empty until the first interim recognition, then its text.
    assert_eq!(normalized(&interims[0]), "");
    assert!(interims.iter().any(|m| normalized(m) == "Привет."));

    let timeline = env.voice_timeline("req-4").await;
    assert!(timeline["interims"].as_u64().unwrap() >= 1);
    assert_eq!(timeline["lastInterimText"], "Привет.");
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recognises_what_was_said_when_the_station_gives_up_with_no_speech_mid_sentence() {
    let env = env().await;
    let station = env.station(&[]).await;
    station.start_voice("voice-5", 11, "req-5", None, None);
    // Still speaking when the station closes its stream on its own timer.
    let audio = station_audio(&wake_word(), &voice(1500.0, -50.0));
    station.send_audio(11, &audio.bytes[..audio.spotter_end_at]);
    station.send(json!({ "streamControl": { "streamId": 11, "messageId": "sc-6", "spotterEnd": {} } }));
    station.send_audio(11, &audio.bytes[audio.spotter_end_at..]);
    station.send(
        json!({ "streamControl": { "streamId": 11, "messageId": "sc-7", "close": { "reason": "CLOSE_NO_SPEECH" } } }),
    );

    let last = station.wait_for(is_final_asr("voice-5"), 4000).await;
    assert_eq!(normalized(&last), "Привет.");
    station.wait_for(response_to("req-5"), 4000).await;
    // The station closed its stream itself: we do not close it again.
    assert!(!station.has(|m| at(m, "streamControl.streamId") == 11));
    let timeline = env.voice_timeline("req-5").await;
    assert_eq!(timeline["outcome"], "station:noSpeech");
    assert_eq!(timeline["recognition"]["text"], "Привет.");
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answers_an_empty_result_when_nothing_follows_the_wake_word_after_one_look_at_all_of_the_audio() {
    let env = env().await;
    env.asr.set_reply("Алиса.");
    let station = env.station(&[]).await;
    let before = env.asr.transcriptions().len();
    station.start_voice("voice-2", 3, "req-2", None, None);
    let audio = station_audio(&wake_word(), &silence(2500.0));
    station.send_audio(3, &audio.bytes[..audio.spotter_end_at]);
    station.send(json!({ "streamControl": { "streamId": 3, "messageId": "sc-2", "spotterEnd": {} } }));
    station.send_audio(3, &audio.bytes[audio.spotter_end_at..]);

    let last = station.wait_for(is_final_asr("voice-2"), 4000).await;
    assert!(heard_nothing(&last), "{last}");
    station.wait_for(response_to("req-2"), 4000).await;
    // No speech for VAD: the look at all audio after the wake word (on the fly too) hears the wake word alone.
    assert!(env.asr.transcriptions().len() > before);
    let timeline = env.voice_timeline("req-2").await;
    assert_eq!(timeline["outcome"], "noSpeech");
    assert!(timeline.get("utterance").is_none());
    assert!(timeline.get("quietSpeech").is_none());
    assert_eq!((&timeline["allAudio"]["raw"], &timeline["allAudio"]["text"]), (&json!("Алиса."), &json!("")));
    until(2000, "the all-audio file", || Path::new(timeline["allAudio"]["file"].as_str().unwrap()).exists()).await;
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hears_a_whisper_vad_cannot_all_audio_on_the_fly_the_end_when_the_text_stops_changing() {
    let env = env().await;
    env.asr.set_reply("Алиса, который час?");
    let station = env.station(&[]).await;
    station.start_voice("voice-6", 13, "req-6", None, None);
    // ~25 dB below the wake word: under VAD's threshold even after the gain.
    let audio = station_audio(&wake_word(), &concat(&[&silence(300.0), &voice(1000.0, -75.0), &silence(3000.0)]));
    station.send_audio(13, &audio.bytes[..audio.spotter_end_at]);
    station.send(json!({ "streamControl": { "streamId": 13, "messageId": "sc-8", "spotterEnd": {} } }));
    send_in_real_time(&station, 13, &audio.bytes[audio.spotter_end_at..], || station.has(is_final_asr("voice-6")))
        .await;

    let last = station.wait_for(is_final_asr("voice-6"), 4000).await;
    assert_eq!(normalized(&last), "Который час?");
    let final_at = station.index(is_final_asr("voice-6")).unwrap();
    let interims: Vec<Value> =
        station.received()[..final_at].iter().filter(|m| is_interim_asr("voice-6")(m)).cloned().collect();
    assert!(interims.iter().any(|m| normalized(m) == "Который час?"));

    let timeline = env.voice_timeline("req-6").await;
    // Ended by the unchanged text, before VAD's no-speech timeout (1.5 s here) would have, and not by it.
    assert_eq!(timeline["outcome"], "quietEndOfUtterance");
    assert_eq!(timeline["quietSpeech"], true);
    assert!(timeline["speech"]["vad"].get("speechStartMs").is_none());
    assert!(timeline.get("utterance").is_none());
    assert_eq!(timeline["allAudio"]["text"], "Который час?");
    // Only 5 dB over the room: no window loud enough to show a pitch, so it counts as a whisper.
    assert_eq!(timeline["voicing"], json!({ "windows": 0, "longestRun": 0 }));
    station.close();
}

/// A voice request without the wake word, as the station sends it for a wake word of our own (START_CONVERSATION).
fn without_wake_word(message_id: &str, stream_id: u32, request_id: &str) -> Value {
    json!({ "event": { "header": { "messageId": message_id, "streamId": stream_id }, "voiceInput": {
        "header": { "requestId": request_id, "sequenceNumber": 5 },
        "request": { "event": { "type": "voice_input" }, "voiceSession": true, "activationType": "directive" } } } })
}

/// A word, then the room: sent as the station would, until the final result.
async fn say_without_wake_word(station: &StationClient, message_id: &str, stream_id: u32, request_id: &str) {
    station.send(without_wake_word(message_id, stream_id, request_id));
    let audio = ogg_opus_chain(&concat(&[&voice(700.0, -50.0), &silence_at(3000.0, -75.0)]), stream_id);
    send_in_real_time(station, stream_id, &audio, || station.has(is_final_asr(message_id))).await;
    station.wait_for(is_final_asr(message_id), 4000).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_without_the_wake_word_starts_from_the_stations_wake_words_in_a_new_session_too() {
    let env = env().await;
    env.asr.set_reply("Который час?");
    let station = env.station(&[]).await;
    // No wake word heard from this station yet: a moderate gain (AUDIO_GAIN_NO_WAKE_WORD_DB), not +40 dB, at which
    // the room's noise is speech for the energy VAD.
    say_without_wake_word(&station, "voice-60", 61, "req-60").await;
    assert_eq!(env.voice_timeline("req-60").await["speech"]["gainDb"].as_f64(), Some(30.0));
    // «Алиса» picks the gain from itself.
    env.speak(&station, "voice-61", 63, "req-61", None, None);
    station.wait_for(response_to("req-61"), 4000).await;
    let picked = env.voice_timeline("req-61").await["speech"]["gainDb"].as_f64().unwrap();
    assert!((25.0..32.0).contains(&picked), "{picked}");
    station.close();

    // The station comes back (it reconnected, or the gateway restarted): this session has heard no wake word, the
    // station has — and only a wake word of our own may follow.
    let station = env.station(&[]).await;
    say_without_wake_word(&station, "voice-62", 65, "req-62").await;
    assert_eq!(env.voice_timeline("req-62").await["speech"]["gainDb"].as_f64(), Some(picked));
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stays_silent_when_the_station_cancels_the_request() {
    let env = env().await;
    let station = env.station(&[]).await;
    station.start_voice("voice-3", 5, "req-3", None, None);
    station.wait_for(|m| !at(m, "event.inputStartAck").is_null(), 4000).await;
    let audio = station_audio(&wake_word(), &voice(300.0, -50.0));
    station.send_audio(5, &audio.bytes[..audio.spotter_end_at]);
    station.send(
        json!({ "streamControl": { "streamId": 5, "messageId": "sc-3", "close": { "reason": "CLOSE_CANCEL" } } }),
    );
    sleep(300).await;
    assert!(!station.has(is_final_asr("voice-3")));
    assert!(!station.has(|m| !at(m, "event.aliceResponse").is_null()));
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keeps_the_log_spotter_audio_as_a_wav_and_acknowledges_the_log() {
    let env = env().await;
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-log").await;
    // The firmware, as a Lite 2 says it: in the spotter's log alone.
    station.send(json!({ "event": { "header": { "messageId": "log-1", "streamId": 7 },
        "logSpotter": { "format": "audio/x-pcm;bit=16;rate=16000;channels=1", "transcript": "* алиса", "source": "spotter",
            "firmware": "0.369.1.50.4794654278.20261021.133" } } }));
    station.wait_for(|m| at(m, "event.header.refMessageId") == "log-1" && !at(m, "event.logAck").is_null(), 4000).await;
    let pcm: Vec<u8> = voice(500.0, -40.0).iter().flat_map(|sample| sample.to_le_bytes()).collect();
    station.send_audio(7, &pcm);
    station.send(json!({ "streamControl": { "streamId": 7, "messageId": "sc-4", "close": {} } }));

    let mut note = Value::Null;
    for _ in 0..40 {
        if let Some(found) = env.all_events().await.into_iter().find(|e| e["kind"] == "spotterLog") {
            note = found;
            break;
        }
        sleep(50).await;
    }
    assert!(note["data"]["file"].as_str().unwrap().ends_with("_spotter_7.wav"));
    assert_eq!(note["data"]["transcript"], "* алиса");
    assert!(note["summary"].as_str().unwrap().contains("16000 bytes"));
    // Kept with the station's record: the panel says it, and that the gateway was tried with it.
    let mut stations = Value::Null;
    for _ in 0..40 {
        stations = env.get("/api/v1/stations").await;
        if stations.to_string().contains("0.369.1.50.4794654278.20261021.133") {
            break;
        }
        sleep(50).await;
    }
    assert!(stations.to_string().contains("\"firmware\":\"0.369.1.50.4794654278.20261021.133\""), "{stations}");
    station.close();
}

/// The finished trip of a request whose words are `text`, from the requests' journal.
async fn finished_trip(env: &Env, text: &str) -> Value {
    for _ in 0..120 {
        let journal = env.get("/api/v1/requests?limit=10").await;
        let found = journal["requests"].as_array().unwrap().iter().find(|view| view["text"] == text).cloned();
        if let Some(view) = found.filter(|view| view["status"] != "active") {
            return view;
        }
        sleep(50).await;
    }
    panic!("no finished request \"{text}\" in /api/v1/requests");
}

/// A trip answered by Yandex alone: Yandex heard it, Alice thought and spoke.
fn assert_heard_and_answered_by_yandex(trip: &Value) {
    assert_eq!(
        (&trip["kind"], &trip["status"], &trip["executor"], &trip["mode"], &trip["answer"]),
        (&json!("voice"), &json!("answered"), &json!("yandex"), &json!("gateway"), &json!("Включаю."))
    );
    let steps: Vec<(&str, &str)> = trip["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| (step["phase"].as_str().unwrap(), step["by"].as_str().unwrap_or("")))
        .collect();
    assert_eq!(steps, vec![("listen", "yandex"), ("think", "yandex"), ("speak", "yandex")]);
}

/// The station's voice with nothing here to recognise it, its stream closed by the station when it is done.
async fn speak_to_yandex_whole(station: &StationClient, message_id: &str, stream_id: u32, request_id: &str) -> usize {
    station.start_voice(message_id, stream_id, request_id, None, None);
    let audio = station_audio(&wake_word(), &concat(&[&voice(700.0, -50.0), &silence(1200.0)]));
    station.send_audio(stream_id, &audio.bytes);
    station.send(
        json!({ "streamControl": { "streamId": stream_id, "messageId": format!("sc-{message_id}"), "close": {} } }),
    );
    audio.bytes.len()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_recognition_the_voice_goes_to_yandex_whole_and_its_trip_has_yandexs_words_and_answer() {
    // A station's gateway with no computer and no cloud: «Шлюз» with nothing to recognise with (live, 07.11).
    let env = env_with(&[("ASR_PROVIDER", "none")]).await;
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-n").await;
    station.start_voice("voice-n", 21, "req-n", None, None);
    let audio = station_audio(&wake_word(), &concat(&[&voice(700.0, -50.0), &silence(1200.0)]));
    station.send_audio(21, &audio.bytes);

    let upstream = env.yandex.server.last();
    let mut frames = Vec::new();
    for _ in 0..40 {
        frames = upstream.frames();
        if frames.iter().any(|frame| frame.len() > 4 && frame[..4] == 21u32.to_be_bytes()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        frames.iter().any(|frame| String::from_utf8_lossy(frame).contains("voice-n")),
        "the station's VoiceInput goes to Yandex"
    );
    assert!(frames.iter().any(|frame| frame.len() > 4 && frame[..4] == 21u32.to_be_bytes()), "and its audio");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(station.index(is_final_asr("voice-n")).is_none(), "no empty answer of ours closes it");

    // The station is done speaking: Yandex hears it, answers it, speaks; the station gets all of it as Yandex said it.
    station.send(json!({ "streamControl": { "streamId": 21, "messageId": "sc-n", "close": {} } }));
    let heard = station.wait_for(is_final_asr("voice-n"), 4000).await;
    assert_eq!(normalized(&heard), "включи кино");
    station.wait_for(response_to("req-n"), 4000).await;
    station.wait_for(speak_of("voice-n"), 4000).await;

    // It is a request like any other: in the journal and on the panel, with Yandex's words and its answer.
    let trip = finished_trip(&env, "включи кино").await;
    assert_heard_and_answered_by_yandex(&trip);
    let answers = notes(&env, "yandexAnswer").await;
    assert!(answers.iter().any(|note| note["data"]["heard"] == "включи кино"), "{answers:?}");
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_recognition_a_voice_that_comes_before_our_session_with_yandex_is_up_waits_for_it_whole() {
    // The station reconnects on «Алиса» and speaks at once (live, 08.11: 77 ms after the socket), before our session
    // with Yandex is open: before, the request was cancelled and its audio went to Yandex on a stream it never heard
    // opened.
    let env = env_with(&[("ASR_PROVIDER", "none")]).await;
    env.yandex.server.open_delay_ms.store(800, std::sync::atomic::Ordering::SeqCst);
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-held").await;
    let sent = speak_to_yandex_whole(&station, "voice-held", 1, "req-held").await;

    station.wait_for(response_to("req-held"), 8000).await;
    let inputs = env.yandex.voice_inputs();
    let input = inputs.iter().find(|input| input.message_id == "voice-held").expect("the VoiceInput reaches Yandex");
    assert_eq!(
        (input.stream_id, input.audio_bytes, input.closed),
        (1, sent, true),
        "with all its audio, then its close"
    );
    assert!(!notes(&env, "voiceHeld").await.is_empty());
    assert_heard_and_answered_by_yandex(&finished_trip(&env, "включи кино").await);
    assert!(notes(&env, "strayYandexStream").await.is_empty());
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_recognition_what_yandex_says_on_the_stations_own_stream_goes_to_the_station_the_rest_is_stray() {
    let env = env_with(&[("ASR_PROVIDER", "none")]).await;
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-own").await;
    speak_to_yandex_whole(&station, "voice-own", 21, "req-own").await;
    station.wait_for(response_to("req-own"), 8000).await;
    finished_trip(&env, "включи кино").await;

    // Yandex closes the station's stream (the station's numbering, given to Yandex as it was): as in «Яндекс», the
    // station gets it as it is. A stream that is nobody's stays dropped and noted.
    let upstream = env.yandex.server.last();
    let close =
        |stream: u32, id: &str| json!({ "streamControl": { "streamId": stream, "messageId": id, "close": {} } });
    upstream.send(server_frame(&close(21, "y-close-21")));
    let passed = station.wait_for(|m| at(m, "streamControl.messageId") == "y-close-21", 4000).await;
    assert_eq!(at(&passed, "streamControl.streamId"), 21);
    upstream.send(server_frame(&close(40, "y-close-40")));
    let mut stray = Vec::new();
    for _ in 0..40 {
        stray = notes(&env, "strayYandexStream").await;
        if !stray.is_empty() {
            break;
        }
        sleep(50).await;
    }
    assert_eq!(stray.len(), 1);
    assert!(stray[0]["summary"].as_str().unwrap().contains("Yandex stream 40 (a stream control)"), "{stray:?}");
    assert!(station.index(|m| at(m, "streamControl.messageId") == "y-close-40").is_none());
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answers_in_the_voice_of_alice_the_phrases_go_to_yandex_through_our_session_one_stream_comes_to_the_station() {
    let env = env().await;
    env.llm.set_reply("Сейчас половина восьмого утра. Хорошего дня!");
    let station = env.station(STATION_HEADERS).await;
    let sync_frame = station.synchronize("sync-7").await;
    station.start_voice("voice-7", 15, "req-7", None, None);
    let audio = station_audio(&wake_word(), &concat(&[&voice(700.0, -50.0), &silence(1200.0)]));
    station.send_audio(15, &audio.bytes[..audio.spotter_end_at]);
    station.send(json!({ "streamControl": { "streamId": 15, "messageId": "sc-9", "spotterEnd": {} } }));
    station.send_audio(15, &audio.bytes[audio.spotter_end_at..]);

    let speak = station.wait_for(speak_of("voice-7"), 8000).await;
    let stream_id = at(&speak, "event.header.streamId").as_u64().unwrap();
    assert_eq!(stream_id % 2, 0);
    station.wait_for(close_of(stream_id), 8000).await;

    // Our session with Yandex: the station's headers and its very SynchronizeState frame, then a TtsGenerate per
    // phrase.
    let upstream = env.yandex.server.last();
    assert_eq!(upstream.headers["x-uprx-oauth-token"], "y0_test-secret");
    assert_eq!(upstream.headers["user-agent"], "WebSocket++/0.8.2");
    assert_eq!(upstream.frames()[0], sync_frame);
    let tts = env.yandex.tts_full();
    assert_eq!(
        tts[tts.len() - 2..].to_vec(),
        vec![
            ("Сейчас половина восьмого утра.".to_string(), Some("ru-RU".to_string()), Some("audio/opus".to_string())),
            ("Хорошего дня!".to_string(), Some("ru-RU".to_string()), Some("audio/opus".to_string())),
        ]
    );

    // The early response: no text yet, the jingle, speech promised; listening is asked for before the text is known.
    let response = station.wait_for(response_to("req-7"), 4000).await;
    let alice = at(&response, "event.aliceResponse");
    assert_eq!(at(alice, "voiceResponse"), &json!({ "hasVoiceResponse": true, "shouldListen": true }));
    assert_eq!(directive_names(&response), vec!["tts_play_placeholder"]);
    let order = [
        station
            .index(|m| at(m, "event.header.refMessageId") == "voice-7" && !at(m, "event.spotterValidation").is_null()),
        station.index(response_to("req-7")),
        station.index(speak_of("voice-7")),
        station.index(|m| at(m, "audio.streamId") == stream_id),
        station.index(close_of(stream_id)),
    ];
    let order: Vec<usize> = order.into_iter().map(Option::unwrap).collect();
    let mut sorted = order.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, order);
    // Both phrases in one Ogg stream, 15 packets each, ending with EOS.
    assert_eq!(station.stream(stream_id as u32), (1, 30, true));

    let timeline = env.answer_timeline("voice-7").await;
    assert_eq!(timeline["outcome"], "spoken");
    assert_eq!(timeline["question"], "Привет.");
    assert_eq!(timeline["text"], "Сейчас половина восьмого утра. Хорошего дня!");
    assert_eq!((&timeline["phrases"], &timeline["streamId"]), (&json!(2), &json!(stream_id)));
    assert_eq!((&timeline["audioMs"], &timeline["expectsFollowUp"]), (&json!(600), &json!(false)));

    // The answer asked nothing: the station's listening after it is closed at once.
    station.send(json!({ "event": { "header": { "messageId": "voice-7b", "streamId": 19 }, "voiceInput": {
        "header": { "requestId": "req-7b", "sequenceNumber": 4 },
        "request": { "event": { "type": "voice_input" }, "voiceSession": true, "activationType": "auto_listening" } } } }));
    let dismissed = station.wait_for(is_final_asr("voice-7b"), 4000).await;
    assert!(heard_nothing(&dismissed), "{dismissed}");
    station.wait_for(close_of(19), 4000).await;
    let empty = station.wait_for(response_to("req-7b"), 4000).await;
    assert_eq!(at(&empty, "event.aliceResponse.voiceResponse.hasVoiceResponse"), false);
    // The station's token and account id never reach the journal.
    let journal = Value::Array(env.all_events().await).to_string();
    assert!(!journal.contains("y0_test-secret"));
    assert!(!journal.contains("123456"));
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn speaks_a_text_the_station_asks_for_itself_tts_generate_without_an_alice_response() {
    let env = env().await;
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-8").await;
    station.send(json!({ "event": { "header": { "messageId": "tts-1" },
        "ttsGenerate": { "text": "Проверка связи.", "lang": "ru-RU", "format": "audio/opus" } } }));
    let speak = station.wait_for(speak_of("tts-1"), 4000).await;
    let stream_id = at(&speak, "event.header.streamId").as_u64().unwrap();
    station.wait_for(close_of(stream_id), 4000).await;
    assert_eq!(env.yandex.tts().last().unwrap(), "Проверка связи.");
    assert_eq!(station.stream(stream_id as u32), (1, 15, true));
    assert!(!station.has(|m| !at(m, "event.aliceResponse").is_null()));
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn speaks_a_text_from_the_debug_api_on_the_latest_station_session_as_the_answer_to_its_callback_of_our_push() {
    let env = env().await;
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-9").await;
    let reply = env.post("/debug/sessions/latest/say", json!({ "text": "Тест озвучки." }), StatusCode::CREATED).await;
    assert_eq!(reply["outcome"], "spoken");
    // As Yandex rings a reminder: a push, the station's callback, and the speech as the answer to it.
    let push = lock(&station.pushes).last().cloned().unwrap();
    assert_eq!((&push["directive"]["type"], &push["directive"]["name"]), (&json!("server_action"), &json!("vgw_say")));
    assert!(push["source"].as_str().unwrap().starts_with("notificator-internal-"));
    let callback = format!("callback-{}", push["id"].as_str().unwrap());
    let answer = station
        .wait_for(
            |m| at(m, "event.header.refMessageId") == callback.as_str() && !at(m, "event.aliceResponse").is_null(),
            4000,
        )
        .await;
    assert_eq!(at(&answer, "event.aliceResponse.voiceResponse.outputSpeech.text"), "Тест озвучки.");
    assert_eq!(at(&answer, "event.aliceResponse.voiceResponse.shouldListen"), false);
    // The speech itself may come a moment after the answer: waited for, not looked up at once (a slow CI runner saw
    // the answer before it).
    station
        .wait_for(
            |m| !at(m, "event.ttsSpeak").is_null() && at(m, "event.header.refMessageId") == callback.as_str(),
            4000,
        )
        .await;

    // A station that never calls back: nothing is spoken, the speech counts as failed.
    station.answers_pushes.store(false, std::sync::atomic::Ordering::SeqCst);
    let unanswered =
        env.post("/debug/sessions/latest/say", json!({ "text": "Никто не спросил." }), StatusCode::CREATED).await;
    assert_eq!(unanswered["outcome"], "failed");
    assert!(!env.yandex.tts().iter().any(|text| text == "Никто не спросил."));
    station.answers_pushes.store(true, std::sync::atomic::Ordering::SeqCst);

    // The silent check synthesizes through Yandex and plays nothing.
    let sent = station.received().len();
    let check = env.post("/debug/sessions/latest/tts", json!({ "text": "Тихая проверка." }), StatusCode::CREATED).await;
    assert_eq!((&check["format"], &check["audioMs"]), (&json!("audio/opus"), &json!(300)));
    assert_eq!(env.yandex.tts().last().unwrap(), "Тихая проверка.");
    assert_eq!(station.received().len(), sent);
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn speaks_to_the_end_when_the_caller_of_say_leaves_before_the_station_calls_back() {
    let env = env().await;
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-left").await;
    station.answers_pushes.store(false, std::sync::atomic::Ordering::SeqCst);
    let (name, value) = bearer(&env.token);
    let left = env
        .gateway
        .post("/debug/sessions/latest/say")
        .header(name, value)
        .json(&json!({ "text": "Договорю до конца." }))
        .timeout(std::time::Duration::from_millis(300))
        .send()
        .await;
    assert!(left.is_err(), "the caller gave up");
    // The station calls back only now, its caller gone: as a Node promise did, the speech goes on to its end.
    let push = lock(&station.pushes).last().cloned().expect("our push");
    let callback = format!("callback-{}", push["id"].as_str().unwrap());
    station.send(json!({ "event": {
        "header": { "messageId": callback },
        "textInput": {
            "header": { "requestId": format!("req-{callback}") },
            "request": { "event": { "type": "server_action", "name": "vgw_say", "payload": push["directive"]["payload"] } },
        },
    } }));
    let answer = station
        .wait_for(
            |m| at(m, "event.header.refMessageId") == callback.as_str() && !at(m, "event.aliceResponse").is_null(),
            4000,
        )
        .await;
    assert_eq!(at(&answer, "event.aliceResponse.voiceResponse.outputSpeech.text"), "Договорю до конца.");
    let speech = station.wait_for(speak_of(&callback), 4000).await;
    let stream = at(&speech, "event.header.streamId").as_u64().expect("the speech's stream");
    station.wait_for(close_of(stream), 4000).await;
    station.close();
}

/// A slow model's answer, asked by text: the early response comes at once, the words after `delay_ms`.
async fn slow_answer(env: &Env, sync_id: &str, message_id: &str, delay_ms: u64) -> StationClient {
    env.llm.set_delay(delay_ms);
    let (station, _) = env.connected_station(sync_id).await;
    let event = json!({ "type": "text_input", "text": "Расскажи, что нового в мире" });
    station.text_input(message_id, &format!("req-{message_id}"), event);
    let response = station.wait_for(response_to(&format!("req-{message_id}")), 4000).await;
    assert_eq!(at(&response, "event.aliceResponse.voiceResponse.outputSpeech.text").as_str().unwrap_or(""), "");
    station
}

/// The journal's notes of a kind.
async fn notes(env: &Env, kind: &str) -> Vec<Value> {
    env.all_events().await.into_iter().filter(|e| e["kind"] == kind).collect()
}

/// The deferredAnswer events of a request, as `event: summary`.
async fn deferred_events(env: &Env, r#ref: &str) -> Vec<String> {
    notes(env, "deferredAnswer")
        .await
        .iter()
        .filter(|e| e["data"]["ref"] == r#ref)
        .map(|e| e["data"]["event"].as_str().unwrap_or("").to_string())
        .collect()
}

/// OpusHead count and packet count of the audio a stream carried.
fn heads_and_packets(station: &StationClient, stream_id: u64) -> (usize, usize) {
    let audio = lock(&station.audio).get(&(stream_id as u32)).cloned().unwrap_or_default().concat();
    let events = OggOpusDemuxer::new().push(&audio);
    let heads = events.iter().filter(|event| matches!(event, OggOpusEvent::Head { .. })).count();
    (heads, events.len() - heads)
}

/// A slow model, as a web search or a long MCP tool is: the station drops speech that keeps it 10 s without audio, so
/// silence keeps its stream alive until the words come, and the words are heard on that stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keeps_the_station_waiting_with_silence_on_the_stream_and_the_slow_answer_is_heard_on_it() {
    let env = env_with(&[("LLM_TIMEOUT_MS", "30000")]).await;
    env.llm.set_reply("В мире всё спокойно.");
    let station = slow_answer(&env, "sync-slow", "text-slow", 8000).await;
    // The model has said nothing yet, the station already hears the stream: silence from 3 s on.
    let speak = station.wait_for(speak_of("text-slow"), 4500).await;
    let stream = at(&speak, "event.header.streamId").as_u64().unwrap();
    until(1000, "silence on the stream", || lock(&station.audio).contains_key(&(stream as u32))).await;
    assert!(!env.yandex.tts().iter().any(|text| text.contains("спокойно")), "the words are not there yet");
    station.wait_for(close_of(stream), 15_000).await;
    assert!(env.yandex.tts().iter().any(|text| text.contains("В мире всё спокойно.")));
    let timeline = env.answer_timeline("text-slow").await;
    assert_eq!(timeline["outcome"], "spoken");
    let (speech, silence) = (timeline["audioMs"].as_u64().unwrap(), timeline["silenceMs"].as_u64().unwrap());
    assert!(silence >= 1500 && speech > 0, "{timeline}");
    // One Ogg/Opus stream: one head, the silence and the speech in it.
    assert_eq!(heads_and_packets(&station, stream), (1, ((silence + speech) / 20) as usize));
    station.close();
}

/// Past ANSWER_WAIT_MAX_MS the stream closes cleanly and the answer goes on in the background; its words come
/// later through our push and the station's callback, as a reminder's do.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closes_the_stream_at_the_wait_limit_and_says_the_late_answer_through_a_push() {
    let env = env_with(&[("LLM_TIMEOUT_MS", "30000"), ("ANSWER_WAIT_MAX_MS", "5000")]).await;
    env.llm.set_reply("Нашлось наконец.");
    let station = slow_answer(&env, "sync-late", "text-late", 9000).await;
    let speak = station.wait_for(speak_of("text-late"), 4500).await;
    let stream = at(&speak, "event.header.streamId").as_u64().unwrap();
    let close = station.wait_for(close_of(stream), 4000).await;
    assert_eq!(at(&close, "streamControl.close.reason"), "CLOSE_OK", "{close}");
    assert_eq!(heads_and_packets(&station, stream).0, 1);
    assert!(lock(&station.pushes).is_empty(), "nothing is said before the words are there");
    let timeline = env.answer_timeline("text-late").await;
    assert_eq!((&timeline["outcome"], &timeline["audioMs"]), (&json!("deferred"), &json!(0)));
    assert!(timeline["deferred"].as_str().unwrap().contains("no speech in 5 s"), "{timeline}");

    until(10_000, "our push", || !lock(&station.pushes).is_empty()).await;
    let push = lock(&station.pushes)[0].clone();
    let callback = format!("callback-{}", push["id"].as_str().unwrap());
    let answer = station
        .wait_for(
            |m| at(m, "event.header.refMessageId") == callback.as_str() && !at(m, "event.aliceResponse").is_null(),
            6000,
        )
        .await;
    assert_eq!(at(&answer, "event.aliceResponse.voiceResponse.outputSpeech.text"), "Нашлось наконец.");
    station.wait_for(speak_of(&callback), 4000).await;
    until_async(4000, "the words said", || async {
        deferred_events(&env, "text-late").await.contains(&"spoken".into())
    })
    .await;
    assert_eq!(deferred_events(&env, "text-late").await, ["deferred", "saying", "spoken"]);
    station.close();
}

/// The station gives up as on 06.11 01:12: it closes the answer's stream, then says why (requestStat «Synthesis chunk
/// timeout»). Nothing more goes to that stream, and the answer is deferred at once; its words come through our push.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn takes_the_stations_timeout_for_the_answer_as_a_deferral_at_once() {
    let env = env_with(&[("LLM_TIMEOUT_MS", "30000")]).await;
    env.llm.set_reply("Ответ после тайм-аута.");
    let station = slow_answer(&env, "sync-gave-up", "text-gave-up", 7000).await;
    let speak = station.wait_for(speak_of("text-gave-up"), 4500).await;
    let stream = at(&speak, "event.header.streamId").as_u64().unwrap();
    until(1000, "silence on the stream", || lock(&station.audio).contains_key(&(stream as u32))).await;
    station.send(json!({ "streamControl": { "streamId": stream, "messageId": "close-gave-up", "close": {} } }));
    station.send(json!({ "event": {
        "header": { "messageId": "stat-gave-up" },
        "requestStat": {
            "requestMessageId": "text-gave-up",
            "error": "asr:TtsPlayingError: Synthesis chunk timeout elapsed",
        },
    } }));
    let timeline = env.answer_timeline("text-gave-up").await;
    assert_eq!(timeline["outcome"], "deferred");
    assert!(timeline["deferred"].as_str().unwrap().contains("Synthesis chunk timeout"), "{timeline}");
    let frames = lock(&station.audio)[&(stream as u32)].len();
    until(12_000, "our push", || !lock(&station.pushes).is_empty()).await;
    let callback = format!("callback-{}", lock(&station.pushes)[0]["id"].as_str().unwrap());
    let answer = station
        .wait_for(
            |m| at(m, "event.header.refMessageId") == callback.as_str() && !at(m, "event.aliceResponse").is_null(),
            6000,
        )
        .await;
    assert_eq!(at(&answer, "event.aliceResponse.voiceResponse.outputSpeech.text"), "Ответ после тайм-аута.");
    // The stream the station closed got nothing more, not even its close from us.
    assert_eq!(lock(&station.audio)[&(stream as u32)].len(), frames);
    assert!(!station.has(close_of(stream)));
    station.close();
}

/// «Алиса, стоп» with nothing playing drops the answers still to be said: the late words never come.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drops_a_deferred_answer_on_stop_with_nothing_else_to_stop() {
    let env = env_with(&[("LLM_TIMEOUT_MS", "30000")]).await;
    env.llm.set_reply("Этого уже не надо.");
    let station = slow_answer(&env, "sync-stop", "text-stop", 5000).await;
    station.send(json!({ "event": {
        "header": { "messageId": "stat-stop" },
        "requestStat": { "requestMessageId": "text-stop", "error": "asr:TtsPlayingError: Synthesis chunk timeout elapsed" },
    } }));
    until_async(4000, "the deferral", || async {
        deferred_events(&env, "text-stop").await.contains(&"deferred".into())
    })
    .await;
    env.asr.set_reply("Алиса, стоп.");
    env.speak(
        &station,
        "voice-stop",
        71,
        "req-stop",
        Some(json!({ "soundLevel": 5, "audioPlayer": { "playerState": 5 } })),
        None,
    );
    let response = station.wait_for(response_to("req-stop"), 8000).await;
    assert_eq!(at(&response, "event.aliceResponse.voiceResponse.hasVoiceResponse"), false);
    until_async(4000, "the drop", || async { deferred_events(&env, "text-stop").await.contains(&"dropped".into()) })
        .await;
    let dropped = notes(&env, "deferredAnswer").await;
    assert!(dropped.iter().any(|e| e["summary"].as_str().unwrap().contains("dropped: «стоп»")), "{dropped:?}");
    // Past the model's 5 s: no push, no words.
    sleep(3500).await;
    assert!(lock(&station.pushes).is_empty());
    assert!(!env.yandex.tts().iter().any(|text| text.contains("не надо")));
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stops_the_answer_when_the_station_cancels_it_and_closes_the_stream_as_cancelled() {
    let env = env().await;
    env.yandex.set_chunk_delay(150);
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-10").await;
    env.speak(&station, "voice-8", 17, "req-8", None, None);
    let speak = station.wait_for(speak_of("voice-8"), 8000).await;
    let stream_id = at(&speak, "event.header.streamId").as_u64().unwrap();
    station.send(json!({ "event": { "header": { "messageId": "cancel-1" }, "cancelRequest": { "requestMessageId": "voice-8", "interruption": {} } } }));
    let close = station.wait_for(close_of(stream_id), 4000).await;
    assert_eq!(at(&close, "streamControl.close.reason"), "CLOSE_CANCEL");
    assert_eq!(env.answer_timeline("voice-8").await["outcome"], "cancelled");
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drops_the_speech_of_a_cancelled_answer_that_yandex_sends_late_it_is_not_a_push() {
    let env = env().await;
    env.yandex.set_tts_delay(800);
    let (station, _) = env.connected_station("sync-32").await;
    let synthesized = env.yandex.tts().len();
    env.speak(&station, "voice-32", 45, "req-32", None, None);
    station.wait_for(response_to("req-32"), 8000).await;
    until(3000, "a phrase for Yandex", || env.yandex.tts().len() > synthesized).await;
    station.send(json!({ "event": { "header": { "messageId": "cancel-32" }, "cancelRequest": { "requestMessageId": "voice-32", "interruption": {} } } }));
    let after = station.received().len();
    sleep(1500).await;
    // Live, the late TtsSpeak and audio went to the station as Yandex's push.
    let late: Vec<Value> = station.received()[after..]
        .iter()
        .filter(|m| !at(m, "event.ttsSpeak").is_null() || !at(m, "streamControl").is_null())
        .cloned()
        .collect();
    assert_eq!(late, Vec::<Value>::new());
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keeps_speaking_while_the_player_asks_yandex_for_its_next_track() {
    let env = env().await;
    env.yandex.set_chunk_delay(150);
    env.yandex.answer(Some(Box::new(|input| {
        (input.name.as_deref() == Some("@@mm_stack_engine_get_next")).then(|| FakeAnswer {
            directives: vec![client_directive(
                "audio_play",
                json!({ "stream": { "url": "https://music.example/next" } }),
            )],
            ..FakeAnswer::default()
        })
    })));
    let (station, _) = env.connected_station("sync-31").await;
    env.speak(&station, "voice-31", 43, "req-31", None, None);
    let answer = station.wait_for(speak_of("voice-31"), 8000).await;
    station.text_input(
        "callback-31",
        "req-31-next",
        json!({ "type": "server_action", "name": "@@mm_stack_engine_get_next" }),
    );
    let next = station.wait_for(response_to("req-31-next"), 4000).await;
    assert_eq!(directive_names(&next), vec!["audio_play"]);
    let stream_id = at(&answer, "event.header.streamId").as_u64().unwrap();
    let close = station.wait_for(close_of(stream_id), 8000).await;
    assert_ne!(at(&close, "streamControl.close.reason"), "CLOSE_CANCEL");
    assert_ne!(env.answer_timeline("voice-31").await["outcome"], "cancelled");
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runs_a_quick_command_itself_gromche_is_a_volume_command_in_the_response_without_the_model() {
    let env = env().await;
    env.asr.set_reply("Алиса, громче.");
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-11").await;
    let before = env.llm.requests().len();
    env.speak(&station, "voice-11", 21, "req-11", None, None);

    let response = station.wait_for(response_to("req-11"), 4000).await;
    // The request carries no device state here, so the relative command goes; with it, sound_set_level.
    assert_eq!(directive_names(&response), vec!["sound_louder", "tts_play_placeholder"]);
    assert_eq!(at(&response, "event.aliceResponse.voiceResponse.hasVoiceResponse"), false);
    assert_eq!(env.llm.requests().len(), before);
    let route = env.route("Громче.").await;
    assert_eq!(
        (&route["capability"], &route["executor"], &route["source"], &route["detail"]),
        (&json!("volume"), &json!("own"), &json!("quick"), &json!("louder"))
    );
    // A command without words is an answer, not silence (live 05.11: the radio's audio_play journaled as silent).
    let mut found = Value::Null;
    for _ in 0..40 {
        let journal = env.get("/api/v1/requests?limit=20").await;
        if let Some(view) = journal["requests"].as_array().unwrap().iter().find(|view| view["text"] == "Громче.")
        {
            if view["status"] != "active" {
                found = view.clone();
                break;
            }
        }
        sleep(50).await;
    }
    assert_eq!(
        (&found["status"], &found["capability"], &found["executor"]),
        (&json!("answered"), &json!("volume"), &json!("own"))
    );
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sets_our_own_timer_the_timer_in_field_15_of_timer_set_directive_and_the_speech_yandex_would_say() {
    let env = env().await;
    env.asr.set_reply("Алиса, поставь таймер на минуту.");
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-23").await;
    env.speak(&station, "voice-23", 39, "req-23", None, None);
    let response = station.wait_for(response_to("req-23"), 8000).await;
    let directives = at(&response, "event.aliceResponse.response.directives").as_array().unwrap().clone();
    assert_eq!(directive_names(&response), vec!["timer_set_directive", "tts_play_placeholder"]);
    assert_eq!(directives[0]["analyticsType"], "set_timer_capability_directive");
    let typed = duowave::gateway::protocol::proto_object::value_bytes(&directives[0]["typedPayload"]).unwrap();
    assert!(typed.len() > 40);
    assert!(directives[0].get("payload").is_none());
    station.wait_for(speak_of("voice-23"), 8000).await;
    assert_eq!(env.yandex.tts().last().unwrap(), "Хорошо. 1 минута. Отсчёт пошёл.");
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn says_not_yet_for_a_capability_whose_route_is_off_and_journals_and_counts_it() {
    let env = env().await;
    env.asr.set_reply("Алиса, включи режим не беспокоить.");
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-12").await;
    env.speak(&station, "voice-12", 23, "req-12", None, None);
    station.wait_for(speak_of("voice-12"), 8000).await;
    assert_eq!(env.yandex.tts().last().unwrap(), "Этого я пока не умею.");

    let unsupported = env.get("/debug/unsupported").await;
    assert_eq!(
        (&unsupported[0]["capability"], &unsupported[0]["route"], &unsupported[0]["reason"]),
        (&json!("do_not_disturb"), &json!("off"), &json!("route off"))
    );
    let health = message(env.gateway.get("/health"), StatusCode::OK).await;
    assert_eq!(
        (&health["skills"]["routes"]["do_not_disturb"], &health["skills"]["routes"]["chat"]),
        (&json!("off"), &json!("own,yandex"))
    );
    assert!(health["skills"]["unsupported"]["do_not_disturb"].as_u64().unwrap() >= 1);
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lets_the_model_pick_a_tool_for_a_command_the_rules_do_not_know_and_runs_it() {
    let env = env().await;
    env.asr.set_reply("Алиса, сделай так, чтобы музыка замолчала.");
    env.llm.set_tool_call(Some(("control_playback", json!({ "action": "stop" }))));
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-13").await;
    env.speak(&station, "voice-13", 25, "req-13", None, None);

    let response = station.wait_for(response_to("req-13"), 8000).await;
    assert_eq!(directive_names(&response), vec!["player_pause", "clear_queue", "tts_play_placeholder"]);
    // The command was offered every tool.
    let call = env.llm.requests().last().cloned().unwrap();
    let tools: Vec<&str> =
        call["tools"].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
    assert!(tools.contains(&"control_playback"));
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn takes_the_answer_to_the_models_question_about_a_command_as_a_command() {
    let env = env().await;
    env.asr.set_reply("Алиса, поставь таймер.");
    env.llm.set_reply("На сколько поставить таймер?");
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-13b").await;
    env.speak(&station, "voice-13b", 67, "req-13b", None, None);
    let question = station.wait_for(response_to("req-13b"), 8000).await;
    assert_eq!(at(&question, "event.aliceResponse.voiceResponse.shouldListen"), true);

    // Said without the wake word, nothing in it looks like a command; it answers the question all the same.
    env.asr.set_reply("На две минуты.");
    env.llm.set_tool_call(Some(("set_timer", json!({ "seconds": 120 }))));
    env.speak(&station, "voice-13c", 69, "req-13c", None, None);
    let response = station.wait_for(response_to("req-13c"), 8000).await;
    assert!(directive_names(&response).contains(&"timer_set_directive".to_string()));
    let last = env.llm.requests().last().cloned().unwrap();
    let tools: Vec<&str> =
        last["tools"].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
    assert!(tools.contains(&"set_timer"));
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answers_a_typed_question_text_input_the_same_way() {
    let env = env().await;
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-14").await;
    station.text_input("text-1", "req-14", json!({ "type": "text_input", "text": "Который час?" }));
    let response = station.wait_for(response_to("req-14"), 8000).await;
    // The clock answers without the model: speech, no listening.
    let speech = at(&response, "event.aliceResponse.voiceResponse.outputSpeech.text").as_str().unwrap().to_string();
    let clock = regex::Regex::new(r"^\d{2}:\d{2}\.$").unwrap();
    assert!(clock.is_match(&speech), "{speech}");
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hears_the_emergency_words_before_the_routes_and_the_model_and_lets_yandex_answer_by_them() {
    let env = env().await;
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-rescue").await;
    station.text_input(
        "rescue-1",
        "req-rescue",
        json!({ "type": "text_input", "text": "Переведи шлюз в режим Яндекса" }),
    );
    let response = station.wait_for(response_to("req-rescue"), 8000).await;
    let speech = at(&response, "event.aliceResponse.voiceResponse.outputSpeech.text").as_str().unwrap().to_string();
    assert!(speech.contains("режим Яндекса"), "{speech}");
    // Once the station has said it, the mode is «Яндекс»: the station reconnects into the relay.
    let mut mode = Value::Null;
    for _ in 0..150 {
        mode = env.get("/debug/mode").await["mode"].clone();
        if mode == "relay" {
            break;
        }
        sleep(100).await;
    }
    assert_eq!(mode, "relay");
    let events = env.all_events().await;
    assert!(events.iter().any(|event| event["kind"] == "rescue" || event["type"] == "rescue"), "journaled");
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follows_requests_live_on_the_panels_channel_the_trip_the_words_the_end_and_keeps_them_in_the_journal() {
    let env = env().await;
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-live").await;
    station.text_input("text-live", "req-live", json!({ "type": "text_input", "text": "Который час?" }));
    let clock = regex::Regex::new(r"^\d{2}:\d{2}\.$").unwrap();
    let mut done = Value::Null;
    for _ in 0..80 {
        let journal = env.get("/api/v1/requests?limit=10").await;
        if let Some(view) =
            journal["requests"].as_array().unwrap().iter().find(|v| v["text"] == "Который час?" && v["kind"] == "text")
        {
            if view["status"] != "active" {
                done = view.clone();
                break;
            }
        }
        sleep(50).await;
    }
    assert_eq!(
        (&done["status"], &done["capability"], &done["executor"], &done["mode"]),
        (&json!("answered"), &json!("chat"), &json!("own"), &json!("gateway"))
    );
    assert!(clock.is_match(done["answer"].as_str().unwrap()));
    let phases: Vec<&str> = done["steps"].as_array().unwrap().iter().map(|s| s["phase"].as_str().unwrap()).collect();
    assert_eq!(phases, vec!["think", "speak"]);
    assert_eq!(done["steps"][0]["by"], "own");
    assert_eq!((&done["steps"][1]["by"], &done["steps"][1]["what"]), (&json!("yandex"), &json!("Алиса")));

    // By voice: our ear listens and recognises, the clock answers, Alice's voice speaks.
    env.asr.set_reply("Алиса, который час?");
    env.speak(&station, "voice-live", 71, "req-voice-live", None, None);
    let mut trip = Value::Null;
    for _ in 0..120 {
        let journal = env.get("/api/v1/requests?limit=10").await;
        if let Some(view) = journal["requests"].as_array().unwrap().iter().find(|v| {
            v["kind"] == "voice" && v["text"].as_str().is_some_and(|text| text.to_lowercase().contains("который час"))
        }) {
            if view["status"] != "active" {
                trip = view.clone();
                break;
            }
        }
        sleep(50).await;
    }
    assert_eq!(trip["status"], "answered");
    let phases: Vec<&str> = trip["steps"].as_array().unwrap().iter().map(|s| s["phase"].as_str().unwrap()).collect();
    assert_eq!(phases, vec!["listen", "recognize", "think", "speak"]);
    assert_eq!((&trip["steps"][1]["by"], &trip["steps"][1]["what"]), (&json!("own"), &json!("test-model")));
    let journal = env.get("/api/v1/requests?limit=10").await;
    assert_eq!(journal["keepDays"], 30);
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answers_early_with_the_jingle_when_our_skill_is_slow_to_speak_the_weather() {
    let env = env().await;
    env.asr.set_reply("Алиса, какая погода?");
    env.llm.set_reply("Сейчас плюс десять и ясно.");
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-14e").await;
    env.speak(&station, "voice-14e", 65, "req-14e", None, None);
    let response = station.wait_for(response_to("req-14e"), 8000).await;
    // Out before any text, as soon as the forecast came: with the temperature for the display, while the model still
    // words the forecast.
    assert_eq!(at(&response, "event.aliceResponse.voiceResponse.outputSpeech.text").as_str().unwrap_or(""), "");
    assert_eq!(directive_names(&response), vec!["draw_animation_directive", "tts_play_placeholder"]);
    assert_ne!(at(&response, "event.aliceResponse.voiceResponse.shouldListen"), true);
    station.wait_for(speak_of("voice-14e"), 8000).await;
    let route = env.route("Какая погода?").await;
    assert_eq!((&route["capability"], &route["executor"]), (&json!("weather"), &json!("own")));
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asks_yandex_what_the_model_asked_for_not_what_the_user_said() {
    let env = env().await;
    env.llm.set_tool_call(Some(("get_news", json!({}))));
    env.llm.set_reply("Главное к этому часу: тест прошёл.");
    env.yandex.answer(Some(Box::new(|input| {
        (input.text.as_deref() == Some("Расскажи новости"))
            .then(|| FakeAnswer {
                speech: Some("Главное к этому часу: тест прошёл.".into()), ..FakeAnswer::default()
            })
    })));
    let (station, _) = env.connected_station("sync-14b").await;
    station.text_input("text-1b", "req-14b", json!({ "type": "text_input", "text": "Что нового?" }));
    until(8000, "Yandex asked for the news", || {
        env.yandex.text_inputs().iter().any(|input| input.text.as_deref() == Some("Расскажи новости"))
    })
    .await;
    assert!(!env.yandex.text_inputs().iter().any(|input| input.text.as_deref() == Some("Что нового?")));
    until(8000, "the model got the tool's result", || {
        env.llm
            .requests()
            .iter()
            .any(|r| r["messages"].as_array().is_some_and(|m| m.iter().any(|m| m["role"] == "tool")))
    })
    .await;
    let mut lookup = Value::Null;
    for _ in 0..40 {
        if let Some(found) = env
            .all_events()
            .await
            .into_iter()
            .find(|e| e["kind"] == "yandexLookup" && e["data"]["phrase"] == "Что нового?")
        {
            lookup = found;
            break;
        }
        sleep(50).await;
    }
    assert!(lookup["summary"]
        .as_str()
        .unwrap()
        .starts_with("\"Расскажи новости\" (for \"Что нового?\") → \"Главное к этому часу"));
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answers_with_a_connected_mcp_server_in_a_conversation_and_in_a_command_and_withholds_what_acts_after_a_text_from_outside(
) {
    let env = env().await;
    let url = fakes::mcp_server::start().await;
    let added = env
        .post(
            "/api/v1/mcp/servers",
            json!({ "transport": "http", "title": "Fixture", "url": url }),
            StatusCode::CREATED,
        )
        .await;
    let id = added["id"].as_str().unwrap().to_string();
    until_async(5000, "the fixture server ready", || async {
        let view = env.get("/api/v1/mcp").await;
        view["servers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|server| server["id"] == id.as_str() && server["state"] == "ready")
    })
    .await;
    for tool in ["lookup", "add_note"] {
        let (name, value) = bearer(&env.token);
        let put = env
            .gateway
            .put(&format!("/api/v1/mcp/servers/{id}/tools/{tool}"))
            .header(name, value)
            .json(&json!({ "on": true }));
        assert_eq!(send(put).await.0, StatusCode::OK);
    }
    let tools_of = |request: &Value| -> Vec<String> {
        request["tools"]
            .as_array()
            .map(|tools| tools.iter().map(|t| t["function"]["name"].as_str().unwrap_or("").to_string()).collect())
            .unwrap_or_default()
    };
    let tool_results = |request: &Value| -> Vec<String> {
        request["messages"]
            .as_array()
            .map(|messages| {
                messages
                    .iter()
                    .filter(|m| m["role"] == "tool")
                    .map(|m| m["content"].as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default()
    };

    // A conversation: the model looks it up, the server answers, the model puts it into words.
    env.llm.set_tool_call(Some(("mcp_fixture__lookup", json!({ "q": "кофе" }))));
    env.llm.set_reply("Нашлось про кофе.");
    let before = env.llm.requests().len();
    let (station, _) = env.connected_station("sync-mcp").await;
    station.text_input("text-mcp", "req-mcp", json!({ "type": "text_input", "text": "Что известно про кофе?" }));
    let answered = |request: &Value| tool_results(request).iter().any(|content| content.contains("Нашлось: кофе"));
    until(8000, "the server's answer at the model", || env.llm.requests()[before..].iter().any(answered)).await;
    let requests = env.llm.requests()[before..].to_vec();
    let first = requests.iter().find(|r| tools_of(r).contains(&"mcp_fixture__lookup".to_string())).unwrap();
    assert!(first["messages"][0]["content"].as_str().unwrap().contains("Подключения."));
    let offered = tools_of(first);
    assert!(offered.contains(&"mcp_fixture__lookup".into()) && offered.contains(&"mcp_fixture__add_note".into()));
    // A text from outside the house: after it the tool that acts is not offered, the one that reads still is.
    let next = requests.iter().find(|r| answered(r)).unwrap();
    assert!(tools_of(next).contains(&"mcp_fixture__lookup".into()));
    assert!(!tools_of(next).contains(&"mcp_fixture__add_note".into()));
    until(8000, "the words spoken", || env.yandex.tts().iter().any(|text| text.contains("Нашлось про кофе"))).await;

    // A command («добавь…») that the model gives to the server goes on as a conversation, with an early response.
    env.llm.set_tool_call(Some(("mcp_fixture__add_note", json!({ "text": "купить кофе" }))));
    env.llm.set_reply("Записала.");
    let command_from = env.llm.requests().len();
    station.text_input(
        "text-mcp2",
        "req-mcp2",
        json!({ "type": "text_input", "text": "Добавь в заметки купить кофе" }),
    );
    let response = station.wait_for(response_to("req-mcp2"), 8000).await;
    assert_eq!(at(&response, "event.aliceResponse.voiceResponse.outputSpeech.text").as_str().unwrap_or(""), "");
    until(8000, "the note saved", || {
        env.llm.requests()[command_from..].iter().any(|r| tool_results(r).iter().any(|content| content == "saved"))
    })
    .await;
    until(8000, "the words spoken", || env.yandex.tts().iter().any(|text| text.contains("Записала"))).await;
    let notes: Vec<String> = env
        .all_events()
        .await
        .into_iter()
        .filter(|e| e["kind"] == "mcp")
        .map(|e| e["summary"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(notes.contains(&"Fixture: mcp_fixture__lookup".into()), "{notes:?}");
    assert!(notes.contains(&"Fixture: mcp_fixture__add_note".into()), "{notes:?}");
    station.close();
    let (name, value) = bearer(&env.token);
    let removed = env.gateway.delete(&format!("/api/v1/mcp/servers/{id}")).header(name, value);
    assert_eq!(send(removed).await.0, StatusCode::NO_CONTENT);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stops_silently_when_nothing_plays_stop_heard_as_sostop_cuts_our_speech_no_model_no_word() {
    let env = env().await;
    env.asr.set_reply("Алиса, состоп.");
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-14c").await;
    let asked = env.llm.requests().len();
    env.speak(
        &station,
        "voice-14c",
        61,
        "req-14c",
        Some(json!({ "soundLevel": 5, "audioPlayer": { "playerState": 5 } })),
        None,
    );
    let response = station.wait_for(response_to("req-14c"), 8000).await;
    assert_eq!(directive_names(&response), Vec::<String>::new());
    assert_eq!(at(&response, "event.aliceResponse.voiceResponse.hasVoiceResponse"), false);
    assert_eq!(at(&response, "event.aliceResponse.voiceResponse.shouldListen"), false);
    assert_eq!(env.llm.requests().len(), asked);

    // With the player playing, «стоп» pauses it as before.
    env.asr.set_reply("Алиса, стоп.");
    env.speak(
        &station,
        "voice-14d",
        63,
        "req-14d",
        Some(json!({ "soundLevel": 5, "audioPlayer": { "playerState": 2 } })),
        None,
    );
    let playing = station.wait_for(response_to("req-14d"), 8000).await;
    let names = directive_names(&playing);
    assert!(names.contains(&"player_pause".to_string()) && names.contains(&"clear_queue".to_string()));
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stops_a_ringing_alarm_on_stop_and_vykluchi_budilnik() {
    let env = env().await;
    env.asr.set_reply("Алиса, стоп.");
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-alarm").await;
    let alarm = |ringing: bool| {
        let state = json!({ "alarms": [{ "alarmId": "alarm-1", "localTime": { "hours": 7, "minutes": 4 }, "ringing": ringing }] });
        let capability = codec().encode_message("AlarmCapability", &json!({ "state": state })).unwrap();
        json!({ "endpoint": { "capabilities": [{ "type": 21, "capability": bytes_value(&capability) }] } })
    };
    let quiet = || Some(json!({ "soundLevel": 5, "audioPlayer": { "playerState": 5 } }));
    let asked = env.llm.requests().len();
    env.speak(&station, "voice-alarm-1", 65, "req-alarm-1", quiet(), Some(alarm(true)));
    let response = station.wait_for(response_to("req-alarm-1"), 8000).await;
    let stop = at(&response, "event.aliceResponse.response.directives.0").clone();
    // As Yandex stops it: field 2 = 1 alone.
    assert_eq!(stop["name"], "alarm_stop_directive");
    let typed = duowave::gateway::protocol::proto_object::value_bytes(&stop["typedPayload"]).unwrap();
    assert_eq!(B64.encode(typed), "EAE=");

    env.asr.set_reply("Алиса, выключи будильник.");
    env.speak(&station, "voice-alarm-2", 67, "req-alarm-2", quiet(), Some(alarm(true)));
    // With the placeholder, as Yandex answered «стоп» to a ringing alarm.
    assert_eq!(
        directive_names(&station.wait_for(response_to("req-alarm-2"), 8000).await),
        vec!["alarm_stop_directive", "tts_play_placeholder"]
    );
    assert_eq!(env.llm.requests().len(), asked);

    // No words while it rings (live, 02.11: the station sent near silence under its timer): a stop too.
    env.asr.set_reply("");
    env.speak(&station, "voice-alarm-4", 71, "req-alarm-4", quiet(), Some(alarm(true)));
    assert!(directive_names(&station.wait_for(response_to("req-alarm-4"), 8000).await)
        .contains(&"alarm_stop_directive".to_string()));

    // When it does not ring, «стоп» is the silent one again, and no words are nothing.
    env.asr.set_reply("Алиса, стоп.");
    env.speak(&station, "voice-alarm-3", 69, "req-alarm-3", quiet(), Some(alarm(false)));
    assert_eq!(directive_names(&station.wait_for(response_to("req-alarm-3"), 8000).await), Vec::<String>::new());
    env.asr.set_reply("");
    env.speak(&station, "voice-alarm-5", 73, "req-alarm-5", quiet(), Some(alarm(false)));
    assert_eq!(directive_names(&station.wait_for(response_to("req-alarm-5"), 8000).await), Vec::<String>::new());
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gives_stop_and_continue_of_what_yandex_played_to_yandex_though_the_player_route_is_ours_alone() {
    let env = env().await;
    let (station, _) = env.connected_station("sync-yc").await;
    station.text_input(
        "started-yc",
        "req-started-yc",
        json!({ "type": "server_action", "name": "music_thin_client_on_started" }),
    );
    station.wait_for(response_to("req-started-yc"), 4000).await;
    // While it plays, its stop is Yandex's too: Yandex keeps the context to continue.
    env.yandex.answer(Some(Box::new(|input| {
        matches!(input.text.as_deref(), Some("Стоп") | Some("Продолжи"))
            .then(|| FakeAnswer { speech: Some(String::new()), ..FakeAnswer::default() })
    })));
    station.text_input("stop-yc", "req-stop-yc", json!({ "type": "text_input", "text": "Стоп" }));
    station.wait_for(response_to("req-stop-yc"), 4000).await;
    assert!(env.yandex.text_inputs().iter().any(|input| input.text.as_deref() == Some("Стоп")));
    station.text_input("resume-yc", "req-resume-yc", json!({ "type": "text_input", "text": "Продолжи" }));
    station.wait_for(response_to("req-resume-yc"), 4000).await;
    assert!(env.yandex.text_inputs().iter().any(|input| input.text.as_deref() == Some("Продолжи")));
    let route = env.route("Продолжи").await;
    assert_eq!((&route["capability"], &route["executor"]), (&json!("playback"), &json!("yandex")));
    // «Алиса, дальше» under the music heard as «Что дальше?»: Yandex gets the command itself.
    env.asr.set_reply("Алиса, что дальше?");
    env.yandex.answer(Some(Box::new(|input| {
        (input.text.as_deref() == Some("Дальше."))
            .then(|| FakeAnswer { speech: Some(String::new()), ..FakeAnswer::default() })
    })));
    env.speak(
        &station,
        "voice-yc",
        47,
        "req-next-yc",
        Some(json!({ "soundLevel": 5, "audioPlayer": { "playerState": 2 } })),
        None,
    );
    until(8000, "Yandex got «Дальше.»", || {
        env.yandex.text_inputs().iter().any(|input| input.text.as_deref() == Some("Дальше."))
    })
    .await;
    assert!(!env.yandex.text_inputs().iter().any(|input| input.text.as_deref() == Some("Что дальше?")));
    station.text_input(
        "stopped-yc",
        "req-stopped-yc",
        json!({ "type": "server_action", "name": "music_thin_client_on_stopped" }),
    );
    station.wait_for(response_to("req-stopped-yc"), 4000).await;
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lets_the_station_continue_its_player_after_reconnecting_only_if_it_played_before_the_break() {
    let env = env().await;
    // As the station sends it: field 1 the typed frame {53: {3: {1: 1}}}, field 2 the analytics with its purpose.
    let purpose = b"player_continue";
    let analytics = [&[0x10, 0x03, 0x1a, purpose.len() as u8][..], purpose].concat();
    let frame = [0xaa, 0x03, 0x04, 0x1a, 0x02, 0x08, 0x01];
    let payload_raw = [&[0x0a, frame.len() as u8][..], &frame, &[0x12, analytics.len() as u8], &analytics].concat();
    let callback = |station: &StationClient, id: &str, name: &str, raw: Option<&[u8]>| {
        let mut event = json!({ "type": "server_action", "name": name });
        if let Some(raw) = raw {
            event["payloadRaw"] = bytes_value(raw);
        }
        station.text_input(id, &format!("req-{id}"), event);
    };
    let reached_yandex = |upstream: &FakeConnection, id: &str| {
        upstream.frames().iter().any(|f| fakes::client_event(f).is_some_and(|event| event.header().message_id == id))
    };

    // The radio was stopped; the station reconnects and asks to continue: it stays quiet.
    let (first, _) = env.connected_station("sync-pc1").await;
    callback(&first, "stopped-1", "music_thin_client_on_stopped", None);
    first.wait_for(response_to("req-stopped-1"), 4000).await;
    first.close();
    let (second, upstream) = env.connected_station("sync-pc2").await;
    callback(&second, "continue-1", "@@mm_semantic_frame", Some(&payload_raw));
    let empty = second.wait_for(response_to("req-continue-1"), 4000).await;
    assert_eq!(directive_names(&empty), Vec::<String>::new());
    assert!(!reached_yandex(&upstream, "continue-1"));

    // The radio played when the connection broke: the request goes to Yandex.
    callback(&second, "started-1", "music_thin_client_on_started", None);
    second.wait_for(response_to("req-started-1"), 4000).await;
    second.close();
    let (third, upstream) = env.connected_station("sync-pc3").await;
    callback(&third, "continue-2", "@@mm_semantic_frame", Some(&payload_raw));
    third.wait_for(response_to("req-continue-2"), 4000).await;
    assert!(reached_yandex(&upstream, "continue-2"));
    third.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gives_a_callback_of_a_yandex_command_to_yandex_and_relays_its_answer() {
    let env = env().await;
    env.yandex.answer(Some(Box::new(|input| {
        (input.name.as_deref() == Some("@@mm_stack_engine_get_next")).then(|| FakeAnswer {
            directives: vec![client_directive(
                "audio_play",
                json!({ "stream": { "url": "https://radio.example/hls.m3u8" } }),
            )],
            ..FakeAnswer::default()
        })
    })));
    let (station, upstream) = env.connected_station("sync-15").await;
    station.text_input(
        "callback-1",
        "req-15",
        json!({ "type": "server_action", "name": "@@mm_stack_engine_get_next" }),
    );
    let response = station.wait_for(response_to("req-15"), 4000).await;
    // The station's own frame went up as it was: Yandex's answer refers to the station's message id.
    assert_eq!(at(&response, "event.header.refMessageId"), "callback-1");
    assert_eq!(directive_names(&response), vec!["audio_play"]);
    assert!(upstream
        .frames()
        .iter()
        .any(|f| fakes::client_event(f).is_some_and(|event| event.header().message_id == "callback-1")));
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answers_a_callback_nobody_takes_with_an_empty_response() {
    let env = env().await;
    let station = env.station(&[]).await;
    station.text_input("callback-2", "req-15b", json!({ "type": "server_action", "name": "vgw_unknown_callback" }));
    let response = station.wait_for(response_to("req-15b"), 4000).await;
    assert_eq!(directive_names(&response), Vec::<String>::new());
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn passes_yandex_pushes_to_the_station_and_its_sync_push_response_back_to_yandex() {
    let env = env().await;
    let (station, upstream) = env.connected_station("sync-16").await;
    // The Yandex connector reads the station's sessions: ours with Yandex is up.
    let connectors = env.get("/api/v1/connectors").await;
    let yandex = connectors["connectors"].as_array().unwrap().iter().find(|c| c["manifest"]["id"] == "yandex").unwrap();
    assert_eq!(yandex["status"], json!({ "state": "ready", "detail": "сессия открыта" }));
    upstream.send(server_frame(&json!({ "event": { "header": { "messageId": "push-1" },
        "push": bytes_value(&[0x1a, 0x08, 0x12, 0x06, 0x6e, 0x6f, 0x74, 0x69, 0x66, 0x79]) } })));
    upstream.send(server_frame(&json!({ "event": { "header": { "messageId": "sync-push-1" }, "syncPush": bytes_value(&[0x0a, 0x02, 0x69, 0x64]) } })));
    let push = station.wait_for(|m| at(m, "event.header.messageId") == "push-1", 4000).await;
    assert!(!at(&push, "event.push").is_null());
    station
        .wait_for(|m| at(m, "event.header.messageId") == "sync-push-1" && !at(m, "event.syncPush").is_null(), 4000)
        .await;

    station.send(json!({ "event": { "header": { "messageId": "spr-1", "refMessageId": "sync-push-1" }, "syncPushResponse": bytes_value(&[0x0a, 0x00]) } }));
    station.send(json!({ "event": { "header": { "messageId": "ack-1" }, "ack": { "receivedHeader": { "messageId": "push-1" } } } }));
    let decoded = || -> Vec<Value> {
        upstream
            .frames()
            .iter()
            .filter(|f| duowave::gateway::protocol::aapi_codec::AapiCodec::is_aapi_frame(f))
            .filter_map(|f| codec().decode_client(f).ok())
            .map(|d| d.decoded["event"].clone())
            .collect()
    };
    until(4000, "the sync push response and the ack at Yandex", || {
        let events = decoded();
        events.iter().any(|e| !e["syncPushResponse"].is_null())
            && events.iter().any(|e| e["ack"]["receivedHeader"]["messageId"] == "push-1")
    })
    .await;
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sends_what_we_cannot_do_to_yandex_as_our_text_and_relays_its_answer_under_our_stream_numbers() {
    let env = env().await;
    env.asr.set_reply("Алиса, включи радио.");
    env.yandex.answer(Some(Box::new(|input| {
        (input.r#type == "text_input").then(|| FakeAnswer {
            speech: Some("Включаю радио.".into()),
            directives: vec![client_directive(
                "audio_play",
                json!({ "stream": { "url": "https://radio.example/hls.m3u8" } }),
            )],
            ..FakeAnswer::default()
        })
    })));
    let (station, upstream) = env.connected_station("sync-17").await;
    env.speak(&station, "voice-17", 27, "req-17", None, None);

    let response = station.wait_for(response_to("req-17"), 8000).await;
    assert_eq!(at(&response, "event.header.refMessageId"), "voice-17");
    assert_eq!(directive_names(&response), vec!["audio_play"]);
    let speak = station.wait_for(speak_of("voice-17"), 8000).await;
    let stream_id = at(&speak, "event.header.streamId").as_u64().unwrap();
    assert_eq!(stream_id % 2, 0);
    station.wait_for(close_of(stream_id), 8000).await;
    // Yandex's speech byte for byte, on our stream number.
    let (heads, packets, _) = station.stream(stream_id as u32);
    assert_eq!((heads, packets), (1, 15));

    // Yandex got our recognised text in the station's own request, not its voice.
    let input = env.yandex.text_inputs().last().cloned().unwrap();
    assert_eq!(
        (input.r#type.as_str(), input.text.as_deref(), input.request_id.as_deref()),
        ("text_input", Some("Включи радио."), Some("req-17"))
    );
    assert!(!upstream.frames().iter().any(|f| matches!(
        fakes::client_event(f),
        Some(duowave::gateway::protocol::protocol_types::ClientEvent::VoiceInput { .. })
    )));
    let route = env.route("Включи радио.").await;
    assert_eq!((&route["capability"], &route["executor"]), (&json!("radio"), &json!("yandex")));
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waits_for_our_session_with_yandex_when_a_request_comes_before_it_is_up() {
    let env = env().await;
    env.asr.set_reply("Алиса, что сейчас играет?");
    env.yandex.answer(Some(Box::new(|input| {
        (input.r#type == "text_input" && input.text.as_deref() == Some("Что сейчас играет?"))
            .then(|| FakeAnswer { speech: Some("Минуточку.".into()), ..FakeAnswer::default() })
    })));
    env.yandex.server.open_delay_ms.store(1500, std::sync::atomic::Ordering::SeqCst);
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-race").await;
    env.speak(&station, "voice-race", 29, "req-race", None, None);
    station.wait_for(response_to("req-race"), 8000).await;
    let route = env.route("Что сейчас играет?").await;
    assert_eq!((&route["capability"], &route["executor"]), (&json!("music_recognition"), &json!("yandex")));
    let input = env.yandex.text_inputs().last().cloned().unwrap();
    assert_eq!((input.text.as_deref(), input.request_id.as_deref()), (Some("Что сейчас играет?"), Some("req-race")));
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keeps_a_yandex_dialog_going_the_follow_up_goes_to_yandex_until_it_lets_go() {
    let env = env().await;
    env.asr.set_reply("Алиса, запусти навык города.");
    env.yandex.answer(Some(Box::new(|input| match input.text.as_deref() {
        Some("Запусти навык города.") => Some(FakeAnswer {
            speech: Some("Запускаю навык.".into()),
            should_listen: Some(true),
            ..FakeAnswer::default()
        }),
        Some("Хватит.") => Some(FakeAnswer {
            speech: Some("Обращайтесь.".into()),
            directives: vec![client_directive("end_dialog_session", json!({ "dialog_id": "d-1" }))],
            ..FakeAnswer::default()
        }),
        _ => None,
    })));
    let (station, _) = env.connected_station("sync-18").await;
    env.speak(&station, "voice-18", 29, "req-18", None, None);
    let first = station.wait_for(response_to("req-18"), 8000).await;
    assert_eq!(at(&first, "event.aliceResponse.voiceResponse.shouldListen"), true);
    station.wait_for(speak_of("voice-18"), 8000).await;

    // "Хватит" is a quick rule (stop), but the skill's dialog takes it.
    env.asr.set_reply("Хватит.");
    env.speak(&station, "voice-19", 31, "req-19", None, None);
    let second = station.wait_for(response_to("req-19"), 8000).await;
    assert_eq!(directive_names(&second), vec!["end_dialog_session"]);
    let texts: Vec<Option<String>> = env.yandex.text_inputs().into_iter().map(|input| input.text).collect();
    assert!(texts.contains(&Some("Запусти навык города.".into())) && texts.contains(&Some("Хватит.".into())));

    // The dialog is over: "хватит" is ours again.
    env.asr.set_reply("Хватит.");
    env.speak(&station, "voice-20", 33, "req-20", None, None);
    let third = station.wait_for(response_to("req-20"), 8000).await;
    assert_eq!(directive_names(&third), vec!["player_pause", "clear_queue", "tts_play_placeholder"]);
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sends_a_request_to_yandex_as_the_station_voice_when_yandex_voice_asks_for_it_and_keeps_our_recognition() {
    let env = env().await;
    env.asr.set_reply("Алиса, включи утреннее шоу.");
    let (station, _) = env.connected_station("sync-22").await;
    env.speak(&station, "voice-22", 37, "req-22", None, None);
    let response = station.wait_for(response_to("req-22"), 8000).await;
    assert_eq!(at(&response, "event.header.refMessageId"), "voice-22");

    let voice_input = env.yandex.voice_inputs().last().cloned().unwrap();
    assert_eq!((voice_input.stream_id, voice_input.request_id.as_deref()), (37, Some("req-22")));
    assert!(voice_input.spotter_end && voice_input.closed);
    assert!(voice_input.audio_bytes > 1000);
    // Yandex's own recognition stays with us: the station's final result is ours.
    let finals: Vec<Value> = station.received().into_iter().filter(|m| is_final_asr("voice-22")(m)).collect();
    assert_eq!(finals.len(), 1);
    assert_eq!(normalized(&finals[0]), "Включи утреннее шоу.");
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plays_our_music_from_media_audio_play_with_our_callbacks_the_next_track_on_the_end_of_one() {
    let env = env().await;
    env.asr.set_reply("Алиса, включи музыку кино.");
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-24").await;
    env.speak(&station, "voice-24", 41, "req-24", None, None);
    let response = station.wait_for(response_to("req-24"), 8000).await;
    // With the request for the next track, as Yandex sends its tracks.
    assert_eq!(directive_names(&response), vec!["tts_play_placeholder", "audio_play", "@@mm_stack_engine_get_next"]);
    let directives = at(&response, "event.aliceResponse.response.directives").clone();
    let get_next = directives[2].clone();
    assert_eq!(get_next["type"], "server_action");
    let play = &directives[1]["payload"]["fields"];
    let url = play["stream"]["structValue"]["fields"]["url"]["stringValue"].as_str().unwrap().to_string();
    // The host the station reached us at, /media and the file.
    assert!(url.starts_with(&format!("http://127.0.0.1:{}/media/music/", env.gateway.port)), "{url}");
    let finished = &play["callbacks"]["structValue"]["fields"]["on_finished"]["structValue"]["fields"];
    assert_eq!(finished["name"]["stringValue"], "vgw_media_on_finished");

    // The station fetches the file, a part of it as a player does.
    let path = url::Url::parse(&url).unwrap().path().to_string();
    let part = env.gateway.get(&path).header("Range", "bytes=0-99").send().await.unwrap();
    assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.bytes().await.unwrap().len(), 100);

    // The station asks for the next track 30 s in: ours, not Yandex's; the next track goes to its queue.
    let to_yandex = env.yandex.text_inputs().len();
    station.text_input(
        "get-next-1",
        "req-24b",
        json!({ "type": "server_action", "name": "@@mm_stack_engine_get_next", "payload": get_next["payload"] }),
    );
    let queued = station.wait_for(response_to("req-24b"), 4000).await;
    assert_eq!(directive_names(&queued), vec!["audio_play", "@@mm_stack_engine_get_next"]);
    let queued_url = at(
        &queued,
        "event.aliceResponse.response.directives.0.payload.fields.stream.structValue.fields.url.stringValue",
    );
    assert_ne!(queued_url, url.as_str());
    assert!(!env.yandex.text_inputs()[to_yandex..]
        .iter()
        .any(|input| input.name.as_deref() == Some("@@mm_stack_engine_get_next")));

    // The track ends: the station calls back, our answer carries the next track.
    let mut payload = Map::new();
    payload.insert("play_id".into(), finished["payload"]["structValue"]["fields"]["play_id"].clone());
    station.text_input(
        "finished-1",
        "req-25",
        json!({ "type": "server_action", "name": "vgw_media_on_finished", "payload": { "fields": payload } }),
    );
    let next = station.wait_for(response_to("req-25"), 4000).await;
    assert_eq!(directive_names(&next), vec!["audio_play", "@@mm_stack_engine_get_next"]);
    let next_url =
        at(&next, "event.aliceResponse.response.directives.0.payload.fields.stream.structValue.fields.url.stringValue");
    assert_ne!(next_url, url.as_str());
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answers_for_yandex_when_it_does_not_answer_in_time() {
    let env = env().await;
    env.asr.set_reply("Алиса, найди мой телефон.");
    let (station, _) = env.connected_station("sync-21").await;
    env.speak(&station, "voice-21", 35, "req-21", None, None);
    station.wait_for(speak_of("voice-21"), 8000).await;
    assert_eq!(env.yandex.tts().last().unwrap(), "Яндекс не ответил.");
    station.close();
}

/// Our calls are a PC's part: the station's build has no /calls.
#[cfg(feature = "pc")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn announces_a_call_on_the_station_in_alice_voice_and_answers_it_by_voice() {
    use fakes::calls_client::CallsClient;
    // The station's tag unhashed: its call party is named by it.
    let env = env_with(&[("RECORDER_REDACT", "false")]).await;
    let (uni, _) = env.connected_station("sync-30").await;
    let calls = format!("ws://127.0.0.1:{}/calls/ws", env.gateway.port);
    // The TS test called from a browser; a browser joins only with the webrtc feature, so here the caller is a
    // second simulated party, and the station's party is named by its device: with two station parties the skill
    // finds the station's by its tag.
    let station = CallsClient::connect(
        &calls,
        json!({ "type": "hello", "role": "sim", "device": "TESTDEVICE01", "name": "Колонка" }),
    )
    .await;
    let browser =
        CallsClient::connect(&calls, json!({ "type": "hello", "role": "sim", "device": "mama", "name": "Мама" })).await;
    browser.send(json!({ "type": "call", "to": "station:TESTDEVICE01" }));
    station.wait_for(|event| event["type"] == "incoming", 4000).await;
    // The skills speak through the live station: our push, its callback, Alice's voice.
    until(5000, "the call announced", || {
        env.yandex.tts().iter().any(|text| text == "Вам звонит Мама. Скажите: Алиса, ответь.")
    })
    .await;

    env.asr.set_reply("Алиса, ответь.");
    env.speak(&uni, "voice-30", 51, "req-30", None, None);
    let connected = browser.wait_for(|event| event["type"] == "connected", 8000).await;
    let route = env.route("Ответь.").await;
    assert_eq!((&route["capability"], &route["executor"]), (&json!("calls"), &json!("own")));

    browser.send(json!({ "type": "hangup", "callId": connected["callId"] }));
    station.wait_for(|event| event["type"] == "ended", 4000).await;
    station.close();
    browser.close();
    uni.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acknowledges_request_statistics_so_the_station_stops_resending_them() {
    let env = env().await;
    let station = env.station(&[]).await;
    station.send(
        json!({ "event": { "header": { "messageId": "stat-1" }, "requestStat": { "requestMessageId": "voice-1" } } }),
    );
    let ack = station.wait_for(|m| !at(m, "event.requestStatAck").is_null(), 4000).await;
    assert_eq!(at(&ack, "event.header.refMessageId"), "stat-1");
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closes_the_live_connections_on_a_mode_switch_so_the_station_reconnects_into_the_new_mode() {
    let env = env().await;
    let station = env.station(&[]).await;
    until_async(2000, "the session", || async {
        env.get("/api/v1/station/sessions").await.as_array().unwrap().len() == 1
    })
    .await;
    let view = env.post("/api/v1/station/mode", json!({ "mode": "relay" }), StatusCode::CREATED).await;
    assert_eq!((&view["mode"], &view["voice"]), (&json!("relay"), &json!("yandex")));
    assert_eq!(station.closed(3000).await, 1000);
    assert_eq!(env.get("/api/v1/station/mode").await["mode"], "relay");
    let (name, value) = bearer(&env.token);
    let refused = env.gateway.post("/api/v1/station/mode").header(name, value).json(&json!({ "mode": "fly" }));
    let (status, body) = send(refused).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::BAD_REQUEST, Some("mode: relay, gateway или own")));
    let health = message(env.gateway.get("/health"), StatusCode::OK).await;
    assert_eq!((&health["mode"], &health["panelMode"]), (&json!("relay"), &json!("relay")));
}

// ---------------------------------------------------------------- whether the station is there

/// The station's device tag as the gateway has it (hashed in the journal), from its live session.
async fn device_tag(env: &Env) -> String {
    env.get("/debug/sessions").await[0]["deviceTag"].as_str().expect("a session").to_string()
}

fn presence_state(env: &Env, tag: &str) -> Option<PresenceState> {
    env.gateway.app.presence.of(tag).map(|view| view.state)
}

/// The journal's notes of a kind, by their summaries.
async fn summaries(env: &Env, kind: &str) -> Vec<String> {
    notes(env, kind).await.iter().map(|note| note["summary"].as_str().unwrap_or("").to_string()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lets_go_of_a_station_that_vanished_without_a_close_within_the_silence_and_says_why() {
    // A ping every 100 ms and a cut after 300 ms of silence; 15 s and 45 s for a real station.
    let env = env_with(&[("STATION_PING_MS", "100")]).await;
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-vanish").await;
    let tag = device_tag(&env).await;
    let health = message(env.gateway.get("/health"), StatusCode::OK).await;
    assert_eq!((&health["activeSessions"], &health["station"]["connected"]), (&json!(1), &json!(true)));
    // It rebooted or left the network (08.11.2026): its socket stays ESTABLISHED here, and nothing more comes from
    // it, not even a pong. Before, it stayed «connected» until a Yandex push happened to be written into it.
    station.vanish();
    until_async(3000, "the gateway to let the station go", || async {
        message(env.gateway.get("/health"), StatusCode::OK).await["activeSessions"] == 0
    })
    .await;
    let health = message(env.gateway.get("/health"), StatusCode::OK).await;
    assert_eq!(health["station"]["connected"], false, "the offline watch sees it gone too");
    // Not «переподключается»: a station that went silent may be gone for good.
    until(2000, "the station offline", || presence_state(&env, &tag) == Some(PresenceState::Offline)).await;
    let errors = summaries(&env, "socketError").await;
    assert!(errors.iter().any(|error| error.contains("sent nothing for 300 ms")), "{errors:?}");
    assert_eq!(summaries(&env, "disconnected").await, vec!["code=1006".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn holds_an_idle_station_that_answers_the_pings_however_long_it_says_nothing() {
    let env = env_with(&[("STATION_PING_MS", "100")]).await;
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-idle").await;
    let tag = device_tag(&env).await;
    // Five times the silence that cuts a vanished one, without a frame of its own: only its socket's pongs.
    sleep(1500).await;
    let health = message(env.gateway.get("/health"), StatusCode::OK).await;
    assert_eq!((&health["activeSessions"], &health["station"]["connected"]), (&json!(1), &json!(true)));
    assert_eq!(presence_state(&env, &tag), Some(PresenceState::Voice));
    let pings = station.pings.load(std::sync::atomic::Ordering::SeqCst);
    assert!(pings >= 5, "pinged {pings} times in 1.5 s");
    assert!(summaries(&env, "socketError").await.is_empty());
    station.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn takes_a_station_whose_socket_a_mode_switch_closed_for_one_on_its_way_back_not_for_gone() {
    let env = env().await;
    let station = env.station(STATION_HEADERS).await;
    station.synchronize("sync-switch").await;
    let tag = device_tag(&env).await;
    env.post("/api/v1/station/mode", json!({ "mode": "relay" }), StatusCode::CREATED).await;
    assert_eq!(station.closed(3000).await, 1000);
    // The station opens its next socket by itself, in up to a few minutes, or at once with «Алиса»: meanwhile the
    // panel says «переподключается…», not «не в сети».
    until(2000, "the station reconnecting", || presence_state(&env, &tag) == Some(PresenceState::Reconnecting)).await;
    assert_eq!(summaries(&env, "disconnected").await, vec!["code=1000 reason=mode switch".to_string()]);
}
