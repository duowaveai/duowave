//! A browser on our call page, over WebRTC: the hub answers the page's offer (webrtc-rs, pure Rust; the TS gateway
//! used werift), Opus from the page is decoded to 16 kHz for the hub, the other party's audio is encoded back. The
//! browser does echo cancellation and noise suppression on its side. The feature `webrtc`.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{anyhow, Result};
use rtc::rtp;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
};
use tokio::sync::{mpsc, watch};
use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::media_stream::MediaStreamTrack;
use webrtc::peer_connection::{
    register_default_interceptors, MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCIceGatheringState, RTCSessionDescription, Registry,
};

use super::call_hub::{CallEndpoint, CallEvent, CallEventType, EndpointKind};
use super::opus::CallsOpus;

/// Opus in RTP counts 48 kHz: 960 per 20 ms packet.
const RTP_STEP: u32 = 960;
/// What the page offers for Opus and we answer with when it offers no other number.
const OPUS_PAYLOAD_TYPE: u8 = 111;
/// How long the answer waits for our candidates (no trickle).
const GATHERING: Duration = Duration::from_secs(3);
/// The other party's frames waiting for the encoder: a second, then the newest are dropped.
const SEND_BACKLOG: usize = 50;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub type AudioSink = Box<dyn Fn(&[i16]) + Send + Sync>;
pub type EventSink = Box<dyn Fn(&CallEvent) + Send + Sync>;

fn opus_codec() -> RTCRtpCodec {
    RTCRtpCodec {
        mime_type: "audio/opus".to_string(),
        clock_rate: 48_000,
        channels: 2,
        sdp_fmtp_line: "minptime=10;useinbandfec=1".to_string(),
        rtcp_feedback: vec![],
    }
}

/// The peer connection's events we act on: the page's microphone, and the end of our candidates' gathering.
struct Events {
    opus: Arc<dyn CallsOpus>,
    on_audio: Arc<AudioSink>,
    gathered: watch::Sender<bool>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Events {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gathered.send(true);
        }
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let mut decoder = match self.opus.decoder() {
            Ok(decoder) => decoder,
            Err(error) => {
                tracing::warn!("call page: no Opus decoder: {error:#}");
                return;
            }
        };
        let on_audio = self.on_audio.clone();
        tokio::spawn(async move {
            while let Some(event) = track.poll().await {
                match event {
                    // A damaged packet is a 20 ms gap.
                    TrackRemoteEvent::OnRtpPacket(packet) => {
                        if let Ok(pcm) = decoder.decode(&packet.payload) {
                            on_audio(&pcm);
                        }
                    }
                    TrackRemoteEvent::OnEnded => break,
                    _ => {}
                }
            }
        });
    }
}

/// The page's media while a call is on.
struct Media {
    connection: Arc<dyn PeerConnection>,
    /// The other party's frames, once the answer is ready.
    frames: Option<mpsc::Sender<Vec<i16>>>,
}

impl Media {
    /// Only `close()` stops a connection's driver and frees its sockets: one dropped without it keeps them until
    /// the process exits.
    async fn close(self) {
        drop(self.frames);
        let _ = self.connection.close().await;
    }
}

/// The page's media and how many times it was taken: an answer under way keeps the connection it builds only while
/// nothing took the media meanwhile (a close, the call's end, the next offer).
#[derive(Default)]
struct Slot {
    media: Option<Media>,
    taken: u64,
    /// The page's socket closed: no media any more.
    closed: bool,
}

pub struct WebRtcEndpoint {
    id: String,
    name: String,
    opus: Arc<dyn CallsOpus>,
    /// Audio from the browser, for the hub.
    on_audio: Arc<AudioSink>,
    /// Control events, to the page over its WebSocket.
    on_event: EventSink,
    /// Where our candidates are: every interface (loopback and link-local skipped); tests take loopback.
    udp_addrs: Vec<String>,
    media: Mutex<Slot>,
}

impl WebRtcEndpoint {
    pub fn new(
        id: String,
        name: String,
        opus: Arc<dyn CallsOpus>,
        on_audio: AudioSink,
        on_event: EventSink,
    ) -> Result<WebRtcEndpoint> {
        // The codec is there before the page offers: an endpoint that could not carry sound is refused at once.
        drop(opus.encoder()?);
        Ok(WebRtcEndpoint {
            id,
            name,
            opus,
            on_audio: Arc::new(on_audio),
            on_event,
            // The crate's tests talk to their page on loopback, through the calls' socket too.
            udp_addrs: vec![if cfg!(test) { "127.0.0.1:0" } else { "0.0.0.0:0" }.to_string()],
            media: Mutex::default(),
        })
    }

    /// Candidates on these addresses only.
    pub fn on_addresses(mut self, udp_addrs: &[&str]) -> WebRtcEndpoint {
        self.udp_addrs = udp_addrs.iter().map(|address| address.to_string()).collect();
        self
    }

    /// The answer to the page's offer, with our candidates in it (no trickle).
    pub async fn answer(&self, offer_sdp: &str) -> Result<String> {
        self.close_media().await;
        let taken = lock(&self.media).taken;
        let (connection, mut gathering) = self.connect().await?;
        // The connection is the page's media at once, as the TS kept its pc from the start: what ends the media
        // from now on closes it. Ended already while it was built: nobody else would close it.
        let stale = {
            let mut slot = lock(&self.media);
            let stale = slot.closed || slot.taken != taken;
            if !stale {
                slot.media = Some(Media { connection: connection.clone(), frames: None });
            }
            stale
        };
        if stale {
            let _ = connection.close().await;
            return Err(anyhow!("the call page's media ended while its offer was answered"));
        }
        match self.negotiate(&connection, offer_sdp, &mut gathering).await {
            Ok((sdp, frames)) => {
                // Taken while it was negotiated, and closed by what took it: the page still gets its answer, as
                // from the TS, but no sound.
                let mut slot = lock(&self.media);
                let ours = slot.taken == taken;
                if let Some(media) = slot.media.as_mut().filter(|_| ours) {
                    media.frames = Some(frames);
                }
                Ok(sdp)
            }
            Err(error) => {
                {
                    let mut slot = lock(&self.media);
                    if slot.taken == taken {
                        slot.taken += 1;
                        slot.media = None;
                    }
                }
                let _ = connection.close().await;
                Err(error)
            }
        }
    }

    /// A peer connection for the page, its sockets bound, and the end of its candidates' gathering.
    async fn connect(&self) -> Result<(Arc<dyn PeerConnection>, watch::Receiver<bool>)> {
        let mut media_engine = MediaEngine::default();
        media_engine.register_codec(
            RTCRtpCodecParameters { rtp_codec: opus_codec(), payload_type: OPUS_PAYLOAD_TYPE },
            RtpCodecKind::Audio,
        )?;
        let registry = register_default_interceptors(Registry::new(), &mut media_engine)?;
        let (gathered, gathering) = watch::channel(false);
        let events = Arc::new(Events { opus: self.opus.clone(), on_audio: self.on_audio.clone(), gathered });
        let connection = PeerConnectionBuilder::new()
            .with_configuration(RTCConfigurationBuilder::new().build())
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .with_handler(events)
            .with_udp_addrs(self.udp_addrs.clone())
            .build()
            .await?;
        let connection: Arc<dyn PeerConnection> = Arc::new(connection);
        Ok((connection, gathering))
    }

    /// The answer on this connection, and where the other party's frames go to be sent.
    async fn negotiate(
        &self,
        connection: &Arc<dyn PeerConnection>,
        offer_sdp: &str,
        gathering: &mut watch::Receiver<bool>,
    ) -> Result<(String, mpsc::Sender<Vec<i16>>)> {
        let ssrc: u32 = rand::random();
        let track = Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
            format!("duowave-{}", self.id),
            format!("duowave-{}-audio", self.id),
            "station".to_string(),
            RtpCodecKind::Audio,
            vec![RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters { ssrc: Some(ssrc), ..Default::default() },
                codec: opus_codec(),
                ..Default::default()
            }],
        )));
        let sender = connection.add_track(track.clone() as Arc<dyn TrackLocal>).await?;
        connection.set_remote_description(RTCSessionDescription::offer(offer_sdp.to_string())?).await?;
        let answer = connection.create_answer(None).await?;
        connection.set_local_description(answer).await?;
        let _ = tokio::time::timeout(GATHERING, gathering.wait_for(|done| *done)).await;
        let sdp = connection.local_description().await.map(|description| description.sdp).unwrap_or_default();
        if sdp.is_empty() {
            return Err(anyhow!("no local description"));
        }
        // The number the page gave Opus, as the answer settled it.
        let payload_type = sender
            .get_parameters()
            .await
            .ok()
            .and_then(|parameters| {
                let codecs = parameters.rtp_parameters.codecs;
                codecs
                    .iter()
                    .find(|codec| codec.rtp_codec.mime_type.eq_ignore_ascii_case("audio/opus"))
                    .map(|c| c.payload_type)
            })
            .unwrap_or(OPUS_PAYLOAD_TYPE);
        let (frames, mut queued) = mpsc::channel::<Vec<i16>>(SEND_BACKLOG);
        let mut encoder = self.opus.encoder()?;
        tokio::spawn(async move {
            let mut sequence: u16 = rand::random();
            let mut timestamp: u32 = rand::random();
            while let Some(frame) = queued.recv().await {
                let Ok(packets) = encoder.encode(&frame) else { continue };
                for packet in packets {
                    let header = rtp::Header {
                        version: 2,
                        payload_type,
                        sequence_number: sequence,
                        timestamp,
                        ssrc,
                        ..Default::default()
                    };
                    sequence = sequence.wrapping_add(1);
                    timestamp = timestamp.wrapping_add(RTP_STEP);
                    // Before the connection is up the track is not bound yet: that frame is lost, as on the wire.
                    let _ = track.write_rtp(rtp::Packet { header, payload: packet.into() }).await;
                }
            }
        });
        Ok((sdp, frames))
    }

    /// The page's media ends: its socket closed, for good (an answer still under way closes what it builds).
    pub async fn close(&self) {
        lock(&self.media).closed = true;
        self.close_media().await;
    }

    /// The page's media ends (the call ended, or the next offer).
    async fn close_media(&self) {
        let media = self.take_media();
        if let Some(media) = media {
            media.close().await;
        }
    }

    fn take_media(&self) -> Option<Media> {
        let mut slot = lock(&self.media);
        slot.taken += 1;
        slot.media.take()
    }
}

impl CallEndpoint for WebRtcEndpoint {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> EndpointKind {
        EndpointKind::Browser
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn deliver(&self, frame: &[i16]) {
        if let Some(frames) = lock(&self.media).media.as_ref().and_then(|media| media.frames.as_ref()) {
            let _ = frames.try_send(frame.to_vec());
        }
    }

    fn notify(&self, event: &CallEvent) {
        if event.kind == CallEventType::Ended {
            if let Some(media) = self.take_media() {
                tokio::spawn(media.close());
            }
        }
        (self.on_event)(event);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use webrtc::peer_connection::RTCPeerConnectionState;

    use super::*;
    use crate::calls::opus::SpeechOpus;

    /// The browser's side of the test: an offer with a microphone, what it hears counted.
    struct Page {
        gathered: watch::Sender<bool>,
        connected: watch::Sender<bool>,
        heard: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl PeerConnectionEventHandler for Page {
        async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
            if state == RTCIceGatheringState::Complete {
                let _ = self.gathered.send(true);
            }
        }

        async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
            if state == RTCPeerConnectionState::Connected {
                let _ = self.connected.send(true);
            }
        }

        async fn on_track(&self, track: Arc<dyn TrackRemote>) {
            let heard = self.heard.clone();
            tokio::spawn(async move {
                while let Some(event) = track.poll().await {
                    if let TrackRemoteEvent::OnRtpPacket(packet) = event {
                        if !packet.payload.is_empty() {
                            heard.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            });
        }
    }

    /// A call page as a browser has it: a peer connection on loopback with a microphone, its offer gathered.
    pub(crate) struct TestPage {
        pub connection: Arc<dyn PeerConnection>,
        pub offer: String,
        /// RTP packets with sound the page heard.
        pub heard: Arc<AtomicUsize>,
        microphone: Arc<TrackLocalStaticRTP>,
        connecting: watch::Receiver<bool>,
    }

    const PAGE_SSRC: u32 = 4242;

    impl TestPage {
        pub async fn new() -> TestPage {
            let mut media_engine = MediaEngine::default();
            media_engine
                .register_codec(
                    RTCRtpCodecParameters { rtp_codec: opus_codec(), payload_type: 111 },
                    RtpCodecKind::Audio,
                )
                .unwrap();
            let (gathered, mut gathering) = watch::channel(false);
            let (connected, connecting) = watch::channel(false);
            let heard = Arc::new(AtomicUsize::new(0));
            let connection = PeerConnectionBuilder::new()
                .with_configuration(RTCConfigurationBuilder::new().build())
                .with_media_engine(media_engine)
                .with_handler(Arc::new(Page { gathered, connected, heard: heard.clone() }))
                .with_udp_addrs(vec!["127.0.0.1:0".to_string()])
                .build()
                .await
                .unwrap();
            let connection: Arc<dyn PeerConnection> = Arc::new(connection);
            let microphone = Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
                "page".to_string(),
                "page-audio".to_string(),
                "microphone".to_string(),
                RtpCodecKind::Audio,
                vec![RTCRtpEncodingParameters {
                    rtp_coding_parameters: RTCRtpCodingParameters { ssrc: Some(PAGE_SSRC), ..Default::default() },
                    codec: opus_codec(),
                    ..Default::default()
                }],
            )));
            connection.add_track(microphone.clone() as Arc<dyn TrackLocal>).await.unwrap();
            let offer = connection.create_offer(None).await.unwrap();
            connection.set_local_description(offer).await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), gathering.wait_for(|done| *done)).await.unwrap().unwrap();
            let offer = connection.local_description().await.unwrap().sdp;
            TestPage { connection, offer, heard, microphone, connecting }
        }

        /// Takes the hub's answer and waits until the connection is up.
        pub async fn connect(&mut self, answer: &str) {
            assert!(answer.contains("a=candidate"), "{answer}");
            let answer = RTCSessionDescription::answer(answer.to_string()).unwrap();
            self.connection.set_remote_description(answer).await.unwrap();
            tokio::time::timeout(Duration::from_secs(10), self.connecting.wait_for(|done| *done))
                .await
                .unwrap()
                .unwrap();
        }

        /// A second of a tone into the microphone, as Opus RTP.
        pub async fn speak(&self) {
            let mut encoder = SpeechOpus.encoder().unwrap();
            for sequence in 0..50u16 {
                let tone: Vec<i16> = (0..320).map(|i| ((i as f64 * 0.2).sin() * 6000.0) as i16).collect();
                for packet in encoder.encode(&tone).unwrap() {
                    let header = rtp::Header {
                        version: 2,
                        payload_type: 111,
                        sequence_number: sequence,
                        timestamp: u32::from(sequence) * RTP_STEP,
                        ssrc: PAGE_SSRC,
                        ..Default::default()
                    };
                    let _ = self.microphone.write_rtp(rtp::Packet { header, payload: packet.into() }).await;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }

    pub(crate) async fn until(what: &str, check: impl Fn() -> bool) {
        for _ in 0..800 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out: {what}");
    }

    fn ended() -> CallEvent {
        let peer = crate::calls::call_hub::Party {
            id: "station:default".into(),
            name: "Колонка".into(),
            kind: EndpointKind::Station,
        };
        CallEvent { kind: CallEventType::Ended, call_id: "c1".into(), peer, reason: Some("hung up".into()) }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn answers_the_pages_offer_and_carries_opus_both_ways() {
        let got = Arc::new(Mutex::new(Vec::<Vec<i16>>::new()));
        let events = Arc::new(Mutex::new(Vec::<CallEventType>::new()));
        let (sink, seen) = (got.clone(), events.clone());
        let endpoint = WebRtcEndpoint::new(
            "browser:1".into(),
            "Мама".into(),
            Arc::new(SpeechOpus),
            Box::new(move |pcm| lock(&sink).push(pcm.to_vec())),
            Box::new(move |event| lock(&seen).push(event.kind)),
        )
        .unwrap()
        .on_addresses(&["127.0.0.1:0"]);

        let mut page = TestPage::new().await;
        let answer = endpoint.answer(&page.offer).await.unwrap();
        page.connect(&answer).await;

        // The station speaks: PCM frames reach the page as Opus RTP.
        for _ in 0..50 {
            endpoint.deliver(&[1000; 320]);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        until("the page hears the station", || page.heard.load(Ordering::Relaxed) >= 10).await;

        // The page speaks: Opus RTP reaches the hub as 16 kHz frames.
        page.speak().await;
        until("the hub hears the page", || lock(&got).len() >= 10).await;
        assert_eq!(lock(&got)[0].len(), 320);

        endpoint.notify(&ended());
        assert_eq!(*lock(&events), vec![CallEventType::Ended]);
        assert!(lock(&endpoint.media).media.is_none(), "the call's media ends with it");
        let _ = page.connection.close().await;
    }

    /// An endpoint whose connections bind one port of our own: while one lives, the port is taken.
    fn on_a_port_of_its_own() -> (WebRtcEndpoint, String) {
        let port = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let address = format!("127.0.0.1:{port}");
        let endpoint = WebRtcEndpoint::new(
            "browser:1".into(),
            "Мама".into(),
            Arc::new(SpeechOpus),
            Box::new(|_| {}),
            Box::new(|_| {}),
        )
        .unwrap()
        .on_addresses(&[&address]);
        (endpoint, address)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn closes_the_connection_of_an_offer_it_cannot_answer() {
        let (endpoint, address) = on_a_port_of_its_own();
        // A page's garbage, and an offer without its sdp (String(undefined) in the TS).
        for offer in ["garbage", "undefined"] {
            assert!(endpoint.answer(offer).await.is_err());
            assert!(lock(&endpoint.media).media.is_none());
            // Dropped without close(), its driver would hold the port until the process exits.
            until("the connection's socket is let go", || std::net::UdpSocket::bind(&address).is_ok()).await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn closes_what_an_offer_builds_after_the_pages_socket_closed() {
        let (endpoint, address) = on_a_port_of_its_own();
        let page = TestPage::new().await;
        // The offer came just before the socket closed; its answer runs after.
        endpoint.close().await;
        let refused = endpoint.answer(&page.offer).await.unwrap_err();
        assert!(refused.to_string().contains("ended"), "{refused}");
        assert!(lock(&endpoint.media).media.is_none());
        until("the connection's socket is let go", || std::net::UdpSocket::bind(&address).is_ok()).await;
        let _ = page.connection.close().await;
    }
}
