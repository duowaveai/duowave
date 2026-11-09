//! Our calls through a LiveKit server, an optional bridge like SIP: the gateway joins the family room as the
//! station, every other participant becomes a party of our hub, and the hub's calls are carried by the room —
//! TURN, mobile SDKs and calls from outside the home come with LiveKit. Signalling is LiveKitCalls'. LiveKit's
//! Rust SDK brings a prebuilt libwebrtc: this file is the feature `livekit`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::StreamExt;
use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::livekit_calls::{LiveKitCalls, RoomLink, LIVEKIT_TOPIC};
use super::token::{room_token, BRIDGE_IDENTITY};
use crate::calls::call_hub::{CallHub, FRAME_SAMPLES};
use crate::config::LiveKitConfig;

const SAMPLE_RATE: u32 = 16_000;
/// The station's voice waiting for the room: a second of frames, then the newest are dropped.
const PLAY_BACKLOG: usize = 50;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A data message for a participant, through the room it was sent in.
struct Outgoing {
    room: Arc<Room>,
    identity: String,
    message: Value,
}

/// The room as LiveKitCalls uses it: data messages through the room we are in, audio in order to our track.
struct Link {
    room: Mutex<Option<Arc<Room>>>,
    audio: mpsc::Sender<Vec<i16>>,
    /// Data messages in the order the hub's events came: a task per message let a later one overtake (the page
    /// hid an `incoming` under the `ended` before it), where rtc-node queued publishData in call order.
    data: mpsc::UnboundedSender<Outgoing>,
}

impl RoomLink for Link {
    fn send(&self, identity: &str, message: Value) {
        let Some(room) = lock(&self.room).clone() else { return };
        let _ = self.data.send(Outgoing { room, identity: identity.to_string(), message });
    }

    fn play(&self, pcm: &[i16]) {
        let _ = self.audio.try_send(pcm.to_vec());
    }
}

pub struct LiveKitBridge {
    config: LiveKitConfig,
    station_name: String,
    calls: Arc<LiveKitCalls>,
    link: Arc<Link>,
    source: NativeAudioSource,
    played: Mutex<Option<mpsc::Receiver<Vec<i16>>>>,
    sent: Mutex<Option<mpsc::UnboundedReceiver<Outgoing>>>,
    /// Its tasks: `shutdown` waits for them, so two bridges are never in the room as the station at once.
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    stop: CancellationToken,
}

impl LiveKitBridge {
    pub fn new(config: LiveKitConfig, hub: Arc<CallHub>, station_name: &str) -> Arc<LiveKitBridge> {
        let (audio, played) = mpsc::channel(PLAY_BACKLOG);
        let (data, sent) = mpsc::unbounded_channel();
        let link = Arc::new(Link { room: Mutex::new(None), audio, data });
        let calls = Arc::new(LiveKitCalls::new(hub, link.clone()));
        let source = NativeAudioSource::new(AudioSourceOptions::default(), SAMPLE_RATE, 1, 200);
        Arc::new(LiveKitBridge {
            config,
            station_name: station_name.to_string(),
            calls,
            link,
            source,
            played: Mutex::new(Some(played)),
            sent: Mutex::new(Some(sent)),
            tasks: Mutex::default(),
            stop: CancellationToken::new(),
        })
    }

    pub fn start(self: &Arc<Self>) {
        let mut tasks = lock(&self.tasks);
        if let Some(played) = lock(&self.played).take() {
            tasks.push(tokio::spawn(self.clone().play(played)));
        }
        if let Some(sent) = lock(&self.sent).take() {
            tasks.push(tokio::spawn(self.clone().deliver(sent)));
        }
        tasks.push(tokio::spawn(self.clone().run()));
    }

    /// Whether these settings are this bridge's room: the addresses phones use and the call page aside.
    pub fn same_link(&self, other: &LiveKitConfig) -> bool {
        self.config.url == other.url
            && self.config.api_key == other.api_key
            && self.config.api_secret == other.api_secret
            && self.config.room == other.room
    }

    /// Leaves the room for good, the panel's settings changed: its tasks end (a join that hangs is cut after 5 s),
    /// its participants leave the hub and their calls end.
    pub async fn shutdown(&self) {
        self.stop.cancel();
        let tasks: Vec<tokio::task::JoinHandle<()>> = lock(&self.tasks).drain(..).collect();
        for mut task in tasks {
            if tokio::time::timeout(Duration::from_secs(5), &mut task).await.is_err() {
                task.abort();
                let _ = task.await;
            }
        }
        *lock(&self.link.room) = None;
        self.calls.leave_all("the LiveKit bridge was set up again");
    }

    /// The data messages into the room, each published before the next one goes.
    async fn deliver(self: Arc<Self>, mut sent: mpsc::UnboundedReceiver<Outgoing>) {
        loop {
            let Outgoing { room, identity, message } = tokio::select! {
                _ = self.stop.cancelled() => return,
                next = sent.recv() => match next { Some(next) => next, None => return },
            };
            let packet = DataPacket {
                payload: message.to_string().into_bytes(),
                topic: Some(LIVEKIT_TOPIC.to_string()),
                reliable: true,
                destination_identities: vec![ParticipantIdentity::from(identity.as_str())],
            };
            if let Err(error) = room.local_participant().publish_data(packet).await {
                tracing::warn!("to {identity}: {error}");
            }
        }
    }

    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// The station's voice into the room, frame by frame in order.
    async fn play(self: Arc<Self>, mut played: mpsc::Receiver<Vec<i16>>) {
        loop {
            let pcm = tokio::select! {
                _ = self.stop.cancelled() => return,
                pcm = played.recv() => match pcm { Some(pcm) => pcm, None => return },
            };
            let samples = if pcm.is_empty() { FRAME_SAMPLES } else { pcm.len() };
            let frame = AudioFrame {
                data: Cow::Owned(pcm),
                sample_rate: SAMPLE_RATE,
                num_channels: 1,
                samples_per_channel: samples as u32,
            };
            let _ = self.source.capture_frame(&frame).await;
        }
    }

    /// In the room until stopped: a lost room is joined again in 5 s, one that would not let us in in 10 s.
    async fn run(self: Arc<Self>) {
        loop {
            let wait = match self.session().await {
                Ok(()) => Duration::from_secs(5),
                Err(error) => {
                    tracing::warn!("cannot join {}: {error}", self.config.url);
                    Duration::from_secs(10)
                }
            };
            *lock(&self.link.room) = None;
            if self.stop.is_cancelled() {
                return;
            }
            tokio::select! {
                _ = self.stop.cancelled() => return,
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }

    async fn session(&self) -> RoomResult<()> {
        let token = room_token(&self.config, BRIDGE_IDENTITY, &self.station_name, Duration::from_secs(24 * 3600));
        let mut options = RoomOptions::default();
        options.auto_subscribe = true;
        options.dynacast = false;
        let (room, mut events) = Room::connect(&self.config.url, &token, options).await?;
        let room = Arc::new(room);
        let track = LocalAudioTrack::create_audio_track("station", RtcAudioSource::Native(self.source.clone()));
        let publish = TrackPublishOptions { source: TrackSource::Microphone, ..Default::default() };
        room.local_participant().publish_track(LocalTrack::Audio(track), publish).await?;
        *lock(&self.link.room) = Some(room.clone());
        for participant in room.remote_participants().values() {
            self.calls.joined(
                participant.identity().as_str(),
                Some(&participant.name()),
                Some(participant.sid().as_str()),
            );
        }
        tracing::info!("in room {} at {} as «{}»", self.config.room, self.config.url, self.station_name);
        // The participants' microphones, by track, with whose they are. A reader's stream ends only when it is
        // closed: its track's unsubscription, its participant leaving and the end of the session end it (rtc-node's
        // AudioStream ended with its track by itself).
        let session = self.stop.child_token();
        let _readers_end = session.clone().drop_guard();
        let mut readers: HashMap<TrackSid, (String, CancellationToken)> = HashMap::new();
        loop {
            let event = tokio::select! {
                _ = self.stop.cancelled() => {
                    let _ = room.close().await;
                    return Ok(());
                }
                event = events.recv() => event,
            };
            match event {
                Some(RoomEvent::ParticipantConnected(participant)) => self.calls.joined(
                    participant.identity().as_str(),
                    Some(&participant.name()),
                    Some(participant.sid().as_str()),
                ),
                Some(RoomEvent::ParticipantDisconnected(participant)) => {
                    let identity = participant.identity();
                    self.calls.left(identity.as_str(), Some(participant.sid().as_str()));
                    readers.retain(|_, (whose, reader)| {
                        let gone = whose == identity.as_str();
                        if gone {
                            reader.cancel();
                        }
                        !gone
                    });
                }
                Some(RoomEvent::TrackUnsubscribed { track, .. }) => {
                    if let Some((_, reader)) = readers.remove(&track.sid()) {
                        reader.cancel();
                    }
                }
                Some(RoomEvent::TrackSubscribed { track: RemoteTrack::Audio(track), participant, .. }) => {
                    let identity = participant.identity().to_string();
                    tracing::info!("{identity}: microphone track {} subscribed", track.sid());
                    let calls = self.calls.clone();
                    let sid = track.sid();
                    let reader = session.child_token();
                    if let Some((_, before)) = readers.insert(sid.clone(), (identity.clone(), reader.clone())) {
                        before.cancel();
                    }
                    let mut stream = NativeAudioStream::new(track.rtc_track(), SAMPLE_RATE as i32, 1);
                    tokio::spawn(async move {
                        let mut samples = 0usize;
                        loop {
                            let frame = tokio::select! {
                                _ = reader.cancelled() => break,
                                frame = stream.next() => match frame { Some(frame) => frame, None => break },
                            };
                            samples += frame.data.len();
                            calls.audio(&identity, &frame.data);
                        }
                        stream.close();
                        let seconds = samples as f64 / f64::from(SAMPLE_RATE);
                        tracing::info!("{identity}: microphone track {sid} ended after {seconds:.1}s");
                    });
                }
                // A participant's microphone in the log: live 02.11 the station heard nothing of a phone that answered.
                Some(RoomEvent::TrackSubscriptionFailed { participant, error, track_sid }) => {
                    tracing::warn!("{}: track {track_sid} not subscribed: {error}", participant.identity());
                }
                Some(RoomEvent::TrackMuted { participant, .. }) => {
                    tracing::info!("{}: microphone muted", participant.identity());
                }
                Some(RoomEvent::TrackUnmuted { participant, .. }) => {
                    tracing::info!("{}: microphone unmuted", participant.identity());
                }
                Some(RoomEvent::DataReceived { payload, topic, participant: Some(participant), .. }) => {
                    if topic.as_deref() != Some(LIVEKIT_TOPIC) {
                        continue;
                    }
                    let identity = participant.identity().to_string();
                    match serde_json::from_slice::<Value>(&payload) {
                        Ok(message) => self.calls.message(&identity, &message),
                        Err(error) => tracing::warn!("message from {identity}: {error}"),
                    }
                }
                Some(RoomEvent::Disconnected { .. }) | None => {
                    tracing::warn!("left the room");
                    return Ok(());
                }
                Some(_) => {}
            }
        }
    }
}
