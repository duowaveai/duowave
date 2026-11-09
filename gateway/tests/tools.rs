//! `duowave call-sim` end to end: the simulator against a gateway built in this process on 127.0.0.1, another party
//! on the same calls socket. The station's build has no calls, nor this test.

#![cfg(feature = "pc")]

mod support;

use std::time::Duration;

use duowave::speech::audio::pcm::{decode_wav, encode_wav};
use duowave::speech::test_audio::voice;
use duowave::tools::call_sim::{self, Options};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use support::Gateway;
use tokio_tungstenite::tungstenite::Message;

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Another party on the calls socket, introduced.
async fn party(url: &str, hello: Value) -> Socket {
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.expect("the calls socket opens");
    socket.send(Message::Text(hello.to_string().into())).await.unwrap();
    event(&mut socket, "welcome").await;
    socket
}

/// The next text event of a type, past the others (and past audio).
async fn event(socket: &mut Socket, kind: &str) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap();
        if let Message::Text(text) = message {
            let event: Value = serde_json::from_str(text.as_str()).unwrap();
            if event["type"] == kind {
                return event;
            }
        }
    }
}

/// The next `count` samples of audio that come.
async fn audio(socket: &mut Socket, count: usize) -> Vec<i16> {
    let mut samples = Vec::new();
    while samples.len() < count {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap();
        if let Message::Binary(bytes) = message {
            samples.extend(bytes.as_chunks::<2>().0.iter().map(|pair| i16::from_le_bytes(*pair)));
        }
    }
    samples.truncate(count);
    samples
}

async fn send(socket: &mut Socket, message: Value) {
    socket.send(Message::Text(message.to_string().into())).await.unwrap();
}

/// 100 ms of our own voice to the simulator: five frames of a counting pattern.
fn ours() -> Vec<i16> {
    (0..1600).map(|i| (i % 300) as i16 - 150).collect()
}

async fn talk(socket: &mut Socket, samples: &[i16]) {
    for frame in samples.chunks(320) {
        let bytes: Vec<u8> = frame.iter().flat_map(|sample| sample.to_le_bytes()).collect();
        socket.send(Message::Binary(bytes.into())).await.unwrap();
    }
}

fn options(gateway: &Gateway, dir: &std::path::Path, play: Option<std::path::PathBuf>, call: Option<&str>) -> Options {
    Options {
        url: format!("ws://127.0.0.1:{}/calls/ws", gateway.port),
        device: "default".into(),
        play,
        record: dir.join("heard.wav"),
        answer_after: Duration::from_millis(50),
        call: call.map(str::to_string),
    }
}

#[tokio::test]
async fn the_simulator_answers_a_call_plays_its_recording_and_keeps_what_it_heard() {
    let gateway = Gateway::start(&[]).await;
    let dir = tempfile::tempdir().unwrap();
    let play = dir.path().join("voice.wav");
    std::fs::write(&play, encode_wav(&voice(1000.0, -20.0), 16000)).unwrap();
    let options = options(&gateway, dir.path(), Some(play), None);
    let microphone = call_sim::load(options.play.as_deref()).unwrap();
    let mut lines: Vec<String> = Vec::new();

    let mut say = |line| lines.push(line);
    let simulator = call_sim::run(&options, &microphone, &mut say);
    let caller = async {
        let joined =
            support::until(4000, || gateway.app.calls.hub.list().iter().any(|party| party.id == "station:default"))
                .await;
        assert!(joined, "the simulator in the hub");
        let mut kitchen =
            party(&options.url, json!({ "type": "hello", "role": "sim", "device": "kitchen", "name": "Кухня" })).await;
        send(&mut kitchen, json!({ "type": "call", "to": "station:default" })).await;
        let connected = event(&mut kitchen, "connected").await;
        // The recording, 20 ms at a time from its start, as the station's microphone.
        assert_eq!(audio(&mut kitchen, 3200).await, microphone[..3200]);
        talk(&mut kitchen, &ours()).await;
        send(&mut kitchen, json!({ "type": "hangup", "callId": connected["callId"] })).await;
        event(&mut kitchen, "ended").await;
    };
    let (heard, ()) = tokio::time::timeout(Duration::from_secs(20), async { tokio::join!(simulator, caller) })
        .await
        .expect("the call ends");

    assert_eq!(heard.unwrap(), Some(1600));
    let record = decode_wav(&std::fs::read(&options.record).unwrap()).unwrap();
    assert_eq!((record.sample_rate, record.pcm), (16000, ours()));
    let kinds: Vec<String> = lines
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .map(|event| event["type"].as_str().unwrap_or_default().to_string())
        .collect();
    for kind in ["welcome", "incoming", "connected", "ended"] {
        assert!(kinds.iter().any(|seen| seen == kind), "{kind} in {lines:?}");
    }
    let welcome = lines
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| event["type"] == "welcome")
        .unwrap();
    assert_eq!((welcome["id"].as_str(), welcome["name"].as_str()), (Some("station:default"), Some(call_sim::NAME)));
    assert_eq!(lines.last().unwrap(), &format!("heard 0.1 s → {}", options.record.display()));
    gateway.stop().await;
}

#[tokio::test]
async fn the_simulator_calls_a_party_by_name_and_plays_a_tone_without_a_recording() {
    let gateway = Gateway::start(&[]).await;
    let dir = tempfile::tempdir().unwrap();
    let options = options(&gateway, dir.path(), None, Some("кухн"));
    let mut kitchen =
        party(&options.url, json!({ "type": "hello", "role": "sim", "device": "kitchen", "name": "Кухня" })).await;
    let tone = call_sim::load(None).unwrap();
    let mut lines: Vec<String> = Vec::new();

    let mut say = |line| lines.push(line);
    let simulator = call_sim::run(&options, &tone, &mut say);
    let callee = async {
        let incoming = event(&mut kitchen, "incoming").await;
        assert_eq!(incoming["peer"]["id"], "station:default");
        send(&mut kitchen, json!({ "type": "accept", "callId": incoming["callId"] })).await;
        event(&mut kitchen, "connected").await;
        assert_eq!(audio(&mut kitchen, 640).await, tone[..640]);
        send(&mut kitchen, json!({ "type": "hangup", "callId": incoming["callId"] })).await;
        event(&mut kitchen, "ended").await;
    };
    let (heard, ()) = tokio::time::timeout(Duration::from_secs(20), async { tokio::join!(simulator, callee) })
        .await
        .expect("the call ends");

    assert_eq!(heard.unwrap(), Some(0), "the kitchen said nothing");
    assert!(lines.iter().any(|line| line.contains(r#""type":"ringing""#)), "{lines:?}");
    assert_eq!(decode_wav(&std::fs::read(&options.record).unwrap()).unwrap().pcm, Vec::<i16>::new());
    gateway.stop().await;
}
