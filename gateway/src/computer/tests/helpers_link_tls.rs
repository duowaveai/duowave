//! The computer's part of whole_tests/helpers_link.rs: what only a computer has, tested with the whole gateway built.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

use crate::helpers::contract::HelperIdentity;
use crate::helpers::link::*;
use crate::helpers::remote::Me;
use crate::settings::definitions::HelperPin;
use crate::whole_tests::helpers_link::*;

mod tls {
    use rustls::pki_types::ServerName;
    use rustls::server::ServerSessionMemoryCache;
    use rustls::ClientConfig;
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
        assert!(down_for(&link, crate::helpers::AWAY).await, "{:?}", link.state());
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
