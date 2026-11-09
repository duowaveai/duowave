//! Our calls' transport: the WebSocket of the call page (control, WebRTC signalling) and of the simulator or a
//! station client (control, PCM), the station client's HTTP streams, and the bridges to SIP (Asterisk), LiveKit and
//! Telegram when they are configured.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::call_hub::{
    js_string, pcm_from_bytes, Call, CallEndpoint, CallError, CallHub, CallState, EndpointKind, HubEvent,
};
use super::http_stations::{HttpStations, ListenFormat, ListenStream, PollAnswer, GONE_MS, POLL_MS};
use super::livekit::{access_link, Invite};
use super::opus::CallsOpus;
use super::sip::AriBridge;
use super::socket_endpoint::SocketEndpoint;
use super::station_mic::{plain, StationMics};
use super::telegram::TelegramMessenger;
use crate::config::{CallsConfig, LiveKitConfig, SipConfig, TelegramConfig};
use crate::core::events::{EventBus, GatewayEvent};
use crate::operations::networks::{in_list, LOOPBACK};

/// What the panel sets up for the calls (src/connectors, through src/links): read at the start and again after each
/// change, so the bot and the bridges follow the panel without a restart.
pub trait CallsConnectors: Send + Sync {
    fn telegram(&self) -> Option<TelegramConfig>;
    fn sip(&self) -> Option<SipConfig>;
    fn livekit(&self) -> Option<LiveKitConfig>;
}

/// Nothing set up: tests.
pub struct NoCallsConnectors;

impl CallsConnectors for NoCallsConnectors {
    fn telegram(&self) -> Option<TelegramConfig> {
        None
    }

    fn sip(&self) -> Option<SipConfig> {
        None
    }

    fn livekit(&self) -> Option<LiveKitConfig> {
        None
    }
}

/// A part of the calls the panel sets up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallsPart {
    Telegram,
    Sip,
    LiveKit,
}

/// Where the call page, the simulator and a station client connect.
pub const CALLS_WS_PATH: &str = "/calls/ws";
/// How long a call's link lets a contact into the room: the call, and a call back the same day.
const INVITE_TTL: Duration = Duration::from_secs(24 * 3600);
/// Messages to one socket not sent yet; a client further behind loses the newest.
const SOCKET_QUEUE: usize = 1024;

/// What calls ask of the stations module (src/stations): whether an address is an approved station's (its call
/// client, its microphone bridge); any, with STATION_PAIRING=open. The stations service answers it (src/links).
pub trait StationTrust: Send + Sync {
    fn trusted_address(&self, address: &str) -> bool;
}

/// A socket's party once it said hello.
enum Joined {
    Socket(Arc<SocketEndpoint>),
    #[cfg(feature = "webrtc")]
    Browser(Arc<super::webrtc_endpoint::WebRtcEndpoint>),
}

impl Joined {
    fn endpoint(&self) -> Arc<dyn CallEndpoint> {
        match self {
            Joined::Socket(endpoint) => endpoint.clone(),
            #[cfg(feature = "webrtc")]
            Joined::Browser(endpoint) => endpoint.clone(),
        }
    }
}

pub struct CallsService {
    config: CallsConfig,
    /// The bot, SIP and LiveKit as the panel sets them up.
    connectors: Arc<dyn CallsConnectors>,
    pub hub: Arc<CallHub>,
    pub telegram: Arc<TelegramMessenger>,
    /// The station clients over HTTP; a station's microphone bridge comes from its client's address.
    pub stations: Arc<HttpStations>,
    opus: Arc<dyn CallsOpus>,
    trust: Arc<dyn StationTrust>,
    events: EventBus,
    sockets: Mutex<HashMap<u64, mpsc::Sender<Message>>>,
    next_socket: AtomicU64,
    browsers: AtomicU64,
    /// The SIP bridge now; another takes its place when the panel changes it.
    ari: Mutex<Option<Arc<AriBridge>>>,
    mics: Mutex<Option<Arc<StationMics>>>,
    /// LiveKit's settings now: tokens, the page, invitations.
    livekit: Mutex<Option<LiveKitConfig>>,
    #[cfg(feature = "livekit")]
    livekit_bridge: Mutex<Option<Arc<super::livekit::LiveKitBridge>>>,
    /// One change of the bot or a bridge at a time, each from the panel's latest settings.
    reconfiguring: tokio::sync::Mutex<()>,
    started: AtomicBool,
    stopping: CancellationToken,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn send(socket: &mpsc::Sender<Message>, message: Value) {
    let _ = socket.try_send(Message::Text(message.to_string().into()));
}

/// No Origin (a native client) or the page's own host: what a browser on our call page sends.
pub fn same_origin(origin: Option<&str>, host: Option<&str>) -> bool {
    let Some(origin) = origin else { return true };
    let Ok(url) = url::Url::parse(origin) else { return false };
    let Some(name) = url.host_str() else { return false };
    let authority = match url.port() {
        Some(port) => format!("{name}:{port}"),
        None => name.to_string(),
    };
    host.is_some_and(|host| host.eq_ignore_ascii_case(&authority))
}

impl CallsService {
    pub fn new(
        config: &CallsConfig,
        events: EventBus,
        trust: Arc<dyn StationTrust>,
        opus: Arc<dyn CallsOpus>,
        connectors: Arc<dyn CallsConnectors>,
    ) -> Arc<CallsService> {
        let hub = CallHub::new();
        let stations = HttpStations::new(hub.clone(), &config.station_name, opus.clone());
        let service = Arc::new(CallsService {
            config: config.clone(),
            hub: hub.clone(),
            // The bot as the panel has it now (its polling starts with `start`).
            telegram: TelegramMessenger::new(connectors.telegram()),
            livekit: Mutex::new(connectors.livekit()),
            connectors,
            stations,
            opus,
            trust,
            events,
            sockets: Mutex::default(),
            next_socket: AtomicU64::new(1),
            browsers: AtomicU64::new(0),
            ari: Mutex::new(None),
            mics: Mutex::new(None),
            #[cfg(feature = "livekit")]
            livekit_bridge: Mutex::new(None),
            reconfiguring: tokio::sync::Mutex::new(()),
            started: AtomicBool::new(false),
            stopping: CancellationToken::new(),
        });
        let weak = Arc::downgrade(&service);
        hub.on(move |event| {
            if let Some(service) = weak.upgrade() {
                service.on_hub(event);
            }
        });
        service
    }

    /// Each call's course in the log: live 02.11 two calls ended unseen, and only the station told how long it
    /// played. The station's microphone is on for the talk and off after.
    fn on_hub(&self, event: &HubEvent) {
        match event {
            HubEvent::Endpoints => {
                self.broadcast(json!({ "type": "endpoints", "endpoints": self.hub.list() }));
            }
            HubEvent::Ringing(call) => {
                tracing::info!("call {}: {} → {} ringing", call.id, call.from.id, call.to.id);
            }
            HubEvent::Connected(call) => {
                tracing::info!("call {}: answered", call.id);
                for party in [&call.from, &call.to] {
                    self.microphone(&party.id, true);
                }
            }
            HubEvent::Ended { call, reason } => {
                for party in [&call.from, &call.to] {
                    self.microphone(&party.id, false);
                }
                tracing::info!("call {}: ended, {reason}{}", call.id, ended_details(call));
            }
        }
    }

    /// The bridges the panel set up, the microphone port, the sweep of gone station clients, the bot.
    pub fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        for part in [CallsPart::Sip, CallsPart::LiveKit] {
            self.refresh(part);
        }
        if let Some(port) = self.config.station_mic_port {
            tokio::spawn(self.clone().start_mics(port));
        }
        let stations = self.stations.clone();
        let stopping = self.stopping.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(30));
            every.tick().await;
            loop {
                tokio::select! {
                    _ = stopping.cancelled() => return,
                    _ = every.tick() => stations.sweep(GONE_MS),
                }
            }
        });
        self.telegram.start();
        tracing::info!(
            "call page: http://<host>:<port>/calls; {CALLS_WS_PATH} for the page, the simulator and a station client"
        );
    }

    /// The panel changed a part: it follows, in a task of its own. Changes go one at a time, each from the panel's
    /// settings at its turn, so the last one wins whatever the order the tasks ran in.
    pub fn refresh(self: &Arc<Self>, part: CallsPart) {
        if self.stopping.is_cancelled() {
            return;
        }
        let service = self.clone();
        tokio::spawn(async move {
            let _one_at_a_time = service.reconfiguring.lock().await;
            if service.stopping.is_cancelled() {
                return;
            }
            match part {
                CallsPart::Telegram => service.telegram.reconfigure(service.connectors.telegram()).await,
                CallsPart::Sip => service.apply_sip(service.connectors.sip()).await,
                CallsPart::LiveKit => service.apply_livekit(service.connectors.livekit()).await,
            }
        });
    }

    /// The SIP bridge for these settings: the same bridge with new contacts, or the old one gone (its calls ended,
    /// its RTP port free) and a new one in its place.
    async fn apply_sip(&self, config: Option<SipConfig>) {
        let current = lock(&self.ari).clone();
        if let (Some(bridge), Some(config)) = (&current, &config) {
            if bridge.same_link(config) {
                bridge.set_contacts(config.contacts.clone());
                return;
            }
        }
        if let Some(old) = current {
            lock(&self.ari).take();
            old.shutdown().await;
            tracing::info!("SIP bridge stopped");
        }
        if let Some(config) = config {
            let ari = AriBridge::new(config.clone(), self.hub.clone());
            ari.start().await;
            *lock(&self.ari) = Some(ari);
            tracing::info!("SIP through Asterisk ARI at {}, app {}", config.url, config.app);
        }
    }

    /// LiveKit's settings for tokens, the page and invitations at once; the bridge in the room again only when its
    /// room or its access changed.
    async fn apply_livekit(&self, config: Option<LiveKitConfig>) {
        *lock(&self.livekit) = config.clone();
        self.bridge_livekit(config).await;
    }

    #[cfg(feature = "livekit")]
    async fn bridge_livekit(&self, config: Option<LiveKitConfig>) {
        let current = lock(&self.livekit_bridge).clone();
        if let (Some(bridge), Some(config)) = (&current, &config) {
            if bridge.same_link(config) {
                return;
            }
        }
        if let Some(old) = current {
            lock(&self.livekit_bridge).take();
            old.shutdown().await;
            tracing::info!("LiveKit bridge left the room");
        }
        if let Some(config) = config {
            let bridge =
                super::livekit::LiveKitBridge::new(config.clone(), self.hub.clone(), &self.config.station_name);
            bridge.start();
            *lock(&self.livekit_bridge) = Some(bridge);
            tracing::info!("LiveKit room {} at {}", config.room, config.url);
        }
    }

    #[cfg(not(feature = "livekit"))]
    async fn bridge_livekit(&self, config: Option<LiveKitConfig>) {
        if let Some(config) = config {
            tracing::warn!(
                "LiveKit bridge: not part of this build (the feature livekit); room {} at {} is not joined",
                config.room,
                config.url
            );
        }
    }

    /// Whether a call goes on now through a bridge (`sip:`, `livekit:`): changing its settings would end it.
    pub fn busy_through(&self, prefix: &str) -> bool {
        self.hub
            .list()
            .iter()
            .filter(|party| party.id.starts_with(prefix))
            .any(|party| self.hub.call_of(&party.id).is_some_and(|call| call.state != CallState::Ended))
    }

    /// The gateway stops: what would hold the HTTP server's graceful close ends now — the station clients' polls
    /// (up to 50 s each) and streams, the calls' sockets — and the bridges leave.
    pub fn stop(&self) {
        self.stations.stop();
        self.stopping.cancel();
        if let Some(ari) = lock(&self.ari).as_ref() {
            ari.stop();
        }
        #[cfg(feature = "livekit")]
        if let Some(livekit) = lock(&self.livekit_bridge).as_ref() {
            livekit.stop();
        }
        if let Some(mics) = lock(&self.mics).take() {
            mics.close();
        }
        self.telegram.stop();
    }

    async fn start_mics(self: Arc<Self>, port: u16) {
        let (audio, came, went) = (Arc::downgrade(&self), Arc::downgrade(&self), Arc::downgrade(&self));
        let mics = StationMics::new(
            move |address, pcm| {
                let Some(service) = audio.upgrade() else { return };
                if let Some(device) = service.stations.device_at(address) {
                    service.hub.audio(&format!("station:{device}"), pcm);
                }
            },
            move |address| {
                if let Some(service) = came.upgrade() {
                    service.bridge_came(address);
                }
            },
            move |address| {
                if let Some(service) = went.upgrade() {
                    service.channel(address, "microphone", false);
                }
            },
        );
        *lock(&self.mics) = Some(mics.clone());
        match mics.listen(port, "0.0.0.0").await {
            Ok(port) => tracing::info!("station microphone bridges on port {port}"),
            Err(error) => {
                tracing::warn!("station microphone port {port}: {error}");
                lock(&self.mics).take();
            }
        }
    }

    /// A bridge came: its station may be in a call already.
    fn bridge_came(&self, address: &str) {
        self.channel(address, "microphone", true);
        let Some(device) = self.stations.device_at(address) else { return };
        let active = self.hub.call_of(&format!("station:{device}")).is_some_and(|call| call.state == CallState::Active);
        if active {
            if let Some(mics) = lock(&self.mics).clone() {
                mics.subscribe(address);
            }
        }
    }

    fn channel(&self, address: &str, channel: &str, up: bool) {
        self.events.emit(GatewayEvent::StationChannel { address: address.to_string(), channel: channel.into(), up });
    }

    /// A station party's microphone, on for its call's talk and off after.
    fn microphone(&self, party: &str, on: bool) {
        let Some(device) = party.strip_prefix("station:") else { return };
        let Some(address) = self.stations.address(device) else { return };
        let Some(mics) = lock(&self.mics).clone() else { return };
        if on {
            mics.subscribe(&address);
        } else {
            mics.unsubscribe(&address);
        }
    }

    /// LiveKit's address and room for a token, when the panel set it up.
    pub fn livekit_config(&self) -> Option<LiveKitConfig> {
        lock(&self.livekit).clone()
    }

    /// How the station is named in our calls («Колонка»).
    pub fn station_name(&self) -> &str {
        &self.config.station_name
    }

    /// Whether the station can call someone outside the room: the LiveKit page has an address from outside, and
    /// the bridge that puts the station in the room is part of this build.
    pub fn can_invite(&self) -> bool {
        cfg!(feature = "livekit") && lock(&self.livekit).as_ref().is_some_and(|config| config.page_url.is_some())
    }

    /// A link to the LiveKit page with a day's access, for a contact the station calls («позвони маме»).
    pub fn invite(&self, name: &str) -> Option<Invite> {
        access_link(&self.livekit_config()?, name, INVITE_TTL)
    }

    /// A SIP contact's number, when the SIP bridge is set up.
    pub fn sip_contact(&self, name: &str) -> Option<String> {
        lock(&self.ari).clone()?.contact(name)
    }

    /// Dials a SIP contact through Asterisk; the phone joins the hub as a party.
    pub async fn dial_sip(&self, number: &str, name: &str) -> anyhow::Result<Arc<dyn CallEndpoint>> {
        let Some(ari) = lock(&self.ari).clone() else { anyhow::bail!("no SIP bridge: set it up in the panel") };
        ari.dial(number, name).await
    }

    /// Whether an address is an approved station's: the station client's routes answer only it.
    pub fn trusted_address(&self, address: &str) -> bool {
        self.trust.trusted_address(address)
    }

    /// The station client's audio from its chunked POST.
    pub fn talk(&self, device: &str, pcm: &[i16]) {
        self.hub.audio(&format!("station:{device}"), pcm);
    }

    /// The station client's poll, logged when it first comes. Each poll says the station is there (its presence),
    /// whether or not a call comes.
    pub async fn next(&self, device: &str, address: Option<&str>) -> PollAnswer {
        if self.stations.address(device).is_none() {
            tracing::info!("station client {device} is polling");
        }
        if let Some(address) = address {
            self.channel(plain(address), "calls", true);
        }
        self.stations.next(device, address, Duration::from_millis(POLL_MS)).await
    }

    /// The station client's stream for its call.
    pub fn listen(&self, device: &str, format: ListenFormat, address: Option<&str>) -> ListenStream {
        tracing::info!("station client {device} listens to its call ({})", format.as_str());
        self.stations.listen(device, format, address)
    }

    pub fn release(&self, device: &str) {
        tracing::info!("station client {device} stops");
        let address = self.stations.address(device);
        self.stations.release(device);
        if let Some(address) = address {
            self.channel(&address, "calls", false);
        }
    }

    fn broadcast(&self, message: Value) {
        let text = message.to_string();
        for socket in lock(&self.sockets).values() {
            let _ = socket.try_send(Message::Text(text.clone().into()));
        }
    }

    /// A socket of `/calls/ws` from `address`, until it closes or the gateway stops.
    pub async fn on_socket(self: Arc<Self>, socket: WebSocket, address: String) {
        let (mut sink, mut incoming) = socket.split();
        let (sender, mut outgoing) = mpsc::channel::<Message>(SOCKET_QUEUE);
        let id = self.next_socket.fetch_add(1, Ordering::Relaxed);
        lock(&self.sockets).insert(id, sender.clone());
        let writer = tokio::spawn(async move {
            while let Some(message) = outgoing.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        });
        let mut joined: Option<Joined> = None;
        loop {
            let message = tokio::select! {
                _ = self.stopping.cancelled() => break,
                message = incoming.next() => message,
            };
            match message {
                Some(Ok(Message::Binary(bytes))) => {
                    if let Some(Joined::Socket(endpoint)) = &joined {
                        self.hub.audio(endpoint.id(), &pcm_from_bytes(&bytes));
                    }
                }
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<Value>(text.as_str()) {
                    Ok(message) => {
                        if let Err(error) = self.on_control(&message, &mut joined, &sender, &address) {
                            send(&sender, json!({ "type": "error", "message": error.0 }));
                        }
                    }
                    Err(_) => send(&sender, json!({ "type": "error", "message": "not JSON" })),
                },
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            }
        }
        lock(&self.sockets).remove(&id);
        if let Some(joined) = joined {
            // Only while it is still the party under its id: a client that came back on a new socket stays.
            let ours = joined.endpoint();
            let current = self.hub.endpoint(ours.id());
            if current.is_some_and(|current| std::ptr::addr_eq(Arc::as_ptr(&current), Arc::as_ptr(&ours))) {
                self.hub.unregister(ours.id(), "left");
            }
            #[cfg(feature = "webrtc")]
            if let Joined::Browser(browser) = joined {
                browser.close().await;
            }
        }
        // A call's record keeps its parties (and their queue) a while: the socket closes now all the same.
        writer.abort();
    }

    fn on_control(
        &self,
        message: &Value,
        joined: &mut Option<Joined>,
        socket: &mpsc::Sender<Message>,
        address: &str,
    ) -> Result<(), CallError> {
        let kind = message.get("type");
        if kind.and_then(Value::as_str) == Some("hello") {
            if joined.is_some() {
                return Err(CallError("already introduced".into()));
            }
            let role = message.get("role").and_then(Value::as_str);
            let given = |field: &str| message.get(field).filter(|value| !value.is_null());
            // Who may stand in for the station: an approved station from its address; the simulator from this
            // computer.
            if role == Some("station") && !self.trust.trusted_address(address) {
                return Err(CallError("only an approved station".into()));
            }
            if role == Some("sim") && !in_list(address, LOOPBACK) {
                return Err(CallError("the simulator runs on this computer".into()));
            }
            let created = if role == Some("station") || role == Some("sim") {
                let device = given("device").map_or_else(|| "default".to_string(), |device| js_string(Some(device)));
                let name = given("name").map_or_else(|| self.config.station_name.clone(), |name| js_string(Some(name)));
                let endpoint =
                    SocketEndpoint::new(format!("station:{device}"), EndpointKind::Station, name, socket.clone());
                Joined::Socket(Arc::new(endpoint))
            } else {
                let name = given("name").map(|name| js_string(Some(name))).unwrap_or_default().trim().to_string();
                self.browser(if name.is_empty() { "Без имени".to_string() } else { name }, socket)?
            };
            let endpoint = created.endpoint();
            self.hub.register(endpoint.clone());
            send(
                socket,
                json!({ "type": "welcome", "id": endpoint.id(), "name": endpoint.name(), "endpoints": self.hub.list() }),
            );
            let kind = serde_json::to_value(endpoint.kind()).ok().and_then(|kind| kind.as_str().map(str::to_string));
            tracing::info!("{} {} ({}) joined", kind.unwrap_or_default(), endpoint.name(), endpoint.id());
            *joined = Some(created);
            return Ok(());
        }
        let Some(party) = joined.as_ref() else { return Err(CallError("say hello first".into())) };
        let id = party.endpoint().id().to_string();
        match kind.and_then(Value::as_str) {
            Some("call") => self.hub.call(&id, &js_string(message.get("to"))).map(drop),
            Some("accept") => self.hub.accept(&js_string(message.get("callId")), &id).map(drop),
            Some("hangup") => self.hub.hangup(&js_string(message.get("callId")), &id),
            Some("offer") => self.offer(party, &js_string(message.get("sdp")), socket),
            _ => Err(CallError(format!("unknown message {}", js_string(kind)))),
        }
    }

    #[cfg(feature = "webrtc")]
    fn browser(&self, name: String, socket: &mpsc::Sender<Message>) -> Result<Joined, CallError> {
        let id = format!("browser:{}", self.browsers.fetch_add(1, Ordering::Relaxed) + 1);
        let hub = Arc::downgrade(&self.hub);
        let audio_id = id.clone();
        let events = socket.clone();
        let endpoint = super::webrtc_endpoint::WebRtcEndpoint::new(
            id,
            name,
            self.opus.clone(),
            Box::new(move |pcm: &[i16]| {
                if let Some(hub) = hub.upgrade() {
                    hub.audio(&audio_id, pcm);
                }
            }),
            Box::new(move |event| {
                if let Ok(text) = serde_json::to_string(event) {
                    let _ = events.try_send(Message::Text(text.into()));
                }
            }),
        )
        .map_err(|error| CallError(format!("{error:#}")))?;
        Ok(Joined::Browser(Arc::new(endpoint)))
    }

    /// Without WebRTC a browser cannot carry a call's sound: it is not let in as a party.
    #[cfg(not(feature = "webrtc"))]
    fn browser(&self, _name: String, _socket: &mpsc::Sender<Message>) -> Result<Joined, CallError> {
        let _ = (&self.opus, &self.browsers);
        Err(CallError("the call page's WebRTC is not part of this build of the gateway".into()))
    }

    /// The page's offer: the answer comes on the socket when ICE has gathered (up to 3 s); meanwhile the socket
    /// goes on.
    #[cfg(feature = "webrtc")]
    fn offer(&self, party: &Joined, sdp: &str, socket: &mpsc::Sender<Message>) -> Result<(), CallError> {
        let Joined::Browser(browser) = party else { return Err(CallError("only the call page sends offers".into())) };
        let (browser, sdp, socket) = (browser.clone(), sdp.to_string(), socket.clone());
        tokio::spawn(async move {
            match browser.answer(&sdp).await {
                Ok(answer) => send(&socket, json!({ "type": "answer", "sdp": answer })),
                Err(error) => send(&socket, json!({ "type": "error", "message": format!("{error:#}") })),
            }
        });
        Ok(())
    }

    #[cfg(not(feature = "webrtc"))]
    fn offer(&self, _party: &Joined, _sdp: &str, _socket: &mpsc::Sender<Message>) -> Result<(), CallError> {
        Err(CallError("only the call page sends offers".into()))
    }
}

/// `, talked 12.3s, sound 11.9s from browser:1, 12.1s from station:default` for an answered call; who hung up.
fn ended_details(call: &Call) -> String {
    let by = call.ended_by.as_ref().map(|by| format!(" by {by}")).unwrap_or_default();
    let Some(answered) = call.answered_at else { return by };
    let seconds = |samples: u64| format!("{:.1}", samples as f64 / 16000.0);
    let talked = crate::utils::time::now_ms().saturating_sub(answered) as f64 / 1000.0;
    format!(
        "{by}, talked {talked:.1}s, sound {}s from {}, {}s from {}",
        seconds(call.sent.from),
        call.from.id,
        seconds(call.sent.to),
        call.to.id
    )
}
