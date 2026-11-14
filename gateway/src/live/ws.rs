//! The panel's live channel over a WebSocket (docs/plans/desk-after-station.md, А): the same messages as the SSE
//! stream of `/api/v1/live` — hello, request, presence, changed — and, for a panel at home, the stations' channels the
//! gateway relays (placement/relay.rs). A browser gives one address six HTTP/1.1 connections, and each long stream
//! held one of them; a WebSocket is one connection per tab, and the station's channel comes along it.
//!
//! A browser cannot give a WebSocket a header, and the sign-in never goes into an address: the panel first takes a
//! ticket (`POST /api/v1/live/ticket`, signed like any API call) and opens `/api/v1/live/ws?ticket=…` with it. A
//! ticket is good once, for 30 s, and the socket is the ticket's principal's.
//!
//! The socket lives like the stream: a ping every 20 s (a WebSocket ping, and a `{"type":"ping"}` the panel's watchdog
//! sees), the end after 15 minutes (close 1000; the panel opens it again, signed anew). A panel that answers no ping
//! for a minute, or cannot take a message in 10 s, is gone. It sends nothing but pongs: its frames are kept small.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{ChannelSink, LiveHub, LIFETIME, PING, QUEUE};
use crate::app::App;
use crate::core::identity::permissions::STATION_VIEW;
use crate::core::identity::Principal;
use crate::platform::http::{created, ApiError, ApiResult, Caller, Zone};

/// A ticket is good this long after it was given.
pub const TICKET_LIFETIME: Duration = Duration::from_secs(30);
/// Tickets given and not used yet, at most: past it the oldest goes.
const TICKETS_KEPT: usize = 256;
/// No pong, nor anything else, from the panel this long: it is gone (a laptop asleep, a network changed).
const SILENT: Duration = Duration::from_secs(60);
/// A message the panel cannot take in this long: it fell behind for good.
const SEND_WAIT: Duration = Duration::from_secs(10);
/// What the panel sends — pongs, a close — is tiny: so are the buffers that read it.
const READ_LIMIT: usize = 4 * 1024;
/// The write buffer: small, flushed every message; a hello is tens of KB at most.
const WRITE_BUFFER: usize = 8 * 1024;
const WRITE_LIMIT: usize = 1024 * 1024;
/// The message the panel's watchdog hears every 20 s (it cannot see a WebSocket ping).
const PING_MESSAGE: &str = r#"{"type":"ping"}"#;

struct Issued {
    principal: Principal,
    until: Instant,
}

/// The tickets given and not used yet.
#[derive(Default)]
pub struct Tickets {
    issued: Mutex<HashMap<String, Issued>>,
}

impl Tickets {
    /// A new ticket for `principal`.
    pub fn issue(&self, principal: &Principal) -> String {
        self.issue_at(principal, Instant::now())
    }

    /// The ticket's principal, once: a ticket taken, expired or never given is nobody's.
    pub fn take(&self, ticket: &str) -> Option<Principal> {
        self.take_at(ticket, Instant::now())
    }

    fn issue_at(&self, principal: &Principal, now: Instant) -> String {
        let ticket = crate::utils::ids::random_hex(32);
        let mut issued = self.lock();
        issued.retain(|_, entry| entry.until > now);
        if issued.len() >= TICKETS_KEPT {
            if let Some(oldest) = issued.iter().min_by_key(|(_, entry)| entry.until).map(|(key, _)| key.clone()) {
                issued.remove(&oldest);
            }
        }
        issued.insert(ticket.clone(), Issued { principal: principal.clone(), until: now + TICKET_LIFETIME });
        ticket
    }

    fn take_at(&self, ticket: &str, now: Instant) -> Option<Principal> {
        let entry = self.lock().remove(ticket)?;
        (entry.until > now).then_some(entry.principal)
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Issued>> {
        self.issued.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TicketView {
    ticket: String,
    /// Seconds.
    expires_in: u64,
}

/// A ticket for the live socket: whoever may see the station.
pub(super) async fn ticket(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<TicketView> {
    let principal = caller.permitted(STATION_VIEW)?;
    Ok(created(TicketView { ticket: app.live.tickets.issue(principal), expires_in: TICKET_LIFETIME.as_secs() }))
}

#[derive(Debug, Deserialize)]
pub(super) struct SocketQuery {
    #[serde(default)]
    ticket: Option<String>,
}

/// The live socket: its ticket's principal, and the stations' channels for a panel at home (the helper's pass to the
/// station is for the home network only, and so is what the station says).
pub(super) async fn socket(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(query): Query<SocketQuery>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    if caller.zone == Zone::Remote && !app.config.gateway.panel_remote {
        return Err(ApiError::forbidden(crate::tr!("Пульт вне дома выключен (PANEL_REMOTE).")));
    }
    let principal = query
        .ticket
        .as_deref()
        .and_then(|ticket| app.live.tickets.take(ticket))
        .ok_or_else(|| ApiError::unauthorized(crate::tr!("Билет живого канала не подошёл: возьмите новый.")))?;
    let stations = matches!(caller.zone, Zone::Local | Zone::Home);
    let hub = app.live.clone();
    Ok(upgrade
        .read_buffer_size(READ_LIMIT)
        .max_message_size(READ_LIMIT)
        .max_frame_size(READ_LIMIT)
        .write_buffer_size(WRITE_BUFFER)
        .max_write_buffer_size(WRITE_LIMIT)
        .on_upgrade(move |socket| serve(socket, hub, principal, stations)))
}

/// How the socket ends: the gateway's end (its lifetime, the hub let it go) — the panel opens it again at once.
fn closing(reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame { code: 1000, reason: reason.into() }))
}

type Out = futures::stream::SplitSink<WebSocket, Message>;

/// A message to the panel: false when it cannot take it in `SEND_WAIT`, or the socket is gone.
async fn send(out: &mut Out, message: Message) -> bool {
    matches!(tokio::time::timeout(SEND_WAIT, out.send(message)).await, Ok(Ok(())))
}

/// One panel's socket: the hello and every message it may see, pings, the end.
async fn serve(socket: WebSocket, hub: Arc<LiveHub>, principal: Principal, stations: bool) {
    let (sender, mut queue) = mpsc::channel(QUEUE);
    let _leave = if stations {
        hub.join_with_stations(&principal, Box::new(ChannelSink(sender)))
    } else {
        hub.join(&principal, Box::new(ChannelSink(sender)))
    };
    let (mut out, mut incoming) = socket.split();
    let start = tokio::time::Instant::now();
    let mut ping = tokio::time::interval_at(start + PING, PING);
    let end = tokio::time::sleep_until(start + LIFETIME);
    tokio::pin!(end);
    let mut heard = Instant::now();
    loop {
        tokio::select! {
            message = queue.recv() => {
                let Some(message) = message else {
                    // The hub let it go: it fell behind, or the gateway stops.
                    let _ = send(&mut out, closing("again")).await;
                    return;
                };
                let text = serde_json::to_string(&message).unwrap_or_else(|_| "{}".into());
                if !send(&mut out, Message::Text(text.into())).await {
                    return;
                }
            }
            _ = ping.tick() => {
                if heard.elapsed() > SILENT {
                    tracing::debug!("a panel's live socket answered no ping");
                    return;
                }
                if !send(&mut out, Message::Ping(Default::default())).await
                    || !send(&mut out, Message::Text(PING_MESSAGE.into())).await
                {
                    return;
                }
            }
            frame = incoming.next() => match frame {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => heard = Instant::now(),
            },
            _ = &mut end => {
                let _ = send(&mut out, closing("lifetime")).await;
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::identity::principal::PrincipalKind;

    fn person(id: &str) -> Principal {
        Principal {
            kind: PrincipalKind::User,
            id: id.into(),
            name: id.into(),
            role: None,
            grants: vec!["station.view".into()],
            denies: Vec::new(),
        }
    }

    #[test]
    fn a_ticket_is_its_principals_once_and_for_thirty_seconds() {
        let tickets = Tickets::default();
        let now = Instant::now();
        let anya = tickets.issue_at(&person("anya"), now);
        let mama = tickets.issue_at(&person("mama"), now);
        assert_eq!(anya.len(), 64);
        assert_ne!(anya, mama);
        assert_eq!(tickets.take_at(&mama, now).map(|principal| principal.id), Some("mama".into()));
        assert_eq!(tickets.take_at(&mama, now), None, "once");
        assert_eq!(tickets.take_at("not-a-ticket", now), None);
        assert_eq!(tickets.take_at(&anya, now + TICKET_LIFETIME), None, "expired");
        assert_eq!(tickets.take_at(&anya, now), None, "an expired one is gone");
    }

    #[test]
    fn tickets_never_used_do_not_pile_up() {
        let tickets = Tickets::default();
        let now = Instant::now();
        let first = tickets.issue_at(&person("first"), now);
        for _ in 1..TICKETS_KEPT {
            tickets.issue_at(&person("p"), now + Duration::from_millis(1));
        }
        assert_eq!(tickets.lock().len(), TICKETS_KEPT);
        let last = tickets.issue_at(&person("last"), now + Duration::from_millis(2));
        assert_eq!(tickets.lock().len(), TICKETS_KEPT, "the oldest went");
        assert_eq!(tickets.take_at(&first, now), None);
        assert!(tickets.take_at(&last, now).is_some());
        // Expired ones go at the next ticket.
        tickets.issue_at(&person("later"), now + TICKET_LIFETIME + Duration::from_secs(1));
        assert_eq!(tickets.lock().len(), 1);
    }
}
