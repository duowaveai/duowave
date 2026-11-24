//! The link end to end (docs/plans/home-and-helpers.md, block 1, stage 7): «в тесте шлюз подключается к помощнику по
//! `wss` с самоподписанным сертификатом; подменённый сертификат — отказ; неверный ключ — отказ без подсказки; слух,
//! голос и поток модели идут по связи; после обрыва — переподключение по паузам». The computer is a computer's build
//! that helps (its link port a free one, LINK_PORT=0 in the support's base); the station is a computer's build playing
//! it (DUOWAVE_ROLE=station). Their servers, a switchable relay and an impostor with a certificate of its own are
//! stand-ins on 127.0.0.1 in this file.

use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::routing::post;
use duowave::helpers::contract::{Ability, HelperIdentity};
use duowave::helpers::link::{
    link_key_hash, link_key_id, link_proof, pair_over, Binding, ConnectionKey, LinkState, LinkTiming,
};
use duowave::helpers::remote::Me;
use duowave::helpers::Giver;
use futures::StreamExt;
use reqwest::header::HeaderName;
use reqwest::StatusCode;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::support::{bearer, message, send, until, Gateway};

const PEER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
/// The words of a refused connection key.
const REFUSED_KEY: &str = "Турбо не приняло ключ подключения";

type Auth = (HeaderName, String);

/// The first admin's header.
async fn admin(gateway: &Gateway) -> Auth {
    let setup = gateway.post("/api/v1/auth/setup").json(&json!({"password": "link-admin-password"}));
    bearer(message(setup, StatusCode::CREATED).await["token"].as_str().expect("a token"))
}

/// A server of the test on 127.0.0.1: its base URL.
async fn serve(router: axum::Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await });
    format!("http://{address}")
}

/// The computer's own servers: its recognition (it hears «включи свет»), its Piper voice (a WAV), and a model whose
/// stream gives its first word, then holds until `release`.
struct Servers {
    asr: String,
    tts: String,
    model: String,
    release: Arc<Notify>,
}

async fn servers() -> Servers {
    let asr = serve(
        axum::Router::new()
            .route("/v1/audio/transcriptions", post(|| async { axum::Json(json!({ "text": "включи свет" })) })),
    )
    .await;
    use duowave::speech::audio::pcm::{encode_wav, resample};
    let wav = encode_wav(&resample(&duowave::speech::test_audio::voice(300.0, -20.0), 16000, 22050), 22050);
    let tts = serve(axum::Router::new().route(
        "/v1/audio/speech",
        post(move || {
            let wav = wav.clone();
            async move { ([("content-type", "audio/wav")], wav) }
        }),
    ))
    .await;
    let release = Arc::new(Notify::new());
    let held = release.clone();
    let model = serve(axum::Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let held = held.clone();
            async move {
                let delta = |text: &str| json!({ "choices": [{ "delta": { "content": text } }] });
                let first = Bytes::from(format!("data: {}\n\n", delta("Первое")));
                let rest = Bytes::from(format!("data: {}\n\ndata: [DONE]\n\n", delta(" второе")));
                let stream = futures::stream::once(async move { Ok::<_, io::Error>(first) }).chain(
                    futures::stream::once(async move {
                        held.notified().await;
                        Ok::<_, io::Error>(rest)
                    }),
                );
                ([("content-type", "text/event-stream")], Body::from_stream(stream))
            }
        }),
    ))
    .await;
    Servers { asr: format!("{asr}/v1"), tts: format!("{tts}/v1"), model: format!("{model}/v1"), release }
}

/// The computer that helps: its recognition, its Piper voice at home, its model among «Мозг»'s rows; its admin.
async fn computer(servers: &Servers, more: &[(&str, &str)]) -> (Gateway, Auth) {
    let mut vars = vec![
        ("ASR_PROVIDER", "openai"),
        ("ASR_BASE_URL", servers.asr.as_str()),
        ("TTS_OWN_URL", servers.tts.as_str()),
        ("TTS_OWN_MODEL", "piper"),
        ("TTS_OWN_VOICE", "ru_RU-irina-medium"),
        ("PEER_KEY", PEER),
    ];
    vars.extend_from_slice(more);
    let pc = Gateway::start(&vars).await;
    let model = servers.model.clone();
    pc.app
        .db
        .with(|c| {
            c.execute(
                "INSERT INTO llm_providers (id, preset, kind, title, base_url, created_at, updated_at)
                 VALUES ('stand', 'custom-openai', 'openai', 'Модель', ?1, ?2, ?2)",
                [model.as_str(), "2026-11-15T10:00:00.000Z"],
            )
        })
        .unwrap();
    let auth = admin(&pc).await;
    assert!(until(5000, || pc.link_port().is_some()).await, "the link port is bound");
    (pc, auth)
}

/// A station's gateway and its admin.
async fn station() -> (Gateway, Auth) {
    let station = Gateway::start(&[("DUOWAVE_ROLE", "station"), ("PEER_KEY", PEER)]).await;
    let auth = admin(&station).await;
    (station, auth)
}

/// The computer's connection key, as its admin opens «Подключить шлюз».
async fn connection_key(pc: &Gateway, (header, auth): &Auth) -> String {
    let sheet = message(pc.get("/api/v1/helpers/key").header(header.clone(), auth.clone()), StatusCode::OK).await;
    sheet["key"].as_str().expect("a key").to_string()
}

/// `POST /api/v1/helpers` on the station.
async fn add(station: &Gateway, (header, auth): &Auth, body: Value) -> (StatusCode, Value) {
    send(station.post("/api/v1/helpers").header(header.clone(), auth.clone()).json(&body)).await
}

/// The station pairs with the computer at `url`: the entry's id.
async fn paired(station: &Gateway, auth: &Auth, url: &str, key: &str) -> String {
    let (status, body) = add(station, auth, json!({ "url": url, "key": key })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["message"]["id"].as_str().expect("the entry's id").to_string()
}

fn link_state(gateway: &Gateway, entry: &str) -> Option<LinkState> {
    gateway.app.helpers.link_of(entry).map(|link| link.state())
}

fn up(gateway: &Gateway, entry: &str) -> bool {
    matches!(link_state(gateway, entry), Some(LinkState::Up { .. }))
        && gateway.app.helpers.link_of(entry).is_some_and(|link| link.connected())
}

/// «Спросить мозг» on a gateway: the events of its answer, as the panel reads them.
async fn ask(gateway: &Gateway, (header, auth): &Auth, question: &str) -> Vec<Value> {
    let asked =
        gateway.post("/api/v1/brain/ask").header(header.clone(), auth.clone()).json(&json!({"question": question}));
    let response = asked.send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect()
}

/// The station's recognition now: what it hears.
async fn hears(gateway: &Gateway) -> String {
    let recognizer = gateway.app.speech.recognizer().expect("the station's chain");
    recognizer
        .recognize(&[0i16; 1600], 16000, &CancellationToken::new())
        .await
        .unwrap_or_else(|error| format!("{error}"))
}

/// The link's times for a test: a hundredth of the real pauses, the rest short.
fn quick() -> LinkTiming {
    LinkTiming {
        connect: Duration::from_secs(2),
        handshake: Duration::from_secs(2),
        ping: Duration::from_millis(100),
        silence: Duration::from_millis(600),
        pauses: [10, 20, 50, 100, 300, 600].into_iter().map(Duration::from_millis).collect(),
        steady: Duration::from_millis(200),
    }
}

// ── A relay and an impostor ──

/// A TCP relay to a port of 127.0.0.1, switchable: cut (its connections closed, new ones closed at once), opened
/// again, pointed elsewhere. It notes when each connection reached it.
struct Relay {
    port: u16,
    target: Arc<Mutex<u16>>,
    open: Arc<AtomicBool>,
    tries: Arc<Mutex<Vec<Instant>>>,
    cut: Arc<Mutex<CancellationToken>>,
}

impl Relay {
    async fn to(target: u16) -> Relay {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay = Relay {
            port: listener.local_addr().unwrap().port(),
            target: Arc::new(Mutex::new(target)),
            open: Arc::new(AtomicBool::new(true)),
            tries: Arc::default(),
            cut: Arc::new(Mutex::new(CancellationToken::new())),
        };
        let (target, open, tries, cut) =
            (relay.target.clone(), relay.open.clone(), relay.tries.clone(), relay.cut.clone());
        tokio::spawn(async move {
            while let Ok((mut inbound, _)) = listener.accept().await {
                tries.lock().unwrap().push(Instant::now());
                if !open.load(Ordering::SeqCst) {
                    continue;
                }
                let (target, cut) = (*target.lock().unwrap(), cut.lock().unwrap().clone());
                tokio::spawn(async move {
                    let Ok(mut outbound) = TcpStream::connect(("127.0.0.1", target)).await else { return };
                    tokio::select! {
                        _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound) => {}
                        _ = cut.cancelled() => {}
                    }
                });
            }
        });
        relay
    }

    fn url(&self) -> String {
        format!("https://127.0.0.1:{}", self.port)
    }

    /// Its connections closed, new ones closed at once.
    fn cut(&self) {
        self.open.store(false, Ordering::SeqCst);
        let old = std::mem::replace(&mut *self.cut.lock().unwrap(), CancellationToken::new());
        old.cancel();
    }

    fn open(&self) {
        self.open.store(true, Ordering::SeqCst);
    }

    fn point(&self, target: u16) {
        *self.target.lock().unwrap() = target;
    }

    fn tries(&self) -> Vec<Instant> {
        self.tries.lock().unwrap().clone()
    }
}

/// Any certificate: the impostor's own way on to the computer.
#[derive(Debug)]
struct AnyCertificate;

impl ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = rustls::crypto::ring::default_provider().signature_verification_algorithms;
        rustls::crypto::verify_tls12_signature(message, cert, dss, &algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = rustls::crypto::ring::default_provider().signature_verification_algorithms;
        rustls::crypto::verify_tls13_signature(message, cert, dss, &algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
    }
}

/// A TLS client that takes any certificate.
fn trusting() -> tokio_rustls::TlsConnector {
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCertificate))
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    tokio_rustls::TlsConnector::from(Arc::new(config))
}

/// A man in the middle: TLS ended with a certificate of its own (made as the helper makes its own), the bytes handed
/// on to `target` — over TLS again (the computer's link port) or plain (its main port, as a proxy hands the link on).
/// It notes whether a link's upgrade (its key's id) ever went through it, and whether the helper's challenge came back
/// through it (the helper took the pairing as far as the proofs).
struct Impostor {
    port: u16,
    saw_key: Arc<AtomicBool>,
    saw_challenge: Arc<AtomicBool>,
    reached: Arc<AtomicUsize>,
}

/// One way of a pipe: the bytes handed on, `seen` set once they hold `word`.
async fn hand_on<R, W>(mut from: R, mut to: W, word: &'static str, seen: Arc<AtomicBool>)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0u8; 16 * 1024];
    while let Ok(read) = from.read(&mut buffer).await {
        if read == 0 {
            break;
        }
        if String::from_utf8_lossy(&buffer[..read]).to_ascii_lowercase().contains(word) {
            seen.store(true, Ordering::SeqCst);
        }
        if to.write_all(&buffer[..read]).await.is_err() {
            break;
        }
    }
}

async fn pipe<A, B>(a: A, b: B, saw_key: Arc<AtomicBool>, saw_challenge: Arc<AtomicBool>)
where
    A: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    B: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (a_read, a_write) = tokio::io::split(a);
    let (b_read, b_write) = tokio::io::split(b);
    tokio::join!(
        hand_on(a_read, b_write, "x-duowave-link-id", saw_key),
        hand_on(b_read, a_write, "\"challenge\"", saw_challenge)
    );
}

impl Impostor {
    async fn to(target: u16, tls_upstream: bool) -> Impostor {
        let identity = duowave::helpers::link_serve::TlsIdentity::make().unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(identity.server_config().unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let impostor = Impostor {
            port: listener.local_addr().unwrap().port(),
            saw_key: Arc::default(),
            saw_challenge: Arc::default(),
            reached: Arc::default(),
        };
        let (saw_key, saw_challenge, reached) =
            (impostor.saw_key.clone(), impostor.saw_challenge.clone(), impostor.reached.clone());
        tokio::spawn(async move {
            while let Ok((inbound, _)) = listener.accept().await {
                reached.fetch_add(1, Ordering::SeqCst);
                let (acceptor, saw_key, saw_challenge) = (acceptor.clone(), saw_key.clone(), saw_challenge.clone());
                tokio::spawn(async move {
                    let Ok(inbound) = acceptor.accept(inbound).await else { return };
                    let Ok(outbound) = TcpStream::connect(("127.0.0.1", target)).await else { return };
                    if tls_upstream {
                        let name = ServerName::IpAddress("127.0.0.1".parse::<std::net::IpAddr>().unwrap().into());
                        let Ok(outbound) = trusting().connect(name, outbound).await else { return };
                        pipe(inbound, outbound, saw_key, saw_challenge).await;
                    } else {
                        pipe(inbound, outbound, saw_key, saw_challenge).await;
                    }
                });
            }
        });
        impostor
    }

    fn url(&self) -> String {
        format!("https://127.0.0.1:{}", self.port)
    }
}

/// A link's next JSON frame, or the close that came instead (its code, no body).
async fn next_json<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
) -> Result<Value, (u16, Vec<u8>)> {
    use tokio_tungstenite::tungstenite::Message;
    loop {
        match tokio::time::timeout(Duration::from_secs(5), socket.next()).await.expect("a frame in time") {
            Some(Ok(Message::Text(text))) => return Ok(serde_json::from_str(&text).unwrap()),
            Some(Ok(Message::Close(frame))) => {
                return Err((frame.map_or(1005, |frame| u16::from(frame.code)), Vec::new()))
            }
            Some(Ok(_)) => continue,
            other => panic!("{other:?}"),
        }
    }
}

/// A later link's socket, as a gateway opened it — the key's id, `gw-1` of `home-1` —, its proof of the key over the
/// helper's challenge and `binding`: the welcome's kind; else the status and the body of the refusal at the door, or
/// the close code after the proof (no body).
async fn prove_link<S: AsyncRead + AsyncWrite + Unpin>(
    socket: Result<tokio_tungstenite::WebSocketStream<S>, (u16, Vec<u8>)>,
    key: &str,
    binding: &Binding,
) -> Result<String, (u16, Vec<u8>)> {
    use base64::Engine;
    use futures::SinkExt;
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut socket = socket?;
    let challenge = next_json(&mut socket).await?;
    let nonce: [u8; 32] = b64.decode(challenge["nonce"].as_str().unwrap()).unwrap().try_into().unwrap();
    let id = link_key_id(key).unwrap_or(key);
    let mac = link_proof(&link_key_hash(key), &nonce, binding, id, "gw-1", "home-1");
    let proof = json!({ "t": "proof", "mac": b64.encode(mac) });
    socket.send(tokio_tungstenite::tungstenite::Message::Text(proof.to_string().into())).await.unwrap();
    let welcome = next_json(&mut socket).await?;
    Ok(welcome["t"].as_str().unwrap_or_default().to_string())
}

/// The headers of a later link's upgrade for this key.
fn link_headers(key: &str) -> Vec<(&'static str, String)> {
    let id = link_key_id(key).unwrap_or(key).to_string();
    vec![
        ("x-duowave-link-id", id),
        ("x-duowave-link-version", duowave::helpers::link::VERSION.to_string()),
        ("x-duowave-gateway", "gw-1".into()),
        ("x-duowave-home", "home-1".into()),
    ]
}

/// A later link to the computer's link port, pinned to `pin` (`prove_link`).
async fn link_with(port: u16, key: &str, pin: [u8; 32]) -> Result<String, (u16, Vec<u8>)> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let name = ServerName::IpAddress("127.0.0.1".parse::<std::net::IpAddr>().unwrap().into());
    let tls = trusting().connect(name, tcp).await.unwrap();
    let mut request = format!("wss://127.0.0.1:{port}/home/v1/link").into_client_request().unwrap();
    for (name, value) in link_headers(key) {
        request.headers_mut().insert(name, value.parse().unwrap());
    }
    let socket = match tokio_tungstenite::client_async(request, tls).await {
        Ok((socket, _)) => Ok(socket),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            Err((response.status().as_u16(), response.body().clone().unwrap_or_default()))
        }
        Err(error) => panic!("the upgrade: {error}"),
    };
    prove_link(socket, key, &Binding::Tls(pin)).await
}

// ── The tests ──

#[tokio::test]
async fn the_station_pairs_with_the_computer_over_wss_and_takes_hearing_voice_and_the_models_stream_over_the_link() {
    let servers = servers().await;
    let (pc, pc_admin) = computer(&servers, &[]).await;
    let (station, station_admin) = station().await;
    let key = connection_key(&pc, &pc_admin).await;
    let url = format!("https://127.0.0.1:{}", pc.link_port().unwrap());
    let (status, body) = add(&station, &station_admin, json!({ "url": url, "key": key })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let entry = body["message"]["id"].as_str().unwrap().to_string();
    let row = body["message"]["helpers"].as_array().unwrap().iter().find(|row| row["id"] == entry.as_str()).unwrap();
    let abilities: Vec<&str> =
        row["abilities"].as_array().unwrap().iter().filter_map(|ability| ability["id"].as_str()).collect();
    assert_eq!(abilities, ["hearing", "voice", "model"], "another home: the muscles the computer has: {row}");
    assert_eq!(row["link"]["pin"], format!("sha256:{}", pc.app.pc.link.fingerprint().unwrap()), "{row}");
    // Pinned: the computer's certificate, its self-signed one.
    let pin = station.app.helpers.pin(&entry).expect("a pin");
    assert_eq!(pin.sha256, pc.app.pc.link.fingerprint());
    assert!(until(5000, || up(&station, &entry)).await, "{:?}", link_state(&station, &entry));

    // Hearing over the link.
    assert_eq!(hears(&station).await, "включи свет");
    // The voice: the computer's Piper voice, as Ogg/Opus.
    station.app.voice.use_home().unwrap();
    let speaker = station.app.voice.speaker().expect("our voice through the helper");
    assert!(speaker.base_url().contains(&format!("/via/{entry}/")), "{}", speaker.base_url());
    let spoken = speaker.synthesize("Привет.", &CancellationToken::new()).await.unwrap();
    assert_eq!(&spoken.audio[..4], b"OggS");
    // The model's stream: its first word while the model holds the second, then the rest.
    let token = station.app.helpers.local().token().to_string();
    let question =
        json!({ "model": "stand", "stream": true, "messages": [{ "role": "user", "content": "Как дела?" }] });
    let response = station
        .post(&format!("/via/{entry}/home/v1/llm/chat/completions"))
        .bearer_auth(&token)
        .json(&question)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut stream = response.bytes_stream();
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next()).await.expect("the first word").unwrap();
    let first = String::from_utf8_lossy(&first.unwrap()).to_string();
    assert!(first.contains("Первое") && !first.contains("[DONE]"), "{first}");
    servers.release.notify_one();
    let mut rest = String::new();
    while let Some(piece) = tokio::time::timeout(Duration::from_secs(5), stream.next()).await.unwrap() {
        rest.push_str(&String::from_utf8_lossy(&piece.unwrap()));
    }
    assert!(rest.contains("второе") && rest.ends_with("data: [DONE]\n\n"), "{rest}");

    // «Мозг» (stage 4): the linked helper's model has its row, which stores neither an address nor a key — they are
    // this start's bridge and its token —, and a question asked through «Мозг» is answered by the computer's model,
    // over the link.
    let brain_row = || {
        let rows = station.app.brain.view().providers;
        rows.into_iter().find(|row| row.helper.as_ref().is_some_and(|helper| helper.id == entry))
    };
    assert!(until(5000, || brain_row().is_some()).await, "the linked helper's model has its row");
    let brain_row = brain_row().unwrap();
    assert_eq!(
        (brain_row.id.as_str(), brain_row.preset.as_str(), brain_row.base_url.as_deref()),
        (format!("helper-{entry}").as_str(), "helper", None)
    );
    let stored: Option<String> = station
        .app
        .db
        .with(|c| {
            c.query_row("SELECT base_url FROM llm_providers WHERE id = ?1", [brain_row.id.as_str()], |row| row.get(0))
        })
        .unwrap();
    // Only the stand-in for none: an older version taken back to would read an empty address as OpenAI's.
    assert_eq!(stored.as_deref(), Some(duowave::dialog::brain::presets::HELPER_NO_ADDRESS), "no address of its own");
    assert!(!station.app.secrets.has(&format!("llm:{}:key", brain_row.id)), "no key stored");
    let (header, auth) = &station_admin;
    let choice = json!({ "fast": { "provider": brain_row.id, "model": "stand" } });
    message(station.put("/api/v1/brain/choice").header(header.clone(), auth.clone()).json(&choice), StatusCode::OK)
        .await;
    // The stand-in model holds its second word until released: at once this time.
    servers.release.notify_one();
    let events = ask(&station, &station_admin, "Как дела?").await;
    let start = events.iter().find(|event| event["type"] == "start").expect("the answer starts");
    let helper_name = station.app.helpers.entry(&entry).unwrap().name;
    assert_eq!(
        (start["provider"].as_str(), start["model"].as_str()),
        (Some(format!("Модель: {helper_name}").as_str()), Some("stand")),
        "{events:?}"
    );
    let text: String =
        events.iter().filter(|event| event["type"] == "text").filter_map(|event| event["text"].as_str()).collect();
    assert!(text.contains("Первое") && text.contains("второе"), "{events:?}");
    assert!(events.iter().all(|event| event["type"] != "error"), "{events:?}");

    // The computer helps one home over the link: another home, the station's gateway by its name, on the line.
    let (header, auth) = &pc_admin;
    let clients = message(pc.get("/api/v1/helpers/clients").header(header.clone(), auth.clone()), StatusCode::OK).await;
    let clients = clients["clients"].as_array().unwrap();
    assert_eq!(clients.len(), 1, "{clients:?}");
    let client = &clients[0];
    let name = station.app.identity.gateway().unwrap().name;
    assert_eq!((client["via"].as_str(), client["ownHome"].as_bool()), (Some("link"), Some(false)), "{client}");
    assert_eq!(
        (client["gateways"][0]["name"].as_str(), client["gateways"][0]["online"].as_bool()),
        (Some(name.as_str()), Some(true))
    );
    assert_eq!(client["gives"], json!(["hearing", "voice", "model", "catalog"]));
    // Both questions came over the link, the stream's and «Мозг»'s: the computer counts them for this home.
    assert_eq!(client["today"]["model"]["count"], json!(2), "{client}");

    // The key was spent: the same key again is refused, in words.
    let (status, body) = add(&station, &station_admin, json!({ "url": url, "key": key })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["message"].as_str().unwrap().starts_with(REFUSED_KEY), "{body}");

    // The link key lives in its secret only: nowhere else in the settings package, never in «Мозг».
    let bundle = duowave::placement::transfer::export(&station.app.db, &station.app.secrets, "station").unwrap();
    for (secret, value) in &bundle.secrets {
        if value.contains("dwl1_") {
            assert_eq!(secret, &format!("helper:{entry}:key"));
        }
        assert!(!value.contains(&token), "the bridge's token in {secret}");
    }
    let rest = serde_json::to_string(&(&bundle.tables, &bundle.settings)).unwrap();
    assert!(!rest.contains("dwl1_") && !rest.contains(&token) && !rest.contains("/via/"), "{rest}");
    for provider in station.app.brain.view().providers {
        assert!(!provider.base_url.unwrap_or_default().contains("/via/"), "«Мозг» stores no bridge");
    }
    // A package for an earlier import (a 0.11 gateway reads a row without an address as OpenAI's own) carries neither
    // the row nor the pick of it (src/placement/transfer.rs, `for_schema`).
    let cut = duowave::placement::transfer::for_schema(bundle, duowave::placement::transfer::BEFORE_HOME_SCHEMA);
    assert!(cut.tables["llm_providers"].iter().all(|row| row["id"] != brain_row.id.as_str()));
    assert_eq!(cut.settings["brain"].get("fast"), None, "{}", cut.settings["brain"]);
    station.stop().await;
    pc.stop().await;
}

#[tokio::test]
async fn a_substituted_certificate_is_refused_before_anything_is_sent() {
    let servers = servers().await;
    let (pc, pc_admin) = computer(&servers, &[]).await;
    let (station, station_admin) = station().await;
    let link_port = pc.link_port().unwrap();
    let clients = || pc.app.pc.clients.view(&[]);

    // (1) At the first meeting: an impostor with its own certificate between them — the bindings differ, the key is
    // refused, nothing is paired; the same key then pairs directly (it was not spent).
    let key = connection_key(&pc, &pc_admin).await;
    let impostor = Impostor::to(link_port, true).await;
    let (status, body) = add(&station, &station_admin, json!({ "url": impostor.url(), "key": key })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["message"].as_str().unwrap().starts_with(REFUSED_KEY), "{body}");
    assert!(impostor.reached.load(Ordering::SeqCst) > 0, "the pairing went through the impostor");
    assert!(clients().is_empty(), "no client made");
    assert!(station.app.helpers.list().is_empty());

    // (2) Later: paired through a plain relay, then the relay hands the link to the impostor — the pinned
    // certificate is not its, the link is down in words, and the impostor never saw the link key.
    let relay = Relay::to(link_port).await;
    station.app.helpers.set_link_timing(quick());
    let entry = paired(&station, &station_admin, &relay.url(), &key).await;
    assert!(until(5000, || up(&station, &entry)).await);
    let impostor = Impostor::to(link_port, true).await;
    relay.point(impostor.port);
    relay.cut();
    relay.open();
    let foreign = "чужой сертификат: это не то Турбо, что было подключено";
    assert!(
        until(
            5000,
            || matches!(link_state(&station, &entry), Some(LinkState::Down { reason, .. }) if reason == foreign)
        )
        .await,
        "{:?}",
        link_state(&station, &entry)
    );
    assert!(impostor.reached.load(Ordering::SeqCst) > 0, "the link tried the impostor");
    assert!(!impostor.saw_key.load(Ordering::SeqCst), "the key is never written before the pinned handshake");
    let view = station.app.helpers.views().into_iter().find(|view| view.id == entry).unwrap();
    assert_eq!(view.state, duowave::helpers::HelperState::Down);
    assert_eq!(view.error.as_deref(), Some(foreign));

    // (3) Behind a proxy with a name: the computer's «Адрес снаружи» is helper.example.com and a proxy on it hands the
    // link to its main port; a relay whose certificate is its own (no public chain for that name) cannot pair.
    let (header, auth) = &pc_admin;
    let outside = pc.put("/api/v1/helpers/key/outside").header(header.clone(), auth.clone());
    message(outside.json(&json!({ "url": "https://helper.example.com" })), StatusCode::OK).await;
    let key = connection_key(&pc, &pc_admin).await;
    let proxy = Impostor::to(pc.port, false).await;
    let (status, body) = add(&station, &station_admin, json!({ "url": proxy.url(), "key": key })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["message"].as_str().unwrap().starts_with(REFUSED_KEY), "{body}");
    assert!(
        proxy.saw_challenge.load(Ordering::SeqCst),
        "refused at the proofs — the bindings differ —, not at the door"
    );
    assert_eq!(clients().len(), 1, "only the pairing of (2)");
    station.stop().await;
    pc.stop().await;
}

#[tokio::test]
async fn a_wrong_key_is_refused_without_a_hint() {
    let servers = servers().await;
    let (pc, pc_admin) = computer(&servers, &[]).await;
    let (station, station_admin) = station().await;
    let url = format!("https://127.0.0.1:{}", pc.link_port().unwrap());
    let link_port = pc.link_port().unwrap();
    let key = connection_key(&pc, &pc_admin).await;

    // A wrong connection key: the helper closes with 4401 and says nothing more; the station says it in words.
    let wrong = ConnectionKey::new().shown();
    let (status, body) = add(&station, &station_admin, json!({ "url": url, "key": wrong })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["message"].as_str().unwrap().starts_with(REFUSED_KEY), "{body}");
    // Something that is no key at all is the station's own words.
    let (status, body) = add(&station, &station_admin, json!({ "url": url, "key": "not a key" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["message"], "Ключ подключения — 26 букв и цифр, как его показывает Турбо.");
    // A name that cannot be kept: refused — the probe as the add — before the helper is asked, the key not spent.
    for dry in [true, false] {
        let (status, body) =
            add(&station, &station_admin, json!({ "url": url, "key": key, "name": " ", "dryRun": dry })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["message"], "Имя Турбо: от 1 до 60 знаков.");
    }

    // Paired with the right key: its link key opens the link — proven over the challenge, never shown.
    let entry = paired(&station, &station_admin, &url, &key).await;
    let link_key = station.app.secrets.get(&format!("helper:{entry}:key")).expect("the link key");
    assert!(link_key.starts_with("dwl1_"));
    let pin = pc.app.pc.link.fingerprint().map(|pin| hex::decode(pin).unwrap().try_into().unwrap()).unwrap();
    assert_eq!(link_with(link_port, &link_key, pin).await, Ok("welcome".to_string()));
    // Another key under the right id, or the right key bound to another certificate (a relay's): 4401 after the proof.
    let other = format!("{}{}", &link_key[..link_key.len() - 1], if link_key.ends_with('0') { '1' } else { '0' });
    assert_eq!(link_with(link_port, &other, pin).await, Err((4401, Vec::new())));
    assert_eq!(link_with(link_port, &link_key, [7; 32]).await, Err((4401, Vec::new())));
    // Garbage, an unknown id, a removed home: one and the same empty 401 at the door.
    let unknown = format!("dwl1_{}_{}", "0".repeat(12), "a".repeat(64));
    let garbage = link_with(link_port, "garbage", pin).await;
    let unknown = link_with(link_port, &unknown, pin).await;
    let (header, auth) = &pc_admin;
    let client = pc.app.pc.clients.view(&[])[0].id.clone();
    let remove = pc.delete(&format!("/api/v1/helpers/clients/{client}")).header(header.clone(), auth.clone());
    message(remove, StatusCode::OK).await;
    let removed = link_with(link_port, &link_key, pin).await;
    assert_eq!(garbage, Err((401, Vec::new())));
    assert_eq!((&unknown, &removed), (&garbage, &garbage), "no hint which one it was");
    station.stop().await;
    pc.stop().await;
}

#[tokio::test]
async fn a_dropped_link_comes_back_after_the_pauses() {
    let servers = servers().await;
    let (pc, pc_admin) = computer(&servers, &[]).await;
    let (station, station_admin) = station().await;
    let relay = Relay::to(pc.link_port().unwrap()).await;
    station.app.helpers.set_link_timing(quick());
    let key = connection_key(&pc, &pc_admin).await;
    let entry = paired(&station, &station_admin, &relay.url(), &key).await;
    assert!(until(5000, || up(&station, &entry)).await);
    assert_eq!(hears(&station).await, "включи свет");
    // Up longer than `steady` (when the pauses start from the first again: the unit test of `Backoff` holds that).
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Cut: on the line for the grace while it tries again, then away.
    let before = relay.tries().len();
    relay.cut();
    assert!(until(5000, || station.app.helpers.link_of(&entry).is_some_and(|link| !link.connected())).await);
    assert!(matches!(link_state(&station, &entry), Some(LinkState::Up { .. })), "a blip does not take it away");
    assert!(until(5000, || matches!(link_state(&station, &entry), Some(LinkState::Down { .. }))).await);
    // The tries came after the pauses, in order: 10, then 20, 50, 100 ms apart at least (less a fifth for the clock).
    assert!(until(5000, || relay.tries().len() >= before + 4).await, "{} tries", relay.tries().len() - before);
    let tries = relay.tries()[before..].to_vec();
    for (gap, pause) in tries.windows(2).zip([20u64, 50, 100]) {
        let between = gap[1].duration_since(gap[0]);
        assert!(between >= Duration::from_millis(pause * 8 / 10), "{between:?} after a pause of {pause} ms");
    }

    // Open again: back on the line by itself, and the station hears over the link again.
    relay.open();
    assert!(until(5000, || up(&station, &entry)).await, "{:?}", link_state(&station, &entry));
    assert_eq!(hears(&station).await, "включи свет");

    // Started again, on a port of its own (its key and pin kept): on the line by itself once it listens, hearing and
    // speaking through its bridge at its new address.
    let station = station.restart().await;
    assert!(until(5000, || up(&station, &entry)).await, "{:?}", link_state(&station, &entry));
    assert_eq!(hears(&station).await, "включи свет");
    station.app.voice.use_home().unwrap();
    let speaker = station.app.voice.speaker().expect("our voice through the helper");
    assert!(speaker.base_url().contains(&format!(":{}/via/{entry}/", station.port)), "{}", speaker.base_url());
    let spoken = speaker.synthesize("Привет.", &CancellationToken::new()).await.unwrap();
    assert_eq!(&spoken.audio[..4], b"OggS");
    station.stop().await;
    pc.stop().await;
}

#[tokio::test]
async fn another_home_gets_the_muscles_until_the_owner_gives_more() {
    let servers = servers().await;
    let (pc, pc_admin) = computer(&servers, &[]).await;
    let (station, station_admin) = station().await;
    let url = format!("https://127.0.0.1:{}", pc.link_port().unwrap());
    let key = connection_key(&pc, &pc_admin).await;
    let entry = paired(&station, &station_admin, &url, &key).await;
    assert!(until(5000, || up(&station, &entry)).await);
    let token = station.app.helpers.local().token().to_string();
    let contacts = || station.get(&format!("/via/{entry}/home/v1/messenger/contacts")).bearer_auth(&token);
    let (status, body) = send(contacts()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["message"], "Это Турбо не даёт этого вашему дому.");

    // The owner gives this home the messages too: the computer answers for its bot (none set up here).
    let (header, auth) = &pc_admin;
    let client = pc.app.pc.clients.view(&[])[0].id.clone();
    let gives = json!({ "gives": ["hearing", "voice", "model", "catalog", "messages"] });
    let put = pc.put(&format!("/api/v1/helpers/clients/{client}")).header(header.clone(), auth.clone());
    let clients = message(put.json(&gives), StatusCode::OK).await;
    assert_eq!(clients["clients"][0]["gives"], json!(["hearing", "voice", "model", "messages", "catalog"]));
    let (status, body) = send(contacts()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body["message"].as_str().unwrap().contains("не настроен бот Telegram"), "{body}");
    // A key of «Доступ» is not changed here.
    let put = pc.put("/api/v1/helpers/clients/key:0123456789ab").header(header.clone(), auth.clone());
    assert_eq!(send(put.json(&gives)).await.0, StatusCode::CONFLICT);

    // The home removes the helper: `bye` over the link, and the computer forgets that home and its key.
    let (header, auth) = &station_admin;
    message(station.delete(&format!("/api/v1/helpers/{entry}")).header(header.clone(), auth.clone()), StatusCode::OK)
        .await;
    assert!(until(5000, || pc.app.pc.clients.view(&[]).is_empty()).await, "{:?}", pc.app.pc.clients.view(&[]));
    assert!(
        station.app.helpers.pin(&entry).is_none() && station.app.secrets.get(&format!("helper:{entry}:key")).is_none()
    );
    station.stop().await;
    pc.stop().await;
}

#[tokio::test]
async fn a_home_claimed_without_its_key_is_another_home() {
    use duowave::settings::definitions::{HomeIdentity, HOME_IDENTITY};
    let servers = servers().await;
    let (pc, pc_admin) = computer(&servers, &[]).await;
    let (station, station_admin) = station().await;
    let url = format!("https://127.0.0.1:{}", pc.link_port().unwrap());
    // The station names the computer's home — anyone at home can read its id — but has not its key.
    let home = pc.app.identity.home().unwrap().id;
    station.app.settings.set(&HOME_IDENTITY, &HomeIdentity { id: home.clone(), name: None }, None).unwrap();
    let key = connection_key(&pc, &pc_admin).await;
    let entry = paired(&station, &station_admin, &url, &key).await;
    let client = pc.app.pc.clients.view(&[]).into_iter().next().unwrap();
    assert_eq!((client.own_home, client.gives.len()), (false, 4), "{client:?}");

    // With the home's key too (the computer's package taken): its own home — everything, by a new pairing that renews
    // the entry in place.
    let home_key = pc.app.secrets.get("home:key").unwrap();
    station.app.secrets.set("home:key", Some(&home_key)).unwrap();
    let key = connection_key(&pc, &pc_admin).await;
    let (status, body) = add(&station, &station_admin, json!({ "url": url, "key": key })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        (body["message"]["id"].as_str(), body["message"]["matched"].as_str()),
        (Some(entry.as_str()), Some(entry.as_str()))
    );
    // One gateway, one client: the pairing that proved the home replaced the one that only claimed it.
    let clients = pc.app.pc.clients.view(&[]);
    assert_eq!(clients.len(), 1, "{clients:?}");
    assert!(clients[0].own_home);
    // Its own home gets contract 1's and the computer's devices and folders (contract 3, docs/plans/turbo-devices.md).
    let everything: Vec<Ability> = Ability::ALL
        .into_iter()
        .filter(|ability| Ability::V1.contains(ability) || Ability::V3.contains(ability))
        .collect();
    assert_eq!(clients[0].gives, everything);
    station.stop().await;
    pc.stop().await;
}

#[tokio::test]
async fn a_link_to_a_helper_the_regulator_keeps_out_is_closed() {
    let servers = servers().await;
    let (pc, pc_admin) = computer(&servers, &[]).await;
    let (station, station_admin) = station().await;
    let relay = Relay::to(pc.link_port().unwrap()).await;
    station.app.helpers.set_link_timing(quick());
    let key = connection_key(&pc, &pc_admin).await;
    // «Мой сервер»: its reach is «моё», though its address is at home.
    let (status, body) = add(&station, &station_admin, json!({ "url": relay.url(), "key": key, "kind": "own" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let entry = body["message"]["id"].as_str().unwrap().to_string();
    assert!(until(5000, || up(&station, &entry)).await);
    assert!(matches!(station.app.helpers.pick(Ability::Hearing), Some(Giver::Helper(_))));

    // «Только дома»: the link closes, and nothing dials the helper while it stays so.
    let (header, auth) = &station_admin;
    let away = station.put("/api/v1/helpers/away").header(header.clone(), auth.clone());
    message(away.json(&json!({ "away": "home" })), StatusCode::OK).await;
    assert!(until(5000, || station.app.helpers.link_of(&entry).is_none()).await, "the link is stopped");
    assert!(station.app.helpers.pick(Ability::Hearing).is_none());
    let tries = relay.tries().len();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(relay.tries().len(), tries, "no connection while the regulator keeps it out");
    // The registry (stage 5) and `/health` say it is the regulator, not the helper away.
    let read = |path: &'static str| station.get(path).header(header.clone(), auth.clone());
    let registry = message(read("/api/v1/availability"), StatusCode::OK).await;
    let helpers = &registry["helpers"];
    assert_eq!((&helpers["shut"], &helpers["online"], &helpers["up"]), (&json!(true), &json!(false), &json!([])));
    let health = message(station.get("/health"), StatusCode::OK).await;
    assert_eq!(health["placement"]["helpers"]["list"][0]["allowed"], false);
    let name = station.app.helpers.entry(&entry).unwrap().name;
    let view = message(read("/api/v1/helpers"), StatusCode::OK).await;
    let hearing = view["now"].as_array().unwrap().iter().find(|row| row["ability"] == "hearing").unwrap().clone();
    assert_eq!(hearing["why"], format!("Регулятор не пускает: {name}"), "{hearing}");

    // «Любые облака» again: on the line.
    let away = station.put("/api/v1/helpers/away").header(header.clone(), auth.clone());
    message(away.json(&json!({ "away": "any" })), StatusCode::OK).await;
    assert!(until(5000, || up(&station, &entry)).await);
    let registry = message(read("/api/v1/availability"), StatusCode::OK).await;
    assert!(registry["helpers"].get("shut").is_none() && has(&registry["helpers"]["up"], "hearing"), "{registry}");
    station.stop().await;
    pc.stop().await;
}

#[tokio::test]
async fn the_connection_key_is_shown_to_the_admin_only() {
    let servers = servers().await;
    let (pc, pc_admin) = computer(&servers, &[]).await;
    let (header, auth) = &pc_admin;
    // A member signed in with a password.
    let member = json!({ "name": "Аня", "password": "anya-password", "role": "member" });
    message(pc.post("/api/v1/users").header(header.clone(), auth.clone()).json(&member), StatusCode::CREATED).await;
    let signed = pc.post("/api/v1/auth/login").json(&json!({ "name": "Аня", "password": "anya-password" }));
    let anya = bearer(message(signed, StatusCode::OK).await["token"].as_str().unwrap());
    // A key with every right, and the pair's pass as an admin.
    let made = json!({ "name": "Сценарий", "grants": ["*"] });
    let made =
        message(pc.post("/api/v1/api-keys").header(header.clone(), auth.clone()).json(&made), StatusCode::CREATED)
            .await;
    let script = bearer(made["secret"].as_str().unwrap());
    let shown = "Ключ подключения показывается только администратору, вошедшему по паролю.";
    for (who, (name, value)) in [("member", anya), ("key", script)] {
        let (status, body) = send(pc.get("/api/v1/helpers/key").header(name, value)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}: {body}");
        assert!(who == "member" || body["message"] == shown, "{who}: {body}");
    }
    let pass = pc
        .get("/api/v1/helpers/key")
        .header("authorization", format!("Bearer {PEER}"))
        .header(duowave::placement::peer::HELPER_ROLE, "admin");
    let (status, body) = send(pass).await;
    assert_eq!((status, body["message"].as_str()), (StatusCode::FORBIDDEN, Some(shown)), "the pass is never a person");

    // The admin: the key, where to reach the link, the text and QR, until when it is taken — never cached.
    let response = pc.get("/api/v1/helpers/key").header(header.clone(), auth.clone()).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let sheet: Value = response.json().await.unwrap();
    let sheet = &sheet["message"];
    let key = sheet["key"].as_str().unwrap();
    assert!(ConnectionKey::parse(key).is_some(), "{sheet}");
    assert_eq!(sheet["port"], pc.link_port().unwrap());
    assert!(sheet["armedUntil"].is_string() && sheet["addresses"].is_array(), "{sheet}");
    let payload = sheet["payload"].as_str().unwrap();
    assert!(payload.ends_with(key), "{payload}");
    // Read again while the sheet stays open: the same key with the same end — a read never moves the quarter of an
    // hour on, so a sheet left open opens nothing 15 minutes after the key was shown (review of stage 7).
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let again = message(pc.get("/api/v1/helpers/key").header(header.clone(), auth.clone()), StatusCode::OK).await;
    assert_eq!(again["key"], key);
    let end = |sheet: &Value| chrono::DateTime::parse_from_rfc3339(sheet["armedUntil"].as_str().unwrap()).unwrap();
    let moved = (end(&again) - end(sheet)).num_milliseconds().abs();
    assert!(moved < 500, "the end moved {moved} ms: {again}");
    // «Сменить ключ»: a new one.
    let rotated =
        message(pc.post("/api/v1/helpers/key/rotate").header(header.clone(), auth.clone()), StatusCode::OK).await;
    assert_ne!(rotated["key"], key);

    // A gateway in the station role helps nobody: no key there.
    let (station, station_admin) = station().await;
    let (header, auth) = &station_admin;
    assert_eq!(
        send(station.get("/api/v1/helpers/key").header(header.clone(), auth.clone())).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(station.link_port(), None, "no link port in the station role");
    station.stop().await;
    pc.stop().await;
}

#[tokio::test]
async fn adding_the_same_helper_again_renews_its_entry_and_the_probe_checks_as_the_add_does() {
    let servers = servers().await;
    let (pc, pc_admin) = computer(&servers, &[]).await;
    let (station, station_admin) = station().await;
    let url = format!("https://127.0.0.1:{}", pc.link_port().unwrap());
    let key = connection_key(&pc, &pc_admin).await;
    let entry = paired(&station, &station_admin, &url, &key).await;
    let (header, auth) = &station_admin;
    let change = json!({ "name": "Ноутбук", "without": ["model"] });
    let put = station.put(&format!("/api/v1/helpers/{entry}")).header(header.clone(), auth.clone());
    message(put.json(&change), StatusCode::OK).await;

    // The probe: what saving would do — the same helper with the pinned certificate is renewed, not doubled.
    let key = connection_key(&pc, &pc_admin).await;
    let (status, body) = add(&station, &station_admin, json!({ "url": url, "key": key, "dryRun": true })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["message"]["matched"], entry.as_str());
    // A probe spends nothing: the same key pairs after it. A plain «Добавить помощника» renews the entry in place —
    // its name and «Не брать у него…» kept, the answer naming it.
    let (status, body) = add(&station, &station_admin, json!({ "url": url, "key": key, "name": "Другое" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        (body["message"]["id"].as_str(), body["message"]["matched"].as_str()),
        (Some(entry.as_str()), Some(entry.as_str()))
    );
    let list = station.app.helpers.list();
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!((list[0].name.as_str(), &list[0].without), ("Ноутбук", &vec![Ability::Model]));
    assert!(until(5000, || up(&station, &entry)).await);
    // The old key's client said `bye` over the old link: the helper keeps one client for the one gateway.
    assert!(until(5000, || pc.app.pc.clients.view(&[]).len() == 1).await, "{:?}", pc.app.pc.clients.view(&[]));
    // «Подключить заново» names an entry that is not there: refused before anything is spent, the probe as the add.
    let key = connection_key(&pc, &pc_admin).await;
    for dry in [true, false] {
        let body = json!({ "url": url, "key": key, "replace": "own-00000000", "dryRun": dry });
        let (status, body) = add(&station, &station_admin, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    }
    let (status, _) = add(&station, &station_admin, json!({ "url": url, "key": key, "replace": entry })).await;
    assert_eq!(status, StatusCode::CREATED, "the key was not spent by the refusals");
    assert!(until(5000, || up(&station, &entry)).await);
    assert!(until(5000, || pc.app.pc.clients.view(&[]).len() == 1).await, "{:?}", pc.app.pc.clients.view(&[]));
    station.stop().await;
    pc.stop().await;
}

/// A plain upgrade to the computer's main port from this computer, as a proxy on it hands the link on: the site's name
/// as the `Host`, the client it came for in `X-Forwarded-For`. The socket when it is up, else the status and the body.
async fn through_the_proxy(
    port: u16,
    headers: &[(&str, &str)],
) -> Result<tokio_tungstenite::WebSocketStream<TcpStream>, (u16, Vec<u8>)> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut request = "ws://helper.example.com/home/v1/link".into_client_request().unwrap();
    request.headers_mut().insert("x-forwarded-for", "198.51.100.7".parse().unwrap());
    let version = duowave::helpers::link::VERSION.to_string();
    request.headers_mut().insert("x-duowave-link-version", version.parse().unwrap());
    for (name, value) in headers {
        request.headers_mut().insert(HeaderName::from_bytes(name.as_bytes()).unwrap(), value.parse().unwrap());
    }
    match tokio_tungstenite::client_async(request, tcp).await {
        Ok((socket, _)) => Ok(socket),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            Err((response.status().as_u16(), response.body().clone().unwrap_or_default()))
        }
        Err(error) => panic!("the upgrade: {error}"),
    }
}

/// A later link through the proxy's door, bound to the outside name (`prove_link`).
async fn link_through_the_proxy(port: u16, key: &str) -> Result<String, (u16, Vec<u8>)> {
    let headers = link_headers(key);
    let headers: Vec<(&str, &str)> = headers.iter().map(|(name, value)| (*name, value.as_str())).collect();
    let socket = through_the_proxy(port, &headers).await;
    prove_link(socket, key, &Binding::Name("helper.example.com".into())).await
}

#[tokio::test]
async fn a_proxy_on_the_helper_hands_the_link_on_by_the_outside_name() {
    let servers = servers().await;
    let (pc, pc_admin) = computer(&servers, &[]).await;
    let (header, auth) = &pc_admin;
    let outside = pc.put("/api/v1/helpers/key/outside").header(header.clone(), auth.clone());
    message(outside.json(&json!({ "url": "https://helper.example.com" })), StatusCode::OK).await;
    let key = ConnectionKey::parse(&connection_key(&pc, &pc_admin).await).unwrap();
    // The gateway saw the site's public chain at the proxy and bound its name; the helper binds «Адрес снаружи».
    let mut socket = through_the_proxy(pc.port, &[("x-duowave-link", "pair")]).await.expect("the pairing's socket");
    let me = Me {
        gateway: HelperIdentity { id: "gateway-far".into(), name: "Колонка".into() },
        home: HelperIdentity { id: "home-far".into(), name: "Дача".into() },
    };
    let name = Binding::Name("helper.example.com".into());
    let accept = |_: &HelperIdentity, _: &duowave::settings::definitions::HelperPin| Ok(());
    let paired = pair_over(&mut socket, &key, &me, "", &name, &quick(), false, &accept).await.expect("paired");
    assert_eq!(paired.pin.public.as_deref(), Some("helper.example.com"));
    let link_key = paired.key.expect("a link key");
    let clients = pc.app.pc.clients.view(&[]);
    assert_eq!((clients.len(), clients[0].own_home), (1, false), "{clients:?}");
    // Later links through the same door: the link key, proven over the outside name, opens it, and the welcome comes.
    assert_eq!(link_through_the_proxy(pc.port, &link_key).await, Ok("welcome".to_string()));
    // «Адрес снаружи» changed, then cleared: the link proves over what its pairing bound, and still comes up.
    let outside = pc.put("/api/v1/helpers/key/outside").header(header.clone(), auth.clone());
    message(outside.json(&json!({ "url": "https://other.example.com" })), StatusCode::OK).await;
    assert_eq!(link_through_the_proxy(pc.port, &link_key).await, Ok("welcome".to_string()));
    pc.app.settings.forget(&duowave::settings::definitions::HELPER_OUTSIDE).unwrap();
    assert_eq!(link_through_the_proxy(pc.port, &link_key).await, Ok("welcome".to_string()));
    // An unknown key at that door: the same empty 401.
    let unknown = format!("dwl1_{}_{}", "0".repeat(12), "a".repeat(64));
    assert_eq!(link_through_the_proxy(pc.port, &unknown).await, Err((401, Vec::new())));
    pc.stop().await;
}

// ── Where the built-in helper (stage 6), the registry (stage 5) and the link meet ──

/// A model server of the computer that can be closed: each chat answered at once. Closed, its port takes nothing.
struct ClosableModel {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl ClosableModel {
    async fn start() -> ClosableModel {
        let router = axum::Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                let chunk = json!({ "choices": [{ "delta": { "content": "Ответ модели." } }] });
                ([("content-type", "text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        ClosableModel { url, task }
    }

    /// It stops listening — for good once a connection to its port is refused.
    async fn close(self) {
        let address = self.url.trim_start_matches("http://").trim_end_matches("/v1").to_string();
        self.task.abort();
        let _ = self.task.await;
        let deadline = Instant::now() + Duration::from_secs(5);
        while TcpStream::connect(address.as_str()).await.is_ok() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// «Где сейчас»'s row of an ability.
fn now_row<'a>(view: &'a Value, ability: &str) -> &'a Value {
    view["now"].as_array().unwrap().iter().find(|row| row["ability"] == ability).expect("a row")
}

fn has(list: &Value, ability: &str) -> bool {
    list.as_array().unwrap().iter().any(|item| item == ability)
}

/// A computer never pairs with itself: its own key at its own link port is refused once the pairing reads its own id
/// — nothing stored, no client, the key not spent —, and a linked entry that names it (as a list taken in a package
/// brings it) is this computer itself: shown so, never dialed, giving nothing, its own parts still first.
#[tokio::test]
async fn a_computer_never_pairs_with_itself_and_an_entry_that_names_it_is_itself() {
    use duowave::helpers::{HelperKind, HelperSetting, HelperState};
    use duowave::settings::definitions::{HelperPin, HELPERS, HELPER_PINS};
    let servers = servers().await;
    let (pc, pc_admin) = computer(&servers, &[]).await;
    let url = format!("https://127.0.0.1:{}", pc.link_port().unwrap());
    let key = connection_key(&pc, &pc_admin).await;
    let (status, body) = add(&pc, &pc_admin, json!({ "url": url, "key": key })).await;
    assert_eq!(
        (status, body["message"].as_str()),
        (StatusCode::CONFLICT, Some("Это сам этот хаб: себя в Турбо не добавляют.")),
        "{body}"
    );
    assert!(pc.app.helpers.list().is_empty() && pc.app.pc.clients.view(&[]).is_empty(), "nothing stored, no client");
    // The key was not spent: a station pairs with it.
    let (station, station_admin) = station().await;
    paired(&station, &station_admin, &url, &key).await;
    station.stop().await;

    // A linked entry naming this computer: its key, its pin, then the entry.
    let me = pc.app.identity.gateway().unwrap().id;
    let id = "home-0a1b2c3d";
    let link_key = format!("dwl1_{}_{}", "0".repeat(12), "a".repeat(64));
    pc.app.secrets.set(&format!("helper:{id}:key"), Some(&link_key)).unwrap();
    let pin = HelperPin { sha256: pc.app.pc.link.fingerprint(), public: None, since: String::new() };
    pc.app.settings.set(&HELPER_PINS, &std::collections::BTreeMap::from([(id.to_string(), pin)]), None).unwrap();
    let entry = HelperSetting {
        id: id.into(),
        name: "Компьютер".into(),
        url: url.clone(),
        kind: HelperKind::Home,
        enabled: true,
        without: Vec::new(),
        helper_id: Some(me),
    };
    pc.app.settings.set(&HELPERS, &vec![entry], None).unwrap();
    assert!(until(5000, || pc.app.helpers.list().len() == 1).await, "the list followed");
    let views = pc.app.helpers.views();
    assert_eq!((views[0].id.as_str(), views[1].id.as_str(), views[1].state), ("self", id, HelperState::Itself));
    let (header, auth) = &pc_admin;
    message(pc.post(&format!("/api/v1/helpers/{id}/check")).header(header.clone(), auth.clone()), StatusCode::OK).await;
    assert!(pc.app.helpers.link_of(id).is_none(), "never dialed, not on «Проверить сейчас» either");
    assert!(matches!(pc.app.helpers.pick(Ability::Hearing), Some(Giver::Itself)), "its own parts first");
    assert!(pc.app.helpers.offering(Ability::Hearing).is_empty(), "the entry gives nothing");
    let view = message(pc.get("/api/v1/helpers").header(header.clone(), auth.clone()), StatusCode::OK).await;
    assert!(view["now"].as_array().unwrap().iter().all(|row| row["helper"] != id), "{}", view["now"]);
    pc.stop().await;
}

/// A computer in the station role has no parts of its own: its hearing, voice and model are its linked computer's,
/// and «Где сейчас», the registry and `/health` say so, with no «Этот компьютер». The computer's model server falling
/// silent reaches it over the link — the computer's next look changes what it gives, and the link tells its home —:
/// the helper stays on the line, its model is said silent, and hearing goes on.
#[tokio::test]
async fn a_station_takes_its_parts_over_the_link_and_hears_its_computers_model_fall_silent() {
    use duowave::helpers::OwnParts;
    let servers = servers().await;
    let model = ClosableModel::start().await;
    let (pc, pc_admin) = computer(&servers, &[]).await;
    // The model the computer hands on is this one, which can be closed.
    let moved = |c: &rusqlite::Connection| {
        c.execute("UPDATE llm_providers SET base_url = ?1 WHERE id = 'stand'", [model.url.as_str()])
    };
    pc.app.db.with(moved).unwrap();
    pc.app.pc.builtin.check().await;
    let (station, station_admin) = station().await;
    let url = format!("https://127.0.0.1:{}", pc.link_port().unwrap());
    let key = connection_key(&pc, &pc_admin).await;
    let entry = paired(&station, &station_admin, &url, &key).await;
    assert!(until(5000, || up(&station, &entry)).await, "{:?}", link_state(&station, &entry));
    station.app.voice.use_home().unwrap();
    let row = format!("helper-{entry}");
    let listed = || station.app.brain.view().providers.iter().any(|provider| provider.id == row);
    assert!(until(5000, listed).await, "the linked helper's model has its row");
    let (header, auth) = &station_admin;
    let choice = json!({ "fast": { "provider": row, "model": "stand" } });
    message(station.put("/api/v1/brain/choice").header(header.clone(), auth.clone()).json(&choice), StatusCode::OK)
        .await;
    let read = |path: &'static str| station.get(path).header(header.clone(), auth.clone());

    let view = message(read("/api/v1/helpers"), StatusCode::OK).await;
    let ids: Vec<&str> =
        view["helpers"].as_array().unwrap().iter().filter_map(|helper| helper["id"].as_str()).collect();
    assert_eq!(ids, [entry.as_str()], "nothing of its own");
    for ability in ["hearing", "voice", "model"] {
        let row = now_row(&view, ability);
        assert_eq!(
            (row["by"].as_str(), row["helper"].as_str(), row["first"].as_bool()),
            (Some("helper"), Some(entry.as_str()), Some(true)),
            "{ability}: {row}"
        );
    }
    let health = message(station.get("/health"), StatusCode::OK).await;
    let list = health["placement"]["helpers"]["list"].as_array().unwrap().clone();
    assert_eq!((list.len(), &list[0]["id"], &list[0]["allowed"]), (1, &json!(entry), &json!(true)), "{list:?}");
    assert!(list[0].get("builtin").is_none());
    let registry = message(read("/api/v1/availability"), StatusCode::OK).await;
    let helpers = &registry["helpers"];
    assert_eq!((&helpers["online"], &helpers["builtin"]), (&json!(true), &json!([])), "{helpers}");
    for ability in ["hearing", "voice", "model"] {
        assert!(has(&helpers["up"], ability), "{ability}: {helpers}");
    }

    // The computer's model server closes; the computer's look sees it and the link tells its home (merged, ≤ 5 s).
    model.close().await;
    pc.app.pc.builtin.check().await;
    let silent = || {
        let views = station.app.helpers.views();
        let view = views.iter().find(|view| view.id == entry);
        view.is_some_and(|view| view.abilities.iter().any(|ability| ability.id == Ability::Model && !ability.ready))
    };
    // Merged to one event per 5 s, and read again by every link each 15 s at the latest.
    assert!(until(20000, silent).await, "told over the link");
    let view = message(read("/api/v1/helpers"), StatusCode::OK).await;
    assert_eq!(view["helpers"][0]["state"], "up", "on the line all the same");
    let name = station.app.helpers.entry(&entry).unwrap().name;
    let model_row = now_row(&view, "model");
    assert!(
        model_row["why"].as_str().unwrap_or_default().starts_with(&format!("{name}: модель сейчас не отвечает")),
        "the model silent, not the helper away: {model_row}"
    );
    assert_ne!(model_row["by"], "helper", "{model_row}");
    let registry = message(read("/api/v1/availability"), StatusCode::OK).await;
    let helpers = &registry["helpers"];
    assert!(!has(&helpers["up"], "model") && has(&helpers["waiting"], "model"), "{helpers}");
    assert!(has(&helpers["up"], "hearing"), "{helpers}");
    assert_eq!(hears(&station).await, "включи свет", "hearing goes on over the link");
    station.stop().await;
    pc.stop().await;
}

/// «Этот компьютер» beside a linked helper: a computer that takes its own parts from itself and pairs with another
/// computer lists itself first and the linked one after it; it speaks with its own voice and hears through the other
/// over the link, and «Где сейчас», the registry and `/health` say each by its place. «Этот компьютер» is never moved.
#[tokio::test]
async fn a_computer_lists_itself_first_and_its_linked_helper_after_it() {
    let servers = servers().await;
    let (other, other_admin) = computer(&servers, &[]).await;
    // This computer: its own voice at home, no hearing of its own.
    let vars = [("ASR_PROVIDER", "none"), ("TTS_OWN_URL", servers.tts.as_str()), ("TTS_PROVIDER", "mixed")];
    let pc = Gateway::start(&vars).await;
    let pc_admin = admin(&pc).await;
    let url = format!("https://127.0.0.1:{}", other.link_port().unwrap());
    let key = connection_key(&other, &other_admin).await;
    let entry = paired(&pc, &pc_admin, &url, &key).await;
    assert!(until(5000, || up(&pc, &entry)).await, "{:?}", link_state(&pc, &entry));
    let (header, auth) = &pc_admin;
    let read = |path: &'static str| pc.get(path).header(header.clone(), auth.clone());

    let view = message(read("/api/v1/helpers"), StatusCode::OK).await;
    let rows = view["helpers"].as_array().unwrap();
    let ids: Vec<&str> = rows.iter().filter_map(|row| row["id"].as_str()).collect();
    assert_eq!(ids, ["self", entry.as_str()]);
    assert_eq!((&rows[0]["builtin"], &rows[1]["link"]["state"]), (&json!(true), &json!("up")), "{rows:?}");
    let hearing = now_row(&view, "hearing");
    assert_eq!(
        (hearing["helper"].as_str(), hearing["first"].as_bool()),
        (Some(entry.as_str()), Some(true)),
        "{hearing}"
    );
    let voice = now_row(&view, "voice");
    assert_eq!((voice["helper"].as_str(), voice["first"].as_bool()), (Some("self"), Some(true)), "{voice}");
    assert!(matches!(pc.app.helpers.pick(Ability::Voice), Some(Giver::Itself)));
    assert!(matches!(pc.app.helpers.pick(Ability::Hearing), Some(Giver::Helper(remote)) if remote.id() == entry));
    assert_eq!(hears(&pc).await, "включи свет", "through the other computer, over the link");
    let health = message(pc.get("/health"), StatusCode::OK).await;
    let briefs: Vec<(String, bool, bool)> = health["placement"]["helpers"]["list"]
        .as_array()
        .unwrap()
        .iter()
        .map(|brief| (brief["id"].as_str().unwrap().to_string(), brief["builtin"] == true, brief["allowed"] == true))
        .collect();
    assert_eq!(briefs, [("self".to_string(), true, true), (entry.clone(), false, true)]);
    let registry = message(read("/api/v1/availability"), StatusCode::OK).await;
    let helpers = &registry["helpers"];
    assert!(has(&helpers["builtin"], "voice") && !has(&helpers["builtin"], "hearing"), "{helpers}");
    assert!(has(&helpers["up"], "hearing") && has(&helpers["up"], "voice"), "{helpers}");
    let feature = |id: &str| {
        let features = registry["features"].as_array().unwrap();
        features.iter().find(|feature| feature["id"] == id).expect("a feature").clone()
    };
    let (hearing, voice) = (feature("asr.own"), feature("voice.own"));
    assert_eq!((hearing["runs"].as_str(), hearing["reason"].as_str()), (Some("homeServer"), Some("у Турбо")));
    assert_eq!((voice["runs"].as_str(), voice["reason"].as_str()), (Some("here"), Some("на этом компьютере")));
    let order =
        |ids: Value| pc.put("/api/v1/helpers/order").header(header.clone(), auth.clone()).json(&json!({ "ids": ids }));
    assert_eq!(send(order(json!(["self", entry]))).await.0, StatusCode::BAD_REQUEST, "never moved");
    message(order(json!([entry])), StatusCode::OK).await;
    // The other computer helps this one as another home: one client, on the line.
    let clients = other.app.pc.clients.view(&[]);
    assert_eq!((clients.len(), clients[0].own_home), (1, false), "{clients:?}");
    pc.stop().await;
    other.stop().await;
}
