//! The channel of notes (docs/plans/midi.md, «Канал нот»): its own live channel beside the panel's, the owner's
//! decision 1. The panel's is one-way and the same for every panel; notes go both ways, only to the page that listens,
//! and the keyboard is open only while it does.
//!
//! A page takes a ticket for its app (`POST /api/v1/midi/ticket {package}`: the person may use apps, the app is on
//! here with the right `midi`, the page is at home) and opens `/api/v1/midi/ws?ticket=…`. A ticket is good once, for
//! 30 s. Frames are short JSON:
//!
//! - the gateway's: `hello` (keyboards, its clock, the owner's token), `devices`, the notes (`on`, `off`, `cc`, …
//!   with `d` the input, `at` the gateway's ms, `s` the number), `playing`, `played`, `why`, `pong`, `renew`;
//! - the page's: `listen` and `unlisten` (`d`: an input or `*`), the same notes to an output (`d`), `play`
//!   `{id, d, notes}`, `stop`, `resume` `{token}`, `ping` `{at}`.
//!
//! The socket lives 15 minutes, as the panel's: a key taken back stops working as soon. Ten seconds before its end the
//! gateway says `renew`; the page opens a new socket and sends `resume` with the old one's token, and the owner moves
//! over whole — nothing is let go and no note is lost on the joint.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message as WsMessage, WebSocket, WebSocketUpgrade};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use super::wire::{self, Message};
use super::MidiService;
use crate::platform::http::Zone;

pub const TICKET_LIFETIME: Duration = Duration::from_secs(30);
const TICKETS_KEPT: usize = 64;
/// The socket's life, as the panel's live channel; `renew` this long before its end.
pub const LIFETIME: Duration = Duration::from_secs(15 * 60);
pub const RENEW_BEFORE: Duration = Duration::from_secs(10);
/// A WebSocket ping this often; no word from the page for `SILENT` — it is gone.
const PING: Duration = Duration::from_secs(20);
const SILENT: Duration = Duration::from_secs(60);
/// How often the socket asks whether its app may still hear.
const CHECK: Duration = Duration::from_secs(2);
const SEND_WAIT: Duration = Duration::from_secs(10);
/// A frame from the page at most: a melody of 2000 notes fits.
pub const READ_LIMIT: usize = 64 * 1024;

struct Issued {
    package: String,
    zone: Zone,
    until: Instant,
}

/// Tickets given and not used yet.
#[derive(Default)]
pub struct Tickets {
    issued: Mutex<HashMap<String, Issued>>,
}

impl Tickets {
    pub fn issue(&self, package: &str, zone: Zone) -> String {
        let ticket = crate::utils::ids::random_hex(32);
        let now = Instant::now();
        let mut issued = self.issued.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        issued.retain(|_, entry| entry.until > now);
        if issued.len() >= TICKETS_KEPT {
            if let Some(oldest) = issued.iter().min_by_key(|(_, entry)| entry.until).map(|(key, _)| key.clone()) {
                issued.remove(&oldest);
            }
        }
        issued.insert(ticket.clone(), Issued { package: package.to_string(), zone, until: now + TICKET_LIFETIME });
        ticket
    }

    /// The ticket's app and zone, once.
    pub fn take(&self, ticket: &str) -> Option<(String, Zone)> {
        let entry = self.issued.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(ticket)?;
        (entry.until > Instant::now()).then_some((entry.package, entry.zone))
    }
}

/// The socket, upgraded: its limits, then `serve`.
pub fn upgrade(
    upgrade: WebSocketUpgrade,
    service: Arc<MidiService>,
    package: String,
    zone: Zone,
) -> axum::response::Response {
    upgrade
        .read_buffer_size(READ_LIMIT)
        .max_message_size(READ_LIMIT)
        .max_frame_size(READ_LIMIT)
        .on_upgrade(move |socket| serve(socket, service, package, zone))
}

type Out = futures::stream::SplitSink<WebSocket, WsMessage>;

async fn send(out: &mut Out, frame: &Value) -> bool {
    let text = serde_json::to_string(frame).unwrap_or_else(|_| "{}".into());
    matches!(tokio::time::timeout(SEND_WAIT, out.send(WsMessage::Text(text.into()))).await, Ok(Ok(())))
}

async fn close(out: &mut Out, code: u16, reason: &str) {
    let reason: String = reason.chars().take(60).collect();
    let _ = out.send(WsMessage::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
}

/// One page's channel of notes.
pub async fn serve(socket: WebSocket, service: Arc<MidiService>, package: String, zone: Zone) {
    let (mut out, mut incoming) = socket.split();
    let (outbox, mut frames) = mpsc::channel::<Value>(256);
    let (owner, token) = match service.attach(&package, zone, outbox) {
        Ok(owner) => owner,
        Err(why) => {
            let _ = send(&mut out, &json!({ "t": "why", "text": why })).await;
            close(&mut out, 1008, "full").await;
            return;
        }
    };
    let mut heard = service.subscribe();
    let mut devices = service.device_changes();
    let hello = json!({ "t": "hello", "devices": service.devices(), "now": service.now_ms(), "token": token });
    if send(&mut out, &hello).await {
        run(&mut out, &mut incoming, &service, owner, &mut heard, &mut devices, &mut frames).await;
    }
    service.detach(owner);
}

#[allow(clippy::too_many_arguments)]
async fn run(
    out: &mut Out,
    incoming: &mut futures::stream::SplitStream<WebSocket>,
    service: &Arc<MidiService>,
    owner: u64,
    heard: &mut broadcast::Receiver<super::Heard>,
    devices: &mut broadcast::Receiver<Vec<super::Device>>,
    frames: &mut mpsc::Receiver<Value>,
) {
    let start = tokio::time::Instant::now();
    let mut ping = tokio::time::interval_at(start + PING, PING);
    let mut check = tokio::time::interval_at(start + CHECK, CHECK);
    let renew = tokio::time::sleep_until(start + LIFETIME - RENEW_BEFORE);
    let end = tokio::time::sleep_until(start + LIFETIME);
    tokio::pin!(renew, end);
    let mut renewed = false;
    let mut last_heard = Instant::now();
    loop {
        tokio::select! {
            event = heard.recv() => match event {
                Ok(event) => {
                    if service.hears(owner, &event.port) && !send(out, &event.frame()).await {
                        return;
                    }
                }
                // The page fell behind: the numbers it gets next say how much it lost.
                Err(broadcast::error::RecvError::Lagged(lost)) => {
                    if !send(out, &json!({ "t": "lost", "n": lost })).await {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            list = devices.recv() => {
                if let Ok(list) = list {
                    if !send(out, &json!({ "t": "devices", "devices": list })).await {
                        return;
                    }
                }
            }
            frame = frames.recv() => {
                if let Some(frame) = frame {
                    if !send(out, &frame).await {
                        return;
                    }
                }
            }
            frame = incoming.next() => match frame {
                Some(Ok(WsMessage::Text(text))) => {
                    last_heard = Instant::now();
                    if let Some(answer) = answer(service, owner, &text) {
                        if !send(out, &answer).await {
                            return;
                        }
                    }
                }
                Some(Ok(WsMessage::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => last_heard = Instant::now(),
            },
            _ = check.tick() => {
                if let Err(why) = service.still(owner) {
                    let _ = send(out, &json!({ "t": "why", "text": why, "end": true })).await;
                    close(out, 1000, "gone").await;
                    return;
                }
            }
            _ = ping.tick() => {
                if last_heard.elapsed() > SILENT {
                    return;
                }
                if !matches!(tokio::time::timeout(SEND_WAIT, out.send(WsMessage::Ping(Default::default()))).await, Ok(Ok(()))) {
                    return;
                }
            }
            _ = &mut renew, if !renewed => {
                renewed = true;
                if !send(out, &json!({ "t": "renew" })).await {
                    return;
                }
            }
            _ = &mut end => {
                close(out, 1000, "lifetime").await;
                return;
            }
        }
    }
}

/// What the page asked: done, and an answer when it needs one.
pub fn answer(service: &Arc<MidiService>, owner: u64, text: &str) -> Option<Value> {
    let Ok(frame) = serde_json::from_str::<Value>(text) else {
        return Some(json!({ "t": "why", "text": crate::tr!("не JSON") }));
    };
    let field = |key: &str| frame.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let failed = |why: String| Some(json!({ "t": "why", "text": why, "of": frame.get("t") }));
    if let Some(message) = Message::from_frame(&frame) {
        return match message.and_then(|message| service.send(owner, &field("d"), message)) {
            Ok(()) => None,
            Err(why) => failed(why),
        };
    }
    match frame.get("t").and_then(Value::as_str).unwrap_or_default() {
        "listen" => match service.listen(owner, &field("d")) {
            Ok(ports) => Some(json!({ "t": "listening", "d": ports })),
            Err(why) => failed(why),
        },
        "unlisten" => {
            service.unlisten(owner, &field("d"));
            None
        }
        "play" => {
            let id = Some(field("id")).filter(|id| !id.is_empty()).unwrap_or_else(|| "play".into());
            match wire::melody(frame.get("notes").unwrap_or(&Value::Null))
                .and_then(|steps| service.play(owner, &id, &field("d"), steps))
            {
                Ok(()) => None,
                Err(why) => failed(why),
            }
        }
        "stop" => {
            service.stop_play(owner);
            None
        }
        "resume" => match service.resume(owner, &field("token")) {
            true => Some(json!({ "t": "resumed" })),
            false => failed(crate::tr!("Прежний канал уже закрыт.")),
        },
        "ping" => Some(json!({ "t": "pong", "at": frame.get("at"), "now": service.now_ms() })),
        other => failed(crate::tr!("Кадр «{other}» не знаю.", other = other)),
    }
}

#[cfg(test)]
mod tests {
    use super::super::ports::{Fake, FakePorts};
    use super::*;

    #[test]
    fn a_ticket_is_its_apps_once() {
        let tickets = Tickets::default();
        let ticket = tickets.issue("teacher", Zone::Home);
        assert_eq!(tickets.take(&ticket), Some(("teacher".into(), Zone::Home)));
        assert_eq!(tickets.take(&ticket), None);
        assert_eq!(tickets.take("nope"), None);
    }

    #[tokio::test]
    async fn the_pages_frames() {
        let fake = FakePorts::new();
        fake.plug("1", "Keys");
        let service = MidiService::new(Arc::new(Fake(fake.clone())), None, false);
        service.set_gate(Arc::new(|_: &str| Ok(())));
        service.refresh();
        let (outbox, _frames) = mpsc::channel(8);
        let (owner, _) = service.attach("teacher", Zone::Local, outbox).unwrap();
        assert_eq!(answer(&service, owner, r#"{"t":"listen","d":"*"}"#).unwrap()["d"], json!(["in-1"]));
        assert!(fake.is_open("in-1"));
        assert_eq!(answer(&service, owner, r#"{"t":"on","d":"out-1","n":60,"v":90}"#), None);
        assert_eq!(fake.sent(), [("out-1".to_string(), vec![0x90, 60, 90])]);
        assert_eq!(answer(&service, owner, r#"{"t":"on","d":"out-1","n":200}"#).unwrap()["t"], "why");
        assert_eq!(answer(&service, owner, r#"{"t":"sysex","data":[240]}"#).unwrap()["t"], "why", "no SysEx");
        assert_eq!(answer(&service, owner, r#"{"t":"ping","at":5}"#).unwrap()["at"], 5);
        assert_eq!(answer(&service, owner, "not json").unwrap()["t"], "why");
        assert_eq!(answer(&service, owner, r#"{"t":"resume","token":"x"}"#).unwrap()["t"], "why");
        assert_eq!(answer(&service, owner, r#"{"t":"play","d":"out-1","notes":[[60]]}"#).unwrap()["t"], "why");
    }
}
