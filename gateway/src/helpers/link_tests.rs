//! The link's gateway side alone: the keys, the proofs and the bindings, the verifiers (over an in-memory TLS pair),
//! the frames and a session over an in-memory socket (its clock paused where time counts), the pauses, the bridge.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Request};
use axum::http::StatusCode;
use futures::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tower::ServiceExt;

use super::*;
use crate::helpers::remote::Me;
use crate::settings::definitions::HelperPin;

#[test]
fn a_connection_key_reads_back_whatever_the_dashes_and_the_case() {
    for _ in 0..50 {
        let key = ConnectionKey::new();
        let shown = key.shown();
        let groups: Vec<usize> = shown.split('-').map(str::len).collect();
        assert_eq!(groups, [5, 5, 5, 5, 6], "{shown}");
        assert!(shown.chars().all(|c| c == '-' || CROCKFORD.contains(&(c as u8))), "{shown}");
        assert_eq!(ConnectionKey::parse(&shown), Some(key.clone()));
        assert_eq!(ConnectionKey::parse(&shown.to_lowercase()), Some(key.clone()), "the case aside");
        assert_eq!(ConnectionKey::parse(&shown.replace('-', " ")), Some(key.clone()), "spaces for dashes");
        assert_eq!(ConnectionKey::parse(&shown.replace('-', "")), Some(key.clone()), "no dashes");
    }
    // I and L read as 1, O as 0: what a person may type for what they saw.
    let one = ConnectionKey([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    assert_eq!(one.shown(), "00000-00000-00000-00000-000001");
    assert_eq!(ConnectionKey::parse("OOOOO-ooooo-OOOOO-OOOOO-OOOOOI"), Some(one.clone()));
    assert_eq!(ConnectionKey::parse("00000 00000 00000 00000 00000l"), Some(one));
    // 25 or 27 characters, a letter Crockford leaves out, more than 128 bits: none.
    assert_eq!(ConnectionKey::parse("00000-00000-00000-00000-00000"), None);
    assert_eq!(ConnectionKey::parse("00000-00000-00000-00000-0000000"), None);
    assert_eq!(ConnectionKey::parse("U0000-00000-00000-00000-000000"), None);
    assert_eq!(ConnectionKey::parse("80000-00000-00000-00000-000000"), None, "2^128 does not fit");
    assert!(ConnectionKey::parse("70000-00000-00000-00000-000000").is_some());
    assert_eq!(ConnectionKey::parse(""), None);
}

#[test]
fn a_link_key_reads_back_by_its_three_parts() {
    let (id, key) = new_link_key();
    assert_eq!(link_key_id(&key), Some(id.as_str()));
    assert_eq!(key.len(), "dwl1_".len() + 12 + 1 + 64);
    assert!(is_link_key(&key));
    let secret = "a".repeat(64);
    assert_eq!(link_key_id(&format!("dwl1_0123456789ab_{secret}")), Some("0123456789ab"));
    for wrong in [
        format!("dwl1_0123456789a_{secret}"),
        format!("dwl1_0123456789AB_{secret}"),
        format!("dwl2_0123456789ab_{secret}"),
        format!("dwl1_0123456789ab_{secret}_"),
        format!("dwl1_0123456789ab_{}", "a".repeat(63)),
        "dwl1_0123456789ab".to_string(),
        format!("vgw_0123456789ab_{secret}"),
    ] {
        assert_eq!(link_key_id(&wrong), None, "{wrong}");
    }
}

fn transcript<'a>(binding: &'a Binding, nonce_g: &'a [u8; 32], nonce_h: &'a [u8; 32]) -> Transcript<'a> {
    Transcript { nonce_g, nonce_h, binding, gateway: "gateway-id", home: "home-id" }
}

#[test]
fn the_proofs_bind_the_certificate_or_the_name_both_sides_saw() {
    let key = [9u8; 16];
    let (g, h) = ([1u8; 32], [2u8; 32]);
    let certificate = Binding::Tls([7; 32]);
    let name = Binding::Name("helper.example.com".into());
    let proof = |binding: &Binding, g: &[u8; 32], h: &[u8; 32], role: &str| transcript(binding, g, h).proof(&key, role);
    // The same binding on both sides: the same proof.
    assert_eq!(proof(&certificate, &g, &h, "gateway"), proof(&Binding::Tls([7; 32]), &g, &h, "gateway"));
    // A certificate against a name, another certificate, another nonce, another key: another proof.
    assert_ne!(proof(&certificate, &g, &h, "gateway"), proof(&name, &g, &h, "gateway"));
    assert_ne!(proof(&certificate, &g, &h, "gateway"), proof(&Binding::Tls([8; 32]), &g, &h, "gateway"));
    assert_ne!(proof(&certificate, &g, &h, "gateway"), proof(&certificate, &h, &g, "gateway"));
    assert_ne!(transcript(&certificate, &g, &h).proof(&[8; 16], "gateway"), proof(&certificate, &g, &h, "gateway"));
    // The gateway's proof is no helper's mac.
    assert_ne!(proof(&certificate, &g, &h, "gateway"), proof(&certificate, &g, &h, "helper"));
    // Typed: a fingerprint never reads as a name.
    assert!(certificate.bytes().starts_with(b"tls:") && name.bytes().starts_with(b"name:"));
    // What goes in: the nonces, the binding and the two ids — no name, no helper id.
    let mut expected = b"duowave link pair v1\0gateway\0".to_vec();
    expected.extend_from_slice(&g);
    expected.extend_from_slice(&h);
    for part in [certificate.bytes(), b"gateway-id".to_vec(), b"home-id".to_vec()] {
        expected.extend_from_slice(&(part.len() as u16).to_be_bytes());
        expected.extend_from_slice(&part);
    }
    assert_eq!(transcript(&certificate, &g, &h).bytes("gateway"), expected);
}

#[test]
fn the_home_proof_holds_only_with_the_same_home_key() {
    let home_key = crate::utils::ids::random_hex(32);
    let (g, h) = ([1u8; 32], [2u8; 32]);
    let binding = Binding::Tls([7; 32]);
    let at = transcript(&binding, &g, &h);
    assert_eq!(at.home_proof(&home_key), transcript(&binding, &g, &h).home_proof(&home_key));
    assert_ne!(at.home_proof(&home_key), at.home_proof(&crate::utils::ids::random_hex(32)), "another home's key");
    assert_ne!(at.home_proof(&home_key), transcript(&binding, &h, &g).home_proof(&home_key), "other nonces");
    assert_ne!(at.home_proof(""), at.home_proof(&home_key), "no key is no home");
}

#[test]
fn the_first_meeting_binds_a_name_only_for_a_public_chain() {
    let dns = Target::of("https://Helper.Example.com.:7781").unwrap();
    assert_eq!((dns.host.as_str(), dns.port, dns.dns), ("helper.example.com", 7781, true));
    let ip = Target::of("https://192.0.2.10").unwrap();
    assert_eq!((ip.host.as_str(), ip.port, ip.dns), ("192.0.2.10", 443, false));
    assert_eq!(Target::of("http://192.0.2.10:7781"), None, "never plain");
    let public = Seen { fingerprint: [3; 32], public: true };
    let private = Seen { fingerprint: [3; 32], public: false };
    assert_eq!(binding_of(&dns, Some(public)), Some(Binding::Name("helper.example.com".into())));
    assert_eq!(binding_of(&ip, Some(public)), Some(Binding::Tls([3; 32])), "an address is bound by its certificate");
    assert_eq!(binding_of(&dns, Some(private)), Some(Binding::Tls([3; 32])), "a self-signed one too");
    assert_eq!(binding_of(&dns, None), None, "no certificate, no meeting");
    assert_eq!(Binding::Name("helper.example.com".into()).pin().public.as_deref(), Some("helper.example.com"));
    assert_eq!(Binding::Tls([3; 32]).pin().sha256, Some(hex::encode([3u8; 32])));
}

/// What a helper says of itself at a pairing goes into the travelling list, its id fixed: read as a contract's
/// (contract.rs) — an id that does not read as a gateway's is none, which the pairing's check refuses
/// (`Helpers::pair_check`); a name cleaned of control and turning characters and cut at 60.
#[tokio::test]
async fn a_pairing_reads_what_the_helper_says_of_itself_as_a_contract_does() {
    for (said, read) in
        [("0b9a6c1e-5d2f-4c8a-9e3b-7f1d2a4c6e80", "0b9a6c1e-5d2f-4c8a-9e3b-7f1d2a4c6e80"), ("pc 1/..", "")]
    {
        let (gateway, helper) = tokio::io::duplex(1 << 16);
        let config = Some(ws_config(PAIR_FRAME_MAX));
        let mut gateway = WebSocketStream::from_raw_socket(gateway, Role::Client, config).await;
        let mut helper = WebSocketStream::from_raw_socket(helper, Role::Server, config).await;
        let (key, binding, me) = (ConnectionKey::new(), Binding::Tls([7; 32]), Me::default_for_tests());
        let (proved, bound, gateway_id, home_id) =
            (key.clone(), binding.clone(), me.gateway.id.clone(), me.home.id.clone());
        // The helper's side by hand: its challenge, then its proof with what it says of itself.
        let helps = tokio::spawn(async move {
            let pair = next_text(&mut helper).await;
            let nonce_g = unb64_32(pair["nonce"].as_str()).expect("the gateway's nonce");
            let nonce_h = [9u8; 32];
            text(&mut helper, json!({ "t": "challenge", "nonce": b64(&nonce_h) })).await;
            assert_eq!(next_text(&mut helper).await["t"], "proof");
            let transcript = Transcript {
                nonce_g: &nonce_g,
                nonce_h: &nonce_h,
                binding: &bound,
                gateway: &gateway_id,
                home: &home_id,
            };
            let name = format!("Ком\u{202E}пьютер\n{}", "я".repeat(100));
            let mac = b64(&transcript.proof(proved.bytes(), "helper"));
            text(
                &mut helper,
                json!({ "t": "paired", "mac": mac, "helper": { "id": said, "name": name }, "status": {} }),
            )
            .await;
            helper
        });
        let checked: Arc<Mutex<Option<HelperIdentity>>> = Arc::default();
        let into = checked.clone();
        let check = move |helper: &HelperIdentity, _: &HelperPin| -> Result<(), crate::platform::http::ApiError> {
            *into.lock().unwrap() = Some(helper.clone());
            Ok(())
        };
        let timing = LinkTiming::default();
        let paired = pair_over(&mut gateway, &key, &me, "", &binding, &timing, true, &check).await.expect("a probe");
        drop(helps.await.unwrap());
        let told = checked.lock().unwrap().clone().expect("checked before anything is spent");
        assert_eq!((told.id.as_str(), paired.helper.id.as_str()), (read, read), "{said}");
        assert!(told.name.starts_with("Компьютер я") && told.name.chars().count() == 60, "{}", told.name);
    }
}

#[test]
fn the_pauses_after_a_drop_are_1_2_5_10_30_then_60_seconds() {
    let timing = LinkTiming::default();
    let pauses: Vec<u64> = (0..8).map(|failures| timing.pause(failures).as_secs()).collect();
    assert_eq!(pauses, [1, 2, 5, 10, 30, 60, 60, 60]);
    assert_eq!(
        (timing.steady, timing.silence, timing.ping),
        (Duration::from_secs(60), Duration::from_secs(30), Duration::from_secs(15))
    );
    // A link's tries: the pauses in turn while it fails.
    let mut backoff = Backoff::default();
    let after = |backoff: &mut Backoff, up: Option<u64>| backoff.after(&timing, up.map(Duration::from_secs)).as_secs();
    let failing: Vec<u64> = (0..3).map(|_| after(&mut backoff, None)).collect();
    assert_eq!(failing, [1, 2, 5]);
    // Back for a moment, less than `steady`: the pauses go on where they stopped.
    assert_eq!(after(&mut backoff, Some(10)), 10);
    assert_eq!(after(&mut backoff, None), 30);
    // Up for `steady` and more: a drop starts them from the first again.
    assert_eq!(after(&mut backoff, Some(60)), 1);
    assert_eq!(after(&mut backoff, None), 2);
    assert_eq!(after(&mut backoff, Some(3600)), 1);
}

#[test]
fn frames_go_both_ways_and_unknown_kinds_are_passed_by() {
    let frame = encode_chunk(7, false, b"piece");
    assert_eq!(&frame[..5], &[0, 0, 0, 7, 0]);
    assert_eq!(decode_chunk(&frame), Some((7, false, Bytes::from_static(b"piece"))));
    assert_eq!(decode_chunk(&encode_chunk(u32::MAX, true, b"")), Some((u32::MAX, true, Bytes::new())));
    assert_eq!(decode_chunk(&Bytes::from_static(&[0, 0, 0, 7])), None, "too short");
    assert_eq!(decode_chunk(&encode_chunk(1, false, &vec![0u8; CHUNK + 1])), None, "over 64 KiB");
    assert!(decode_chunk(&encode_chunk(1, false, &vec![0u8; CHUNK])).is_some());
    for kind in ["req", "res", "cancel", "error", "event", "bye", "welcome", "something-new"] {
        let (read, _) = read_text(&json!({ "t": kind, "id": 1 }).to_string()).unwrap();
        assert_eq!(read, kind);
    }
    assert_eq!(read_text("[1, 2]"), None);
    assert_eq!(read_text("{\"id\": 1}"), None, "no kind");
    // The request carries what its answer depends on, never the bearer; the answer all but hop-by-hop and cookies.
    let mut headers = axum::http::HeaderMap::new();
    for (name, value) in
        [("content-type", "audio/wav"), ("authorization", "Bearer x"), ("accept-language", "ru"), ("cookie", "c")]
    {
        headers.insert(name, value.parse().unwrap());
    }
    let carried = headers_json(&headers, |name| CARRIED.contains(&name));
    assert_eq!(carried, json!({ "content-type": "audio/wav", "accept-language": "ru" }));
    let back = headers_from(
        &json!({ "Content-Type": "text/event-stream", "set-cookie": "a", "transfer-encoding": "chunked" }),
        carried_back,
    );
    assert_eq!(back.len(), 1);
    assert_eq!(back["content-type"], "text/event-stream");
}

#[test]
fn a_wrapped_link_key_keeps_its_id_and_unwraps_with_the_same_pad() {
    let (id, key) = new_link_key();
    let (pad, other) = ([9u8; 32], [8u8; 32]);
    let wrapped = wrapped_link_key(&key, &pad).unwrap();
    assert_ne!(wrapped, key);
    assert_eq!(link_key_id(&wrapped), Some(id.as_str()));
    assert_eq!(wrapped_link_key(&wrapped, &pad), Some(key.clone()));
    assert_ne!(wrapped_link_key(&wrapped, &other), Some(key));
    assert_eq!(wrapped_link_key("not a key", &pad), None);
    // The pad is the transcript's own, under its own label: never a proof of the key.
    let binding = Binding::Tls([7; 32]);
    let at = transcript(&binding, &[1; 32], &[2; 32]);
    assert_ne!(at.key_wrap(&[3; 16]), at.proof(&[3; 16], "helper"));
    assert_ne!(at.key_wrap(&[3; 16]), transcript(&binding, &[1; 32], &[4; 32]).key_wrap(&[3; 16]));
}

#[test]
fn a_links_proof_binds_its_key_the_pin_the_nonce_and_the_ids() {
    let (key, nonce) = (link_key_hash("dwl1_0123456789ab_x"), [4u8; 32]);
    let pin = Binding::Tls([7; 32]);
    let proof = |key: &[u8; 32], nonce: &[u8; 32], binding: &Binding, ids: [&str; 3]| {
        link_proof(key, nonce, binding, ids[0], ids[1], ids[2])
    };
    let ids = ["0123456789ab", "gateway-id", "home-id"];
    assert_eq!(proof(&key, &nonce, &pin, ids), proof(&key, &nonce, &Binding::Tls([7; 32]), ids));
    assert_ne!(proof(&key, &nonce, &pin, ids), proof(&link_key_hash("dwl1_0123456789ab_y"), &nonce, &pin, ids));
    assert_ne!(proof(&key, &nonce, &pin, ids), proof(&key, &[5; 32], &pin, ids), "another challenge");
    assert_ne!(proof(&key, &nonce, &pin, ids), proof(&key, &nonce, &Binding::Tls([8; 32]), ids), "another certificate");
    assert_ne!(proof(&key, &nonce, &pin, ids), proof(&key, &nonce, &Binding::Name("a.example".into()), ids));
    assert_ne!(proof(&key, &nonce, &pin, ids), proof(&key, &nonce, &pin, ["0123456789ab", "gateway-id", "home-2"]));
    // Never a pairing's proof under the same key: another label.
    let transcript = Transcript { nonce_g: &nonce, nonce_h: &nonce, binding: &pin, gateway: ids[1], home: ids[2] };
    assert_ne!(proof(&key, &nonce, &pin, ids), transcript.proof(&key, "gateway"));
}

/// While a link opens, only the helper's 4401 ends the tries; no place for it now (`BUSY`), or anything else, is tried
/// again.
#[tokio::test]
async fn only_a_refusal_ends_a_links_tries_and_busy_is_tried_again() {
    for (code, expected) in [
        (REFUSED, Failure::Final(REFUSED_WORDS)),
        (BUSY, Failure::Again(super::super::AWAY)),
        (PROTOCOL, Failure::Again(super::super::AWAY)),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let helper = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(tcp).await.unwrap();
            let frame = CloseFrame { code: CloseCode::from(code), reason: "".into() };
            let _ = socket.send(Message::Close(Some(frame))).await;
            let _ = socket.next().await;
        });
        let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let url = format!("ws://127.0.0.1:{port}{PATH}");
        let (mut socket, _) =
            tokio_tungstenite::client_async(url, tokio_tungstenite::MaybeTlsStream::Plain(tcp)).await.unwrap();
        assert_eq!(opening(&mut socket, Duration::from_secs(5)).await.unwrap_err(), expected, "{code}");
        drop(socket);
        let _ = helper.await;
    }
}

// ── A session over an in-memory socket ──

fn news_kept() -> (Arc<Mutex<Vec<LinkNews>>>, NewsSink) {
    let kept: Arc<Mutex<Vec<LinkNews>>> = Arc::default();
    let into = kept.clone();
    (kept, Arc::new(move |_: &str, news: LinkNews| into.lock().unwrap().push(news)))
}

fn status(helper: &str) -> Value {
    json!({ "version": "0.12.0", "peerApi": 2, "contract": { "version": 1, "helper": { "id": helper, "name": "Компьютер" }, "abilities": [] } })
}

/// A link that does not dial, its session over an in-memory socket served by hand; the test is the helper.
struct Served {
    link: Arc<Link>,
    session: Arc<Session>,
    helper: WebSocketStream<tokio::io::DuplexStream>,
    news: Arc<Mutex<Vec<LinkNews>>>,
    end: tokio::task::JoinHandle<End>,
}

async fn session(timing: LinkTiming) -> Served {
    session_with(timing, None).await
}

/// A session as `session`, with a door for the helper's own requests (`up`).
async fn session_with(timing: LinkTiming, up: Option<Arc<dyn UpDoor>>) -> Served {
    let (kept, news) = news_kept();
    let setup = LinkSetup {
        entry: "own-1".into(),
        url: "https://192.0.2.10:7781".into(),
        pin: HelperPin { sha256: Some("00".repeat(32)), public: None, since: String::new() },
        key: "dwl1_".into(),
        helper_id: "helper-1".into(),
        me: Arc::new(Me::default_for_tests),
        up,
    };
    let link = Link::idle(setup, timing, news);
    let (gateway, helper) = tokio::io::duplex(1 << 20);
    let config = Some(ws_config(FRAME_MAX));
    let gateway = WebSocketStream::from_raw_socket(gateway, Role::Client, config).await;
    let helper = WebSocketStream::from_raw_socket(helper, Role::Server, config).await;
    let (sink, stream) = tungstenite_wire(gateway);
    let session = Session::start(sink);
    let (serving, serve_link) = (session.clone(), link.clone());
    let end = tokio::spawn(async move { serving.serve(stream, &serve_link).await });
    Served { link, session, helper, news: kept, end }
}

/// The helper's next JSON frame.
async fn next_text(helper: &mut WebSocketStream<tokio::io::DuplexStream>) -> Value {
    loop {
        match helper.next().await.expect("a frame").expect("no error") {
            Message::Text(text) => return serde_json::from_str(&text).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("not a text: {other:?}"),
        }
    }
}

async fn text(helper: &mut WebSocketStream<tokio::io::DuplexStream>, value: Value) {
    helper.send(Message::Text(value.to_string().into())).await.unwrap();
}

async fn piece(helper: &mut WebSocketStream<tokio::io::DuplexStream>, id: u32, end: bool, data: &[u8]) {
    helper.send(Message::Binary(encode_chunk(id, end, data))).await.unwrap();
}

fn get(path: &str) -> Request {
    Request::builder().uri(path).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn a_session_carries_requests_answers_bodies_and_the_helpers_news() {
    let Served { link, session, mut helper, news: kept, .. } = session(LinkTiming::default()).await;
    // A request with a body: its head, its pieces, its end.
    let call = {
        let session = session.clone();
        tokio::spawn(async move {
            let request = Request::builder()
                .method("POST")
                .uri("/home/v1/asr/audio/transcriptions?x=1")
                .header("content-type", "audio/wav")
                .header("authorization", "Bearer the-bridge-token")
                .body(Body::from(vec![5u8; CHUNK + 10]))
                .unwrap();
            session.call(request).await
        })
    };
    let head = next_text(&mut helper).await;
    assert_eq!((head["t"].as_str(), head["method"].as_str()), (Some("req"), Some("POST")));
    assert_eq!(head["path"], "/home/v1/asr/audio/transcriptions?x=1");
    assert_eq!(head["headers"], json!({ "content-type": "audio/wav" }), "never the bearer");
    assert_eq!(head["body"], true);
    let id = head["id"].as_u64().unwrap() as u32;
    let mut received = Vec::new();
    loop {
        match helper.next().await.unwrap().unwrap() {
            Message::Binary(frame) => {
                let (at, end, data) = decode_chunk(&frame).unwrap();
                assert_eq!(at, id);
                received.extend_from_slice(&data);
                if end {
                    break;
                }
            }
            Message::Ping(_) => {}
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(received.len(), CHUNK + 10);
    // Its news first, a kind this gateway does not know passed by, then its answer.
    text(&mut helper, json!({ "t": "event", "name": "status", "data": status("helper-1") })).await;
    text(&mut helper, json!({ "t": "something-new", "id": id })).await;
    text(
        &mut helper,
        json!({ "t": "res", "id": id, "status": 200, "headers": { "content-type": "application/json" }, "body": true }),
    )
    .await;
    piece(&mut helper, id, false, b"{\"text\":").await;
    piece(&mut helper, id, true, b"\"yes\"}").await;
    let response = call.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/json");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"{\"text\":\"yes\"}");
    assert_eq!(*kept.lock().unwrap(), vec![LinkNews::Status(status("helper-1"))]);

    // A body broken midway reaches its reader as an error, never as a short whole one.
    let call = {
        let session = session.clone();
        tokio::spawn(async move { session.call(get("/home/v1/llm/models")).await })
    };
    let id = next_text(&mut helper).await["id"].as_u64().unwrap() as u32;
    text(&mut helper, json!({ "t": "res", "id": id, "status": 200, "headers": {}, "body": true })).await;
    piece(&mut helper, id, false, b"half").await;
    text(&mut helper, json!({ "t": "error", "id": id, "message": "the model stopped" })).await;
    let body = call.await.unwrap().unwrap().into_body().collect().await;
    assert!(body.is_err(), "an error, not a short body");

    // A caller that goes away: the helper hears `cancel`.
    let call = {
        let session = session.clone();
        tokio::spawn(async move { session.call(get("/home/v1/status")).await })
    };
    let id = next_text(&mut helper).await["id"].as_u64().unwrap();
    call.abort();
    let cancel = next_text(&mut helper).await;
    assert_eq!(cancel, json!({ "t": "cancel", "id": id }));

    // Another helper's status: the link ends, in words.
    text(&mut helper, json!({ "t": "event", "name": "status", "data": status("someone-else") })).await;
    assert!(tokio::time::timeout(Duration::from_secs(5), async {
        while !link.stopped() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok());
    assert!(matches!(link.state(), LinkState::Down { reason, .. } if reason == ANOTHER));
    // Stopped, it says nothing more: what it might still tell would land on its entry's next link.
    let told = kept.lock().unwrap().len();
    link.set_down(super::super::AWAY);
    link.set_status(status("helper-1"));
    assert_eq!(kept.lock().unwrap().len(), told);
}

/// A body still being sent when its answer came — the helper refused it — is ended at once with its last piece, so the
/// helper lets that body's place go now, not after its silence (review of block 1).
#[tokio::test]
async fn a_body_whose_answer_came_first_is_ended_at_once() {
    let Served { session, mut helper, .. } = session(LinkTiming::default()).await;
    let (more, pieces) = futures::channel::mpsc::unbounded::<Result<Bytes, std::io::Error>>();
    more.unbounded_send(Ok(Bytes::from_static(b"first"))).unwrap();
    let call = {
        let session = session.clone();
        let request = Request::builder().method("POST").uri("/home/v1/asr/models").body(Body::from_stream(pieces));
        tokio::spawn(async move { session.call(request.unwrap()).await })
    };
    let id = next_text(&mut helper).await["id"].as_u64().unwrap() as u32;
    match helper.next().await.unwrap().unwrap() {
        Message::Binary(frame) => assert_eq!(decode_chunk(&frame), Some((id, false, Bytes::from_static(b"first")))),
        other => panic!("{other:?}"),
    }
    text(&mut helper, json!({ "t": "res", "id": id, "status": 404, "headers": {}, "body": false })).await;
    assert_eq!(call.await.unwrap().unwrap().status(), StatusCode::NOT_FOUND);
    // The caller's body goes on: its next piece is not sent — its end is, at once.
    more.unbounded_send(Ok(Bytes::from_static(b"more"))).unwrap();
    match helper.next().await.unwrap().unwrap() {
        Message::Binary(frame) => assert_eq!(decode_chunk(&frame), Some((id, true, Bytes::new()))),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn bye_tells_the_helper_to_forget_this_home_and_ends_the_link() {
    let Served { link, mut helper, end, .. } = session(LinkTiming::default()).await;
    link.bye();
    assert_eq!(next_text(&mut helper).await, json!({ "t": "bye" }));
    match helper.next().await.unwrap().unwrap() {
        Message::Close(Some(frame)) => assert_eq!(u16::from(frame.code), DONE),
        other => panic!("{other:?}"),
    }
    assert_eq!(end.await.unwrap(), End::Stopped);
}

#[tokio::test(start_paused = true)]
async fn a_stream_whose_reader_stalls_is_cancelled_and_the_others_go_on() {
    let Served { session, mut helper, .. } = session(LinkTiming::default()).await;
    let start = |path: &'static str| {
        let session = session.clone();
        tokio::spawn(async move { session.call(get(path)).await })
    };
    let stalled = start("/home/v1/llm/models");
    let first = next_text(&mut helper).await["id"].as_u64().unwrap() as u32;
    let reader = start("/home/v1/asr/models");
    let second = next_text(&mut helper).await["id"].as_u64().unwrap() as u32;
    for id in [first, second] {
        text(&mut helper, json!({ "t": "res", "id": id, "status": 200, "headers": {}, "body": true })).await;
    }
    // The first's reader never reads: past 1 MiB its stream waits, five seconds later it is cancelled.
    let stalled = stalled.await.unwrap().unwrap();
    let reader = reader.await.unwrap().unwrap();
    for _ in 0..17 {
        piece(&mut helper, first, false, &vec![1u8; CHUNK]).await;
    }
    piece(&mut helper, second, false, b"the other").await;
    piece(&mut helper, second, true, b" goes on").await;
    assert_eq!(next_text(&mut helper).await, json!({ "t": "cancel", "id": first }));
    let body = reader.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"the other goes on");
    assert!(stalled.into_body().collect().await.is_err(), "the stalled one ends in an error");

    // Eight streams' full queues fill the link's 8 MiB: the newest stream is the one cancelled.
    let mut streams = Vec::new();
    for _ in 0..9 {
        let call = start("/home/v1/status");
        let id = next_text(&mut helper).await["id"].as_u64().unwrap() as u32;
        text(&mut helper, json!({ "t": "res", "id": id, "status": 200, "headers": {}, "body": true })).await;
        streams.push((id, call.await.unwrap().unwrap()));
    }
    for (id, _) in &streams[..8] {
        for _ in 0..16 {
            piece(&mut helper, *id, false, &vec![2u8; CHUNK]).await;
        }
    }
    let newest = streams[8].0;
    piece(&mut helper, newest, false, b"one more").await;
    assert_eq!(next_text(&mut helper).await, json!({ "t": "cancel", "id": newest }));
}

#[tokio::test(start_paused = true)]
async fn a_silent_helper_ends_the_link_and_a_ping_keeps_it() {
    let timing =
        LinkTiming { ping: Duration::from_secs(15), silence: Duration::from_secs(30), ..LinkTiming::default() };
    let Served { session, mut helper, end, .. } = session(timing).await;
    // A helper that reads answers the gateway's pings (every 15 s): a minute on, the link is still there.
    let mut pings = 0;
    let _ = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(Ok(message)) = helper.next().await {
            if matches!(message, Message::Ping(_)) {
                pings += 1;
            }
        }
    })
    .await;
    assert!(pings >= 3, "{pings} pings");
    assert!(!session.gone.is_cancelled(), "the pongs keep it");
    // A helper that says nothing for 30 s is gone: the gateway closes with 4408.
    let ended = tokio::time::timeout(Duration::from_secs(60), end).await.expect("the session ends").unwrap();
    assert_eq!(ended, End::Silent);
}

/// A way out over an in-memory socket of `room` bytes, and the peer's end.
async fn way_out(room: usize) -> (Out, WebSocketStream<tokio::io::DuplexStream>) {
    let (near, far) = tokio::io::duplex(room);
    let near = WebSocketStream::from_raw_socket(near, Role::Client, None).await;
    let far = WebSocketStream::from_raw_socket(far, Role::Server, None).await;
    let (sink, _) = tungstenite_wire(near);
    (Out::start(sink), far)
}

#[tokio::test(start_paused = true)]
async fn control_frames_wait_in_order_and_a_peer_that_reads_nothing_ends_the_link() {
    let numbered = |at: usize| Wire::json(&json!({ "t": "event", "name": "n", "data": at }));
    // A peer that reads: the frames past the writer's queue wait in the overflow, and all of them come, in order.
    let (out, mut far) = way_out(1 << 20).await;
    for at in 0..100 {
        out.control(numbered(at));
    }
    assert_eq!(out.waiting(), 100 - WRITER_QUEUE, "the queue full, the rest waiting");
    for at in 0..100 {
        assert_eq!(next_text(&mut far).await["data"], at);
    }
    assert!(!out.gone.is_cancelled());
    // A peer that reads nothing while frames keep coming: the queue and the overflow full, the next one ends the link —
    // no task waits for each.
    let (out, _far) = way_out(64).await;
    for at in 0..2 * WRITER_QUEUE {
        out.control(numbered(at));
    }
    assert!(!out.gone.is_cancelled());
    out.control(numbered(0));
    assert!(out.gone.is_cancelled(), "a flood");
    // A peer that stops reading: the overflow waits five seconds for room, then the link ends.
    let (out, _far) = way_out(64).await;
    for at in 0..WRITER_QUEUE + 10 {
        out.control(numbered(at));
    }
    tokio::time::sleep(STALL + Duration::from_secs(1)).await;
    assert!(out.gone.is_cancelled(), "a writer full for five seconds");
}

// ── Up: the helper's own requests at the gateway ──

/// A gateway's door for the test: open or not; a small router of its own — who asks, and a body echoed —; the person
/// as their role names them, nobody without one.
struct Door {
    open: std::sync::atomic::AtomicBool,
}

impl UpDoor for Door {
    fn open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    fn router(&self) -> Option<Router> {
        let who = |request: Request| async move {
            let origin = request.extensions().get::<UpOrigin>().cloned();
            axum::Json(json!({ "name": origin.map(|origin| origin.principal.name) }))
        };
        let echo = |body: Bytes| async move { body };
        Some(
            Router::new()
                .route("/api/v1/who", axum::routing::get(who))
                .route("/api/v1/echo", axum::routing::post(echo)),
        )
    }

    fn origin(&self, person: &Value) -> Option<UpOrigin> {
        let role = crate::core::identity::roles::Role::parse(person.get("role")?.as_str()?)?;
        let name = person.get("name").and_then(Value::as_str).unwrap_or("");
        let principal = crate::core::identity::principal::user_principal("u-1", name, role, None);
        Some(UpOrigin { entry: "own-1".into(), principal })
    }
}

/// A body of the helper's own request, as its pieces come to the helper: whole, up to its end.
async fn body_of(helper: &mut WebSocketStream<tokio::io::DuplexStream>, id: u32) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        match helper.next().await.expect("a frame").expect("no error") {
            Message::Binary(frame) => {
                let (at, end, data) = decode_chunk(&frame).unwrap();
                assert_eq!(at, id);
                body.extend_from_slice(&data);
                if end {
                    return body;
                }
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("not a piece: {other:?}"),
        }
    }
}

/// The helper's own requests at the gateway (stage 7 of docs/plans/home-setup-flow.md): run as the person they name,
/// their bodies and answers in pieces of the helper's own ids; refused outside the panel's API, without a person, and
/// while the door is shut; one in the gateway's ids breaks the protocol.
#[tokio::test]
async fn the_helpers_own_requests_run_as_the_person_they_name_and_only_where_the_door_lets_them() {
    let door = Arc::new(Door { open: std::sync::atomic::AtomicBool::new(true) });
    let Served { mut helper, end, .. } = session_with(LinkTiming::default(), Some(door.clone())).await;
    let mama = json!({ "name": "Мама", "role": "admin", "id": "u-1" });
    let up = |id: u32, method: &str, path: &str, person: &Value, body: bool| {
        json!({ "t": "up", "id": UP_BIT | id, "method": method, "path": path,
                "headers": { "content-type": "application/json", "authorization": "Bearer nobody" },
                "body": body, "person": person })
    };
    // A reading, as the person it names: its answer comes back as `up-res` and pieces of the same id.
    text(&mut helper, up(1, "GET", "/api/v1/who", &mama, false)).await;
    let head = next_text(&mut helper).await;
    assert_eq!(
        (head["t"].as_str(), head["id"].as_u64(), head["status"].as_u64()),
        (Some("up-res"), Some(u64::from(UP_BIT | 1)), Some(200)),
        "{head}"
    );
    let body: Value = serde_json::from_slice(&body_of(&mut helper, UP_BIT | 1).await).unwrap();
    assert_eq!(body, json!({ "name": "Мама" }));
    // A body in pieces of the helper's own ids reaches its handler whole.
    text(&mut helper, up(2, "POST", "/api/v1/echo", &mama, true)).await;
    piece(&mut helper, UP_BIT | 2, false, b"{\"say\":").await;
    piece(&mut helper, UP_BIT | 2, true, b"\"hi\"}").await;
    assert_eq!(next_text(&mut helper).await["status"], 200);
    assert_eq!(body_of(&mut helper, UP_BIT | 2).await, b"{\"say\":\"hi\"}");
    // Refused, no handler reached: a path that is not the panel's API, a change of the health, nobody named.
    let refusals = [
        (3, "GET", "/home/v1/status", &mama, 404),
        (4, "GET", "/station/h:kitchen/api/v1/who", &mama, 404),
        (5, "GET", "/api/v1/../home/v1/status", &mama, 404),
        (6, "POST", "/health", &mama, 404),
        (7, "GET", "/api/v1/who", &Value::Null, 403),
    ];
    for (id, method, path, person, status) in refusals {
        text(&mut helper, up(id, method, path, person, false)).await;
        let head = next_text(&mut helper).await;
        assert_eq!(
            (head["t"].as_str(), head["id"].as_u64(), head["status"].as_u64(), head["body"].as_bool()),
            (Some("up-res"), Some(u64::from(UP_BIT | id)), Some(status), Some(false)),
            "{path}"
        );
    }
    // The door shut (the entry is no Turbo at home now): refused at the next request.
    door.open.store(false, Ordering::SeqCst);
    text(&mut helper, up(8, "GET", "/api/v1/who", &mama, false)).await;
    assert_eq!(next_text(&mut helper).await["status"], 403);
    // An `up` in the gateway's own ids: a protocol error, the link closed.
    let theirs = json!({ "t": "up", "id": 9, "method": "GET", "path": "/api/v1/who", "headers": {}, "body": false });
    text(&mut helper, theirs).await;
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), end).await.unwrap().unwrap(), End::Protocol);
}

/// A gateway's ids stay below `UP_BIT`, the helper's own above it, whatever their count; the pieces of each are told
/// apart by their first byte.
#[test]
fn the_two_ways_number_their_streams_apart() {
    assert_eq!(Way::Req.id(UP_BIT + 5), 5, "the gateway's count wraps below the helper's");
    assert_eq!(Way::Up.id(5), UP_BIT | 5);
    assert!(up_piece(&encode_chunk(UP_BIT | 5, false, b"x")) && !up_piece(&encode_chunk(5, false, b"x")));
    let reading = Method::GET;
    for path in ["/api/v1/brain", "/api/v1/brain?x=/../", "/health"] {
        assert!(up_path(path, &reading), "{path}");
    }
    for path in [
        "/home/v1/status",
        "/station/h:k/api/v1/brain",
        "/api/v1/../debug",
        "/api/v1//brain",
        "/api/v1/%2e%2e/x",
        "api/v1/brain",
        "/debug/sessions",
        "/via/own-1/home/v1/status",
    ] {
        assert!(!up_path(path, &reading), "{path}");
    }
    assert!(!up_path("/health", &Method::POST), "the health is only read");
}

// ── The verifiers, over an in-memory TLS pair ──

#[cfg(feature = "pc")]
mod tls {
    use rustls::server::ServerSessionMemoryCache;
    use rustls::ServerConfig;

    use super::*;
    use crate::helpers::link_serve::TlsIdentity;

    /// A TLS handshake between these two over memory; whether it finished.
    async fn handshake(server: Arc<ServerConfig>, client: Arc<ClientConfig>, name: &str) -> bool {
        let (near, far) = tokio::io::duplex(64 * 1024);
        let accepted = tokio::spawn(async move { tokio_rustls::TlsAcceptor::from(server).accept(far).await.is_ok() });
        let name = ServerName::try_from(name.to_string()).unwrap();
        let connected = tokio_rustls::TlsConnector::from(client).connect(name, near).await.is_ok();
        let _ = accepted.await;
        connected
    }

    #[tokio::test]
    async fn a_pinned_verifier_takes_only_its_own_certificate() {
        let own = TlsIdentity::make().unwrap();
        let other = TlsIdentity::make().unwrap();
        let pinned = Pinned::new(own.fingerprint);
        assert!(
            handshake(
                own.server_config().unwrap(),
                client_config(Some(pinned.clone()), false).unwrap(),
                "helper.example.com"
            )
            .await
        );
        assert!(!pinned.foreign());
        // Another certificate made the same way, whatever name it is dialed by: refused, and said to be foreign.
        let pinned = Pinned::new(own.fingerprint);
        assert!(
            !handshake(
                other.server_config().unwrap(),
                client_config(Some(pinned.clone()), false).unwrap(),
                "192.0.2.10"
            )
            .await
        );
        assert!(pinned.foreign());
    }

    #[tokio::test]
    async fn two_pairings_through_one_verifier_both_record_the_certificate() {
        // A server that offers tickets and keeps sessions: a resumed handshake would skip the verifier.
        let identity = TlsIdentity::make().unwrap();
        let mut server = (*identity.server_config().unwrap()).clone();
        server.send_tls13_tickets = 2;
        server.session_storage = ServerSessionMemoryCache::new(16);
        let server = Arc::new(server);
        let meeting = FirstMeeting::new();
        let client = client_config(Some(meeting.clone()), true).unwrap();
        for _ in 0..2 {
            *locked(&meeting.seen) = None;
            assert!(handshake(server.clone(), client.clone(), "helper.example.com").await);
            let seen = meeting.seen().expect("recorded at each handshake");
            assert_eq!(
                seen,
                Seen { fingerprint: identity.fingerprint, public: false },
                "a self-signed one has no public chain"
            );
        }
    }

    /// A TLS server that answers every upgrade with the next status of `answers`, and counts the tries.
    async fn answering(answers: &'static [u16]) -> (String, [u8; 32], Arc<AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let identity = TlsIdentity::make().unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(identity.server_config().unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let tries = Arc::new(AtomicUsize::new(0));
        let counted = tries.clone();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let Ok(mut tls) = acceptor.accept(tcp).await else { continue };
                let at = counted.fetch_add(1, Ordering::SeqCst);
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let Ok(read) = tls.read(&mut buffer).await else { break };
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let status = answers[at.min(answers.len() - 1)];
                // A helper of another version says its own with its 426.
                let version = if status == 426 { format!("{VERSION_HEADER}: 1\r\n") } else { String::new() };
                let answer = format!("HTTP/1.1 {status} No\r\n{version}content-length: 0\r\nconnection: close\r\n\r\n");
                let _ = tls.write_all(answer.as_bytes()).await;
                let _ = tls.shutdown().await;
            }
        });
        (url, identity.fingerprint, tries)
    }

    /// A link to `url`, pinned to `fingerprint`, retried every 10 ms.
    fn quick_link(url: String, fingerprint: [u8; 32]) -> Arc<Link> {
        let (_, news) = news_kept();
        let setup = LinkSetup {
            entry: "own-1".into(),
            url,
            pin: HelperPin { sha256: Some(hex::encode(fingerprint)), public: None, since: String::new() },
            key: format!("dwl1_0123456789ab_{}", "a".repeat(64)),
            helper_id: "helper-1".into(),
            me: Arc::new(Me::default_for_tests),
            up: None,
        };
        let timing = LinkTiming { pauses: vec![Duration::from_millis(10)], ..LinkTiming::default() };
        Link::start(setup, timing, news)
    }

    /// Waits until the link is down for `reason`.
    async fn down_for(link: &Link, reason: &str) -> bool {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !matches!(link.state(), LinkState::Down { reason: ref now, .. } if now == reason) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    }

    /// A helper of another link version: the link says so in words and keeps trying — updating either side fixes it —;
    /// a pairing says so in its own words.
    #[tokio::test]
    async fn a_helper_of_another_version_is_said_so_and_tried_again() {
        let (url, fingerprint, tries) = answering(&[426]).await;
        let link = quick_link(url.clone(), fingerprint);
        assert!(down_for(&link, OTHER_VERSION).await, "{:?}", link.state());
        let before = tries.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(tries.load(Ordering::SeqCst) > before, "tried again");
        link.stop();
        let refuse = |_: &HelperIdentity, _: &HelperPin| -> Result<(), crate::platform::http::ApiError> { Ok(()) };
        let me = Me::default_for_tests();
        let paired = pair(&url, &ConnectionKey::new(), &me, "", &LinkTiming::default(), true, &refuse).await;
        let error = paired.unwrap_err();
        assert!(matches!(error, PairError::Version), "{error:?}");
        assert_eq!(error.into_api(&url).message, "Турбо другой версии DuoWave — обновите оба.");
    }

    /// A TLS helper that upgrades every link, sends its challenge, reads the proof and closes with `code`.
    async fn challenging(code: u16) -> (String, [u8; 32], Arc<AtomicUsize>) {
        let identity = TlsIdentity::make().unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(identity.server_config().unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let tries = Arc::new(AtomicUsize::new(0));
        let counted = tries.clone();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let Ok(tls) = acceptor.accept(tcp).await else { continue };
                counted.fetch_add(1, Ordering::SeqCst);
                let Ok(mut socket) = tokio_tungstenite::accept_async(tls).await else { continue };
                let challenge = json!({ "t": "challenge", "nonce": b64(&[3u8; 32]) });
                let _ = socket.send(Message::Text(challenge.to_string().into())).await;
                let _ = socket.next().await;
                let frame = CloseFrame { code: CloseCode::from(code), reason: "".into() };
                let _ = socket.send(Message::Close(Some(frame))).await;
            }
        });
        (url, identity.fingerprint, tries)
    }

    /// A proof the helper found late or missing (`SILENT`, a station on a poor network) is tried again; only a proof
    /// it refused (4401) ends the tries (review of block 1).
    #[tokio::test]
    async fn a_late_proof_is_tried_again_and_a_refused_one_no_more() {
        let (url, fingerprint, tries) = challenging(SILENT).await;
        let link = quick_link(url, fingerprint);
        assert!(down_for(&link, super::super::super::AWAY).await, "{:?}", link.state());
        let until = tokio::time::Instant::now() + Duration::from_secs(10);
        while tries.load(Ordering::SeqCst) < 3 && tokio::time::Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(tries.load(Ordering::SeqCst) >= 3, "tried again");
        assert!(!matches!(link.state(), LinkState::Down { ref reason, .. } if reason == REFUSED_WORDS));
        link.stop();

        let (url, fingerprint, tries) = challenging(REFUSED).await;
        let link = quick_link(url, fingerprint);
        assert!(down_for(&link, REFUSED_WORDS).await, "{:?}", link.state());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(tries.load(Ordering::SeqCst), 1, "no more tries");
    }

    #[tokio::test]
    async fn a_busy_helper_is_tried_again_and_a_refusing_one_no_more() {
        // Busy, then a 403 of something between (a proxy's rule: the helper itself never answers a gateway so), then
        // the helper's own refusal.
        let (url, fingerprint, tries) = answering(&[429, 403, 401]).await;
        let (kept, news) = news_kept();
        let setup = LinkSetup {
            entry: "own-1".into(),
            url,
            pin: HelperPin { sha256: Some(hex::encode(fingerprint)), public: None, since: String::new() },
            key: format!("dwl1_0123456789ab_{}", "a".repeat(64)),
            helper_id: "helper-1".into(),
            me: Arc::new(Me::default_for_tests),
            up: None,
        };
        let timing = LinkTiming { pauses: vec![Duration::from_millis(10)], ..LinkTiming::default() };
        let link = Link::start(setup, timing, news);
        let refused = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if matches!(link.state(), LinkState::Down { ref reason, .. } if *reason == REFUSED_WORDS) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(refused.is_ok(), "{:?}", link.state());
        assert_eq!(tries.load(Ordering::SeqCst), 3, "the busy one and the 403 tried again, the refusal not");
        // The refused link stays as it is — down, in words — and is not started again: its key would be refused again.
        assert!(!link.stopped());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(tries.load(Ordering::SeqCst), 3);
        let downs = kept.lock().unwrap().iter().filter(|news| matches!(news, LinkNews::Down(_))).count();
        assert!(downs >= 1);
    }
}

// ── The bridge ──

#[test]
fn the_bridge_reads_its_inner_path_as_it_came() {
    assert_eq!(
        inner_of("/via/own-1/home/v1/plugins/a%3Fb/call?x=%2F"),
        Some(("own-1".to_string(), "/home/v1/plugins/a%3Fb/call?x=%2F".to_string())),
        "an escaped ? stays in the path, the query as it came"
    );
    assert_eq!(inner_of("/via/own-1/home/v1/link"), None);
    assert_eq!(inner_of("/via/own-1/home/v1/link/more"), None);
    assert_eq!(inner_of("/via/own-1/api/v1/helpers"), None);
    assert_eq!(inner_of("/via/own-1/home/v1"), None);
    assert_eq!(inner_of("/via//home/v1/status"), None);
    assert_eq!(via_base(7780, "own-1"), "http://127.0.0.1:7780/via/own-1");
}

#[tokio::test]
async fn the_bridge_takes_only_its_token_from_this_computer() {
    use crate::config::{Config, Env};
    use crate::settings::definitions::{HELPERS, HELPER_PINS};
    let env = Env::from_pairs([("STATE_DIR", ""), ("DUOWAVE_ROLE", "station")]);
    let app = crate::app::App::build(Config::for_tests(&env)).unwrap();
    // A linked helper nobody answers for: its link runs and stays down.
    let entry = crate::helpers::HelperSetting {
        id: "own-1".into(),
        name: "Сервер".into(),
        url: "https://127.0.0.1:9".into(),
        kind: crate::helpers::HelperKind::Own,
        enabled: true,
        without: Vec::new(),
        helper_id: Some("helper-1".into()),
    };
    let link_key = format!("dwl1_0123456789ab_{}", "a".repeat(64));
    app.secrets.set("helper:own-1:key", Some(&link_key)).unwrap();
    let pin = HelperPin { sha256: Some("00".repeat(32)), public: None, since: String::new() };
    app.settings.set(&HELPER_PINS, &std::collections::BTreeMap::from([("own-1".to_string(), pin)]), None).unwrap();
    app.settings.set(&HELPERS, &vec![entry], None).unwrap();
    app.helpers.start();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(app.helpers.link_of("own-1").is_none(), "no link before the server listens: its bridge would not answer");
    app.helpers.set_local_port(9);
    let listed = tokio::time::timeout(Duration::from_secs(5), async {
        while app.helpers.link_of("own-1").is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    assert!(listed.await.is_ok(), "the link runs");
    let token = app.helpers.local().token().to_string();
    let ask = |uri: &str, from: &str, headers: &[(&str, &str)]| {
        let mut request = Request::builder().uri(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let mut request = request.body(Body::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo(std::net::SocketAddr::new(from.parse().unwrap(), 5000)));
        let app = app.clone();
        async move {
            let response = crate::app::router(app).oneshot(request).await.unwrap();
            let status = response.status();
            let body = response.into_body().collect().await.unwrap().to_bytes();
            (status, String::from_utf8_lossy(&body).to_string())
        }
    };
    let bearer = format!("Bearer {token}");
    let status = "/via/own-1/home/v1/status";
    assert_eq!(
        ask(status, "127.0.0.1", &[("authorization", "Bearer wrong")]).await,
        (StatusCode::UNAUTHORIZED, String::new())
    );
    let with_link_key = format!("Bearer {link_key}");
    assert_eq!(ask(status, "127.0.0.1", &[("authorization", &with_link_key)]).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        ask(status, "127.0.0.1", &[("authorization", &bearer), ("origin", "http://a.example")]).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ask(status, "127.0.0.1", &[("authorization", &bearer), ("x-forwarded-for", "127.0.0.1")]).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ask(status, "192.168.1.20", &[("authorization", &bearer)]).await.0,
        StatusCode::FORBIDDEN,
        "this computer only"
    );
    assert_eq!(
        ask("/via/own-2/home/v1/status", "127.0.0.1", &[("authorization", &bearer)]).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        ask("/via/own-1/home/v1/link", "127.0.0.1", &[("authorization", &bearer)]).await.0,
        StatusCode::NOT_FOUND
    );
    let (code, words) = ask(status, "127.0.0.1", &[("authorization", &bearer)]).await;
    assert_eq!((code, words.as_str()), (StatusCode::SERVICE_UNAVAILABLE, crate::helpers::AWAY), "the link is not up");
    app.helpers.stop();
}

#[tokio::test]
async fn a_link_never_goes_plain() {
    use crate::config::{Config, Env};
    use crate::helpers::remote::RemoteHelper;
    use crate::helpers::{HelperKind, HelperSetting, HelperState};
    let app = crate::app::App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
    // Where the helper would be: a listener that counts who comes.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let came = Arc::new(AtomicUsize::new(0));
    {
        let came = came.clone();
        tokio::spawn(async move {
            while listener.accept().await.is_ok() {
                came.fetch_add(1, Ordering::SeqCst);
            }
        });
    }
    let link_key = format!("dwl1_0123456789ab_{}", "a".repeat(64));
    let pin = HelperPin { sha256: Some("00".repeat(32)), public: None, since: String::new() };
    let entry = |url: String| HelperSetting {
        id: "own-1".into(),
        name: "Сервер".into(),
        url,
        kind: HelperKind::Own,
        enabled: true,
        without: Vec::new(),
        helper_id: Some("helper-1".into()),
    };
    let reach = |url: String, pin: Option<HelperPin>| {
        RemoteHelper::reached(
            &entry(url),
            link_key.clone(),
            pin,
            &app.config,
            app.identity.clone(),
            app.helpers.local(),
        )
    };
    // A pinned entry at a plain address: never dialed, down in words — the key goes nowhere.
    let plain = reach(format!("http://127.0.0.1:{port}"), Some(pin));
    let (_, news) = news_kept();
    plain.run_link(LinkTiming::default(), news, None);
    assert!(plain.link().is_none());
    assert_eq!((plain.state(), plain.error()), (HelperState::Down, Some(REFUSED_WORDS.to_string())));
    // A link key without a pin: no plain look sends it, at whatever address.
    for url in [format!("https://127.0.0.1:{port}"), format!("http://127.0.0.1:{port}")] {
        assert_eq!(reach(url, None).look().await.unwrap_err(), REFUSED_WORDS);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(came.load(Ordering::SeqCst), 0, "nothing dialed");
}
