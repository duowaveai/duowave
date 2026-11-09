//! Relay mode, to study the original Alice: the station's uniproxy session goes to Yandex unchanged, with the
//! station's own handshake headers, and every frame both ways is journaled: decoded by our schema plus a raw view of
//! all its fields (field numbers for what the schema lacks), and the audio of each stream — the station's voice,
//! Alice's speech — as a file. Tokens never reach the journal; the raw view of SynchronizeState, which carries them,
//! is not written at all. Nothing is answered here.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::ws::WebSocket;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use regex::Regex;
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as UpstreamMessage;
use tokio_util::sync::CancellationToken;

use super::device_tag::device_tag_of;
use super::relay_trips::{recognition_text_raw, RelayTrips};
use super::station_session::{DeviceView, SessionSnapshot};
use super::summaries::summarize_client;
use super::uniproxy_server::{pump, Handshake, Liveness, Outbox, Outgoing, SocketEnd};
use crate::config::GatewayConfig;
use crate::core::events::{EventBus, GatewayEvent};
use crate::core::requests::RequestStarter;
use crate::gateway::observability::redact::hash_id;
use crate::gateway::observability::session_recorder::{
    clock_tag, AudioSink, EventDirection, SessionEventRecord, SessionJournal, SessionRecorder,
};
use crate::gateway::protocol::aapi_codec::AapiCodec;
use crate::gateway::protocol::ids::new_session_id;
use crate::gateway::protocol::proto_object::value_bytes;
use crate::gateway::protocol::protocol_types::{ClientEvent, ClientInfo, ClientMessage, StreamAction};
use crate::gateway::protocol::raw_proto::describe_raw;
use crate::gateway::yandex::yandex_session::{close_reason, connect_upstream, upstream_headers, ConnectError};
use crate::utils::time::now_iso;

const HEADERS_TO_RECORD: [&str; 3] = ["user-agent", "sec-websocket-extensions", "sec-websocket-protocol"];
// Handshake headers whose values must never reach the journal, and those that identify the device or account.
const SECRET_HEADERS: [&str; 2] = ["x-uprx-oauth-token", "x-uprx-auth-token"];
const IDENTIFIER_HEADERS: [&str; 3] = ["x-uprx-device-id", "x-uprx-uuid", "x-uprx-uid"];
static OAUTH_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"y[0-9]_[A-Za-z0-9_-]{20,}").expect("a valid pattern"));
// Close codes a WebSocket endpoint may not send itself.
const RESERVED_CLOSE_CODES: [u16; 4] = [1004, 1005, 1006, 1015];
/// How long the opening handshake with Yandex may take.
const HANDSHAKE_MS: u64 = 10_000;
/// How long Yandex has to answer our close before the connection is cut.
const UPSTREAM_CLOSE_MS: u64 = 5_000;

/// What the session needs from the gateway.
pub struct RelayDeps {
    pub codec: Arc<AapiCodec>,
    pub config: Arc<GatewayConfig>,
    /// The station said who it is (synchronizeState): its model and colour, for its record and look.
    pub events: EventBus,
    /// Where requests are followed as they go (src/requests): read off the frames here, nothing is answered.
    pub requests: Option<Arc<dyn RequestStarter>>,
}

/// A handshake header's value (a repeated one joined, as Node gave it).
pub fn header(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    let values: Vec<&str> = headers.get_all(name).iter().filter_map(|value| value.to_str().ok()).collect();
    (!values.is_empty()).then(|| values.join(", "))
}

/// The handshake headers the journal keeps: the station's own (x-uprx-…) and the WebSocket's.
pub fn recorded_headers(headers: &axum::http::HeaderMap) -> Value {
    let mut picked = Map::new();
    for name in headers.keys() {
        let name = name.as_str();
        if name.starts_with("x-uprx-") || HEADERS_TO_RECORD.contains(&name) {
            if let Some(value) = header(headers, name) {
                picked.insert(name.to_string(), Value::String(value));
            }
        }
    }
    Value::Object(picked)
}

/// An id's first eight characters, `-` for none.
fn short(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) if !text.is_empty() => text.chars().take(8).collect(),
        Some(Value::Null) | None => "-".into(),
        Some(Value::String(_)) => "-".into(),
        Some(other) => other.to_string().chars().take(8).collect(),
    }
}

/// Bytes become raw protobuf views here, so the secrets scrub sees their strings too.
fn raw_views(value: Value) -> Value {
    if let Some(bytes) = value_bytes(&value) {
        return json!({ "raw": describe_raw(&bytes) });
    }
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(raw_views).collect()),
        Value::Object(object) => {
            Value::Object(object.into_iter().map(|(key, child)| (key, raw_views(child))).collect())
        }
        other => other,
    }
}

/// One line for a frame from Yandex: what kind of event and its key fields.
fn describe_server(codec: &AapiCodec, decoded: &Value) -> (String, String) {
    if let Some(control) = decoded.get("streamControl").filter(|control| !control.is_null()) {
        let action = if let Some(close) = control.get("close").filter(|close| !close.is_null()) {
            format!("close:{}", close_reason(close))
        } else if control.get("spotterEnd").is_some_and(|end| !end.is_null()) {
            "spotterEnd".into()
        } else {
            "other".into()
        };
        let stream = control.get("streamId").map_or_else(|| "undefined".into(), Value::to_string);
        return ("streamControl".into(), format!("stream={stream} {action}"));
    }
    let empty = Value::Object(Map::new());
    let event = decoded.get("event").unwrap_or(&empty);
    let kind = codec.server_event_kind(decoded).unwrap_or("unknown").to_string();
    let r#ref = format!("ref={}", short(event.get("header").and_then(|header| header.get("refMessageId"))));
    let summary = match kind.as_str() {
        "asrResult" => {
            let asr = event.get("asrResult");
            let end = asr.and_then(|asr| asr.get("endOfUtt")).and_then(Value::as_bool).unwrap_or(false);
            format!("{ref} endOfUtt={end} \"{}\"", recognition_text_raw(asr))
        }
        "aliceResponse" => {
            let response = event.get("aliceResponse").unwrap_or(&empty);
            let names: Vec<&str> = response
                .get("response")
                .and_then(|body| body.get("directives"))
                .and_then(Value::as_array)
                .map(|directives| directives.iter().filter_map(|d| d.get("name").and_then(Value::as_str)).collect())
                .unwrap_or_default();
            let directives = if names.is_empty() { "-".to_string() } else { names.join(",") };
            let voice = response.get("voiceResponse");
            let speech = voice
                .and_then(|voice| voice.get("outputSpeech"))
                .and_then(|speech| speech.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let listen = voice.and_then(|voice| voice.get("shouldListen")).and_then(Value::as_bool).unwrap_or(false);
            format!("{ref} speech=\"{speech}\" listen={listen} directives={directives}")
        }
        "ttsSpeak" => {
            let stream = event
                .get("header")
                .and_then(|header| header.get("streamId"))
                .map_or_else(|| "undefined".into(), Value::to_string);
            let format =
                event.get("ttsSpeak").and_then(|speak| speak.get("format")).and_then(Value::as_str).unwrap_or("");
            format!("{ref} stream={stream} {format}")
        }
        _ => r#ref,
    };
    (kind, summary)
}

struct RelayedStream {
    sink: AudioSink,
    bytes: usize,
}

/// Our connection to Yandex: opening (the station's frames wait, in order), open, or gone.
enum Upstream {
    Connecting(Vec<(Bytes, bool)>),
    Open(mpsc::UnboundedSender<UpstreamMessage>),
    Closed,
}

struct Inner {
    closed_at: Option<String>,
    client_info: Option<ClientInfo>,
    upstream: Upstream,
    streams: HashMap<String, RelayedStream>,
    stream_labels: HashMap<String, String>,
    /// The requests' trips, read off the frames as they pass.
    trips: RelayTrips,
    /// Yandex answered a ping or sent a frame since the last ping.
    yandex_heard: bool,
}

pub struct RelaySession {
    pub id: String,
    pub connected_at: String,
    pub device_tag: String,
    pub remote_address: String,
    deps: RelayDeps,
    outbox: Arc<Outbox>,
    secrets: Vec<String>,
    /// Cuts the connection to Yandex (the heartbeat found it dead, the station left while it opened).
    upstream_cut: CancellationToken,
    /// Stops the pings: the station left.
    heartbeat_stop: CancellationToken,
    /// A leaf lock (see SessionJournal): anything may journal under its own lock.
    journal: SessionJournal,
    inner: Mutex<Inner>,
}

impl RelaySession {
    /// The session for a station that just connected, and its socket's queue.
    pub fn new(handshake: &Handshake, deps: RelayDeps) -> (Arc<RelaySession>, mpsc::UnboundedReceiver<Outgoing>) {
        let redact = deps.config.recorder.redact;
        let id = new_session_id();
        let connected_at = now_iso();
        let device_tag = device_tag_of(&handshake.headers, redact);
        let recorder = SessionRecorder::new(&deps.config.recorder, &id, &device_tag, &connected_at);
        let mut names: Vec<&str> = SECRET_HEADERS.to_vec();
        if redact {
            names.extend(IDENTIFIER_HEADERS);
        }
        let secrets = names
            .into_iter()
            .filter_map(|name| header(&handshake.headers, name))
            .filter(|value| value.chars().count() >= 6)
            .collect();
        let (outbox, outgoing) = Outbox::new();
        let trips = RelayTrips::new(deps.requests.clone(), &device_tag, &id);
        let journal = SessionJournal::new(recorder, deps.config.session_events_limit);
        let session = Arc::new(RelaySession {
            id,
            connected_at,
            device_tag,
            remote_address: if handshake.remote_address.is_empty() {
                "?".into()
            } else {
                handshake.remote_address.clone()
            },
            deps,
            outbox,
            secrets,
            upstream_cut: CancellationToken::new(),
            heartbeat_stop: CancellationToken::new(),
            journal,
            inner: Mutex::new(Inner {
                closed_at: None,
                client_info: None,
                upstream: Upstream::Connecting(Vec::new()),
                streams: HashMap::new(),
                stream_labels: HashMap::new(),
                trips,
                yandex_heard: true,
            }),
        });
        (session, outgoing)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The station's socket, relayed until it closes; then the session's end, and how the socket ended. The
    /// connection to Yandex opens at once, with the station's handshake headers. Our pings to Yandex
    /// (RELAY_PING_MS) say nothing of the station: its socket has pings of its own (STATION_PING_MS), as in
    /// gateway mode — Yandex's pings stop at our client and never reach the station either.
    pub async fn run(
        self: &Arc<Self>,
        socket: WebSocket,
        outgoing: mpsc::UnboundedReceiver<Outgoing>,
        handshake: Handshake,
        stopping: CancellationToken,
    ) -> SocketEnd {
        self.note(
            "connected",
            &format!(
                "relay to {}: from {} device={}",
                self.deps.config.yandex_url, self.remote_address, self.device_tag
            ),
            Some(json!({ "headers": recorded_headers(&handshake.headers) })),
        );
        let upstream = self.clone();
        let headers = upstream_headers(&handshake.headers);
        tokio::spawn(async move { upstream.relay_upstream(headers).await });
        let me = self.clone();
        let liveness = Liveness::every(self.deps.config.station_ping_ms);
        let end =
            pump(socket, outgoing, stopping, liveness, move |frame, is_binary| me.on_station_frame(frame, is_binary))
                .await;
        if let Some(error) = &end.error {
            self.note("socketError", error, None);
        }
        self.outbox.closed();
        self.on_close(end.code, &end.reason);
        end
    }

    /// Closes the station's socket (a mode switch): it reconnects into the new mode.
    pub fn close_socket(&self, code: u16, reason: &str) {
        self.outbox.close(code, reason);
    }

    pub fn closed_at(&self) -> Option<String> {
        self.lock().closed_at.clone()
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        let inner = self.lock();
        SessionSnapshot {
            id: self.id.clone(),
            mode: Some("relay"),
            device_tag: self.device_tag.clone(),
            remote_address: self.remote_address.clone(),
            connected_at: self.connected_at.clone(),
            closed_at: inner.closed_at.clone(),
            wire: Some("aapi"),
            device: inner.client_info.as_ref().map(DeviceView::of),
            active_voice_requests: 0,
            can_speak: false,
            yandex_connected: None,
            events: self.journal.events(),
            journal: self.journal.dir_display(),
        }
    }

    pub fn recent_events(&self, limit: usize) -> Vec<SessionEventRecord> {
        self.journal.recent(limit)
    }

    // ---------------------------------------------------------------- the connection to Yandex

    async fn relay_upstream(self: Arc<Self>, headers: Vec<(String, String)>) {
        let url = self.deps.config.yandex_url.clone();
        let connected = tokio::select! {
            _ = self.upstream_cut.cancelled() => Err(ConnectError::Failed("WebSocket was closed before the connection was established".into())),
            connected = tokio::time::timeout(Duration::from_millis(HANDSHAKE_MS), connect_upstream(&url, &headers)) => {
                connected.unwrap_or_else(|_| Err(ConnectError::Failed("Opening handshake has timed out".into())))
            }
        };
        let socket = match connected {
            Ok(socket) => socket,
            Err(error) => {
                match error {
                    ConnectError::Rejected(status) => {
                        self.note("yandexRejected", &format!("HTTP {status} on the upgrade"), None)
                    }
                    ConnectError::Failed(message) => self.note("yandexError", &message, None),
                }
                return self.upstream_closed(1006, "");
            }
        };
        let (mut sink, mut stream) = socket.split();
        let (sender, mut outgoing) = mpsc::unbounded_channel::<UpstreamMessage>();
        tokio::spawn(async move {
            while let Some(message) = outgoing.recv().await {
                let closing = matches!(message, UpstreamMessage::Close(_));
                if sink.send(message).await.is_err() || closing {
                    break;
                }
            }
        });
        {
            let mut inner = self.lock();
            let pending = match std::mem::replace(&mut inner.upstream, Upstream::Open(sender.clone())) {
                Upstream::Connecting(pending) => pending,
                // The station left while we were connecting: close at once.
                other => {
                    inner.upstream = other;
                    let _ = sender.send(UpstreamMessage::Close(None));
                    return;
                }
            };
            self.note("yandexConnected", &format!("{} station frame(s) were waiting", pending.len()), None);
            for (frame, is_binary) in pending {
                let _ = sender.send(upstream_message(frame, is_binary));
            }
        }
        self.start_heartbeat(sender);
        let (code, reason) = loop {
            let next = tokio::select! {
                _ = self.upstream_cut.cancelled() => break (1006, String::new()),
                next = stream.next() => next,
            };
            match next {
                Some(Ok(UpstreamMessage::Binary(frame))) => self.on_yandex_frame(frame, true),
                Some(Ok(UpstreamMessage::Text(text))) => {
                    self.on_yandex_frame(Bytes::copy_from_slice(text.as_bytes()), false)
                }
                Some(Ok(UpstreamMessage::Pong(_))) => self.lock().yandex_heard = true,
                Some(Ok(UpstreamMessage::Close(frame))) => {
                    break frame
                        .map_or((1005, String::new()), |frame| (u16::from(frame.code), frame.reason.to_string()))
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => {
                    self.note("yandexError", &error.to_string(), None);
                    break (1006, String::new());
                }
                None => break (1006, String::new()),
            }
        };
        self.upstream_closed(code, &reason);
    }

    /// The connection to Yandex is gone: the station's goes too, with Yandex's code where it may be sent.
    fn upstream_closed(&self, code: u16, reason: &str) {
        self.upstream_cut.cancel();
        self.lock().upstream = Upstream::Closed;
        let reason_part = if reason.is_empty() { String::new() } else { format!(" reason={reason}") };
        self.note("yandexClosed", &format!("code={code}{reason_part}"), None);
        if self.outbox.is_open() {
            self.outbox.close(if RESERVED_CLOSE_CODES.contains(&code) { 1011 } else { code }, "");
        }
    }

    /// The station's pings are answered by our socket itself, so on a quiet session nothing reaches Yandex and a
    /// dead connection goes unnoticed: on 31.10 one stayed silent for 42 idle minutes, then swallowed the next
    /// request. We ping Yandex ourselves; a connection with no answer by the next ping is cut, and the station
    /// reconnects.
    fn start_heartbeat(self: &Arc<Self>, sender: mpsc::UnboundedSender<UpstreamMessage>) {
        let interval_ms = self.deps.config.relay_ping_ms;
        if interval_ms == 0 {
            return;
        }
        let me = Arc::downgrade(self);
        let (cut, stop) = (self.upstream_cut.clone(), self.heartbeat_stop.clone());
        tokio::spawn(async move {
            let period = Duration::from_millis(interval_ms);
            let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            loop {
                tokio::select! {
                    _ = cut.cancelled() => return,
                    _ = stop.cancelled() => return,
                    _ = ticks.tick() => {}
                }
                let Some(session) = me.upgrade() else { return };
                let heard = std::mem::replace(&mut session.lock().yandex_heard, false);
                if !heard {
                    session.note("yandexSilent", &format!("no answer from Yandex in {interval_ms} ms"), None);
                    session.upstream_cut.cancel();
                    return;
                }
                let _ = sender.send(UpstreamMessage::Ping(Bytes::new()));
            }
        });
    }

    // ---------------------------------------------------------------- frames

    fn on_station_frame(&self, frame: Bytes, is_binary: bool) {
        {
            let mut inner = self.lock();
            match &mut inner.upstream {
                Upstream::Open(sender) => {
                    let _ = sender.send(upstream_message(frame.clone(), is_binary));
                }
                Upstream::Connecting(pending) => pending.push((frame.clone(), is_binary)),
                Upstream::Closed => {}
            }
        }
        self.observe(EventDirection::In, &frame, is_binary);
    }

    fn on_yandex_frame(&self, frame: Bytes, is_binary: bool) {
        self.lock().yandex_heard = true;
        if is_binary {
            self.outbox.send_binary(frame.clone());
        } else {
            self.outbox.send_text(String::from_utf8_lossy(&frame).into_owned());
        }
        self.observe(EventDirection::Out, &frame, is_binary);
    }

    /// Journals a frame without ever stopping it: a frame we cannot read still went through.
    fn observe(&self, dir: EventDirection, frame: &Bytes, is_binary: bool) {
        let observed = if !is_binary {
            let text = String::from_utf8_lossy(frame).into_owned();
            let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
            self.record(dir, "json", None, Some(parsed));
            Ok(())
        } else if !AapiCodec::is_aapi_frame(frame) {
            self.observe_audio(dir, frame);
            Ok(())
        } else if dir == EventDirection::In {
            self.observe_station_event(frame)
        } else {
            self.observe_yandex_event(frame)
        };
        if let Err(error) = observed {
            let side = if dir == EventDirection::In { "in" } else { "out" };
            self.note("frameError", &format!("{side}: {error}"), None);
        }
    }

    fn observe_station_event(&self, frame: &Bytes) -> anyhow::Result<()> {
        let decoded = self.deps.codec.decode_client(frame).map_err(|error| anyhow::anyhow!("{error}"))?;
        let message = &decoded.message;
        let summary = summarize_client(message);
        if let ClientMessage::Event(ClientEvent::SynchronizeState { application, .. }) = message {
            self.lock().client_info = application.clone();
            self.deps.events.emit(GatewayEvent::StationSynced {
                device_tag: self.device_tag.clone(),
                device_model: application.as_ref().and_then(|app| app.device_model.clone()),
                device_color: application.as_ref().and_then(|app| app.device_color.clone()),
                firmware: application.as_ref().and_then(|app| app.firmware_version.clone()),
            });
            // Its raw view would carry the OAuth token and the API key: the decoded part has no such fields.
            self.record(EventDirection::In, "synchronizeState", Some(summary), Some(decoded.decoded));
            return Ok(());
        }
        let (kind, capabilities) = {
            let mut inner = self.lock();
            match message {
                ClientMessage::StreamControl(control) => {
                    if control.action == StreamAction::Close {
                        self.close_stream(&mut inner, control.stream_id);
                    }
                }
                ClientMessage::Event(event) => {
                    if let Some(stream_id) = event.header().stream_id {
                        let label = if matches!(event, ClientEvent::LogSpotter { .. }) { "spotter" } else { "voice" };
                        inner.stream_labels.insert(format!("in:{stream_id}"), label.into());
                    }
                    // The station's firmware is said in its spotter's log alone: kept with its record.
                    if let ClientEvent::LogSpotter { firmware: Some(firmware), .. } = event {
                        self.deps.events.emit(GatewayEvent::StationSynced {
                            device_tag: self.device_tag.clone(),
                            device_model: None,
                            device_color: None,
                            firmware: Some(firmware.clone()),
                        });
                    }
                }
            }
            inner.trips.from_station(message);
            let kind = match message {
                ClientMessage::StreamControl(_) => "streamControl",
                ClientMessage::Event(event) => event.kind(),
            };
            // Capability states whole (alarms, timers, …): the raw view cuts long bytes.
            let capabilities = match message {
                ClientMessage::Event(ClientEvent::VoiceInput { .. } | ClientEvent::TextInput { .. }) => {
                    self.deps.codec.capability_states(frame)
                }
                _ => Vec::new(),
            };
            (kind, capabilities)
        };
        let mut data = json!({ "decoded": decoded.decoded, "raw": describe_raw(&frame[4..]) });
        if !capabilities.is_empty() {
            data["capabilities"] = serde_json::to_value(capabilities)?;
        }
        self.record(EventDirection::In, kind, Some(summary), Some(data));
        Ok(())
    }

    fn observe_yandex_event(&self, frame: &Bytes) -> anyhow::Result<()> {
        let decoded = self.deps.codec.decode_server_frame(frame).map_err(|error| anyhow::anyhow!("{error}"))?;
        let (kind, summary) = describe_server(&self.deps.codec, &decoded);
        {
            let mut inner = self.lock();
            let stream_id = decoded
                .get("event")
                .and_then(|event| event.get("header"))
                .and_then(|header| header.get("streamId"))
                .and_then(Value::as_u64);
            if let (true, Some(stream_id)) = (kind == "ttsSpeak", stream_id) {
                inner.stream_labels.insert(format!("out:{stream_id}"), "tts".into());
            }
            if kind == "streamControl" {
                let control = decoded.get("streamControl");
                if control.and_then(|control| control.get("close")).is_some_and(|close| !close.is_null()) {
                    let stream_id = control.and_then(|control| control.get("streamId")).and_then(Value::as_u64);
                    self.close_stream(&mut inner, stream_id.unwrap_or(0) as u32);
                }
            }
            inner.trips.from_yandex(&kind, &decoded);
        }
        // Typed commands whole (a timer, an alarm, an animation in field 15): the raw view cuts long bytes.
        let typed: Vec<Value> = decoded
            .get("event")
            .and_then(|event| event.get("aliceResponse"))
            .and_then(|response| response.get("response"))
            .and_then(|body| body.get("directives"))
            .and_then(Value::as_array)
            .map(|directives| {
                directives
                    .iter()
                    .filter_map(|directive| {
                        let payload =
                            directive.get("typedPayload").and_then(value_bytes).filter(|bytes| !bytes.is_empty())?;
                        let mut entry = Map::new();
                        for key in ["name", "analyticsType"] {
                            if let Some(value) = directive.get(key) {
                                entry.insert(key.into(), value.clone());
                            }
                        }
                        entry.insert("base64".into(), json!(B64.encode(payload)));
                        Some(Value::Object(entry))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut data = json!({ "decoded": decoded, "raw": describe_raw(&frame[4..]) });
        if !typed.is_empty() {
            data["typedPayloads"] = Value::Array(typed);
        }
        self.record(EventDirection::Out, &kind, Some(summary), Some(data));
        Ok(())
    }

    /// Audio frames are counted and written to a file per stream and direction, not journaled one by one.
    fn observe_audio(&self, dir: EventDirection, frame: &Bytes) {
        if frame.len() < 4 {
            return;
        }
        let stream_id = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]);
        let payload = &frame[4..];
        let side = if dir == EventDirection::In { "in" } else { "out" };
        let key = format!("{side}:{stream_id}");
        let mut inner = self.lock();
        if !inner.streams.contains_key(&key) {
            let label = inner
                .stream_labels
                .get(&key)
                .cloned()
                .unwrap_or_else(|| if dir == EventDirection::In { "station" } else { "yandex" }.to_string());
            let ext = if payload.starts_with(b"OggS") { "ogg" } else { "pcm" };
            let name = format!("{}_{label}_{stream_id}.{ext}", clock_tag());
            let sink = self.journal.open_audio(&name);
            inner.streams.insert(key.clone(), RelayedStream { sink, bytes: 0 });
        }
        if let Some(stream) = inner.streams.get_mut(&key) {
            stream.bytes += payload.len();
            stream.sink.write(payload);
        }
    }

    fn close_stream(&self, inner: &mut Inner, stream_id: u32) {
        for (side, who) in [("in", "station"), ("out", "yandex")] {
            let Some(mut stream) = inner.streams.remove(&format!("{side}:{stream_id}")) else { continue };
            stream.sink.close();
            let file = stream.sink.file.as_ref().map(|file| file.to_string_lossy().into_owned());
            self.note(
                "audio",
                &format!("{who} stream {stream_id}: {} bytes", stream.bytes),
                Some(json!({ "file": file, "bytes": stream.bytes })),
            );
        }
    }

    // ---------------------------------------------------------------- lifecycle and journal

    fn on_close(&self, code: u16, reason: &str) {
        self.heartbeat_stop.cancel();
        {
            let mut inner = self.lock();
            match std::mem::replace(&mut inner.upstream, Upstream::Closed) {
                // A close without a code, as ws's close() sent; Yandex's reply ends the connection.
                Upstream::Open(sender) => {
                    let _ = sender.send(UpstreamMessage::Close(None));
                    let cut = self.upstream_cut.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(UPSTREAM_CLOSE_MS)).await;
                        cut.cancel();
                    });
                }
                // Still opening: the handshake is abandoned.
                Upstream::Connecting(_) => self.upstream_cut.cancel(),
                Upstream::Closed => {}
            }
            inner.trips.close();
            let mut ids: Vec<u32> =
                inner.streams.keys().filter_map(|key| key.split(':').nth(1)?.parse().ok()).collect();
            ids.sort_unstable();
            ids.dedup();
            for stream_id in ids {
                self.close_stream(&mut inner, stream_id);
            }
            inner.closed_at = Some(now_iso());
        }
        let reason = if reason.is_empty() { String::new() } else { format!(" reason={reason}") };
        self.note("disconnected", &format!("code={code}{reason}"), None);
        self.journal.close();
    }

    fn note(&self, kind: &str, summary: &str, data: Option<Value>) {
        tracing::info!(target: "relay", "Relay {} {kind}: {summary}", &self.id[..8]);
        self.record(EventDirection::Note, kind, Some(summary.to_string()), data);
    }

    fn record(&self, dir: EventDirection, kind: &str, summary: Option<String>, data: Option<Value>) {
        let summary = summary.filter(|summary| !summary.is_empty()).map(|summary| self.scrub_text(&summary));
        let data = data.map(|data| self.scrub(raw_views(data)));
        if dir != EventDirection::Note {
            let arrow = if dir == EventDirection::In { "station →" } else { "→ station" };
            tracing::debug!(target: "relay", "Relay {} {arrow} {kind}{}", &self.id[..8], summary.as_deref().map(|s| format!(" {s}")).unwrap_or_default());
        }
        self.journal.record(dir, kind, summary, data.as_ref());
    }

    /// Secrets from the handshake and anything shaped like an OAuth token become hashes, wherever they occur.
    fn scrub(&self, value: Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.scrub_text(&text)),
            Value::Array(items) => Value::Array(items.into_iter().map(|item| self.scrub(item)).collect()),
            Value::Object(object) => {
                Value::Object(object.into_iter().map(|(key, child)| (key, self.scrub(child))).collect())
            }
            other => other,
        }
    }

    fn scrub_text(&self, text: &str) -> String {
        let mut scrubbed = text.to_string();
        for secret in &self.secrets {
            if scrubbed.contains(secret.as_str()) {
                scrubbed = scrubbed.replace(secret.as_str(), &hash_id(secret));
            }
        }
        OAUTH_TOKEN.replace_all(&scrubbed, |found: &regex::Captures| hash_id(&found[0])).into_owned()
    }
}

fn upstream_message(frame: Bytes, is_binary: bool) -> UpstreamMessage {
    if is_binary {
        UpstreamMessage::Binary(frame)
    } else {
        UpstreamMessage::Text(String::from_utf8_lossy(&frame).into_owned().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_yandex_frames_in_a_line() {
        let codec = AapiCodec::new();
        let close = codec
            .encode_server(&crate::gateway::protocol::protocol_types::ServerMessage::StreamClose {
                stream_id: 2,
                reason: crate::gateway::protocol::protocol_types::CloseReason::Ok,
            })
            .unwrap();
        let decoded = codec.decode_server_frame(&close.frame).unwrap();
        let (kind, summary) = describe_server(&codec, &decoded);
        assert_eq!(kind, "streamControl");
        assert!(summary.starts_with("stream=2 close:"), "{summary}");
        let asr = codec
            .encode_server(&crate::gateway::protocol::protocol_types::ServerMessage::AsrResult {
                r#ref: "voice-123456789".into(),
                text: "который час".into(),
                end_of_utt: true,
            })
            .unwrap();
        let decoded = codec.decode_server_frame(&asr.frame).unwrap();
        assert_eq!(describe_server(&codec, &decoded).1, "ref=voice-12 endOfUtt=true \"который час\"");
    }

    #[test]
    fn turns_bytes_into_raw_views_for_the_scrub() {
        use crate::gateway::protocol::proto_object::bytes_value;
        let viewed = raw_views(json!({ "push": bytes_value(&[0x0a, 0x02, 0x69, 0x64]), "list": [bytes_value(b"")] }));
        assert_eq!(viewed["push"]["raw"][0]["value"], "id");
        assert_eq!(viewed["list"][0], json!({ "raw": [] }));
    }
}
