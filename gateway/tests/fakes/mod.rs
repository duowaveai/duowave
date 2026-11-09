//! What the station-link tests talk to: a station built on the codec (its frames as the real one sends them), and
//! local fakes of Yandex's uniproxy, the recognition server, the model and the weather. All on 127.0.0.1; nothing
//! leaves the computer.

#![allow(dead_code)]

pub mod calls_client;
pub mod gateway_yandex;
pub mod mcp_server;

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::body::Bytes as AxumBytes;
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, Method};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::Router;
use bytes::Bytes;
use duowave::gateway::protocol::aapi_codec::AapiCodec;
use duowave::gateway::protocol::proto_edit::{edit_message, set_bytes};
use duowave::gateway::protocol::proto_object::{bytes_value, value_bytes};
use duowave::gateway::protocol::protocol_types::{ClientEvent, ClientMessage, ServerMessage, StreamAction};
use duowave::speech::audio::ogg_opus_demuxer::{OggOpusDemuxer, OggOpusEvent};
use duowave::speech::test_audio::{ogg_opus_chain, ogg_pages, voice};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::server::{Request as WsRequest, Response as WsResponse};
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub async fn sleep(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// Waits until `check` holds, polling; panics after `ms` with `what`.
pub async fn until(ms: u64, what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    while !check() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        sleep(20).await;
    }
}

/// Waits for an async check.
pub async fn until_async<F, Fut>(ms: u64, what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    while !check().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        sleep(25).await;
    }
}

pub fn codec() -> AapiCodec {
    AapiCodec::new()
}

/// `[u32 BE stream id][payload]`.
pub fn audio_frame(stream_id: u32, payload: &[u8]) -> Bytes {
    let mut frame = stream_id.to_be_bytes().to_vec();
    frame.extend_from_slice(payload);
    Bytes::from(frame)
}

pub fn varint(mut value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    while value > 0x7f {
        bytes.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    bytes.push(value as u8);
    bytes
}

/// A length-delimited protobuf field, to build messages our schema does not describe.
pub fn field(number: u64, value: &[u8]) -> Vec<u8> {
    [varint((number << 3) | 2), varint(value.len() as u64), value.to_vec()].concat()
}

pub fn encode_server(message: &ServerMessage) -> Bytes {
    codec().encode_server(message).expect("an encodable message").frame
}

/// Yandex's speech for a phrase: 300 ms of Ogg/Opus as frames of five pages on a stream.
pub fn speech_frames(stream_id: u32) -> Vec<Bytes> {
    let chain = ogg_opus_chain(&voice(300.0, -20.0), 0x9999);
    ogg_pages(&chain).chunks(5).map(|pages| audio_frame(stream_id, &pages.concat())).collect()
}

/// A get at a JSON value's path of keys and indexes (`"event.asrResult.endOfUtt"`, `recognition.0`).
pub fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    let mut current = value;
    for key in path.split('.') {
        current = match key.parse::<usize>() {
            Ok(index) if current.is_array() => current.get(index).unwrap_or(&Value::Null),
            _ => current.get(key).unwrap_or(&Value::Null),
        };
    }
    current
}

// ---------------------------------------------------------------- a WebSocket server for the fake Yandex

/// One connection to a fake Yandex: what the gateway sent it, and a way to answer.
pub struct FakeConnection {
    pub headers: HeaderMap,
    pub frames: Mutex<Vec<Bytes>>,
    out: mpsc::UnboundedSender<Message>,
    paused: AtomicBool,
    pause: Notify,
    pub closed: AtomicBool,
}

impl FakeConnection {
    pub fn send(&self, frame: Bytes) {
        let _ = self.out.send(Message::Binary(frame));
    }

    pub fn open(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
    }

    /// Closes the connection with a close frame.
    pub fn close(&self, code: u16) {
        let _ = self.out.send(Message::Close(Some(CloseFrame { code: CloseCode::from(code), reason: "".into() })));
    }

    /// Reads nothing more, as a connection that died without a close frame: pings go unanswered.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        self.pause.notify_waiters();
    }

    pub fn frames(&self) -> Vec<Bytes> {
        lock(&self.frames).clone()
    }
}

/// What a fake does with each frame it gets: answers synchronously through the connection, or starts a task.
pub type OnFrame = Arc<dyn Fn(&Arc<FakeConnection>, &Bytes) + Send + Sync>;

pub struct WsServer {
    pub url: String,
    pub connections: Arc<Mutex<Vec<Arc<FakeConnection>>>>,
    /// Waits this long before answering the upgrade: a request can come before our session is up.
    pub open_delay_ms: Arc<AtomicU64>,
}

impl WsServer {
    pub async fn start(on_frame: OnFrame) -> WsServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let url = format!("ws://{}/uni.ws", listener.local_addr().unwrap());
        let connections: Arc<Mutex<Vec<Arc<FakeConnection>>>> = Arc::default();
        let open_delay_ms = Arc::new(AtomicU64::new(0));
        let (list, delay) = (connections.clone(), open_delay_ms.clone());
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (list, on_frame, delay) = (list.clone(), on_frame.clone(), delay.clone());
                tokio::spawn(async move {
                    sleep(delay.load(Ordering::SeqCst)).await;
                    let captured: Arc<Mutex<HeaderMap>> = Arc::default();
                    let into = captured.clone();
                    // tungstenite's handshake callback returns its own ErrorResponse: the size is not ours to choose.
                    #[allow(clippy::result_large_err)]
                    let callback = move |request: &WsRequest, response: WsResponse| {
                        *lock(&into) = request.headers().clone();
                        Ok(response)
                    };
                    let Ok(socket) = tokio_tungstenite::accept_hdr_async(stream, callback).await else { return };
                    let (mut sink, mut incoming) = socket.split();
                    let (out, mut outgoing) = mpsc::unbounded_channel::<Message>();
                    let connection = Arc::new(FakeConnection {
                        headers: lock(&captured).clone(),
                        frames: Mutex::new(Vec::new()),
                        out,
                        paused: AtomicBool::new(false),
                        pause: Notify::new(),
                        closed: AtomicBool::new(false),
                    });
                    lock(&list).push(connection.clone());
                    tokio::spawn(async move {
                        while let Some(message) = outgoing.recv().await {
                            let closing = matches!(message, Message::Close(_));
                            if sink.send(message).await.is_err() || closing {
                                break;
                            }
                        }
                    });
                    loop {
                        if connection.paused.load(Ordering::SeqCst) {
                            // Parked until the test ends: nothing is read, nothing is answered.
                            futures::future::pending::<()>().await;
                        }
                        let next = tokio::select! {
                            _ = connection.pause.notified() => continue,
                            next = incoming.next() => next,
                        };
                        match next {
                            Some(Ok(Message::Binary(frame))) => {
                                lock(&connection.frames).push(frame.clone());
                                on_frame(&connection, &frame);
                            }
                            Some(Ok(Message::Text(text))) => {
                                lock(&connection.frames).push(Bytes::copy_from_slice(text.as_bytes()));
                            }
                            Some(Ok(_)) => {}
                            Some(Err(_)) | None => break,
                        }
                    }
                    connection.closed.store(true, Ordering::SeqCst);
                });
            }
        });
        WsServer { url, connections, open_delay_ms }
    }

    pub fn connections(&self) -> Vec<Arc<FakeConnection>> {
        lock(&self.connections).clone()
    }

    pub fn count(&self) -> usize {
        lock(&self.connections).len()
    }

    pub fn last(&self) -> Arc<FakeConnection> {
        self.connections().last().cloned().expect("a connection")
    }
}

// ---------------------------------------------------------------- the station

/// A station on our uniproxy: collects decoded server frames and audio frames, answers our pushes as the real one.
pub struct StationClient {
    /// Decoded server frames, and `{audio: {streamId, bytes}}` per audio frame, in order.
    pub received: Arc<Mutex<Vec<Value>>>,
    /// Audio the gateway streamed, by stream id.
    pub audio: Arc<Mutex<HashMap<u32, Vec<Vec<u8>>>>>,
    /// Our server_action pushes, decoded.
    pub pushes: Arc<Mutex<Vec<Value>>>,
    /// The station calls back with each push, as the real one does, unless told not to.
    pub answers_pushes: Arc<AtomicBool>,
    /// The socket's close code, once closed.
    pub close_code: Arc<Mutex<Option<u16>>>,
    /// Every binary frame as it came.
    pub frames: Arc<Mutex<Vec<Bytes>>>,
    /// The gateway's pings it got; its socket answers each (RFC 6455), as WebSocket++ on the real one should.
    pub pings: Arc<AtomicU64>,
    /// Gone without a word (`vanish`): nothing more is read or answered.
    vanished: Arc<AtomicBool>,
    vanish: Arc<Notify>,
    out: mpsc::UnboundedSender<Message>,
}

impl StationClient {
    /// Connects with the station's device id and the given headers; Err(status) when the upgrade is refused.
    pub async fn connect(url: &str, headers: &[(&str, &str)]) -> Result<StationClient, u16> {
        let mut all: Vec<(&str, &str)> = vec![("x-uprx-device-id", "TESTDEVICE01")];
        all.extend_from_slice(headers);
        StationClient::connect_with(url, &all).await
    }

    /// Connects with exactly these headers.
    pub async fn connect_with(url: &str, headers: &[(&str, &str)]) -> Result<StationClient, u16> {
        let mut request = url.into_client_request().expect("a WebSocket URL");
        for (name, value) in headers {
            request
                .headers_mut()
                .insert(HeaderName::from_bytes(name.as_bytes()).unwrap(), HeaderValue::from_str(value).unwrap());
        }
        let socket = match tokio_tungstenite::connect_async(request).await {
            Ok((socket, _)) => socket,
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => return Err(response.status().as_u16()),
            Err(error) => panic!("the station cannot connect: {error}"),
        };
        let (mut sink, mut incoming) = socket.split();
        let (out, mut outgoing) = mpsc::unbounded_channel::<Message>();
        let client = StationClient {
            received: Arc::default(),
            audio: Arc::default(),
            pushes: Arc::default(),
            answers_pushes: Arc::new(AtomicBool::new(true)),
            close_code: Arc::default(),
            frames: Arc::default(),
            pings: Arc::default(),
            vanished: Arc::default(),
            vanish: Arc::default(),
            out: out.clone(),
        };
        tokio::spawn(async move {
            while let Some(message) = outgoing.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        });
        let (received, audio, pushes, answers, close_code, frames) = (
            client.received.clone(),
            client.audio.clone(),
            client.pushes.clone(),
            client.answers_pushes.clone(),
            client.close_code.clone(),
            client.frames.clone(),
        );
        let (pings, vanished, vanish) = (client.pings.clone(), client.vanished.clone(), client.vanish.clone());
        tokio::spawn(async move {
            let codec = codec();
            loop {
                let next = tokio::select! {
                    _ = vanish.notified() => None,
                    next = incoming.next() => next,
                };
                if vanished.load(Ordering::SeqCst) {
                    // Parked until the test ends, the socket left open: nothing read, no ping answered.
                    futures::future::pending::<()>().await;
                }
                let Some(next) = next else { break };
                if let Ok(Message::Ping(_)) = &next {
                    pings.fetch_add(1, Ordering::SeqCst);
                }
                if let Ok(Message::Binary(data)) = &next {
                    lock(&frames).push(data.clone());
                }
                match next {
                    Ok(Message::Binary(data)) if AapiCodec::is_aapi_frame(&data) => {
                        let Ok(message) = codec.decode_server_frame(&data) else { continue };
                        let push = at(&message, "event.push").clone();
                        lock(&received).push(message);
                        if let Some(bytes) = value_bytes(&push) {
                            on_push(&codec, &bytes, &pushes, &answers, &out);
                        }
                    }
                    Ok(Message::Binary(data)) if data.len() >= 4 => {
                        let stream_id = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                        lock(&audio).entry(stream_id).or_default().push(data[4..].to_vec());
                        lock(&received).push(json!({ "audio": { "streamId": stream_id, "bytes": data.len() - 4 } }));
                    }
                    Ok(Message::Close(frame)) => {
                        *lock(&close_code) = Some(frame.map_or(1005, |frame| u16::from(frame.code)));
                    }
                    Ok(_) => {}
                    Err(_) => {
                        lock(&close_code).get_or_insert(1006);
                        break;
                    }
                }
            }
            lock(&close_code).get_or_insert(1006);
        });
        Ok(client)
    }

    pub fn send_frame(&self, frame: Bytes) {
        let _ = self.out.send(Message::Binary(frame));
    }

    pub fn send(&self, object: Value) {
        self.send_frame(codec().encode_client_object(&object).expect("an encodable station message"));
    }

    pub fn close(&self) {
        let _ = self.out.send(Message::Close(None));
    }

    /// Gone without a close: a station that rebooted or lost the network. Its socket stays open here, and it reads,
    /// answers and sends nothing more, so the gateway's pings go unanswered.
    pub fn vanish(&self) {
        self.vanished.store(true, Ordering::SeqCst);
        self.vanish.notify_waiters();
    }

    pub fn received(&self) -> Vec<Value> {
        lock(&self.received).clone()
    }

    pub fn has(&self, predicate: impl Fn(&Value) -> bool) -> bool {
        lock(&self.received).iter().any(predicate)
    }

    pub fn index(&self, predicate: impl Fn(&Value) -> bool) -> Option<usize> {
        lock(&self.received).iter().position(predicate)
    }

    pub async fn wait_for(&self, predicate: impl Fn(&Value) -> bool, timeout_ms: u64) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            if let Some(found) = lock(&self.received).iter().find(|message| predicate(message)).cloned() {
                return found;
            }
            if tokio::time::Instant::now() > deadline {
                panic!("timed out; received: {}", Value::Array(self.received()));
            }
            sleep(10).await;
        }
    }

    pub async fn closed(&self, timeout_ms: u64) -> u16 {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            if let Some(code) = *lock(&self.close_code) {
                return code;
            }
            assert!(tokio::time::Instant::now() < deadline, "the station's socket stayed open");
            sleep(10).await;
        }
    }

    /// SynchronizeState as the station opens its session; the frame, which our Yandex session must repeat.
    pub async fn synchronize(&self, message_id: &str) -> Bytes {
        let frame = codec()
            .encode_client_object(
                &json!({ "event": { "header": { "messageId": message_id }, "synchronizeStateRequest": {
                "application": { "deviceModel": "plum", "timezone": "Asia/Dubai" } } } }),
            )
            .unwrap();
        self.send_frame(frame.clone());
        let id = message_id.to_string();
        self.wait_for(
            move |m| {
                at(m, "event.header.refMessageId") == id.as_str() && !at(m, "event.synchronizeStateResponse").is_null()
            },
            4000,
        )
        .await;
        frame
    }

    /// The joined answer stream as a player sees it.
    pub fn stream(&self, stream_id: u32) -> (usize, usize, bool) {
        let bytes: Vec<u8> = lock(&self.audio).get(&stream_id).cloned().unwrap_or_default().concat();
        let events = OggOpusDemuxer::new().push(&bytes);
        let pages = ogg_pages(&bytes);
        let packets = events.iter().filter(|event| matches!(event, OggOpusEvent::Packet { .. })).count();
        let heads = events.iter().filter(|event| matches!(event, OggOpusEvent::Head { .. })).count();
        let eos = pages.last().is_some_and(|page| page[5] & 0x04 != 0);
        (heads, packets, eos)
    }

    pub fn send_audio(&self, stream_id: u32, payload: &[u8]) {
        for chunk in payload.chunks(600) {
            self.send_frame(audio_frame(stream_id, chunk));
        }
    }

    /// VoiceInput as the station sends it after the wake word; with a device state (its player) and an environment
    /// state (its capabilities: alarms and timers) when given: fields 18 and 19 of the request body, as the station
    /// puts them.
    pub fn start_voice(
        &self,
        message_id: &str,
        stream_id: u32,
        request_id: &str,
        device_state: Option<Value>,
        environment: Option<Value>,
    ) {
        let codec = codec();
        let frame = codec
            .encode_client_object(&json!({ "event": {
                "header": { "messageId": message_id, "streamId": stream_id },
                "voiceInput": {
                    "header": { "requestId": request_id, "sequenceNumber": 3 },
                    "request": { "event": { "type": "voice_input" }, "voiceSession": true, "activationType": "spotter" },
                    "enableSpotterValidation": true,
                    "spotterPhrase": "алиса",
                },
            } }))
            .unwrap();
        if device_state.is_none() && environment.is_none() {
            return self.send_frame(frame);
        }
        let with_states = |request: &[u8]| {
            let mut body = request.to_vec();
            if let Some(state) = &device_state {
                body = set_bytes(&body, 18, codec.encode_message("DeviceState", state).unwrap())?;
            }
            if let Some(environment) = &environment {
                body = set_bytes(&body, 19, codec.encode_message("EnvironmentState", environment).unwrap())?;
            }
            Ok(body)
        };
        let body = edit_message(&frame[4..], 1, |event| {
            edit_message(event, 3, |voice_input| edit_message(voice_input, 3, with_states))
        })
        .unwrap();
        self.send_frame(Bytes::from([&frame[..4], &body[..]].concat()));
    }

    /// A text the station sends (typed, or a callback of a command it ran).
    pub fn text_input(&self, message_id: &str, request_id: &str, event: Value) {
        self.send(json!({ "event": {
            "header": { "messageId": message_id },
            "textInput": { "header": { "requestId": request_id }, "request": { "event": event } },
        } }));
    }
}

/// Our pushes: the station calls back with each one (a server_action of ours), as the real one does.
fn on_push(
    codec: &AapiCodec,
    bytes: &[u8],
    pushes: &Mutex<Vec<Value>>,
    answers: &AtomicBool,
    out: &mpsc::UnboundedSender<Message>,
) {
    // Yandex's own pushes are passed on as they are; this test station does not read them.
    let Ok(push) = codec.decode_push(bytes) else { return };
    let directive = at(&push, "directive");
    let name = at(directive, "name").as_str().unwrap_or("").to_string();
    if at(directive, "type") != "server_action" || !name.starts_with("vgw_") {
        return;
    }
    lock(pushes).push(push.clone());
    if !answers.load(Ordering::SeqCst) {
        return;
    }
    let id = at(&push, "id").as_str().unwrap_or("").to_string();
    let callback = json!({ "event": {
        "header": { "messageId": format!("callback-{id}") },
        "textInput": {
            "header": { "requestId": format!("req-callback-{id}") },
            "request": { "event": { "type": "server_action", "name": name, "payload": at(directive, "payload") } },
        },
    } });
    if let Ok(frame) = codec.encode_client_object(&callback) {
        let _ = out.send(Message::Binary(frame));
    }
}

// ---------------------------------------------------------------- HTTP fakes: recognition, the model, the weather

/// OpenAI-compatible transcription stub: records what the gateway sends, answers with `reply` (tests change it).
pub struct AsrStub {
    pub url: String,
    pub calls: Arc<Mutex<Vec<String>>>,
    pub reply: Arc<Mutex<String>>,
}

pub const DEFAULT_REPLY: &str = "Алиса, привет.";

#[derive(Clone)]
struct AsrState {
    calls: Arc<Mutex<Vec<String>>>,
    reply: Arc<Mutex<String>>,
}

async fn asr_any(
    State(state): State<AsrState>,
    method: Method,
    uri: OriginalUri,
    headers: HeaderMap,
    body: AxumBytes,
) -> Response {
    let kind = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or("").to_string())
        .unwrap_or_else(|| "undefined".into());
    let latin1: String = body.iter().map(|&byte| byte as char).collect();
    lock(&state.calls).push(format!("{method} {} {kind} {latin1}", uri.0));
    let answer = if uri.path().ends_with("/models") {
        json!({ "data": [] })
    } else {
        json!({ "text": lock(&state.reply).clone() })
    };
    axum::Json(answer).into_response()
}

impl AsrStub {
    pub async fn start() -> AsrStub {
        let state = AsrState { calls: Arc::default(), reply: Arc::new(Mutex::new(DEFAULT_REPLY.into())) };
        let app = Router::new().fallback(any(asr_any)).with_state(state.clone());
        let url = serve(app).await;
        AsrStub { url: format!("{url}/v1"), calls: state.calls, reply: state.reply }
    }

    pub fn set_reply(&self, reply: &str) {
        *lock(&self.reply) = reply.to_string();
    }

    pub fn transcriptions(&self) -> Vec<String> {
        lock(&self.calls).iter().filter(|call| call.starts_with("POST /v1/audio/transcriptions")).cloned().collect()
    }
}

pub const DEFAULT_ANSWER: &str = "Здравствуйте.";

#[derive(Clone, Default)]
pub struct ChatState {
    pub reply: Arc<Mutex<String>>,
    /// When set, a request that offers tools gets this call instead of text (the next one, with its result, the
    /// reply).
    pub tool_call: Arc<Mutex<Option<(String, Value)>>>,
    pub requests: Arc<Mutex<Vec<Value>>>,
    /// A slow model (a web search, a long tool): this long before it says anything.
    pub delay_ms: Arc<Mutex<u64>>,
}

/// OpenAI-compatible chat stub: streams `reply` in small deltas, as a model does, or calls a tool.
pub struct ChatStub {
    pub url: String,
    pub state: ChatState,
}

async fn chat(State(state): State<ChatState>, body: AxumBytes) -> Response {
    let request: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    lock(&state.requests).push(request.clone());
    let delay = *lock(&state.delay_ms);
    if delay > 0 {
        sleep(delay).await;
    }
    let answered = request["messages"].as_array().is_some_and(|messages| messages.iter().any(|m| m["role"] == "tool"));
    let offers_tools = request["tools"].as_array().is_some_and(|tools| !tools.is_empty());
    let mut events = String::new();
    let tool_call = lock(&state.tool_call).clone();
    match tool_call {
        Some((name, arguments)) if offers_tools && !answered => {
            let call =
                json!({ "index": 0, "id": "call_1", "function": { "name": name, "arguments": arguments.to_string() } });
            events.push_str(&format!("data: {}\n\n", json!({ "choices": [{ "delta": { "tool_calls": [call] } }] })));
        }
        _ => {
            let reply: Vec<char> = lock(&state.reply).chars().collect();
            for chunk in reply.chunks(6) {
                let content: String = chunk.iter().collect();
                events.push_str(&format!("data: {}\n\n", json!({ "choices": [{ "delta": { "content": content } }] })));
            }
        }
    }
    events.push_str("data: [DONE]\n\n");
    ([("content-type", "text/event-stream")], events).into_response()
}

impl ChatStub {
    pub async fn start() -> ChatStub {
        let state = ChatState { reply: Arc::new(Mutex::new(DEFAULT_ANSWER.into())), ..ChatState::default() };
        let app = Router::new().route("/v1/chat/completions", post(chat)).with_state(state.clone());
        let url = serve(app).await;
        ChatStub { url: format!("{url}/v1"), state }
    }

    pub fn set_reply(&self, reply: &str) {
        *lock(&self.state.reply) = reply.to_string();
    }

    pub fn set_delay(&self, ms: u64) {
        *lock(&self.state.delay_ms) = ms;
    }

    pub fn set_tool_call(&self, call: Option<(&str, Value)>) {
        *lock(&self.state.tool_call) = call.map(|(name, arguments)| (name.to_string(), arguments));
    }

    pub fn requests(&self) -> Vec<Value> {
        lock(&self.state.requests).clone()
    }
}

/// Open-Meteo as a slow stub: our weather takes its time, as it does live.
pub async fn start_forecast() -> String {
    let app = Router::new().route(
        "/v1/forecast",
        get(|| async {
            sleep(700).await;
            axum::Json(json!({ "current": { "temperature_2m": 10, "weather_code": 0 } }))
        }),
    );
    format!("{}/v1/forecast", serve(app).await)
}

/// Serves a router on a free port of 127.0.0.1; its base URL.
pub async fn serve(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{address}")
}

/// The kind of a station message, as the gateway's journal names it: its decoded event's field.
pub fn station_kind(frame: &[u8]) -> String {
    match codec().decode_client(frame).map(|decoded| decoded.message) {
        Ok(ClientMessage::StreamControl(control)) => format!("streamControl:{:?}", control.action),
        Ok(ClientMessage::Event(event)) => event.kind().to_string(),
        Err(_) => "?".into(),
    }
}

/// Whether a frame is the station's VoiceInput, TextInput, …: by its decoded event.
pub fn client_event(frame: &[u8]) -> Option<ClientEvent> {
    match codec().decode_client(frame).ok()?.message {
        ClientMessage::Event(event) => Some(event),
        ClientMessage::StreamControl(_) => None,
    }
}

/// A stream control's action, when the frame is one.
pub fn stream_action(frame: &[u8]) -> Option<(u32, StreamAction)> {
    match codec().decode_client(frame).ok()?.message {
        ClientMessage::StreamControl(control) => Some((control.stream_id, control.action)),
        ClientMessage::Event(_) => None,
    }
}

/// A server frame from a plain object, for what encode_server does not build (Yandex's pushes).
pub fn server_frame(object: &Value) -> Bytes {
    codec().encode_server_object(object).expect("an encodable server object")
}

pub fn bytes_of(bytes: &[u8]) -> Value {
    bytes_value(bytes)
}

/// encodeURIComponent of a path part.
pub fn encode_uri(text: &str) -> String {
    duowave::skills::js::encode_uri_component(text)
}
