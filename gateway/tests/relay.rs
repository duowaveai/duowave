//! Relay mode end to end (test/relay.e2e-spec.ts): a station on our uniproxy, a fake Yandex behind it; every frame
//! passes both ways unchanged and is journaled, with the audio as files and no token anywhere.
//!
//! test/relay.e2e-spec.ts, the Node original, is removed from the tree: git tag `node-gateway-last`.

mod fakes;
mod support;

use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use bytes::Bytes;
use duowave::gateway::protocol::protocol_types::{
    AliceResponseModel, ClientEvent, CloseReason, Directive, ServerMessage,
};
use duowave::speech::test_audio::{ogg_opus_chain, voice};
use fakes::{audio_frame, codec, field, lock, sleep, until, StationClient, WsServer};
use reqwest::StatusCode;
use serde_json::{json, Value};
use support::{bearer, message, send, Gateway};

const SECRET: &str = "y0_relay-secret-token-0123456789";

/// 200 bytes of an animation, longer than the 64 bytes the raw view keeps.
fn animation() -> Vec<u8> {
    (0..200).map(|i| (i % 251) as u8).collect()
}

/// A server event unknown to our schema: field 77 with a nested message, as a new Yandex feature would look.
fn unknown_event() -> Bytes {
    let feature = [field(1, b"is_whisper"), vec![0x10, 0x01]].concat();
    let event = [field(1, &field(1, b"m-77")), field(77, &feature)].concat();
    Bytes::from([b"AAPI".to_vec(), field(1, &event)].concat())
}

/// Yandex as the relay sees it: SynchronizeStateResponse, then for a VoiceInput the recognition, the answer and its
/// speech. What it sent is kept, to compare with what reached the station.
async fn fake_yandex() -> (WsServer, Arc<Mutex<Vec<Bytes>>>) {
    let sent: Arc<Mutex<Vec<Bytes>>> = Arc::default();
    let kept = sent.clone();
    let server = WsServer::start(Arc::new(move |connection, frame| {
        let send = |frame: Bytes| {
            lock(&kept).push(frame.clone());
            connection.send(frame);
        };
        let Some(event) = fakes::client_event(frame) else { return };
        let codec = codec();
        if let ClientEvent::SynchronizeState { header, .. } = &event {
            send(fakes::encode_server(&ServerMessage::SynchronizeStateResponse {
                r#ref: header.message_id.clone(),
                session_id: "yandex-1".into(),
            }));
        }
        let ClientEvent::VoiceInput { header, .. } = event else { return };
        let r#ref = header.message_id;
        send(fakes::encode_server(&ServerMessage::AsrResult {
            r#ref: r#ref.clone(),
            text: "который час".into(),
            end_of_utt: true,
        }));
        let mut directive = Directive::client("draw_animation_directive", Default::default());
        directive.payload = None;
        // A typed command longer than the raw view keeps: its field 15 must reach the journal whole.
        directive.typed_payload = Some(Bytes::from(animation()));
        let response = AliceResponseModel {
            request_id: "req-1".into(),
            response_id: "resp-1".into(),
            speech: Some("Сейчас десять часов.".into()),
            text: Some("Сейчас десять часов.".into()),
            directives: vec![directive],
            ..Default::default()
        };
        send(codec.encode_server(&ServerMessage::AliceResponse { r#ref: r#ref.clone(), response }).unwrap().frame);
        send(unknown_event());
        send(fakes::encode_server(&ServerMessage::TtsSpeak { r#ref, stream_id: 2, format: "audio/opus".into() }));
        send(audio_frame(2, &ogg_opus_chain(&voice(200.0, -20.0), 0x42)));
        send(fakes::encode_server(&ServerMessage::StreamClose { stream_id: 2, reason: CloseReason::Ok }));
    }))
    .await;
    (server, sent)
}

async fn start(yandex: &WsServer, journal: &std::path::Path) -> (Gateway, String) {
    start_with(yandex, journal, &[]).await
}

async fn start_with(yandex: &WsServer, journal: &std::path::Path, extra: &[(&str, &str)]) -> (Gateway, String) {
    let journal = journal.to_string_lossy().into_owned();
    let mut vars: Vec<(&str, &str)> = vec![
        ("RECORDER_ENABLED", "true"),
        ("RECORDER_DIR", &journal),
        ("UNIPROXY_MODE", "relay"),
        // The defaults are for a new household (no audio, no debug API); this test reads both.
        ("RECORDER_AUDIO", "true"),
        ("DEBUG_API_ENABLED", "true"),
        ("YANDEX_UNIPROXY_URL", &yandex.url),
        ("RELAY_PING_MS", "100"),
        ("STATION_PAIRING", "open"),
    ];
    vars.extend_from_slice(extra);
    let gateway = Gateway::start(&vars).await;
    let setup = message(
        gateway.post("/api/v1/auth/setup").json(&json!({"password": "e2e-admin-password"})),
        StatusCode::CREATED,
    );
    let token = setup.await["token"].as_str().unwrap().to_string();
    (gateway, token)
}

async fn all_events(gateway: &Gateway, token: &str) -> Vec<Value> {
    let (name, value) = bearer(token);
    let sessions = message(gateway.get("/debug/sessions").header(name.clone(), value.clone()), StatusCode::OK).await;
    let mut events = Vec::new();
    for session in sessions.as_array().unwrap() {
        let path = format!("/debug/sessions/{}/events?limit=500", session["id"].as_str().unwrap());
        let found = message(gateway.get(&path).header(name.clone(), value.clone()), StatusCode::OK).await;
        events.extend(found.as_array().unwrap().iter().cloned());
    }
    events
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn passes_every_frame_both_ways_unchanged_and_journals_them_with_audio_files_and_no_token() {
    let (yandex, sent) = fake_yandex().await;
    let journal = tempfile::tempdir().unwrap();
    let (gateway, token) = start(&yandex, journal.path()).await;
    let url = format!("ws://127.0.0.1:{}/uni.ws", gateway.port);
    let station = StationClient::connect_with(
        &url,
        &[("x-uprx-device-id", "RELAYDEVICE"), ("x-uprx-oauth-token", SECRET), ("user-agent", "WebSocket++/0.8.2")],
    )
    .await
    .unwrap();

    // Sent at once, before the relay has reached Yandex: they must arrive in order all the same.
    let codec = codec();
    let command = ogg_opus_chain(&voice(300.0, -40.0), 0x77);
    let voice_input = codec
        .encode_client_object(&json!({ "event": {
            "header": { "messageId": "voice-1", "streamId": 1 },
            "voiceInput": { "header": { "requestId": "req-1" }, "request": { "event": { "type": "voice_input" } } },
        } }))
        .unwrap();
    let frames: Vec<Bytes> = vec![
        codec
            .encode_client_object(&json!({ "event": { "header": { "messageId": "sync-1" },
                "synchronizeStateRequest": { "application": { "deviceModel": "plum" } } } }))
            .unwrap(),
        // A field the schema does not know, carrying the token: the raw view must not leak it.
        Bytes::from([voice_input.to_vec(), field(99, format!("token {SECRET}").as_bytes())].concat()),
        audio_frame(1, &command),
        codec
            .encode_client_object(&json!({ "streamControl": { "streamId": 1, "messageId": "sc-1", "close": {} } }))
            .unwrap(),
    ];
    for frame in &frames {
        station.send_frame(frame.clone());
    }

    until(2000, "Yandex's frames at the station", || lock(&station.frames).len() >= 7).await;
    let connection = yandex.last();
    assert_eq!(connection.frames(), frames);
    assert_eq!(*lock(&station.frames), *lock(&sent));
    assert_eq!(connection.headers["x-uprx-oauth-token"], SECRET);
    assert_eq!(connection.headers["user-agent"], "WebSocket++/0.8.2");
    assert!(connection.headers.get("sec-websocket-extensions").is_none());

    sleep(50).await;
    let events = all_events(&gateway, &token).await;
    let find = |dir: &str, kind: &str| {
        events
            .iter()
            .find(|e| e["dir"] == dir && e["kind"] == kind)
            .cloned()
            .unwrap_or_else(|| panic!("no {dir} {kind}"))
    };
    let sync = find("in", "synchronizeState");
    assert!(sync["data"].get("raw").is_none(), "the sync's raw view would carry the tokens");
    let voice_event = find("in", "voiceInput");
    assert!(voice_event["data"]["raw"].to_string().contains("\"field\":99"));
    let response = find("out", "aliceResponse");
    assert!(response["summary"].as_str().unwrap().contains("speech=\"Сейчас десять часов.\""));
    assert_eq!(
        response["data"]["typedPayloads"],
        json!([{ "name": "draw_animation_directive", "analyticsType": "draw_animation_directive", "base64": B64.encode(animation()) }])
    );
    let unknown = find("out", "unknown");
    assert!(unknown["data"]["raw"].to_string().contains("is_whisper"));

    let audio: Vec<&Value> = events.iter().filter(|e| e["kind"] == "audio").collect();
    let mut summaries: Vec<String> = audio.iter().map(|e| e["summary"].as_str().unwrap().to_string()).collect();
    summaries.sort();
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0], format!("station stream 1: {} bytes", command.len()));
    assert!(summaries[1].starts_with("yandex stream 2: ") && summaries[1].ends_with(" bytes"), "{}", summaries[1]);
    for note in &audio {
        let file = note["data"]["file"].as_str().unwrap();
        assert_eq!(std::fs::metadata(file).unwrap().len(), note["data"]["bytes"].as_u64().unwrap());
    }
    let file_of = |prefix: &str| {
        audio.iter().find(|e| e["summary"].as_str().unwrap().starts_with(prefix)).unwrap()["data"]["file"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert!(file_of("yandex").ends_with("_tts_2.ogg"), "{}", file_of("yandex"));
    assert!(file_of("station").ends_with("_voice_1.ogg"), "{}", file_of("station"));

    // The token is in neither the journal nor the debug API, raw views included.
    assert!(!Value::Array(events.clone()).to_string().contains(SECRET));
    let folder = std::path::Path::new(&file_of("station")).parent().unwrap().parent().unwrap().to_path_buf();
    let journal_text = std::fs::read_to_string(folder.join("events.jsonl")).unwrap();
    assert!(!journal_text.contains(SECRET));
    assert!(journal_text.contains("is_whisper"));

    let health = message(gateway.get("/health"), StatusCode::OK).await;
    assert_eq!(health["mode"], "relay");
    let (name, value) = bearer(&token);
    let sessions = message(gateway.get("/debug/sessions").header(name.clone(), value.clone()), StatusCode::OK).await;
    let session = &sessions[0];
    assert_eq!(session["mode"], "relay");
    let say = gateway
        .post(&format!("/debug/sessions/{}/say", session["id"].as_str().unwrap()))
        .header(name.clone(), value.clone())
        .json(&json!({ "text": "привет" }));
    assert_eq!(send(say).await.0, StatusCode::BAD_REQUEST);

    // Alice's trip read off the frames as they passed — her ear, her brain, her voice — and journaled like ours.
    let journal = message(gateway.get("/api/v1/requests").header(name, value), StatusCode::OK).await;
    let trip = &journal["requests"][0];
    assert_eq!(
        (&trip["kind"], &trip["mode"], &trip["status"], &trip["text"], &trip["answer"], &trip["executor"]),
        (
            &json!("voice"),
            &json!("relay"),
            &json!("answered"),
            &json!("который час"),
            &json!("Сейчас десять часов."),
            &json!("yandex")
        )
    );
    let steps: Vec<String> = trip["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| format!("{} {}", step["phase"].as_str().unwrap(), step["by"].as_str().unwrap()))
        .collect();
    assert_eq!(steps, vec!["listen yandex", "think yandex", "speak yandex"]);
    station.close();
    gateway.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closes_the_station_connection_when_yandex_closes_the_session() {
    let (yandex, _) = fake_yandex().await;
    let journal = tempfile::tempdir().unwrap();
    let (gateway, _) = start(&yandex, journal.path()).await;
    let url = format!("ws://127.0.0.1:{}/uni.ws", gateway.port);
    let station = StationClient::connect_with(&url, &[("x-uprx-device-id", "RELAYDEVICE")]).await.unwrap();
    until(1000, "the relay's connection to Yandex", || yandex.count() == 1).await;
    yandex.last().close(1000);
    assert_eq!(station.closed(3000).await, 1000);
    gateway.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cuts_a_yandex_connection_that_stopped_answering_so_the_station_reconnects() {
    let (yandex, _) = fake_yandex().await;
    let journal = tempfile::tempdir().unwrap();
    let (gateway, _) = start(&yandex, journal.path()).await;
    let url = format!("ws://127.0.0.1:{}/uni.ws", gateway.port);
    let station = StationClient::connect_with(&url, &[("x-uprx-device-id", "RELAYDEVICE")]).await.unwrap();
    until(1000, "the relay's connection to Yandex", || yandex.count() == 1).await;
    // A connection that died without a close frame: Yandex reads nothing more, so our pings go unanswered.
    yandex.last().pause();
    assert_eq!(station.closed(3000).await, 1011);
    gateway.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lets_go_of_a_station_that_vanished_and_of_its_yandex_connection_with_it() {
    let (yandex, _) = fake_yandex().await;
    let journal = tempfile::tempdir().unwrap();
    let (gateway, _) = start_with(&yandex, journal.path(), &[("STATION_PING_MS", "100")]).await;
    let url = format!("ws://127.0.0.1:{}/uni.ws", gateway.port);
    let station = StationClient::connect_with(&url, &[("x-uprx-device-id", "RELAYDEVICE")]).await.unwrap();
    until(1000, "the relay's connection to Yandex", || yandex.count() == 1).await;
    // Yandex goes on answering our pings to it: only the station's own socket can tell the station is gone.
    station.vanish();
    until(3000, "the relay to close its Yandex connection", || !yandex.last().open()).await;
    let health = message(gateway.get("/health"), StatusCode::OK).await;
    assert_eq!((&health["activeSessions"], &health["station"]["connected"]), (&json!(0), &json!(false)));
    gateway.stop().await;
}
