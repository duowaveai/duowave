//! The station's own parties: its client (or the simulator) over our calls WebSocket, and its client over plain
//! HTTP, the way its GStreamer can do it without a WebSocket.

use std::sync::{Mutex, MutexGuard};

use axum::extract::ws::Message;
use tokio::sync::mpsc;

use super::call_hub::{pcm_to_bytes, CallEndpoint, CallEvent, EndpointKind};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The station's client (or the simulator) over our calls WebSocket: control as JSON text, audio as binary frames
/// of 16 kHz mono PCM16LE. What does not fit the socket's queue (a client that stopped reading) is dropped.
pub struct SocketEndpoint {
    id: String,
    kind: EndpointKind,
    name: String,
    socket: mpsc::Sender<Message>,
}

impl SocketEndpoint {
    pub fn new(id: String, kind: EndpointKind, name: String, socket: mpsc::Sender<Message>) -> SocketEndpoint {
        SocketEndpoint { id, kind, name, socket }
    }
}

impl CallEndpoint for SocketEndpoint {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> EndpointKind {
        self.kind
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn deliver(&self, frame: &[i16]) {
        let _ = self.socket.try_send(Message::Binary(pcm_to_bytes(frame).into()));
    }

    fn notify(&self, event: &CallEvent) {
        if let Ok(text) = serde_json::to_string(event) {
            let _ = self.socket.try_send(Message::Text(text.into()));
        }
    }
}

/// What an open listening stream does with a frame.
pub type FrameListener = Box<dyn FnMut(&[i16]) + Send>;

/// The station's client over plain HTTP: it listens to a chunked stream (GET /calls/listen/<device>: PCM, or
/// Ogg/Opus for the station's decodebin) and talks with a chunked upload of PCM (POST /calls/talk/<device>).
/// Control stays with the gateway: "ответь" and "положи трубку" are voice commands.
pub struct HttpStationEndpoint {
    id: String,
    name: String,
    /// The open GET stream, as it takes frames, by its number; one at a time.
    listener: Mutex<Option<(u64, FrameListener)>>,
}

impl HttpStationEndpoint {
    pub fn new(id: String, name: String) -> HttpStationEndpoint {
        HttpStationEndpoint { id, name, listener: Mutex::new(None) }
    }

    pub fn attach_listener(&self, stream: u64, listener: FrameListener) {
        *lock(&self.listener) = Some((stream, listener));
    }

    /// Takes this stream's listener off; another stream's stays.
    pub fn detach_listener(&self, stream: u64) {
        let mut listener = lock(&self.listener);
        if listener.as_ref().is_some_and(|(attached, _)| *attached == stream) {
            *listener = None;
        }
    }

    pub fn listening(&self) -> bool {
        lock(&self.listener).is_some()
    }
}

impl CallEndpoint for HttpStationEndpoint {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> EndpointKind {
        EndpointKind::Station
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn deliver(&self, frame: &[i16]) {
        if let Some((_, listener)) = lock(&self.listener).as_mut() {
            listener(frame);
        }
    }

    fn notify(&self, _event: &CallEvent) {
        // The station hears its calls announced by voice; nothing to tell its audio client.
    }
}
