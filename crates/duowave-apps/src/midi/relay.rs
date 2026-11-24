//! The notes' channel through the centre (docs/plans/turbo-devices.md, «Ноты через центр»): a page at the centre whose
//! keyboards are at its Турбо — or at the centre and a Турбо both — hears and plays them through one socket of the
//! same frames as a page's own (socket.rs). The page notices nothing but the keyboards' names: «Keystation 49 · Mac».
//!
//! - **Legs.** The relay keeps one leg per place: this gateway's own service (an owner of the app, as a page's socket
//!   would be), and each Турбо at home by its own channel `/home/v1/devices/midi/stream` (the centre's owner there,
//!   src/home_server/devices.rs) — by the home network only: the link carries no WebSocket, and a thing at hand is the
//!   home's only at home (owner 20.11, 6).
//! - **Ids name their place**: a Турбо's `in-2` is `home-computer:in-2` to the page (src/home/turbo_devices.rs), this
//!   gateway's own stay bare. What the page listens to or plays goes to the leg its id names; `listen *` and `stop` to
//!   all; a melody with no keyboard named to the place the home chose.
//! - **Look everywhere** (owner 20.11, 1): the notes of every leg come to the page in one numbered stream (`s`); the
//!   times (`at`) are put on the centre's clock by each leg's offset at its hello.
//! - **Renewal** as a page's socket: 15 minutes, `renew` before the end; a new socket's `resume` carries the old one's
//!   token, and each leg takes its old owner over by that leg's own token — nothing is let go on the joint. A Турбо's
//!   channel renews itself in its leg.
//! - **The centre's limits**: at most `MAX_RELAYS` at once (the station has little memory); a leg that falls behind
//!   loses notes and says so, as a page does.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message as WsMessage, WebSocket, WebSocketUpgrade};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as UpMessage;
use tokio_util::sync::CancellationToken;

use super::socket::{self, LIFETIME, READ_LIMIT, RENEW_BEFORE};
use super::wire::Message;
use super::MidiService;
use crate::bus::providers::devices::{home_id, split_id, HERE};
use crate::platform::http::Zone;

/// Relays at once on this gateway: each holds a socket to every Турбо at home with a keyboard.
pub const MAX_RELAYS: usize = 4;
/// A place's hello is waited for this long before the page hears its own.
const HELLO_WAIT: Duration = Duration::from_secs(3);
/// A Турбо's channel answers its opening this fast at home, or it is not there.
const CONNECT_WAIT: Duration = Duration::from_secs(5);
/// How often the relay asks whether the app may still hear (as a page's socket does).
const CHECK: Duration = Duration::from_secs(2);
const PING: Duration = Duration::from_secs(20);
const SILENT: Duration = Duration::from_secs(60);
const SEND_WAIT: Duration = Duration::from_secs(10);
/// A relay's resume tokens are kept this long after its start: its lifetime and a page's minute to come back.
const RESUME_KEPT: Duration = Duration::from_secs(16 * 60);

/// May the app's page still hear (the centre's decision: the person, the app's link, the home's keyboards).
pub type Check = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;
/// The place a melody with no keyboard named goes to, and the words of that choice (src/home/turbo_devices.rs).
pub type Choose = Arc<dyn Fn() -> Option<(String, Option<String>)> + Send + Sync>;

/// Where a leg goes.
#[derive(Clone)]
pub enum Way {
    /// This gateway's own keyboards.
    Local(Arc<MidiService>),
    /// A Турбо's channel: `ws://…/home/v1/devices/midi/stream` and its key.
    Remote { url: String, key: String },
}

/// One place the relay hears.
#[derive(Clone)]
pub struct Place {
    /// '' — this gateway; a Турбо's id.
    pub id: String,
    /// For people: «Mac».
    pub name: String,
    pub way: Way,
}

/// One page's relay: its places, its app, its checks.
pub struct Relay {
    pub places: Vec<Place>,
    pub package: String,
    pub zone: Zone,
    pub check: Check,
    pub choose: Choose,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Relays running now.
static RUNNING: Mutex<usize> = Mutex::new(0);
/// A relay's legs' tokens by place, and when it began.
type LegTokens = (Instant, HashMap<String, String>);
/// A relay's token → its legs' tokens: a new socket's `resume` takes them over.
static RESUMES: Mutex<BTreeMap<String, LegTokens>> = Mutex::new(BTreeMap::new());

/// One relay counted while it runs.
pub struct Running;

impl Running {
    fn take() -> Option<Running> {
        let mut running = locked(&RUNNING);
        if *running >= MAX_RELAYS {
            return None;
        }
        *running += 1;
        Some(Running)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let mut running = locked(&RUNNING);
        *running = running.saturating_sub(1);
    }
}

/// The page's socket upgraded with a page's limits, then the relay.
pub fn upgrade(upgrade: WebSocketUpgrade, relay: Relay) -> axum::response::Response {
    upgrade
        .read_buffer_size(READ_LIMIT)
        .max_message_size(READ_LIMIT)
        .max_frame_size(READ_LIMIT)
        .on_upgrade(move |socket| serve(socket, relay))
}

/// One page's socket: its frames to the relay and back, its pings, its life.
pub async fn serve(socket: WebSocket, relay: Relay) {
    let (mut out, mut incoming) = socket.split();
    let Some(_running) = Running::take() else {
        let _ = send(&mut out, &json!({ "t": "why", "text": crate::tr!("Клавиатуру уже слушают несколько страниц. Закройте лишние."), "end": true })).await;
        let _ = out.send(WsMessage::Close(Some(CloseFrame { code: 1008, reason: "full".into() }))).await;
        return;
    };
    let (page_in, from_page) = mpsc::channel::<Value>(64);
    let (to_page, mut page_out) = mpsc::channel::<Value>(256);
    let done = CancellationToken::new();
    let core = tokio::spawn(run(relay, from_page, to_page, done.clone()));
    let start = tokio::time::Instant::now();
    let mut ping = tokio::time::interval_at(start + PING, PING);
    let mut last_heard = Instant::now();
    loop {
        tokio::select! {
            frame = page_out.recv() => match frame {
                Some(frame) => {
                    let end = frame.get("close").and_then(Value::as_str).map(str::to_string);
                    if let Some(reason) = end {
                        let _ = out.send(WsMessage::Close(Some(CloseFrame { code: 1000, reason: reason.into() }))).await;
                        break;
                    }
                    if !send(&mut out, &frame).await {
                        break;
                    }
                }
                None => break,
            },
            frame = incoming.next() => match frame {
                Some(Ok(WsMessage::Text(text))) => {
                    last_heard = Instant::now();
                    let frame = serde_json::from_str::<Value>(&text)
                        .unwrap_or_else(|_| json!({ "t": "not-json" }));
                    if page_in.send(frame).await.is_err() {
                        break;
                    }
                }
                Some(Ok(WsMessage::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => last_heard = Instant::now(),
            },
            _ = ping.tick() => {
                if last_heard.elapsed() > SILENT {
                    break;
                }
                if !matches!(tokio::time::timeout(SEND_WAIT, out.send(WsMessage::Ping(Default::default()))).await, Ok(Ok(()))) {
                    break;
                }
            }
        }
    }
    done.cancel();
    let _ = core.await;
}

type Out = futures::stream::SplitSink<WebSocket, WsMessage>;

async fn send(out: &mut Out, frame: &Value) -> bool {
    let text = serde_json::to_string(frame).unwrap_or_else(|_| "{}".into());
    matches!(tokio::time::timeout(SEND_WAIT, out.send(WsMessage::Text(text.into()))).await, Ok(Ok(())))
}

/// What one leg holds for the relay: its place, its way in, what the page knows of it.
struct Leg {
    place: Place,
    to: mpsc::Sender<Value>,
    hello: bool,
    token: Option<String>,
    devices: Vec<Value>,
    listening: BTreeSet<String>,
    /// The centre's ms minus the place's at its hello: its times put on the centre's clock.
    offset: i64,
}

/// The relay itself, between the page's frames and the legs: the page's side is two channels, so the tests drive it
/// without a socket. A frame `{close: why}` to the page ends its socket.
pub async fn run(
    relay: Relay,
    mut from_page: mpsc::Receiver<Value>,
    to_page: mpsc::Sender<Value>,
    done: CancellationToken,
) {
    let started = Instant::now();
    let now_ms = move || started.elapsed().as_millis() as i64;
    let (up_tx, mut up) = mpsc::channel::<(usize, Value)>(1024);
    let mut legs: Vec<Leg> = Vec::new();
    for (index, place) in relay.places.iter().enumerate() {
        let (to, commands) = mpsc::channel::<Value>(256);
        let leg_done = done.child_token();
        match place.way.clone() {
            Way::Local(service) => {
                tokio::spawn(local_leg(
                    index,
                    service,
                    relay.package.clone(),
                    relay.zone,
                    commands,
                    up_tx.clone(),
                    leg_done,
                ));
            }
            Way::Remote { url, key } => {
                tokio::spawn(remote_leg(index, place.name.clone(), url, key, commands, up_tx.clone(), leg_done));
            }
        }
        legs.push(Leg {
            place: place.clone(),
            to,
            hello: false,
            token: None,
            devices: Vec::new(),
            listening: BTreeSet::new(),
            offset: 0,
        });
    }
    drop(up_tx);
    let token = crate::utils::ids::random_hex(16);
    let several = legs.len() > 1;
    // The page's hello waits for every leg's, or `HELLO_WAIT`.
    let hello_by = tokio::time::Instant::now() + HELLO_WAIT;
    let mut said_hello = false;
    let mut seq: u64 = 0;
    let begun = tokio::time::Instant::now();
    let renew = tokio::time::sleep_until(begun + LIFETIME - RENEW_BEFORE);
    let end = tokio::time::sleep_until(begun + LIFETIME);
    let hello_late = tokio::time::sleep_until(hello_by);
    tokio::pin!(renew, end, hello_late);
    let mut renewed = false;
    let mut check = tokio::time::interval_at(begun + CHECK, CHECK);
    let say = |frame: Value| {
        let to_page = to_page.clone();
        async move { to_page.send(frame).await.is_ok() }
    };
    loop {
        tokio::select! {
            _ = done.cancelled() => break,
            event = up.recv() => {
                let Some((index, frame)) = event else { break };
                let kind = frame.get("t").and_then(Value::as_str).unwrap_or_default().to_string();
                match kind.as_str() {
                    "hello" => {
                        let leg = &mut legs[index];
                        leg.hello = true;
                        leg.token = frame.get("token").and_then(Value::as_str).map(str::to_string);
                        leg.devices = frame.get("devices").and_then(Value::as_array).cloned().unwrap_or_default();
                        leg.offset = now_ms() - frame.get("now").and_then(Value::as_i64).unwrap_or(0);
                        remember(&token, started, &legs);
                        if said_hello {
                            if !say(json!({ "t": "devices", "devices": merged(&legs, several) })).await {
                                break;
                            }
                        } else if legs.iter().all(|leg| leg.hello) {
                            said_hello = true;
                            if !say(json!({ "t": "hello", "devices": merged(&legs, several), "now": now_ms(), "token": token })).await {
                                break;
                            }
                        }
                    }
                    "devices" => {
                        legs[index].devices = frame.get("devices").and_then(Value::as_array).cloned().unwrap_or_default();
                        if said_hello && !say(json!({ "t": "devices", "devices": merged(&legs, several) })).await {
                            break;
                        }
                    }
                    "listening" => {
                        let place = legs[index].place.id.clone();
                        legs[index].listening = frame
                            .get("d")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .map(|port| home_id(&place, port))
                            .collect();
                        let all: BTreeSet<&String> = legs.iter().flat_map(|leg| leg.listening.iter()).collect();
                        if !say(json!({ "t": "listening", "d": all })).await {
                            break;
                        }
                    }
                    // The relay answers the page's pings and resumes itself.
                    "pong" | "resumed" => {}
                    "why" => {
                        let mut frame = frame;
                        let ends = frame.get("end").and_then(Value::as_bool).unwrap_or(false);
                        if several {
                            let text = frame.get("text").and_then(Value::as_str).unwrap_or_default();
                            let named = format!("{}: {text}", legs[index].place.name);
                            frame["text"] = Value::String(named);
                            // One place gone does not end the page's channel while another hears.
                            frame.as_object_mut().map(|fields| fields.remove("end"));
                        }
                        if ends && !several {
                            let _ = say(frame).await;
                            let _ = say(json!({ "close": "gone" })).await;
                            break;
                        }
                        if ends {
                            legs[index].devices.clear();
                            legs[index].listening.clear();
                            let _ = say(json!({ "t": "devices", "devices": merged(&legs, several) })).await;
                        }
                        if !say(frame).await {
                            break;
                        }
                    }
                    _ => {
                        let mut frame = frame;
                        let place = legs[index].place.id.clone();
                        if let Some(Value::String(port)) = frame.get("d") {
                            let named = home_id(&place, port);
                            frame["d"] = Value::String(named);
                        }
                        // Notes: one number for the page, the centre's clock.
                        if frame.get("s").is_some() {
                            seq += 1;
                            frame["s"] = json!(seq);
                            if let Some(at) = frame.get("at").and_then(Value::as_i64) {
                                frame["at"] = json!((at + legs[index].offset).max(0));
                            }
                        }
                        if !say(frame).await {
                            break;
                        }
                    }
                }
            }
            frame = from_page.recv() => {
                let Some(frame) = frame else { break };
                if let Some(answer) = page_frame(&relay, &mut legs, frame, now_ms()).await {
                    if !say(answer).await {
                        break;
                    }
                }
            }
            _ = &mut hello_late, if !said_hello => {
                // A place that does not answer in time is left out; it joins when its hello comes.
                said_hello = true;
                if !say(json!({ "t": "hello", "devices": merged(&legs, several), "now": now_ms(), "token": token })).await {
                    break;
                }
            }
            _ = check.tick() => {
                if let Err(why) = (relay.check)() {
                    let _ = say(json!({ "t": "why", "text": why, "end": true })).await;
                    let _ = say(json!({ "close": "gone" })).await;
                    break;
                }
            }
            _ = &mut renew, if !renewed => {
                renewed = true;
                if !say(json!({ "t": "renew" })).await {
                    break;
                }
            }
            _ = &mut end => {
                let _ = say(json!({ "close": "lifetime" })).await;
                break;
            }
        }
    }
    done.cancel();
}

/// The keyboards of every leg as the page sees them: ids with their place, and — when there are several places — the
/// place's name after each keyboard's («Keystation 49 · Mac»), so the page tells two of one name apart.
fn merged(legs: &[Leg], several: bool) -> Vec<Value> {
    let mut all = Vec::new();
    for leg in legs {
        for device in &leg.devices {
            let mut device = device.clone();
            for key in ["input", "output"] {
                if let Some(Value::String(port)) = device.get(key) {
                    let named = home_id(&leg.place.id, port);
                    device[key] = Value::String(named);
                }
            }
            if several {
                let name = device.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                device["name"] = Value::String(format!("{name} · {}", leg.place.name));
            }
            device["place"] = Value::String(leg.place.id.clone());
            all.push(device);
        }
    }
    all
}

/// The relay's legs' tokens kept under its own, for a new socket's `resume`.
fn remember(token: &str, started: Instant, legs: &[Leg]) {
    let tokens: HashMap<String, String> =
        legs.iter().filter_map(|leg| Some((leg.place.id.clone(), leg.token.clone()?))).collect();
    let mut resumes = locked(&RESUMES);
    resumes.retain(|_, (since, _)| since.elapsed() < RESUME_KEPT);
    resumes.insert(token.to_string(), (started, tokens));
}

/// One frame of the page: answered by the relay, or sent on to the leg it names.
async fn page_frame(relay: &Relay, legs: &mut [Leg], frame: Value, now: i64) -> Option<Value> {
    let kind = frame.get("t").and_then(Value::as_str).unwrap_or_default().to_string();
    let field = |key: &str| frame.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let places: Vec<String> = legs.iter().map(|leg| leg.place.id.clone()).collect();
    let failed = |why: String| Some(json!({ "t": "why", "text": why, "of": kind }));
    // The leg a device's id names, and the frame with the id as that leg knows it.
    let routed = |frame: &Value, id: &str| -> Option<(usize, Value)> {
        let (place, port) = split_id(id, &places);
        let index = legs.iter().position(|leg| leg.place.id == place)?;
        let mut frame = frame.clone();
        frame["d"] = Value::String(port.to_string());
        Some((index, frame))
    };
    match kind.as_str() {
        "ping" => return Some(json!({ "t": "pong", "at": frame.get("at"), "now": now })),
        "not-json" => return failed(crate::tr!("не JSON")),
        "resume" => {
            let old = locked(&RESUMES).remove(&field("token"));
            let Some((_, tokens)) = old else {
                return failed(crate::tr!("Прежний канал уже закрыт."));
            };
            for leg in legs.iter() {
                if let Some(token) = tokens.get(&leg.place.id) {
                    let _ = leg.to.send(json!({ "t": "resume", "token": token })).await;
                }
            }
            return Some(json!({ "t": "resumed" }));
        }
        "listen" | "unlisten" | "stop" if field("d").is_empty() || field("d") == "*" => {
            for leg in legs.iter() {
                let _ = leg.to.send(frame.clone()).await;
            }
            return None;
        }
        "play" if field("d").is_empty() => {
            let chosen = (relay.choose)();
            let place = chosen.as_ref().map(|(place, _)| place.clone()).unwrap_or_else(|| HERE.to_string());
            let index =
                legs.iter().position(|leg| leg.place.id == place).or_else(|| (!legs.is_empty()).then_some(0))?;
            // The place's first keyboard that sounds, by the id it knows.
            let output = legs[index].devices.iter().find_map(|device| device.get("output").and_then(Value::as_str));
            let Some(output) = output.map(str::to_string) else {
                return failed(crate::tr!("Клавиатуры, которая звучит, сейчас нет: её отключили?"));
            };
            let mut frame = frame;
            frame["d"] = Value::String(output);
            let _ = legs[index].to.send(frame).await;
            return chosen.and_then(|(_, said)| said).map(|said| json!({ "t": "chosen", "text": said }));
        }
        _ => {}
    }
    let named = field("d");
    let is_note = Message::from_frame(&frame).is_some();
    if is_note || matches!(kind.as_str(), "listen" | "unlisten" | "play" | "stop") {
        return match routed(&frame, &named) {
            Some((index, frame)) => {
                let _ = legs[index].to.send(frame).await;
                None
            }
            None => failed(crate::tr!("Такой клавиатуры нет: её отключили?")),
        };
    }
    failed(crate::tr!("Кадр «{other}» не знаю.", other = kind))
}

/// This gateway's own keyboards as one leg: an owner of the app, as a page's socket would have.
async fn local_leg(
    index: usize,
    service: Arc<MidiService>,
    package: String,
    zone: Zone,
    mut commands: mpsc::Receiver<Value>,
    up: mpsc::Sender<(usize, Value)>,
    done: CancellationToken,
) {
    let (outbox, mut frames) = mpsc::channel::<Value>(256);
    let (owner, token) = match service.attach(&package, zone, outbox) {
        Ok(owner) => owner,
        Err(why) => {
            let _ = up.send((index, json!({ "t": "why", "text": why, "end": true }))).await;
            return;
        }
    };
    let mut heard = service.subscribe();
    let mut devices = service.device_changes();
    let hello = json!({ "t": "hello", "devices": service.devices(), "now": service.now_ms(), "token": token });
    if up.send((index, hello)).await.is_ok() {
        loop {
            let frame = tokio::select! {
                _ = done.cancelled() => break,
                event = heard.recv() => match event {
                    Ok(event) if service.hears(owner, &event.port) => event.frame(),
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Lagged(lost)) => json!({ "t": "lost", "n": lost }),
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                list = devices.recv() => match list {
                    Ok(list) => json!({ "t": "devices", "devices": list }),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                // The owner is gone: a new socket took it over, or the service let it go.
                frame = frames.recv() => match frame {
                    Some(frame) => frame,
                    None => break,
                },
                command = commands.recv() => match command {
                    Some(command) => match socket::answer(&service, owner, &command.to_string()) {
                        Some(answer) => answer,
                        None => continue,
                    },
                    None => break,
                },
            };
            if up.send((index, frame)).await.is_err() {
                break;
            }
        }
    }
    service.detach(owner);
}

/// The channel opened to a Турбо's keyboards: its key in the header, the home network's time to answer.
async fn open(
    url: &str,
    key: &str,
) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, String> {
    let mut request = url.into_client_request().map_err(|_| crate::tr!("Адрес Турбо не читается."))?;
    let bearer = format!("Bearer {key}").parse().map_err(|_| crate::tr!("Ключ Турбо не читается."))?;
    request.headers_mut().insert(axum::http::header::AUTHORIZATION, bearer);
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(READ_LIMIT))
        .max_frame_size(Some(READ_LIMIT));
    match tokio::time::timeout(CONNECT_WAIT, tokio_tungstenite::connect_async_with_config(request, Some(config), true))
        .await
    {
        Ok(Ok((stream, _))) => Ok(stream),
        Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response))) => {
            let words = response
                .body()
                .as_ref()
                .and_then(|body| serde_json::from_slice::<Value>(body).ok())
                .and_then(|body| body.get("message").and_then(Value::as_str).map(str::to_string));
            Err(words.unwrap_or_else(|| {
                crate::tr!("Турбо не открыло клавиатуру (HTTP {status}).", status = response.status().as_u16())
            }))
        }
        _ => Err(crate::tr!("Турбо не ответило.")),
    }
}

/// A Турбо's keyboards as one leg: its channel, renewed in place when it says `renew` — the new channel takes the old
/// owner over by its token, so the page notices nothing.
async fn remote_leg(
    index: usize,
    name: String,
    url: String,
    key: String,
    mut commands: mpsc::Receiver<Value>,
    up: mpsc::Sender<(usize, Value)>,
    done: CancellationToken,
) {
    let mut token: Option<String> = None;
    'channel: loop {
        let stream = match open(&url, &key).await {
            Ok(stream) => stream,
            Err(why) => {
                let text = crate::tr!("{name}: {why}", name = name, why = why);
                let _ = up.send((index, json!({ "t": "why", "text": text, "end": true }))).await;
                return;
            }
        };
        let (mut sink, mut source) = stream.split();
        // A renewed channel: its first word takes the old owner over.
        let taking_over = token.clone();
        loop {
            tokio::select! {
                _ = done.cancelled() => {
                    let _ = sink.close().await;
                    return;
                }
                message = source.next() => match message {
                    Some(Ok(UpMessage::Text(text))) => {
                        let Ok(frame) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                        match frame.get("t").and_then(Value::as_str).unwrap_or_default() {
                            "renew" => continue 'channel,
                            "hello" => {
                                if let Some(old) = &taking_over {
                                    let resume = json!({ "t": "resume", "token": old }).to_string();
                                    if sink.send(UpMessage::Text(resume.into())).await.is_err() {
                                        break;
                                    }
                                }
                                token = frame.get("token").and_then(Value::as_str).map(str::to_string);
                                if up.send((index, frame)).await.is_err() {
                                    return;
                                }
                            }
                            _ => {
                                if up.send((index, frame)).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    Some(Ok(UpMessage::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => {}
                },
                command = commands.recv() => match command {
                    Some(command) => {
                        if sink.send(UpMessage::Text(command.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    None => {
                        let _ = sink.close().await;
                        return;
                    }
                },
            }
        }
        let text = crate::tr!("Клавиатура отключилась, потому что {name} не на связи.", name = name);
        let _ = up.send((index, json!({ "t": "why", "text": text, "end": true }))).await;
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::super::ports::{Fake, FakePorts};
    use super::*;

    fn keyboard(name: &str) -> (Arc<MidiService>, Arc<FakePorts>) {
        let fake = FakePorts::new();
        fake.plug("1", name);
        let service = MidiService::new(Arc::new(Fake(fake.clone())), None, false);
        service.set_gate(Arc::new(|_: &str| Ok(())));
        service.refresh();
        (service, fake)
    }

    async fn next(page: &mut mpsc::Receiver<Value>, kind: &str) -> Value {
        loop {
            let frame =
                tokio::time::timeout(Duration::from_secs(5), page.recv()).await.expect("in time").expect("open");
            if frame["t"] == kind {
                return frame;
            }
        }
    }

    fn relay(places: Vec<Place>, choose: Option<&str>) -> Relay {
        let chosen = choose.map(|place| (place.to_string(), Some("Беру у ПК".to_string())));
        Relay {
            places,
            package: "teacher".into(),
            zone: Zone::Home,
            check: Arc::new(|| Ok(())),
            choose: Arc::new(move || chosen.clone()),
        }
    }

    #[tokio::test]
    async fn two_places_are_one_channel_with_their_names_and_ids() {
        let (mac, mac_keys) = keyboard("Keystation 49");
        let (pc, pc_keys) = keyboard("Keystation 49");
        let places = vec![
            Place { id: "mac".into(), name: "Mac".into(), way: Way::Local(mac.clone()) },
            Place { id: "pc".into(), name: "ПК".into(), way: Way::Local(pc.clone()) },
        ];
        let (page_in, from_page) = mpsc::channel(16);
        let (to_page, mut page) = mpsc::channel(64);
        let done = CancellationToken::new();
        tokio::spawn(run(relay(places, Some("pc")), from_page, to_page, done.clone()));
        let hello = next(&mut page, "hello").await;
        let names: Vec<&str> =
            hello["devices"].as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["Keystation 49 · Mac", "Keystation 49 · ПК"], "two of one name told apart");
        assert_eq!(hello["devices"][0]["input"], "mac:in-1");
        // Listening to every keyboard: both places open their inputs.
        page_in.send(json!({ "t": "listen", "d": "*" })).await.unwrap();
        let listening = next(&mut page, "listening").await;
        let _ = listening;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(mac_keys.is_open("in-1") && pc_keys.is_open("in-1"));
        // A note on the ПК comes with its place and the relay's number.
        assert!(pc_keys.press("in-1", 1000, &[0x90, 60, 100]));
        let note = next(&mut page, "on").await;
        assert_eq!((note["d"].as_str(), note["s"].as_u64()), (Some("pc:in-1"), Some(1)));
        // A note to the Mac's output goes to the Mac only, its id bare there.
        page_in.send(json!({ "t": "on", "d": "mac:out-1", "n": 64, "v": 90 })).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(mac_keys.sent(), [("out-1".to_string(), vec![0x90, 64, 90])]);
        assert!(pc_keys.sent().is_empty());
        // A melody with no keyboard named goes to the place the home chose, and the choice is said.
        page_in.send(json!({ "t": "play", "id": "demo", "notes": [[60, 90, 0, 10]] })).await.unwrap();
        let chosen = next(&mut page, "chosen").await;
        assert_eq!(chosen["text"], "Беру у ПК");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(pc_keys.sent().iter().any(|(port, bytes)| port == "out-1" && bytes[0] == 0x90));
        // The relay answers the page's ping on its own clock.
        page_in.send(json!({ "t": "ping", "at": 5 })).await.unwrap();
        assert_eq!(next(&mut page, "pong").await["at"], 5);
        done.cancel();
    }

    #[tokio::test]
    async fn a_new_socket_takes_every_place_over_by_the_relays_token() {
        let (mac, mac_keys) = keyboard("Keys");
        let places = || vec![Place { id: "mac".into(), name: "Mac".into(), way: Way::Local(mac.clone()) }];
        let (old_in, from_old) = mpsc::channel(16);
        let (to_old, mut old) = mpsc::channel(64);
        let old_done = CancellationToken::new();
        tokio::spawn(run(relay(places(), None), from_old, to_old, old_done.clone()));
        let token = next(&mut old, "hello").await["token"].as_str().unwrap().to_string();
        old_in.send(json!({ "t": "listen", "d": "mac:in-1" })).await.unwrap();
        next(&mut old, "listening").await;
        // The renewed socket: a new relay, `resume` with the old one's token — the keyboard stays open throughout.
        let (new_in, from_new) = mpsc::channel(16);
        let (to_new, mut new) = mpsc::channel(64);
        let new_done = CancellationToken::new();
        tokio::spawn(run(relay(places(), None), from_new, to_new, new_done.clone()));
        next(&mut new, "hello").await;
        new_in.send(json!({ "t": "resume", "token": token })).await.unwrap();
        next(&mut new, "resumed").await;
        old_done.cancel();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(mac_keys.is_open("in-1"), "nothing let go on the joint");
        assert!(mac_keys.press("in-1", 2000, &[0x90, 62, 100]));
        assert_eq!(next(&mut new, "on").await["d"], "mac:in-1");
        new_done.cancel();
    }

    #[tokio::test]
    async fn one_place_keeps_its_names_and_its_end_ends_the_channel() {
        let (here, _keys) = keyboard("Keys");
        let places = vec![Place { id: HERE.into(), name: "Mac".into(), way: Way::Local(here) }];
        let (_page_in, from_page) = mpsc::channel(16);
        let (to_page, mut page) = mpsc::channel(64);
        let done = CancellationToken::new();
        let refused = Arc::new(Mutex::new(false));
        let check: Check = {
            let refused = refused.clone();
            Arc::new(move || if *locked(&refused) { Err("нет права".into()) } else { Ok(()) })
        };
        let relay = Relay { check, ..relay(places, None) };
        tokio::spawn(run(relay, from_page, to_page, done.clone()));
        let hello = next(&mut page, "hello").await;
        assert_eq!(
            (hello["devices"][0]["name"].as_str(), hello["devices"][0]["input"].as_str()),
            (Some("Keys"), Some("in-1"))
        );
        *locked(&refused) = true;
        let why = next(&mut page, "why").await;
        assert_eq!((why["text"].as_str(), why["end"].as_bool()), (Some("нет права"), Some(true)));
        done.cancel();
    }

    #[tokio::test]
    async fn a_turbo_that_does_not_answer_is_said_by_its_name() {
        let places = vec![Place {
            id: "mac".into(),
            name: "Mac".into(),
            way: Way::Remote { url: "ws://127.0.0.1:9/home/v1/devices/midi/stream".into(), key: "k".into() },
        }];
        let (_page_in, from_page) = mpsc::channel(16);
        let (to_page, mut page) = mpsc::channel(64);
        let done = CancellationToken::new();
        tokio::spawn(run(relay(places, None), from_page, to_page, done.clone()));
        let why = next(&mut page, "why").await;
        assert!(why["text"].as_str().unwrap().starts_with("Mac: "), "{why}");
        done.cancel();
    }
}
