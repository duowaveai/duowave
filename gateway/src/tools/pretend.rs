//! `duowave pretend`: the voice of pretend stations (docs/plans/pretend-stations.md) — what quasar and Yandex are to a
//! real one, in one terminal, for the owner's hand checks of a home with several stations on one Mac.
//!
//!   duowave pretend --station Kitchen=10.77.0.2:8790 [--station Bedroom=10.77.0.3:8790 …] [--log Kitchen=<gateway.log>]
//!                   [--yandex-port 17700] [--say]
//!
//! - a pretend Yandex on `127.0.0.1:<yandex-port>/uni.ws` (each pretend station's YANDEX_UNIPROXY_URL): it answers the
//!   station's handshake, a typed phrase with «Yandex: …», and every phrase to say with a short sound;
//! - a pretend quasar per station: the voice socket to its gateway, from 127.0.0.1 as quasar on the station opens it
//!   (the gateway takes it for its own station); opened again 3 s after it closes;
//! - a line typed here — `Bedroom: what's the weather` (or `2: …`, by number) — goes to that station as a phrase typed on it;
//!   the texts of its answers come back here;
//! - what a station «says» (a join code, src/stations/pretend.rs) comes from its gateway's log;
//! - `--say`: the answers aloud with the Mac's voice too.
//!
//! Its frames are the station's own (the codec of src/gateway/protocol), as the tests' station sends them.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Context as _;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use super::{flag, option, Usage};
use crate::gateway::protocol::aapi_codec::AapiCodec;
use crate::gateway::protocol::proto_object::value_bytes;
use crate::gateway::protocol::protocol_types::{
    AliceResponseModel, ClientEvent, ClientMessage, CloseReason, ServerMessage,
};
use crate::speech::test_audio::{ogg_opus_chain, ogg_pages, voice};

const USAGE: &str = "usage: duowave pretend --station <name>=<address>:<port> [--station …] [--log <name>=<file>] \
                     [--yandex-port 17700] [--say]";
/// What a pretend station's log line starts with when it says something (src/stations/pretend.rs, SAYS).
const SAYS: &str = "🔊 ";

/// A pretend station as the command line names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Station {
    pub name: String,
    pub address: SocketAddr,
    pub log: Option<PathBuf>,
}

impl Station {
    /// Its quasar's device id: the same at every start, so its gateway knows it again.
    fn device(&self) -> String {
        match self.address.ip() {
            IpAddr::V4(ip) => format!("PRETEND{:03}", ip.octets()[3]),
            IpAddr::V6(_) => "PRETEND000".to_string(),
        }
    }
}

/// `Kitchen=10.77.0.2:8790` → the station.
pub fn parse_station(text: &str) -> Result<Station, String> {
    let (name, address) = text.split_once('=').ok_or_else(|| format!("--station {text}: <name>=<address>:<port>"))?;
    let address: SocketAddr = address.trim().parse().map_err(|_| format!("--station {text}: <address>:<port>"))?;
    let name = name.trim();
    if name.is_empty() {
        return Err(format!("--station {text}: a name first"));
    }
    Ok(Station { name: name.to_string(), address, log: None })
}

/// Every value of `--<name>`, in order.
fn options<'a>(args: &'a [String], name: &str) -> Vec<&'a str> {
    let flag = format!("--{name}");
    args.windows(2).filter(|pair| pair[0] == flag).map(|pair| pair[1].as_str()).collect()
}

/// The station a typed line is for, and its phrase: `Bedroom: …` by name (any case), `2: …` by number.
pub fn addressed<'a>(line: &'a str, stations: &[Station]) -> Option<(usize, &'a str)> {
    let (who, phrase) = line.split_once(':')?;
    let (who, phrase) = (who.trim(), phrase.trim());
    if phrase.is_empty() {
        return None;
    }
    if let Ok(number) = who.parse::<usize>() {
        return (1..=stations.len()).contains(&number).then_some((number - 1, phrase));
    }
    let who = who.to_lowercase();
    stations.iter().position(|station| station.name.to_lowercase() == who).map(|index| (index, phrase))
}

/// The texts an answer frame carries: `text` and `speech` of the response, also inside a JSON payload.
pub fn texts(value: &Value) -> Vec<String> {
    let mut found = Vec::new();
    walk(value, &mut found);
    found.dedup();
    found
}

fn walk(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                match inner {
                    Value::String(text) if matches!(key.as_str(), "text" | "speech" | "outputSpeech") => {
                        let text = text.trim();
                        if !text.is_empty() && !found.iter().any(|known| known == text) {
                            found.push(text.to_string());
                        }
                    }
                    Value::String(text) if text.starts_with('{') => {
                        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                            walk(&parsed, found);
                        }
                    }
                    _ => walk(inner, found),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| walk(item, found)),
        _ => {}
    }
}

pub fn main(args: &[String]) -> anyhow::Result<()> {
    if flag(args, "help") || args.is_empty() {
        return Err(Usage(USAGE.into()).into());
    }
    let mut stations = Vec::new();
    for text in options(args, "station") {
        stations.push(parse_station(text).map_err(Usage)?);
    }
    if stations.is_empty() {
        return Err(Usage(USAGE.into()).into());
    }
    for text in options(args, "log") {
        let (name, file) = text.split_once('=').ok_or_else(|| Usage(format!("--log {text}: <name>=<file>")))?;
        let station = stations
            .iter_mut()
            .find(|station| station.name == name.trim())
            .ok_or_else(|| Usage(format!("--log {text}: no --station {name}")))?;
        station.log = Some(PathBuf::from(file.trim()));
    }
    let yandex_port = match option(args, "yandex-port") {
        Some(port) => port.parse::<u16>().map_err(|_| Usage(format!("--yandex-port {port}: a port")))?,
        None => 17700,
    };
    let say = flag(args, "say");
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(run(stations, yandex_port, say))
}

/// What goes to the owner's terminal, and with `--say` to the Mac's voice.
#[derive(Clone)]
struct Out {
    say: bool,
}

impl Out {
    fn line(&self, who: &str, text: &str) {
        println!("{who}: {text}");
    }

    fn answer(&self, who: &str, text: &str) {
        println!("💬 {who}: {text}");
        if self.say {
            let text = text.to_string();
            tokio::spawn(async move {
                let _ = tokio::process::Command::new("say").arg(text).status().await;
            });
        }
    }
}

async fn run(stations: Vec<Station>, yandex_port: u16, say: bool) -> anyhow::Result<()> {
    let out = Out { say };
    let yandex = TcpListener::bind((Ipv4Addr::LOCALHOST, yandex_port))
        .await
        .with_context(|| format!("the pretend Yandex: 127.0.0.1:{yandex_port} is taken"))?;
    tokio::spawn(serve_yandex(yandex, out.clone()));
    println!("Pretend Yandex: ws://127.0.0.1:{yandex_port}/uni.ws");
    let mut senders = Vec::new();
    for station in &stations {
        let (send, phrases) = mpsc::unbounded_channel::<String>();
        senders.push(send);
        tokio::spawn(quasar(station.clone(), phrases, out.clone()));
        if let Some(log) = &station.log {
            tokio::spawn(follow(log.clone()));
        }
    }
    let names: Vec<String> =
        stations.iter().enumerate().map(|(index, station)| format!("{} {}", index + 1, station.name)).collect();
    println!(
        "Stations: {}. A phrase to a station: «Name: phrase» or «number: phrase»; quit with Ctrl+C.",
        names.join(", ")
    );
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Ok(Some(line)) = line else {
                    // No terminal (run in the background): the sockets stay up until Ctrl+C.
                    tokio::signal::ctrl_c().await?;
                    return Ok(());
                };
                if line.trim().is_empty() {
                    continue;
                }
                match addressed(&line, &stations) {
                    Some((index, phrase)) => {
                        let _ = senders[index].send(phrase.to_string());
                    }
                    None => println!("To whom? «Name: phrase» or «number: phrase» — {}", names.join(", ")),
                }
            }
            _ = tokio::signal::ctrl_c() => return Ok(()),
        }
    }
}

/// A pretend quasar: the station's voice socket to its gateway, opened again after it closes; typed phrases go up
/// as TextInput, the texts of the answers come down to the terminal.
async fn quasar(station: Station, mut phrases: mpsc::UnboundedReceiver<String>, out: Out) {
    let codec = AapiCodec::new();
    let counter = AtomicU64::new(1);
    let mut said_down = false;
    loop {
        match open(&station).await {
            Ok(socket) => {
                said_down = false;
                out.line(&station.name, "voice connection to the gateway open");
                let (mut sink, mut incoming) = socket.split();
                let id = counter.fetch_add(1, Ordering::Relaxed);
                let hello = json!({ "event": { "header": { "messageId": format!("pretend-sync-{id}") },
                    "synchronizeStateRequest": { "application": { "deviceModel": "pretend", "timezone": "UTC" } } } });
                if let Ok(frame) = codec.encode_client_object(&hello) {
                    let _ = sink.send(Message::Binary(frame)).await;
                }
                let mut ping = tokio::time::interval(Duration::from_secs(5));
                loop {
                    tokio::select! {
                        phrase = phrases.recv() => {
                            let Some(phrase) = phrase else { return };
                            let id = counter.fetch_add(1, Ordering::Relaxed);
                            let text = json!({ "event": {
                                "header": { "messageId": format!("pretend-text-{id}") },
                                "textInput": { "header": { "requestId": format!("pretend-req-{id}") },
                                    "request": { "event": { "type": "text_input", "text": phrase } } },
                            } });
                            if let Ok(frame) = codec.encode_client_object(&text) {
                                if sink.send(Message::Binary(frame)).await.is_err() {
                                    break;
                                }
                            }
                        }
                        _ = ping.tick() => {
                            if sink.send(Message::Ping(Bytes::new())).await.is_err() {
                                break;
                            }
                        }
                        next = incoming.next() => {
                            let Some(Ok(message)) = next else { break };
                            let Message::Binary(data) = message else { continue };
                            if !AapiCodec::is_aapi_frame(&data) {
                                continue;
                            }
                            let Ok(decoded) = codec.decode_server_frame(&data) else { continue };
                            if let Some(callback) = callback_for(&codec, &decoded) {
                                let _ = sink.send(Message::Binary(callback)).await;
                                continue;
                            }
                            for text in texts(&decoded) {
                                out.answer(&station.name, &text);
                            }
                        }
                    }
                }
                out.line(&station.name, "voice connection closed; again in 3 s");
            }
            Err(error) if !said_down => {
                said_down = true;
                out.line(
                    &station.name,
                    &format!("the gateway does not take the voice connection ({error}); trying every 3 s"),
                );
            }
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

type StationSocket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The voice socket, from 127.0.0.1 — as quasar opens it on the station itself.
async fn open(station: &Station) -> anyhow::Result<StationSocket> {
    let url = format!("ws://{}/uni.ws", station.address);
    let mut request = url.into_client_request()?;
    request.headers_mut().insert("x-uprx-device-id", station.device().parse()?);
    let local = if station.address.is_ipv4() { IpAddr::from(Ipv4Addr::LOCALHOST) } else { "::1".parse()? };
    let stream = tokio::time::timeout(Duration::from_secs(5), crate::utils::net::dial_from(local, station.address))
        .await
        .context("no answer in 5 s")??;
    let (socket, _) =
        tokio_tungstenite::client_async(request, tokio_tungstenite::MaybeTlsStream::Plain(stream)).await?;
    Ok(socket)
}

/// The gateway's own command (a server_action `vgw_…` push): the station calls back with it, as the real one does.
fn callback_for(codec: &AapiCodec, decoded: &Value) -> Option<Bytes> {
    let push = decoded.get("event")?.get("push")?;
    let push = codec.decode_push(&value_bytes(push)?).ok()?;
    let directive = push.get("directive")?;
    let name = directive.get("name")?.as_str()?;
    if directive.get("type")?.as_str()? != "server_action" || !name.starts_with("vgw_") {
        return None;
    }
    let id = push.get("id").and_then(Value::as_str).unwrap_or_default();
    let callback = json!({ "event": {
        "header": { "messageId": format!("callback-{id}") },
        "textInput": {
            "header": { "requestId": format!("req-callback-{id}") },
            "request": { "event": { "type": "server_action", "name": name, "payload": directive.get("payload") } },
        },
    } });
    codec.encode_client_object(&callback).ok()
}

/// The lines a pretend station «says» (its gateway's log): shown as they come.
async fn follow(log: PathBuf) {
    let mut at = tokio::fs::metadata(&log).await.map(|meta| meta.len()).unwrap_or(0);
    let mut rest = String::new();
    loop {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let Ok(mut file) = tokio::fs::File::open(&log).await else { continue };
        let len = file.metadata().await.map(|meta| meta.len()).unwrap_or(0);
        if len < at {
            at = 0;
        }
        if len == at || file.seek(std::io::SeekFrom::Start(at)).await.is_err() {
            continue;
        }
        let mut chunk = Vec::new();
        if file.read_to_end(&mut chunk).await.is_err() {
            continue;
        }
        at += chunk.len() as u64;
        rest.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = rest.find('\n') {
            let line: String = rest.drain(..=end).collect();
            if let Some(said) = line.trim_end().split_once(SAYS).map(|(_, said)| said) {
                println!("{SAYS}{said}");
            }
        }
    }
}

/// The pretend Yandex: each gateway's relayed or own connection.
async fn serve_yandex(listener: TcpListener, out: Out) {
    let connections = AtomicU32::new(0);
    while let Ok((stream, _)) = listener.accept().await {
        let number = connections.fetch_add(1, Ordering::Relaxed) + 1;
        let out = out.clone();
        tokio::spawn(async move {
            let Ok(socket) = tokio_tungstenite::accept_async(stream).await else { return };
            yandex_connection(socket, format!("Yandex #{number}"), out).await;
        });
    }
}

async fn yandex_connection(socket: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, who: String, out: Out) {
    let codec = AapiCodec::new();
    let (mut sink, mut incoming) = socket.split();
    let mut next_stream = 1u32;
    while let Some(Ok(message)) = incoming.next().await {
        let Message::Binary(frame) = message else { continue };
        if !AapiCodec::is_aapi_frame(&frame) {
            continue;
        }
        let Ok(decoded) = codec.decode_client(&frame) else { continue };
        let mut sends: Vec<Bytes> = Vec::new();
        let encode = |message: &ServerMessage| codec.encode_server(message).ok().map(|encoded| encoded.frame);
        match decoded.message {
            ClientMessage::Event(ClientEvent::SynchronizeState { header, .. }) => {
                sends.extend(encode(&ServerMessage::SynchronizeStateResponse {
                    r#ref: header.message_id,
                    session_id: "pretend-yandex".into(),
                }));
            }
            ClientMessage::Event(ClientEvent::TextInput { header, request }) => {
                let Some(text) = request.event.text.clone().filter(|_| request.event.r#type.as_str() == "text_input")
                else {
                    continue;
                };
                let speech = format!("Yandex: you said «{text}».");
                let response = AliceResponseModel {
                    request_id: request.request_id.clone().filter(|id| !id.is_empty()).unwrap_or_default(),
                    response_id: format!("pretend-response-{}", header.message_id),
                    text: Some(speech.clone()),
                    speech: Some(speech),
                    ..Default::default()
                };
                sends.extend(encode(&ServerMessage::AliceResponse { r#ref: header.message_id.clone(), response }));
                sends.extend(speech_of(&codec, &header.message_id, next_stream));
                next_stream += 2;
            }
            ClientMessage::Event(ClientEvent::TtsGenerate { header, text, .. }) => {
                out.line(&who, &format!("voices «{text}»"));
                sends.extend(speech_of(&codec, &header.message_id, next_stream));
                next_stream += 2;
            }
            _ => {}
        }
        for frame in sends {
            if sink.send(Message::Binary(frame)).await.is_err() {
                return;
            }
        }
    }
}

/// Speech on a stream: TtsSpeak, 300 ms of Ogg/Opus in frames of five pages, the stream's close.
fn speech_of(codec: &AapiCodec, r#ref: &str, stream_id: u32) -> Vec<Bytes> {
    let encode = |message: &ServerMessage| codec.encode_server(message).ok().map(|encoded| encoded.frame);
    let mut frames: Vec<Bytes> = Vec::new();
    frames.extend(encode(&ServerMessage::TtsSpeak {
        r#ref: r#ref.to_string(),
        stream_id,
        format: "audio/opus".into(),
    }));
    let chain = ogg_opus_chain(&voice(300.0, -20.0), 0x5eed);
    for pages in ogg_pages(&chain).chunks(5) {
        let mut frame = stream_id.to_be_bytes().to_vec();
        frame.extend_from_slice(&pages.concat());
        frames.push(Bytes::from(frame));
    }
    frames.extend(encode(&ServerMessage::StreamClose { stream_id, reason: CloseReason::Ok }));
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three() -> Vec<Station> {
        ["Kitchen=10.77.0.2:8790", "Bedroom=10.77.0.3:8790", "Nursery=10.77.0.4:8790"]
            .iter()
            .map(|text| parse_station(text).unwrap())
            .collect()
    }

    #[test]
    fn reads_the_stations_and_who_a_line_is_for() {
        let stations = three();
        assert_eq!(stations[1].address, "10.77.0.3:8790".parse().unwrap());
        assert_eq!(stations[1].device(), "PRETEND003");
        assert_eq!(addressed("bedroom: what's the weather", &stations), Some((1, "what's the weather")));
        assert_eq!(addressed("3: stop", &stations), Some((2, "stop")));
        assert_eq!(addressed("4: stop", &stations), None, "no fourth");
        assert_eq!(addressed("what is the weather", &stations), None, "to whom?");
        assert_eq!(addressed("Kitchen:", &stations), None, "no phrase");
        assert!(parse_station("Kitchen").is_err());
        assert!(parse_station("=10.77.0.2:8790").is_err());
    }

    #[test]
    fn finds_the_answers_texts_inside_a_payload_too() {
        let frame = json!({ "event": { "vinsResponse": {
            "payload": "{\"response\":{\"card\":{\"text\":\"Sunny\"}},\"voice_response\":{\"outputSpeech\":\"Sunny\"}}"
        } } });
        assert_eq!(texts(&frame), vec!["Sunny".to_string()]);
        assert!(texts(&json!({ "event": { "header": { "messageId": "x" } } })).is_empty());
    }

    /// The pretend Yandex answers the station's handshake and a typed phrase as Yandex does, speech and all.
    #[tokio::test]
    async fn the_pretend_yandex_answers_the_handshake_and_a_phrase() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(serve_yandex(listener, Out { say: false }));
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/uni.ws")).await.unwrap();
        let codec = AapiCodec::new();
        let hello = json!({ "event": { "header": { "messageId": "sync-1" }, "synchronizeStateRequest": {
            "application": { "deviceModel": "pretend" } } } });
        socket.send(Message::Binary(codec.encode_client_object(&hello).unwrap())).await.unwrap();
        let text = json!({ "event": { "header": { "messageId": "text-1" }, "textInput": {
            "header": { "requestId": "req-1" }, "request": { "event": { "type": "text_input", "text": "hello" } } } } });
        socket.send(Message::Binary(codec.encode_client_object(&text).unwrap())).await.unwrap();
        let mut heard = Vec::new();
        let mut audio = 0;
        while let Ok(Some(Ok(Message::Binary(frame)))) =
            tokio::time::timeout(Duration::from_millis(1500), socket.next()).await
        {
            if AapiCodec::is_aapi_frame(&frame) {
                heard.push(codec.decode_server_frame(&frame).unwrap());
            } else {
                audio += 1;
            }
            if heard.iter().any(|frame| frame.to_string().contains("streamClose")) {
                break;
            }
        }
        let all = Value::Array(heard.clone()).to_string();
        assert!(all.contains("sync-1"), "the handshake answered: {all}");
        assert!(heard.iter().flat_map(texts).any(|said| said.contains("hello")), "{all}");
        assert!(audio > 0, "speech came: {all}");
    }
}
