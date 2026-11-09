//! Station clients over plain HTTP (station-client/): a client polls `next` and is a party of our calls while it
//! does; once its call is answered it opens `listen` for that call alone, and the stream ends with the call — so
//! the station's own GStreamer holds the shared speaker only while someone talks, and stops by the end of its
//! stream, not by a kill (live 02.11: a pipeline holding the speaker between calls, then killed, left the station
//! silent for everyone until a reboot).

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures::Stream;
use tokio::sync::{mpsc, oneshot};

use super::call_hub::{pcm_to_bytes, Call, CallEndpoint, CallHub, CallState, HubEvent};
use super::opus::CallsOpus;
use super::socket_endpoint::{FrameListener, HttpStationEndpoint};
use super::station_mic::plain;
use crate::utils::time::now_ms;

/// How long a client's poll waits for its call before it is answered «idle» and asks again.
pub const POLL_MS: u64 = 50_000;
/// A station whose client has not polled for this long is no longer a party.
pub const GONE_MS: u64 = 120_000;
/// 200 ms of 16 kHz silence that starts the station's Ogg stream.
const PRIME_SAMPLES: usize = 3200;
/// Chunks of a stream not sent yet: ~5 s of 20 ms pages; a client further behind loses the newest.
const STREAM_BACKLOG: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollAnswer {
    Call,
    Idle,
}

impl PollAnswer {
    pub fn as_str(self) -> &'static str {
        match self {
            PollAnswer::Call => "call",
            PollAnswer::Idle => "idle",
        }
    }
}

/// The station's GStreamer has no raw audio parser (read 02.11): it takes Ogg/Opus; other clients take PCM16LE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenFormat {
    Pcm,
    Ogg,
}

impl ListenFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ListenFormat::Pcm => "pcm",
            ListenFormat::Ogg => "ogg",
        }
    }
}

/// An open stream of a call: ending it ends the HTTP response.
struct OpenStream {
    id: u64,
    endpoint: Arc<HttpStationEndpoint>,
    sender: Mutex<Option<mpsc::Sender<Bytes>>>,
}

impl OpenStream {
    /// The station's pipeline plays what it has and closes the speaker by itself. The listener goes too, even one
    /// attached after the stream ended.
    fn end(&self) {
        lock(&self.sender).take();
        self.endpoint.detach_listener(self.id);
    }

    fn send(&self, chunk: Vec<u8>) {
        if let Some(sender) = lock(&self.sender).as_ref() {
            let _ = sender.try_send(Bytes::from(chunk));
        }
    }
}

struct Station {
    endpoint: Arc<HttpStationEndpoint>,
    address: Option<String>,
    seen_at: u64,
    /// The open stream of the call, ended when the call ends.
    stream: Option<Arc<OpenStream>>,
}

struct Waiting {
    device: String,
    answer: oneshot::Sender<PollAnswer>,
}

#[derive(Default)]
struct Inner {
    /// In the order they came (the TS Map's): `device_at` takes the first.
    stations: Vec<(String, Station)>,
    /// Every waiting poll: its call wakes it, the gateway stopping answers it «idle».
    polls: HashMap<u64, Waiting>,
}

impl Inner {
    fn station(&mut self, device: &str) -> Option<&mut Station> {
        self.stations.iter_mut().find(|(known, _)| known == device).map(|(_, station)| station)
    }
}

pub struct HttpStations {
    hub: Arc<CallHub>,
    name: String,
    opus: Arc<dyn CallsOpus>,
    now: Box<dyn Fn() -> u64 + Send + Sync>,
    inner: Mutex<Inner>,
    next: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The device of a station party: `station:<device>`.
fn device_of(party: &str) -> Option<&str> {
    party.strip_prefix("station:")
}

impl HttpStations {
    pub fn new(hub: Arc<CallHub>, name: &str, opus: Arc<dyn CallsOpus>) -> Arc<HttpStations> {
        HttpStations::with_clock(hub, name, opus, now_ms)
    }

    /// With its own clock: tests.
    pub fn with_clock(
        hub: Arc<CallHub>,
        name: &str,
        opus: Arc<dyn CallsOpus>,
        now: impl Fn() -> u64 + Send + Sync + 'static,
    ) -> Arc<HttpStations> {
        let stations = Arc::new(HttpStations {
            hub: hub.clone(),
            name: name.to_string(),
            opus,
            now: Box::new(now),
            inner: Mutex::default(),
            next: AtomicU64::new(1),
        });
        let weak = Arc::downgrade(&stations);
        hub.on(move |event| {
            let Some(stations) = weak.upgrade() else { return };
            match event {
                HubEvent::Connected(call) => stations.wake(call),
                HubEvent::Ended { call, .. } => stations.end_streams(call),
                _ => {}
            }
        });
        stations
    }

    /// The call is on: its station's polls answer «call».
    fn wake(&self, call: &Call) {
        let mut inner = lock(&self.inner);
        for party in [&call.from, &call.to] {
            let Some(device) = device_of(&party.id) else { continue };
            if inner.station(device).is_none() {
                continue;
            }
            let woken: Vec<u64> =
                inner.polls.iter().filter(|(_, waiting)| waiting.device == device).map(|(id, _)| *id).collect();
            for id in woken {
                if let Some(waiting) = inner.polls.remove(&id) {
                    let _ = waiting.answer.send(PollAnswer::Call);
                }
            }
        }
    }

    /// The call is over: its station's stream ends.
    fn end_streams(&self, call: &Call) {
        let streams: Vec<Arc<OpenStream>> = {
            let mut inner = lock(&self.inner);
            [&call.from, &call.to]
                .into_iter()
                .filter_map(|party| device_of(&party.id))
                .filter_map(|device| inner.station(device).and_then(|station| station.stream.take()))
                .collect()
        };
        for stream in streams {
            stream.end();
        }
    }

    /// A client's poll: the station is a party; `call` once its call is on, `idle` after `wait` (or when the
    /// gateway stops). A poll dropped half-way (the client went) leaves nothing behind.
    pub async fn next(&self, device: &str, address: Option<&str>, wait: Duration) -> PollAnswer {
        self.present(device, address);
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (answer, answered) = oneshot::channel();
        lock(&self.inner).polls.insert(id, Waiting { device: device.to_string(), answer });
        let _forget = Forget { stations: self, id };
        // Waiting first, then the look: a call answered in between is not missed.
        if self.active_call(device) {
            return PollAnswer::Call;
        }
        tokio::select! {
            answer = answered => answer.unwrap_or(PollAnswer::Idle),
            _ = tokio::time::sleep(wait) => PollAnswer::Idle,
        }
    }

    /// The other party's audio for the station's call: PCM, or Ogg/Opus for the station's GStreamer. Ends with the
    /// call; without a call on, at once. Dropping the stream (the client went) leaves the call.
    pub fn listen(self: &Arc<Self>, device: &str, format: ListenFormat, address: Option<&str>) -> ListenStream {
        let endpoint = self.present(device, address);
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel(STREAM_BACKLOG);
        let open = Arc::new(OpenStream { id, endpoint: endpoint.clone(), sender: Mutex::new(Some(sender)) });
        let stream =
            ListenStream { receiver, open: open.clone(), stations: Arc::downgrade(self), device: device.to_string() };
        let mut ogg = None;
        if format == ListenFormat::Ogg {
            // A moment of silence after the headers: the station names the stream and opens its speaker now, not on
            // the first words.
            let primed = self.opus.ogg_stream().and_then(|mut ogg| {
                let mut chunk = ogg.headers()?;
                chunk.extend(ogg.pages(&[0; PRIME_SAMPLES])?);
                Ok((ogg, chunk))
            });
            match primed {
                Ok((stream, chunk)) => {
                    open.send(chunk);
                    ogg = Some(stream);
                }
                Err(error) => {
                    tracing::warn!("station client {device}: no Ogg/Opus stream: {error:#}");
                    open.end();
                    return stream;
                }
            }
        }
        if !self.active_call(device) {
            open.end();
            return stream;
        }
        let sink = open.clone();
        self.attach(
            device,
            &open,
            Box::new(move |frame: &[i16]| {
                let chunk = match ogg.as_mut() {
                    Some(ogg) => match ogg.pages(frame) {
                        Ok(pages) => pages,
                        Err(error) => {
                            tracing::warn!("call stream: {error:#}");
                            return;
                        }
                    },
                    None => pcm_to_bytes(frame),
                };
                if !chunk.is_empty() {
                    sink.send(chunk);
                }
            }),
        );
        stream
    }

    /// The stream becomes its station's and takes the call's frames. The steps before ran apart from the hub: a
    /// call that ended meanwhile (its `ended` found no stream to end) or a station that left ends it here; the TS
    /// ran them without a break.
    fn attach(&self, device: &str, open: &Arc<OpenStream>, listener: FrameListener) {
        let previous = {
            let mut inner = lock(&self.inner);
            match inner.station(device) {
                Some(station) if Arc::ptr_eq(&station.endpoint, &open.endpoint) => station.stream.replace(open.clone()),
                _ => {
                    drop(inner);
                    open.end();
                    return;
                }
            }
        };
        if let Some(previous) = previous {
            previous.end();
        }
        open.endpoint.attach_listener(open.id, listener);
        // Recorded first, then the look: a call that ends from now on finds this stream and ends it.
        if !self.active_call(device) {
            self.forget_stream(device, open);
            open.end();
        }
    }

    /// The client stops: its stream ends (the pipeline closes the speaker by itself) and the station leaves.
    pub fn release(&self, device: &str) {
        let station = {
            let mut inner = lock(&self.inner);
            let Some(at) = inner.stations.iter().position(|(known, _)| known == device) else { return };
            inner.stations.remove(at).1
        };
        if let Some(stream) = station.stream {
            stream.end();
        }
        self.hub.unregister(station.endpoint.id(), "the station client left");
    }

    /// The gateway stops: every waiting poll is answered «idle» now and every stream ends. A poll waits up to 50 s,
    /// and the HTTP server waits for it before it closes (live 05.11: Ctrl+C took 48 s); a call's stream would hold
    /// it for the whole call.
    pub fn stop(&self) {
        let (polls, streams): (Vec<Waiting>, Vec<Arc<OpenStream>>) = {
            let mut inner = lock(&self.inner);
            let polls = inner.polls.drain().map(|(_, waiting)| waiting).collect();
            let streams = inner.stations.iter_mut().filter_map(|(_, station)| station.stream.take()).collect();
            (polls, streams)
        };
        for waiting in polls {
            let _ = waiting.answer.send(PollAnswer::Idle);
        }
        for stream in streams {
            stream.end();
        }
    }

    /// Stations whose client stopped polling are no longer parties.
    pub fn sweep(&self, gone_ms: u64) {
        let now = (self.now)();
        let gone: Vec<String> = lock(&self.inner)
            .stations
            .iter()
            .filter(|(_, station)| station.stream.is_none() && now.saturating_sub(station.seen_at) > gone_ms)
            .map(|(device, _)| device.clone())
            .collect();
        for device in gone {
            self.release(&device);
        }
    }

    pub fn address(&self, device: &str) -> Option<String> {
        lock(&self.inner).station(device).and_then(|station| station.address.clone())
    }

    pub fn device_at(&self, address: &str) -> Option<String> {
        let wanted = plain(address);
        lock(&self.inner)
            .stations
            .iter()
            .find(|(_, station)| station.address.as_deref() == Some(wanted))
            .map(|(device, _)| device.clone())
    }

    /// The station's party, registered anew when the hub has another under its id (the simulator took it).
    fn present(&self, device: &str, address: Option<&str>) -> Arc<HttpStationEndpoint> {
        let id = format!("station:{device}");
        let current = self.hub.endpoint(&id);
        let now = (self.now)();
        let (endpoint, fresh) = {
            let mut inner = lock(&self.inner);
            let position = inner.stations.iter().position(|(known, _)| known == device);
            let known = position.is_some_and(|at| {
                current.as_ref().is_some_and(|current| {
                    let ours: Arc<dyn CallEndpoint> = inner.stations[at].1.endpoint.clone();
                    std::ptr::addr_eq(Arc::as_ptr(current), Arc::as_ptr(&ours))
                })
            });
            let fresh = || Station {
                endpoint: Arc::new(HttpStationEndpoint::new(id.clone(), self.name.clone())),
                address: None,
                seen_at: now,
                stream: None,
            };
            let at = match position {
                Some(at) if known => at,
                Some(at) => {
                    inner.stations[at].1 = fresh();
                    at
                }
                None => {
                    inner.stations.push((device.to_string(), fresh()));
                    inner.stations.len() - 1
                }
            };
            let station = &mut inner.stations[at].1;
            station.seen_at = now;
            if let Some(address) = address {
                station.address = Some(plain(address).to_string());
            }
            (station.endpoint.clone(), !known)
        };
        // Outside the lock: a replaced party's calls end, and their listeners come back here.
        if fresh {
            self.hub.register(endpoint.clone());
        }
        endpoint
    }

    fn active_call(&self, device: &str) -> bool {
        self.hub.call_of(&format!("station:{device}")).is_some_and(|call| call.state == CallState::Active)
    }

    fn forget_stream(&self, device: &str, open: &Arc<OpenStream>) {
        if let Some(station) = lock(&self.inner).station(device) {
            if station.stream.as_ref().is_some_and(|stream| Arc::ptr_eq(stream, open)) {
                station.stream = None;
            }
        }
    }
}

/// Removes a poll from the waiting ones, however it ended.
struct Forget<'a> {
    stations: &'a HttpStations,
    id: u64,
}

impl Drop for Forget<'_> {
    fn drop(&mut self) {
        lock(&self.stations.inner).polls.remove(&self.id);
    }
}

/// A call's audio as the HTTP response's body.
pub struct ListenStream {
    receiver: mpsc::Receiver<Bytes>,
    open: Arc<OpenStream>,
    stations: Weak<HttpStations>,
    device: String,
}

impl Stream for ListenStream {
    type Item = Bytes;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Bytes>> {
        self.receiver.poll_recv(context)
    }
}

impl Drop for ListenStream {
    fn drop(&mut self) {
        if let Some(stations) = self.stations.upgrade() {
            stations.forget_stream(&self.device, &self.open);
        }
        self.open.end();
    }
}

#[cfg(test)]
mod tests {
    use futures::FutureExt;
    use futures::StreamExt;

    use super::*;
    use crate::calls::call_hub::tests::TestParty;
    use crate::calls::call_hub::{CallEventType, EndpointKind};
    use crate::calls::opus::SpeechOpus;

    fn stations(hub: &Arc<CallHub>) -> Arc<HttpStations> {
        HttpStations::new(hub.clone(), "Колонка", Arc::new(SpeechOpus))
    }

    /// What the stream has now, and whether it ended.
    fn drain(stream: &mut ListenStream) -> (Vec<Bytes>, bool) {
        let mut chunks = Vec::new();
        loop {
            match stream.next().now_or_never() {
                Some(Some(chunk)) => chunks.push(chunk),
                Some(None) => return (chunks, true),
                None => return (chunks, false),
            }
        }
    }

    #[tokio::test]
    async fn answers_a_poll_call_once_the_call_is_answered_streams_it_and_ends_the_stream_with_the_call() {
        let hub = CallHub::new();
        let stations = stations(&hub);
        let phone = TestParty::new("livekit:app-mom", "Мама", EndpointKind::Livekit);
        hub.register(phone.clone());

        let poll = {
            let stations = stations.clone();
            tokio::spawn(
                async move { stations.next("default", Some("::ffff:192.0.2.23"), Duration::from_secs(10)).await },
            )
        };
        tokio::task::yield_now().await;
        while hub.endpoint("station:default").is_none() {
            tokio::task::yield_now().await;
        }
        assert_eq!(stations.device_at("192.0.2.23").as_deref(), Some("default"));
        let call = hub.call(&phone.id, "station:default").unwrap();
        hub.accept(&call.id, "station:default").unwrap();
        assert_eq!(poll.await.unwrap(), PollAnswer::Call);

        let mut stream = stations.listen("default", ListenFormat::Pcm, None);
        hub.audio(&phone.id, &[7; 320]);
        let (chunks, ended) = drain(&mut stream);
        assert_eq!(chunks.len(), 1);
        assert!(!ended);
        hub.hangup(&call.id, &phone.id).unwrap();
        assert!(drain(&mut stream).1, "the stream ends with the call");
        // The station stays a party between calls: it keeps polling.
        assert!(hub.endpoint("station:default").is_some());
    }

    #[tokio::test]
    async fn ends_a_stream_at_once_without_a_call_answers_idle_after_the_wait_and_lets_a_stopped_client_go() {
        let now = Arc::new(AtomicU64::new(0));
        let clock = now.clone();
        let hub = CallHub::new();
        let stations = HttpStations::with_clock(hub.clone(), "Колонка", Arc::new(SpeechOpus), move || {
            clock.load(Ordering::Relaxed)
        });
        let mut stream = stations.listen("default", ListenFormat::Ogg, None);
        let (chunks, ended) = drain(&mut stream);
        assert!(ended);
        assert_eq!(&chunks[0][..4], b"OggS", "the headers and a moment of silence came first");

        let started = std::time::Instant::now();
        assert_eq!(stations.next("default", None, Duration::from_millis(100)).await, PollAnswer::Idle);
        assert!(started.elapsed() >= Duration::from_millis(100));

        now.store(200_000, Ordering::Relaxed);
        stations.sweep(120_000);
        assert!(hub.endpoint("station:default").is_none());
    }

    #[tokio::test]
    async fn answers_every_waiting_poll_idle_at_once_when_the_gateway_stops_they_held_its_shutdown_for_50_s() {
        let stations = stations(&CallHub::new());
        let polls = [("default", "192.0.2.23"), ("kids", "192.0.2.10")].map(|(device, address)| {
            let stations = stations.clone();
            tokio::spawn(async move { stations.next(device, Some(address), Duration::from_secs(50)).await })
        });
        while lock(&stations.inner).polls.len() < 2 {
            tokio::task::yield_now().await;
        }
        stations.stop();
        for poll in polls {
            assert_eq!(poll.await.unwrap(), PollAnswer::Idle);
        }
        // A poll that ended by itself is not answered twice.
        stations.stop();
    }

    #[tokio::test]
    async fn stops_a_calls_stream_and_the_station_when_its_client_stops() {
        let hub = CallHub::new();
        let stations = stations(&hub);
        let phone = TestParty::new("livekit:app-mom", "Мама", EndpointKind::Livekit);
        hub.register(phone.clone());
        assert_eq!(stations.next("default", None, Duration::from_millis(10)).await, PollAnswer::Idle);
        let call = hub.call(&phone.id, "station:default").unwrap();
        hub.accept(&call.id, "station:default").unwrap();
        let mut stream = stations.listen("default", ListenFormat::Pcm, None);
        stations.release("default");
        assert!(drain(&mut stream).1);
        assert_eq!(phone.last().kind, CallEventType::Ended);
        assert_eq!(phone.last().reason.as_deref(), Some("the station client left"));
        assert!(hub.endpoint("station:default").is_none());
    }

    #[tokio::test]
    async fn a_new_stream_ends_the_one_before_and_a_dropped_stream_leaves_its_call() {
        let hub = CallHub::new();
        let stations = stations(&hub);
        let phone = TestParty::new("livekit:app-mom", "Мама", EndpointKind::Livekit);
        hub.register(phone.clone());
        let _ = stations.next("default", None, Duration::from_millis(1)).await;
        let call = hub.call(&phone.id, "station:default").unwrap();
        hub.accept(&call.id, "station:default").unwrap();
        let mut first = stations.listen("default", ListenFormat::Ogg, None);
        let second = stations.listen("default", ListenFormat::Pcm, None);
        assert!(drain(&mut first).1, "one stream per station");
        let endpoint = lock(&stations.inner).station("default").unwrap().endpoint.clone();
        assert!(endpoint.listening());
        drop(second);
        assert!(!endpoint.listening());
        assert!(lock(&stations.inner).station("default").unwrap().stream.is_none());
        assert_eq!(hub.call_of("station:default").map(|call| call.state), Some(CallState::Active));
    }

    /// A stream as `listen` makes it, up to its attach.
    fn open_stream(stations: &Arc<HttpStations>, device: &str) -> (Arc<OpenStream>, mpsc::Receiver<Bytes>) {
        let endpoint = lock(&stations.inner).station(device).unwrap().endpoint.clone();
        let (sender, receiver) = mpsc::channel(4);
        let id = stations.next.fetch_add(1, Ordering::Relaxed);
        (Arc::new(OpenStream { id, endpoint, sender: Mutex::new(Some(sender)) }), receiver)
    }

    #[tokio::test]
    async fn ends_a_stream_whose_call_ended_or_station_left_while_it_was_set_up() {
        let hub = CallHub::new();
        let stations = stations(&hub);
        let phone = TestParty::new("livekit:app-mom", "Мама", EndpointKind::Livekit);
        hub.register(phone.clone());
        let _ = stations.next("default", None, Duration::from_millis(1)).await;
        let call = hub.call(&phone.id, "station:default").unwrap();
        hub.accept(&call.id, "station:default").unwrap();

        // `listen` saw the call on; the phone hangs up before the stream is the station's.
        let (open, mut receiver) = open_stream(&stations, "default");
        hub.hangup(&call.id, &phone.id).unwrap();
        stations.attach("default", &open, Box::new(|_: &[i16]| {}));
        assert_eq!(ended(&mut receiver).await, Some(true), "the stream ends: nothing else would end it");
        assert!(!open.endpoint.listening());
        assert!(lock(&stations.inner).station("default").unwrap().stream.is_none(), "the sweep may take the station");

        // The client stopped while its stream was set up: the stream is nobody's, so it ends now.
        let call = hub.call(&phone.id, "station:default").unwrap();
        hub.accept(&call.id, "station:default").unwrap();
        let (open, mut receiver) = open_stream(&stations, "default");
        stations.release("default");
        stations.attach("default", &open, Box::new(|_: &[i16]| {}));
        assert_eq!(ended(&mut receiver).await, Some(true));
        assert!(!open.endpoint.listening());
    }

    /// Whether the stream ended within a second (None: it is still open).
    async fn ended(receiver: &mut mpsc::Receiver<Bytes>) -> Option<bool> {
        tokio::time::timeout(Duration::from_secs(1), receiver.recv()).await.ok().map(|chunk| chunk.is_none())
    }
}
