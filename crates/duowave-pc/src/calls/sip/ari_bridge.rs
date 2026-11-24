//! SIP phones through an Asterisk of ours (deploy/asterisk): its dialplan sends calls to our Stasis application,
//! ARI tells us about them, and an external media channel carries the call's audio as RTP (slin16) between
//! Asterisk and us. A phone's call rings the station; "позвони маме" with a SIP contact dials out. A SIP operator
//! for real numbers is a trunk in the same Asterisk.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, Weak};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use futures::StreamExt;
use reqwest::Method;
use serde_json::Value;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::calls::call_hub::{CallEndpoint, CallEvent, CallEventType, CallHub, CallState, EndpointKind};
use crate::config::{Contacts, SipConfig};

/// RTP of signed linear 16 kHz: Asterisk sends it big-endian, 320 samples per 20 ms.
pub fn rtp_payload_to_pcm(payload: &[u8]) -> Vec<i16> {
    payload.as_chunks::<2>().0.iter().map(|pair| i16::from_be_bytes(*pair)).collect()
}

pub fn pcm_to_rtp_payload(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().flat_map(|sample| sample.to_be_bytes()).collect()
}

/// An RTP packet's payload type and payload.
#[derive(Debug, PartialEq, Eq)]
pub struct RtpPayload<'a> {
    pub payload_type: u8,
    pub payload: &'a [u8],
}

/// The payload of an RTP packet, past its header, CSRCs and extension (and without its padding).
pub fn rtp_payload(packet: &[u8]) -> Option<RtpPayload<'_>> {
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return None;
    }
    let mut offset = 12 + usize::from(packet[0] & 0x0f) * 4;
    if packet[0] & 0x10 != 0 {
        if packet.len() < offset + 4 {
            return None;
        }
        let words = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]);
        offset += 4 + usize::from(words) * 4;
    }
    let mut end = packet.len();
    if packet[0] & 0x20 != 0 {
        end = end.checked_sub(usize::from(packet[packet.len() - 1]))?;
    }
    (offset <= end).then(|| RtpPayload { payload_type: packet[1] & 0x7f, payload: &packet[offset..end] })
}

/// encodeURIComponent: everything but letters, digits and `-_.!~*'()` percent-encoded.
fn component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

struct Leg {
    endpoint_id: String,
    name: String,
    channel_id: String,
    media_channel_id: Option<String>,
    bridge_id: Option<String>,
    /// Asterisk's end of the media channel.
    remote: Option<SocketAddr>,
    payload_type: u8,
    sequence: u16,
    timestamp: u32,
    ssrc: u32,
}

impl Leg {
    fn new(endpoint_id: &str, name: &str, channel_id: &str) -> Leg {
        Leg {
            endpoint_id: endpoint_id.to_string(),
            name: name.to_string(),
            channel_id: channel_id.to_string(),
            media_channel_id: None,
            bridge_id: None,
            remote: None,
            payload_type: 118,
            sequence: 0,
            timestamp: 0,
            ssrc: rand::random(),
        }
    }
}

/// A SIP phone in our call, through Asterisk.
struct SipEndpoint {
    id: String,
    name: String,
    bridge: Weak<AriBridge>,
}

impl CallEndpoint for SipEndpoint {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> EndpointKind {
        EndpointKind::Sip
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn deliver(&self, frame: &[i16]) {
        if let Some(bridge) = self.bridge.upgrade() {
            bridge.send(&self.id, frame);
        }
    }

    fn notify(&self, event: &CallEvent) {
        let connected = match event.kind {
            CallEventType::Connected => true,
            CallEventType::Ended => false,
            _ => return,
        };
        let (Some(bridge), Ok(runtime)) = (self.bridge.upgrade(), tokio::runtime::Handle::try_current()) else {
            return;
        };
        let id = self.id.clone();
        runtime.spawn(async move {
            if connected {
                bridge.connected(&id).await;
            } else {
                bridge.hangup(&id).await;
            }
        });
    }
}

pub struct AriBridge {
    config: SipConfig,
    /// The contacts change in place (the panel's): the same bridge goes on with them.
    contacts: RwLock<Contacts>,
    hub: Arc<CallHub>,
    client: reqwest::Client,
    /// In the order they came: an RTP packet from an unknown port goes to the first leg waiting for its media.
    legs: Mutex<Vec<Leg>>,
    udp: Mutex<Option<Arc<UdpSocket>>>,
    /// The RTP reader and the events: `shutdown` waits for them, so the port is free when it returns.
    tasks: Mutex<Vec<JoinHandle<()>>>,
    stop: CancellationToken,
    me: Weak<AriBridge>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl AriBridge {
    pub fn new(config: SipConfig, hub: Arc<CallHub>) -> Arc<AriBridge> {
        Arc::new_cyclic(|me| AriBridge {
            contacts: RwLock::new(config.contacts.clone()),
            config,
            hub,
            client: crate::utils::http::client(),
            legs: Mutex::default(),
            udp: Mutex::new(None),
            tasks: Mutex::default(),
            stop: CancellationToken::new(),
            me: me.clone(),
        })
    }

    /// Our RTP socket on its port, then ARI's events (again 5 s after the socket is lost).
    pub async fn start(self: &Arc<Self>) {
        match UdpSocket::bind(("0.0.0.0", self.config.rtp_port)).await {
            Ok(socket) => {
                let socket = Arc::new(socket);
                *lock(&self.udp) = Some(socket.clone());
                lock(&self.tasks).push(tokio::spawn(self.clone().receive(socket)));
            }
            Err(error) => tracing::warn!("RTP socket on port {}: {error}", self.config.rtp_port),
        }
        lock(&self.tasks).push(tokio::spawn(self.clone().events()));
    }

    /// The port our RTP socket took.
    pub fn rtp_port(&self) -> Option<u16> {
        lock(&self.udp).as_ref().and_then(|socket| socket.local_addr().ok()).map(|address| address.port())
    }

    /// Whether these settings are this bridge's, the contacts aside: then the bridge goes on with new contacts.
    pub fn same_link(&self, other: &SipConfig) -> bool {
        let mut mine = self.config.clone();
        mine.contacts = other.contacts.clone();
        mine == *other
    }

    pub fn set_contacts(&self, contacts: Contacts) {
        *self.contacts.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = contacts;
    }

    /// The gateway stops: the tasks end; the calls end with the process.
    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// Leaves for good, the panel's settings changed: its calls end (their phones hung up, their parties out of
    /// the hub), its tasks end, and its RTP port is free when this returns — the next bridge may take it at once.
    pub async fn shutdown(&self) {
        let legs: Vec<String> = lock(&self.legs).iter().map(|leg| leg.endpoint_id.clone()).collect();
        let hangups = futures::future::join_all(legs.iter().map(|id| self.hangup(id)));
        if tokio::time::timeout(Duration::from_secs(5), hangups).await.is_err() {
            // Asterisk does not answer: the calls end on our side all the same.
            for id in &legs {
                self.hub.unregister(id, "the SIP bridge was set up again");
            }
            lock(&self.legs).clear();
        }
        self.stop.cancel();
        let tasks: Vec<JoinHandle<()>> = lock(&self.tasks).drain(..).collect();
        for mut task in tasks {
            if tokio::time::timeout(Duration::from_secs(3), &mut task).await.is_err() {
                task.abort();
                let _ = task.await;
            }
        }
        lock(&self.udp).take();
    }

    /// Dials a contact (an extension or a number through the operator's trunk) and puts it in the hub.
    pub async fn dial(&self, number: &str, name: &str) -> Result<Arc<dyn CallEndpoint>> {
        let path = format!(
            "/channels?endpoint={}&app={}&appArgs=outgoing&callerId={}",
            component(&format!("PJSIP/{number}")),
            component(&self.config.app),
            component("Колонка")
        );
        let channel = self.rest(Method::POST, &path).await?;
        let channel_id = channel.get("id").and_then(Value::as_str).ok_or_else(|| anyhow!("ARI gave no channel"))?;
        let endpoint = self.add_leg(channel_id, name);
        self.hub.register(endpoint.clone());
        Ok(endpoint)
    }

    /// A contact's number, by the name people say: «маме» holds «мам».
    pub fn contact(&self, name: &str) -> Option<String> {
        let wanted = name.to_lowercase();
        self.contacts
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|(contact, _)| {
                let mut stem: Vec<char> = contact.to_lowercase().chars().collect();
                stem.pop();
                wanted.contains(&stem.into_iter().collect::<String>())
            })
            .map(|(_, number)| number.clone())
    }

    fn add_leg(&self, channel_id: &str, name: &str) -> Arc<dyn CallEndpoint> {
        let id = format!("sip:{channel_id}");
        lock(&self.legs).push(Leg::new(&id, name, channel_id));
        Arc::new(SipEndpoint { id, name: name.to_string(), bridge: self.me.clone() })
    }

    fn send(&self, endpoint_id: &str, frame: &[i16]) {
        let Some(udp) = lock(&self.udp).clone() else { return };
        let (packet, remote) = {
            let mut legs = lock(&self.legs);
            let Some(leg) = legs.iter_mut().find(|leg| leg.endpoint_id == endpoint_id) else { return };
            let Some(remote) = leg.remote else { return };
            let mut packet = Vec::with_capacity(12 + frame.len() * 2);
            packet.push(0x80);
            packet.push(leg.payload_type);
            packet.extend_from_slice(&leg.sequence.to_be_bytes());
            packet.extend_from_slice(&leg.timestamp.to_be_bytes());
            packet.extend_from_slice(&leg.ssrc.to_be_bytes());
            packet.extend(pcm_to_rtp_payload(frame));
            leg.sequence = leg.sequence.wrapping_add(1);
            leg.timestamp = leg.timestamp.wrapping_add(frame.len() as u32);
            (packet, remote)
        };
        let _ = udp.try_send_to(&packet, remote);
    }

    /// Our side answered (the station said "ответь"): the phone is answered and its audio joined.
    async fn connected(&self, endpoint_id: &str) {
        let leg = lock(&self.legs)
            .iter()
            .find(|leg| leg.endpoint_id == endpoint_id && leg.media_channel_id.is_none())
            .map(|leg| (leg.channel_id.clone(), leg.name.clone()));
        let Some((channel_id, name)) = leg else { return };
        let _ = self.rest(Method::POST, &format!("/channels/{}/answer", component(&channel_id))).await;
        if let Err(error) = self.join_media(endpoint_id, &channel_id).await {
            tracing::warn!("media for {name}: {error:#}");
            self.hub.unregister(endpoint_id, "no media");
        }
    }

    async fn hangup(&self, endpoint_id: &str) {
        let leg = {
            let mut legs = lock(&self.legs);
            let Some(at) = legs.iter().position(|leg| leg.endpoint_id == endpoint_id) else { return };
            legs.remove(at)
        };
        for channel in [Some(leg.channel_id), leg.media_channel_id].into_iter().flatten() {
            let _ = self.rest(Method::DELETE, &format!("/channels/{}", component(&channel))).await;
        }
        if let Some(bridge) = leg.bridge_id {
            let _ = self.rest(Method::DELETE, &format!("/bridges/{}", component(&bridge))).await;
        }
        self.hub.unregister(endpoint_id, "hung up");
    }

    fn events_url(&self) -> String {
        let url = self.config.url.trim_end_matches('/');
        let base = match url.strip_prefix("http") {
            Some(rest) => format!("ws{rest}"),
            None => url.to_string(),
        };
        let key = format!("{}:{}", self.config.user, self.config.password);
        format!("{base}/ari/events?app={}&api_key={}", component(&self.config.app), component(&key))
    }

    async fn events(self: Arc<Self>) {
        loop {
            // The URL carries the password: never in the log.
            let url = self.events_url();
            let connect = tokio_tungstenite::connect_async_tls_with_config(
                url.as_str(),
                None,
                false,
                Some(crate::utils::http::ws_connector()),
            );
            let connected = tokio::select! {
                _ = self.stop.cancelled() => return,
                connected = connect => connected,
            };
            match connected {
                Ok((mut socket, _)) => {
                    tracing::info!("ARI events connected");
                    loop {
                        let message = tokio::select! {
                            _ = self.stop.cancelled() => return,
                            message = socket.next() => message,
                        };
                        match message {
                            Some(Ok(Message::Text(text))) => match serde_json::from_str::<Value>(&text) {
                                Ok(event) => self.on_event(&event).await,
                                Err(error) => tracing::warn!("ARI event: {error}"),
                            },
                            Some(Ok(Message::Close(_))) | None => break,
                            Some(Err(error)) => {
                                tracing::warn!("ARI: {error}");
                                break;
                            }
                            Some(Ok(_)) => {}
                        }
                    }
                }
                Err(error) => tracing::warn!("ARI: {error}"),
            }
            tokio::select! {
                _ = self.stop.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
    }

    async fn on_event(&self, event: &Value) {
        let Some(channel) = event.get("channel") else { return };
        let Some(channel_id) = channel.get("id").and_then(Value::as_str) else { return };
        match event.get("type").and_then(Value::as_str) {
            Some("StasisStart") => {
                let kind = event.pointer("/args/0").and_then(Value::as_str);
                if kind == Some("media") {
                    return;
                }
                let endpoint_id = format!("sip:{channel_id}");
                if kind == Some("outgoing") {
                    // The contact picked up: our call to it is answered.
                    if let Some(call) = self.hub.call_of(&endpoint_id).filter(|call| call.state == CallState::Ringing) {
                        if let Err(error) = self.hub.accept(&call.id, &endpoint_id) {
                            tracing::warn!("ARI event: {error}");
                        }
                    }
                    return;
                }
                // A phone calls the station.
                let text = |pointer: &str| channel.pointer(pointer).and_then(Value::as_str).filter(|s| !s.is_empty());
                let name = text("/caller/name").or_else(|| text("/caller/number")).unwrap_or("Телефон").to_string();
                let endpoint = self.add_leg(channel_id, &name);
                self.hub.register(endpoint);
                let station = self.hub.list().into_iter().find(|party| party.kind == EndpointKind::Station);
                let Some(station) = station else {
                    let _ = self.rest(Method::DELETE, &format!("/channels/{}", component(channel_id))).await;
                    self.hub.unregister(&endpoint_id, "the station is not on our calls");
                    return;
                };
                let _ = self.rest(Method::POST, &format!("/channels/{}/ring", component(channel_id))).await;
                if let Err(error) = self.hub.call(&endpoint_id, &station.id) {
                    // The station is busy: the phone hears its call end rather than ring on.
                    tracing::warn!("ARI event: {error}");
                    self.hangup(&endpoint_id).await;
                }
            }
            Some("StasisEnd") | Some("ChannelDestroyed") => {
                let leg =
                    lock(&self.legs).iter().find(|leg| leg.channel_id == channel_id).map(|leg| leg.endpoint_id.clone());
                if let Some(endpoint_id) = leg {
                    self.hangup(&endpoint_id).await;
                }
            }
            _ => {}
        }
    }

    async fn join_media(&self, endpoint_id: &str, channel_id: &str) -> Result<()> {
        let host = format!("{}:{}", self.config.rtp_host, self.config.rtp_port);
        let media = self
            .rest(
                Method::POST,
                &format!(
                    "/channels/externalMedia?app={}&external_host={}&format=slin16&data=media",
                    component(&self.config.app),
                    component(&host)
                ),
            )
            .await?;
        let media_id = media.get("id").and_then(Value::as_str).ok_or_else(|| anyhow!("ARI gave no media channel"))?;
        let variable = |name: &str| media.pointer(&format!("/channelvars/{name}")).and_then(Value::as_str);
        let address = variable("UNICASTRTP_LOCAL_ADDRESS").and_then(|address| address.parse::<IpAddr>().ok());
        let port = variable("UNICASTRTP_LOCAL_PORT").and_then(|port| port.parse::<u16>().ok()).filter(|port| *port > 0);
        self.update(endpoint_id, |leg| {
            leg.media_channel_id = Some(media_id.to_string());
            if let (Some(address), Some(port)) = (address, port) {
                leg.remote = Some(SocketAddr::new(address, port));
            }
        });
        let bridge = self.rest(Method::POST, "/bridges?type=mixing").await?;
        let bridge_id = bridge.get("id").and_then(Value::as_str).ok_or_else(|| anyhow!("ARI gave no bridge"))?;
        self.update(endpoint_id, |leg| leg.bridge_id = Some(bridge_id.to_string()));
        let channels = format!("{},{}", component(channel_id), component(media_id));
        self.rest(Method::POST, &format!("/bridges/{}/addChannel?channel={channels}", component(bridge_id))).await?;
        Ok(())
    }

    fn update(&self, endpoint_id: &str, change: impl FnOnce(&mut Leg)) {
        if let Some(leg) = lock(&self.legs).iter_mut().find(|leg| leg.endpoint_id == endpoint_id) {
            change(leg);
        }
    }

    async fn receive(self: Arc<Self>, socket: Arc<UdpSocket>) {
        let mut buffer = vec![0u8; 2048];
        loop {
            let received = tokio::select! {
                _ = self.stop.cancelled() => return,
                received = socket.recv_from(&mut buffer) => received,
            };
            match received {
                Ok((length, from)) => self.on_rtp(&buffer[..length], from),
                Err(error) => {
                    tracing::warn!("RTP socket: {error}");
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        }
    }

    fn on_rtp(&self, packet: &[u8], from: SocketAddr) {
        let Some(parsed) = rtp_payload(packet) else { return };
        let endpoint_id = {
            let mut legs = lock(&self.legs);
            let at = legs
                .iter()
                .position(|leg| leg.remote == Some(from))
                .or_else(|| legs.iter().position(|leg| leg.media_channel_id.is_some() && leg.remote.is_none()));
            let Some(at) = at else { return };
            let leg = &mut legs[at];
            leg.remote.get_or_insert(from);
            leg.payload_type = parsed.payload_type;
            leg.endpoint_id.clone()
        };
        self.hub.audio(&endpoint_id, &rtp_payload_to_pcm(parsed.payload));
    }

    async fn rest(&self, method: Method, path: &str) -> Result<Value> {
        let url = format!("{}/ari{path}", self.config.url.trim_end_matches('/'));
        let what = format!("ARI {method} {}", path.split('?').next().unwrap_or(path));
        let response = self
            .client
            .request(method, url)
            .basic_auth(&self.config.user, Some(&self.config.password))
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|error| anyhow!("{what}: {}", error.without_url()))?;
        if !response.status().is_success() {
            bail!("{what}: HTTP {}", response.status().as_u16());
        }
        let text = response.text().await.map_err(|error| anyhow!("{what}: {}", error.without_url()))?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text)?)
    }
}

#[cfg(test)]
mod tests {
    use axum::extract::ws::{Message as WsMessage, WebSocketUpgrade};
    use axum::extract::{OriginalUri, RawQuery, State};
    use axum::http::{HeaderMap, Method as HttpMethod};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{any, get};
    use axum::Router;
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;
    use crate::calls::call_hub::tests::TestParty;

    #[test]
    fn reads_rtp_and_converts_slin16_to_our_samples_and_back() {
        let pcm = [1i16, -2, 300, -32768];
        let header = [0x80u8, 118, 0, 1, 0, 0, 1, 64, 0, 0, 0, 7];
        let packet = [header.as_slice(), &pcm_to_rtp_payload(&pcm)].concat();
        let parsed = rtp_payload(&packet).unwrap();
        assert_eq!(parsed.payload_type, 118);
        assert_eq!(rtp_payload_to_pcm(parsed.payload), pcm);
        assert!(rtp_payload(&[0x40, 0]).is_none());
        // Padding past the payload's start is not a payload.
        assert!(rtp_payload(&[0xa0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 7, 200]).is_none());
        assert_eq!(component("PJSIP/101"), "PJSIP%2F101");
        assert_eq!(component("duowave:p@ss"), "duowave%3Ap%40ss");
    }

    struct Fake {
        requests: Mutex<Vec<String>>,
        authorization: Mutex<Vec<String>>,
        events_query: Mutex<Option<String>>,
        events: Mutex<Option<mpsc::UnboundedReceiver<Value>>>,
        rtp_port: u16,
    }

    async fn rest(
        State(fake): State<Arc<Fake>>,
        method: HttpMethod,
        OriginalUri(uri): OriginalUri,
        headers: HeaderMap,
    ) -> Response {
        let path = uri.path_and_query().map(|pq| pq.as_str().to_string()).unwrap_or_default();
        let path = path.trim_start_matches("/ari").to_string();
        lock(&fake.requests).push(format!("{method} {path}"));
        if let Some(value) = headers.get("authorization").and_then(|value| value.to_str().ok()) {
            lock(&fake.authorization).push(value.to_string());
        }
        let body = match (method.as_str(), uri.path()) {
            ("POST", "/ari/channels/externalMedia") => json!({
                "id": "media-1",
                "channelvars": {"UNICASTRTP_LOCAL_ADDRESS": "127.0.0.1", "UNICASTRTP_LOCAL_PORT": fake.rtp_port.to_string()},
            }),
            ("POST", "/ari/bridges") => json!({"id": "bridge-1"}),
            ("POST", "/ari/channels") => json!({"id": "out-1"}),
            _ => return axum::http::StatusCode::NO_CONTENT.into_response(),
        };
        axum::Json(body).into_response()
    }

    async fn events(State(fake): State<Arc<Fake>>, RawQuery(query): RawQuery, upgrade: WebSocketUpgrade) -> Response {
        *lock(&fake.events_query) = query;
        let Some(mut events) = lock(&fake.events).take() else {
            return axum::http::StatusCode::CONFLICT.into_response();
        };
        upgrade.on_upgrade(move |mut socket| async move {
            while let Some(event) = events.recv().await {
                if socket.send(WsMessage::Text(event.to_string().into())).await.is_err() {
                    return;
                }
            }
        })
    }

    /// Asterisk as far as ARI goes: its REST calls recorded, its events from the test, and its RTP socket.
    async fn fake_asterisk() -> (Arc<Fake>, mpsc::UnboundedSender<Value>, UdpSocket, String) {
        let rtp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (sender, receiver) = mpsc::unbounded_channel();
        let fake = Arc::new(Fake {
            requests: Mutex::default(),
            authorization: Mutex::default(),
            events_query: Mutex::default(),
            events: Mutex::new(Some(receiver)),
            rtp_port: rtp.local_addr().unwrap().port(),
        });
        let app =
            Router::new().route("/ari/events", get(events)).route("/ari/{*path}", any(rest)).with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (fake, sender, rtp, url)
    }

    fn config(url: &str) -> SipConfig {
        SipConfig {
            url: format!("{url}/"),
            user: "duowave".into(),
            password: "p@ss".into(),
            app: "duowave".into(),
            rtp_host: "127.0.0.1".into(),
            rtp_port: 0,
            contacts: vec![("мама".to_string(), "101".to_string())],
        }
    }

    #[test]
    fn takes_the_first_contact_in_the_order_sip_contacts_lists_them() {
        let mut ordered = config("http://127.0.0.1:1");
        ordered.contacts = vec![("бабушка".to_string(), "201".to_string()), ("баба".to_string(), "202".to_string())];
        // «баб» is in «бабушке» too: the first listed wins, as in the TS, not the first by the alphabet.
        assert_eq!(AriBridge::new(ordered, CallHub::new()).contact("позвони бабушке").as_deref(), Some("201"));
    }

    async fn until(what: &str, mut check: impl FnMut() -> bool) {
        for _ in 0..500 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out: {what}");
    }

    fn requested(fake: &Fake, request: &str) -> bool {
        lock(&fake.requests).iter().any(|seen| seen == request)
    }

    #[tokio::test]
    async fn rings_the_station_from_a_phone_carries_its_audio_both_ways_and_hangs_up_with_it() {
        let (fake, events, asterisk_rtp, url) = fake_asterisk().await;
        let hub = CallHub::new();
        let station = TestParty::new("station:default", "Колонка", EndpointKind::Station);
        hub.register(station.clone());
        let bridge = AriBridge::new(config(&url), hub.clone());
        bridge.start().await;
        let our_port = bridge.rtp_port().unwrap();
        until("the events socket", || lock(&fake.events_query).is_some()).await;
        assert_eq!(lock(&fake.events_query).as_deref(), Some("app=duowave&api_key=duowave%3Ap%40ss"));

        events
            .send(json!({"type": "StasisStart", "args": [], "channel": {"id": "ch-1", "caller": {"name": "", "number": "101"}}}))
            .unwrap();
        until("the station rings", || lock(&station.events).iter().any(|event| event.kind == CallEventType::Incoming))
            .await;
        assert_eq!(station.last().peer.name, "101");
        assert!(requested(&fake, "POST /channels/ch-1/ring"));

        let call = hub.call_of("station:default").unwrap();
        hub.accept(&call.id, "station:default").unwrap();
        until("the media bridge", || requested(&fake, "POST /bridges/bridge-1/addChannel?channel=ch-1,media-1")).await;
        assert!(requested(&fake, "POST /channels/ch-1/answer"));
        assert!(requested(
            &fake,
            "POST /channels/externalMedia?app=duowave&external_host=127.0.0.1%3A0&format=slin16&data=media"
        ));
        assert!(requested(&fake, "POST /bridges?type=mixing"));

        // The station speaks: slin16 RTP to Asterisk's end of the media channel.
        hub.audio("station:default", &[5; 320]);
        let mut packet = vec![0u8; 2048];
        let (length, _) =
            tokio::time::timeout(Duration::from_secs(2), asterisk_rtp.recv_from(&mut packet)).await.unwrap().unwrap();
        let heard = rtp_payload(&packet[..length]).unwrap();
        assert_eq!(heard.payload_type, 118);
        assert_eq!(rtp_payload_to_pcm(heard.payload), vec![5; 320]);

        // The phone speaks: its RTP reaches the station as our frames.
        let header = [0x80u8, 118, 0, 1, 0, 0, 1, 64, 0, 0, 0, 9];
        let phone = [header.as_slice(), &pcm_to_rtp_payload(&[9; 320])].concat();
        asterisk_rtp.send_to(&phone, ("127.0.0.1", our_port)).await.unwrap();
        until("the phone's voice", || !station.frames().is_empty()).await;
        assert_eq!(station.frames()[0], vec![9; 320]);

        events.send(json!({"type": "StasisEnd", "channel": {"id": "ch-1"}})).unwrap();
        until("the call ends", || station.last().kind == CallEventType::Ended).await;
        assert_eq!(station.last().reason.as_deref(), Some("hung up"));
        until("the channels go", || requested(&fake, "DELETE /bridges/bridge-1")).await;
        assert!(requested(&fake, "DELETE /channels/ch-1") && requested(&fake, "DELETE /channels/media-1"));
        assert!(hub.endpoint("sip:ch-1").is_none());
        let basic =
            format!("Basic {}", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "duowave:p@ss"));
        assert!(lock(&fake.authorization).iter().all(|value| *value == basic));
        bridge.stop();
    }

    #[tokio::test]
    async fn dials_a_contact_and_turns_away_a_phone_when_no_station_is_on() {
        let (fake, events, _rtp, url) = fake_asterisk().await;
        let hub = CallHub::new();
        let station = TestParty::new("station:default", "Колонка", EndpointKind::Station);
        hub.register(station.clone());
        let bridge = AriBridge::new(config(&url), hub.clone());
        bridge.start().await;
        until("the events socket", || lock(&fake.events_query).is_some()).await;

        assert_eq!(bridge.contact("маме").as_deref(), Some("101"));
        assert_eq!(bridge.contact("папе"), None);
        let phone = bridge.dial("101", "мама").await.unwrap();
        assert_eq!(phone.id(), "sip:out-1");
        assert!(requested(
            &fake,
            "POST /channels?endpoint=PJSIP%2F101&app=duowave&appArgs=outgoing&callerId=%D0%9A%D0%BE%D0%BB%D0%BE%D0%BD%D0%BA%D0%B0"
        ));
        hub.call("station:default", phone.id()).unwrap();
        events.send(json!({"type": "StasisStart", "args": ["outgoing"], "channel": {"id": "out-1"}})).unwrap();
        until("the contact picks up", || station.last().kind == CallEventType::Connected).await;

        hub.unregister("station:default", "gone");
        until("the phone is hung up", || requested(&fake, "DELETE /channels/out-1")).await;
        events
            .send(json!({"type": "StasisStart", "args": [], "channel": {"id": "ch-2", "caller": {"name": "Папа"}}}))
            .unwrap();
        until("the phone is turned away", || requested(&fake, "DELETE /channels/ch-2")).await;
        assert!(hub.endpoint("sip:ch-2").is_none());
        assert!(!requested(&fake, "POST /channels/ch-2/ring"));
        bridge.stop();
    }

    #[tokio::test]
    async fn set_up_again_it_hangs_its_calls_up_and_the_next_bridge_takes_the_same_rtp_port() {
        let (fake, _events, _asterisk_rtp, url) = fake_asterisk().await;
        let port = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let settings = SipConfig { rtp_port: port, ..config(&url) };
        let hub = CallHub::new();
        let bridge = AriBridge::new(settings.clone(), hub.clone());
        bridge.start().await;
        assert_eq!(bridge.rtp_port(), Some(port));
        let phone = bridge.add_leg("ch-9", "Мама");
        hub.register(phone);
        // New contacts alone: the same bridge goes on with them.
        assert!(bridge.same_link(&SipConfig { contacts: vec![("папа".into(), "102".into())], ..settings.clone() }));
        bridge.set_contacts(vec![("папа".into(), "102".into())]);
        assert_eq!(bridge.contact("позвони папе").as_deref(), Some("102"));
        assert!(!bridge.same_link(&SipConfig { password: "other".into(), ..settings.clone() }));

        bridge.shutdown().await;
        assert!(requested(&fake, "DELETE /channels/ch-9"), "its phone hung up");
        assert!(hub.endpoint("sip:ch-9").is_none(), "its party left the hub");
        assert_eq!(bridge.rtp_port(), None);
        let next = AriBridge::new(settings, hub.clone());
        next.start().await;
        assert_eq!(next.rtp_port(), Some(port), "the port was free again");
        next.shutdown().await;
    }
}
