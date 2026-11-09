//! Accepts the station's voice WebSocket at UNIPROXY_PATH. The station offers zstd compression
//! (permessage_deflate_zstd); we decline it (axum negotiates no extension), which the station accepts.
//!
//! A station's socket is one task reading it (`pump`) and a writer task fed by its `Outbox`: every send of a session,
//! from whichever task, goes through the outbox in order.
//!
//! The socket is kept honest (`Liveness`, STATION_PING_MS): the writer pings the station, and the reader cuts the
//! socket once nothing at all has come from it for three pings. A station that rebooted or lost the network sends
//! no FIN or RST; the socket stays ESTABLISHED and the session «connected» until we happen to write (08.11.2026: 21
//! minutes, until a Yandex push). An idle station is never silent that long: it pings us every 5 s itself.
//!
//! The socket is taken ahead of the gateway's routes (`station_socket`), outside the Host check, as Node's 'upgrade'
//! listener was.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Request, State};
use axum::handler::Handler;
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::device_tag::device_tag_of;
use crate::app::App;
use crate::operations::networks::{in_networks, via_tunnel};
use crate::platform::http::Caller;

/// Connections one address may hold at once (the station keeps a few), and all of them together.
pub const PER_ADDRESS: usize = 16;
pub const IN_ALL: usize = 64;

/// The largest message a station may send: 16 MB.
const MAX_MESSAGE: usize = 16 * 1024 * 1024;

/// What the handshake says of a station's connection.
#[derive(Debug, Clone)]
pub struct Handshake {
    pub headers: HeaderMap,
    /// The socket's address, as the station connected from.
    pub remote_address: String,
}

/// What a session sends its station: frames in order, then perhaps the close.
pub enum Outgoing {
    Message(Message),
    Close(u16, String),
}

/// A session's way to its station's socket. Sends after the close (begun by either side) are dropped, as on a
/// WebSocket that is no longer open.
pub struct Outbox {
    sender: mpsc::UnboundedSender<Outgoing>,
    open: AtomicBool,
}

impl Outbox {
    pub fn new() -> (Arc<Outbox>, mpsc::UnboundedReceiver<Outgoing>) {
        let (sender, outgoing) = mpsc::unbounded_channel();
        (Arc::new(Outbox { sender, open: AtomicBool::new(true) }), outgoing)
    }

    /// The socket is open: frames still reach the station.
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    pub fn send_binary(&self, frame: Bytes) -> bool {
        self.is_open() && self.sender.send(Outgoing::Message(Message::Binary(frame))).is_ok()
    }

    pub fn send_text(&self, text: String) -> bool {
        self.is_open() && self.sender.send(Outgoing::Message(Message::Text(text.into()))).is_ok()
    }

    /// Closes the station's socket after what is queued; nothing more goes out.
    pub fn close(&self, code: u16, reason: &str) {
        if self.open.swap(false, Ordering::SeqCst) {
            let _ = self.sender.send(Outgoing::Close(code, reason.to_string()));
        }
    }

    /// The socket is gone.
    pub fn closed(&self) {
        self.open.store(false, Ordering::SeqCst);
    }
}

/// How long the station has to answer our close before its socket is cut, as ws's closeTimeout did.
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(30);

/// Pings in a row the station may leave without a word before its socket is cut.
const SILENT_PINGS: u32 = 3;

/// How a station's socket shows it is alive: our ping every `ping_every`, which the station answers (RFC 6455), and
/// anything at all from it — its own pings (one every 5 s, seen on the live socket on 08.11.2026), the pongs to ours,
/// its frames. Nothing for `silent_after` and the socket is gone. A ping written to a station that rebooted brings
/// back a reset at once; one that lost the network says nothing, and the silence ends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Liveness {
    pub ping_every: Duration,
    pub silent_after: Duration,
}

impl Liveness {
    /// STATION_PING_MS: a ping this often, a cut after three of them unanswered; 0 turns both off.
    pub fn every(ping_ms: u64) -> Option<Liveness> {
        let ping_every = Duration::from_millis(ping_ms);
        (ping_ms > 0).then(|| Liveness { ping_every, silent_after: ping_every * SILENT_PINGS })
    }
}

/// How the station's socket ended: its close code and reason (1005 for a close without a code, 1006 for a
/// connection that ended without one), and the socket's error when it broke on one (journaled as socketError).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketEnd {
    pub code: u16,
    pub reason: String,
    pub error: Option<String>,
}

impl SocketEnd {
    fn abnormal(error: Option<String>) -> SocketEnd {
        SocketEnd { code: 1006, reason: String::new(), error }
    }
}

/// The station's socket: its frames to `on_frame` as they come and ours from the outbox, until it closes, the
/// gateway stops (then it is cut, as `terminate` did), the station leaves our close unanswered for CLOSE_TIMEOUT, or
/// — with `liveness` — the station sends nothing at all for its `silent_after` (cut with 1006 and why).
pub async fn pump(
    socket: WebSocket,
    outgoing: mpsc::UnboundedReceiver<Outgoing>,
    stopping: CancellationToken,
    liveness: Option<Liveness>,
    on_frame: impl FnMut(Bytes, bool),
) -> SocketEnd {
    pump_closing_in(socket, outgoing, stopping, liveness, on_frame, CLOSE_TIMEOUT).await
}

/// Waits for the next ping, or forever without pings.
async fn next_ping(pings: &mut Option<tokio::time::Interval>) {
    match pings {
        Some(pings) => {
            pings.tick().await;
        }
        None => std::future::pending().await,
    }
}

/// «45 s», or «300 ms» for what is shorter than a second or not whole.
fn span(duration: Duration) -> String {
    if duration.as_millis().is_multiple_of(1000) {
        format!("{} s", duration.as_secs())
    } else {
        format!("{} ms", duration.as_millis())
    }
}

async fn pump_closing_in(
    socket: WebSocket,
    mut outgoing: mpsc::UnboundedReceiver<Outgoing>,
    stopping: CancellationToken,
    liveness: Option<Liveness>,
    mut on_frame: impl FnMut(Bytes, bool),
    close_timeout: Duration,
) -> SocketEnd {
    let (mut sink, mut stream) = socket.split();
    // Our close went out: from then on the station has `close_timeout` to answer it.
    let close_sent = CancellationToken::new();
    let sent = close_sent.clone();
    let writer = tokio::spawn(async move {
        // Our pings go out between the session's frames, none after our close.
        let mut pings = liveness.map(|liveness| {
            let first = tokio::time::Instant::now() + liveness.ping_every;
            let mut pings = tokio::time::interval_at(first, liveness.ping_every);
            pings.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            pings
        });
        loop {
            let next = tokio::select! {
                next = outgoing.recv() => next,
                _ = next_ping(&mut pings) => Some(Outgoing::Message(Message::Ping(Bytes::new()))),
            };
            match next {
                None => break,
                Some(Outgoing::Message(message)) => {
                    if sink.send(message).await.is_err() {
                        break;
                    }
                }
                Some(Outgoing::Close(code, reason)) => {
                    let _ = sink.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
                    sent.cancel();
                    break;
                }
            }
        }
    });
    let unanswered = async {
        close_sent.cancelled().await;
        tokio::time::sleep(close_timeout).await;
    };
    tokio::pin!(unanswered);
    // Put off by anything at all the station sends; off without liveness.
    let silent_after = liveness.map(|liveness| liveness.silent_after);
    let silence = tokio::time::sleep(silent_after.unwrap_or_default());
    tokio::pin!(silence);
    let end =
        loop {
            let next = tokio::select! {
                _ = stopping.cancelled() => break SocketEnd::abnormal(None),
                // A station gone silent (a half-open connection) never answers: ws cut such a socket, and its 'close'
                // said 1006.
                _ = &mut unanswered => break SocketEnd::abnormal(None),
                // Not even a pong to our pings: it rebooted or left the network without a word, and nothing would
                // ever tell us otherwise.
                _ = &mut silence, if silent_after.is_some() => break SocketEnd::abnormal(Some(format!(
                    "the station sent nothing for {}, not even a pong to our pings: the connection is gone",
                    span(silent_after.unwrap_or_default())
                ))),
                next = stream.next() => next,
            };
            if let (Some(after), Some(Ok(_))) = (silent_after, &next) {
                silence.as_mut().reset(tokio::time::Instant::now() + after);
            }
            match next {
                Some(Ok(Message::Binary(frame))) => on_frame(frame, true),
                Some(Ok(Message::Text(text))) => on_frame(Bytes::copy_from_slice(text.as_bytes()), false),
                Some(Ok(Message::Close(frame))) => {
                    // Read once more: the reply to the station's close goes out with it.
                    let _ = tokio::time::timeout(Duration::from_millis(500), stream.next()).await;
                    break frame.map_or(SocketEnd { code: 1005, reason: String::new(), error: None }, |frame| {
                        SocketEnd { code: frame.code, reason: frame.reason.to_string(), error: None }
                    });
                }
                Some(Ok(_)) => {}
                // ws's 'error' with its message (a message over the limit, a broken frame), then its 'close' with 1006.
                Some(Err(error)) => break SocketEnd::abnormal(Some(error.to_string())),
                None => break SocketEnd::abnormal(None),
            }
        };
    writer.abort();
    end
}

/// Middleware ahead of every route and of the Host check: an upgrade to UNIPROXY_PATH is the station's socket, as
/// Node's 'upgrade' listener took it before Express and its securityMiddleware saw the request. So a station that
/// reaches us by any name (a router's domain, a DDNS name) connects; the upgrade keeps its own checks. The path is
/// compared as it is, as Node compared the pathname; everything else goes on to the routes.
pub async fn station_socket(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    if request.uri().path() != app.config.gateway.uniproxy_path || !request.headers().contains_key(header::UPGRADE) {
        return next.run(request).await;
    }
    upgrade.call(request, app).await
}

fn refused(status: StatusCode) -> Response {
    status.into_response()
}

/// A browser's page: it opens a socket to any address it likes, and always says whose page it is; a station never does.
/// A page on the home network could otherwise pass for a station — a pending one in the panel, or worse.
pub fn from_a_page(headers: &HeaderMap) -> bool {
    headers.contains_key(header::ORIGIN)
}

/// The checks before the upgrade: the station is at home (nothing of it through a tunnel), it is no browser's page,
/// it does not open sockets without end, and it is one we know. The socket's place among the limits is taken here, at once with the count, and
/// held until its session is served (or the upgrade fails): sockets upgraded together cannot all pass one count.
async fn upgrade(State(app): State<Arc<App>>, caller: Caller, upgrade: WebSocketUpgrade) -> Response {
    let config = &app.config.gateway;
    if via_tunnel(&caller.headers) {
        tracing::warn!(target: "uniproxy", "rejected WebSocket upgrade to {} through a tunnel", config.uniproxy_path);
        return refused(StatusCode::FORBIDDEN);
    }
    let address = caller.peer.map(|peer: SocketAddr| peer.ip().to_string()).unwrap_or_default();
    if from_a_page(&caller.headers) {
        let origin = caller.headers.get(header::ORIGIN).and_then(|value| value.to_str().ok()).unwrap_or("?");
        tracing::warn!(target: "uniproxy", "rejected the station socket from {address}: a browser's page ({origin})");
        return refused(StatusCode::FORBIDDEN);
    }
    if !in_networks(&address, &config.debug_api_allow) {
        tracing::warn!(target: "uniproxy", "rejected the station socket from {address}: not the home network");
        return refused(StatusCode::FORBIDDEN);
    }
    let slot = match app.sessions.reserve(&address, PER_ADDRESS, IN_ALL) {
        Ok(slot) => slot,
        Err((in_all, from_here)) => {
            tracing::warn!(
                target: "uniproxy",
                "rejected the station socket from {address}: too many connections ({from_here} from it, {in_all} in all)"
            );
            return refused(StatusCode::TOO_MANY_REQUESTS);
        }
    };
    let device = device_tag_of(&caller.headers, config.recorder.redact);
    let admitted = app.stations.admit(&device, &address);
    if !admitted.ok {
        tracing::warn!(
            target: "uniproxy",
            "rejected station {device} from {address}: {} — approve it in the panel («Колонки»)",
            admitted.status.as_str()
        );
        return refused(StatusCode::FORBIDDEN);
    }
    let sessions = app.sessions.clone();
    let handshake = Handshake { headers: caller.headers.clone(), remote_address: address };
    upgrade
        .max_message_size(MAX_MESSAGE)
        .on_upgrade(move |socket| async move { sessions.serve(socket, handshake, slot).await })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::routing::get;
    use axum::Router;
    use tokio_tungstenite::tungstenite;

    use super::*;

    type Client = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

    /// A server whose one socket runs `pump` (closing in `close_timeout`, after `close` if given, messages up to 1 KB)
    /// and reports how it ended; a client that connects to it.
    async fn socket_end(
        close: Option<(u16, &'static str)>,
        close_timeout: Duration,
    ) -> (Client, tokio::sync::oneshot::Receiver<SocketEnd>) {
        socket_living(close, close_timeout, None).await
    }

    /// The same with the socket's liveness.
    async fn socket_living(
        close: Option<(u16, &'static str)>,
        close_timeout: Duration,
        liveness: Option<Liveness>,
    ) -> (Client, tokio::sync::oneshot::Receiver<SocketEnd>) {
        let (report, ended) = tokio::sync::oneshot::channel();
        let report = Arc::new(Mutex::new(Some(report)));
        let route = get(move |upgrade: WebSocketUpgrade| async move {
            upgrade.max_message_size(1024).on_upgrade(move |socket| async move {
                let (outbox, outgoing) = Outbox::new();
                if let Some((code, reason)) = close {
                    outbox.close(code, reason);
                }
                let stopping = CancellationToken::new();
                let end = pump_closing_in(socket, outgoing, stopping, liveness, |_, _| {}, close_timeout).await;
                if let Some(report) = report.lock().unwrap().take() {
                    let _ = report.send(end);
                }
            })
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, Router::new().route("/", route)).await });
        let (client, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/")).await.unwrap();
        (client, ended)
    }

    /// A ping every 100 ms, a cut after 300 ms of silence.
    fn quick() -> Option<Liveness> {
        Liveness::every(100)
    }

    #[test]
    fn a_browsers_page_says_whose_it_is_and_a_station_does_not() {
        let mut station = HeaderMap::new();
        station.insert("x-uniproxy-device-id", "abc".parse().unwrap());
        assert!(!from_a_page(&station));
        let mut page = station.clone();
        page.insert(header::ORIGIN, "http://evil.example".parse().unwrap());
        assert!(from_a_page(&page));
    }

    #[tokio::test]
    async fn cuts_a_station_that_leaves_our_close_unanswered_with_1006() {
        // The client never reads, so it never answers the close: a station gone silent.
        let (_client, ended) = socket_end(Some((1000, "mode switch")), Duration::from_millis(200)).await;
        let end = tokio::time::timeout(Duration::from_secs(5), ended).await.expect("the socket is cut").unwrap();
        assert_eq!(end, SocketEnd { code: 1006, reason: String::new(), error: None });
    }

    #[tokio::test]
    async fn ends_with_the_stations_answer_to_our_close() {
        let (mut client, ended) = socket_end(Some((1000, "mode switch")), Duration::from_secs(30)).await;
        // Reading on past the close answers it.
        while let Some(Ok(_)) = client.next().await {}
        let end = tokio::time::timeout(Duration::from_secs(5), ended).await.expect("the socket closes").unwrap();
        assert_eq!((end.code, end.error), (1000, None));
    }

    #[tokio::test]
    async fn keeps_the_sockets_error_when_it_breaks_on_one() {
        let (mut client, ended) = socket_end(None, CLOSE_TIMEOUT).await;
        client.send(tungstenite::Message::Binary(vec![0u8; 4096].into())).await.unwrap();
        let end = tokio::time::timeout(Duration::from_secs(5), ended).await.expect("the socket ends").unwrap();
        assert_eq!(end.code, 1006);
        let error = end.error.expect("the error is kept");
        assert!(error.to_lowercase().contains("too long") || error.to_lowercase().contains("size"), "{error}");
    }

    #[test]
    fn pings_as_often_as_set_and_gives_up_after_three_unanswered_or_never_when_off() {
        assert_eq!(
            Liveness::every(15_000),
            Some(Liveness { ping_every: Duration::from_secs(15), silent_after: Duration::from_secs(45) })
        );
        assert_eq!(Liveness::every(0), None);
        assert_eq!(span(Duration::from_secs(45)), "45 s");
        assert_eq!(span(Duration::from_millis(300)), "300 ms");
    }

    #[tokio::test]
    async fn cuts_a_station_that_vanished_without_a_close_and_says_why() {
        // The client never reads, so it never answers a ping: a station that rebooted or left the network, its
        // socket still ESTABLISHED here. Without liveness nothing would ever end it.
        let (_client, ended) = socket_living(None, CLOSE_TIMEOUT, quick()).await;
        let end = tokio::time::timeout(Duration::from_secs(5), ended).await.expect("the socket is cut").unwrap();
        assert_eq!(end.code, 1006);
        let error = end.error.expect("why it was cut");
        assert!(error.contains("sent nothing for 300 ms"), "{error}");
    }

    #[tokio::test]
    async fn holds_an_idle_station_that_answers_our_pings() {
        let (client, mut ended) = socket_living(None, CLOSE_TIMEOUT, quick()).await;
        // It sends nothing of its own, only reads, and its socket answers each ping, as RFC 6455 has it.
        let pings = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = pings.clone();
        let (_sink, mut incoming) = client.split();
        tokio::spawn(async move {
            while let Some(Ok(message)) = incoming.next().await {
                if message.is_ping() {
                    counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
        });
        // Five times the silence that cuts a vanished one.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(ended.try_recv().is_err(), "an idle station that answers was cut");
        let pinged = pings.load(std::sync::atomic::Ordering::SeqCst);
        assert!(pinged >= 5, "pinged {pinged} times in 1.5 s, every 100 ms");
    }

    #[tokio::test]
    async fn takes_the_stations_own_pings_for_life_even_with_ours_unread() {
        // The real station pings every 5 s. This one never reads (ours go unanswered), but pings every 100 ms.
        let (mut client, mut ended) = socket_living(None, CLOSE_TIMEOUT, quick()).await;
        for _ in 0..12 {
            client.send(tungstenite::Message::Ping(Bytes::new())).await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(ended.try_recv().is_err(), "a station that pings was cut");
        // Then it falls silent: gone within the silence.
        let end = tokio::time::timeout(Duration::from_secs(5), ended).await.expect("the socket is cut").unwrap();
        assert_eq!(end.code, 1006);
    }
}
