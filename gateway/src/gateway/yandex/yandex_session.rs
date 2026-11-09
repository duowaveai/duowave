//! Our own uniproxy session with Yandex for one station, opened with the station's credentials (its handshake
//! headers and its SynchronizeState frame) and kept open while the station is connected: pinged, and reopened after
//! a close. Over it go speech in Alice's voice (TtsGenerate per phrase, answered with TtsSpeak and an Ogg/Opus
//! stream), requests on the Yandex route and the callbacks of Yandex's commands (an exchange each: the replies refer
//! to its message id), and Yandex's pushes come back. The station's audio and the questions we answer ourselves
//! never go up.
//!
//! The state is behind one lock taken for each step, as JavaScript ran each callback whole; it is never held while
//! the station session is called (its pushes, the exchanges' handlers), which takes its own.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use axum::http::HeaderMap;
use bytes::Bytes;
use futures::future::{BoxFuture, FutureExt, Shared};
use futures::stream::{BoxStream, SplitStream, StreamExt};
use futures::SinkExt;
use serde_json::{json, Map, Value};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;

use crate::config::TtsConfig;
use crate::gateway::observability::session_recorder::EventDirection;
use crate::gateway::protocol::aapi_codec::AapiCodec;
use crate::gateway::protocol::ids::new_message_id;
use crate::gateway::protocol::proto_object::bytes_value;
use crate::utils::http::ws_connector;

/// A WebSocket to Yandex's uniproxy.
pub type UpstreamSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct StationCredentials {
    /// Handshake headers of the station's own connection: device id, OAuth token, app and firmware.
    pub headers: Vec<(String, String)>,
    /// The station's SynchronizeState frame as received. It carries the session's tokens, which our schema does not
    /// declare, so it goes up byte for byte.
    pub synchronize_state: Bytes,
}

/// Where our session's frames are journaled: the station session's journal (toYandex, fromYandex, notes).
pub trait UpstreamJournal: Send + Sync {
    fn record(&self, dir: EventDirection, kind: &str, summary: Option<String>, data: Option<Value>);
    fn note(&self, kind: &str, summary: &str);
}

/// One phrase of synthesis: "audio/opus" (Ogg/Opus) bytes until the phrase ends; an `Err` when the stream breaks or
/// stalls.
pub struct Synthesis {
    pub format: String,
    pub audio: BoxStream<'static, Result<Bytes>>,
}

/// Takes Yandex's frames about one request of ours: its events, the audio of the streams they open. A failure (a
/// frame that cannot be edited) is journaled by the session as a frame error.
pub trait ExchangeHandler: Send + Sync {
    fn on_event(&self, frame: &Bytes, kind: &str, decoded: &Value) -> Result<()>;
    fn on_audio(&self, frame: &Bytes, stream_id: u32) -> Result<()>;
    fn on_stream_control(&self, frame: &Bytes, stream_id: u32, decoded: &Value) -> Result<()>;
    /// The session closed under the request.
    fn on_closed(&self, error: &str);
}

/// A request of ours in flight: Yandex's replies refer to its message id.
pub struct Exchange {
    pub message_id: String,
    pub handler: Arc<dyn ExchangeHandler>,
    streams: Mutex<Vec<u32>>,
}

/// What comes from Yandex unasked.
pub trait YandexEvents: Send + Sync {
    /// A frame that belongs to no request of ours: a push (5), a sync push (8), anything else unasked.
    fn on_push(&self, frame: Bytes, kind: &str, decoded: &Value);
    /// Audio, or a stream's close, on a stream no request of ours opened (a push that speaks).
    fn on_stray_stream(&self, frame: Bytes, stream_id: u32, is_control: bool);
    /// The session is up (synchronized) or down.
    fn on_state(&self, _connected: bool) {}
}

#[derive(Debug, Clone)]
pub struct YandexSessionOptions {
    /// Yandex's uniproxy (YANDEX_UNIPROXY_URL).
    pub url: String,
    pub tts: TtsConfig,
    /// YANDEX_PING_MS: the 01.11 lesson — a quiet connection can die without a close.
    pub ping_ms: u64,
    /// Pauses before reopening after a close or a failed open; the last one repeats.
    pub reconnect_ms: Vec<u64>,
}

// Hop-by-hop and WebSocket handshake headers belong to the station's connection, not to ours.
const SKIP_HEADERS: [&str; 10] = [
    "host",
    "connection",
    "upgrade",
    "origin",
    "content-length",
    "transfer-encoding",
    "sec-websocket-key",
    "sec-websocket-version",
    "sec-websocket-extensions",
    "sec-websocket-protocol",
];

/// The station's handshake headers to repeat on our connection to Yandex (a repeated one joined with ", ").
pub fn upstream_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    let mut picked: Vec<(String, String)> = Vec::new();
    for name in headers.keys() {
        if SKIP_HEADERS.contains(&name.as_str()) {
            continue;
        }
        let values: Vec<&str> = headers.get_all(name).iter().filter_map(|value| value.to_str().ok()).collect();
        picked.push((name.as_str().to_string(), values.join(", ")));
    }
    picked
}

/// Why a connection to Yandex did not open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    /// Yandex answered the upgrade with this HTTP status.
    Rejected(u16),
    Failed(String),
}

/// A WebSocket to Yandex with the station's headers, without compression (as the station's own, which Yandex
/// accepts): our TLS (`ws_connector`), nothing from the system.
pub async fn connect_upstream(url: &str, headers: &[(String, String)]) -> Result<UpstreamSocket, ConnectError> {
    let mut request = url.into_client_request().map_err(|error| ConnectError::Failed(error.to_string()))?;
    for (name, value) in headers {
        let (Ok(name), Ok(value)) = (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value)) else {
            continue;
        };
        request.headers_mut().insert(name, value);
    }
    match tokio_tungstenite::connect_async_tls_with_config(request, None, false, Some(ws_connector())).await {
        Ok((socket, _)) => Ok(socket),
        Err(WsError::Http(response)) => Err(ConnectError::Rejected(response.status().as_u16())),
        Err(error) => Err(ConnectError::Failed(error.to_string())),
    }
}

/// An id's first eight characters, `-` for none.
/// A stream close's reason as the TS read it (`close.reason ?? 'CLOSE_OK'`): the reason's name, or its number when
/// our schema names no such reason (protobufjs gave the number then too); CLOSE_OK only when the close has none.
/// Only "CLOSE_OK" is a clean end.
pub fn close_reason(close: &Value) -> String {
    match close.get("reason") {
        None | Some(Value::Null) => "CLOSE_OK".into(),
        Some(Value::String(name)) => name.clone(),
        Some(other) => other.to_string(),
    }
}

fn short(value: &str) -> String {
    if value.is_empty() {
        "-".into()
    } else {
        value.chars().take(8).collect()
    }
}

/// Dropped phrases remembered until their late frames come, at most this many.
const DROPPED_KEPT: usize = 64;

/// The station session ended: nothing of ours goes up any more.
const SESSION_ENDED: &str = "the station session ended";

struct PhraseState {
    stream_id: Option<u32>,
    bytes: usize,
    start: Option<oneshot::Sender<std::result::Result<Synthesis, String>>>,
    audio: Option<mpsc::UnboundedSender<std::result::Result<Bytes, String>>>,
    waiting: Option<mpsc::UnboundedReceiver<std::result::Result<Bytes, String>>>,
    /// When the armed timeout fires, and what it waits for.
    deadline: Instant,
    what: &'static str,
}

/// One phrase being synthesized: waits for TtsSpeak, then queues its audio until Yandex closes the stream.
struct Phrase {
    message_id: String,
    timeout: Duration,
    /// Ended or failed: its watcher stops.
    released: CancellationToken,
    state: Mutex<PhraseState>,
}

impl Phrase {
    fn new(timeout_ms: u64) -> (Arc<Phrase>, oneshot::Receiver<std::result::Result<Synthesis, String>>) {
        let (start, started) = oneshot::channel();
        let (audio, waiting) = mpsc::unbounded_channel();
        let timeout = Duration::from_millis(timeout_ms);
        let phrase = Arc::new(Phrase {
            message_id: new_message_id(),
            timeout,
            released: CancellationToken::new(),
            state: Mutex::new(PhraseState {
                stream_id: None,
                bytes: 0,
                start: Some(start),
                audio: Some(audio),
                waiting: Some(waiting),
                // Armed when the phrase goes up (`ask`).
                deadline: Instant::now() + timeout,
                what: "no TtsSpeak",
            }),
        });
        (phrase, started)
    }

    fn lock(&self) -> MutexGuard<'_, PhraseState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn stream_id(&self) -> Option<u32> {
        self.lock().stream_id
    }

    fn arm(&self, state: &mut PhraseState, what: &'static str) {
        state.deadline = Instant::now() + self.timeout;
        state.what = what;
    }

    fn start(&self, stream_id: u32, format: String) {
        let mut state = self.lock();
        state.stream_id = Some(stream_id);
        self.arm(&mut state, "no audio");
        if let (Some(start), Some(waiting)) = (state.start.take(), state.waiting.take()) {
            let audio = UnboundedReceiverStream::new(waiting).map(|chunk| chunk.map_err(|error| anyhow!(error)));
            let _ = start.send(Ok(Synthesis { format, audio: audio.boxed() }));
        }
    }

    fn push(&self, chunk: Bytes) {
        let mut state = self.lock();
        state.bytes += chunk.len();
        self.arm(&mut state, "audio stalled");
        if let Some(audio) = &state.audio {
            let _ = audio.send(Ok(chunk));
        }
    }

    fn end(&self) {
        self.released.cancel();
        let mut state = self.lock();
        state.audio = None;
        state.start = None;
    }

    fn fail(&self, error: &str) {
        self.released.cancel();
        let mut state = self.lock();
        if let Some(start) = state.start.take() {
            let _ = start.send(Err(error.to_string()));
        }
        if let Some(audio) = state.audio.take() {
            let _ = audio.send(Err(error.to_string()));
        }
    }
}

/// The open connection: its writer's queue and its end.
struct Socket {
    id: u64,
    sender: mpsc::UnboundedSender<Message>,
    cancel: CancellationToken,
    /// False once it is being cut (`terminate`): its close comes from the reader.
    open: bool,
}

type Opening = Shared<BoxFuture<'static, std::result::Result<(), String>>>;

/// A JS Set kept in insertion order and bounded by dropping its oldest.
struct Bounded<T>(VecDeque<T>);

impl<T: PartialEq> Bounded<T> {
    fn contains(&self, value: &T) -> bool {
        self.0.contains(value)
    }

    fn add(&mut self, value: T) {
        if !self.0.contains(&value) {
            self.0.push_back(value);
        }
        while self.0.len() > DROPPED_KEPT {
            self.0.pop_front();
        }
    }

    fn remove(&mut self, value: &T) {
        self.0.retain(|kept| kept != value);
    }
}

/// A phrase asked for before the session was up: it goes up, in its turn, once it is.
struct Unsent {
    phrase: Arc<Phrase>,
    text: String,
    cancel: CancellationToken,
}

struct State {
    socket: Option<Socket>,
    /// Phrases waiting for the session, in the order they were asked for.
    unsent: Vec<Unsent>,
    opening: Option<Opening>,
    by_message: HashMap<String, Arc<Phrase>>,
    by_stream: HashMap<u32, Arc<Phrase>>,
    exchanges: HashMap<String, Arc<Exchange>>,
    exchange_streams: HashMap<u32, Arc<Exchange>>,
    /// Phrases dropped before their audio ended (a cancelled answer): what Yandex still sends for them is ours, not a
    /// push. Live, the audio of a cancelled answer went to the station as Yandex's push.
    dropped_messages: Bounded<String>,
    dropped_streams: Bounded<u32>,
    closed: bool,
    persistent: bool,
    reconnect: Option<CancellationToken>,
    failures: usize,
    heartbeat: Option<CancellationToken>,
    /// Yandex answered a ping or sent a frame since the last ping.
    heard: bool,
}

impl State {
    fn open_socket(&self) -> Option<&Socket> {
        self.socket.as_ref().filter(|socket| socket.open)
    }
}

/// What failAll hands out of the lock: phrases to fail, exchanges to tell.
struct Failed {
    exchanges: Vec<Arc<Exchange>>,
}

pub struct YandexSession {
    options: YandexSessionOptions,
    credentials: StationCredentials,
    codec: Arc<AapiCodec>,
    journal: Arc<dyn UpstreamJournal>,
    events: Arc<dyn YandexEvents>,
    state: Mutex<State>,
    next_socket: AtomicU64,
    me: Weak<YandexSession>,
}

impl YandexSession {
    pub fn new(
        options: YandexSessionOptions,
        credentials: StationCredentials,
        codec: Arc<AapiCodec>,
        journal: Arc<dyn UpstreamJournal>,
        events: Arc<dyn YandexEvents>,
    ) -> Arc<YandexSession> {
        Arc::new_cyclic(|me| YandexSession {
            options,
            credentials,
            codec,
            journal,
            events,
            state: Mutex::new(State {
                socket: None,
                unsent: Vec::new(),
                opening: None,
                by_message: HashMap::new(),
                by_stream: HashMap::new(),
                exchanges: HashMap::new(),
                exchange_streams: HashMap::new(),
                dropped_messages: Bounded(VecDeque::new()),
                dropped_streams: Bounded(VecDeque::new()),
                closed: false,
                persistent: false,
                reconnect: None,
                failures: 0,
                heartbeat: None,
                heard: true,
            }),
            next_socket: AtomicU64::new(1),
            me: me.clone(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// For the lock-order test: holds the state, runs `meanwhile`, then journals a note still under it, as `open`,
    /// `dispatch` and the heartbeat journal.
    #[cfg(test)]
    pub(crate) fn note_under_state(&self, meanwhile: impl FnOnce(), kind: &str, summary: &str) {
        let _state = self.lock();
        meanwhile();
        self.journal.note(kind, summary);
    }

    /// Synchronized with Yandex right now.
    pub fn connected(&self) -> bool {
        self.lock().open_socket().is_some()
    }

    /// Whether the session is up, waiting up to `timeout_ms` for one that is opening (the station just connected).
    pub async fn ready(&self, timeout_ms: u64) -> bool {
        let opening = {
            let state = self.lock();
            if state.open_socket().is_some() {
                return true;
            }
            state.opening.clone()
        };
        let Some(opening) = opening else { return false };
        tokio::select! {
            _ = opening => {}
            _ = tokio::time::sleep(Duration::from_millis(timeout_ms)) => {}
        }
        self.connected()
    }

    /// Opens the session now and keeps it open: pushes reach the station only through it.
    pub fn start(&self) {
        self.lock().persistent = true;
        drop(self.connect());
    }

    /// Opens the session ahead of a phrase or a request, while the station is still listening.
    pub fn warm_up(&self) {
        let opening = self.connect();
        let journal = self.journal.clone();
        tokio::spawn(async move {
            if let Err(error) = opening.await {
                journal.note("yandexUnavailable", &error);
            }
        });
    }

    /// Resolves when the audio starts; the stream then yields Ogg bytes until the phrase ends. The phrase is asked
    /// for at once, in the order of the calls (a phrase asked before the session is up goes up as soon as it is, in
    /// its turn): the first phrase's audio comes first, as the TS's awaits kept it.
    pub fn synthesize(
        self: &Arc<Self>,
        text: &str,
        cancel: &CancellationToken,
    ) -> BoxFuture<'static, Result<Synthesis>> {
        let (phrase, started) = Phrase::new(self.options.tts.timeout_ms);
        let unsent = Unsent { phrase, text: text.to_string(), cancel: cancel.clone() };
        let waits = {
            let mut state = self.lock();
            if state.closed {
                return futures::future::ready(Err(anyhow!(SESSION_ENDED))).boxed();
            }
            if state.open_socket().is_some() {
                self.ask(&mut state, unsent);
                false
            } else {
                state.unsent.push(unsent);
                true
            }
        };
        if waits {
            drop(self.connect());
        }
        async move {
            match started.await {
                Ok(Ok(synthesis)) => Ok(synthesis),
                Ok(Err(error)) => Err(anyhow!(error)),
                Err(_) => Err(anyhow!("the phrase ended before its speech began")),
            }
        }
        .boxed()
    }

    /// A phrase goes up: TtsGenerate under its message id, its timeout armed; a phrase cancelled while it waited for
    /// the session fails instead.
    fn ask(&self, state: &mut State, unsent: Unsent) {
        let Unsent { phrase, text, cancel } = unsent;
        if cancel.is_cancelled() {
            return phrase.fail("This operation was aborted");
        }
        let tts = &self.options.tts;
        let mut generate = json!({ "text": text, "lang": tts.lang, "format": "audio/opus" });
        if let Some(voice) = &tts.voice {
            generate["voice"] = json!(voice);
        }
        let object = json!({ "event": { "header": { "messageId": phrase.message_id }, "ttsGenerate": generate } });
        let frame = match self.codec.encode_client_object(&object) {
            Ok(frame) => frame,
            Err(error) => return phrase.fail(&error.to_string()),
        };
        state.by_message.insert(phrase.message_id.clone(), phrase.clone());
        if let Some(socket) = state.open_socket() {
            let _ = socket.sender.send(Message::Binary(frame));
        }
        let mut data = Map::new();
        data.insert("messageId".into(), json!(phrase.message_id));
        if let Value::Object(fields) = generate {
            data.extend(fields);
        }
        let quoted: String = text.chars().take(60).collect();
        self.journal.record(
            EventDirection::ToYandex,
            "ttsGenerate",
            Some(format!("\"{quoted}\"")),
            Some(Value::Object(data)),
        );
        {
            let mut armed = phrase.lock();
            phrase.arm(&mut armed, "no TtsSpeak");
        }
        self.watch_phrase(phrase, cancel);
    }

    /// A phrase's timeout and its answer's cancel: either drops it, until it ends.
    fn watch_phrase(&self, phrase: Arc<Phrase>, cancel: CancellationToken) {
        let Some(session) = self.me.upgrade() else { return };
        tokio::spawn(async move {
            loop {
                let (deadline, what) = {
                    let state = phrase.lock();
                    (state.deadline, state.what)
                };
                tokio::select! {
                    _ = phrase.released.cancelled() => return,
                    _ = cancel.cancelled() => {
                        session.drop_phrase(&phrase, "This operation was aborted");
                        return;
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        if phrase.lock().deadline > Instant::now() {
                            continue;
                        }
                        let ms = phrase.timeout.as_millis();
                        session.drop_phrase(&phrase, &format!("{what} from Yandex in {ms} ms"));
                        return;
                    }
                }
            }
        });
    }

    /// Sends a request (its frames: the event, then any audio and stream controls) and routes Yandex's replies to
    /// `handler` until `end_exchange`. Fails when the session is down.
    pub fn exchange(
        &self,
        message_id: &str,
        frames: Vec<Bytes>,
        handler: Arc<dyn ExchangeHandler>,
        label: &str,
    ) -> Result<Arc<Exchange>> {
        let mut state = self.lock();
        let Some(sender) = state.open_socket().map(|socket| socket.sender.clone()) else {
            bail!("the Yandex session is not open");
        };
        let exchange =
            Arc::new(Exchange { message_id: message_id.to_string(), handler, streams: Mutex::new(Vec::new()) });
        state.exchanges.insert(message_id.to_string(), exchange.clone());
        let count = frames.len();
        for frame in frames {
            let _ = sender.send(Message::Binary(frame));
        }
        self.journal.record(
            EventDirection::ToYandex,
            label,
            Some(format!("ref={} {count} frame(s)", short(message_id))),
            None,
        );
        Ok(exchange)
    }

    /// Stops routing an exchange's replies, after a moment for late frames.
    pub fn end_exchange(&self, exchange: Arc<Exchange>, grace_ms: u64) {
        let me = self.me.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(grace_ms)).await;
            let Some(session) = me.upgrade() else { return };
            let mut state = session.lock();
            if state.exchanges.get(&exchange.message_id).is_some_and(|known| Arc::ptr_eq(known, &exchange)) {
                state.exchanges.remove(&exchange.message_id);
            }
            let streams = exchange.streams.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
            for stream_id in streams {
                if state.exchange_streams.get(&stream_id).is_some_and(|known| Arc::ptr_eq(known, &exchange)) {
                    state.exchange_streams.remove(&stream_id);
                }
            }
        });
    }

    /// A station frame for Yandex as it is (a sync push response, an ack of a push, a stream of a request); false
    /// when the session is down.
    pub fn send(&self, frame: Bytes, kind: &str) -> bool {
        let state = self.lock();
        let Some(socket) = state.open_socket() else { return false };
        let _ = socket.sender.send(Message::Binary(frame));
        if kind != "audio" {
            self.journal.record(EventDirection::ToYandex, kind, Some("from the station".into()), None);
        }
        true
    }

    pub fn close(&self) {
        let failed = {
            let mut state = self.lock();
            state.closed = true;
            if let Some(reconnect) = state.reconnect.take() {
                reconnect.cancel();
            }
            if let Some(heartbeat) = state.heartbeat.take() {
                heartbeat.cancel();
            }
            if let Some(socket) = state.socket.as_mut() {
                socket.open = false;
                socket.cancel.cancel();
            }
            self.fail_all(&mut state, SESSION_ENDED)
        };
        Self::tell_failed(failed, SESSION_ENDED);
    }

    fn connect(&self) -> BoxFuture<'static, std::result::Result<(), String>> {
        let mut state = self.lock();
        if state.closed {
            return futures::future::ready(Err(SESSION_ENDED.to_string())).boxed();
        }
        if state.open_socket().is_some() {
            return futures::future::ready(Ok(())).boxed();
        }
        if let Some(opening) = &state.opening {
            return opening.clone().boxed();
        }
        let Some(session) = self.me.upgrade() else {
            return futures::future::ready(Err(SESSION_ENDED.to_string())).boxed();
        };
        // Started now, whoever waits for it: as a promise did.
        let task = tokio::spawn(async move {
            let opened = session.open().await;
            let mut state = session.lock();
            match &opened {
                Ok(()) => state.failures = 0,
                Err(error) => {
                    state.failures += 1;
                    session.schedule_reconnect(&mut state);
                    for unsent in std::mem::take(&mut state.unsent) {
                        unsent.phrase.fail(error);
                    }
                }
            }
            state.opening = None;
            opened
        });
        let opening: Opening =
            async move { task.await.unwrap_or_else(|error| Err(error.to_string())) }.boxed().shared();
        state.opening = Some(opening.clone());
        opening.boxed()
    }

    fn schedule_reconnect(&self, state: &mut State) {
        if !state.persistent || state.closed || state.reconnect.is_some() {
            return;
        }
        let pauses = &self.options.reconnect_ms;
        let index = state.failures.saturating_sub(1).min(pauses.len().saturating_sub(1));
        let delay = pauses.get(index).copied().unwrap_or(5000);
        let cancel = CancellationToken::new();
        state.reconnect = Some(cancel.clone());
        let me = self.me.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = tokio::time::sleep(Duration::from_millis(delay)) => {
                    let Some(session) = me.upgrade() else { return };
                    session.lock().reconnect = None;
                    drop(session.connect());
                }
            }
        });
    }

    /// Fails the open: the socket goes, the reason is journaled.
    fn unavailable(&self, error: String) -> std::result::Result<(), String> {
        self.journal.note("yandexUnavailable", &error);
        Err(error)
    }

    async fn open(&self) -> std::result::Result<(), String> {
        let timeout_ms = self.options.tts.timeout_ms;
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let no_response = || format!("no SynchronizeStateResponse from Yandex in {timeout_ms} ms");
        let socket =
            match tokio::time::timeout_at(deadline, connect_upstream(&self.options.url, &self.credentials.headers))
                .await
            {
                Err(_) => return self.unavailable(no_response()),
                Ok(Err(ConnectError::Rejected(status))) => {
                    return self.unavailable(format!("Yandex answered the upgrade with HTTP {status}"))
                }
                Ok(Err(ConnectError::Failed(error))) => return self.unavailable(error),
                Ok(Ok(socket)) => socket,
            };
        let (mut sink, mut stream) = socket.split();
        let sync = self.credentials.synchronize_state.clone();
        let length = sync.len();
        match tokio::time::timeout_at(deadline, sink.send(Message::Binary(sync))).await {
            Err(_) => return self.unavailable(no_response()),
            Ok(Err(error)) => return self.unavailable(error.to_string()),
            Ok(Ok(())) => {}
        }
        self.journal.record(
            EventDirection::ToYandex,
            "synchronizeState",
            Some(format!("the station's frame, {length} bytes")),
            None,
        );
        loop {
            let next = match tokio::time::timeout_at(deadline, stream.next()).await {
                Err(_) => return self.unavailable(no_response()),
                Ok(next) => next,
            };
            match next {
                None => return self.unavailable("Yandex closed the connection during the handshake: 1006 ".into()),
                Some(Err(error)) => return self.unavailable(error.to_string()),
                Some(Ok(Message::Close(frame))) => {
                    let (code, reason) =
                        frame.map_or((1005, String::new()), |f| (u16::from(f.code), f.reason.to_string()));
                    return self
                        .unavailable(format!("Yandex closed the connection during the handshake: {code} {reason}"));
                }
                Some(Ok(Message::Binary(frame))) => {
                    let synced = AapiCodec::is_aapi_frame(&frame)
                        && self.codec.decode_server_frame(&frame).ok().is_some_and(|decoded| {
                            decoded.get("event").and_then(|event| event.get("synchronizeStateResponse")).is_some()
                        });
                    if synced {
                        break;
                    }
                    self.lock().heard = true;
                    self.on_frame(frame, true);
                }
                Some(Ok(Message::Text(text))) => {
                    self.lock().heard = true;
                    self.on_frame(Bytes::copy_from_slice(text.as_bytes()), false);
                }
                Some(Ok(Message::Pong(_))) => self.lock().heard = true,
                Some(Ok(_)) => {}
            }
        }
        let id = self.next_socket.fetch_add(1, Ordering::Relaxed);
        let cancel = CancellationToken::new();
        let (sender, mut outgoing) = mpsc::unbounded_channel::<Message>();
        let writer_cancel = cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = writer_cancel.cancelled() => break,
                    message = outgoing.recv() => match message {
                        Some(message) => if sink.send(message).await.is_err() { break },
                        None => break,
                    },
                }
            }
            let _ = sink.close().await;
        });
        {
            let mut state = self.lock();
            if state.closed {
                // The station left while we were opening: nothing of ours stays up (the TS kept this one open).
                cancel.cancel();
                return Err(SESSION_ENDED.into());
            }
            state.socket = Some(Socket { id, sender, cancel: cancel.clone(), open: true });
            self.journal.note("yandexConnected", &format!("session open at {}", self.options.url));
            self.start_heartbeat(&mut state, id);
            for unsent in std::mem::take(&mut state.unsent) {
                self.ask(&mut state, unsent);
            }
        }
        self.events.on_state(true);
        let Some(session) = self.me.upgrade() else { return Err(SESSION_ENDED.into()) };
        tokio::spawn(async move { session.read(stream, id, cancel).await });
        Ok(())
    }

    async fn read(self: Arc<Self>, mut stream: SplitStream<UpstreamSocket>, id: u64, cancel: CancellationToken) {
        let (code, reason) = loop {
            let next = tokio::select! {
                _ = cancel.cancelled() => break (1006, String::new()),
                next = stream.next() => next,
            };
            match next {
                None => break (1006, String::new()),
                Some(Err(error)) => {
                    self.journal.note("yandexSocketError", &error.to_string());
                    break (1006, String::new());
                }
                Some(Ok(Message::Close(frame))) => {
                    break frame.map_or((1005, String::new()), |f| (u16::from(f.code), f.reason.to_string()))
                }
                Some(Ok(Message::Pong(_))) => self.lock().heard = true,
                Some(Ok(Message::Binary(frame))) => {
                    self.lock().heard = true;
                    self.on_frame(frame, true);
                }
                Some(Ok(Message::Text(text))) => {
                    self.lock().heard = true;
                    self.on_frame(Bytes::copy_from_slice(text.as_bytes()), false);
                }
                Some(Ok(_)) => {}
            }
        };
        self.on_closed(id, code, &reason);
    }

    /// Between answers nothing goes over the session, and a dead connection is found only by the next phrase, which
    /// then waits out TTS_TIMEOUT_MS. A ping per interval finds it early: no answer by the next ping and the socket
    /// is cut, then reopened.
    fn start_heartbeat(&self, state: &mut State, id: u64) {
        if let Some(heartbeat) = state.heartbeat.take() {
            heartbeat.cancel();
        }
        let ping_ms = self.options.ping_ms;
        if ping_ms == 0 {
            return;
        }
        state.heard = true;
        let cancel = CancellationToken::new();
        state.heartbeat = Some(cancel.clone());
        let me = self.me.clone();
        tokio::spawn(async move {
            let period = Duration::from_millis(ping_ms);
            let mut ticks = tokio::time::interval_at(Instant::now() + period, period);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = ticks.tick() => {}
                }
                let Some(session) = me.upgrade() else { return };
                let mut state = session.lock();
                let heard = state.heard;
                let Some(socket) = state.socket.as_mut().filter(|socket| socket.id == id && socket.open) else {
                    continue;
                };
                if !heard {
                    session.journal.note(
                        "yandexSilent",
                        &format!("no answer from Yandex in {ping_ms} ms: the session is cut and opened again"),
                    );
                    socket.open = false;
                    socket.cancel.cancel();
                    continue;
                }
                let _ = socket.sender.send(Message::Ping(Bytes::new()));
                state.heard = false;
            }
        });
    }

    fn on_frame(&self, frame: Bytes, is_binary: bool) {
        if let Err(error) = self.dispatch(frame, is_binary) {
            self.journal.note("yandexFrameError", &format!("{error:#}"));
        }
    }

    fn dispatch(&self, frame: Bytes, is_binary: bool) -> Result<()> {
        if !is_binary {
            let text: String = String::from_utf8_lossy(&frame).chars().take(300).collect();
            self.journal.record(EventDirection::FromYandex, "text", Some(text), None);
            return Ok(());
        }
        if !AapiCodec::is_aapi_frame(&frame) {
            return self.on_audio(frame);
        }
        let decoded = self.codec.decode_server_frame(&frame).map_err(|error| anyhow!("{error}"))?;
        if let Some(control) = decoded.get("streamControl") {
            let stream_id = control.get("streamId").and_then(Value::as_u64).unwrap_or(0) as u32;
            return self.on_stream_control(frame, stream_id, &decoded);
        }
        let empty = Value::Object(Map::new());
        let event = decoded.get("event").unwrap_or(&empty);
        let header = event.get("header");
        let r#ref = header
            .and_then(|header| header.get("refMessageId"))
            .and_then(Value::as_str)
            .filter(|r#ref| !r#ref.is_empty());
        let stream_id = header.and_then(|header| header.get("streamId")).and_then(Value::as_u64);
        let kind = self.codec.server_event_kind(&decoded).unwrap_or("unknown").to_string();
        let exchange = {
            let mut state = self.lock();
            let phrase = r#ref.and_then(|r#ref| state.by_message.get(r#ref).cloned());
            if let Some(r#ref) = r#ref.filter(|r#ref| state.dropped_messages.contains(&r#ref.to_string())) {
                let summary = format!("ref={}: of a dropped phrase, not played", short(r#ref));
                self.journal.record(EventDirection::FromYandex, &kind, Some(summary), Some(decoded.clone()));
                if kind == "ttsSpeak" {
                    if let Some(stream_id) = stream_id {
                        state.dropped_streams.add(stream_id as u32);
                    }
                }
                return Ok(());
            }
            if let (true, Some(phrase)) = (kind == "ttsSpeak", phrase) {
                let format = event
                    .get("ttsSpeak")
                    .and_then(|speak| speak.get("format"))
                    .and_then(Value::as_str)
                    .unwrap_or("audio/opus")
                    .to_string();
                let stream = stream_id.map_or_else(|| "undefined".to_string(), |id| id.to_string());
                let summary = format!("ref={} stream={stream} {format}", short(r#ref.unwrap_or_default()));
                self.journal.record(EventDirection::FromYandex, "ttsSpeak", Some(summary), Some(decoded.clone()));
                let stream_id = stream_id.unwrap_or(0) as u32;
                state.by_stream.insert(stream_id, phrase.clone());
                phrase.start(stream_id, format);
                return Ok(());
            }
            let exchange = r#ref.and_then(|r#ref| state.exchanges.get(r#ref).cloned());
            // Events our schema does not describe, errors among them, are kept raw to study the protocol.
            let data = if kind == "unknown" { json!({ "frame": bytes_value(&frame[4..]) }) } else { decoded.clone() };
            self.journal.record(
                EventDirection::FromYandex,
                &kind,
                r#ref.map(|r#ref| format!("ref={}", short(r#ref))),
                Some(data),
            );
            if let Some(exchange) = &exchange {
                if let Some(stream_id) = stream_id.filter(|&id| id != 0).map(|id| id as u32) {
                    let mut streams = exchange.streams.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    if !streams.contains(&stream_id) {
                        streams.push(stream_id);
                    }
                    state.exchange_streams.insert(stream_id, exchange.clone());
                }
            }
            exchange
        };
        if let Some(exchange) = exchange {
            return exchange.handler.on_event(&frame, &kind, &decoded);
        }
        if kind == "synchronizeStateResponse" {
            return Ok(());
        }
        self.events.on_push(frame, &kind, &decoded);
        Ok(())
    }

    /// Audio: [u32 BE stream id][Ogg pages].
    fn on_audio(&self, frame: Bytes) -> Result<()> {
        if frame.len() < 4 {
            return Ok(());
        }
        let stream_id = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]);
        let exchange = {
            let state = self.lock();
            if let Some(phrase) = state.by_stream.get(&stream_id) {
                phrase.push(frame.slice(4..));
                return Ok(());
            }
            let exchange = state.exchange_streams.get(&stream_id).cloned();
            if exchange.is_none() && state.dropped_streams.contains(&stream_id) {
                return Ok(());
            }
            exchange
        };
        match exchange {
            Some(exchange) => exchange.handler.on_audio(&frame, stream_id),
            None => {
                self.events.on_stray_stream(frame, stream_id, false);
                Ok(())
            }
        }
    }

    fn on_stream_control(&self, frame: Bytes, stream_id: u32, decoded: &Value) -> Result<()> {
        let control = decoded.get("streamControl").cloned().unwrap_or(Value::Null);
        let close = control.get("close").filter(|close| !close.is_null());
        let reason = close.map(close_reason).unwrap_or_else(|| "CLOSE_OK".into());
        let summary =
            format!("stream={stream_id}{}", if close.is_some() { format!(" close:{reason}") } else { String::new() });
        self.journal.record(EventDirection::FromYandex, "streamControl", Some(summary), Some(decoded.clone()));
        let exchange = {
            let mut state = self.lock();
            let by_message = control.get("messageId").and_then(Value::as_str).and_then(|id| state.by_message.get(id));
            let phrase = state.by_stream.get(&stream_id).or(by_message).cloned();
            if let Some(phrase) = phrase {
                if close.is_some() {
                    if reason == "CLOSE_OK" {
                        self.forget(&mut state, &phrase);
                        phrase.end();
                    } else {
                        self.drop_locked(&mut state, &phrase, &format!("Yandex closed the audio: {reason}"));
                    }
                }
                return Ok(());
            }
            let exchange = state.exchange_streams.get(&stream_id).cloned();
            if exchange.is_none() && state.dropped_streams.contains(&stream_id) {
                if close.is_some() {
                    state.dropped_streams.remove(&stream_id);
                }
                return Ok(());
            }
            exchange
        };
        match exchange {
            Some(exchange) => exchange.handler.on_stream_control(&frame, stream_id, decoded),
            None => {
                self.events.on_stray_stream(frame, stream_id, true);
                Ok(())
            }
        }
    }

    fn drop_phrase(&self, phrase: &Arc<Phrase>, error: &str) {
        let mut state = self.lock();
        self.drop_locked(&mut state, phrase, error);
    }

    fn drop_locked(&self, state: &mut State, phrase: &Arc<Phrase>, error: &str) {
        self.forget(state, phrase);
        match phrase.stream_id() {
            Some(stream_id) => state.dropped_streams.add(stream_id),
            None => state.dropped_messages.add(phrase.message_id.clone()),
        }
        phrase.fail(error);
    }

    fn forget(&self, state: &mut State, phrase: &Arc<Phrase>) {
        state.by_message.remove(&phrase.message_id);
        if let Some(stream_id) = phrase.stream_id() {
            if state.by_stream.get(&stream_id).is_some_and(|known| Arc::ptr_eq(known, phrase)) {
                state.by_stream.remove(&stream_id);
            }
        }
    }

    fn on_closed(&self, id: u64, code: u16, reason: &str) {
        let error = format!("Yandex closed the session: {code}");
        let failed = {
            let mut state = self.lock();
            if !state.closed {
                let reason = if reason.is_empty() { String::new() } else { format!(" {reason}") };
                self.journal.note("yandexClosed", &format!("session closed: {code}{reason}"));
            }
            // A late close of a replaced socket must not fail the phrases of the new one; its own phrases time out.
            if state.socket.as_ref().is_none_or(|socket| socket.id != id) {
                return;
            }
            state.socket = None;
            if let Some(heartbeat) = state.heartbeat.take() {
                heartbeat.cancel();
            }
            let failed = self.fail_all(&mut state, &error);
            self.schedule_reconnect(&mut state);
            failed
        };
        self.events.on_state(false);
        Self::tell_failed(failed, &error);
    }

    /// Every phrase fails now; the exchanges are taken out, to be told outside the lock.
    fn fail_all(&self, state: &mut State, error: &str) -> Failed {
        for unsent in std::mem::take(&mut state.unsent) {
            unsent.phrase.fail(error);
        }
        let phrases: Vec<Arc<Phrase>> = state.by_message.values().cloned().collect();
        for phrase in phrases {
            self.drop_locked(state, &phrase, error);
        }
        let exchanges: Vec<Arc<Exchange>> = state.exchanges.drain().map(|(_, exchange)| exchange).collect();
        state.exchange_streams.clear();
        Failed { exchanges }
    }

    fn tell_failed(failed: Failed, error: &str) {
        for exchange in failed.exchanges {
            exchange.handler.on_closed(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_close_reason_our_schema_does_not_name_as_its_number_not_as_a_clean_end() {
        assert_eq!(close_reason(&json!({})), "CLOSE_OK");
        assert_eq!(close_reason(&json!({ "reason": null })), "CLOSE_OK");
        assert_eq!(close_reason(&json!({ "reason": "CLOSE_OK" })), "CLOSE_OK");
        assert_eq!(close_reason(&json!({ "reason": "CANCEL" })), "CANCEL");
        assert_eq!(close_reason(&json!({ "reason": 6 })), "6");
    }

    #[test]
    fn repeats_the_stations_headers_but_not_its_handshake() {
        let mut headers = HeaderMap::new();
        headers.insert("host", "192.168.1.10:8080".parse().unwrap());
        headers.insert("sec-websocket-key", "abc".parse().unwrap());
        headers.insert("sec-websocket-extensions", "permessage-deflate".parse().unwrap());
        headers.insert("x-uprx-oauth-token", "y0_secret".parse().unwrap());
        headers.append("x-uprx-app", "a".parse().unwrap());
        headers.append("x-uprx-app", "b".parse().unwrap());
        headers.insert("user-agent", "WebSocket++/0.8.2".parse().unwrap());
        let mut picked = upstream_headers(&headers);
        picked.sort();
        assert_eq!(
            picked,
            vec![
                ("user-agent".to_string(), "WebSocket++/0.8.2".to_string()),
                ("x-uprx-app".to_string(), "a, b".to_string()),
                ("x-uprx-oauth-token".to_string(), "y0_secret".to_string()),
            ]
        );
    }
}
