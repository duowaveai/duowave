//! The station's live channel, relayed (docs/plans/desk-after-station.md, А). While a station's gateway runs on it,
//! this computer's gateway keeps one subscription to that station's live channel — with the pair's key, as the
//! helper's pass does (proxy.rs) — and tells its own panels every message of it, marked with the station:
//! `{type: 'station', station, message}` (live/mod.rs). The panels open no stream to the station themselves: a
//! browser gives one address six HTTP/1.1 connections, and two streams in every tab took them all.
//!
//! The subscription asks with the admin's role, and the computer's hub gives the words of the station's requests only
//! to who may read the journal here. It keeps the station's state — its last hello with the requests and presence
//! since — so a panel that joins hears it at once. It starts and stops as the station's record changes
//! (`placement.changed`), opens again at once when the station ends it (every 15 minutes), and after a failure waits
//! 1, 2, 5, 10, 30 s, then a minute — or less, when the watch sees the station again. A station that is off is told
//! in the log once, and once more when it is back.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use reqwest::header::ACCEPT;
use serde_json::Value;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::peer::HELPER_ROLE;
use super::watch::StationGateways;
use crate::core::events::{EventBus, GatewayEvent};
use crate::live::{LiveHub, StationSource, HELLO_RECENT};
use crate::utils::time::now_ms;

/// The station's channel says something at least every 20 s (a ping): silence this long is a dead connection.
const SILENCE: Duration = Duration::from_secs(45);
/// Pauses before the next try after failures in a row: 1, 2, 5, 10, 30 s, then a minute.
const BACKOFF: [Duration; 6] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(60),
];
/// The station did not take the pair's key: the pair may be linked anew — asked that rarely.
const REFUSED_WAIT: Duration = Duration::from_secs(60);
/// The station ended the stream itself (its lifetime): opened again at once.
const AGAIN: Duration = Duration::from_millis(300);
/// An SSE block larger than this is not the station's.
const BLOCK_LIMIT: usize = 1024 * 1024;
/// What the station says, as the panel hears it; anything else (a newer build's) is not passed on.
const KINDS: &[&str] = &["hello", "request", "presence", "changed"];

/// Where the station's gateway is and its pair key: a change of either opens the channel anew.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    base: String,
    key: String,
}

pub struct Running {
    target: Target,
    task: JoinHandle<()>,
    /// Wakes a channel waiting after a failure: the station's record changed, or the watch saw it.
    wake: Arc<Notify>,
}

/// A station's state as its channel said it: its last hello with every request and presence since, and its clock
/// against ours (ms) — a panel joining later gets the hello with the station's time now.
struct Kept {
    hello: Value,
    offset: i64,
}

/// How a subscription ended.
#[derive(Debug, PartialEq)]
enum Outcome {
    /// The station ended it (its lifetime): opened again at once.
    Ended,
    /// The station did not take the pair's key.
    Refused(String),
    /// Unreachable, broken, silent.
    Failed(String),
}

/// What the log says of a station's channel: its loss once, its return once — a station that is off for the night
/// does not fill the log.
#[derive(Debug, Default)]
pub struct Quiet {
    told: bool,
}

impl Quiet {
    /// The channel failed: whether to tell it.
    pub fn failed(&mut self) -> bool {
        !std::mem::replace(&mut self.told, true)
    }

    /// The channel is up: whether to tell that it is back.
    pub fn up(&mut self) -> bool {
        std::mem::take(&mut self.told)
    }
}

/// The pause after this many failures in a row.
pub fn pause_after(failures: usize) -> Duration {
    BACKOFF[failures.clamp(1, BACKOFF.len()) - 1]
}

pub struct StationRelay {
    me: Weak<StationRelay>,
    stations: Arc<StationGateways>,
    hub: Weak<LiveHub>,
    running: Mutex<BTreeMap<String, Running>>,
    kept: Mutex<BTreeMap<String, Kept>>,
    /// The stations whose watch looks again in a moment (`look_soon`).
    soon: Mutex<std::collections::BTreeSet<String>>,
}

impl StationRelay {
    /// Relays the channel of every station whose gateway runs on it, from now on: the hub's panels hear them, and
    /// the station's record changing starts or stops its channel.
    pub fn start(stations: Arc<StationGateways>, hub: &Arc<LiveHub>, events: &EventBus) -> Arc<StationRelay> {
        let relay = Arc::new_cyclic(|me| StationRelay {
            me: me.clone(),
            stations,
            hub: Arc::downgrade(hub),
            running: Mutex::new(BTreeMap::new()),
            kept: Mutex::new(BTreeMap::new()),
            soon: Mutex::new(std::collections::BTreeSet::new()),
        });
        hub.set_stations(relay.clone());
        let weak = Arc::downgrade(&relay);
        events.on("station-relay", move |event| {
            if let GatewayEvent::PlacementChanged { .. } = &*event {
                if let Some(relay) = weak.upgrade() {
                    relay.reconcile();
                }
            }
        });
        relay.reconcile();
        relay
    }

    /// The channels that should be open, opened; the others closed. A channel waiting after a failure tries now.
    fn reconcile(&self) {
        let Some(me) = self.me.upgrade() else { return };
        let wanted: BTreeMap<String, Target> = self
            .stations
            .all()
            .into_iter()
            .filter_map(|watch| {
                let client = watch.client()?;
                Some((
                    watch.key().to_string(),
                    Target { base: client.base().to_string(), key: client.key().to_string() },
                ))
            })
            .collect();
        let mut gone = Vec::new();
        {
            let mut running = self.lock_running();
            running.retain(|key, run| {
                let keep = wanted.get(key) == Some(&run.target);
                if !keep {
                    run.task.abort();
                    gone.push(key.clone());
                }
                keep
            });
            for (key, target) in wanted {
                if let Some(run) = running.get(&key) {
                    run.wake.notify_one();
                    continue;
                }
                tracing::debug!(target: "placement", "relay: following the station {key}'s live channel");
                let wake = Arc::new(Notify::new());
                let task = tokio::spawn(me.clone().follow(key.clone(), target.clone(), wake.clone()));
                running.insert(key, Running { target, task, wake });
            }
        }
        for key in gone {
            self.lost(&key);
        }
    }

    /// One station's channel, for as long as it is wanted: opened, read, opened again.
    async fn follow(self: Arc<Self>, key: String, target: Target, wake: Arc<Notify>) {
        let mut failures = 0usize;
        let mut quiet = Quiet::default();
        loop {
            let outcome = self.listen(&key, &target, &mut failures, &mut quiet).await;
            let pause = match &outcome {
                Outcome::Ended => {
                    tracing::debug!(target: "placement", "relay: the station {key} ended its live channel");
                    AGAIN
                }
                Outcome::Refused(why) | Outcome::Failed(why) => {
                    self.lost(&key);
                    if quiet.failed() {
                        tracing::info!(target: "placement", "relay: the station {key}'s live channel: {why}; trying again quietly");
                    } else {
                        tracing::debug!(target: "placement", "relay: the station {key}'s live channel: {why}");
                    }
                    if matches!(outcome, Outcome::Refused(_)) {
                        REFUSED_WAIT
                    } else {
                        failures += 1;
                        pause_after(failures)
                    }
                }
            };
            tokio::select! {
                _ = tokio::time::sleep(pause) => {}
                _ = wake.notified() => {}
            }
        }
    }

    /// The station's channel once: what it says, relayed, until it ends, breaks or goes silent.
    async fn listen(&self, key: &str, target: &Target, failures: &mut usize, quiet: &mut Quiet) -> Outcome {
        let request = super::proxy::client()
            .get(format!("{}/api/v1/live", target.base))
            .bearer_auth(&target.key)
            .header(HELPER_ROLE, "admin")
            .header(ACCEPT, "text/event-stream");
        let mut response = match request.send().await {
            Ok(response) => response,
            Err(error) => return Outcome::Failed(describe(&error)),
        };
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Outcome::Refused(format!("колонка не приняла ключ пары ({})", status.as_u16()));
        }
        if !status.is_success() {
            return Outcome::Failed(format!("колонка ответила {}", status.as_u16()));
        }
        let mut buffer: Vec<u8> = Vec::new();
        loop {
            let chunk = match tokio::time::timeout(SILENCE, response.chunk()).await {
                Err(_) => return Outcome::Failed("канал колонки молчит".into()),
                Ok(Err(error)) => return Outcome::Failed(describe(&error)),
                Ok(Ok(None)) => return Outcome::Ended,
                Ok(Ok(Some(chunk))) => chunk,
            };
            buffer.extend_from_slice(&chunk);
            while let Some(end) = buffer.windows(2).position(|pair| pair == b"\n\n") {
                let block: Vec<u8> = buffer.drain(..end + 2).collect();
                let Some(message) = data_of(&block) else { continue };
                if message.get("type").and_then(Value::as_str) == Some("hello") {
                    *failures = 0;
                    if quiet.up() {
                        tracing::info!(target: "placement", "relay: the station {key}'s live channel is back");
                    }
                }
                self.heard(key, message);
            }
            if buffer.len() > BLOCK_LIMIT {
                return Outcome::Failed("канал колонки прислал непонятное".into());
            }
        }
    }

    /// A station's message: kept into its state, then told to the hub's panels.
    fn heard(&self, key: &str, message: Value) {
        let kind = message.get("type").and_then(Value::as_str).unwrap_or_default();
        if !KINDS.contains(&kind) {
            return;
        }
        {
            let mut kept = self.lock_kept();
            match kind {
                "hello" => {
                    kept.insert(key.to_string(), Kept { offset: offset_of(&message), hello: message.clone() });
                }
                "request" => {
                    if let (Some(state), Some(request)) = (kept.get_mut(key), message.get("request")) {
                        keep_request(&mut state.hello, request);
                    }
                }
                "presence" => {
                    if let (Some(state), Some(presence)) = (kept.get_mut(key), message.get("presence")) {
                        keep_presence(&mut state.hello, presence);
                    }
                }
                _ => {}
            }
        }
        if kind == "changed" && home_news(message.get("name").and_then(Value::as_str).unwrap_or_default()) {
            self.look_soon(key);
        }
        if let Some(hub) = self.hub.upgrade() {
            hub.relay(key, Some(&message));
        }
    }

    /// The centre changed something of the home: the watch looks at it in a moment, and the home's copy here follows
    /// within seconds, not at the next 30-s look (docs/plans/home-and-devices.md, stage 3). One look per burst.
    fn look_soon(&self, key: &str) {
        if !lock(&self.soon).insert(key.to_string()) {
            return;
        }
        let Some(me) = self.me.upgrade() else { return };
        let key = key.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(700)).await;
            lock(&me.soon).remove(&key);
            if let Some(watch) = me.stations.find(&key) {
                watch.poll().await;
            }
        });
    }

    /// The station's channel is lost: its state goes, and the panels hear it.
    fn lost(&self, key: &str) {
        let had = self.lock_kept().remove(key).is_some();
        if let (true, Some(hub)) = (had, self.hub.upgrade()) {
            hub.relay(key, None);
        }
    }

    pub fn lock_running(&self) -> MutexGuard<'_, BTreeMap<String, Running>> {
        self.running.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_kept(&self) -> MutexGuard<'_, BTreeMap<String, Kept>> {
        self.kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl StationSource for StationRelay {
    /// Each station's state as a hello, with the station's time now.
    fn snapshots(&self) -> Vec<(String, Value)> {
        let now = now_ms() as i64;
        self.lock_kept()
            .iter()
            .map(|(key, kept)| {
                let mut hello = kept.hello.clone();
                if let (Some(fields), Some(at)) =
                    (hello.as_object_mut(), chrono::DateTime::from_timestamp_millis(now + kept.offset))
                {
                    fields.insert("serverTime".into(), Value::String(crate::utils::time::iso(at)));
                }
                (key.clone(), hello)
            })
            .collect()
    }
}

impl Drop for StationRelay {
    fn drop(&mut self) {
        for run in self.lock_running().values() {
            run.task.abort();
        }
    }
}

/// The station's news that change what the home's copy holds.
fn home_news(name: &str) -> bool {
    matches!(
        name,
        "settings.changed"
            | "persona.changed"
            | "brain.changed"
            | "mcp.changed"
            | "connector.changed"
            | "routes.changed"
            | "voice.changed"
            | "mode.changed"
            | "station.updated"
            | "helpers.changed"
            | "home.room.changed"
            | "memory.changed"
    )
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn describe(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "колонка не ответила вовремя".into()
    } else if error.is_connect() {
        "колонка не отвечает".into()
    } else {
        format!("колонка: {error}")
    }
}

/// An SSE block's message: its `data` lines as JSON. A comment (the ping) has none.
pub fn data_of(block: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(block).ok()?;
    let data: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|line| line.strip_prefix(' ').unwrap_or(line))
        .collect();
    if data.is_empty() {
        return None;
    }
    serde_json::from_str(&data.join("\n")).ok()
}

/// The station's clock against ours, by its hello's time (ms).
fn offset_of(hello: &Value) -> i64 {
    hello
        .get("serverTime")
        .and_then(Value::as_str)
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .map(|at| at.timestamp_millis() - now_ms() as i64)
        .unwrap_or(0)
}

/// A request as it goes, into the hello: among those going while it goes, then first of the last finished.
pub fn keep_request(hello: &mut Value, request: &Value) {
    let Some(id) = request.get("id").cloned() else { return };
    for list in ["active", "recent"] {
        if let Some(Value::Array(requests)) = hello.get_mut(list) {
            requests.retain(|entry| entry.get("id") != Some(&id));
        }
    }
    let going = request.get("status").and_then(Value::as_str) == Some("active");
    if let Some(Value::Array(requests)) = hello.get_mut(if going { "active" } else { "recent" }) {
        requests.insert(0, request.clone());
        if !going {
            requests.truncate(HELLO_RECENT);
        }
    }
}

/// A station's presence, into the hello: in place of its previous one.
pub fn keep_presence(hello: &mut Value, presence: &Value) {
    let device = presence.get("device").cloned();
    if let Some(Value::Array(list)) = hello.get_mut("presence") {
        match list.iter_mut().find(|entry| entry.get("device") == device.as_ref()) {
            Some(entry) => *entry = presence.clone(),
            None => list.push(presence.clone()),
        }
    }
}
