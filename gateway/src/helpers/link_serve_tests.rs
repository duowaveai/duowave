//! The link's helper side alone: the certificate, the link port, the pairing and its key, the throttle, a home's link
//! served over an in-memory socket (its clock paused where time counts), the limits.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request};
use axum::http::StatusCode;
use futures::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use super::*;
use crate::config::{Config, Env};
use crate::helpers::contract::Ability;
use crate::helpers::link::{
    client_config, encode_chunk, link_key_hash, link_key_id, pair_over, tungstenite_wire, FirstMeeting, LinkTiming,
    PairError, Paired, Pinned, UpPerson, CHUNK,
};
use crate::helpers::remote::Me;
use crate::settings::definitions::{HelperOutside, HELPER_CLIENTS};

/// A computer that helps, with these more variables.
fn computer(vars: &[(&str, &str)]) -> Arc<App> {
    let env = Env::from_pairs([("STATE_DIR", "")].into_iter().chain(vars.iter().copied()));
    App::build(Config::for_tests(&env)).unwrap()
}

fn fingerprint(app: &App) -> [u8; 32] {
    app.link.fingerprint.expect("a certificate")
}

/// A pairing over an in-memory socket: the gateway's side here, the helper's served by the app.
async fn pairing(app: &Arc<App>, key: &ConnectionKey, probe: bool, accept: bool) -> Result<Paired, PairError> {
    let (gateway, helper) = tokio::io::duplex(1 << 16);
    let mut gateway = WebSocketStream::from_raw_socket(gateway, Role::Client, None).await;
    let helper = WebSocketStream::from_raw_socket(helper, Role::Server, None).await;
    let (sink, stream) = tungstenite_wire(helper);
    let binding = Binding::Tls(fingerprint(app));
    let served = {
        let (app, binding) = (app.clone(), binding.clone());
        tokio::spawn(async move { pair_session(app, sink, stream, binding, "127.0.0.1").await })
    };
    let check = move |_: &HelperIdentity, _: &crate::settings::definitions::HelperPin| {
        if accept {
            Ok(())
        } else {
            Err(crate::platform::http::ApiError::conflict("уже в списке"))
        }
    };
    let me = Me::default_for_tests();
    let home_key = app.secrets.get(HOME_KEY).unwrap_or_default();
    let paired = pair_over(&mut gateway, key, &me, &home_key, &binding, &LinkTiming::default(), probe, &check).await;
    let _ = served.await;
    paired
}

/// The gateway's next JSON frame, or the close that came instead: its code and its reason.
async fn next_frame(socket: &mut WebSocketStream<tokio::io::DuplexStream>) -> Result<Value, Option<(u16, String)>> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => return Ok(serde_json::from_str(&text).unwrap()),
            Some(Ok(Message::Close(frame))) => {
                return Err(frame.map(|frame| (u16::from(frame.code), frame.reason.to_string())))
            }
            Some(Ok(_)) => continue,
            _ => return Err(None),
        }
    }
}

/// A refusal as the helper closes with it: 4401, and not a word of why.
fn refused() -> Result<Value, Option<(u16, String)>> {
    Err(Some((REFUSED, String::new())))
}

async fn say(socket: &mut WebSocketStream<tokio::io::DuplexStream>, value: Value) {
    socket.send(Message::Text(value.to_string().into())).await.unwrap();
}

/// A pairing driven frame by frame from `source`: the gateway's side here, the helper's served by the app.
async fn by_hand(app: &Arc<App>, source: &'static str) -> (WebSocketStream<tokio::io::DuplexStream>, Binding) {
    let (gateway, helper) = tokio::io::duplex(1 << 16);
    let gateway = WebSocketStream::from_raw_socket(gateway, Role::Client, None).await;
    let helper = WebSocketStream::from_raw_socket(helper, Role::Server, None).await;
    let (sink, stream) = tungstenite_wire(helper);
    let binding = Binding::Tls(fingerprint(app));
    let (app, served) = (app.clone(), binding.clone());
    tokio::spawn(async move { pair_session(app, sink, stream, served, source).await });
    (gateway, binding)
}

/// A pairing by hand with this key, as far as the helper lets it go: the helper's answer to the proof — `paired`, or
/// the close that came instead (before the challenge when it was not armed).
async fn proven_with(
    app: &Arc<App>,
    key: &ConnectionKey,
    source: &'static str,
) -> Result<Value, Option<(u16, String)>> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let (mut gateway, binding) = by_hand(app, source).await;
    let nonce_g = [5u8; 32];
    say(
        &mut gateway,
        json!({ "t": "pair", "v": VERSION, "nonce": b64.encode(nonce_g), "gateway": "gw", "home": "home" }),
    )
    .await;
    let challenge = next_frame(&mut gateway).await?;
    let nonce_h: [u8; 32] = b64.decode(challenge["nonce"].as_str().unwrap()).unwrap().try_into().unwrap();
    let transcript =
        Transcript { nonce_g: &nonce_g, nonce_h: &nonce_h, binding: &binding, gateway: "gw", home: "home" };
    let mac = b64.encode(transcript.proof(key.bytes(), "gateway"));
    say(&mut gateway, json!({ "t": "proof", "mac": mac, "homeMac": "" })).await;
    next_frame(&mut gateway).await
}

#[tokio::test]
async fn the_certificate_is_made_once_and_kept() {
    let app = computer(&[]);
    assert!(app.secrets.has(TLS_SECRET), "made at the first start of a computer that helps");
    let first = TlsIdentity::load_or_make(&app.secrets).unwrap();
    let again = TlsIdentity::load_or_make(&app.secrets).unwrap();
    assert_eq!((first.cert.clone(), first.fingerprint), (again.cert.clone(), again.fingerprint));
    assert_eq!(app.link.fingerprint(), Some(hex::encode(first.fingerprint)));
    // rustls serves it, and a pinned gateway's handshake — the signature checked with the certificate's own key,
    // which webpki reads — finishes over it.
    let server = first.server_config().unwrap();
    let pinned = Pinned::new(first.fingerprint);
    let (near, far) = tokio::io::duplex(64 * 1024);
    let accepted = tokio::spawn(async move { tokio_rustls::TlsAcceptor::from(server).accept(far).await.is_ok() });
    let name = rustls::pki_types::ServerName::try_from("helper.example.com").unwrap();
    let connected =
        tokio_rustls::TlsConnector::from(client_config(Some(pinned), false).unwrap()).connect(name, near).await;
    assert!(connected.is_ok() && accepted.await.unwrap());
    // A computer in the station role helps nobody: no certificate is made there.
    let station = computer(&[("DUOWAVE_ROLE", "station")]);
    assert!(!station.link.helps() && station.link.tls().is_none() && !station.secrets.has(TLS_SECRET));
}

/// The app's link port on 127.0.0.1, served until the token is cancelled; with these times to the handshake and the
/// upgrade.
async fn link_port(
    app: &Arc<App>,
    to_upgrade: Duration,
) -> (u16, CancellationToken, tokio::task::JoinHandle<io::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let listener =
        LinkListener::new(listener, app.link.tls().unwrap(), Limits::new(CONNECTIONS, CONNECTIONS_PER_SOURCE))
            .with_times(Duration::from_secs(2), to_upgrade);
    let stop = CancellationToken::new();
    let served = tokio::spawn(serve(app.clone(), listener, stop.clone().cancelled_owned()));
    (port, stop, served)
}

/// TLS to the link port, any certificate taken (as at a first meeting).
async fn tls(port: u16) -> tokio_rustls::client::TlsStream<TcpStream> {
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let config = client_config(Some(FirstMeeting::new()), true).unwrap();
    let name = rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap().to_owned();
    tokio_rustls::TlsConnector::from(config).connect(name, tcp).await.unwrap()
}

/// An upgrade to the link port with these headers and this link version: 101, or the status it got; the socket when it
/// is up.
async fn upgrade(
    port: u16,
    headers: &[(&str, &str)],
) -> (u16, Option<WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>>) {
    let mut request = format!("wss://127.0.0.1:{port}/home/v1/link").into_client_request().unwrap();
    let version = VERSION.to_string();
    for (name, value) in [(VERSION_HEADER, version.as_str())].iter().chain(headers) {
        request.headers_mut().insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            axum::http::HeaderValue::from_str(value).unwrap(),
        );
    }
    let config = WebSocketConfig::default().max_frame_size(Some(1 << 20)).max_message_size(Some(1 << 20));
    match tokio_tungstenite::client_async_with_config(request, tls(port).await, Some(config)).await {
        Ok((socket, _)) => (101, Some(socket)),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => (response.status().as_u16(), None),
        Err(error) => panic!("{error}"),
    }
}

/// The close a socket ended with, its frames before it passed by.
async fn closed_with<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
) -> Option<u16> {
    let closed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(frame))) => return frame.map(|frame| u16::from(frame.code)),
                Some(Ok(_)) => continue,
                _ => return None,
            }
        }
    });
    closed.await.expect("closed in time")
}

/// A later link over the link port as a gateway opens it: the key's id in the upgrade, then the proof of the key over
/// the helper's challenge and this certificate. The socket once its welcome came; else the status at the door, or the
/// close code that came after the proof.
async fn open_link(
    app: &App,
    port: u16,
    key: &str,
) -> Result<WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>, u16> {
    let id = link_key_id(key).unwrap_or(key);
    let headers = [(LINK_ID_HEADER, id), (GATEWAY_HEADER, "gw-1"), (HOME_HEADER, "home-1")];
    let (status, socket) = upgrade(port, &headers).await;
    let mut socket = socket.ok_or(status)?;
    let challenge = match socket.next().await {
        Some(Ok(Message::Text(text))) => serde_json::from_str::<Value>(&text).unwrap(),
        other => panic!("no challenge: {other:?}"),
    };
    socket.send(Message::Text(proof_of(app, key, &challenge).to_string().into())).await.unwrap();
    match socket.next().await {
        Some(Ok(Message::Text(text))) if text.contains("\"welcome\"") => Ok(socket),
        Some(Ok(Message::Close(frame))) => Err(frame.map_or(1005, |frame| u16::from(frame.code))),
        other => panic!("{other:?}"),
    }
}

/// The proof a gateway gives of `key` over this challenge and this computer's certificate, as `gw-1` of `home-1`.
fn proof_of(app: &App, key: &str, challenge: &Value) -> Value {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let nonce: [u8; 32] = b64.decode(challenge["nonce"].as_str().unwrap()).unwrap().try_into().unwrap();
    let id = link_key_id(key).unwrap_or("");
    let mac = link_proof(&link_key_hash(key), &nonce, &Binding::Tls(fingerprint(app)), id, "gw-1", "home-1");
    json!({ "t": "proof", "mac": b64.encode(mac) })
}

#[tokio::test]
async fn the_link_port_serves_the_link_only_and_frees_the_port_when_dropped() {
    let app = computer(&[]);
    let (port, stop, served) = link_port(&app, Duration::from_millis(300)).await;
    // The link: an upgrade over TLS. A pairing while no key is taken is upgraded all the same, and refused with 4401
    // once it spoke — as a wrong key is, after its proof —, holding no pairing's place.
    let (status, socket) = upgrade(port, &[(PAIR_HEADER, "pair")]).await;
    assert_eq!(status, 101);
    let mut socket = socket.unwrap();
    socket.send(Message::Text(json!({ "t": "pair", "v": VERSION }).to_string().into())).await.unwrap();
    assert_eq!(closed_with(&mut socket).await, Some(REFUSED));
    assert!(locked(&app.link.shared.pairing.open).is_empty(), "no pairing's place taken");
    assert!(!app.link.shared.pairing.failures.blocked("127.0.0.1"));
    app.link.connection_key(true);
    assert_eq!(upgrade(port, &[(PAIR_HEADER, "pair")]).await.0, 101);
    // Anything else over TLS: an empty 404.
    let mut stream = tls(port).await;
    stream.write_all(b"GET /health HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\n\r\n").await.unwrap();
    let mut answer = Vec::new();
    let _ = stream.read_to_end(&mut answer).await;
    let answer = String::from_utf8_lossy(&answer);
    assert!(answer.starts_with("HTTP/1.1 404"), "{answer}");
    assert!(answer.ends_with("\r\n\r\n"), "no body: {answer}");
    // Plain bytes: no TLS, nothing served.
    let mut plain = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    plain.write_all(b"GET /health HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n").await.unwrap();
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), plain.read_to_end(&mut answer)).await.expect("closed");
    assert!(!String::from_utf8_lossy(&answer).contains("HTTP/1.1"));
    // A connection that asks for nothing is closed after its time (here 300 ms).
    let mut idle = tls(port).await;
    let mut byte = [0u8; 1];
    let read = tokio::time::timeout(Duration::from_secs(5), idle.read(&mut byte)).await.expect("closed in time");
    assert!(!matches!(read, Ok(n) if n > 0));
    // Stopped: the port is free at once.
    stop.cancel();
    served.await.unwrap().unwrap();
    assert!(TcpListener::bind(("127.0.0.1", port)).await.is_ok(), "the port is free");
}

#[tokio::test]
async fn pairing_over_plain_http_is_taken_only_from_this_computer() {
    let app = computer(&[("LINK_PROXY_FROM", "192.168.1.30")]);
    let server = &app.link;
    let peer = |ip: &str| SocketAddr::new(ip.parse().unwrap(), 5000);
    let named = |host: &str| Some(Binding::Name(host.to_string()));
    // Without «Адрес снаружи» no plain pairing at all: a proxy that hands on whatever name a client asked for would let
    // a relay with a certificate of its own name choose what is bound.
    assert_eq!(server.binding(Transport::Plain, peer("127.0.0.1")), None);
    assert_eq!(server.binding(Transport::Plain, peer("192.168.1.30")), None);
    // «Адрес снаружи» says the name, from this computer or a proxy the owner named; on the link port the certificate
    // is bound, from wherever.
    let outside = HelperOutside { url: "https://Outside.Example.com.:8443".into() };
    app.settings.set(&HELPER_OUTSIDE, &outside, None).unwrap();
    assert_eq!(server.binding(Transport::Plain, peer("127.0.0.1")), named("outside.example.com"));
    assert_eq!(server.binding(Transport::Plain, peer("192.168.1.30")), named("outside.example.com"));
    assert_eq!(server.binding(Transport::Plain, peer("192.168.1.20")), None, "not a proxy the owner named");
    assert_eq!(server.binding(Transport::Plain, peer("198.51.100.7")), None);
    assert_eq!(server.binding(Transport::Tls, peer("198.51.100.7")), Some(Binding::Tls(fingerprint(&app))));
    // Through the main port: a pairing from outside is one empty 401.
    let upgrade = |from: &str| {
        let mut request = Request::builder()
            .uri("/home/v1/link")
            .header("host", "helper.example.com")
            .header("upgrade", "websocket")
            .header("connection", "upgrade")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header(PAIR_HEADER, "pair")
            .header(VERSION_HEADER, VERSION.to_string())
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(peer(from)));
        crate::app::router(app.clone()).oneshot(request)
    };
    let response = upgrade("198.51.100.7").await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.into_body().collect().await.unwrap().to_bytes().is_empty());
    // From this computer with «Адрес снаружи» gone: the same empty 401, whatever `Host` the proxy passes.
    app.settings.forget(&HELPER_OUTSIDE).unwrap();
    let response = upgrade("127.0.0.1").await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.into_body().collect().await.unwrap().to_bytes().is_empty());
}

#[tokio::test]
async fn the_connection_key_is_spent_only_by_a_confirmed_pairing() {
    let app = computer(&[]);
    let key = app.link.connection_key(true);
    let clients = || app.settings.get(&HELPER_CLIENTS, None).unwrap_or_default();
    // A probe: the helper's own proof and what it would give, no key, no client, nothing spent.
    let probe = pairing(&app, &key, true, true).await.unwrap();
    assert_eq!((probe.key.as_deref(), probe.helper.id.as_str()), (None, app.identity.gateway().unwrap().id.as_str()));
    assert!(probe.status["contract"]["abilities"].is_array(), "{}", probe.status);
    assert!(clients().is_empty());
    assert_eq!(app.link.connection_key(false), key);
    // The gateway turns the helper down before `ok`: its pending client is let go, the key stays.
    assert!(matches!(pairing(&app, &key, false, false).await, Err(PairError::Checked(_))));
    assert!(clients().is_empty(), "{:?}", clients());
    assert_eq!(app.link.connection_key(false), key);
    // A wrong key: refused after the proof, with 4401 and not a word of why.
    assert_eq!(proven_with(&app, &ConnectionKey::new(), "192.0.2.4").await, refused());
    // `ok`: the client confirmed, the key spent — a new one shown —, the old one refused from then on, as a wrong one.
    let paired = pairing(&app, &key, false, true).await.unwrap();
    let link_key = paired.key.expect("a link key");
    assert!(app.clients.check(&link_key).unwrap().is_some());
    assert_ne!(app.link.connection_key(false), key);
    assert!(matches!(pairing(&app, &key, true, true).await, Err(PairError::Refused)));
    assert_eq!(proven_with(&app, &key, "192.0.2.4").await, refused(), "a spent key: the same refusal");

    // Two pairings with one key: the one that says `ok` second meets 4401.
    let key = app.link.connection_key(true);
    let (gateway, helper) = tokio::io::duplex(1 << 16);
    let mut gateway = WebSocketStream::from_raw_socket(gateway, Role::Client, None).await;
    let helper = WebSocketStream::from_raw_socket(helper, Role::Server, None).await;
    let (sink, stream) = tungstenite_wire(helper);
    let binding = Binding::Tls(fingerprint(&app));
    let served = {
        let (app, binding) = (app.clone(), binding.clone());
        tokio::spawn(async move { pair_session(app, sink, stream, binding, "192.0.2.5").await })
    };
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let nonce_g = [5u8; 32];
    say(
        &mut gateway,
        json!({ "t": "pair", "v": VERSION, "nonce": b64.encode(nonce_g), "gateway": "gw-2", "home": "home-2" }),
    )
    .await;
    let challenge = next_frame(&mut gateway).await.unwrap();
    let nonce_h: [u8; 32] = b64.decode(challenge["nonce"].as_str().unwrap()).unwrap().try_into().unwrap();
    let transcript =
        Transcript { nonce_g: &nonce_g, nonce_h: &nonce_h, binding: &binding, gateway: "gw-2", home: "home-2" };
    say(
        &mut gateway,
        json!({ "t": "proof", "mac": b64.encode(transcript.proof(key.bytes(), "gateway")), "homeMac": "" }),
    )
    .await;
    let paired = next_frame(&mut gateway).await.unwrap();
    assert_eq!(paired["t"], "paired");
    // Meanwhile another pairing with the same key goes through to the end — the same gateway of this computer's own
    // home again, so it replaces that gateway's first pairing and its key.
    let again = pairing(&app, &key, false, true).await.unwrap().key.unwrap();
    say(&mut gateway, ok_for(&transcript, &key, &paired, false)).await;
    assert_eq!(next_frame(&mut gateway).await, refused());
    served.await.unwrap();
    let confirmed = app.clients.view(&[]);
    assert_eq!(confirmed.len(), 1, "{confirmed:?}");
    assert!(confirmed[0].own_home);
    assert!(
        app.clients.check(&again).unwrap().is_some() && app.clients.check(&link_key).unwrap().is_none(),
        "the old key forgotten"
    );

    // A pending client's key opens nothing.
    let (_, pending) = app.clients.pair("home-3", "gw-3", false, None).unwrap();
    assert!(app.clients.check(&pending).unwrap().is_none());
}

#[tokio::test(start_paused = true)]
async fn the_key_is_taken_only_within_a_quarter_hour_of_being_shown() {
    let app = computer(&[]);
    let unshown = app.link.connection_key(false);
    // Never shown: refused before any challenge, and not a word of why. It proves nothing about the key, so however
    // many come, the place they came from still pairs.
    for _ in 0..PAIR_FAILURES + 2 {
        assert_eq!(proven_with(&app, &unshown, "192.0.2.5").await, refused(), "no challenge");
    }
    assert!(!app.link.shared.pairing.failures.blocked("192.0.2.5"));
    // Shown for the first time since the start: a key of its own — the one kept from before opens nothing —, taken for
    // a quarter of an hour.
    let key = app.link.connection_key(true);
    assert_ne!(key, unshown);
    assert!(matches!(pairing(&app, &unshown, true, true).await, Err(PairError::Refused)));
    assert!(pairing(&app, &key, true, true).await.is_ok());
    // Read again meanwhile — the sheet re-reads it as the panel changes —: the same key with the same end, never moved
    // on, so a sheet left open, or photographed, opens nothing 15 minutes after the key was shown (review of stage 7).
    let end = *locked(&app.link.armed_until);
    tokio::time::advance(ARMED_FOR - Duration::from_secs(1)).await;
    assert_eq!(app.link.connection_key(true), key, "the sheet read again while it is open");
    assert_eq!(*locked(&app.link.armed_until), end, "a read does not move the end on");
    assert!(pairing(&app, &key, true, true).await.is_ok());
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(matches!(pairing(&app, &key, true, true).await, Err(PairError::Refused)));
    // Shown again once its quarter of an hour is over: a new key — the one seen before, or photographed, opens nothing.
    let next = app.link.connection_key(true);
    assert_ne!(next, key);
    assert!(matches!(pairing(&app, &key, true, true).await, Err(PairError::Refused)));
    assert!(pairing(&app, &next, true, true).await.is_ok());
}

#[tokio::test]
async fn only_a_wrong_proof_of_the_key_counts_against_its_place() {
    let app = computer(&[]);
    let key = app.link.connection_key(true);
    for _ in 0..PAIR_FAILURES - 1 {
        assert_eq!(proven_with(&app, &ConnectionKey::new(), "192.0.2.6").await, refused());
    }
    assert!(!app.link.shared.pairing.failures.blocked("192.0.2.6"));
    assert_eq!(proven_with(&app, &ConnectionKey::new(), "192.0.2.6").await, refused());
    assert!(app.link.shared.pairing.failures.blocked("192.0.2.6"), "ten wrong proofs in ten minutes");
    assert!(!app.link.shared.pairing.failures.blocked("192.0.2.7"));
    assert!(pairing(&app, &key, true, true).await.is_ok(), "another place pairs on");
}

/// A pairing socket holds its place only while it pairs (review of stage 7): one that says nothing gives it up after
/// `PAIR_FIRST`, not the exchange's 10 s, refused; it and one that is not the exchange count against their place as a
/// wrong proof does — ten in ten minutes and that place waits —, while another place pairs on.
#[tokio::test(start_paused = true)]
async fn a_silent_or_malformed_pairing_is_refused_soon_and_counted_against_its_place() {
    let app = computer(&[]);
    let key = app.link.connection_key(true);
    for _ in 0..PAIR_FAILURES - 1 {
        let started = Instant::now();
        let (mut gateway, _) = by_hand(&app, "192.0.2.8").await;
        assert_eq!(next_frame(&mut gateway).await, refused(), "silent");
        let held = started.elapsed();
        assert!((PAIR_FIRST..PAIR_EXCHANGE).contains(&held), "{held:?}");
    }
    assert!(!app.link.shared.pairing.failures.blocked("192.0.2.8"));
    let (mut gateway, _) = by_hand(&app, "192.0.2.8").await;
    say(&mut gateway, json!({ "t": "hello", "v": 1 })).await;
    assert_eq!(next_frame(&mut gateway).await, refused(), "not the exchange");
    assert!(app.link.shared.pairing.failures.blocked("192.0.2.8"), "ten in ten minutes");
    assert!(!app.link.shared.pairing.failures.blocked("192.0.2.9"));
    assert!(pairing(&app, &key, true, true).await.is_ok(), "another place pairs on");
}

/// The refused pairings told to the panels — and so logged — since the last call: their places.
fn refusals(heard: &mut tokio::sync::broadcast::Receiver<Arc<GatewayEvent>>) -> Vec<String> {
    let mut places = Vec::new();
    while let Ok(event) = heard.try_recv() {
        if let GatewayEvent::HelperClients { change, client, .. } = &*event {
            if change == "refused" {
                places.push(client.clone());
            }
        }
    }
    places
}

/// Only a wrong proof of the key is logged (review of stage 7): a pairing that proves nothing — the key not taken, a
/// silent or a malformed one — writes no row and tells no panel; wrong proofs one row per place, and from however many
/// places at most `REFUSALS_LOGGED` in ten minutes, so they never push the log's history out.
#[tokio::test(start_paused = true)]
async fn only_a_wrong_proof_is_logged_and_never_more_than_a_few_rows_in_ten_minutes() {
    let app = computer(&[]);
    let mut heard = app.events.subscribe();
    let unshown = app.link.connection_key(false);
    assert_eq!(proven_with(&app, &unshown, "192.0.2.10").await, refused(), "the key not taken");
    app.link.connection_key(true);
    let (mut silent, _) = by_hand(&app, "192.0.2.11").await;
    assert_eq!(next_frame(&mut silent).await, refused(), "silent");
    let (mut malformed, _) = by_hand(&app, "192.0.2.12").await;
    say(&mut malformed, json!({ "t": "pair", "v": 2 })).await;
    assert_eq!(next_frame(&mut malformed).await, refused(), "malformed");
    assert_eq!(refusals(&mut heard), Vec::<String>::new());

    // Wrong proofs: one row per place in ten minutes.
    for _ in 0..3 {
        assert_eq!(proven_with(&app, &ConnectionKey::new(), "192.0.2.13").await, refused());
    }
    assert_eq!(refusals(&mut heard), ["192.0.2.13"]);
    // From many places: a few rows, then none until they age out.
    for n in 0..REFUSALS_LOGGED * 2 {
        app.link.pairing_failed(&format!("198.51.100.{n}"), true);
    }
    assert_eq!(refusals(&mut heard).len(), REFUSALS_LOGGED - 1);
    tokio::time::advance(WINDOW).await;
    app.link.pairing_failed("198.51.100.200", true);
    assert_eq!(refusals(&mut heard), ["198.51.100.200"]);
}

#[tokio::test]
async fn a_flood_of_unknown_keys_makes_its_place_wait() {
    // The throttle: twenty failures make a source wait, another source does not; at most 4096 sources are kept.
    let throttle = LinkThrottle::new(KEY_FAILURES, WINDOW);
    for _ in 0..19 {
        throttle.fail("192.0.2.1");
    }
    assert!(!throttle.blocked("192.0.2.1"));
    throttle.fail("192.0.2.1");
    assert!(throttle.blocked("192.0.2.1") && !throttle.blocked("192.0.2.2"));
    for source in 0..10_000 {
        throttle.fail(&format!("198.51.100.{source}"));
    }
    assert!(throttle.sources() <= SOURCES);

    // Over the link port: a valid key opens the link — never counted —; then 25 unknown ids from one place — the same
    // empty 401, then 429 —, and that place waits now, a valid key too (a gateway tries a 429 again later), while
    // another place does not.
    let app = computer(&[]);
    let (port, stop, _) = link_port(&app, Duration::from_secs(10)).await;
    let (id, key) = app.clients.pair("home-1", "gw-1", false, None).unwrap();
    assert!(app.clients.confirm(&id, Names { gateway: "Колонка".into(), home: "Дача".into() }));
    assert!(open_link(&app, port, &key).await.is_ok());
    assert!(!app.link.throttle.blocked("127.0.0.1"), "a valid key is never counted");
    let mut statuses = Vec::new();
    for _ in 0..25 {
        statuses.push(upgrade(port, &[(LINK_ID_HEADER, "000000000000")]).await.0);
    }
    assert_eq!(&statuses[..20], &[401; 20], "{statuses:?}");
    assert!(statuses[20..].iter().all(|status| *status == 429), "{statuses:?}");
    assert_eq!(open_link(&app, port, &key).await.err(), Some(429));
    assert!(!app.link.throttle.blocked("192.0.2.1"));
    stop.cancel();
}

/// A home's link at the app over an in-memory socket: what serving it ended with, and the gateway's side.
async fn home_link(
    app: &Arc<App>,
    client: &str,
) -> (tokio::task::JoinHandle<()>, WebSocketStream<tokio::io::DuplexStream>) {
    home_link_taking(app, client, false).await
}

/// A home's link as `home_link`, its gateway taking this computer's own requests (`up`) or not.
async fn home_link_taking(
    app: &Arc<App>,
    client: &str,
    up: bool,
) -> (tokio::task::JoinHandle<()>, WebSocketStream<tokio::io::DuplexStream>) {
    let (gateway, helper) = tokio::io::duplex(1 << 22);
    let gateway = WebSocketStream::from_raw_socket(gateway, Role::Client, None).await;
    let config = WebSocketConfig::default().max_frame_size(Some(FRAME_MAX)).max_message_size(Some(FRAME_MAX));
    let helper = WebSocketStream::from_raw_socket(helper, Role::Server, Some(config)).await;
    let (sink, stream) = tungstenite_wire(helper);
    let record = app.clients.record_of(client).expect("a client");
    let client = LinkClient { id: client.to_string(), own_home: record.own_home, home: record.home, binding: None };
    let named = HelperIdentity { id: "gw-1".into(), name: "Колонка".into() };
    let peer = SocketAddr::new("192.0.2.5".parse().unwrap(), 5000);
    let permit = app.link.links.take("192.0.2.5").unwrap();
    let app = app.clone();
    let served = tokio::spawn(async move { serve_home(app, sink, stream, client, named, peer, permit, up).await });
    (served, gateway)
}

fn confirmed(app: &App, gateway: &str) -> String {
    confirmed_key(app, gateway).0
}

/// A confirmed home's client and its link key.
fn confirmed_key(app: &App, gateway: &str) -> (String, String) {
    let (id, key) = app.clients.pair("home-1", gateway, false, None).unwrap();
    assert!(app.clients.confirm(&id, Names { gateway: "Колонка".into(), home: "Дача".into() }));
    (id, key)
}

#[tokio::test(start_paused = true)]
async fn a_removed_home_is_cut_off_at_the_next_look() {
    let app = computer(&[]);
    // Its record gone behind the server's back: the next look (every 15 s) cuts it — 4401.
    let client = confirmed(&app, "gw-1");
    let (served, mut gateway) = home_link(&app, &client).await;
    assert_eq!(next_frame(&mut gateway).await.unwrap()["t"], "welcome");
    assert_eq!(app.link.live_links(), 1);
    app.settings
        .set(&HELPER_CLIENTS, &BTreeMap::<String, crate::settings::definitions::HelperClient>::new(), None)
        .unwrap();
    assert_eq!(next_frame(&mut gateway).await, refused());
    tokio::time::timeout(Duration::from_secs(5), served).await.unwrap().unwrap();
    assert_eq!(app.link.live_links(), 0);
    // Removed on «Этот компьютер»: cut at once.
    let client = confirmed(&app, "gw-2");
    let (_, mut gateway) = home_link(&app, &client).await;
    assert_eq!(next_frame(&mut gateway).await.unwrap()["t"], "welcome");
    app.clients.remove(&client, None).unwrap();
    assert_eq!(next_frame(&mut gateway).await, refused());
    // `bye` from the gateway: the home removed the helper, its client is forgotten.
    let client = confirmed(&app, "gw-3");
    let (served, mut gateway) = home_link(&app, &client).await;
    assert_eq!(next_frame(&mut gateway).await.unwrap()["t"], "welcome");
    say(&mut gateway, json!({ "t": "bye" })).await;
    tokio::time::timeout(Duration::from_secs(5), served).await.unwrap().unwrap();
    assert!(!app.clients.alive(&client));
}

/// «Помогаю» made unreadable (an I/O error of a small disk, say; writing still works).
fn unreadable(db: &Arc<crate::storage::Db>, broken: bool) {
    let flag = if broken { "'unreadable'" } else { "0" };
    let sql = format!("UPDATE settings SET is_default = {flag} WHERE key = 'helper_clients'");
    db.with(|c| c.execute(&sql, [])).unwrap();
}

/// «Помогаю» that does not read cuts no live link (review of stage 7): the next look keeps it — what the home is given
/// is told anew, nothing while the record does not read, as each request finds it —, and once it reads, the home is
/// given its own again.
#[tokio::test(start_paused = true)]
async fn a_record_that_does_not_read_cuts_no_live_link_at_the_next_look() {
    let app = computer(&[]);
    let client = confirmed(&app, "gw-1");
    // Given the apps, which every computer declares: what the home is given shows in its status.
    app.clients.set_gives(&client, vec![Ability::Apps], None).unwrap();
    let (_, mut gateway) = home_link(&app, &client).await;
    let welcome = next_frame(&mut gateway).await.unwrap();
    assert_eq!(welcome["t"], "welcome");
    unreadable(&app.db, true);
    // The next look (15 s): the link stays.
    let told = next_frame(&mut gateway).await.unwrap_or_else(|close| panic!("the link was cut: {close:?}"));
    assert_eq!((told["t"].as_str(), told["name"].as_str()), (Some("event"), Some("status")), "{told}");
    let abilities = told["data"]["contract"]["abilities"].as_array().cloned().unwrap_or_default();
    assert!(abilities.is_empty(), "nothing given meanwhile: {abilities:?}");
    assert_eq!(app.link.live_links(), 1);
    // Read again: what it is given comes back over the same link.
    unreadable(&app.db, false);
    app.link.tell_status();
    let told = next_frame(&mut gateway).await.unwrap();
    assert_eq!((told["name"].as_str(), &told["data"]), (Some("status"), &welcome["status"]), "{told}");
    assert_eq!(app.link.live_links(), 1);
}

/// A known link key's id while «Помогаю» does not read is told to come again — 503, which a gateway tries again —, never
/// refused with the 401 that ends its tries; a wrong one gets the same 401 as ever.
#[tokio::test]
async fn a_right_key_is_told_to_come_again_while_the_record_does_not_read() {
    let app = computer(&[]);
    let (port, stop, _) = link_port(&app, Duration::from_secs(10)).await;
    let (id, key) = app.clients.pair("home-1", "gw-1", false, None).unwrap();
    assert!(app.clients.confirm(&id, Names { gateway: "Колонка".into(), home: "Дача".into() }));
    unreadable(&app.db, true);
    assert_eq!(open_link(&app, port, &key).await.err(), Some(503));
    assert_eq!(upgrade(port, &[(LINK_ID_HEADER, "000000000000")]).await.0, 401);
    unreadable(&app.db, false);
    assert!(open_link(&app, port, &key).await.is_ok());
    stop.cancel();
}

#[tokio::test(start_paused = true)]
async fn the_status_is_told_when_it_changes_at_most_once_per_five_seconds() {
    let app = computer(&[]);
    let client = confirmed(&app, "gw-1");
    let (_, mut gateway) = home_link(&app, &client).await;
    let welcome = next_frame(&mut gateway).await.unwrap();
    assert_eq!(welcome["t"], "welcome");
    let told = tokio::time::Instant::now();
    // Three changes in a row: one event, after five seconds, with the last of them.
    for gives in [vec![Ability::Hearing], vec![Ability::Hearing, Ability::Voice], vec![Ability::Catalog]] {
        app.clients.set_gives(&client, gives, None).unwrap();
        app.link.tell_status();
        tokio::task::yield_now().await;
    }
    let event = next_frame(&mut gateway).await.unwrap();
    assert!(told.elapsed() >= Duration::from_secs(5), "{:?}", told.elapsed());
    assert_eq!((event["t"].as_str(), event["name"].as_str()), (Some("event"), Some("status")));
    let declared: Vec<&str> = event["data"]["contract"]["abilities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|ability| ability["id"].as_str())
        .collect();
    assert!(declared.iter().all(|id| *id == "catalog"), "{declared:?}");
    // A change taken back within the five seconds after it — while it waits its turn —: nothing is told, never the
    // one taken back.
    app.clients.set_gives(&client, vec![Ability::Hearing], None).unwrap();
    app.link.tell_status();
    tokio::task::yield_now().await;
    app.clients.set_gives(&client, vec![Ability::Catalog], None).unwrap();
    app.link.tell_status();
    tokio::task::yield_now().await;
    // Nothing changed since: nothing more.
    let late = tokio::time::timeout(Duration::from_secs(10), next_frame(&mut gateway)).await;
    assert!(late.is_err(), "nothing told: {late:?}");
}

#[tokio::test]
async fn limits_hold_frames_connections_and_bodies() {
    // Connections: 8 from one source, IPv6 by its /56.
    let limits = Limits::new(CONNECTIONS, CONNECTIONS_PER_SOURCE);
    let held: Vec<Permit> = (0..8).map(|_| limits.take("192.0.2.1").unwrap()).collect();
    assert!(limits.take("192.0.2.1").is_none(), "the 9th from one source");
    assert!(limits.take("192.0.2.2").is_some());
    drop(held);
    assert!(limits.take("192.0.2.1").is_some(), "given back with the connections");
    let source = |ip: &str| source_of(ip.parse().unwrap());
    assert_eq!(source("2001:db8::1"), source("2001:db8::2:3"));
    assert_eq!(source("2001:db8::1"), source("2001:db8:0:ff::1"), "one /56: a home's or a server's");
    assert_eq!(source("2001:db8:0:ff::1"), "2001:db8:0:0::/56");
    assert_ne!(source("2001:db8::1"), source("2001:db8:0:100::1"));
    assert_eq!(source("::ffff:192.0.2.1"), "192.0.2.1");
    // Links: one home's at most `LINKS_PER_CLIENT`, whatever its sources; given back with them.
    let links = Limits::with_clients(LINKS, LINKS_PER_SOURCE, LINKS_PER_CLIENT);
    let held: Vec<Permit> =
        (0..LINKS_PER_CLIENT).map(|at| links.take_for(&format!("192.0.2.{at}"), Some("link:a")).unwrap()).collect();
    assert!(links.take_for("192.0.2.200", Some("link:a")).is_none(), "one more of that home");
    assert!(links.take_for("192.0.2.200", Some("link:b")).is_some(), "another home's");
    drop(held);
    assert!(links.take_for("192.0.2.200", Some("link:a")).is_some());

    // A frame over 256 KiB: the link is closed.
    let app = computer(&[]);
    let (port, stop, _) = link_port(&app, Duration::from_secs(10)).await;
    let client = confirmed(&app, "gw-1");
    let (_, key) = {
        // A fresh key for the confirmed client: pair again from the same home and gateway is another client, so take
        // its key from a new pairing of its own.
        let (id, key) = app.clients.pair("home-9", "gw-9", false, None).unwrap();
        assert!(app.clients.confirm(&id, Names::default()));
        (id, key)
    };
    let mut socket = open_link(&app, port, &key).await.expect("the link");
    let _ = socket.send(Message::Binary(vec![0u8; 300 * 1024].into())).await;
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "closed");
    stop.cancel();

    // TLS handshakes going on: two from one source; a third that has said nothing yet is let go at once — a few sources
    // holding silent sockets never take every handshake.
    let (port, stop, _) = link_port(&app, Duration::from_secs(10)).await;
    let mut silent = Vec::new();
    for _ in 0..HANDSHAKES_PER_SOURCE {
        silent.push(TcpStream::connect(("127.0.0.1", port)).await.unwrap());
    }
    let mut third = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut byte = [0u8; 1];
    let read = tokio::time::timeout(Duration::from_secs(2), third.read(&mut byte)).await.expect("let go at once");
    assert!(!matches!(read, Ok(n) if n > 0));
    drop(silent);
    stop.cancel();

    // A body over 16 MiB: 413.
    let (_, mut gateway) = home_link(&app, &client).await;
    assert_eq!(next_frame(&mut gateway).await.unwrap()["t"], "welcome");
    say(&mut gateway, json!({ "t": "req", "id": 1, "method": "POST", "path": "/home/v1/llm/chat/completions", "headers": { "content-type": "application/json" }, "body": true })).await;
    let piece = vec![b' '; CHUNK];
    let mut answer = None;
    for _ in 0..(17 * 16) {
        if gateway.send(Message::Binary(encode_chunk(1, false, &piece))).await.is_err() {
            break;
        }
        if let Ok(Some(Ok(Message::Text(text)))) = tokio::time::timeout(Duration::from_millis(1), gateway.next()).await
        {
            answer = Some(serde_json::from_str::<Value>(&text).unwrap());
            break;
        }
    }
    let answer = match answer {
        Some(answer) => answer,
        None => next_frame(&mut gateway).await.unwrap(),
    };
    assert_eq!((answer["t"].as_str(), answer["status"].as_u64()), (Some("res"), Some(413)), "{answer}");
    // A path outside /home/v1: an empty 404, the handlers never reached.
    say(
        &mut gateway,
        json!({ "t": "req", "id": 2, "method": "GET", "path": "/api/v1/helpers", "headers": {}, "body": false }),
    )
    .await;
    let answer = loop {
        let frame = next_frame(&mut gateway).await.unwrap();
        if frame["id"] == 2 {
            break frame;
        }
    };
    assert_eq!((answer["status"].as_u64(), answer["body"].as_bool()), (Some(404), Some(false)));
}

/// The gateway's next frame for this request, the others passed by.
async fn answer_to(gateway: &mut WebSocketStream<tokio::io::DuplexStream>, id: u64) -> Value {
    loop {
        let frame = next_frame(gateway).await.unwrap();
        if frame["id"] == id {
            return frame;
        }
    }
}

#[tokio::test]
async fn a_body_counts_until_its_request_ends_however_soon_its_handler_read_it() {
    let app = computer(&[]);
    let client = confirmed(&app, "gw-1");
    let (_, mut gateway) = home_link(&app, &client).await;
    assert_eq!(next_frame(&mut gateway).await.unwrap()["t"], "welcome");
    let ask = |id: u64| {
        json!({ "t": "req", "id": id, "method": "POST", "path": "/home/v1/llm/chat/completions",
                "headers": { "content-type": "application/json" }, "body": true })
    };
    // Two whole bodies of 16 MiB, not ended: their handlers read them as they come, and still hold them.
    let piece = vec![b' '; CHUNK];
    for id in [1u32, 2] {
        say(&mut gateway, ask(u64::from(id))).await;
        for _ in 0..BODY_MAX / CHUNK {
            gateway.send(Message::Binary(encode_chunk(id, false, &piece))).await.unwrap();
        }
    }
    // The link's 32 MiB are taken: the next body is refused at its first piece — the link's own answer, no body.
    say(&mut gateway, ask(3)).await;
    gateway.send(Message::Binary(encode_chunk(3, false, &piece))).await.unwrap();
    let refused = tokio::time::timeout(Duration::from_secs(10), answer_to(&mut gateway, 3)).await.expect("refused");
    assert_eq!(
        (refused["t"].as_str(), refused["status"].as_u64(), refused["body"].as_bool()),
        (Some("res"), Some(503), Some(false)),
        "{refused}"
    );
    // One of them given up: its 16 MiB come back, and a body reaches its handler again (no model here: its own words).
    say(&mut gateway, json!({ "t": "cancel", "id": 1 })).await;
    say(&mut gateway, ask(4)).await;
    gateway.send(Message::Binary(encode_chunk(4, true, b"{}"))).await.unwrap();
    let answered = answer_to(&mut gateway, 4).await;
    assert_eq!(answered["body"].as_bool(), Some(true), "the handler answered: {answered}");
}

#[tokio::test]
async fn an_app_whose_link_served_a_request_is_let_go_when_it_stops() {
    let app = computer(&[]);
    let client = confirmed(&app, "gw-1");
    let (served, mut gateway) = home_link(&app, &client).await;
    assert_eq!(next_frame(&mut gateway).await.unwrap()["t"], "welcome");
    say(
        &mut gateway,
        json!({ "t": "req", "id": 1, "method": "GET", "path": "/home/v1/status", "headers": {}, "body": false }),
    )
    .await;
    assert_eq!(answer_to(&mut gateway, 1).await["status"], 200);
    gateway.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), served).await.unwrap().unwrap();
    app.helpers.stop();
    let left = Arc::downgrade(&app);
    drop(app);
    assert!(left.upgrade().is_none(), "the link's server holds no app of its own");
}

/// A pairing's `ok` as a gateway says it: its names and the confirmation of the key it unwrapped from `paired` — or,
/// `damaged`, of that key with one bit turned.
fn ok_for(transcript: &Transcript, key: &ConnectionKey, paired: &Value, damaged: bool) -> Value {
    use base64::Engine;
    let mut unwrapped = wrapped_link_key(paired["key"].as_str().unwrap(), &transcript.key_wrap(key.bytes())).unwrap();
    if damaged {
        let last = if unwrapped.ends_with('0') { '1' } else { '0' };
        unwrapped.pop();
        unwrapped.push(last);
    }
    let mac = transcript.key_confirm(&link_key_hash(&unwrapped));
    json!({ "t": "ok", "gateway": { "name": "Кухня" }, "home": { "name": "Дом" },
            "keyMac": base64::engine::general_purpose::STANDARD.encode(mac) })
}

/// A later link's session at the app over an in-memory socket, as `upgrade` hands it on after the door's checks: what
/// serving it ended with, and the gateway's side. The gateway names itself `gw-1` of `home-1`.
async fn knocking(
    app: &Arc<App>,
    key: &str,
    source: &str,
) -> (tokio::task::JoinHandle<()>, WebSocketStream<tokio::io::DuplexStream>) {
    let (gateway, helper) = tokio::io::duplex(1 << 20);
    let gateway = WebSocketStream::from_raw_socket(gateway, Role::Client, None).await;
    let config = WebSocketConfig::default().max_frame_size(Some(FRAME_MAX)).max_message_size(Some(FRAME_MAX));
    let helper = WebSocketStream::from_raw_socket(helper, Role::Server, Some(config)).await;
    let (sink, stream) = tungstenite_wire(helper);
    let link_id = link_key_id(key).unwrap().to_string();
    let (client, key_hash) = app.clients.by_key_id(&link_id).unwrap().expect("a confirmed home");
    let knock = Knock {
        client,
        key_hash,
        binding: Binding::Tls(fingerprint(app)),
        link_id,
        gateway: "gw-1".into(),
        home: "home-1".into(),
        named: HelperIdentity { id: "gw-1".into(), name: "Колонка".into() },
        source: source.into(),
        bucket: source.into(),
        peer: SocketAddr::new(source.parse().unwrap(), 5000),
        deadline: None,
        unproven: None,
        up: false,
    };
    let app = app.clone();
    (tokio::spawn(async move { link_session(app, sink, stream, knock).await }), gateway)
}

/// Failures counted for a place now.
fn failures(app: &App, source: &str) -> usize {
    locked(&app.link.throttle.failures).get(source).map_or(0, |times| times.len())
}

/// A later link proves its key over a fresh challenge before it takes any place (review of block 1): a right proof
/// gets the welcome; the same proof again on a new connection, a proof of another key under the same id, and none
/// within `PROOF` are refused with 4401, each counted for its place.
#[tokio::test(start_paused = true)]
async fn a_link_proves_its_key_over_a_fresh_challenge_before_it_takes_a_place() {
    let app = computer(&[]);
    let (_, key) = confirmed_key(&app, "gw-1");
    let (_, mut gateway) = knocking(&app, &key, "192.0.2.20").await;
    let challenge = next_frame(&mut gateway).await.unwrap();
    assert_eq!(challenge["t"], "challenge");
    tokio::task::yield_now().await;
    assert_eq!(app.link.live_links(), 0, "no place before the proof");
    let proof = proof_of(&app, &key, &challenge);
    say(&mut gateway, proof.clone()).await;
    assert_eq!(next_frame(&mut gateway).await.unwrap()["t"], "welcome");
    assert_eq!((app.link.live_links(), failures(&app, "192.0.2.20")), (1, 0));

    // A proof seen once — on a plain hop, say — replayed on a new connection: its challenge is another.
    let (served, mut replayed) = knocking(&app, &key, "192.0.2.21").await;
    assert_eq!(next_frame(&mut replayed).await.unwrap()["t"], "challenge");
    say(&mut replayed, proof).await;
    assert_eq!(next_frame(&mut replayed).await, refused());
    served.await.unwrap();
    assert_eq!(failures(&app, "192.0.2.21"), 1);

    // Another key under the same id: refused the same.
    let (served, mut wrong) = knocking(&app, &key, "192.0.2.22").await;
    let challenge = next_frame(&mut wrong).await.unwrap();
    let other = format!("{}{}", &key[..key.len() - 1], if key.ends_with('0') { '1' } else { '0' });
    say(&mut wrong, proof_of(&app, &other, &challenge)).await;
    assert_eq!(next_frame(&mut wrong).await, refused());
    served.await.unwrap();
    assert_eq!(failures(&app, "192.0.2.22"), 1);

    // Silent after its challenge: closed once `PROOF` is over — `SILENT`, which a gateway tries again, never the 4401
    // that would end its tries (a station on a poor network) —, counted.
    let (served, mut silent) = knocking(&app, &key, "192.0.2.23").await;
    assert_eq!(next_frame(&mut silent).await.unwrap()["t"], "challenge");
    let started = Instant::now();
    assert_eq!(next_frame(&mut silent).await, Err(Some((SILENT, String::new()))));
    assert!(started.elapsed() >= PROOF, "{:?}", started.elapsed());
    served.await.unwrap();
    assert_eq!(failures(&app, "192.0.2.23"), 1);
    assert_eq!(app.link.live_links(), 1);
}

/// One home holds at most `LINKS_PER_CLIENT` links however many prove at once (review of block 1): the place is taken
/// with the port's under one lock, the rest closed `BUSY` — a gateway tries that again —, and never counted.
#[tokio::test]
async fn one_home_holds_at_most_its_links_however_many_prove_at_once() {
    let app = computer(&[]);
    let (_, key) = confirmed_key(&app, "gw-1");
    let mut gateways = Vec::new();
    for at in 0..LINKS_PER_CLIENT + 4 {
        let (_, mut gateway) = knocking(&app, &key, &format!("192.0.2.{at}")).await;
        let challenge = next_frame(&mut gateway).await.unwrap();
        let proof = proof_of(&app, &key, &challenge);
        gateways.push((gateway, proof));
    }
    for (gateway, proof) in &mut gateways {
        say(gateway, proof.clone()).await;
    }
    let (mut welcomed, mut busy) = (0, 0);
    for (gateway, _) in &mut gateways {
        match next_frame(gateway).await {
            Ok(frame) if frame["t"] == "welcome" => welcomed += 1,
            Err(Some((BUSY, _))) => busy += 1,
            other => panic!("{other:?}"),
        }
    }
    assert_eq!((welcomed, busy), (LINKS_PER_CLIENT, 4));
    assert_eq!(app.link.live_links(), LINKS_PER_CLIENT);
    assert!((0..LINKS_PER_CLIENT + 4).all(|at| failures(&app, &format!("192.0.2.{at}")) == 0));
}

/// A pairing socket that goes before its proof held — it read the challenge and closed — counts against its place as
/// a silent one does (review of block 1): ten in ten minutes and the place waits.
#[tokio::test]
async fn a_pairing_that_goes_before_its_proof_counts_against_its_place() {
    use base64::Engine;
    let app = computer(&[]);
    app.link.connection_key(true);
    for _ in 0..PAIR_FAILURES {
        let (mut gateway, _) = by_hand(&app, "192.0.2.40").await;
        let nonce = base64::engine::general_purpose::STANDARD.encode([5u8; 32]);
        say(&mut gateway, json!({ "t": "pair", "v": VERSION, "nonce": nonce, "gateway": "gw", "home": "home" })).await;
        assert_eq!(next_frame(&mut gateway).await.unwrap()["t"], "challenge");
        gateway.close(None).await.unwrap();
        let _ = closed_with(&mut gateway).await;
    }
    let blocked = tokio::time::timeout(Duration::from_secs(5), async {
        while !app.link.shared.pairing.failures.blocked("192.0.2.40") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    assert!(blocked.await.is_ok(), "ten in ten minutes");
}

/// A home's link state with no socket behind it: its way out goes nowhere.
fn bare_home(app: &Arc<App>) -> Home {
    let sink: WireSink = Box::pin(futures::sink::drain().sink_map_err(|_| ()));
    let (done, _) = mpsc::unbounded_channel();
    let identity = |id: &str| HelperIdentity { id: id.into(), name: String::new() };
    Home {
        origin: LinkOrigin {
            client: "link:000000000000".into(),
            own_home: false,
            home: identity("home-1"),
            gateway: identity("gw-1"),
        },
        peer: SocketAddr::new("192.0.2.5".parse().unwrap(), 5000),
        router: inner_router(app),
        out: Out::start(sink),
        running: HashMap::new(),
        held: 0,
        client_held: Arc::default(),
        done,
    }
}

/// A refused body's place is let go once its body falls silent, and a body that trickles is given up five minutes
/// after its request began, its place after it (review of block 1): neither is held for the link's whole life.
#[tokio::test(start_paused = true)]
async fn refused_and_endless_bodies_are_let_go() {
    let app = computer(&[]);
    let mut home = bare_home(&app);
    home.refuse(1, StatusCode::NOT_FOUND, true);
    assert!(home.running.get(&1).is_some_and(|running| running.task.is_none()), "its place waits for the end");
    tokio::time::advance(BODY_SILENCE).await;
    home.give_up_late_bodies(BODY_SILENCE, BODY_WHOLE);
    assert!(home.running.is_empty(), "a silent refused body's place is let go");

    let ask = json!({ "t": "req", "id": 2, "method": "POST", "path": "/home/v1/llm/chat/completions",
                      "headers": { "content-type": "application/json" }, "body": true });
    home.request(&ask).unwrap();
    let started = Instant::now();
    // A byte every ten seconds: never silent, but never whole either.
    while home.running.get(&2).is_some_and(|running| running.task.is_some()) {
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(home.feed(&encode_chunk(2, false, b" ")));
        home.give_up_late_bodies(BODY_SILENCE, BODY_WHOLE);
        assert!(started.elapsed() <= BODY_WHOLE + Duration::from_secs(10), "given up in time");
    }
    assert!(started.elapsed() >= BODY_WHOLE);
    assert_eq!(home.client_held.load(Ordering::SeqCst), 0, "its bytes given back");
    tokio::time::advance(Duration::from_secs(10)).await;
    assert!(home.feed(&encode_chunk(2, false, b" ")));
    home.give_up_late_bodies(BODY_SILENCE, BODY_WHOLE);
    assert!(home.running.is_empty(), "its place goes too");
}

/// `LINK_PROXY_FROM` without a warning: this machine, one proxy container's address, a small compose network (a /24,
/// an IPv6 /64); anything wider is warned of — a Docker host's whole 172.16.0.0/12 too (review of block 1).
#[test]
fn only_one_proxy_or_a_small_network_passes_without_a_warning() {
    let wide = |entries: &[&str]| wide_proxies(&entries.iter().map(|entry| entry.to_string()).collect::<Vec<_>>());
    let small = ["127.0.0.1", "127.0.0.0/8", "::1", "172.18.0.5", "172.18.0.0/24", "10.88.0.7/32", "fd00:1:2:3::/64"];
    assert!(wide(&small).is_empty(), "{:?}", wide(&small));
    let broad = ["172.16.0.0/12", "10.88.0.0/16", "172.18.0.0/23", "fd00::/48", "private", "0.0.0.0/0"];
    assert_eq!(wide(&broad), broad);
    assert!(wide(&["172.18.0.0/33"]).is_empty(), "a typo is named by its own warning");
}

/// The new link key crosses the pairing wrapped under the connection key and the transcript (review of block 1): the
/// `paired` frame's key opens nothing — a sniffer of a plain hop behind a proxy learns nothing usable —, while the
/// gateway, which knows the connection key, unwraps the key that works.
#[tokio::test]
async fn the_link_key_crosses_the_pairing_wrapped() {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let app = computer(&[]);
    let key = app.link.connection_key(true);
    let (mut gateway, binding) = by_hand(&app, "192.0.2.50").await;
    let nonce_g = [6u8; 32];
    say(
        &mut gateway,
        json!({ "t": "pair", "v": VERSION, "nonce": b64.encode(nonce_g), "gateway": "gw", "home": "home" }),
    )
    .await;
    let challenge = next_frame(&mut gateway).await.unwrap();
    let nonce_h: [u8; 32] = b64.decode(challenge["nonce"].as_str().unwrap()).unwrap().try_into().unwrap();
    let transcript =
        Transcript { nonce_g: &nonce_g, nonce_h: &nonce_h, binding: &binding, gateway: "gw", home: "home" };
    say(&mut gateway, json!({ "t": "proof", "mac": b64.encode(transcript.proof(key.bytes(), "gateway")) })).await;
    let paired = next_frame(&mut gateway).await.unwrap();
    let seen = paired["key"].as_str().expect("a key").to_string();
    say(&mut gateway, ok_for(&transcript, &key, &paired, false)).await;
    assert_eq!(next_frame(&mut gateway).await.unwrap()["t"], "confirmed");
    let unwrapped = wrapped_link_key(&seen, &transcript.key_wrap(key.bytes())).unwrap();
    assert_ne!(seen, unwrapped);
    assert_eq!(link_key_id(&seen), link_key_id(&unwrapped), "the id stays clear");
    assert!(app.clients.check(&seen).unwrap().is_none(), "the key as it crossed opens nothing");
    assert!(app.clients.check(&unwrapped).unwrap().is_some(), "the gateway's unwrapped key works");
    // Unwrapped with another connection key: nothing that works either.
    let guessed = wrapped_link_key(&seen, &transcript.key_wrap(ConnectionKey::new().bytes())).unwrap();
    assert!(app.clients.check(&guessed).unwrap().is_none());
}

/// Behind a proxy, links that have not proven their key yet are at most `PLAIN_UNPROVEN` at once: past them 429 before
/// the upgrade (review of block 1).
#[tokio::test]
async fn plain_links_not_proven_yet_are_few_at_once() {
    let app = computer(&[]);
    let outside = HelperOutside { url: "https://helper.example.com".into() };
    app.settings.set(&HELPER_OUTSIDE, &outside, None).unwrap();
    let (_, key) = confirmed_key(&app, "gw-1");
    let id = link_key_id(&key).unwrap().to_string();
    let knock = || {
        let mut request = Request::builder()
            .uri("/home/v1/link")
            .header("host", "helper.example.com")
            .header("x-forwarded-for", "198.51.100.7")
            .header("upgrade", "websocket")
            .header("connection", "upgrade")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header(LINK_ID_HEADER, id.as_str())
            .header(VERSION_HEADER, VERSION.to_string())
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new("127.0.0.1".parse().unwrap(), 5000)));
        crate::app::router(app.clone()).oneshot(request)
    };
    let held = app.link.plain_unproven.clone().acquire_many_owned(PLAIN_UNPROVEN as u32).await.unwrap();
    assert_eq!(knock().await.unwrap().status(), StatusCode::TOO_MANY_REQUESTS);
    drop(held);
    assert_ne!(knock().await.unwrap().status(), StatusCode::TOO_MANY_REQUESTS, "on to the upgrade");
    assert!(!app.link.throttle.blocked("proxied:198.51.100.7"), "never counted");
}

/// A proxy's clients are counted as the link port counts its own: IPv6 by its /56.
#[test]
fn a_proxys_ipv6_clients_are_counted_by_their_56() {
    let source = |client: &str| {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", client.parse().unwrap());
        proxied_source(&headers)
    };
    assert_eq!(source("2001:db8::1"), "proxied:2001:db8:0:0::/56");
    assert_eq!(source("2001:db8::1"), source("2001:db8:0:ff::2"));
    assert_ne!(source("2001:db8::1"), source("2001:db8:0:100::1"));
    assert_eq!(source("198.51.100.7"), "proxied:198.51.100.7");
    assert_eq!(proxied_source(&HeaderMap::new()), "proxied");
}

/// A link key damaged on its way — one bit of the wrapped key turned — fails the pairing at the gateway's `ok` (4400),
/// before anything is kept or the connection key spent (review of block 1), and the gateway hears the answer did not
/// match.
#[tokio::test]
async fn a_link_key_damaged_on_its_way_fails_the_pairing() {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let app = computer(&[]);
    let key = app.link.connection_key(true);
    let (mut gateway, binding) = by_hand(&app, "192.0.2.51").await;
    let nonce_g = [7u8; 32];
    say(
        &mut gateway,
        json!({ "t": "pair", "v": VERSION, "nonce": b64.encode(nonce_g), "gateway": "gw", "home": "home" }),
    )
    .await;
    let challenge = next_frame(&mut gateway).await.unwrap();
    let nonce_h: [u8; 32] = b64.decode(challenge["nonce"].as_str().unwrap()).unwrap().try_into().unwrap();
    let transcript =
        Transcript { nonce_g: &nonce_g, nonce_h: &nonce_h, binding: &binding, gateway: "gw", home: "home" };
    say(&mut gateway, json!({ "t": "proof", "mac": b64.encode(transcript.proof(key.bytes(), "gateway")) })).await;
    let paired = next_frame(&mut gateway).await.unwrap();
    say(&mut gateway, ok_for(&transcript, &key, &paired, true)).await;
    assert_eq!(next_frame(&mut gateway).await, Err(Some((PROTOCOL, String::new()))));
    let forgotten = tokio::time::timeout(Duration::from_secs(5), async {
        while app.settings.get(&HELPER_CLIENTS, None).is_some_and(|clients| !clients.is_empty()) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    assert!(forgotten.await.is_ok(), "nothing kept");
    assert_eq!(app.link.connection_key(false), key, "the key not spent");
    assert!(!app.link.shared.pairing.failures.blocked("192.0.2.51"));
}

/// On the link port a refusal at the door ends its connection (review of block 1): a pairing refused for want of a
/// place (429) is answered `Connection: close` and its connection — and its place among the connections — goes, rather
/// than staying alive with no deadline.
#[tokio::test]
async fn a_refusal_at_the_door_ends_its_connection_and_gives_its_place_back() {
    let app = computer(&[]);
    app.link.connection_key(true);
    for at in 0..PAIRINGS {
        locked(&app.link.shared.pairing.open).insert(format!("192.0.2.{at}"));
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let connections = Limits::new(CONNECTIONS, CONNECTIONS_PER_SOURCE);
    let listener = LinkListener::new(listener, app.link.tls().unwrap(), connections.clone())
        .with_times(Duration::from_secs(2), Duration::from_secs(30));
    let stop = CancellationToken::new();
    tokio::spawn(serve(app.clone(), listener, stop.clone().cancelled_owned()));
    let mut stream = tls(port).await;
    let request = format!(
        "GET /home/v1/link HTTP/1.1\r\nhost: 127.0.0.1\r\nupgrade: websocket\r\nconnection: upgrade\r\n\
         sec-websocket-version: 13\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n{PAIR_HEADER}: pair\r\n\
         {VERSION_HEADER}: {VERSION}\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut answer)).await;
    assert!(read.is_ok(), "the connection ends well before its 30 s");
    let answer = String::from_utf8_lossy(&answer).to_ascii_lowercase();
    assert!(answer.starts_with("http/1.1 429") && answer.contains("connection: close"), "{answer}");
    let given_back = tokio::time::timeout(Duration::from_secs(5), async {
        while locked(&connections.counts).all > 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    assert!(given_back.await.is_ok(), "its place among the connections given back");
    stop.cancel();
}

/// What a link proves over is what its pairing bound, whatever «Адрес снаружи» says by then; a home paired before that
/// was kept proves over today's, and with none at all it is told to come again — 503, never the 401 that ends its
/// tries. Behind a proxy an unarmed pairing socket takes a place among the plain sockets not proven yet; another link
/// version is answered 426 with this one's (review of block 1).
#[tokio::test]
async fn the_door_behind_a_proxy_and_the_link_version() {
    let app = computer(&[]);
    let plain = |headers: &[(&str, String)]| {
        let mut request = Request::builder()
            .uri("/home/v1/link")
            .header("host", "helper.example.com")
            .header("x-forwarded-for", "198.51.100.7")
            .header("upgrade", "websocket")
            .header("connection", "upgrade")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
        for (name, value) in headers {
            request = request.header(*name, value.as_str());
        }
        let mut request = request.body(Body::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new("127.0.0.1".parse().unwrap(), 5000)));
        let app = app.clone();
        async move {
            let response = crate::app::router(app).oneshot(request).await.unwrap();
            (response.status(), response.headers().get(VERSION_HEADER).cloned())
        }
    };
    let version = (VERSION_HEADER, VERSION.to_string());
    // Paired behind the proxy by the outside name, then «Адрес снаружи» cleared: the stored binding still serves.
    let bound = Binding::Name("outside.example.com".into());
    let (id, key) = app.clients.pair("home-1", "gw-1", false, Some(&bound)).unwrap();
    assert!(app.clients.confirm(&id, Names::default()));
    let (client, _) = app.clients.by_key_id(link_key_id(&key).unwrap()).unwrap().unwrap();
    assert_eq!(client.binding, Some(bound));
    let knock = |key: &str| vec![version.clone(), (LINK_ID_HEADER, link_key_id(key).unwrap().to_string())];
    let past_the_door = |status: StatusCode| status != StatusCode::UNAUTHORIZED && status.as_u16() != 503;
    assert!(past_the_door(plain(&knock(&key)).await.0), "on to the upgrade with no «Адрес снаружи»");
    // Paired before the binding was kept: today's «Адрес снаружи», and with none — come again.
    let (old, old_key) = app.clients.pair("home-2", "gw-2", false, None).unwrap();
    assert!(app.clients.confirm(&old, Names::default()));
    assert_eq!(plain(&knock(&old_key)).await.0.as_u16(), 503);
    app.settings.set(&HELPER_OUTSIDE, &HelperOutside { url: "https://outside.example.com".into() }, None).unwrap();
    assert!(past_the_door(plain(&knock(&old_key)).await.0));

    // Unarmed pairing sockets behind the proxy: past `PLAIN_UNPROVEN` of them, 429 before the upgrade.
    let pair = vec![version.clone(), (PAIR_HEADER, "pair".to_string())];
    let held = app.link.plain_unproven.clone().acquire_many_owned(PLAIN_UNPROVEN as u32).await.unwrap();
    assert_eq!(plain(&pair).await.0, StatusCode::TOO_MANY_REQUESTS);
    drop(held);
    assert_ne!(plain(&pair).await.0, StatusCode::TOO_MANY_REQUESTS);

    // Another version, or none: 426 naming this one, before anything else.
    for other in [vec![(VERSION_HEADER, "1".to_string())], vec![]] {
        let (status, told) = plain(&other).await;
        assert_eq!(status, StatusCode::UPGRADE_REQUIRED);
        assert_eq!(told.as_ref().and_then(|told| told.to_str().ok()), Some(VERSION.to_string().as_str()));
    }
}

// ── Up: this computer's own requests to its home's centre over the link (stage 7 of home-setup-flow) ──

/// A request to the app's router from `from`, signed with `token` (none: no header): its status and its JSON.
async fn ask(app: &Arc<App>, from: &str, method: &str, uri: &str, token: &str) -> (StatusCode, Value) {
    let body = if method == "GET" || method == "HEAD" { Body::empty() } else { Body::from("{}") };
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("accept-language", "ru");
    if !token.is_empty() {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let mut request = request.body(body).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(from.parse().unwrap(), 5000)));
    let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// A person of the app with this role, signed in: their token.
async fn signed_in(app: &Arc<App>, name: &str, role: &str) -> String {
    let input = json!({ "name": name, "role": role, "password": "person-password-1" });
    app.access.users.create(input.as_object().unwrap(), None).await.unwrap();
    match app.access.auth.login(Some(name), "person-password-1", "test").await.unwrap() {
        crate::access::auth_service::LoginResult::Ok(session) => session.token,
        _ => panic!("{name} signs in"),
    }
}

/// This computer's own request straight over its link — past the pass's checks —, for `person` when named.
fn own_request(method: &str, path: &str, person: Option<UpPerson>) -> Request {
    let mut request = Request::builder().method(method).uri(path).body(Body::empty()).unwrap();
    if let Some(person) = person {
        request.extensions_mut().insert(person);
    }
    request
}

/// An answer's status and JSON.
async fn answered(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// Two gateways over a real link (TLS on this machine's link port): the centre pairs this computer as a Turbo at home
/// by its connection key, its link says it takes the Turbo's own requests, and the pass `/station/<client>/…` reaches
/// the centre's panel API as the person signed in at the Turbo — their name, their role there. The centre refuses what
/// is not its panel's API and a request that names nobody, never lets a role above an admin's, and shuts its door once
/// the Turbo is no longer «дома» in its list.
#[tokio::test]
async fn an_up_request_over_a_real_link_reaches_the_centres_api_as_the_person_signed_in_at_the_turbo() {
    // The Turbo: a computer that helps, its link port open on this machine; its admin and a member.
    let turbo = computer(&[]);
    let (port, stop, _) = link_port(&turbo, Duration::from_secs(10)).await;
    let mama = turbo.access.auth.setup("admin-password-1", "Мама", "test").await.unwrap().unwrap().token;
    let anya = signed_in(&turbo, "Аня", "member").await;
    // The centre: another computer with its own admin; it pairs the Turbo by its connection key («Подключить Турбо»).
    let centre = computer(&[]);
    centre.access.auth.setup("centre-password-1", "Папа", "test").await.unwrap();
    centre.helpers.start();
    centre.helpers.set_local_port(9);
    let key = turbo.link.connection_key(true).shown();
    let url = format!("https://127.0.0.1:{port}");
    let entry = crate::helpers::routes::pair_with_key(
        (&centre.identity, &centre.secrets, &centre.helpers),
        &url,
        &key,
        None,
        None,
    )
    .await
    .expect("paired");
    assert!(centre.helpers.takes_up(&entry), "a Turbo at home");
    let client = turbo.clients.view(&[]).into_iter().find(|view| view.via == "link").expect("the centre's home").id;
    let linked = tokio::time::timeout(Duration::from_secs(10), async {
        while turbo.link.up_of(&client).is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    assert!(linked.await.is_ok(), "the centre's link takes the Turbo's own requests");
    assert_eq!(turbo.link.takes_up(&client), Some(true));
    // The Turbo's /health names the home's centre by the key of its pass: the panel goes there (`?via=`).
    let home = "192.168.1.20";
    let (_, health) = ask(&turbo, home, "GET", "/health", &mama).await;
    let named = &health["message"]["placement"]["centre"];
    assert_eq!((named["key"].as_str(), named["online"].as_bool()), (Some(client.as_str()), Some(true)), "{health}");

    let pass = |rest: &str| format!("/station/{client}/{rest}");
    // Each person reaches the centre's API as who they are here, with their role here.
    let (status, body) = ask(&turbo, home, "GET", &pass("api/v1/auth/me"), &mama).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (body["message"]["role"].as_str(), body["message"]["name"].as_str()),
        (Some("admin"), Some("Турбо (Мама)"))
    );
    let (status, body) = ask(&turbo, home, "GET", &pass("api/v1/auth/me"), &anya).await;
    assert_eq!((status, body["message"]["role"].as_str()), (StatusCode::OK, Some("member")), "{body}");
    // The centre's own health, for the panel's window.
    let (status, body) = ask(&turbo, home, "GET", &pass("health"), &anya).await;
    assert_eq!((status, body["message"]["placement"]["role"].as_str()), (StatusCode::OK, Some("pc")), "{body}");
    // A change of the home's settings: an admin's here; a member's is refused before it goes.
    let (status, body) = ask(&turbo, home, "PUT", &pass("api/v1/brain"), &anya).await;
    assert_eq!(
        (status, body["message"].as_str()),
        (StatusCode::FORBIDDEN, Some("Править настройки дома отсюда может администратор."))
    );
    // Refused at the Turbo, nothing sent: from outside the home, a key's script, what is not the panel's API.
    assert_eq!(ask(&turbo, "8.8.8.8", "GET", &pass("api/v1/auth/me"), &mama).await.0, StatusCode::FORBIDDEN);
    let grants = serde_json::Map::from_iter([
        ("name".to_string(), json!("Скрипт")),
        ("grants".to_string(), json!(["station.view"])),
    ]);
    let script = turbo.access.keys.create(&grants, None).unwrap().secret;
    let (status, body) = ask(&turbo, home, "GET", &pass("api/v1/auth/me"), &script).await;
    assert_eq!(
        (status, body["message"].as_str()),
        (StatusCode::FORBIDDEN, Some("Пульт дома отсюда открывает человек, а не ключ."))
    );
    for rest in ["home/v1/status", "station/h:kitchen/api/v1/brain", "api/v1/../home/v1/status", "debug/sessions"] {
        assert_eq!(ask(&turbo, home, "GET", &pass(rest), &mama).await.0, StatusCode::NOT_FOUND, "{rest}");
    }
    assert_eq!(
        ask(&turbo, home, "GET", "/station/link:000000000000/api/v1/auth/me", &mama).await.0,
        StatusCode::NOT_FOUND
    );

    // The centre's own door, straight over the link: only its panel's API, only for a person, never above an admin.
    let calls = turbo.link.up_of(&client).unwrap();
    let wait = Duration::from_secs(10);
    let mother = || Some(UpPerson { name: "Мама".into(), role: "admin".into(), id: "not-the-centres".into() });
    for path in ["/home/v1/status", "/station/h:kitchen/api/v1/brain", "/api/v1/../home/v1/status", "/debug/sessions"] {
        let response = calls.call_within(own_request("GET", path, mother()), wait).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    let response = calls.call_within(own_request("GET", "/api/v1/auth/me", None), wait).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN, "nobody named");
    let owner = Some(UpPerson { name: "Кто-то".into(), role: "owner".into(), id: String::new() });
    let (status, body) =
        answered(calls.call_within(own_request("GET", "/api/v1/auth/me", owner), wait).await.unwrap()).await;
    assert_eq!(
        (status, body["message"]["role"].as_str()),
        (StatusCode::OK, Some("guest")),
        "an unknown role: the least"
    );
    let (_, body) =
        answered(calls.call_within(own_request("GET", "/api/v1/auth/me", mother()), wait).await.unwrap()).await;
    assert_eq!(body["message"]["id"], "helper", "an id the centre does not know is nobody's there");

    // The centre makes it a server of its own: no longer «дома», the door shuts at the next request; the pass says why.
    centre
        .helpers
        .write(None, |list| {
            list.iter_mut().find(|listed| listed.id == entry).unwrap().kind = crate::helpers::HelperKind::Own;
            Ok(Some(()))
        })
        .unwrap();
    assert!(!centre.helpers.takes_up(&entry));
    let (status, body) = ask(&turbo, home, "GET", &pass("api/v1/auth/me"), &mama).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body["message"].as_str().unwrap_or("").contains("не «дома»"), "{body}");
    centre.helpers.stop();
    stop.cancel();
}

/// A centre whose link says nothing of `up` (an older DuoWave) is never asked: the pass says so at once and nothing
/// crosses the link, and /health names no centre — the panel keeps this computer's own desk. A centre that takes `up`
/// but keeps silent: the request waits its time and goes — cancelled at the centre, its place let go —, and the link
/// goes on. A request of the centre's own in this computer's ids breaks the protocol.
#[tokio::test(start_paused = true)]
async fn an_older_centre_is_never_asked_and_a_silent_one_lets_the_request_go_in_time() {
    let app = computer(&[]);
    let mama = app.access.auth.setup("admin-password-1", "Мама", "test").await.unwrap().unwrap().token;
    let home = "192.168.1.20";
    let pass = |client: &str| format!("/station/{client}/api/v1/brain");
    let older = confirmed(&app, "gw-1");
    let (_, mut gateway) = home_link_taking(&app, &older, false).await;
    assert_eq!(next_frame(&mut gateway).await.unwrap()["t"], "welcome");
    assert!(app.link.up_of(&older).is_none());
    assert_eq!(app.link.takes_up(&older), Some(false));
    let (status, body) = ask(&app, home, "GET", &pass(&older), &mama).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let quiet = tokio::time::timeout(Duration::from_secs(1), next_frame(&mut gateway)).await;
    assert!(quiet.is_err(), "nothing asked of it: {quiet:?}");
    let (_, health) = ask(&app, home, "GET", "/health", &mama).await;
    assert!(health["message"]["placement"].get("centre").is_none(), "{health}");

    let silent = confirmed(&app, "gw-2");
    let (_, mut gateway) = home_link_taking(&app, &silent, true).await;
    assert_eq!(next_frame(&mut gateway).await.unwrap()["t"], "welcome");
    let calls = app.link.up_of(&silent).expect("its own requests go over this link");
    let asking = {
        let (app, uri, mama) = (app.clone(), pass(&silent), mama.clone());
        tokio::spawn(async move { ask(&app, home, "GET", &uri, &mama).await })
    };
    let asked = next_frame(&mut gateway).await.unwrap();
    assert_eq!(
        (asked["t"].as_str(), asked["method"].as_str(), asked["path"].as_str()),
        (Some("up"), Some("GET"), Some("/api/v1/brain")),
        "{asked}"
    );
    let id = asked["id"].as_u64().unwrap();
    assert!(id >= u64::from(UP_BIT), "in this computer's own ids: {id}");
    assert_eq!((asked["person"]["name"].as_str(), asked["person"]["role"].as_str()), (Some("Мама"), Some("admin")));
    assert!(asked["headers"].get("authorization").is_none(), "never the panel's key: {asked}");
    let (status, body) = asking.await.unwrap();
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(next_frame(&mut gateway).await.unwrap(), json!({ "t": "up-cancel", "id": id }));
    assert_eq!(calls.open_streams(), 0, "its place let go");
    // The link goes on: the centre's own request is answered as ever.
    say(
        &mut gateway,
        json!({ "t": "req", "id": 1, "method": "GET", "path": "/home/v1/status", "headers": {}, "body": false }),
    )
    .await;
    assert_eq!(answer_to(&mut gateway, 1).await["status"], 200);
    // The centre's request in this computer's ids: a protocol error, the link closed.
    let theirs = u64::from(UP_BIT) + 1;
    say(
        &mut gateway,
        json!({ "t": "req", "id": theirs, "method": "GET", "path": "/home/v1/status", "headers": {}, "body": false }),
    )
    .await;
    assert_eq!(next_frame(&mut gateway).await, Err(Some((PROTOCOL, String::new()))));
}
