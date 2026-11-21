//! The link's helper side (docs/plans/home-and-helpers.md, stage 7; pc): a computer that helps listens on a port of
//! the link's own (`LINK_PORT`, PORT + 1 by default), ends TLS there itself with a certificate it made at its first
//! start, and serves nothing but `/home/v1/link` behind it — so a port forwarded for the outside puts no panel and no
//! plain HTTP on the internet. A proxy on this computer (or in `LINK_PROXY_FROM`: one proxy container's address or a
//! small compose network on this machine) may hand the link on to the main port in plain HTTP instead (`link_socket`),
//! for the site «Адрес снаружи» names: without it no pairing comes that way. On that plain hop a later link shows only
//! its key's id and a proof that opens nothing again, and a pairing's answer carries the new link key wrapped.
//!
//! - **Pairing**: within 15 minutes of «Подключить шлюз» an admin opened (a new key each time it is opened again after
//!   that; re-reads never move that end on), a gateway that proves the connection key over this certificate's
//!   fingerprint (or «Адрес снаружи»'s name, behind a proxy) gets a link key for its home; the key is spent only once
//!   the gateway confirms, and a new one is shown. While no key is taken a pairing socket is refused (4401) after its
//!   first frame, holding no pairing's place; a socket that keeps silent, or is not the exchange, gives its place up
//!   and counts against its source.
//! - **Key mode**: the upgrade names a confirmed home's link key by its id (an unknown one: the same empty 401 as a
//!   removed home's), then proves the key over a challenge (`link_proof`) within 3 s, before it takes a place, bound to
//!   what its pairing bound; a wrong proof is closed with 4401 (the gateway stops), a late or missing one with 4408 (it
//!   tries again). All count against the source: twenty failures in ten minutes from one place make it wait (429).
//!   Another link version is told 426 before any socket.
//! - **Serving**: each request frame runs through `/home/v1` as a request of that home (`LinkOrigin`: outside the
//!   home, with only what this computer gives it), its answer streamed back; the status is told when it changes.
//! - **Up** (stage 7 of docs/plans/home-setup-flow.md): a home whose gateway said at its upgrade that it takes them
//!   (`UP_HEADER`) gets this computer's own requests over the same link — the pass to that home's panel
//!   (placement/proxy.rs) for the person signed in here. Their answers come back as `up-res` and pieces.

use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Router;
use futures::{SinkExt, Stream, StreamExt};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinSet;
use tokio::time::{Instant, Sleep};
use tokio_rustls::server::TlsStream;
use tokio_util::sync::CancellationToken;

use super::clients::{kept_name, Clients, LinkClient, Names, MUSCLES};
use super::contract::{Abilities, HelperIdentity};
use super::link::{
    answer, bound_host, decode_chunk, headers_from, link_proof, read_text, same, sha256, up_piece, wrapped_link_key,
    Binding, ConnectionKey, Out, Session, Transcript, Way, Wire, WireSink, WireStream, BODY_MAX, BUSY, CARRIED, DONE,
    FRAME_MAX, GATEWAY_HEADER, GATEWAY_NAME_HEADER, GOING, HOME_HEADER, LINK_ID_HEADER, PAIR_FRAME_MAX, PAIR_HEADER,
    PATH, PROTOCOL, REFUSED, SILENT, UP_BIT, UP_HEADER, VERSION, VERSION_HEADER,
};
use crate::app::App;
use crate::config::Config;
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::core::identity::home::HOME_KEY;
use crate::home_server::serve::{status_of, Given};
use crate::operations::networks::{beyond, in_list, in_networks, invalid_networks, wider_than, LOOPBACK};
use crate::platform::http::LinkOrigin;
use crate::secrets::SecretsService;
use crate::settings::definitions::HELPER_OUTSIDE;
use crate::settings::SettingsService;
use crate::utils::time::now_iso;

/// How long a connection key is taken after an admin opened «Подключить шлюз» or changed it (owner, 15.11).
pub const ARMED_FOR: Duration = Duration::from_secs(15 * 60);
/// The secrets of this computer as a helper: none of them travels (`helping:` is no travelling prefix).
const KEY_SECRET: &str = "helping:link";
const TLS_SECRET: &str = "helping:tls";
/// Connections to the link port, in all and from one source; TLS handshakes at once, in all and from one source, the
/// time a connection has to say its ClientHello, and the whole handshake's.
const CONNECTIONS: usize = 128;
const CONNECTIONS_PER_SOURCE: usize = 8;
const HANDSHAKES: usize = 32;
const HANDSHAKES_PER_SOURCE: usize = 2;
const HELLO: Duration = Duration::from_secs(3);
const HANDSHAKE: Duration = Duration::from_secs(10);
/// From the end of the TLS handshake to the upgrade.
const TO_UPGRADE: Duration = Duration::from_secs(3);
/// From a link's upgrade to its proof of the key.
const PROOF: Duration = Duration::from_secs(3);
/// Links, in all and from one source; one home's links (as many as the gateways it keeps).
const LINKS: usize = 64;
const LINKS_PER_SOURCE: usize = 8;
const LINKS_PER_CLIENT: usize = super::clients::GATEWAYS;
/// Pairing sockets at once; failed pairings from one source in the window before it waits.
const PAIRINGS: usize = 8;
const PAIR_FAILURES: usize = 10;
/// A pairing's first frame, after the upgrade: a gateway says it at once; a socket that keeps silent gives its place up.
const PAIR_FIRST: Duration = Duration::from_secs(3);
/// A pairing's whole exchange.
const PAIR_EXCHANGE: Duration = Duration::from_secs(10);
/// Wrong proofs of the key logged in the window in all, whatever their sources: no number of places fills the log.
const REFUSALS_LOGGED: usize = 16;
/// This machine itself, where a proxy needs no entry in `LINK_PROXY_FROM`. Any other entry wider than one proxy
/// container's address or a small fixed compose network (`PROXY_RANGE_V4` and `_V6` bits) is warned of at the start:
/// whoever holds an address in it hands the link on as a proxy would — naming whom it came for —, and the link's
/// requests and answers cross that hop in plain HTTP.
const ON_THIS_MACHINE: &[&str] = &["127.0.0.0/8", "::1/128"];
const PROXY_RANGE_V4: u32 = 24;
const PROXY_RANGE_V6: u32 = 64;
/// Wrong link keys over plain HTTP waiting their second at once; past them refused at once with 429. Plain links between
/// their upgrade and their proof at once (the link port's connections count its own); past them 429 before the upgrade.
const PLAIN_WAITS: usize = 16;
const PLAIN_UNPROVEN: usize = 32;
/// Wrong link keys from one source in the window before it waits; the sources kept.
const KEY_FAILURES: usize = 20;
const WINDOW: Duration = Duration::from_secs(600);
const SOURCES: usize = 4096;
/// Requests running on one link; open on it before it is closed as a flood.
const RUNNING: usize = 32;
const OPEN: usize = 64;
/// Request bodies received for requests still running — their handlers may hold them whole —: per link, per home over
/// all its links, and on this computer in all.
const BODIES_PER_LINK: usize = 32 * 1024 * 1024;
const BODIES_PER_CLIENT: usize = 64 * 1024 * 1024;
const BODIES_IN_ALL: usize = 256 * 1024 * 1024;
static BODIES: AtomicUsize = AtomicUsize::new(0);
/// A link silent this long is closed; each link's client looked at again this often (removed: cut), its status too;
/// status events merged to one per `STATUS_EVERY`; a request body that brings nothing this long is given up, and one
/// still coming this long after its request began, however it trickles.
const SILENCE: Duration = Duration::from_secs(45);
const RECHECK: Duration = Duration::from_secs(15);
const STATUS_EVERY: Duration = Duration::from_secs(5);
const BODY_SILENCE: Duration = Duration::from_secs(30);
const BODY_WHOLE: Duration = Duration::from_secs(5 * 60);

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn take(counter: &AtomicUsize, amount: usize) {
    let mut left = counter.load(Ordering::SeqCst);
    while let Err(now) =
        counter.compare_exchange_weak(left, left.saturating_sub(amount), Ordering::SeqCst, Ordering::SeqCst)
    {
        left = now;
    }
}

/// The place failures and connections are counted by: an IPv4 address, an IPv6 network's /56 — what one home or one
/// rented server is usually given, so its addresses are one place.
pub fn source_of(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => {
                let segments = v6.segments();
                format!("{:x}:{:x}:{:x}:{:x}::/56", segments[0], segments[1], segments[2], segments[3] & 0xff00)
            }
        },
    }
}

/// The place a plain request is counted by: the client its proxy wrote, an IPv6 one by its /56 as on the link port
/// (`source_of`) — one network's addresses never make many places —; "proxied" when it wrote none that agrees.
fn proxied_source(headers: &HeaderMap) -> String {
    let source = crate::platform::http::zone::forwarded_source(headers);
    match source.strip_prefix("proxied:").and_then(|ip| ip.parse::<IpAddr>().ok()) {
        Some(ip) => format!("proxied:{}", source_of(ip)),
        None => source,
    }
}

/// The entries of `LINK_PROXY_FROM` warned of at the start: not this machine, and wider than one proxy container's
/// address or a small compose network.
fn wide_proxies(entries: &[String]) -> Vec<String> {
    wider_than(&beyond(entries, ON_THIS_MACHINE), PROXY_RANGE_V4, PROXY_RANGE_V6)
}

// ── The certificate ──

/// The helper's own TLS identity: an ECDSA P-256 key made by ring and a minimal self-signed certificate written
/// here, kept in the secret `helping:tls`. Its SHA-256 is what gateways pin.
pub struct TlsIdentity {
    pub cert: Vec<u8>,
    key: Vec<u8>,
    pub fingerprint: [u8; 32],
}

impl TlsIdentity {
    /// The one kept, else one made now and kept.
    pub fn load_or_make(secrets: &SecretsService) -> anyhow::Result<TlsIdentity> {
        let b64 = base64::engine::general_purpose::STANDARD;
        use base64::Engine;
        if let Some(stored) = secrets.get(TLS_SECRET) {
            let value: Value = serde_json::from_str(&stored)?;
            let field = |name: &str| -> anyhow::Result<Vec<u8>> {
                Ok(b64.decode(value.get(name).and_then(Value::as_str).unwrap_or(""))?)
            };
            let (cert, key) = (field("cert")?, field("key")?);
            let identity = TlsIdentity { fingerprint: sha256(&cert), cert, key };
            identity.server_config()?;
            return Ok(identity);
        }
        let identity = TlsIdentity::make()?;
        let stored = json!({ "cert": b64.encode(&identity.cert), "key": b64.encode(&identity.key) });
        secrets.set(TLS_SECRET, Some(&stored.to_string())).map_err(|error| anyhow::anyhow!(error.message))?;
        Ok(identity)
    }

    pub fn make() -> anyhow::Result<TlsIdentity> {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
            .map_err(|_| anyhow::anyhow!("no key pair"))?;
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
            .map_err(|error| anyhow::anyhow!("the key pair does not read: {error}"))?;
        let cert = self_signed(&pair, &rng)?;
        Ok(TlsIdentity { fingerprint: sha256(&cert), cert, key: pkcs8.as_ref().to_vec() })
    }

    /// TLS 1.3 only (both ends are ours), HTTP/1.1, no tickets and no session cache: every link is a full handshake.
    pub fn server_config(&self) -> anyhow::Result<Arc<ServerConfig>> {
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone()));
        let mut config = ServerConfig::builder_with_provider(super::link::provider())
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(self.cert.clone())], key)?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        config.send_tls13_tickets = 0;
        config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
        Ok(Arc::new(config))
    }
}

/// A DER element: its tag, its length, its body.
fn der(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    match body.len() {
        length if length < 0x80 => out.push(length as u8),
        length if length < 0x100 => out.extend([0x81, length as u8]),
        length => out.extend([0x82, (length >> 8) as u8, length as u8]),
    }
    out.extend_from_slice(body);
    out
}

const ECDSA_WITH_SHA256: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const EC_PUBLIC_KEY: &[u8] = &[0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const PRIME256V1: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const COMMON_NAME: &[u8] = &[0x06, 0x03, 0x55, 0x04, 0x03];

/// A minimal X.509 v3 certificate for the key pair, signed by itself: a random serial, a random 16-hex-character
/// name (no product name in it), valid from yesterday for thirty years, no extensions — what rustls and webpki read.
/// The tests make their impostor's certificate the same way.
pub fn self_signed(pair: &EcdsaKeyPair, rng: &dyn SecureRandom) -> anyhow::Result<Vec<u8>> {
    let mut serial = [0u8; 16];
    rng.fill(&mut serial).map_err(|_| anyhow::anyhow!("no random serial"))?;
    // Positive and minimal: the top bit cleared, the first byte never zero.
    serial[0] = (serial[0] & 0x7f) | 0x40;
    let mut random = [0u8; 8];
    rng.fill(&mut random).map_err(|_| anyhow::anyhow!("no random name"))?;
    let common = hex::encode(random);
    let name = der(0x30, &der(0x31, &der(0x30, &[COMMON_NAME, &der(0x0c, common.as_bytes())].concat())));
    let algorithm = der(0x30, ECDSA_WITH_SHA256);
    let now = chrono::Utc::now();
    let from = (now - chrono::TimeDelta::days(1)).format("%y%m%d%H%M%SZ").to_string();
    let until = (now + chrono::TimeDelta::days(365 * 30)).format("%Y%m%d%H%M%SZ").to_string();
    let validity = der(0x30, &[der(0x17, from.as_bytes()), der(0x18, until.as_bytes())].concat());
    let point = pair.public_key().as_ref();
    let key_info = der(
        0x30,
        &[der(0x30, &[EC_PUBLIC_KEY, PRIME256V1].concat()), der(0x03, &[&[0u8][..], point].concat())].concat(),
    );
    let tbs = der(
        0x30,
        &[der(0xa0, &der(0x02, &[2])), der(0x02, &serial), algorithm.clone(), name.clone(), validity, name, key_info]
            .concat(),
    );
    let signature = pair.sign(rng, &tbs).map_err(|_| anyhow::anyhow!("the certificate is not signed"))?;
    Ok(der(0x30, &[tbs, algorithm, der(0x03, &[&[0u8][..], signature.as_ref()].concat())].concat()))
}

// ── The link port's listener ──

/// Counted places: connections or links, in all, per source and (links) per home; a place given back when its permit
/// goes. All three are counted under one lock: parallel takes never pass a limit together.
pub struct Limits {
    counts: Mutex<Counts>,
    in_all: usize,
    per_source: usize,
    per_client: usize,
}

#[derive(Default)]
struct Counts {
    all: usize,
    sources: HashMap<String, usize>,
    clients: HashMap<String, usize>,
}

fn count_in(places: &mut HashMap<String, usize>, key: &str) {
    *places.entry(key.to_string()).or_default() += 1;
}

fn count_out(places: &mut HashMap<String, usize>, key: &str) {
    if let Some(here) = places.get_mut(key) {
        *here = here.saturating_sub(1);
        if *here == 0 {
            places.remove(key);
        }
    }
}

impl Limits {
    pub fn new(in_all: usize, per_source: usize) -> Arc<Limits> {
        Limits::with_clients(in_all, per_source, usize::MAX)
    }

    pub fn with_clients(in_all: usize, per_source: usize, per_client: usize) -> Arc<Limits> {
        Arc::new(Limits { counts: Mutex::default(), in_all, per_source, per_client })
    }

    pub fn take(self: &Arc<Self>, source: &str) -> Option<Permit> {
        self.take_for(source, None)
    }

    /// A place for a source, and for a home when given.
    pub fn take_for(self: &Arc<Self>, source: &str, client: Option<&str>) -> Option<Permit> {
        let mut counts = locked(&self.counts);
        let here = counts.sources.get(source).copied().unwrap_or(0);
        let held = client.map_or(0, |client| counts.clients.get(client).copied().unwrap_or(0));
        if counts.all >= self.in_all || here >= self.per_source || held >= self.per_client {
            return None;
        }
        counts.all += 1;
        count_in(&mut counts.sources, source);
        if let Some(client) = client {
            count_in(&mut counts.clients, client);
        }
        Some(Permit { limits: self.clone(), source: source.to_string(), client: client.map(str::to_string) })
    }
}

/// A place taken among `Limits`.
pub struct Permit {
    limits: Arc<Limits>,
    source: String,
    client: Option<String>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut counts = locked(&self.limits.counts);
        counts.all = counts.all.saturating_sub(1);
        count_out(&mut counts.sources, &self.source);
        if let Some(client) = &self.client {
            count_out(&mut counts.clients, client);
        }
    }
}

/// The time a link connection has to ask for its upgrade, from the end of its TLS handshake; and its place among the
/// connections, held until a link proves its key (from then on LINKS counts it) or a pairing socket ends.
pub struct Deadline {
    armed: AtomicBool,
    permit: Mutex<Option<Permit>>,
}

impl Deadline {
    /// The upgrade came in time.
    pub fn disarm(&self) {
        self.armed.store(false, Ordering::SeqCst);
    }

    /// The connection's place given back.
    pub fn release(&self) {
        locked(&self.permit).take();
    }
}

/// One connection of the link port: its TLS stream, and the deadline until its upgrade — reads and writes fail after it
/// unless the upgrade disarmed it — with its place.
pub struct LinkIo {
    stream: TlsStream<TcpStream>,
    deadline: Arc<Deadline>,
    expires: Pin<Box<Sleep>>,
}

impl LinkIo {
    fn expired(&mut self, cx: &mut Context<'_>) -> bool {
        self.deadline.armed.load(Ordering::SeqCst) && self.expires.as_mut().poll(cx).is_ready()
    }
}

fn too_late() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "the link was not asked for in time")
}

impl AsyncRead for LinkIo {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if self.expired(cx) {
            return Poll::Ready(Err(too_late()));
        }
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for LinkIo {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        if self.expired(cx) {
            return Poll::Ready(Err(too_late()));
        }
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

/// The link port's listener: TLS ended here, handshakes in parallel (at most 32, two from one source, the ClientHello
/// within 3 s and the whole within 10 s), every connection within the places of `Limits`. Dropped at shutdown, it frees
/// the port at once and drops the handshakes going on.
pub struct LinkListener {
    listener: TcpListener,
    tls: Arc<ServerConfig>,
    handshakes: JoinSet<Option<(LinkIo, SocketAddr)>>,
    limits: Arc<Limits>,
    shaking: Arc<Limits>,
    /// From the TLS handshake's end to the upgrade, and for the TLS handshake (`TO_UPGRADE`, `HANDSHAKE`; tests
    /// shorten them).
    to_upgrade: Duration,
    handshake: Duration,
}

impl LinkListener {
    pub fn new(listener: TcpListener, tls: Arc<ServerConfig>, limits: Arc<Limits>) -> LinkListener {
        LinkListener {
            listener,
            tls,
            handshakes: JoinSet::new(),
            limits,
            shaking: Limits::new(HANDSHAKES, HANDSHAKES_PER_SOURCE),
            to_upgrade: TO_UPGRADE,
            handshake: HANDSHAKE,
        }
    }

    /// For tests: shorter times to the TLS handshake and the upgrade.
    pub fn with_times(mut self, handshake: Duration, to_upgrade: Duration) -> LinkListener {
        self.handshake = handshake;
        self.to_upgrade = to_upgrade;
        self
    }
}

impl axum::serve::Listener for LinkListener {
    type Io = LinkIo;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (LinkIo, SocketAddr) {
        loop {
            tokio::select! {
                accepted = self.listener.accept() => match accepted {
                    Ok((tcp, address)) => {
                        let source = source_of(address.ip());
                        let Some(permit) = self.limits.take(&source) else { continue };
                        // Handshakes going on: a few sources holding silent sockets never take them all.
                        let Some(shaking) = self.shaking.take(&source) else { continue };
                        let (tls, handshake, to_upgrade) = (self.tls.clone(), self.handshake, self.to_upgrade);
                        let accepted = Instant::now();
                        self.handshakes.spawn(async move {
                            let _ = tcp.set_nodelay(true);
                            let hello = tokio_rustls::LazyConfigAcceptor::new(rustls::server::Acceptor::default(), tcp);
                            let start = tokio::time::timeout(HELLO.min(handshake), hello).await.ok()?.ok()?;
                            let stream =
                                tokio::time::timeout_at(accepted + handshake, start.into_stream(tls)).await.ok()?.ok()?;
                            drop(shaking);
                            let deadline =
                                Arc::new(Deadline { armed: AtomicBool::new(true), permit: Mutex::new(Some(permit)) });
                            let expires = Box::pin(tokio::time::sleep(to_upgrade));
                            Some((LinkIo { stream, deadline, expires }, address))
                        });
                    }
                    Err(error) => {
                        // As axum meets them: a connection's own error is nothing; running out of files waits a little.
                        if !matches!(
                            error.kind(),
                            io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset
                        ) {
                            tracing::warn!("link: accept: {error}");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                },
                Some(done) = self.handshakes.join_next(), if !self.handshakes.is_empty() => {
                    if let Ok(Some(connection)) = done {
                        return connection;
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

/// Over which transport a link request came: the link port's TLS, or plain HTTP from a proxy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Plain,
    Tls,
}

/// Per connection of the link port: the link's router with the peer, the deadline and the transport.
#[derive(Clone)]
pub struct LinkService {
    router: Router,
}

impl<'a> tower::Service<axum::serve::IncomingStream<'a, LinkListener>> for LinkService {
    type Response = Router;
    type Error = Infallible;
    type Future = std::future::Ready<Result<Router, Infallible>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, stream: axum::serve::IncomingStream<'a, LinkListener>) -> Self::Future {
        let router = self
            .router
            .clone()
            .layer(axum::Extension(ConnectInfo(*stream.remote_addr())))
            .layer(axum::Extension(stream.io().deadline.clone()))
            .layer(axum::Extension(Transport::Tls));
        std::future::ready(Ok(router))
    }
}

/// The link port's one route; everything else is an empty 404.
fn link_router(app: Arc<App>) -> Router {
    Router::new()
        .route(PATH, axum::routing::get(on_link_port))
        .fallback(|| async { StatusCode::NOT_FOUND })
        .with_state(app)
}

async fn on_link_port(State(app): State<Arc<App>>, request: Request) -> Response {
    upgrade(app, Transport::Tls, request).await
}

/// The link port bound beside the main one — on the same address —, on a computer that helps and has its
/// certificate; a port taken is said on the key sheet, and the gateway serves without the link.
pub async fn bind(app: &Arc<App>, main: &TcpListener) -> Option<LinkListener> {
    let server = &app.link;
    if !server.helps() {
        return None;
    }
    let tls = server.tls()?;
    let port = app.config.app.link_port?;
    let ip = main.local_addr().map(|address| address.ip()).unwrap_or(IpAddr::from([0, 0, 0, 0]));
    match TcpListener::bind((ip, port)).await {
        Ok(listener) => {
            let bound = listener.local_addr().map(|address| address.port()).unwrap_or(port);
            server.set_port(bound);
            tracing::info!("link: listening on port {bound} (TLS)");
            Some(LinkListener::new(listener, tls, server.connections.clone()))
        }
        Err(error) => {
            tracing::warn!("link: port {port} is not free: {error}");
            server.set_problem(crate::tr!(
                "Порт связи {port} занят: другие дома пока не подключатся к этому компьютеру. Освободите порт или задайте LINK_PORT в .env.",
                port = port
            ));
            None
        }
    }
}

/// The link port served until `stop`.
pub async fn serve(
    app: Arc<App>,
    listener: LinkListener,
    stop: impl std::future::Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    axum::serve(listener, LinkService { router: link_router(app) }).with_graceful_shutdown(stop).await
}

/// The soft limit of open files raised to 4096 (or the hard one): the desktop app starts with 256 on a Mac, and the
/// link's connections share them with the gateway's own.
pub fn raise_open_files() {
    #[cfg(unix)]
    {
        let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: plain system calls on a struct of our own.
        unsafe {
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == 0 {
                let wanted = limit.rlim_max.min(4096);
                if limit.rlim_cur < wanted {
                    limit.rlim_cur = wanted;
                    libc::setrlimit(libc::RLIMIT_NOFILE, &limit);
                }
            }
        }
    }
}

// ── Throttles ──

/// Failures per source within a window: a source with `limit` of them waits until they age out. At most `SOURCES`
/// sources are kept; the one failing longest ago goes first.
pub struct LinkThrottle {
    failures: Mutex<HashMap<String, VecDeque<Instant>>>,
    limit: usize,
    window: Duration,
}

impl LinkThrottle {
    pub fn new(limit: usize, window: Duration) -> LinkThrottle {
        LinkThrottle { failures: Mutex::new(HashMap::new()), limit, window }
    }

    pub fn blocked(&self, source: &str) -> bool {
        let now = Instant::now();
        let mut failures = locked(&self.failures);
        let Some(times) = failures.get_mut(source) else { return false };
        while times.front().is_some_and(|at| now.duration_since(*at) >= self.window) {
            times.pop_front();
        }
        if times.is_empty() {
            failures.remove(source);
            return false;
        }
        times.len() >= self.limit
    }

    pub fn fail(&self, source: &str) {
        let now = Instant::now();
        let mut failures = locked(&self.failures);
        let times = failures.entry(source.to_string()).or_default();
        times.push_back(now);
        while times.len() > self.limit {
            times.pop_front();
        }
        if failures.len() > SOURCES {
            let oldest =
                failures.iter().min_by_key(|(_, times)| times.back().copied()).map(|(source, _)| source.clone());
            if let Some(oldest) = oldest {
                failures.remove(&oldest);
            }
        }
    }

    pub fn sources(&self) -> usize {
        locked(&self.failures).len()
    }
}

/// The pairing sockets: at most 4 at once and one per source; ten failures in ten minutes and a source waits; a wrong
/// proof of the key logged at most once per source, and `REFUSALS_LOGGED` in all, per ten minutes.
struct PairGate {
    open: Mutex<HashSet<String>>,
    failures: LinkThrottle,
    logged: Mutex<Logged>,
}

/// The wrong proofs logged in the window, by source; `full` once one more was passed by meanwhile (said once).
#[derive(Default)]
struct Logged {
    at: HashMap<String, Instant>,
    full: bool,
}

/// A pairing socket's place.
struct PairTicket {
    gate: Arc<LinkServerShared>,
    source: String,
}

impl Drop for PairTicket {
    fn drop(&mut self) {
        locked(&self.gate.pairing.open).remove(&self.source);
    }
}

// ── The helper's link server ──

/// What the link's tasks share of the server.
struct LinkServerShared {
    pairing: PairGate,
}

/// The connection key now: its shown form's key, and since when.
struct KeyState {
    key: ConnectionKey,
    since: Option<String>,
}

/// A live link: whose, its way to be cut, and — when its home's gateway takes them — this computer's own requests to it
/// (`up`).
struct LiveLink {
    client: String,
    cut: CancellationToken,
    up: Option<Arc<Session>>,
}

/// The helper's side of the link on this computer.
pub struct LinkServer {
    tls: Option<Arc<ServerConfig>>,
    fingerprint: Option<[u8; 32]>,
    pub(crate) clients: Arc<Clients>,
    secrets: Arc<SecretsService>,
    settings: Arc<SettingsService>,
    events: EventBus,
    config: Arc<Config>,
    app: OnceLock<Weak<App>>,
    helps: bool,
    port: AtomicU16,
    problem: Mutex<Option<String>>,
    key: Mutex<Option<KeyState>>,
    armed_until: Mutex<Option<Instant>>,
    shared: Arc<LinkServerShared>,
    throttle: LinkThrottle,
    /// Wrong link keys over plain HTTP waiting their second (`PLAIN_WAITS`).
    plain_waits: Arc<tokio::sync::Semaphore>,
    /// Plain links not proven yet (`PLAIN_UNPROVEN`).
    plain_unproven: Arc<tokio::sync::Semaphore>,
    connections: Arc<Limits>,
    links: Arc<Limits>,
    /// The bytes of the bodies running for each home, over all its links (`BODIES_PER_CLIENT`).
    client_bodies: Mutex<HashMap<String, Arc<AtomicUsize>>>,
    live: Mutex<HashMap<u64, LiveLink>>,
    next_live: AtomicU64,
    /// Whether each home's gateway took this computer's own requests (`up`) at its last link since the start.
    centres: Mutex<HashMap<String, bool>>,
    stopping: CancellationToken,
    nudge: Arc<Notify>,
}

impl LinkServer {
    /// Never fails: a certificate that cannot be made or read is logged and leaves the link off, said on the key sheet.
    pub fn new(
        config: Arc<Config>,
        secrets: Arc<SecretsService>,
        settings: Arc<SettingsService>,
        events: EventBus,
        clients: Arc<Clients>,
        helps: bool,
    ) -> LinkServer {
        let mut problem = None;
        let identity = if helps {
            match TlsIdentity::load_or_make(&secrets) {
                Ok(identity) => Some(identity),
                Err(error) => {
                    tracing::error!("link: the certificate is not made: {error:#}");
                    problem = Some(crate::tr!("Сертификат связи не создан: {error}", error = format!("{error:#}")));
                    None
                }
            }
        } else {
            None
        };
        let tls = identity.as_ref().and_then(|identity| match identity.server_config() {
            Ok(tls) => Some(tls),
            Err(error) => {
                tracing::error!("link: the certificate does not serve: {error:#}");
                problem = Some(crate::tr!("Сертификат связи не создан: {error}", error = format!("{error:#}")));
                None
            }
        });
        let typos = invalid_networks(&config.app.link_proxy_from);
        if !typos.is_empty() {
            tracing::warn!("link: LINK_PROXY_FROM has entries that are no network, passed by: {}", typos.join(", "));
        }
        let wide = wide_proxies(&config.app.link_proxy_from);
        if helps && !wide.is_empty() {
            tracing::warn!(
                "link: LINK_PROXY_FROM {} is wider than one proxy's address or a small compose network (an IPv4 /24, an \
                 IPv6 /64): any address in it hands the link on as a proxy would, and the link's requests and answers \
                 cross that hop in plain HTTP — name the proxy container's own address, or forward the link's TLS port (LINK_PORT) \
                 from a proxy on another machine",
                wide.join(", ")
            );
        }
        LinkServer {
            fingerprint: identity.as_ref().map(|identity| identity.fingerprint),
            tls,
            clients,
            secrets,
            settings,
            events,
            config,
            app: OnceLock::new(),
            helps,
            port: AtomicU16::new(0),
            problem: Mutex::new(problem),
            key: Mutex::new(None),
            armed_until: Mutex::new(None),
            shared: Arc::new(LinkServerShared {
                pairing: PairGate {
                    open: Mutex::new(HashSet::new()),
                    failures: LinkThrottle::new(PAIR_FAILURES, WINDOW),
                    logged: Mutex::new(Logged::default()),
                },
            }),
            throttle: LinkThrottle::new(KEY_FAILURES, WINDOW),
            plain_waits: Arc::new(tokio::sync::Semaphore::new(PLAIN_WAITS)),
            plain_unproven: Arc::new(tokio::sync::Semaphore::new(PLAIN_UNPROVEN)),
            connections: Limits::new(CONNECTIONS, CONNECTIONS_PER_SOURCE),
            links: Limits::with_clients(LINKS, LINKS_PER_SOURCE, LINKS_PER_CLIENT),
            client_bodies: Mutex::new(HashMap::new()),
            live: Mutex::new(HashMap::new()),
            next_live: AtomicU64::new(1),
            centres: Mutex::new(HashMap::new()),
            stopping: CancellationToken::new(),
            nudge: Arc::new(Notify::new()),
        }
    }

    /// The whole app, for the inner requests; and the news that change what a home is given.
    pub fn attach(self: &Arc<Self>, app: Weak<App>) {
        let _ = self.app.set(app);
        let me = Arc::downgrade(self);
        self.clients.set_cut(Arc::new(move |client: &str| {
            if let Some(server) = me.upgrade() {
                server.cut(client);
            }
        }));
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let me = Arc::downgrade(self);
        self.events.on("link-status", move |event| {
            let told = match &*event {
                GatewayEvent::HelpersChanged { id: Some(id), .. } => id == "self",
                GatewayEvent::HelperClients { change, .. } => change == "gives",
                GatewayEvent::VoiceChanged { .. } => true,
                _ => false,
            };
            if let Some(server) = me.upgrade().filter(|_| told) {
                server.tell_status();
            }
        });
    }

    /// A computer build that is not in the station role: it helps, so it has the key and «Помогаю».
    pub fn helps(&self) -> bool {
        self.helps
    }

    pub fn tls(&self) -> Option<Arc<ServerConfig>> {
        self.tls.clone()
    }

    pub fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::SeqCst);
    }

    /// The port the link listens on now; none before it is bound or when it could not be.
    pub fn port(&self) -> Option<u16> {
        Some(self.port.load(Ordering::SeqCst)).filter(|port| *port > 0)
    }

    pub fn set_problem(&self, words: String) {
        *locked(&self.problem) = Some(words);
    }

    pub fn problem(&self) -> Option<String> {
        locked(&self.problem).clone()
    }

    /// The certificate's SHA-256 as hex.
    pub fn fingerprint(&self) -> Option<String> {
        self.fingerprint.map(hex::encode)
    }

    fn key_state(&self) -> MutexGuard<'_, Option<KeyState>> {
        let mut key = locked(&self.key);
        if key.is_none() {
            *key = self
                .secrets
                .get(KEY_SECRET)
                .and_then(|shown| ConnectionKey::parse(&shown))
                .map(|key| KeyState { key, since: None });
        }
        key
    }

    fn store_key(&self, key: &ConnectionKey) {
        if let Err(error) = self.secrets.set(KEY_SECRET, Some(&key.shown())) {
            tracing::warn!("link: the connection key is not kept: {}", error.message);
        }
    }

    /// The connection key, made when there is none; `arm` takes it for the quarter of an hour that opening «Подключить
    /// шлюз» starts. Read again while that quarter runs — the sheet re-reads it as the panel changes — it is the same key
    /// with the same end, never moved on: a key on a sheet left open, or photographed, opens nothing 15 minutes after
    /// it was first shown. Read once that time is over (or for the first time since the start), it is a new key for a
    /// new quarter of an hour (owner, 15.11).
    pub fn connection_key(&self, arm: bool) -> ConnectionKey {
        let mut state = self.key_state();
        let mut armed_until = locked(&self.armed_until);
        let open = armed_until.is_some_and(|until| Instant::now() < until);
        if state.is_none() || (arm && !open) {
            let key = ConnectionKey::new();
            self.store_key(&key);
            *state = Some(KeyState { key, since: Some(now_iso()) });
        }
        if arm && !open {
            *armed_until = Some(Instant::now() + ARMED_FOR);
        }
        state.as_ref().map(|state| state.key.clone()).unwrap_or_default()
    }

    /// «Сменить ключ»: a new key, taken for the next 15 minutes; the old one opens nothing more.
    pub fn rotate(&self, actor: Option<Actor>) -> ConnectionKey {
        let key = ConnectionKey::new();
        self.store_key(&key);
        *self.key_state() = Some(KeyState { key: key.clone(), since: Some(now_iso()) });
        *locked(&self.armed_until) = Some(Instant::now() + ARMED_FOR);
        self.events.emit(GatewayEvent::HelperKeyRotated { actor });
        key
    }

    /// When the key was made, when known.
    pub fn key_since(&self) -> Option<String> {
        self.key_state().as_ref().and_then(|state| state.since.clone())
    }

    /// Whether a pairing is taken now.
    pub fn armed(&self) -> bool {
        locked(&self.armed_until).is_some_and(|until| Instant::now() < until)
    }

    /// Until when, as a time of day.
    pub fn armed_until(&self) -> Option<String> {
        let until = (*locked(&self.armed_until))?;
        let left = until.checked_duration_since(Instant::now())?;
        Some(crate::utils::time::iso(chrono::Utc::now() + chrono::TimeDelta::from_std(left).ok()?))
    }

    fn current_key(&self) -> Option<ConnectionKey> {
        self.key_state().as_ref().map(|state| state.key.clone())
    }

    /// A confirmed pairing spends the key it proved, if it is still the key: a new one is shown from now on. False
    /// when another pairing spent it meanwhile.
    fn spend(&self, used: &ConnectionKey) -> bool {
        let mut state = self.key_state();
        if !state.as_ref().is_some_and(|state| same(state.key.bytes(), used.bytes())) {
            return false;
        }
        let key = ConnectionKey::new();
        self.store_key(&key);
        *state = Some(KeyState { key, since: Some(now_iso()) });
        true
    }

    /// Each live link tells its home's status anew (merged: at most one event per 5 s).
    pub fn tell_status(&self) {
        self.nudge.notify_waiters();
    }

    /// A home removed: its live links closed with 4401.
    pub fn cut(&self, client: &str) {
        for live in locked(&self.live).values().filter(|live| live.client == client) {
            live.cut.cancel();
        }
    }

    /// Shutdown: every live link closed with 1001.
    pub fn close_all(&self) {
        self.stopping.cancel();
    }

    /// The links live now, for tests.
    pub fn live_links(&self) -> usize {
        locked(&self.live).len()
    }

    /// This computer's own requests to a home's gateway (`up`): over the newest live link of that home whose gateway
    /// takes them; none while there is no such link.
    pub(crate) fn up_of(&self, client: &str) -> Option<Arc<Session>> {
        let live = locked(&self.live);
        let newest =
            live.iter().filter(|(_, live)| live.client == client && live.up.is_some()).max_by_key(|(id, _)| **id);
        newest.and_then(|(_, live)| live.up.clone())
    }

    /// Whether a home's gateway takes this computer's own requests, as its last link since the start said; none before
    /// its first link.
    pub(crate) fn takes_up(&self, client: &str) -> Option<bool> {
        locked(&self.centres).get(client).copied()
    }

    /// A plain link request comes from a proxy on this computer, or from one the owner named.
    fn proxied_from(&self, peer: SocketAddr) -> bool {
        let address = peer.ip().to_string();
        in_list(&address, LOOPBACK) || in_networks(&address, &self.config.app.link_proxy_from)
    }

    /// What a pairing binds on this side: on the link port this certificate's fingerprint; over plain HTTP — only from
    /// a proxy on this computer or one the owner named — the name of «Адрес снаружи». Never the request's `Host`: a
    /// proxy that hands on any name would let a relay with a certificate of its own name choose the binding. None: no
    /// pairing this way.
    pub(crate) fn binding(&self, transport: Transport, peer: SocketAddr) -> Option<Binding> {
        match transport {
            Transport::Tls => self.fingerprint.map(Binding::Tls),
            Transport::Plain if self.proxied_from(peer) => self.outside_host().map(Binding::Name),
            Transport::Plain => None,
        }
    }

    /// The name of «Адрес снаружи», when it is set.
    fn outside_host(&self) -> Option<String> {
        let outside = self.settings.get(&HELPER_OUTSIDE, None).and_then(|outside| url::Url::parse(&outside.url).ok());
        outside.and_then(|url| url.host_str().map(bound_host)).filter(|host| !host.is_empty())
    }

    /// A pairing that failed while the key was taken, counted for its source — ten in ten minutes and it waits —: a
    /// wrong proof of the key (`guess`), or a socket that held its place without pairing (silent, or not the exchange).
    /// A refusal that held nothing and proved nothing (the key not taken, or spent meanwhile) never counts. Only a wrong
    /// proof is logged (and told to the panels): once per source, and at most `REFUSALS_LOGGED` in all, per ten minutes.
    fn pairing_failed(&self, source: &str, guess: bool) {
        self.shared.pairing.failures.fail(source);
        if !guess {
            return;
        }
        let mut logged = locked(&self.shared.pairing.logged);
        let now = Instant::now();
        logged.at.retain(|_, at| now.duration_since(*at) < WINDOW);
        if logged.at.contains_key(source) {
            return;
        }
        if logged.at.len() >= REFUSALS_LOGGED {
            if !std::mem::replace(&mut logged.full, true) {
                tracing::warn!(
                    "link: more wrong connection keys in these ten minutes; the audit log names the first \
                     {REFUSALS_LOGGED} places only"
                );
            }
            return;
        }
        logged.full = false;
        logged.at.insert(source.to_string(), now);
        self.events.emit(GatewayEvent::HelperClients {
            change: "refused".into(),
            client: source.to_string(),
            actor: None,
        });
    }
}

/// A plain upgrade to `/home/v1/link` on the main port: the link's door for a proxy on this computer. Ahead of every
/// route and of the Host check (a proxy passes the site's name); anything else goes on.
pub async fn link_socket(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    if request.uri().path() != PATH || !request.headers().contains_key(header::UPGRADE) || !app.link.helps() {
        return next.run(request).await;
    }
    upgrade(app, Transport::Plain, request).await
}

fn header_text(headers: &HeaderMap, name: &str) -> String {
    headers.get(name).and_then(|value| std::str::from_utf8(value.as_bytes()).ok()).map(kept_name).unwrap_or_default()
}

/// A header as the proof of a link covers it: as it came, trimmed.
fn said(headers: &HeaderMap, name: &str) -> String {
    headers.get(name).and_then(|value| std::str::from_utf8(value.as_bytes()).ok()).unwrap_or("").trim().to_string()
}

/// The checks before an upgrade, in this order: a page's `Origin` — 403; a pairing — its binding (none: one empty 401)
/// and its source's failures (429), then the socket: while the key is not taken it is refused after its first frame
/// holding no pairing's place, else it takes its place among the pairings; else a later link — its source's failures
/// (429), a transport allowed and its key's id naming a confirmed home (else one same empty 401, counted for its source;
/// an empty 503 while «Помогаю» does not read), then the socket, which proves the key before it takes a place
/// (`link_session`).
/// The places are counted by the socket's peer on the link port, and behind a proxy by the client it wrote — never all
/// of a proxy's clients as one.
/// On the link port every answer but the upgrade closes its connection: a connection kept alive after its deadline was
/// disarmed would hold its place among the connections for good.
async fn upgrade(app: Arc<App>, transport: Transport, request: Request) -> Response {
    let mut response = door(app, transport, request).await;
    if transport == Transport::Tls && response.status() != StatusCode::SWITCHING_PROTOCOLS {
        response.headers_mut().insert(header::CONNECTION, HeaderValue::from_static("close"));
    }
    response
}

async fn door(app: Arc<App>, transport: Transport, request: Request) -> Response {
    let server = app.link.clone();
    let (mut parts, _) = request.into_parts();
    if parts.headers.contains_key(header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    // Another version of the link: said before any socket, with this one's, so the gateway tells «update both» rather
    // than a refusal.
    if parts.headers.get(VERSION_HEADER).and_then(|value| value.to_str().ok()) != Some(VERSION.to_string().as_str()) {
        return (StatusCode::UPGRADE_REQUIRED, [(VERSION_HEADER, VERSION.to_string())]).into_response();
    }
    let Some(peer) = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|info| info.0) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let deadline = parts.extensions.get::<Arc<Deadline>>().cloned();
    let source = match transport {
        Transport::Tls => source_of(peer.ip()),
        Transport::Plain => proxied_source(&parts.headers),
    };
    let pairing = parts.headers.get(PAIR_HEADER).and_then(|value| value.to_str().ok()) == Some("pair");
    if pairing {
        let Some(binding) = server.binding(transport, peer) else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        if server.shared.pairing.failures.blocked(&source) {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
        // The key not taken now: nothing can pair. The socket is answered as a taken key's is — upgraded, refused with
        // 4401 after the gateway spoke — but takes no pairing's place, so such sockets never keep the owner's pairing
        // out. The connection's own place it keeps until it ends, as every pairing socket does; behind a proxy, a place
        // among the plain sockets not proven yet.
        let key_taken = server.armed();
        let unproven = match transport {
            Transport::Plain if !key_taken => match server.plain_unproven.clone().try_acquire_owned() {
                Ok(unproven) => Some(unproven),
                Err(_) => return StatusCode::TOO_MANY_REQUESTS.into_response(),
            },
            _ => None,
        };
        let socket = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
            Ok(socket) => socket.max_frame_size(PAIR_FRAME_MAX).max_message_size(PAIR_FRAME_MAX),
            Err(rejection) => return rejection.into_response(),
        };
        // The deadline is disarmed only on the way to the socket: an answer is the end of its connection.
        let disarm = || {
            if let Some(deadline) = &deadline {
                deadline.disarm();
            }
        };
        if !key_taken {
            disarm();
            return socket.on_upgrade(move |socket| async move {
                unarmed(socket).await;
                drop(unproven);
            });
        }
        let ticket = {
            let mut open = locked(&server.shared.pairing.open);
            if open.len() >= PAIRINGS || !open.insert(source.clone()) {
                return StatusCode::TOO_MANY_REQUESTS.into_response();
            }
            PairTicket { gate: server.shared.clone(), source: source.clone() }
        };
        disarm();
        return socket.on_upgrade(move |socket| pair_socket(app, socket, binding, ticket));
    }
    // Where a later link's failures count: on the link port, and from a socket that is no proxy, its own place; behind
    // a proxy the client it wrote — a proxy that writes `X-Forwarded-For` lets nobody choose another's —, except when it
    // wrote none, or a chain that does not agree: those share one bucket that only delays (anyone behind the proxy
    // could fill it to keep the others out).
    let (bucket, counted) = match transport {
        Transport::Tls => (source.clone(), true),
        Transport::Plain if !server.proxied_from(peer) => (source_of(peer.ip()), true),
        Transport::Plain if source != "proxied" => (source.clone(), true),
        Transport::Plain => (source.clone(), false),
    };
    // A waiting place waits whatever key it brings — a valid one too (a gateway tries a 429 again later): its key is
    // known only after a proof, and a socket refused at the door costs nothing.
    if counted && server.throttle.blocked(&bucket) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let allowed = transport == Transport::Tls || server.proxied_from(peer);
    let link_id = said(&parts.headers, LINK_ID_HEADER);
    let found = if allowed { server.clients.by_key_id(&link_id) } else { Ok(None) };
    // A known id while «Помогаю» does not read: come again (a gateway tries again), never the 401 that ends its tries.
    let Ok(found) = found else { return StatusCode::SERVICE_UNAVAILABLE.into_response() };
    let Some((client, key_hash)) = found else {
        server.throttle.fail(&bucket);
        if transport == Transport::Plain {
            // Over plain HTTP a refusal waits a second; at most `PLAIN_WAITS` at once, past them it is told at once.
            let Ok(_waiting) = server.plain_waits.clone().try_acquire_owned() else {
                return StatusCode::TOO_MANY_REQUESTS.into_response();
            };
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        return StatusCode::UNAUTHORIZED.into_response();
    };
    // Proven over what its pairing bound — whatever «Адрес снаружи» says now —; a home paired before that was kept,
    // over what a pairing this way would bind now. Neither («Адрес снаружи» cleared since): come again, never refused.
    let Some(binding) = client.binding.clone().or_else(|| server.binding(transport, peer)) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    // Behind a proxy no connection of the link port counts the sockets that have not proven their key yet.
    let unproven = match transport {
        Transport::Plain => match server.plain_unproven.clone().try_acquire_owned() {
            Ok(unproven) => Some(unproven),
            Err(_) => return StatusCode::TOO_MANY_REQUESTS.into_response(),
        },
        Transport::Tls => None,
    };
    let socket = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(socket) => socket,
        Err(rejection) => return rejection.into_response(),
    };
    if let Some(deadline) = &deadline {
        deadline.disarm();
    }
    let knock = Knock {
        client,
        key_hash,
        binding,
        link_id,
        gateway: said(&parts.headers, GATEWAY_HEADER),
        home: said(&parts.headers, HOME_HEADER),
        named: HelperIdentity {
            id: header_text(&parts.headers, GATEWAY_HEADER),
            name: header_text(&parts.headers, GATEWAY_NAME_HEADER),
        },
        source,
        bucket,
        peer,
        deadline,
        unproven,
        // The gateway takes this computer's own requests (`up`): it said so at its upgrade.
        up: parts.headers.get(UP_HEADER).and_then(|value| value.to_str().ok()).map(str::trim) == Some("1"),
    };
    socket.max_frame_size(FRAME_MAX).max_message_size(FRAME_MAX).on_upgrade(move |socket| async move {
        let (sink, stream) = axum_wire(socket);
        link_session(app, sink, stream, knock).await;
    })
}

/// A pairing socket while no key is taken: its first frame awaited (at most `PAIR_FIRST`), then 4401 — no place held,
/// nothing counted.
async fn unarmed(socket: WebSocket) {
    let (mut sink, mut stream) = axum_wire(socket);
    let _ = tokio::time::timeout(PAIR_FIRST, stream.next()).await;
    let _ = sink.send(Wire::Close(REFUSED, String::new())).await;
}

/// What a later link said at its upgrade: the home its key's id names, the key's hash, what the proof binds and the ids
/// it covers, the gateway as it named itself, where it came from and where its failures count, its connection's
/// deadline and place.
pub(crate) struct Knock {
    pub(crate) client: LinkClient,
    pub(crate) key_hash: [u8; 32],
    pub(crate) binding: Binding,
    pub(crate) link_id: String,
    pub(crate) gateway: String,
    pub(crate) home: String,
    pub(crate) named: HelperIdentity,
    pub(crate) source: String,
    pub(crate) bucket: String,
    pub(crate) peer: SocketAddr,
    pub(crate) deadline: Option<Arc<Deadline>>,
    /// A plain link's place among those not proven yet, given back with its proof.
    pub(crate) unproven: Option<tokio::sync::OwnedSemaphorePermit>,
    /// The gateway takes this computer's own requests (`UP_HEADER`).
    pub(crate) up: bool,
}

/// A later link after its upgrade: the challenge, the proof of the key within `PROOF` of the upgrade — a wrong one closed
/// with 4401, which ends the gateway's tries; a late or missing one with `SILENT`, which it tries again (a station on
/// a poor network); either counted for its source —, then its places, the home's and the port's — none free:
/// `BUSY`, which the gateway tries again later —, its connection's place given back (LINKS counts it from now on),
/// and its home served. Until its proof a socket holds nothing but its connection's place.
pub(crate) async fn link_session(app: Arc<App>, mut sink: WireSink, mut stream: WireStream, mut knock: Knock) {
    let server = app.link.clone();
    let proven = tokio::time::timeout(PROOF, prove(&mut sink, &mut stream, &knock)).await.unwrap_or(Proof::Missing);
    knock.unproven.take();
    if proven != Proof::Valid {
        server.throttle.fail(&knock.bucket);
        let code = if proven == Proof::Wrong { REFUSED } else { SILENT };
        let _ = sink.send(Wire::Close(code, String::new())).await;
        return;
    }
    let Some(permit) = server.links.take_for(&knock.source, Some(&knock.client.id)) else {
        let _ = sink.send(Wire::Close(BUSY, String::new())).await;
        return;
    };
    if let Some(deadline) = &knock.deadline {
        deadline.release();
    }
    serve_home(app, sink, stream, knock.client, knock.named, knock.peer, permit, knock.up).await;
}

/// How a link's proof came: right, a `proof` whose MAC does not hold, or none (gone, silent, or not a proof).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Proof {
    Valid,
    Wrong,
    Missing,
}

/// The challenge sent and the proof checked: a fresh nonce each time, so a proof seen once proves nothing again.
async fn prove(sink: &mut WireSink, stream: &mut WireStream, knock: &Knock) -> Proof {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut nonce = [0u8; 32];
    rand::fill(&mut nonce[..]);
    if sink.send(Wire::json(&json!({ "t": "challenge", "nonce": b64.encode(nonce) }))).await.is_err() {
        return Proof::Missing;
    }
    let Some((_, proof)) = next_json(stream).await.filter(|(kind, _)| kind == "proof") else {
        return Proof::Missing;
    };
    let mac = proof.get("mac").and_then(Value::as_str).and_then(|text| b64.decode(text).ok()).unwrap_or_default();
    let expected = link_proof(&knock.key_hash, &nonce, &knock.binding, &knock.link_id, &knock.gateway, &knock.home);
    if same(&mac, &expected) {
        Proof::Valid
    } else {
        Proof::Wrong
    }
}

/// axum's socket as the link's two halves.
pub fn axum_wire(socket: WebSocket) -> (WireSink, WireStream) {
    let (sink, stream) = socket.split();
    let sink = sink.sink_map_err(|_| ()).with(|wire: Wire| {
        futures::future::ready(Ok(match wire {
            Wire::Text(text) => Message::Text(text.into()),
            Wire::Binary(bytes) => Message::Binary(bytes),
            Wire::Ping(bytes) => Message::Ping(bytes),
            Wire::Pong(bytes) => Message::Pong(bytes),
            Wire::Close(code, reason) => Message::Close(Some(CloseFrame { code, reason: reason.into() })),
        }))
    });
    let stream = stream.take_while(|item| futures::future::ready(item.is_ok())).filter_map(|item| {
        futures::future::ready(item.ok().map(|message| match message {
            Message::Text(text) => Wire::Text(text.to_string()),
            Message::Binary(bytes) => Wire::Binary(bytes),
            Message::Ping(bytes) => Wire::Ping(bytes),
            Message::Pong(bytes) => Wire::Pong(bytes),
            Message::Close(frame) => match frame {
                Some(frame) => Wire::Close(frame.code, frame.reason.to_string()),
                None => Wire::Close(1005, String::new()),
            },
        }))
    });
    (Box::pin(sink), Box::pin(stream))
}

// ── The pairing ──

/// A pairing's pending client: forgotten unless the home confirmed it.
struct Pending {
    clients: Arc<Clients>,
    id: Option<String>,
}

impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            self.clients.forget(&id);
        }
    }
}

/// How a pairing ended badly: a wrong proof of the key (refused with 4401, counted for its source and logged); a socket
/// that held its place without pairing — silent, gone or not the exchange before its proof held — (4401, counted for
/// its source); refused with nothing to count (4401: the key no longer taken, or spent meanwhile); a link key the
/// gateway could not confirm (4400: damaged or changed on the way, nothing kept); or quietly, after a proof that held
/// (the gateway went away or turned the helper down).
enum Spoiled {
    Guess,
    Broken,
    Refused,
    Unconfirmed,
    Quiet,
}

async fn pair_socket(app: Arc<App>, socket: WebSocket, binding: Binding, ticket: PairTicket) {
    let (sink, stream) = axum_wire(socket);
    pair_session(app, sink, stream, binding, &ticket.source).await;
    drop(ticket);
}

/// One pairing over a socket: its first frame within 3 s, the exchange within its 10 s, a refusal closed with 4401 and
/// no reason — whatever it was, the gateway hears the same.
pub(crate) async fn pair_session(
    app: Arc<App>,
    mut sink: WireSink,
    mut stream: WireStream,
    binding: Binding,
    source: &str,
) {
    let mut pending = Pending { clients: app.link.clients.clone(), id: None };
    let outcome =
        tokio::time::timeout(PAIR_EXCHANGE, exchange(&app, &mut sink, &mut stream, &binding, &mut pending)).await;
    match outcome {
        Ok(Ok(())) => {
            // The gateway closes; its close is awaited a moment.
            let _ = tokio::time::timeout(Duration::from_secs(2), stream.next()).await;
        }
        Ok(Err(Spoiled::Quiet)) => {
            let _ = sink.send(Wire::Close(DONE, String::new())).await;
        }
        Ok(Err(Spoiled::Guess)) => {
            app.link.pairing_failed(source, true);
            let _ = sink.send(Wire::Close(REFUSED, String::new())).await;
        }
        Ok(Err(Spoiled::Broken)) | Err(_) => {
            app.link.pairing_failed(source, false);
            let _ = sink.send(Wire::Close(REFUSED, String::new())).await;
        }
        Ok(Err(Spoiled::Unconfirmed)) => {
            let _ = sink.send(Wire::Close(PROTOCOL, String::new())).await;
        }
        Ok(Err(Spoiled::Refused)) => {
            let _ = sink.send(Wire::Close(REFUSED, String::new())).await;
        }
    }
    drop(pending);
}

async fn next_json(stream: &mut WireStream) -> Option<(String, Value)> {
    loop {
        match stream.next().await? {
            Wire::Text(text) => return read_text(&text),
            Wire::Ping(_) | Wire::Pong(_) => continue,
            Wire::Binary(_) | Wire::Close(..) => return None,
        }
    }
}

fn nonce_of(value: &Value) -> Option<[u8; 32]> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(value.get("nonce")?.as_str()?).ok()?.try_into().ok()
}

fn id_of(value: &Value, field: &str) -> Option<String> {
    let id = value.get(field)?.as_str()?;
    (!id.is_empty() && id.len() <= 100 && id.chars().all(|c| c.is_ascii_graphic())).then(|| id.to_string())
}

async fn exchange(
    app: &Arc<App>,
    sink: &mut WireSink,
    stream: &mut WireStream,
    binding: &Binding,
    pending: &mut Pending,
) -> Result<(), Spoiled> {
    use base64::Engine;
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    let server = &app.link;
    let first = tokio::time::timeout(PAIR_FIRST, next_json(stream)).await.map_err(|_| Spoiled::Broken)?;
    let (kind, pair) = first.ok_or(Spoiled::Broken)?;
    let fields = (nonce_of(&pair), id_of(&pair, "gateway"), id_of(&pair, "home"));
    let (true, Some(nonce_g), Some(gateway), Some(home)) =
        (kind == "pair" && pair["v"] == VERSION, fields.0, fields.1, fields.2)
    else {
        return Err(Spoiled::Broken);
    };
    let probe = pair.get("probe").and_then(Value::as_bool) == Some(true);
    // Unarmed: refused before any challenge.
    let key = server.current_key().filter(|_| server.armed()).ok_or(Spoiled::Refused)?;
    let mut nonce_h = [0u8; 32];
    rand::fill(&mut nonce_h[..]);
    // Until the proof holds, any end counts against the source: a socket that holds a pairing's place and goes proves
    // nothing.
    sink.send(Wire::json(&json!({ "t": "challenge", "nonce": b64(&nonce_h) }))).await.map_err(|_| Spoiled::Broken)?;
    let (kind, proof) = next_json(stream).await.ok_or(Spoiled::Broken)?;
    let transcript = Transcript { nonce_g: &nonce_g, nonce_h: &nonce_h, binding, gateway: &gateway, home: &home };
    let field = |name: &str| {
        proof
            .get(name)
            .and_then(Value::as_str)
            .and_then(|text| base64::engine::general_purpose::STANDARD.decode(text).ok())
            .unwrap_or_default()
    };
    if kind != "proof" {
        return Err(Spoiled::Broken);
    }
    if !same(&field("mac"), &transcript.proof(key.bytes(), "gateway")) {
        return Err(Spoiled::Guess);
    }
    let own_home =
        app.secrets.get(HOME_KEY).is_some_and(|home_key| same(&field("homeMac"), &transcript.home_proof(&home_key)));
    let given = Given { abilities: if own_home { Abilities::V1 } else { MUSCLES }, own_home };
    let mut paired = json!({
        "t": "paired",
        "mac": b64(&transcript.proof(key.bytes(), "helper")),
        "helper": app.identity.as_helper(),
        "status": status_of(app, Some(given)),
    });
    if probe {
        sink.send(Wire::json(&paired)).await.map_err(|_| Spoiled::Quiet)?;
        return Ok(());
    }
    // «Помогаю» that does not read is never written over: no key given out, the connection key not spent.
    let (client, link_key) =
        server.clients.pair(&home, &gateway, own_home, Some(binding)).map_err(|_| Spoiled::Quiet)?;
    pending.id = Some(client.clone());
    // Wrapped: behind a proxy this answer crosses a plain hop, and the key must not be read there.
    let wrapped = wrapped_link_key(&link_key, &transcript.key_wrap(key.bytes())).ok_or(Spoiled::Quiet)?;
    paired["key"] = Value::String(wrapped);
    sink.send(Wire::json(&paired)).await.map_err(|_| Spoiled::Quiet)?;
    let (kind, ok) = next_json(stream).await.ok_or(Spoiled::Quiet)?;
    if kind != "ok" {
        return Err(Spoiled::Quiet);
    }
    let name = |path: &str| {
        ok.get(path).and_then(|named| named.get("name")).and_then(Value::as_str).map(kept_name).unwrap_or_default()
    };
    let names = Names { gateway: name("gateway"), home: name("home") };
    // The gateway unwrapped the very key: else nothing is kept and the connection key is not spent.
    let confirmed = ok
        .get("keyMac")
        .and_then(Value::as_str)
        .and_then(|text| base64::engine::general_purpose::STANDARD.decode(text).ok());
    if !confirmed.is_some_and(|mac| same(&mac, &transcript.key_confirm(&super::link::link_key_hash(&link_key)))) {
        return Err(Spoiled::Unconfirmed);
    }
    // The key this pairing proved, if it is still the key: else a pairing beside it won.
    if !server.spend(&key) || !server.clients.confirm(&client, names) {
        return Err(Spoiled::Refused);
    }
    pending.id = None;
    tracing::info!("link: a home paired from a gateway ({})", if own_home { "this home" } else { "another home" });
    sink.send(Wire::json(&json!({ "t": "confirmed" }))).await.map_err(|_| Spoiled::Quiet)?;
    Ok(())
}

// ── Serving a home's link ──

/// A request running for a home: its body's queue while it comes, the bytes of it received — counted until the request
/// ends, as its handler may hold them whole (`Bytes`) —, when it began, when its body last brought something, whether
/// its answer has begun, its task (none: a refused body's place, kept until its end).
struct Running {
    body: Option<mpsc::UnboundedSender<Result<Bytes, io::Error>>>,
    held: usize,
    started: Instant,
    last: Instant,
    answered: Arc<AtomicBool>,
    task: Option<tokio::task::AbortHandle>,
}

/// A home's request body as its pieces come.
struct Fed {
    pieces: mpsc::UnboundedReceiver<Result<Bytes, io::Error>>,
}

impl Stream for Fed {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.pieces.poll_recv(cx)
    }
}

/// `/home/v1` for a link's requests, their errors in the envelope, their language: made for each link and gone with it,
/// so the app is never held by its own link server.
fn inner_router(app: &Arc<App>) -> Router {
    crate::home_server::serve::router()
        .with_state(app.clone())
        .layer(axum::middleware::from_fn(crate::platform::http::api::error_bodies))
        .layer(axum::middleware::from_fn(crate::locale::request_language))
}

/// A home's link at this computer.
pub(crate) struct Home {
    origin: LinkOrigin,
    peer: SocketAddr,
    router: Router,
    out: Out,
    running: HashMap<u32, Running>,
    /// The bytes of the bodies of this link's running requests, and of its home's over all its links.
    held: usize,
    client_held: Arc<AtomicUsize>,
    done: mpsc::UnboundedSender<u32>,
}

/// A home's link at this computer, as its record names the home (never as the gateway's headers do); `up` — its gateway
/// takes this computer's own requests over it.
#[allow(clippy::too_many_arguments)]
async fn serve_home(
    app: Arc<App>,
    mut sink: WireSink,
    stream: WireStream,
    client: LinkClient,
    named: HelperIdentity,
    peer: SocketAddr,
    permit: Permit,
    up: bool,
) {
    let server = app.link.clone();
    let id = server.next_live.fetch_add(1, Ordering::SeqCst);
    let cut = CancellationToken::new();
    locked(&server.live).insert(id, LiveLink { client: client.id.clone(), cut: cut.clone(), up: None });
    // Removed between its proof and now: `cut` found no link of it to close then.
    if !server.clients.alive(&client.id) {
        locked(&server.live).remove(&id);
        let _ = sink.send(Wire::Close(REFUSED, String::new())).await;
        return;
    }
    let origin = LinkOrigin { client: client.id.clone(), own_home: client.own_home, home: client.home, gateway: named };
    let out = Out::start(sink);
    // This computer's own requests to the home's gateway share the link's writer; the pass finds them here.
    let calls = up.then(|| Session::over(out.clone(), Way::Up));
    if let Some(live) = locked(&server.live).get_mut(&id) {
        live.up = calls.clone();
    }
    locked(&server.centres).insert(client.id.clone(), up);
    server.clients.online(&client.id, &origin.gateway, true);
    let code = serve_link(app.clone(), out, stream, origin.clone(), peer, cut, calls.clone()).await;
    tracing::info!("link: a home's link closed ({code})");
    locked(&server.live).remove(&id);
    if let Some(calls) = calls {
        calls.fail_all("the link closed");
    }
    server.clients.online(&client.id, &origin.gateway, false);
    // The home's count of bodies goes with its last link.
    locked(&server.client_bodies).retain(|_, held| Arc::strong_count(held) > 1);
    drop(permit);
}

/// The status a home sees now: what it is given, and the voice only within D31's limits.
fn status_for(app: &App, origin: &LinkOrigin) -> Value {
    status_of(app, Some(Given { abilities: app.link.clients.given(&origin.client), own_home: origin.own_home }))
}

/// A home's link served until it ends: the welcome, then its requests through `/home/v1`, the status told as it
/// changes, the client looked at again every `RECHECK` (removed: 4401), silence closed (4408); the answers to this
/// computer's own requests (`up`, over `calls`) to their callers. The close code.
pub(crate) async fn serve_link(
    app: Arc<App>,
    out: Out,
    mut stream: WireStream,
    origin: LinkOrigin,
    peer: SocketAddr,
    cut: CancellationToken,
    calls: Option<Arc<Session>>,
) -> u16 {
    let server = app.link.clone();
    let gone = out.gone.clone();
    let (done, mut finished) = mpsc::unbounded_channel();
    let client_held = locked(&server.client_bodies).entry(origin.client.clone()).or_default().clone();
    let router = inner_router(&app);
    let mut home = Home { origin, peer, router, out, running: HashMap::new(), held: 0, client_held, done };
    let mut told = status_for(&app, &home.origin);
    home.control(Wire::json(&json!({ "t": "welcome", "v": super::link::VERSION, "status": told })));
    let mut told_at = Instant::now();
    // The status to tell once `STATUS_EVERY` has passed since the last: always the latest — none when it came back to
    // the one told.
    let mut waiting: Option<Value> = None;
    let mut last = Instant::now();
    let mut recheck = tokio::time::interval_at(Instant::now() + RECHECK, RECHECK);
    let code = loop {
        let nudged = server.nudge.notified();
        tokio::select! {
            frame = stream.next() => {
                let Some(frame) = frame else { break 1006 };
                last = Instant::now();
                match frame {
                    Wire::Text(text) => match read_text(&text) {
                        Some((kind, value)) if kind == "req" => {
                            if let Err(code) = home.request(&value) {
                                break code;
                            }
                        }
                        Some((kind, value)) if kind == "cancel" => {
                            if let Some(id) = value.get("id").and_then(Value::as_u64).and_then(|id| u32::try_from(id).ok()) {
                                home.drop_stream(id);
                            }
                        }
                        Some((kind, _)) if kind == "bye" => {
                            // This home removed the helper: its client forgotten.
                            let _ = server.clients.remove(&home.origin.client, None);
                            break DONE;
                        }
                        // The answers to this computer's own requests (`up`): to the callers that wait for them.
                        Some((kind, value)) if kind == Way::Up.answer() => {
                            if let Some(calls) = &calls {
                                calls.on_answer(&value);
                            }
                        }
                        Some((kind, value)) if kind == Way::Up.failure() => {
                            if let Some(calls) = &calls {
                                calls.on_failure(&value);
                            }
                        }
                        _ => {}
                    },
                    Wire::Binary(bytes) => {
                        // The pieces of those answers (their ids above `UP_BIT`) to their readers; the gateway's request
                        // bodies to their handlers here.
                        let fed = match (&calls, up_piece(&bytes)) {
                            (Some(calls), true) => calls.on_chunk(&bytes).await,
                            (None, true) => true,
                            (_, false) => home.feed(&bytes),
                        };
                        if !fed {
                            break PROTOCOL;
                        }
                    }
                    Wire::Close(..) => break DONE,
                    Wire::Ping(_) | Wire::Pong(_) => {}
                }
            }
            Some(id) = finished.recv() => {
                if let Some(running) = home.running.remove(&id) {
                    home.release(&running);
                }
            }
            _ = tokio::time::sleep_until(last + SILENCE) => break SILENT,
            _ = recheck.tick() => {
                if !server.clients.alive(&home.origin.client) {
                    break REFUSED;
                }
                home.give_up_late_bodies(BODY_SILENCE, BODY_WHOLE);
                let now = status_for(&app, &home.origin);
                waiting = (now != told).then_some(now);
            }
            _ = nudged => {
                let now = status_for(&app, &home.origin);
                waiting = (now != told).then_some(now);
            }
            _ = tokio::time::sleep_until(told_at + STATUS_EVERY), if waiting.is_some() => {
                if let Some(status) = waiting.take() {
                    home.control(Wire::json(&json!({ "t": "event", "name": "status", "data": status })));
                    told = status;
                    told_at = Instant::now();
                }
            }
            _ = cut.cancelled() => break REFUSED,
            _ = server.stopping.cancelled() => break GOING,
            _ = gone.cancelled() => break PROTOCOL,
        }
    };
    for (_, running) in home.running.drain() {
        if let Some(task) = &running.task {
            task.abort();
        }
        take(&BODIES, running.held);
        take(&home.client_held, running.held);
    }
    home.out.close(code);
    home.out.closed(Duration::from_secs(1)).await;
    gone.cancel();
    code
}

impl Home {
    /// A frame that must go and never waits (`Out::control`).
    fn control(&self, wire: Wire) {
        self.out.control(wire);
    }

    /// A request ended: the bytes of its body given back.
    fn release(&mut self, running: &Running) {
        self.held = self.held.saturating_sub(running.held);
        take(&self.client_held, running.held);
        take(&BODIES, running.held);
    }

    /// A stream given up: its task stopped, its body's room given back.
    fn drop_stream(&mut self, id: u32) {
        if let Some(running) = self.running.remove(&id) {
            if let Some(task) = &running.task {
                task.abort();
            }
            self.release(&running);
        }
    }

    /// A stream refused or given up while it runs: before its answer a `res` with the status, after it an `error`.
    /// While its body is still coming, a place with no task is kept for it: its later pieces are passed by, and its
    /// end lets the place go.
    fn refuse(&mut self, id: u32, status: StatusCode, body_coming: bool) {
        let answered = self.running.get(&id).is_some_and(|running| running.answered.load(Ordering::SeqCst));
        let started = self.running.get(&id).map_or_else(Instant::now, |running| running.started);
        self.drop_stream(id);
        if body_coming {
            self.running.insert(
                id,
                Running {
                    body: None,
                    held: 0,
                    started,
                    last: Instant::now(),
                    answered: Arc::new(AtomicBool::new(true)),
                    task: None,
                },
            );
        }
        let frame = if answered {
            json!({ "t": "error", "id": id, "message": status.canonical_reason().unwrap_or("") })
        } else {
            json!({ "t": "res", "id": id, "status": status.as_u16(), "headers": {}, "body": false })
        };
        self.control(Wire::json(&frame));
    }

    /// A request frame: run through `/home/v1` as this home's, unless it is a flood or not of the gateway's ids (close
    /// 4400: those above `UP_BIT` are this computer's own), more than may run (503), or not a `/home/v1` path (404).
    fn request(&mut self, value: &Value) -> Result<(), u16> {
        let id = value.get("id").and_then(Value::as_u64).and_then(|id| u32::try_from(id).ok()).ok_or(PROTOCOL)?;
        if id & UP_BIT != 0 || self.running.contains_key(&id) || self.running.len() >= OPEN {
            return Err(PROTOCOL);
        }
        let path = value.get("path").and_then(Value::as_str).unwrap_or("");
        let has_body = value.get("body").and_then(Value::as_bool).unwrap_or(false);
        let inner = path.split('?').next().unwrap_or("");
        let method =
            value.get("method").and_then(Value::as_str).and_then(|method| method.parse::<axum::http::Method>().ok());
        let running = self.running.values().filter(|running| running.task.is_some()).count();
        let status = if running >= RUNNING {
            Some(StatusCode::SERVICE_UNAVAILABLE)
        } else if !inner.starts_with("/home/v1/")
            || inner == PATH
            || inner.starts_with(&format!("{PATH}/"))
            || method.is_none()
        {
            Some(StatusCode::NOT_FOUND)
        } else {
            None
        };
        let uri = path.parse::<axum::http::Uri>().ok();
        let (None, Some(method), Some(uri)) = (status, method, uri) else {
            self.refuse(id, status.unwrap_or(StatusCode::NOT_FOUND), has_body);
            return Ok(());
        };
        let (body, sender) = if has_body {
            let (sender, pieces) = mpsc::unbounded_channel();
            (Body::from_stream(Fed { pieces }), Some(sender))
        } else {
            (Body::empty(), None)
        };
        let mut request = Request::new(body);
        *request.method_mut() = method;
        *request.uri_mut() = uri;
        *request.headers_mut() = headers_from(&value["headers"], |name| CARRIED.contains(&name));
        request.extensions_mut().insert(ConnectInfo(self.peer));
        request.extensions_mut().insert(self.origin.clone());
        let answered = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(answer(
            self.router.clone(),
            request,
            id,
            self.out.clone(),
            answered.clone(),
            self.done.clone(),
            Way::Req,
        ));
        let now = Instant::now();
        self.running.insert(
            id,
            Running { body: sender, held: 0, started: now, last: now, answered, task: Some(task.abort_handle()) },
        );
        Ok(())
    }

    /// A body piece to its request: at most `BODY_MAX` for one (413); the bodies of the running requests at most
    /// `BODIES_PER_LINK` on the link, `BODIES_PER_CLIENT` over its home's links and `BODIES_IN_ALL` on this computer
    /// (503), counted until each request ends. Only a piece that brings something, or the end, keeps a body from being
    /// given up as silent. False on a malformed piece.
    fn feed(&mut self, frame: &Bytes) -> bool {
        let Some((id, end, data)) = decode_chunk(frame) else { return false };
        let Some(running) = self.running.get_mut(&id) else { return true };
        if !data.is_empty() || end {
            running.last = Instant::now();
        }
        // A refused stream's place: passed by, let go at its end.
        if running.task.is_none() {
            if end {
                self.running.remove(&id);
            }
            return true;
        }
        if running.body.is_none() {
            return true;
        }
        if !data.is_empty() {
            if running.held + data.len() > BODY_MAX {
                self.refuse(id, StatusCode::PAYLOAD_TOO_LARGE, !end);
                return true;
            }
            if self.held + data.len() > BODIES_PER_LINK
                || self.client_held.load(Ordering::SeqCst) + data.len() > BODIES_PER_CLIENT
                || BODIES.load(Ordering::SeqCst) + data.len() > BODIES_IN_ALL
            {
                self.refuse(id, StatusCode::SERVICE_UNAVAILABLE, !end);
                return true;
            }
            running.held += data.len();
            self.held += data.len();
            self.client_held.fetch_add(data.len(), Ordering::SeqCst);
            BODIES.fetch_add(data.len(), Ordering::SeqCst);
            if let Some(body) = &running.body {
                let _ = body.send(Ok(data));
            }
        }
        if end {
            if let Some(running) = self.running.get_mut(&id) {
                running.body = None;
            }
        }
        true
    }

    /// Bodies that stopped coming for `silence`, or still come `whole` after their request began, are given up (408);
    /// a refused body's place, already answered, is let go as quietly.
    fn give_up_late_bodies(&mut self, silence: Duration, whole: Duration) {
        let late = |running: &Running| running.last.elapsed() >= silence || running.started.elapsed() >= whole;
        self.running.retain(|_, running| running.task.is_some() || !late(running));
        let given_up: Vec<u32> = self
            .running
            .iter()
            .filter(|(_, running)| running.body.is_some() && late(running))
            .map(|(id, _)| *id)
            .collect();
        for id in given_up {
            self.refuse(id, StatusCode::REQUEST_TIMEOUT, true);
        }
    }
}

#[cfg(test)]
#[path = "link_serve_tests.rs"]
mod tests;
