//! The link a gateway opens to its helper (docs/plans/home-and-helpers.md, «Связь»; stage 7): a WebSocket over TLS to
//! `wss://<helper>/home/v1/link` that the gateway dials itself, so a helper behind a router or across the internet
//! is reached all the same. This is the gateway's side, in both builds; the helper's side is `link_serve.rs` (pc).
//!
//! - **The first meeting** (`pair`): the helper shows a one-time connection key («Подключить шлюз»); the gateway dials
//!   with any certificate, records it, and both sides prove they know the key with an HMAC over both nonces and the
//!   certificate's fingerprint (or, behind the owner's proxy with a real domain, the name it was checked for). A
//!   relay with its own certificate binds another fingerprint and fails; 128 random bits are not guessed. The gateway
//!   also proves its home with a key derived from the home key: only a gateway of the helper's own home gets messages,
//!   apps and history by default. Then the helper gives this home a link key of its own (`dwl1_…`), and the gateway
//!   pins the certificate (`helper_pins`).
//! - **Later connections**: only the pinned certificate (TLS 1.3, its DER's hash) or a public chain for the pinned
//!   name; the upgrade names the link key's id only, and the key is proven by an HMAC over the helper's challenge and
//!   the pin (`link_proof`) — the key itself never travels again. A refusal stops the tries.
//! - **Frames**: the same `/home/v1/*` requests as before, each a `req` with an id, bodies streamed in binary frames
//!   `[u32 id][u8 flags][payload]`, answers as `res` and frames, the helper's news as `event`. A ping every 15 s;
//!   30 s of silence is a dead link. A drop is retried after 1, 2, 5, 10, 30 and then 60 s.
//! - **The bridge**: the gateway's own clients — hearing, voice, «Мозг», apps, history, the catalog — keep asking a
//!   plain URL, `http://127.0.0.1:<port>/via/<entry>/home/v1/…`, with a per-start token; the bridge hands each request
//!   to the link. Nothing new listens on the station.
//! - **Up** (stage 7 of docs/plans/home-setup-flow.md): the helper's own requests to the gateway over the same socket —
//!   a Turbo whose home's centre is this gateway opens the home's panel through it, for the person signed in at the
//!   Turbo. `up` with an id above `UP_BIT` (the gateway's stay below it), its body in the same binary pieces, answered
//!   with `up-res` and pieces, `up-error` after a broken body, `up-cancel` from the helper. Only a gateway whose upgrade
//!   said `UP_HEADER` is asked; one from before `up` passes the new kinds and ids by as unknown.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use base64::Engine;
use futures::{Sink, SinkExt, Stream, StreamExt};
use hmac::{Hmac, KeyInit, Mac};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::{Resumption, WebPkiServerVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch, Notify};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use super::contract::{HelperIdentity, HelperStatus};
use super::remote::Me;
use crate::app::App;
use crate::core::identity::home::clean_name;
use crate::platform::http::{ApiError, Caller, UpOrigin, Zone};
use crate::settings::definitions::HelperPin;
use crate::utils::time::now_iso;

/// The link's one path, on the helper's link port (and, from a proxy on the helper, on its main port).
pub const PATH: &str = "/home/v1/link";
/// The link's version this gateway speaks; a helper of another answers `426` with its own in `VERSION_HEADER`, before
/// any socket (2: a later link proves its key, the pairing wraps it and confirms it).
pub const VERSION: u32 = 2;
/// `pair` on a first meeting.
pub const PAIR_HEADER: &str = "x-duowave-link";
/// A later link: the id of the link key it will prove (`dwl1_<id>_…`).
pub const LINK_ID_HEADER: &str = "x-duowave-link-id";
pub const VERSION_HEADER: &str = "x-duowave-link-version";
/// Who this gateway is, for the helper's «Помогаю» — never for rights, and never before the helper proved itself.
pub const GATEWAY_HEADER: &str = "x-duowave-gateway";
pub const GATEWAY_NAME_HEADER: &str = "x-duowave-gateway-name";
pub const HOME_HEADER: &str = "x-duowave-home";
/// Close codes: done, the helper stops, refused (no reason given), a protocol error or a limit, silence, no place for
/// one more link now (tried again later).
pub const DONE: u16 = 1000;
pub const GOING: u16 = 1001;
pub const REFUSED: u16 = 4401;
pub const PROTOCOL: u16 = 4400;
pub const SILENT: u16 = 4408;
pub const BUSY: u16 = 4429;
/// The largest body piece in a frame; a larger one is a protocol error.
pub const CHUNK: usize = 64 * 1024;
/// The largest WebSocket frame and message of a link, and of a pairing.
pub const FRAME_MAX: usize = 256 * 1024;
pub const PAIR_FRAME_MAX: usize = 64 * 1024;
/// A request's body in all, as `/home/v1`'s own limit.
pub const BODY_MAX: usize = 16 * 1024 * 1024;
/// What one stream's reader may leave unread, and all of a link's together.
const STREAM_QUEUE: usize = 1024 * 1024;
const LINK_QUEUE: usize = 8 * 1024 * 1024;
/// Requests in flight on one link at the gateway; the next one is refused at once.
const IN_FLIGHT: usize = 16;
/// Messages waiting for the socket's writer.
const WRITER_QUEUE: usize = 64;
/// How long a full queue may stay full.
const STALL: Duration = Duration::from_secs(5);
/// A request's safety cap: a model's long answer streams within it.
const SAFETY: Duration = Duration::from_secs(600);
/// The longest name this gateway gives of itself or its home.
const NAME_SENT: usize = 60;
/// The link key's form: `dwl1_<12 hex id>_<64 hex>`.
const LINK_KEY_PREFIX: &str = "dwl1";
/// The top bit of a stream's id marks the helper's own requests (`up`): the gateway numbers its streams below it, the
/// helper above it, so a body piece always finds its own stream — and a gateway from before `up` passes the helper's
/// pieces by as an unknown stream's.
pub const UP_BIT: u32 = 1 << 31;
/// A gateway's upgrade says it takes its helper's own requests (`up`) — while the entry is a helper at home: a helper
/// asks only a gateway that said so.
pub const UP_HEADER: &str = "x-duowave-link-up";
/// The helper's own requests in flight on one link at the helper; running and open (with the places of refused bodies)
/// at the gateway — the helper's own limits for the gateway's requests (link_serve.rs).
const UP_IN_FLIGHT: usize = 32;
const UP_RUNNING: usize = 32;
const UP_OPEN: usize = 64;
/// The bodies of a link's running `up` requests at the gateway, held until each request ends; a body silent this long,
/// or still coming this long after its request began, is given up.
const UP_BODIES: usize = 32 * 1024 * 1024;
const UP_BODY_SILENCE: Duration = Duration::from_secs(30);
const UP_BODY_WHOLE: Duration = Duration::from_secs(5 * 60);

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64_32(text: Option<&str>) -> Option<[u8; 32]> {
    base64::engine::general_purpose::STANDARD.decode(text?).ok()?.try_into().ok()
}

fn random_32() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes[..]);
    bytes
}

/// Constant-time equality of two byte strings.
pub(crate) fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

// ── Keys ──

/// Crockford's base32: no I, L, O or U.
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The connection key a helper shows at «Подключить шлюз»: 128 random bits, written as 26 letters and digits in groups
/// 5-5-5-5-6. One-time: a confirmed pairing spends it.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionKey([u8; 16]);

impl std::fmt::Debug for ConnectionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConnectionKey(…)")
    }
}

impl ConnectionKey {
    pub fn new() -> ConnectionKey {
        let mut bytes = [0u8; 16];
        rand::fill(&mut bytes[..]);
        ConnectionKey(bytes)
    }

    pub fn bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// `ABCDE-FGHJK-MNPQR-STVWX-YZ012`.
    pub fn shown(&self) -> String {
        let value = u128::from_be_bytes(self.0);
        let digits: Vec<u8> = (0..26).rev().map(|at| CROCKFORD[((value >> (5 * at)) & 31) as usize]).collect();
        [&digits[0..5], &digits[5..10], &digits[10..15], &digits[15..20], &digits[20..26]]
            .iter()
            .map(|group| String::from_utf8_lossy(group).into_owned())
            .collect::<Vec<_>>()
            .join("-")
    }

    /// As a person typed or scanned it: dashes, spaces and case aside, I and L read as 1, O as 0; exactly 26 characters
    /// that hold 128 bits, else none.
    pub fn parse(text: &str) -> Option<ConnectionKey> {
        let mut value: u128 = 0;
        let mut count = 0;
        for character in text.chars().filter(|c| *c != '-' && !c.is_whitespace()) {
            let character = match character.to_ascii_uppercase() {
                'I' | 'L' => '1',
                'O' => '0',
                other => other,
            };
            let digit = CROCKFORD.iter().position(|known| *known as char == character)? as u128;
            value = value.checked_mul(32)?.checked_add(digit)?;
            count += 1;
        }
        (count == 26).then(|| ConnectionKey(value.to_be_bytes()))
    }
}

impl Default for ConnectionKey {
    fn default() -> ConnectionKey {
        ConnectionKey::new()
    }
}

/// A new link key: its id (12 hex) and the key itself, `dwl1_<id>_<64 hex>` — 256 random bits.
pub fn new_link_key() -> (String, String) {
    let id = crate::utils::ids::random_hex(6);
    let key = format!("{LINK_KEY_PREFIX}_{id}_{}", crate::utils::ids::random_hex(32));
    (id, key)
}

/// The id of a token shaped like a link key; none for anything else.
pub fn link_key_id(token: &str) -> Option<&str> {
    let mut parts = token.splitn(3, '_');
    let (prefix, id, secret) = (parts.next()?, parts.next()?, parts.next()?);
    let hex = |text: &str, length: usize| {
        text.len() == length && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    (prefix == LINK_KEY_PREFIX && hex(id, 12) && hex(secret, 64)).then_some(id)
}

/// Whether a stored key is a link key: it belongs to a pinned link and is never sent another way.
pub fn is_link_key(key: &str) -> bool {
    key.starts_with("dwl1_")
}

/// A link key with its 32 secret bytes XORed with `pad`, its id clear: the key wrapped, or a wrapped key unwrapped (the
/// same either way). None for what is no link key.
pub fn wrapped_link_key(key: &str, pad: &[u8; 32]) -> Option<String> {
    let id = link_key_id(key)?;
    let secret = hex::decode(key.rsplit('_').next()?).ok()?;
    let wrapped: Vec<u8> = secret.iter().zip(pad).map(|(byte, pad)| byte ^ pad).collect();
    Some(format!("{LINK_KEY_PREFIX}_{id}_{}", hex::encode(wrapped)))
}

/// What a helper keeps of a link key, and the key of the link's proof (`link_proof`): its SHA-256.
pub fn link_key_hash(key: &str) -> [u8; 32] {
    sha256(key.as_bytes())
}

// ── The proofs of the first meeting ──

/// What both sides bind the proofs to, typed so that one kind never matches the other: the certificate's SHA-256, or
/// the DNS name a public certificate was checked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    Tls([u8; 32]),
    Name(String),
}

impl Binding {
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            Binding::Tls(fingerprint) => [b"tls:".as_slice(), fingerprint].concat(),
            Binding::Name(host) => [b"name:".as_slice(), host.as_bytes()].concat(),
        }
    }

    /// As a helper keeps it in its client's record: the binding a later link of that home proves over, whatever
    /// «Адрес снаружи» says by then.
    pub fn stored(&self) -> String {
        match self {
            Binding::Tls(fingerprint) => format!("tls:{}", hex::encode(fingerprint)),
            Binding::Name(host) => format!("name:{host}"),
        }
    }

    pub fn from_stored(stored: &str) -> Option<Binding> {
        if let Some(fingerprint) = stored.strip_prefix("tls:") {
            return hex::decode(fingerprint).ok()?.try_into().ok().map(Binding::Tls);
        }
        stored.strip_prefix("name:").filter(|host| !host.is_empty()).map(|host| Binding::Name(host.to_string()))
    }

    /// The pin it leaves at the gateway.
    pub fn pin(&self) -> HelperPin {
        match self {
            Binding::Tls(fingerprint) => {
                HelperPin { sha256: Some(hex::encode(fingerprint)), public: None, since: now_iso() }
            }
            Binding::Name(host) => HelperPin { sha256: None, public: Some(host.clone()), since: now_iso() },
        }
    }
}

/// A host as a binding names it: lowercase, no port, no trailing dot.
pub fn bound_host(host: &str) -> String {
    let host = host.trim().to_ascii_lowercase();
    let host = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(rest).to_string(),
        None if host.matches(':').count() == 1 => host.split(':').next().unwrap_or(&host).to_string(),
        None => host,
    };
    host.trim_end_matches('.').to_string()
}

/// What both proofs of a pairing cover: both nonces, the binding, and the two random ids the gateway named — no name
/// and no helper id (the binding already says which helper it is).
pub struct Transcript<'a> {
    pub nonce_g: &'a [u8; 32],
    pub nonce_h: &'a [u8; 32],
    pub binding: &'a Binding,
    pub gateway: &'a str,
    pub home: &'a str,
}

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes a key of any size");
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

fn with_length(bytes: &[u8]) -> Vec<u8> {
    let length = u16::try_from(bytes.len()).unwrap_or(u16::MAX);
    [length.to_be_bytes().as_slice(), &bytes[..usize::from(length)]].concat()
}

impl Transcript<'_> {
    fn bytes(&self, role: &str) -> Vec<u8> {
        let mut t = b"duowave link pair v1\0".to_vec();
        t.extend_from_slice(role.as_bytes());
        t.push(0);
        t.extend_from_slice(self.nonce_g);
        t.extend_from_slice(self.nonce_h);
        t.extend(with_length(&self.binding.bytes()));
        t.extend(with_length(self.gateway.as_bytes()));
        t.extend(with_length(self.home.as_bytes()));
        t
    }

    /// The gateway's proof (`role` "gateway") or the helper's (`role` "helper") of the connection key.
    pub fn proof(&self, key: &[u8], role: &str) -> [u8; 32] {
        hmac(key, &[&self.bytes(role)])
    }

    /// What the helper's new link key is wrapped in, in its `paired` answer: a pad derived from the connection key
    /// over the whole transcript under a label of its own. A pairing behind a proxy crosses a plain hop; whoever sees it
    /// there, without the connection key, learns nothing of the link key.
    pub fn key_wrap(&self, key: &[u8]) -> [u8; 32] {
        hmac(key, &[b"duowave link key wrap v1\0", &self.bytes("wrap")])
    }

    /// The gateway's confirmation of the link key it unwrapped, in its `ok`: an HMAC keyed by that key's hash over the
    /// transcript under a label of its own. A key damaged or changed on the way fails the pairing there, before the
    /// helper keeps a client whose key no link could prove.
    pub fn key_confirm(&self, key_hash: &[u8; 32]) -> [u8; 32] {
        hmac(key_hash, &[b"duowave link key confirm v1\0", &self.bytes("confirm")])
    }

    /// The gateway's proof of its home: a key derived from the home key over both nonces, the binding and the
    /// gateway's id. A helper of the same home computes the same; any other does not.
    pub fn home_proof(&self, home_key: &str) -> [u8; 32] {
        let bytes = hex::decode(home_key.trim()).unwrap_or_else(|_| home_key.as_bytes().to_vec());
        let derived = hmac(&bytes, &[b"duowave link home key v1"]);
        let binding = with_length(&self.binding.bytes());
        let gateway = with_length(self.gateway.as_bytes());
        hmac(&derived, &[b"duowave link home v1\0", self.nonce_g, self.nonce_h, &binding, &gateway])
    }
}

/// A later link's proof of its link key, the answer to the helper's challenge: an HMAC keyed by the key's SHA-256 over
/// the helper's nonce, the binding (the gateway's pin; at the helper, what a pairing over that transport binds), the
/// key's id and the ids the gateway named in the upgrade. A fresh nonce each time and the pin in it: a proof seen on a
/// plain hop behind a proxy, or relayed through another certificate, opens nothing. The helper keeps only the hash, so
/// a copy of its secrets can link to that one helper as the home — accepted: it never shows the key itself.
pub fn link_proof(
    key_hash: &[u8; 32],
    nonce: &[u8; 32],
    binding: &Binding,
    link_id: &str,
    gateway: &str,
    home: &str,
) -> [u8; 32] {
    let binding = with_length(&binding.bytes());
    let (link_id, gateway, home) =
        (with_length(link_id.as_bytes()), with_length(gateway.as_bytes()), with_length(home.as_bytes()));
    hmac(key_hash, &[b"duowave link key v1\0", nonce, &binding, &link_id, &gateway, &home])
}

// ── TLS: the verifiers, a fresh configuration for each pairing and each link ──

pub(crate) fn provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER.get_or_init(|| Arc::new(rustls::crypto::ring::default_provider())).clone()
}

fn roots() -> Arc<RootCertStore> {
    static ROOTS: OnceLock<Arc<RootCertStore>> = OnceLock::new();
    ROOTS.get_or_init(|| Arc::new(RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() })).clone()
}

fn webpki() -> Option<Arc<WebPkiServerVerifier>> {
    WebPkiServerVerifier::builder_with_provider(roots(), provider()).build().ok()
}

/// What the first meeting saw: the certificate's SHA-256, and whether a public chain vouches for the DNS name dialed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    pub fingerprint: [u8; 32],
    pub public: bool,
}

/// The first meeting's verifier: any certificate whose key signed the handshake (rustls checks that with it), recorded
/// for the proofs to bind. A certificate never seen is no meeting.
#[derive(Debug)]
pub struct FirstMeeting {
    webpki: Option<Arc<WebPkiServerVerifier>>,
    seen: Mutex<Option<Seen>>,
}

impl FirstMeeting {
    pub fn new() -> Arc<FirstMeeting> {
        Arc::new(FirstMeeting { webpki: webpki(), seen: Mutex::new(None) })
    }

    pub fn seen(&self) -> Option<Seen> {
        *locked(&self.seen)
    }
}

fn verify_tls12(
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, rustls::Error> {
    rustls::crypto::verify_tls12_signature(message, cert, dss, &provider().signature_verification_algorithms)
}

fn verify_tls13(
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, rustls::Error> {
    rustls::crypto::verify_tls13_signature(message, cert, dss, &provider().signature_verification_algorithms)
}

impl ServerCertVerifier for FirstMeeting {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let public = matches!(server_name, ServerName::DnsName(_))
            && self.webpki.as_ref().is_some_and(|webpki| {
                webpki.verify_server_cert(end_entity, intermediates, server_name, ocsp, now).is_ok()
            });
        *locked(&self.seen) = Some(Seen { fingerprint: sha256(end_entity.as_ref()), public });
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        provider().signature_verification_algorithms.supported_schemes()
    }
}

/// A pinned helper's verifier: only the certificate whose DER hashes to the pin, dates and names aside; whether a
/// foreign one was shown is remembered for the words.
#[derive(Debug)]
pub struct Pinned {
    pin: [u8; 32],
    foreign: AtomicBool,
}

impl Pinned {
    pub fn new(pin: [u8; 32]) -> Arc<Pinned> {
        Arc::new(Pinned { pin, foreign: AtomicBool::new(false) })
    }

    pub fn foreign(&self) -> bool {
        self.foreign.load(Ordering::SeqCst)
    }
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if same(&sha256(end_entity.as_ref()), &self.pin) {
            return Ok(ServerCertVerified::assertion());
        }
        self.foreign.store(true, Ordering::SeqCst);
        Err(rustls::Error::InvalidCertificate(rustls::CertificateError::ApplicationVerificationFailure))
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        // A pinned link speaks TLS 1.3 only.
        Err(rustls::Error::PeerIncompatible(rustls::PeerIncompatible::Tls12NotOffered))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        provider().signature_verification_algorithms.supported_schemes()
    }
}

/// A client configuration of its own: no resumption (a resumed session would skip the verifier), HTTP/1.1.
pub(crate) fn client_config(verifier: Option<Arc<dyn ServerCertVerifier>>, tls12: bool) -> Option<Arc<ClientConfig>> {
    let versions: &[&rustls::SupportedProtocolVersion] =
        if tls12 { &[&rustls::version::TLS13, &rustls::version::TLS12] } else { &[&rustls::version::TLS13] };
    let builder = ClientConfig::builder_with_provider(provider()).with_protocol_versions(versions).ok()?;
    let mut config = match verifier {
        Some(verifier) => builder.dangerous().with_custom_certificate_verifier(verifier).with_no_client_auth(),
        None => builder.with_root_certificates(roots()).with_no_client_auth(),
    };
    config.resumption = Resumption::disabled();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Some(Arc::new(config))
}

// ── Dialing ──

/// Where a link goes: an `https://` address's host and port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    /// The host is a DNS name, not an address.
    pub dns: bool,
}

impl Target {
    pub fn of(url: &str) -> Option<Target> {
        let parsed = url::Url::parse(url).ok()?;
        if parsed.scheme() != "https" {
            return None;
        }
        let (host, dns) = match parsed.host()? {
            url::Host::Domain(name) => (bound_host(name), true),
            url::Host::Ipv4(ip) => (ip.to_string(), false),
            url::Host::Ipv6(ip) => (ip.to_string(), false),
        };
        Some(Target { host, port: parsed.port().unwrap_or(443), dns })
    }

    fn authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    fn server_name(&self) -> Option<ServerName<'static>> {
        match self.host.parse::<IpAddr>() {
            Ok(ip) => Some(ServerName::IpAddress(ip.into())),
            Err(_) => ServerName::try_from(self.host.clone()).ok(),
        }
    }
}

/// Why a dial did not give a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialError {
    /// Nothing answered the TCP connect.
    Unreachable,
    /// The TLS handshake failed; the pinned verifier saw a foreign certificate when `foreign`.
    Tls {
        foreign: bool,
    },
    /// 401 (the helper's own refusal), or a close 4401 after the upgrade.
    Refused,
    /// 429: too many failures from here, or too many links; tried again later.
    Busy,
    /// 426 naming another link version: a helper of another DuoWave.
    Version,
    /// Something answered, but no link: another status, or no WebSocket.
    NoLink,
    Timeout,
}

fn ws_config(frames: usize) -> WebSocketConfig {
    WebSocketConfig::default().max_frame_size(Some(frames)).max_message_size(Some(frames))
}

/// The link's socket at the gateway: the stream type the Yandex client's socket has too, so the station's program
/// carries one copy of the WebSocket's code for both.
pub type LinkSocket = WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

/// The last failure of a dial to each address told to the log: a link retried every minute the same way is told once.
static DIALS_TOLD: std::sync::LazyLock<Mutex<HashMap<String, String>>> = std::sync::LazyLock::new(Default::default);

fn tell_dial(target: &Target, cause: &str) {
    let mut told = DIALS_TOLD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if told.get(&target.authority()).map(String::as_str) == Some(cause) {
        return;
    }
    told.insert(target.authority(), cause.to_string());
    tracing::warn!("link: dialing {} failed: {cause}", target.authority());
}

fn forget_dial(target: &Target) {
    let removed = DIALS_TOLD.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&target.authority());
    if removed.is_some() {
        tracing::info!("link: {} answers again", target.authority());
    }
}

/// Dials `wss://host:port/home/v1/link` over TLS it makes itself: the upgrade — and whatever it carries — is written
/// only once the handshake finished with the verifier's consent.
async fn dial(
    target: &Target,
    config: Arc<ClientConfig>,
    headers: Vec<(&'static str, String)>,
    frames: usize,
    timing: &LinkTiming,
    pinned: Option<&Pinned>,
) -> Result<LinkSocket, DialError> {
    let tcp = tokio::time::timeout(timing.connect, crate::utils::net::dial(&target.host, target.port))
        .await
        .map_err(|_| {
            tell_dial(target, &format!("no TCP answer in {} ms", timing.connect.as_millis()));
            DialError::Timeout
        })?
        .map_err(|error| {
            // Refused, no route, reset: the system's words in the log (Windows test 21.11 — the Turbo's computer
            // behind its firewall, and nothing said why).
            tell_dial(target, &format!("TCP: {error}"));
            DialError::Unreachable
        })?;
    let _ = tcp.set_nodelay(true);
    let name = target.server_name().ok_or(DialError::Unreachable)?;
    let tls = tokio::time::timeout(timing.handshake, tokio_rustls::TlsConnector::from(config).connect(name, tcp))
        .await
        .map_err(|_| {
            tell_dial(target, "no TLS handshake in time");
            DialError::Timeout
        })?
        .map_err(|error| {
            tell_dial(target, &format!("TLS: {error}"));
            DialError::Tls { foreign: pinned.is_some_and(Pinned::foreign) }
        })?;
    forget_dial(target);
    let mut request =
        format!("wss://{}{PATH}", target.authority()).into_client_request().map_err(|_| DialError::NoLink)?;
    for (name, value) in headers {
        if let Ok(value) = HeaderValue::from_bytes(value.as_bytes()) {
            if !value.is_empty() {
                request.headers_mut().insert(HeaderName::from_static(name), value);
            }
        }
    }
    let upgraded = tokio::time::timeout(
        timing.handshake,
        tokio_tungstenite::client_async_with_config(
            request,
            tokio_tungstenite::MaybeTlsStream::Rustls(tls),
            Some(ws_config(frames)),
        ),
    )
    .await
    .map_err(|_| DialError::Timeout)?;
    match upgraded {
        Ok((socket, _)) => Ok(socket),
        // Only the helper's own 401 is a refusal: a 403 comes from something between (it answers 403 to a page's
        // `Origin` alone, which a gateway never sends) — a proxy's rule, a firewall —, and passes like any other status.
        // A 426 is another version only when it names one: axum answers 426 too to a request a proxy took the upgrade
        // from.
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => Err(match response.status().as_u16() {
            401 => DialError::Refused,
            429 => DialError::Busy,
            426 if response.headers().contains_key(VERSION_HEADER) => DialError::Version,
            _ => DialError::NoLink,
        }),
        Err(_) => Err(DialError::NoLink),
    }
}

// ── The socket's messages, whichever WebSocket carries them ──

/// A WebSocket message as both sides of the link read and write it: tungstenite's at the gateway, axum's at the
/// helper.
#[derive(Debug, Clone, PartialEq)]
pub enum Wire {
    Text(String),
    Binary(Bytes),
    Ping(Bytes),
    Pong(Bytes),
    Close(u16, String),
}

impl Wire {
    fn from_tungstenite(message: Message) -> Option<Wire> {
        Some(match message {
            Message::Text(text) => Wire::Text(text.to_string()),
            Message::Binary(bytes) => Wire::Binary(bytes),
            Message::Ping(bytes) => Wire::Ping(bytes),
            Message::Pong(bytes) => Wire::Pong(bytes),
            Message::Close(frame) => match frame {
                Some(frame) => Wire::Close(u16::from(frame.code), frame.reason.to_string()),
                None => Wire::Close(1005, String::new()),
            },
            Message::Frame(_) => return None,
        })
    }

    fn into_tungstenite(self) -> Message {
        match self {
            Wire::Text(text) => Message::Text(text.into()),
            Wire::Binary(bytes) => Message::Binary(bytes),
            Wire::Ping(bytes) => Message::Ping(bytes),
            Wire::Pong(bytes) => Message::Pong(bytes),
            Wire::Close(code, reason) => {
                Message::Close(Some(CloseFrame { code: CloseCode::from(code), reason: reason.into() }))
            }
        }
    }

    pub fn json(value: &Value) -> Wire {
        Wire::Text(value.to_string())
    }
}

pub type WireSink = Pin<Box<dyn Sink<Wire, Error = ()> + Send>>;
pub type WireStream = Pin<Box<dyn Stream<Item = Wire> + Send>>;

/// A tungstenite socket as the link's two halves; a read error ends the stream.
pub fn tungstenite_wire<S>(socket: WebSocketStream<S>) -> (WireSink, WireStream)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (sink, stream) = socket.split();
    let sink = sink.sink_map_err(|_| ()).with(|wire: Wire| futures::future::ready(Ok(wire.into_tungstenite())));
    let stream = stream
        .take_while(|item| futures::future::ready(item.is_ok()))
        .filter_map(|item| futures::future::ready(item.ok().and_then(Wire::from_tungstenite)));
    (Box::pin(sink), Box::pin(stream))
}

/// A body piece: `[u32 BE stream id][u8 flags][payload]`, flag 1 — the body's end.
pub fn encode_chunk(id: u32, end: bool, payload: &[u8]) -> Bytes {
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.extend_from_slice(&id.to_be_bytes());
    frame.push(u8::from(end));
    frame.extend_from_slice(payload);
    Bytes::from(frame)
}

/// A body piece read: none when it is too short or its payload too large — a protocol error.
pub fn decode_chunk(frame: &Bytes) -> Option<(u32, bool, Bytes)> {
    if frame.len() < 5 || frame.len() - 5 > CHUNK {
        return None;
    }
    let id = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]);
    Some((id, frame[4] & 1 == 1, frame.slice(5..)))
}

/// A frame's JSON object and its kind; none for what is no object.
pub fn read_text(text: &str) -> Option<(String, Value)> {
    let value: Value = serde_json::from_str(text).ok()?;
    let kind = value.get("t")?.as_str()?.to_string();
    value.is_object().then_some((kind, value))
}

/// The headers a request carries over the link: what the answer depends on, never `authorization`.
pub const CARRIED: [&str; 3] = ["content-type", "accept", "accept-language"];

/// The headers an answer carries back: all but the hop-by-hop ones and cookies.
pub fn carried_back(name: &str) -> bool {
    !matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "set-cookie"
            | "content-length"
    )
}

pub fn headers_json(headers: &HeaderMap, keep: impl Fn(&str) -> bool) -> Value {
    let mut map = serde_json::Map::new();
    for (name, value) in headers {
        if keep(name.as_str()) {
            if let Ok(value) = value.to_str() {
                map.insert(name.as_str().to_string(), Value::String(value.to_string()));
            }
        }
    }
    Value::Object(map)
}

pub fn headers_from(value: &Value, keep: impl Fn(&str) -> bool) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in value.as_object().into_iter().flatten() {
        let name = name.to_ascii_lowercase();
        if !keep(&name) {
            continue;
        }
        if let (Ok(name), Some(Ok(value))) =
            (HeaderName::from_bytes(name.as_bytes()), value.as_str().map(HeaderValue::from_str))
        {
            headers.insert(name, value);
        }
    }
    headers
}

/// A frame's stream id; none for what names none.
fn frame_id(value: &Value) -> Option<u32> {
    value.get("id").and_then(Value::as_u64).and_then(|id| u32::try_from(id).ok())
}

// ── Up: the helper's own requests to the gateway ──

/// Which way a socket's requests go: the gateway's to its helper (`req`, its answer `res`, a failure after it `error`,
/// given up `cancel`), or the helper's own to the gateway (`up`, `up-res`, `up-error`, `up-cancel`) — in the ids above
/// `UP_BIT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    Req,
    Up,
}

impl Way {
    pub fn ask(self) -> &'static str {
        match self {
            Way::Req => "req",
            Way::Up => "up",
        }
    }

    pub fn answer(self) -> &'static str {
        match self {
            Way::Req => "res",
            Way::Up => "up-res",
        }
    }

    pub fn failure(self) -> &'static str {
        match self {
            Way::Req => "error",
            Way::Up => "up-error",
        }
    }

    pub fn cancel(self) -> &'static str {
        match self {
            Way::Req => "cancel",
            Way::Up => "up-cancel",
        }
    }

    /// A stream's id in this way's space: the gateway's below `UP_BIT`, the helper's above it.
    fn id(self, number: u32) -> u32 {
        match self {
            Way::Req => number & !UP_BIT,
            Way::Up => number | UP_BIT,
        }
    }

    fn in_flight(self) -> usize {
        match self {
            Way::Req => IN_FLIGHT,
            Way::Up => UP_IN_FLIGHT,
        }
    }

    /// The headers a request carries: what its answer depends on — the helper's own a range too (a recording played
    /// from the middle) —, never `authorization`.
    fn carried(self, name: &str) -> bool {
        CARRIED.contains(&name) || (self == Way::Up && name == "range")
    }
}

/// A body piece of the helper's own requests (`up`): its stream's id has the top bit.
pub fn up_piece(frame: &Bytes) -> bool {
    frame.first().is_some_and(|byte| byte & 0x80 != 0)
}

/// The person a helper's own request is asked for (a request's extension at the helper, link_serve.rs's pass): who is
/// signed in there — their name, their role there, their id. It travels in the `up` frame, never as a header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpPerson {
    pub name: String,
    pub role: String,
    pub id: String,
}

/// A gateway's door for its helper's own requests (`up`), given with the link (helpers/mod.rs, `EntryDoor`): whether
/// this entry may send them now — the trust rule is `Helpers::takes_up` —, this gateway's routes, and a request's origin
/// as they read it: the person its frame names (`caller::up_origin`; none when it names nobody).
pub trait UpDoor: Send + Sync {
    fn open(&self) -> bool;
    fn router(&self) -> Option<Router>;
    fn origin(&self, person: &Value) -> Option<UpOrigin>;
}

/// A path that stays where it says, before a whitelist reads it (`rest`: without its leading slash): no empty segment,
/// no «.» or «..» — nor their percent-encoded forms or the same with spaces around, which an address reads as the
/// same — and no backslash, query, fragment, control character or encoded slash hidden in a segment (a router decodes
/// the path once: `..%2F` comes as `../`). The pass checks the address made of it against its whitelist once more.
pub fn plain_path(rest: &str) -> bool {
    !rest.contains(|c: char| matches!(c, '\\' | '?' | '#') || c.is_control())
        && rest.split('/').all(|segment| {
            let segment = segment.to_ascii_lowercase();
            let dotted = segment.trim().replace("%2e", ".");
            !matches!(dotted.as_str(), "" | "." | "..") && !segment.contains("%2f") && !segment.contains("%5c")
        })
}

/// What a helper may ask this gateway over its link (`up`): the panel's API, and the health for a reading — the panel
/// reads it through the pass —; never `/home/v1` (the help a helper gives, not takes), a station's pass (`/station/…`),
/// the bridge, the debug routes or anything else. Every segment plain (`plain_path`); the query is the router's.
pub fn up_path(path: &str, method: &Method) -> bool {
    let path = path.split('?').next().unwrap_or("");
    let Some(rest) = path.strip_prefix('/') else { return false };
    let reading = matches!(*method, Method::GET | Method::HEAD);
    plain_path(rest) && (rest.starts_with("api/v1/") || (reading && rest == "health"))
}

// ── The first meeting ──

/// What a pairing gave: the helper as it named itself, its link key (none for a probe), the pin, its status as this
/// home sees it.
#[derive(Debug, Clone)]
pub struct Paired {
    pub helper: HelperIdentity,
    pub key: Option<String>,
    pub pin: HelperPin,
    pub status: Value,
}

/// Why a pairing did not happen.
#[derive(Debug, Clone)]
pub enum PairError {
    BadAddress,
    Unreachable,
    NoLink,
    /// A wrong, used or expired key — or a relay, whose binding differs: the helper does not say which.
    Refused,
    /// The helper's own proof did not hold.
    Mismatch,
    Timeout,
    Busy,
    /// A helper of another DuoWave: its link version is another.
    Version,
    /// This gateway's own check refused the helper (itself, or a helper already listed).
    Checked(ApiError),
}

impl PairError {
    /// The answer of `POST /api/v1/helpers`.
    pub fn into_api(self, url: &str) -> ApiError {
        tracing::warn!("helpers: pairing with {url} failed: {self:?}");
        match self {
            PairError::BadAddress => {
                ApiError::bad_request(crate::tr!("Адрес Турбо: http://адрес:порт или https://…"))
            }
            PairError::Unreachable => {
                ApiError::bad_gateway(crate::tr!("Турбо не отвечает по адресу {url}.", url = url))
            }
            PairError::NoLink => ApiError::bad_gateway(crate::tr!(
                "По этому адресу нет связи DuoWave: возьмите адрес из «Подключить дом» на Турбо или обновите на нём DuoWave."
            )),
            PairError::Refused => ApiError::conflict(crate::tr!(
                "Турбо не приняло ключ подключения: ключ неверный, уже использован или устарел — откройте «Подключить дом» на Турбо и возьмите ключ оттуда."
            )),
            PairError::Mismatch => {
                ApiError::bad_gateway(crate::tr!("Ответ Турбо не сходится — подключение отменено."))
            }
            PairError::Timeout => {
                ApiError::new(StatusCode::GATEWAY_TIMEOUT, crate::tr!("Турбо не ответило вовремя."))
            }
            PairError::Busy => {
                ApiError::unavailable(crate::tr!("Турбо занято подключениями — попробуйте через минуту."))
            }
            PairError::Version => ApiError::conflict(crate::tr!("Турбо другой версии DuoWave — обновите оба.")),
            PairError::Checked(error) => error,
        }
    }
}

impl From<DialError> for PairError {
    fn from(error: DialError) -> PairError {
        match error {
            DialError::Unreachable => PairError::Unreachable,
            DialError::Tls { .. } | DialError::NoLink => PairError::NoLink,
            DialError::Refused => PairError::Refused,
            DialError::Busy => PairError::Busy,
            DialError::Timeout => PairError::Timeout,
            DialError::Version => PairError::Version,
        }
    }
}

/// The next JSON frame of a pairing; a close 4401 is the helper's refusal.
async fn next_json<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
) -> Result<(String, Value), PairError> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => return read_text(&text).ok_or(PairError::Mismatch),
            Some(Ok(Message::Close(frame))) => {
                return Err(match frame.map(|frame| u16::from(frame.code)) {
                    Some(REFUSED) => PairError::Refused,
                    // The helper could not confirm the key this gateway unwrapped.
                    Some(PROTOCOL) => PairError::Mismatch,
                    _ => PairError::NoLink,
                });
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
            Some(Ok(Message::Binary(_))) => return Err(PairError::Mismatch),
            Some(Err(_)) | None => return Err(PairError::Unreachable),
        }
    }
}

async fn close<S: AsyncRead + AsyncWrite + Unpin>(socket: &mut WebSocketStream<S>, code: u16) {
    let frame = CloseFrame { code: CloseCode::from(code), reason: "".into() };
    let _ = tokio::time::timeout(Duration::from_secs(2), socket.close(Some(frame))).await;
}

/// The first meeting with a helper at `url` (`https://host:port`): its certificate recorded and bound, the key and
/// the home proven both ways, then `check` on the helper as it named itself and the pin this gateway would keep —
/// before anything is spent. A probe stops there; else the helper's link key comes, and its `confirmed`.
pub async fn pair(
    url: &str,
    key: &ConnectionKey,
    me: &Me,
    home_key: &str,
    timing: &LinkTiming,
    probe: bool,
    check: &(dyn Fn(&HelperIdentity, &HelperPin) -> Result<(), ApiError> + Sync),
) -> Result<Paired, PairError> {
    let target = Target::of(url).ok_or(PairError::BadAddress)?;
    let meeting = FirstMeeting::new();
    let config = client_config(Some(meeting.clone()), true).ok_or(PairError::NoLink)?;
    let headers = vec![(PAIR_HEADER, "pair".to_string()), (VERSION_HEADER, VERSION.to_string())];
    let mut socket = dial(&target, config, headers, PAIR_FRAME_MAX, timing, None).await?;
    let Some(binding) = binding_of(&target, meeting.seen()) else {
        close(&mut socket, DONE).await;
        return Err(PairError::Mismatch);
    };
    pair_over(&mut socket, key, me, home_key, &binding, timing, probe, check).await
}

/// What the first meeting binds: the DNS name dialed when a public chain vouched for it, else the certificate's
/// fingerprint; nothing without a certificate.
pub fn binding_of(target: &Target, seen: Option<Seen>) -> Option<Binding> {
    let seen = seen?;
    Some(if seen.public && target.dns { Binding::Name(target.host.clone()) } else { Binding::Tls(seen.fingerprint) })
}

/// The pairing's exchange over a socket already up, within `timing.handshake`; the socket closed after it (a
/// refusal is the helper's own close).
#[allow(clippy::too_many_arguments)]
pub async fn pair_over<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    key: &ConnectionKey,
    me: &Me,
    home_key: &str,
    binding: &Binding,
    timing: &LinkTiming,
    probe: bool,
    check: &(dyn Fn(&HelperIdentity, &HelperPin) -> Result<(), ApiError> + Sync),
) -> Result<Paired, PairError> {
    let exchanged = tokio::time::timeout(timing.handshake, exchange(socket, key, me, home_key, binding, probe, check))
        .await
        .unwrap_or(Err(PairError::Timeout));
    match &exchanged {
        Err(PairError::Refused) => {}
        _ => close(socket, DONE).await,
    }
    exchanged
}

async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    key: &ConnectionKey,
    me: &Me,
    home_key: &str,
    binding: &Binding,
    probe: bool,
    check: &(dyn Fn(&HelperIdentity, &HelperPin) -> Result<(), ApiError> + Sync),
) -> Result<Paired, PairError> {
    let send = |value: Value| Message::Text(value.to_string().into());
    let nonce_g = random_32();
    let mut pair = json!({
        "t": "pair", "v": VERSION, "nonce": b64(&nonce_g), "gateway": me.gateway.id, "home": me.home.id,
    });
    if probe {
        pair["probe"] = Value::Bool(true);
    }
    socket.send(send(pair)).await.map_err(|_| PairError::Unreachable)?;
    let (kind, challenge) = next_json(socket).await?;
    let nonce_h = unb64_32(challenge.get("nonce").and_then(Value::as_str)).filter(|_| kind == "challenge");
    let nonce_h = nonce_h.ok_or(PairError::Mismatch)?;
    let transcript =
        Transcript { nonce_g: &nonce_g, nonce_h: &nonce_h, binding, gateway: &me.gateway.id, home: &me.home.id };
    let proof = json!({
        "t": "proof",
        "mac": b64(&transcript.proof(key.bytes(), "gateway")),
        "homeMac": b64(&transcript.home_proof(home_key)),
    });
    socket.send(send(proof)).await.map_err(|_| PairError::Unreachable)?;
    let (kind, paired) = next_json(socket).await?;
    let mac = unb64_32(paired.get("mac").and_then(Value::as_str)).filter(|_| kind == "paired");
    if !mac.is_some_and(|mac| same(&mac, &transcript.proof(key.bytes(), "helper"))) {
        return Err(PairError::Mismatch);
    }
    // Read as a contract's (contract.rs): its id goes into the travelling list, fixed; one that does not read as a
    // gateway's is none, and the check refuses it.
    let helper = HelperIdentity::read(paired.get("helper"));
    let status = paired.get("status").cloned().unwrap_or(Value::Null);
    let pin = binding.pin();
    check(&helper, &pin).map_err(PairError::Checked)?;
    if probe {
        return Ok(Paired { helper, key: None, pin, status });
    }
    // The key comes wrapped under the connection key and this transcript (`Transcript::key_wrap`).
    let pad = transcript.key_wrap(key.bytes());
    let link_key = paired.get("key").and_then(Value::as_str).and_then(|wrapped| wrapped_link_key(wrapped, &pad));
    let link_key = link_key.ok_or(PairError::Mismatch)?;
    let name = |name: &str| clean_name(name).chars().take(NAME_SENT).collect::<String>();
    let ok = json!({
        "t": "ok", "gateway": { "name": name(&me.gateway.name) }, "home": { "name": name(&me.home.name) },
        "keyMac": b64(&transcript.key_confirm(&link_key_hash(&link_key))),
    });
    socket.send(send(ok)).await.map_err(|_| PairError::Unreachable)?;
    match next_json(socket).await? {
        (kind, _) if kind == "confirmed" => Ok(Paired { helper, key: Some(link_key), pin, status }),
        _ => Err(PairError::Mismatch),
    }
}

// ── The link ──

/// The link's times; tests scale them down.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkTiming {
    /// The TCP connect.
    pub connect: Duration,
    /// The TLS handshake, the upgrade, a pairing's whole exchange, the welcome.
    pub handshake: Duration,
    pub ping: Duration,
    /// Nothing from the helper this long: the link is dead; a drop stays «on the line» this long while it reconnects.
    pub silence: Duration,
    /// The pauses between tries after a drop or a failed try; the last one repeats.
    pub pauses: Vec<Duration>,
    /// A link up this long starts the pauses again.
    pub steady: Duration,
}

impl Default for LinkTiming {
    fn default() -> LinkTiming {
        LinkTiming {
            connect: Duration::from_secs(10),
            handshake: Duration::from_secs(10),
            ping: Duration::from_secs(15),
            silence: Duration::from_secs(30),
            pauses: [1, 2, 5, 10, 30, 60].into_iter().map(Duration::from_secs).collect(),
            steady: Duration::from_secs(60),
        }
    }
}

impl LinkTiming {
    /// The pause after this many failures in a row.
    pub fn pause(&self, failures: usize) -> Duration {
        self.pauses.get(failures).or(self.pauses.last()).copied().unwrap_or(Duration::from_secs(60))
    }
}

/// What a link needs to reach its helper.
#[derive(Clone)]
pub struct LinkSetup {
    /// The entry's id.
    pub entry: String,
    /// `https://host:port`.
    pub url: String,
    pub pin: HelperPin,
    /// The link key.
    pub key: String,
    /// The helper's id, fixed at pairing.
    pub helper_id: String,
    /// This gateway and its home as they are named now: read at each connect.
    pub me: Arc<dyn Fn() -> Me + Send + Sync>,
    /// The door for the helper's own requests (`up`); none: this gateway takes none of them.
    pub up: Option<Arc<dyn UpDoor>>,
}

/// Who hears what a link tells: its entry's id and the news.
pub type NewsSink = Arc<dyn Fn(&str, LinkNews) + Send + Sync>;

/// What the link tells its owner: on the line (with the helper's status), a new status, away (why).
#[derive(Debug, Clone, PartialEq)]
pub enum LinkNews {
    Up(Value),
    Status(Value),
    Down(String),
}

/// The link as the panel sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkState {
    /// No outcome yet.
    Connecting,
    Up {
        since: String,
    },
    Down {
        since: String,
        reason: String,
    },
}

/// Why a request could not go over the link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// The link is not up now.
    Away,
    /// Too many requests in flight.
    Busy,
    /// The link broke before the answer.
    Broken(String),
}

/// How a connected session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum End {
    Stopped,
    /// 1001: the helper stops.
    Going,
    Refused,
    Protocol,
    Silent,
    /// A drop by error: the grace while it reconnects.
    Dropped,
}

/// The link to one helper: one task that connects, serves, and tries again after the pauses, until stopped.
pub struct Link {
    setup: LinkSetup,
    timing: LinkTiming,
    news: NewsSink,
    state: watch::Sender<LinkState>,
    status: Mutex<Option<Value>>,
    session: Mutex<Option<Arc<Session>>>,
    wake: Notify,
    stop: CancellationToken,
    bye: AtomicBool,
}

/// How long a link waits before its next try: the pauses in turn after each failed try or drop, from the first again
/// once a link stayed up `steady` — a link that kept failing and came back only for a moment goes on where it stopped.
#[derive(Debug, Default)]
pub(crate) struct Backoff {
    failures: usize,
}

impl Backoff {
    /// The pause after a try; `up` — how long its link stayed up, none when it never came up.
    pub(crate) fn after(&mut self, timing: &LinkTiming, up: Option<Duration>) -> Duration {
        if up.is_some_and(|up| up >= timing.steady) {
            self.failures = 0;
        }
        let pause = timing.pause(self.failures);
        self.failures += 1;
        pause
    }
}

impl Link {
    pub fn start(setup: LinkSetup, timing: LinkTiming, news: NewsSink) -> Arc<Link> {
        let link = Link::idle(setup, timing, news);
        tokio::spawn(link.clone().run());
        link
    }

    /// A link that does not dial: its sessions are run by hand (tests).
    pub(crate) fn idle(setup: LinkSetup, timing: LinkTiming, news: NewsSink) -> Arc<Link> {
        Arc::new(Link {
            setup,
            timing,
            news,
            state: watch::Sender::new(LinkState::Connecting),
            status: Mutex::new(None),
            session: Mutex::new(None),
            wake: Notify::new(),
            stop: CancellationToken::new(),
            bye: AtomicBool::new(false),
        })
    }

    pub fn state(&self) -> LinkState {
        self.state.borrow().clone()
    }

    pub fn status(&self) -> Option<Value> {
        locked(&self.status).clone()
    }

    /// A socket is up now: requests go.
    pub fn connected(&self) -> bool {
        locked(&self.session).is_some()
    }

    /// Waits until the link has an outcome, at most `wait`.
    pub async fn settled(&self, wait: Duration) -> LinkState {
        let mut state = self.state.subscribe();
        let _ = tokio::time::timeout(wait, state.wait_for(|state| *state != LinkState::Connecting)).await;
        self.state()
    }

    /// Try now («Проверить сейчас»): the current pause is skipped, its count kept.
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    /// This home removed the helper: `bye` over a live link (the helper forgets this home), then the end.
    pub fn bye(&self) {
        self.bye.store(true, Ordering::SeqCst);
        self.stop.cancel();
    }

    /// The end: 1000 over a live link, no more tries.
    pub fn stop(&self) {
        self.stop.cancel();
    }

    pub fn stopped(&self) -> bool {
        self.stop.is_cancelled()
    }

    /// What it tells, unless it was stopped: a link stopped — replaced by another for its entry, kept out — says
    /// nothing more, so its last news never lands on the entry's next link.
    fn tell(&self, news: LinkNews) {
        if !self.stop.is_cancelled() {
            (self.news)(&self.setup.entry, news);
        }
    }

    fn set_up(&self, status: Value) {
        *locked(&self.status) = Some(status.clone());
        self.state.send_if_modified(|state| {
            if matches!(state, LinkState::Up { .. }) {
                return false;
            }
            *state = LinkState::Up { since: now_iso() };
            true
        });
        self.tell(LinkNews::Up(status));
    }

    fn set_status(&self, status: Value) {
        *locked(&self.status) = Some(status.clone());
        self.tell(LinkNews::Status(status));
    }

    /// Away, for one of the static reasons (kept as written; translated where it is shown).
    fn set_down(&self, reason: &'static str) {
        let changed = self.state.send_if_modified(|state| {
            if matches!(state, LinkState::Down { reason: known, .. } if known == reason) {
                return false;
            }
            *state = LinkState::Down { since: now_iso(), reason: reason.to_string() };
            true
        });
        if changed {
            self.tell(LinkNews::Down(reason.to_string()));
        }
    }

    /// Connect, serve, and again after the pauses; a refusal or a changed helper ends the tries.
    async fn run(self: Arc<Self>) {
        let mut backoff = Backoff::default();
        let mut grace: Option<Instant> = None;
        loop {
            if self.stop.is_cancelled() {
                break;
            }
            let connected = tokio::select! {
                connected = self.connect() => connected,
                _ = self.stop.cancelled() => break,
            };
            let mut up = None;
            match connected {
                Ok((socket, status)) => {
                    let (sink, stream) = tungstenite_wire(socket);
                    let session = Session::start(sink);
                    *locked(&self.session) = Some(session.clone());
                    grace = None;
                    let up_at = Instant::now();
                    self.set_up(status);
                    let end = session.serve(stream, &self).await;
                    *locked(&self.session) = None;
                    session.fail_all(&crate::core::tx(super::AWAY));
                    up = Some(up_at.elapsed());
                    match end {
                        End::Stopped => break,
                        End::Refused => {
                            self.set_down(REFUSED_WORDS);
                            break;
                        }
                        End::Going | End::Protocol | End::Silent => self.set_down(super::AWAY),
                        End::Dropped => grace = Some(Instant::now() + self.timing.silence),
                    }
                }
                Err(Failure::Final(words)) => {
                    self.set_down(words);
                    break;
                }
                Err(Failure::Again(words)) => {
                    if grace.is_none_or(|until| Instant::now() >= until) {
                        grace = None;
                        self.set_down(words);
                    }
                }
            }
            let next = Instant::now() + backoff.after(&self.timing, up);
            loop {
                let grace_end = grace.unwrap_or(next);
                tokio::select! {
                    _ = tokio::time::sleep_until(next) => break,
                    _ = tokio::time::sleep_until(grace_end), if grace.is_some_and(|until| until < next) => {
                        grace = None;
                        self.set_down(super::AWAY);
                    }
                    _ = self.wake.notified() => break,
                    _ = self.stop.cancelled() => return,
                }
            }
        }
    }

    /// One try: the pinned TLS (or the public chain for the pinned name), the upgrade with the link key's id and this
    /// gateway's names, the proof of the key over the helper's challenge, the welcome — from the very helper paired
    /// with.
    async fn connect(&self) -> Result<(LinkSocket, Value), Failure> {
        let target = Target::of(&self.setup.url).ok_or(Failure::Final(REFUSED_WORDS))?;
        let (config, pinned, binding) = match (&self.setup.pin.sha256, &self.setup.pin.public) {
            (Some(hash), _) => {
                let pin: [u8; 32] =
                    hex::decode(hash).ok().and_then(|pin| pin.try_into().ok()).ok_or(Failure::Final(REFUSED_WORDS))?;
                let pinned = Pinned::new(pin);
                (client_config(Some(pinned.clone()), false), Some(pinned), Binding::Tls(pin))
            }
            (None, Some(host)) => {
                if !target.dns || target.host != *host {
                    return Err(Failure::Final(MOVED));
                }
                (client_config(None, true), None, Binding::Name(host.clone()))
            }
            (None, None) => return Err(Failure::Final(REFUSED_WORDS)),
        };
        let link_id = link_key_id(&self.setup.key).ok_or(Failure::Final(REFUSED_WORDS))?.to_string();
        let config = config.ok_or(Failure::Again(super::AWAY))?;
        let me = (self.setup.me)();
        let name: String = clean_name(&me.gateway.name).chars().take(NAME_SENT).collect();
        // The ids the proof covers, as the helper reads them from the upgrade: one a header cannot carry goes as none.
        let said = |value: &str| {
            HeaderValue::from_bytes(value.as_bytes()).map(|_| value.trim().to_string()).unwrap_or_default()
        };
        let (gateway, home) = (said(&me.gateway.id), said(&me.home.id));
        let mut headers = vec![
            (LINK_ID_HEADER, link_id.clone()),
            (VERSION_HEADER, VERSION.to_string()),
            (GATEWAY_HEADER, gateway.clone()),
            (GATEWAY_NAME_HEADER, name),
            (HOME_HEADER, home.clone()),
        ];
        // This gateway takes the helper's own requests while the entry is a helper at home: only then is it asked.
        if self.setup.up.as_ref().is_some_and(|door| door.open()) {
            headers.push((UP_HEADER, "1".to_string()));
        }
        let mut socket =
            dial(&target, config, headers, FRAME_MAX, &self.timing, pinned.as_deref()).await.map_err(|error| {
                match error {
                    DialError::Refused => Failure::Final(REFUSED_WORDS),
                    DialError::Version => Failure::Again(OTHER_VERSION),
                    DialError::Tls { foreign: true } => Failure::Again(FOREIGN),
                    DialError::Timeout => Failure::Again(LATE),
                    _ => Failure::Again(super::AWAY),
                }
            })?;
        let (kind, challenge) = opening(&mut socket, self.timing.handshake).await?;
        let Some(nonce) = unb64_32(challenge.get("nonce").and_then(Value::as_str)).filter(|_| kind == "challenge")
        else {
            close(&mut socket, PROTOCOL).await;
            return Err(Failure::Again(super::AWAY));
        };
        let mac = link_proof(&link_key_hash(&self.setup.key), &nonce, &binding, &link_id, &gateway, &home);
        let proof = json!({ "t": "proof", "mac": b64(&mac) });
        if socket.send(Message::Text(proof.to_string().into())).await.is_err() {
            return Err(Failure::Again(super::AWAY));
        }
        let status = match opening(&mut socket, self.timing.handshake).await? {
            (kind, welcome) if kind == "welcome" => welcome.get("status").cloned().unwrap_or(Value::Null),
            _ => {
                close(&mut socket, PROTOCOL).await;
                return Err(Failure::Again(super::AWAY));
            }
        };
        if !self.same_helper(&status) {
            close(&mut socket, PROTOCOL).await;
            return Err(Failure::Final(ANOTHER));
        }
        Ok((socket, status))
    }

    /// The status names the helper this entry was paired with.
    fn same_helper(&self, status: &Value) -> bool {
        HelperStatus::read(status).is_some_and(|status| status.contract.helper.id == self.setup.helper_id)
    }

    /// A request over the link; the answer streams back as it comes.
    pub(crate) async fn call(&self, request: Request) -> Result<Response, LinkError> {
        let session = locked(&self.session).clone().ok_or(LinkError::Away)?;
        session.call(request).await
    }
}

/// Why a try failed: for good (until the entry changes), or for now — in the words of the reasons below.
#[derive(Debug, PartialEq, Eq)]
enum Failure {
    Final(&'static str),
    Again(&'static str),
}

/// The helper's next JSON frame while a link opens, within `wait`: its close 4401 is a refusal, for good; no place for
/// one more link (`BUSY`), silence or anything else — again later.
async fn opening(socket: &mut LinkSocket, wait: Duration) -> Result<(String, Value), Failure> {
    let frame = tokio::time::timeout(wait, async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => return Ok(read_text(&text)),
                Some(Ok(Message::Close(frame))) => return Err(frame.map(|frame| u16::from(frame.code))),
                Some(Ok(_)) => continue,
                _ => return Err(None),
            }
        }
    })
    .await;
    match frame {
        Ok(Ok(Some(frame))) => Ok(frame),
        Ok(Err(Some(REFUSED))) => Err(Failure::Final(REFUSED_WORDS)),
        Ok(Err(_)) => Err(Failure::Again(super::AWAY)),
        Ok(Ok(None)) | Err(_) => {
            close(socket, PROTOCOL).await;
            Err(Failure::Again(super::AWAY))
        }
    }
}

/// Why a link is away, kept as written (static; their English is in placement.json, `AWAY`'s in services.json) and put
/// in the reader's language where it is shown (`tx_dyn`): a link's own task has no reader.
pub const REFUSED_WORDS: &str = "Турбо не приняло ключ связи — подключите его заново";
pub const FOREIGN: &str = "чужой сертификат: это не то Турбо, что было подключено";
pub const MOVED: &str = "адрес Турбо изменился — подключите его заново";
pub const ANOTHER: &str = "это другое Турбо — подключите его заново";
pub const LATE: &str = "Турбо не ответило вовремя";
pub const OTHER_VERSION: &str = "Турбо другой версии DuoWave — обновите оба";

// ── A session: the requests in flight on one connected socket ──

/// An answer's head.
struct Head {
    status: u16,
    headers: HeaderMap,
    body: bool,
}

/// One request in flight: its answer's head while awaited, its body's queue once streaming.
struct Slot {
    head: Option<oneshot::Sender<Result<Head, String>>>,
    body: Option<mpsc::UnboundedSender<Result<Bytes, io::Error>>>,
    queued: Arc<AtomicUsize>,
    room: Arc<Notify>,
}

/// A connected socket's requests one way (`Way`): the way out, the streams open, what their readers have not read yet.
/// The gateway's own to its helper (the link's session), or the helper's own to its gateway (`up`, link_serve.rs).
pub(crate) struct Session {
    out: Out,
    way: Way,
    streams: Mutex<HashMap<u32, Slot>>,
    next: AtomicU32,
    queued: Arc<AtomicUsize>,
    gone: CancellationToken,
}

impl Session {
    /// The writer started over the socket's sink: the gateway's requests.
    pub(crate) fn start(sink: WireSink) -> Arc<Session> {
        Session::over(Out::start(sink), Way::Req)
    }

    /// Requests over a way out already running: the helper's own (`Way::Up`) share its link's writer.
    pub(crate) fn over(out: Out, way: Way) -> Arc<Session> {
        let gone = out.gone.clone();
        Arc::new(Session {
            out,
            way,
            streams: Mutex::new(HashMap::new()),
            next: AtomicU32::new(1),
            queued: Arc::new(AtomicUsize::new(0)),
            gone,
        })
    }

    /// The requests open now (the helper side's tests).
    #[cfg(all(test, feature = "pc"))]
    pub(crate) fn open_streams(&self) -> usize {
        locked(&self.streams).len()
    }

    /// A control frame (`cancel`, a ping): never waits.
    fn control(&self, wire: Wire) {
        self.out.control(wire);
    }

    /// A frame that may wait for room, at most `STALL`; past it the link is closed.
    async fn send(&self, wire: Wire) -> bool {
        self.out.send(wire).await
    }

    fn close(&self, code: u16) {
        self.out.close(code);
    }

    /// A stream given up: `cancel` (`up-cancel`) sent, its reader told.
    fn cancel(self: &Arc<Self>, id: u32, why: &str) {
        if let Some(slot) = locked(&self.streams).remove(&id) {
            self.release(&slot);
            if let Some(head) = slot.head {
                let _ = head.send(Err(why.to_string()));
            }
            if let Some(body) = slot.body {
                let _ = body.send(Err(io::Error::other(why.to_string())));
            }
            self.control(Wire::json(&json!({ "t": self.way.cancel(), "id": id })));
        }
    }

    fn release(&self, slot: &Slot) {
        take(&self.queued, slot.queued.swap(0, Ordering::SeqCst));
    }

    /// The socket is gone: every request still waiting fails, every body still streaming ends in an error.
    pub(crate) fn fail_all(&self, why: &str) {
        self.gone.cancel();
        for (_, slot) in locked(&self.streams).drain() {
            self.release(&slot);
            if let Some(head) = slot.head {
                let _ = head.send(Err(why.to_string()));
            }
            if let Some(body) = slot.body {
                let _ = body.send(Err(io::Error::other(why.to_string())));
            }
        }
    }

    /// Reads the socket until it ends: answers and their bodies to their requests, the helper's status to the link;
    /// a ping every `ping`, the end after `silence` of nothing. The helper's own requests (`up`) run beside them.
    async fn serve(self: &Arc<Self>, mut stream: WireStream, link: &Link) -> End {
        let timing = &link.timing;
        let mut last = Instant::now();
        let mut ping = tokio::time::interval_at(Instant::now() + timing.ping, timing.ping);
        let (done, mut finished) = mpsc::unbounded_channel();
        let mut up = Up::new(self.out.clone(), link.setup.up.clone(), done);
        loop {
            tokio::select! {
                frame = stream.next() => {
                    let Some(frame) = frame else { return End::Dropped };
                    last = Instant::now();
                    match frame {
                        Wire::Text(text) => {
                            if let Some(end) = self.on_text(&text, link, &mut up) {
                                return end;
                            }
                        }
                        Wire::Binary(bytes) => {
                            // The helper's own requests' bodies to their handlers here; the answers' to their callers.
                            let fed = if up_piece(&bytes) { up.feed(&bytes) } else { self.on_chunk(&bytes).await };
                            if !fed {
                                self.close(PROTOCOL);
                                return End::Protocol;
                            }
                        }
                        Wire::Close(code, _) => {
                            return match code {
                                GOING => End::Going,
                                REFUSED => End::Refused,
                                PROTOCOL | SILENT => End::Protocol,
                                _ => End::Dropped,
                            }
                        }
                        Wire::Ping(_) | Wire::Pong(_) => {}
                    }
                }
                Some(id) = finished.recv() => up.finished(id),
                _ = ping.tick() => {
                    if last.elapsed() >= timing.silence {
                        self.close(SILENT);
                        return End::Silent;
                    }
                    self.control(Wire::Ping(Bytes::new()));
                    up.give_up_late_bodies(UP_BODY_SILENCE, UP_BODY_WHOLE);
                }
                _ = link.stop.cancelled() => {
                    if link.bye.load(Ordering::SeqCst) {
                        self.out.last(Wire::json(&json!({ "t": "bye" })));
                    }
                    self.close(DONE);
                    // The writer gets a moment to write the last frames.
                    self.out.closed(Duration::from_secs(1)).await;
                    return End::Stopped;
                }
                _ = self.gone.cancelled() => {
                    self.close(PROTOCOL);
                    return End::Protocol;
                }
            }
        }
    }

    /// A text frame: an answer's head, a body's error, the helper's news, the helper's own request or its cancel; unknown
    /// kinds passed by. Some(end) when the link must end (another helper, an `up` that breaks the protocol).
    fn on_text(self: &Arc<Self>, text: &str, link: &Link, up: &mut Up) -> Option<End> {
        let (kind, value) = read_text(text)?;
        match (kind.as_str(), frame_id(&value)) {
            ("res", Some(_)) => self.on_answer(&value),
            ("error", Some(_)) => self.on_failure(&value),
            ("up", Some(_)) => {
                if let Err(code) = up.request(&value) {
                    self.close(code);
                    return Some(End::Protocol);
                }
            }
            ("up-cancel", Some(id)) => up.drop_stream(id),
            ("event", _) if value.get("name").and_then(Value::as_str) == Some("status") => {
                let status = value.get("data").cloned().unwrap_or(Value::Null);
                if !link.same_helper(&status) {
                    self.close(PROTOCOL);
                    link.set_down(ANOTHER);
                    link.stop.cancel();
                    return Some(End::Stopped);
                }
                link.set_status(status);
            }
            _ => {}
        }
        None
    }

    /// An answer's head (`res`, `up-res`) to its request; its body, when one comes, streams to it.
    pub(crate) fn on_answer(&self, value: &Value) {
        let Some(id) = frame_id(value) else { return };
        let head = Head {
            status: value.get("status").and_then(Value::as_u64).and_then(|s| u16::try_from(s).ok()).unwrap_or(502),
            headers: headers_from(&value["headers"], carried_back),
            body: value.get("body").and_then(Value::as_bool).unwrap_or(false),
        };
        let mut streams = locked(&self.streams);
        let body = head.body;
        if let Some(slot) = streams.get_mut(&id) {
            if let Some(sender) = slot.head.take() {
                let _ = sender.send(Ok(head));
            }
            if !body {
                streams.remove(&id);
            }
        }
    }

    /// A request that failed at the other end (`error`, `up-error`): its waiter, or its body's reader, told.
    pub(crate) fn on_failure(&self, value: &Value) {
        let Some(id) = frame_id(value) else { return };
        let message = value.get("message").and_then(Value::as_str).unwrap_or("").to_string();
        if let Some(slot) = locked(&self.streams).remove(&id) {
            self.release(&slot);
            if let Some(head) = slot.head {
                let _ = head.send(Err(message.clone()));
            }
            if let Some(body) = slot.body {
                let _ = body.send(Err(io::Error::other(message)));
            }
        }
    }

    /// A body piece to its reader. Its stream's reader may leave at most `STREAM_QUEUE` unread — past `STALL` of it
    /// the stream is cancelled and the others go on —, the link's at most `LINK_QUEUE` — past it the newest stream
    /// is cancelled. False on a malformed piece: a protocol error.
    pub(crate) async fn on_chunk(self: &Arc<Self>, frame: &Bytes) -> bool {
        let Some((id, end, data)) = decode_chunk(frame) else { return false };
        let room = {
            let streams = locked(&self.streams);
            let Some(slot) = streams.get(&id) else { return true };
            (slot.queued.clone(), slot.room.clone())
        };
        let (queued, room) = room;
        if !data.is_empty() {
            let deadline = Instant::now() + STALL;
            while queued.load(Ordering::SeqCst) + data.len() > STREAM_QUEUE {
                if tokio::time::timeout_at(deadline, room.notified()).await.is_err() {
                    self.cancel(id, "the reader stalled");
                    return true;
                }
            }
            while self.queued.load(Ordering::SeqCst) + data.len() > LINK_QUEUE {
                let newest = locked(&self.streams)
                    .iter()
                    .filter(|(_, slot)| slot.queued.load(Ordering::SeqCst) > 0 || slot.body.is_some())
                    .map(|(id, _)| *id)
                    .max();
                let Some(newest) = newest else { break };
                self.cancel(newest, "too much unread on the link");
                if newest == id {
                    return true;
                }
            }
        }
        let mut streams = locked(&self.streams);
        let Some(slot) = streams.get_mut(&id) else { return true };
        if !data.is_empty() {
            if let Some(body) = &slot.body {
                slot.queued.fetch_add(data.len(), Ordering::SeqCst);
                self.queued.fetch_add(data.len(), Ordering::SeqCst);
                let _ = body.send(Ok(data));
            }
        }
        if end {
            // The body is whole: its reader ends after what is queued.
            slot.body = None;
            if slot.head.is_none() && slot.queued.load(Ordering::SeqCst) == 0 {
                streams.remove(&id);
            }
        }
        true
    }

    /// A request over this socket: its head, its body in pieces, then its answer, streamed as it comes.
    async fn call(self: &Arc<Self>, request: Request) -> Result<Response, LinkError> {
        self.call_within(request, SAFETY).await
    }

    /// A request over this socket, its answer's head awaited at most `wait`: past it the request is given up (its
    /// `cancel` sent) — so a gateway that never answers the helper's own request leaves nothing behind.
    pub(crate) async fn call_within(self: &Arc<Self>, request: Request, wait: Duration) -> Result<Response, LinkError> {
        if self.gone.is_cancelled() {
            return Err(LinkError::Away);
        }
        let (head_tx, head_rx) = oneshot::channel();
        let (body_tx, body_rx) = mpsc::unbounded_channel();
        let (queued, room) = (Arc::new(AtomicUsize::new(0)), Arc::new(Notify::new()));
        let id = {
            let mut streams = locked(&self.streams);
            if streams.len() >= self.way.in_flight() {
                return Err(LinkError::Busy);
            }
            let id = self.way.id(self.next.fetch_add(1, Ordering::SeqCst));
            let slot = Slot { head: Some(head_tx), body: Some(body_tx), queued: queued.clone(), room: room.clone() };
            streams.insert(id, slot);
            id
        };
        // Until its answer is handed out, a request dropped by its caller is cancelled at the other end.
        let mut guard = Cancel { id, session: Arc::downgrade(self), armed: true };
        let (parts, body) = request.into_parts();
        let path = parts.uri.path_and_query().map(|path| path.as_str().to_string()).unwrap_or_else(|| "/".into());
        let has_body = !axum::body::HttpBody::is_end_stream(&body);
        let way = self.way;
        let mut head = json!({
            "t": way.ask(), "id": id, "method": parts.method.as_str(), "path": path,
            "headers": headers_json(&parts.headers, |name| way.carried(name)), "body": has_body,
        });
        // The helper's own request names the person it is asked for — in the frame, never as a header.
        if let Some(person) = parts.extensions.get::<UpPerson>().filter(|_| way == Way::Up) {
            head["person"] = json!({ "name": person.name, "role": person.role, "id": person.id });
        }
        if !self.send(Wire::json(&head)).await {
            return Err(LinkError::Away);
        }
        if has_body {
            tokio::spawn(pump(self.clone(), id, body));
        }
        let head = match tokio::time::timeout(wait, head_rx).await {
            Ok(Ok(Ok(head))) => head,
            Ok(Ok(Err(why))) => return Err(LinkError::Broken(why)),
            Ok(Err(_)) => return Err(LinkError::Broken(crate::core::tx(super::AWAY))),
            Err(_) => return Err(LinkError::Broken(crate::core::tx(LATE))),
        };
        guard.armed = false;
        let status = StatusCode::from_u16(head.status).unwrap_or(StatusCode::BAD_GATEWAY);
        let body = if head.body {
            Body::from_stream(Pieces {
                pieces: body_rx,
                queued,
                link_queued: self.queued.clone(),
                room,
                cancel: Cancel { id, session: Arc::downgrade(self), armed: true },
            })
        } else {
            Body::empty()
        };
        let mut response = Response::new(body);
        *response.status_mut() = status;
        *response.headers_mut() = head.headers;
        Ok(response)
    }
}

/// Takes `amount` from a counter, never below zero.
fn take(counter: &AtomicUsize, amount: usize) {
    let mut left = counter.load(Ordering::SeqCst);
    while let Err(now) =
        counter.compare_exchange_weak(left, left.saturating_sub(amount), Ordering::SeqCst, Ordering::SeqCst)
    {
        left = now;
    }
}

/// The request's body to the helper, in pieces of at most `CHUNK`; `BODY_MAX` in all, past it the stream is cancelled.
/// A stream gone meanwhile (its answer came, refused before the body's end) has its body ended at once: the helper lets
/// a refused body's place go at its end, not after its silence.
async fn pump(session: Arc<Session>, id: u32, body: Body) {
    let mut stream = body.into_data_stream();
    let mut sent = 0usize;
    while let Some(piece) = stream.next().await {
        if !locked(&session.streams).contains_key(&id) {
            let _ = session.send(Wire::Binary(encode_chunk(id, true, &[]))).await;
            return;
        }
        let Ok(piece) = piece else {
            session.cancel(id, "the request's body broke");
            return;
        };
        sent += piece.len();
        if sent > BODY_MAX {
            session.cancel(id, "the request's body is too large");
            return;
        }
        for part in piece.chunks(CHUNK) {
            if !session.send(Wire::Binary(encode_chunk(id, false, part))).await {
                return;
            }
        }
    }
    let _ = session.send(Wire::Binary(encode_chunk(id, true, &[]))).await;
}

/// Cancels its request at the helper when dropped while armed.
struct Cancel {
    id: u32,
    session: std::sync::Weak<Session>,
    armed: bool,
}

impl Drop for Cancel {
    fn drop(&mut self) {
        if self.armed {
            if let Some(session) = self.session.upgrade() {
                session.cancel(self.id, "the caller went away");
            }
        }
    }
}

/// An answer's body as its pieces come; the space they took given back as they are read. Dropped before its end, it
/// cancels the stream at the helper.
struct Pieces {
    pieces: mpsc::UnboundedReceiver<Result<Bytes, io::Error>>,
    queued: Arc<AtomicUsize>,
    link_queued: Arc<AtomicUsize>,
    room: Arc<Notify>,
    cancel: Cancel,
}

impl Stream for Pieces {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.pieces.poll_recv(cx) {
            Poll::Ready(Some(Ok(piece))) => {
                take(&self.queued, piece.len());
                take(&self.link_queued, piece.len());
                self.room.notify_one();
                Poll::Ready(Some(Ok(piece)))
            }
            Poll::Ready(Some(Err(error))) => {
                self.cancel.armed = false;
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                // The sender is gone: whole only when the helper marked the end (a failure sends its error first).
                self.cancel.armed = false;
                if let Some(session) = self.cancel.session.upgrade() {
                    locked(&session.streams).remove(&self.cancel.id);
                }
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// A socket's way out, on either side of a link: its writer's bounded queue (`WRITER_QUEUE` messages) for the frames
/// that may wait, and for the frames that must go without waiting — a refused request's answer, `cancel`, a ping, the
/// status — a small overflow that one task drains in order while the queue is full. A peer that leaves the queue full
/// for `STALL`, or lets the overflow fill as well, ends the link (`gone`): no task is ever made per frame.
#[derive(Clone)]
pub(crate) struct Out {
    queue: mpsc::Sender<Wire>,
    pub(crate) gone: CancellationToken,
    overflow: Arc<Mutex<Overflow>>,
}

/// Control frames waiting for room in the writer's queue, and whether a task drains them now.
#[derive(Default)]
struct Overflow {
    frames: std::collections::VecDeque<Wire>,
    draining: bool,
}

impl Out {
    /// The writer started over the socket's sink.
    pub(crate) fn start(sink: WireSink) -> Out {
        let (queue, frames) = mpsc::channel(WRITER_QUEUE);
        let gone = CancellationToken::new();
        tokio::spawn(write(sink, frames, gone.clone()));
        Out { queue, gone, overflow: Arc::default() }
    }

    /// A frame that may wait for room, at most `STALL`; past it the link ends.
    pub(crate) async fn send(&self, wire: Wire) -> bool {
        if self.queue.send_timeout(wire, STALL).await.is_ok() {
            return true;
        }
        self.gone.cancel();
        false
    }

    /// A frame that must go and never waits: into the writer's queue, or — while it is full, or frames wait already —
    /// after them into the overflow.
    pub(crate) fn control(&self, wire: Wire) {
        let mut waiting = locked(&self.overflow);
        let wire = if waiting.frames.is_empty() && !waiting.draining {
            match self.queue.try_send(wire) {
                Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => return,
                Err(mpsc::error::TrySendError::Full(wire)) => wire,
            }
        } else {
            wire
        };
        if waiting.frames.len() >= WRITER_QUEUE {
            // A peer that reads nothing while it sends: the link ends.
            self.gone.cancel();
            return;
        }
        waiting.frames.push_back(wire);
        if waiting.draining {
            return;
        }
        waiting.draining = true;
        let (queue, overflow, gone) = (self.queue.clone(), self.overflow.clone(), self.gone.clone());
        tokio::spawn(async move {
            loop {
                let wire = {
                    let mut waiting = locked(&overflow);
                    match waiting.frames.pop_front() {
                        Some(wire) => wire,
                        None => {
                            waiting.draining = false;
                            return;
                        }
                    }
                };
                if queue.send_timeout(wire, STALL).await.is_err() {
                    gone.cancel();
                    return;
                }
            }
        });
    }

    /// One of the last frames before a close, if there is room for it.
    pub(crate) fn last(&self, wire: Wire) {
        let _ = self.queue.try_send(wire);
    }

    /// The close, if there is room for it: the writer stops after it.
    pub(crate) fn close(&self, code: u16) {
        self.last(Wire::Close(code, String::new()));
    }

    /// Waits until the writer stopped, at most `wait`.
    pub(crate) async fn closed(&self, wait: Duration) {
        let _ = tokio::time::timeout(wait, self.queue.closed()).await;
    }

    /// Control frames waiting in the overflow (tests).
    #[cfg(test)]
    pub(crate) fn waiting(&self) -> usize {
        locked(&self.overflow).frames.len()
    }
}

/// The socket's one writer: frames in order, the close last.
async fn write(mut sink: WireSink, mut queue: mpsc::Receiver<Wire>, gone: CancellationToken) {
    loop {
        let wire = tokio::select! {
            wire = queue.recv() => wire,
            _ = gone.cancelled() => None,
        };
        let Some(wire) = wire else { break };
        let close = matches!(wire, Wire::Close(..));
        let written = tokio::select! {
            written = sink.send(wire) => written.is_ok(),
            _ = gone.cancelled() => false,
        };
        if !written || close {
            break;
        }
    }
    gone.cancel();
    queue.close();
    let _ = tokio::time::timeout(Duration::from_secs(1), sink.close()).await;
}

/// One request's answer over a link, on either side: its head (`res`; `up-res` for the helper's own request), then its
/// body in pieces of at most `CHUNK`, the end marked; a body that breaks midway ends in an `error` (`up-error`), never
/// as a short whole one. `done` hears its id at the end.
pub(crate) async fn answer(
    router: Router,
    request: Request,
    id: u32,
    out: Out,
    answered: Arc<AtomicBool>,
    done: mpsc::UnboundedSender<u32>,
    way: Way,
) {
    let send = |wire: Wire| {
        let out = out.clone();
        async move { out.send(wire).await }
    };
    let response = router.oneshot(request).await.unwrap_or_else(|never| match never {});
    let (parts, body) = response.into_parts();
    let has_body = !axum::body::HttpBody::is_end_stream(&body);
    let head = json!({
        "t": way.answer(), "id": id, "status": parts.status.as_u16(),
        "headers": headers_json(&parts.headers, carried_back), "body": has_body,
    });
    if !send(Wire::json(&head)).await {
        return;
    }
    answered.store(true, Ordering::SeqCst);
    if has_body {
        let mut pieces = body.into_data_stream();
        while let Some(piece) = pieces.next().await {
            match piece {
                Ok(piece) => {
                    for part in piece.chunks(CHUNK) {
                        if !send(Wire::Binary(encode_chunk(id, false, part))).await {
                            return;
                        }
                    }
                }
                Err(error) => {
                    let _ =
                        send(Wire::json(&json!({ "t": way.failure(), "id": id, "message": error.to_string() }))).await;
                    let _ = done.send(id);
                    return;
                }
            }
        }
        if !send(Wire::Binary(encode_chunk(id, true, &[]))).await {
            return;
        }
    }
    let _ = done.send(id);
}

/// One of the helper's own requests running at this gateway: its body's queue while it comes, the bytes of it held
/// (until the request ends: its handler may hold them whole), when it began and last brought something, whether its
/// answer has begun, its task (none: a refused body's place, kept until that body's end).
struct UpRunning {
    body: Option<mpsc::UnboundedSender<Result<Bytes, io::Error>>>,
    held: usize,
    started: Instant,
    last: Instant,
    answered: Arc<AtomicBool>,
    task: Option<tokio::task::AbortHandle>,
}

/// The helper's own requests at this gateway on one socket (`up`, stage 7 of docs/plans/home-setup-flow.md): the
/// panel's requests of the person signed in at a Turbo of this home. Each one runs through this gateway's routes as
/// that person — the door (`UpDoor`) decides at each request whether this helper may ask at all —, its answer streamed
/// back. The routes are made at the socket's first such request and go with it. The limits are the helper's own for
/// the gateway's requests (link_serve.rs): so many running and open, `BODY_MAX` a body, `UP_BODIES` of bodies, a body
/// that falls silent or keeps trickling given up.
struct Up {
    out: Out,
    door: Option<Arc<dyn UpDoor>>,
    router: Option<Router>,
    running: HashMap<u32, UpRunning>,
    held: usize,
    done: mpsc::UnboundedSender<u32>,
}

impl Drop for Up {
    /// The socket is gone: what still runs for it stops.
    fn drop(&mut self) {
        for running in self.running.values() {
            if let Some(task) = &running.task {
                task.abort();
            }
        }
    }
}

impl Up {
    fn new(out: Out, door: Option<Arc<dyn UpDoor>>, done: mpsc::UnboundedSender<u32>) -> Up {
        Up { out, door, router: None, running: HashMap::new(), held: 0, done }
    }

    /// An `up` frame: run through this gateway's routes as the person it names — unless it is a flood or not of the
    /// helper's ids (a protocol error, the link closed), more than may run (503), a gateway that takes none (404), a
    /// helper this home does not let in now (403), a path it may not ask (404), or it names nobody (403).
    fn request(&mut self, value: &Value) -> Result<(), u16> {
        let id = frame_id(value).filter(|id| id & UP_BIT != 0).ok_or(PROTOCOL)?;
        if self.running.contains_key(&id) || self.running.len() >= UP_OPEN {
            return Err(PROTOCOL);
        }
        let has_body = value.get("body").and_then(Value::as_bool).unwrap_or(false);
        let path = value.get("path").and_then(Value::as_str).unwrap_or("");
        let method = value.get("method").and_then(Value::as_str).and_then(|method| method.parse::<Method>().ok());
        let running = self.running.values().filter(|running| running.task.is_some()).count();
        // Who may ask is the door's to say, at each request: the entry is a Turbo of this home at home now
        // (`Helpers::takes_up`). What it may ask is the panel's API only; as whom — the person its pass names.
        let door = self.door.clone();
        let open = door.as_ref().filter(|door| door.open());
        let uri = path.parse::<Uri>().ok();
        let refused = if running >= UP_RUNNING {
            Some(StatusCode::SERVICE_UNAVAILABLE)
        } else if door.is_none() {
            Some(StatusCode::NOT_FOUND)
        } else if open.is_none() {
            Some(StatusCode::FORBIDDEN)
        } else if uri.is_none() || !method.as_ref().is_some_and(|method| up_path(path, method)) {
            Some(StatusCode::NOT_FOUND)
        } else {
            None
        };
        // Nobody named: refused, never let in as the helper itself or a key.
        let origin = open.filter(|_| refused.is_none()).and_then(|door| door.origin(&value["person"]));
        let refused = refused.or_else(|| origin.is_none().then_some(StatusCode::FORBIDDEN));
        let (None, Some(door), Some(method), Some(origin), Some(uri)) = (refused, open, method, origin, uri) else {
            self.refuse(id, refused.unwrap_or(StatusCode::FORBIDDEN), has_body);
            return Ok(());
        };
        if self.router.is_none() {
            self.router = door.router();
        }
        let Some(router) = self.router.clone() else {
            self.refuse(id, StatusCode::SERVICE_UNAVAILABLE, has_body);
            return Ok(());
        };
        let (body, sender) = if has_body {
            let (sender, pieces) = mpsc::unbounded_channel();
            (Body::from_stream(tokio_stream::wrappers::UnboundedReceiverStream::new(pieces)), Some(sender))
        } else {
            (Body::empty(), None)
        };
        let mut request = Request::new(body);
        *request.method_mut() = method;
        *request.uri_mut() = uri;
        *request.headers_mut() = headers_from(&value["headers"], |name| Way::Up.carried(name));
        // No socket of its own here: an address that is neither this computer nor the home, for the routes that read
        // the socket's; the caller is the origin's (`Caller`: at home, the person, whatever the request carries).
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([0, 0, 0, 0], 0))));
        request.extensions_mut().insert(origin);
        let answered = Arc::new(AtomicBool::new(false));
        let task =
            tokio::spawn(answer(router, request, id, self.out.clone(), answered.clone(), self.done.clone(), Way::Up));
        let now = Instant::now();
        self.running.insert(
            id,
            UpRunning { body: sender, held: 0, started: now, last: now, answered, task: Some(task.abort_handle()) },
        );
        Ok(())
    }

    /// A body piece to its request: at most `BODY_MAX` for one (413); the bodies of the running requests at most
    /// `UP_BODIES` (503), counted until each request ends. False on a malformed piece.
    fn feed(&mut self, frame: &Bytes) -> bool {
        let Some((id, end, data)) = decode_chunk(frame) else { return false };
        let Some(running) = self.running.get_mut(&id) else { return true };
        if !data.is_empty() || end {
            running.last = Instant::now();
        }
        // A refused body's place: passed by, let go at its end.
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
            if self.held + data.len() > UP_BODIES {
                self.refuse(id, StatusCode::SERVICE_UNAVAILABLE, !end);
                return true;
            }
            running.held += data.len();
            self.held += data.len();
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

    /// A stream given up (`up-cancel`, a refusal): its task stopped, its body's bytes given back.
    fn drop_stream(&mut self, id: u32) {
        if let Some(running) = self.running.remove(&id) {
            if let Some(task) = &running.task {
                task.abort();
            }
            self.held = self.held.saturating_sub(running.held);
        }
    }

    /// A request's answer is out: its place and its body's bytes given back.
    fn finished(&mut self, id: u32) {
        if self.running.get(&id).is_some_and(|running| running.task.is_some()) {
            self.drop_stream(id);
        }
    }

    /// A request refused or given up while it runs: before its answer an `up-res` with the status, after it an
    /// `up-error`. While its body is still coming, a place with no task is kept for it until that body's end.
    fn refuse(&mut self, id: u32, status: StatusCode, body_coming: bool) {
        let answered = self.running.get(&id).is_some_and(|running| running.answered.load(Ordering::SeqCst));
        let started = self.running.get(&id).map_or_else(Instant::now, |running| running.started);
        self.drop_stream(id);
        if body_coming {
            let place = UpRunning {
                body: None,
                held: 0,
                started,
                last: Instant::now(),
                answered: Arc::new(AtomicBool::new(true)),
                task: None,
            };
            self.running.insert(id, place);
        }
        let frame = if answered {
            json!({ "t": Way::Up.failure(), "id": id, "message": status.canonical_reason().unwrap_or("") })
        } else {
            json!({ "t": Way::Up.answer(), "id": id, "status": status.as_u16(), "headers": {}, "body": false })
        };
        self.out.control(Wire::json(&frame));
    }

    /// Bodies that stopped coming for `silence`, or still come `whole` after their request began, are given up (408); a
    /// refused body's place, already answered, is let go as quietly.
    fn give_up_late_bodies(&mut self, silence: Duration, whole: Duration) {
        let late = |running: &UpRunning| running.last.elapsed() >= silence || running.started.elapsed() >= whole;
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

// ── The bridge ──

/// Where this gateway's own clients reach a linked helper: its port on loopback and the entry.
pub fn via_base(port: u16, entry: &str) -> String {
    format!("http://127.0.0.1:{port}/via/{entry}")
}

/// A bridge request's entry and inner path, taken from the raw URI as it came — `/via/<entry>` cut, the query and any
/// escapes kept —; none unless it is under `/home/v1/` and not the link itself.
pub fn inner_of(raw: &str) -> Option<(String, String)> {
    let (entry, inner) = raw.strip_prefix("/via/")?.split_once('/')?;
    let inner = format!("/{inner}");
    let path = inner.split('?').next().unwrap_or("");
    let link = path == PATH || path.starts_with(&format!("{PATH}/"));
    (path.starts_with("/home/v1/") && !link && !entry.is_empty()).then(|| (entry.to_string(), inner))
}

/// `/via/{entry}/home/v1/*` on the gateway's own router.
pub fn via_routes() -> Router<Arc<App>> {
    Router::new().route("/via/{entry}/{*rest}", axum::routing::any(via))
}

/// The bridge: only from this computer, never from a page or through a proxy, only with this start's token, only to
/// `/home/v1/` (the link itself aside); then the request goes over the entry's link as it came.
async fn via(State(app): State<Arc<App>>, caller: Caller, request: Request) -> Response {
    if caller.zone != Zone::Local || crate::platform::http::zone::forwarded(&caller.headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if caller.headers.contains_key(header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let token = caller
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    let local = app.helpers.local();
    if !same(token.as_bytes(), local.token().as_bytes()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let raw = request.uri().path_and_query().map(|path| path.as_str().to_string()).unwrap_or_default();
    let Some((entry, inner)) = inner_of(&raw) else { return StatusCode::NOT_FOUND.into_response() };
    let Some(link) = app.helpers.link_of(&entry) else { return StatusCode::NOT_FOUND.into_response() };
    let (mut parts, body) = request.into_parts();
    let Ok(uri) = inner.parse() else { return StatusCode::NOT_FOUND.into_response() };
    parts.uri = uri;
    match link.call(Request::from_parts(parts, body)).await {
        Ok(response) => response,
        Err(LinkError::Away) => (StatusCode::SERVICE_UNAVAILABLE, crate::core::tx(super::AWAY)).into_response(),
        Err(LinkError::Busy) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        Err(LinkError::Broken(why)) => (StatusCode::BAD_GATEWAY, why).into_response(),
    }
}

#[cfg(test)]
#[path = "link_tests.rs"]
mod tests;
