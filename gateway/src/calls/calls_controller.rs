//! The call page, the calls' status, the calls' WebSocket, and the station client's HTTP audio streams. These
//! answer in their own format (the page, the station's wget and GStreamer), without the API's envelope; the zone
//! check before them answers as the API does.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::json;

use super::calls_service::{same_origin, CALLS_WS_PATH};
use super::http_stations::ListenFormat;
use super::livekit::{app_access, app_name, livekit_page, APP_TOKEN_TTL};
use super::page::call_page;
use crate::app::App;
use crate::core::identity::permissions;
use crate::operations::networks::via_tunnel;
use crate::platform::http::{ApiError, Caller, Zone};

/// The largest message, and frame, of the calls' socket.
const MAX_MESSAGE: usize = 1024 * 1024;

pub fn router() -> Router<Arc<App>> {
    Router::new()
        .route("/calls", get(page))
        .route(CALLS_WS_PATH, get(socket))
        .route("/calls/livekit", get(livekit))
        .route("/calls/livekit/token", get(livekit_token))
        .route("/calls/status", get(status))
        .route("/calls/listen/{device}", get(listen))
        .route("/calls/next/{device}", get(next))
        .route("/calls/release/{device}", post(release))
        .route("/calls/talk/{device}", post(talk))
}

/// The caller's address as the socket gave it ("" without one).
fn peer_address(caller: &Caller) -> String {
    caller.peer.map(|peer| peer.ip().to_string()).unwrap_or_default()
}

/// The station client's routes answer an approved station's address only (any home address with
/// STATION_PAIRING=open).
fn refused(app: &App, caller: &Caller) -> Option<Response> {
    if app.calls.trusted_address(&peer_address(caller)) {
        return None;
    }
    Some((StatusCode::FORBIDDEN, Json(json!({ "error": "only an approved station" }))).into_response())
}

async fn page() -> Html<String> {
    Html(call_page())
}

/// The LiveKit page: the family room through LiveKit, from home or outside.
async fn livekit(State(app): State<Arc<App>>) -> Html<String> {
    let server =
        app.calls.livekit_config().map(|config| config.public_url.clone().unwrap_or_else(|| config.url.clone()));
    Html(livekit_page(server.as_deref()))
}

/// A join token for the family room, given only inside the home network: a phone takes it at home and keeps it (a
/// month) to call from outside.
async fn livekit_token(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    caller.zone_at_least(Zone::Home)?;
    let Some(config) = app.calls.livekit_config() else {
        return Ok(
            (StatusCode::NOT_FOUND, Json(json!({ "error": "no LiveKit: set it up in the panel" }))).into_response()
        );
    };
    let who = app_name(query.get("name").map(String::as_str));
    Ok(Json(app_access(&config, who, APP_TOKEN_TTL)).into_response())
}

/// Who is on the calls: names of contacts and their endpoints — for someone signed in.
async fn status(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
    caller.permitted(permissions::STATION_VIEW)?;
    Ok(Json(json!({ "endpoints": app.calls.hub.list(), "calls": app.calls.hub.snapshot() })).into_response())
}

/// The calls' socket: the call page, the simulator, a station client. Calls from outside go through LiveKit; the
/// hub's own socket is the home network's.
async fn socket(State(app): State<Arc<App>>, caller: Caller, upgrade: WebSocketUpgrade) -> Response {
    if via_tunnel(&caller.headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    // A browser always names the page that opens a socket: only our own call page may (no other site's script).
    let header = |name: header::HeaderName| caller.headers.get(name).and_then(|value| value.to_str().ok());
    if !same_origin(header(header::ORIGIN), header(header::HOST)) {
        tracing::warn!(
            "rejected the calls socket from {}: page {}",
            peer_address(&caller),
            header(header::ORIGIN).unwrap_or_default()
        );
        return StatusCode::FORBIDDEN.into_response();
    }
    let address = peer_address(&caller);
    let calls = app.calls.clone();
    // A frame is refused by its header, as ws's maxPayload did: a message's limit applies only once the whole frame
    // is in, and the frame's own limit is 16 MiB by default.
    upgrade
        .max_frame_size(MAX_MESSAGE)
        .max_message_size(MAX_MESSAGE)
        .on_upgrade(move |socket| calls.on_socket(socket, address))
}

/// The other party's audio for the station's call, ended with the call: 16 kHz mono PCM16LE, or Ogg/Opus with
/// `?format=ogg` (the station's GStreamer). The home network's only.
async fn listen(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(device): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    caller.zone_at_least(Zone::Home)?;
    if let Some(refused) = refused(&app, &caller) {
        return Ok(refused);
    }
    let format =
        if query.get("format").map(String::as_str) == Some("ogg") { ListenFormat::Ogg } else { ListenFormat::Pcm };
    let address = peer_address(&caller);
    let stream = app.calls.listen(&device, format, Some(address.as_str()).filter(|address| !address.is_empty()));
    let mut response = Body::from_stream(stream.map(Ok::<_, Infallible>)).into_response();
    let kind = if format == ListenFormat::Ogg { "audio/ogg" } else { "application/octet-stream" };
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// The station client's poll: the station is a party of our calls while its client polls; the answer is `call`
/// once its call is answered (then the client opens `listen`), `idle` after 50 s. The home network's only.
async fn next(State(app): State<Arc<App>>, caller: Caller, Path(device): Path<String>) -> Result<Response, ApiError> {
    caller.zone_at_least(Zone::Home)?;
    if let Some(refused) = refused(&app, &caller) {
        return Ok(refused);
    }
    let address = peer_address(&caller);
    let answer = app.calls.next(&device, Some(address.as_str()).filter(|address| !address.is_empty())).await;
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], answer.as_str()).into_response())
}

/// The station client stops: its call's stream ends and the station leaves our calls.
async fn release(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(device): Path<String>,
) -> Result<Response, ApiError> {
    caller.zone_at_least(Zone::Home)?;
    if let Some(refused) = refused(&app, &caller) {
        return Ok(refused);
    }
    app.calls.release(&device);
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// The station's microphone, 16 kHz mono PCM16LE, as a chunked upload. A sample split between two chunks is put
/// back together.
async fn talk(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(device): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    caller.zone_at_least(Zone::Home)?;
    if let Some(refused) = refused(&app, &caller) {
        return Ok(refused);
    }
    let mut body = request.into_body().into_data_stream();
    let mut odd: Option<u8> = None;
    while let Some(Ok(chunk)) = body.next().await {
        let mut bytes = Vec::with_capacity(chunk.len() + 1);
        bytes.extend(odd.take());
        bytes.extend_from_slice(&chunk);
        if bytes.len() % 2 == 1 {
            odd = bytes.pop();
        }
        app.calls.talk(&device, &super::call_hub::pcm_from_bytes(&bytes));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::HeaderMap;
    use futures::{SinkExt, StreamExt};
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message;
    use tower::ServiceExt;

    use super::*;
    use crate::calls::livekit::app_identity;
    use crate::config::{Config, Env};

    fn app(pairs: &[(&str, &str)]) -> Arc<App> {
        let mut env = Env::from_pairs([("STATE_DIR", ""), ("CALLS_STATION_MIC_PORT", "")]);
        for (key, value) in pairs {
            env = env.with(key, value);
        }
        App::build(Config::for_tests(&env)).unwrap()
    }

    async fn call(app: &Arc<App>, method: &str, uri: &str, peer: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut request = axum::http::Request::builder().method(method).uri(uri).body(Body::empty()).unwrap();
        let peer: SocketAddr = format!("{peer}:5000").parse().unwrap();
        request.extensions_mut().insert(axum::extract::ConnectInfo(peer));
        let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes().to_vec();
        (status, headers, bytes)
    }

    fn json(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).unwrap_or(Value::Null)
    }

    #[tokio::test]
    async fn serves_the_call_page_and_the_livekit_page_to_anyone() {
        let app = app(&[]);
        let (status, headers, body) = call(&app, "GET", "/calls", "203.0.113.7").await;
        assert_eq!(status, StatusCode::OK);
        assert!(headers[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/html"));
        assert!(String::from_utf8(body).unwrap().contains("RTCPeerConnection"));
        let (status, _, body) = call(&app, "GET", "/calls/livekit", "203.0.113.7").await;
        assert_eq!(status, StatusCode::OK);
        assert!(String::from_utf8(body).unwrap().contains(r#"const LIVEKIT_SERVER = "";"#));
    }

    #[tokio::test]
    async fn gives_a_livekit_token_at_home_only_and_says_when_there_is_no_livekit() {
        let none = app(&[]);
        let (status, _, body) = call(&none, "GET", "/calls/livekit/token?name=Мама", "192.168.1.20").await;
        assert_eq!(
            (status, json(&body)),
            (StatusCode::NOT_FOUND, json!({"error": "no LiveKit: set it up in the panel"}))
        );

        let app = app(&[
            ("LIVEKIT_URL", "ws://127.0.0.1:7880"),
            ("LIVEKIT_PUBLIC_URL", "wss://lk.example"),
            ("LIVEKIT_API_KEY", "key"),
            ("LIVEKIT_API_SECRET", "a-secret-of-thirty-two-characters!!"),
        ]);
        let (status, _, body) =
            call(&app, "GET", "/calls/livekit/token?name=%20%D0%9C%D0%B0%D0%BC%D0%B0%20", "192.168.1.20").await;
        assert_eq!(status, StatusCode::OK);
        let body = json(&body);
        assert_eq!(body["url"], "wss://lk.example");
        let payload = body["token"].as_str().unwrap().split('.').nth(1).unwrap().to_string();
        use base64::Engine as _;
        let claims: Value =
            serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
        assert_eq!(
            (claims["name"].as_str(), claims["sub"].as_str()),
            (Some("Мама"), Some(app_identity("Мама").as_str()))
        );
        let (status, _, body) = call(&app, "GET", "/calls/livekit/token", "203.0.113.7").await;
        assert_eq!(
            (status, json(&body)["message"].as_str()),
            (StatusCode::FORBIDDEN, Some("Только из домашней сети."))
        );
        let (_, _, page) = call(&app, "GET", "/calls/livekit", "203.0.113.7").await;
        assert!(String::from_utf8(page).unwrap().contains(r#"const LIVEKIT_SERVER = "wss://lk.example";"#));
    }

    #[tokio::test]
    async fn tells_the_calls_status_to_someone_signed_in_only() {
        let app = app(&[]);
        let (status, _, body) = call(&app, "GET", "/calls/status", "127.0.0.1").await;
        assert_eq!((status, json(&body)["message"].as_str()), (StatusCode::UNAUTHORIZED, Some("Нужно войти.")));
    }

    #[tokio::test]
    async fn answers_the_station_clients_routes_for_an_approved_station_at_home_only() {
        let closed = app(&[]);
        let (status, _, body) = call(&closed, "POST", "/calls/release/default", "192.168.1.23").await;
        assert_eq!((status, json(&body)), (StatusCode::FORBIDDEN, json!({"error": "only an approved station"})));
        let (status, _, body) = call(&closed, "GET", "/calls/next/default", "203.0.113.7").await;
        assert_eq!(
            (status, json(&body)["message"].as_str()),
            (StatusCode::FORBIDDEN, Some("Только из домашней сети."))
        );

        let open = app(&[("STATION_PAIRING", "open")]);
        let (status, headers, body) = call(&open, "GET", "/calls/listen/default?format=ogg", "192.168.1.23").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CONTENT_TYPE], "audio/ogg");
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        assert_eq!(&body[..4], b"OggS", "no call on: the headers and a moment of silence, then the end");
        assert_eq!(open.calls.stations.device_at("192.168.1.23").as_deref(), Some("default"));
        let (status, _, _) = call(&open, "POST", "/calls/release/default", "192.168.1.23").await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(open.calls.hub.endpoint("station:default").is_none());
    }

    #[tokio::test]
    async fn carries_a_call_from_the_poll_through_the_listening_stream_and_the_talk_upload() {
        let app = app(&[("STATION_PAIRING", "open")]);
        let phone = crate::calls::call_hub::tests::TestParty::new(
            "livekit:app-mom",
            "Мама",
            crate::calls::call_hub::EndpointKind::Livekit,
        );
        app.calls.hub.register(phone.clone());
        let poll = tokio::spawn({
            let app = app.clone();
            async move { call(&app, "GET", "/calls/next/default", "192.168.1.23").await }
        });
        while app.calls.hub.endpoint("station:default").is_none() {
            tokio::task::yield_now().await;
        }
        let ringing = app.calls.hub.call(&phone.id, "station:default").unwrap();
        app.calls.hub.accept(&ringing.id, "station:default").unwrap();
        let (status, headers, body) = poll.await.unwrap();
        assert_eq!((status, body.as_slice()), (StatusCode::OK, b"call".as_slice()));
        assert!(headers[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/plain"));

        let mut request = axum::http::Request::builder().uri("/calls/listen/default").body(Body::empty()).unwrap();
        request.extensions_mut().insert(axum::extract::ConnectInfo("192.168.1.23:5000".parse::<SocketAddr>().unwrap()));
        let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/octet-stream");
        let mut body = response.into_body().into_data_stream();
        app.calls.hub.audio(&phone.id, &[7; 320]);
        let chunk = body.next().await.unwrap().unwrap();
        assert_eq!(chunk.len(), 640);

        // Three bytes, then three: the sample split between them is whole again.
        let upload = Body::from_stream(futures::stream::iter([
            Ok::<_, Infallible>(bytes::Bytes::from_static(&[1, 0, 2])),
            Ok(bytes::Bytes::from(vec![0u8; 637])),
        ]));
        let mut request =
            axum::http::Request::builder().method("POST").uri("/calls/talk/default").body(upload).unwrap();
        request.extensions_mut().insert(axum::extract::ConnectInfo("192.168.1.23:5000".parse::<SocketAddr>().unwrap()));
        let response = crate::app::router(app.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(phone.frames().len(), 1);
        assert_eq!(&phone.frames()[0][..2], &[1, 2]);

        app.calls.hub.hangup(&ringing.id, &phone.id).unwrap();
        assert!(body.next().await.is_none(), "the stream ends with the call");
    }

    async fn serve(app: &Arc<App>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = app.clone();
        tokio::spawn(async move { crate::app::serve(app, listener, std::future::pending()).await.unwrap() });
        format!("ws://{address}{CALLS_WS_PATH}")
    }

    type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

    async fn connect(url: &str, hello: Value) -> Socket {
        let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        socket.send(Message::Text(hello.to_string().into())).await.unwrap();
        socket
    }

    /// The next text event of a type, past the others.
    async fn event(socket: &mut Socket, kind: &str) -> Value {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap();
            if let Message::Text(text) = message {
                let event: Value = serde_json::from_str(text.as_str()).unwrap();
                if event["type"] == kind {
                    return event;
                }
            }
        }
    }

    async fn binary(socket: &mut Socket) -> Vec<u8> {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap();
            if let Message::Binary(bytes) = message {
                return bytes.to_vec();
            }
        }
    }

    #[tokio::test]
    async fn connects_the_simulator_and_another_station_client_through_the_hub_with_audio_both_ways() {
        let app = app(&[("STATION_PAIRING", "open")]);
        let url = serve(&app).await;
        let mut sim = connect(&url, json!({"type": "hello", "role": "sim", "device": "default"})).await;
        let welcome = event(&mut sim, "welcome").await;
        assert_eq!((welcome["id"].as_str(), welcome["name"].as_str()), (Some("station:default"), Some("Колонка")));
        let mut kitchen =
            connect(&url, json!({"type": "hello", "role": "station", "device": "kitchen", "name": "Кухня"})).await;
        event(&mut kitchen, "welcome").await;
        assert_eq!(event(&mut sim, "endpoints").await["endpoints"].as_array().unwrap().len(), 2);

        kitchen.send(Message::Text(json!({"type": "call", "to": "station:default"}).to_string().into())).await.unwrap();
        let incoming = event(&mut sim, "incoming").await;
        assert_eq!(incoming["peer"]["name"], "Кухня");
        sim.send(Message::Text(json!({"type": "accept", "callId": incoming["callId"]}).to_string().into()))
            .await
            .unwrap();
        event(&mut kitchen, "connected").await;

        sim.send(Message::Binary(vec![1u8; 640].into())).await.unwrap();
        assert_eq!(binary(&mut kitchen).await.len(), 640);
        kitchen.send(Message::Binary(vec![2u8; 640].into())).await.unwrap();
        assert_eq!(binary(&mut sim).await, vec![2u8; 640]);

        kitchen
            .send(Message::Text(json!({"type": "hangup", "callId": incoming["callId"]}).to_string().into()))
            .await
            .unwrap();
        assert_eq!(event(&mut sim, "ended").await["reason"], "hung up");
        kitchen.close(None).await.unwrap();
        app.calls.stop();
    }

    #[tokio::test]
    async fn refuses_what_is_not_ours_on_the_calls_socket() {
        let app = app(&[]);
        let url = serve(&app).await;
        let mut socket = connect(&url, json!({"type": "call", "to": "x"})).await;
        assert_eq!(event(&mut socket, "error").await["message"], "say hello first");
        socket.send(Message::Text("{oops".into())).await.unwrap();
        assert_eq!(event(&mut socket, "error").await["message"], "not JSON");
        socket.send(Message::Text(json!({"type": "hello", "role": "station"}).to_string().into())).await.unwrap();
        assert_eq!(event(&mut socket, "error").await["message"], "only an approved station");
        if cfg!(not(feature = "webrtc")) {
            socket.send(Message::Text(json!({"type": "hello", "name": "Мама"}).to_string().into())).await.unwrap();
            let refused = event(&mut socket, "error").await;
            assert!(refused["message"].as_str().unwrap().contains("not part of this build"));
        }
        socket.send(Message::Text(json!({"type": "hello", "role": "sim"}).to_string().into())).await.unwrap();
        event(&mut socket, "welcome").await;
        socket.send(Message::Text(json!({"type": "hello", "role": "sim"}).to_string().into())).await.unwrap();
        assert_eq!(event(&mut socket, "error").await["message"], "already introduced");
        socket.send(Message::Text(json!({"type": "accept"}).to_string().into())).await.unwrap();
        assert_eq!(event(&mut socket, "error").await["message"], "no such call ringing");
        socket.send(Message::Text(json!({"type": "dance"}).to_string().into())).await.unwrap();
        assert_eq!(event(&mut socket, "error").await["message"], "unknown message dance");
        socket.send(Message::Text(json!({"type": "offer", "sdp": "v=0"}).to_string().into())).await.unwrap();
        assert_eq!(event(&mut socket, "error").await["message"], "only the call page sends offers");

        // Another site's page, and anything through a tunnel, get no socket at all.
        let mut foreign = url.clone().into_client_request().unwrap();
        foreign.headers_mut().insert("origin", "https://evil.example".parse().unwrap());
        assert!(tokio_tungstenite::connect_async(foreign).await.is_err());
        let mut tunnel = url.clone().into_client_request().unwrap();
        tunnel.headers_mut().insert("cf-ray", "8f00000000000000-WAW".parse().unwrap());
        assert!(tokio_tungstenite::connect_async(tunnel).await.is_err());
        let mut own = url.clone().into_client_request().unwrap();
        let host = own.headers()["host"].to_str().unwrap().to_string();
        own.headers_mut().insert("origin", format!("http://{host}").parse().unwrap());
        assert!(tokio_tungstenite::connect_async(own).await.is_ok());
        app.calls.stop();
    }

    #[cfg(feature = "webrtc")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connects_a_browser_and_the_station_through_the_hub_over_webrtc_audio_both_ways() {
        use std::sync::atomic::Ordering;

        use crate::calls::webrtc_endpoint::tests::{until, TestPage};

        let app = app(&[]);
        let url = serve(&app).await;
        let mut station =
            connect(&url, json!({"type": "hello", "role": "sim", "device": "default", "name": "Колонка"})).await;
        event(&mut station, "welcome").await;
        let mut browser = connect(&url, json!({"type": "hello", "role": "browser", "name": "Мама"})).await;
        event(&mut browser, "welcome").await;
        browser.send(Message::Text(json!({"type": "call", "to": "station:default"}).to_string().into())).await.unwrap();
        let incoming = event(&mut station, "incoming").await;
        assert_eq!(incoming["peer"]["name"], "Мама");
        station
            .send(Message::Text(json!({"type": "accept", "callId": incoming["callId"]}).to_string().into()))
            .await
            .unwrap();
        event(&mut browser, "connected").await;

        // An offer it cannot take is answered with an error, and the socket goes on.
        browser.send(Message::Text(json!({"type": "offer", "sdp": "garbage"}).to_string().into())).await.unwrap();
        assert!(!event(&mut browser, "error").await["message"].as_str().unwrap_or_default().is_empty());

        // The "browser": webrtc-rs on our side of the test, as a page would offer.
        let mut page = TestPage::new().await;
        browser.send(Message::Text(json!({"type": "offer", "sdp": page.offer}).to_string().into())).await.unwrap();
        let answer = event(&mut browser, "answer").await;
        page.connect(answer["sdp"].as_str().unwrap()).await;

        // The station speaks: PCM frames over its socket reach the browser as Opus RTP.
        let tone: Vec<i16> = (0..320).map(|i| ((i as f64 * 0.2).sin() * 6000.0) as i16).collect();
        for _ in 0..50 {
            station.send(Message::Binary(crate::calls::call_hub::pcm_to_bytes(&tone).into())).await.unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        until("the browser hears the station", || page.heard.load(Ordering::Relaxed) >= 10).await;

        // The browser speaks: Opus RTP reaches the station as PCM frames.
        page.speak().await;
        assert_eq!(binary(&mut station).await.len(), 640);

        browser
            .send(Message::Text(json!({"type": "hangup", "callId": incoming["callId"]}).to_string().into()))
            .await
            .unwrap();
        assert_eq!(event(&mut station, "ended").await["reason"], "hung up");
        let _ = page.connection.close().await;
        app.calls.stop();
    }

    #[tokio::test]
    async fn refuses_a_frame_over_a_megabyte_by_its_header_before_buffering_it() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let app = app(&[]);
        let url = serve(&app).await;
        let authority = url.trim_start_matches("ws://").trim_end_matches(CALLS_WS_PATH).to_string();
        let mut tcp = tokio::net::TcpStream::connect(&authority).await.unwrap();
        let handshake = format!(
            "GET {CALLS_WS_PATH} HTTP/1.1\r\nHost: {authority}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        tcp.write_all(handshake.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        while !response.ends_with(b"\r\n\r\n") {
            let mut byte = [0u8; 1];
            tcp.read_exact(&mut byte).await.unwrap();
            response.push(byte[0]);
        }
        assert!(String::from_utf8_lossy(&response).starts_with("HTTP/1.1 101"));

        // A binary frame announcing 2 MiB, masked as a client's, and only the start of it.
        let mut frame = vec![0x82, 0x80 | 127];
        frame.extend_from_slice(&(2u64 * 1024 * 1024).to_be_bytes());
        frame.extend_from_slice(&[1, 2, 3, 4]);
        frame.extend_from_slice(&[0u8; 4096]);
        tcp.write_all(&frame).await.unwrap();
        // The socket closes now; with the frame waited for, the read would wait for the rest of the 2 MiB.
        let mut rest = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(5), tcp.read_to_end(&mut rest)).await;
        assert!(read.is_ok(), "the gateway waited for the rest of the frame");
        app.calls.stop();
    }

    #[test]
    fn takes_only_our_own_page_for_the_socket() {
        assert!(same_origin(None, Some("192.168.1.10:8080")));
        assert!(same_origin(Some("http://192.168.1.10:8080"), Some("192.168.1.10:8080")));
        assert!(same_origin(Some("https://gw.local"), Some("gw.local")));
        assert!(!same_origin(Some("https://gw.local:443"), Some("gw.local:443")), "the default port is no part of it");
        assert!(!same_origin(Some("https://evil.example"), Some("192.168.1.10:8080")));
        assert!(!same_origin(Some("not a url"), Some("192.168.1.10")));
        assert!(!same_origin(Some("http://192.168.1.10"), None));
    }
}
