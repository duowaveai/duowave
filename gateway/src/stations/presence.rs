//! Whether each approved station is there (core/stations/presence.rs), from what it opens to the gateway — no probe
//! goes to the station. Its voice sessions come from the session registry's events, its own clients' channels from
//! the calls (`station.channel`, by address: the address its voice socket comes from). A change of state or
//! channels goes on the bus (`station.presence`) for the panel; while anything of it is up, its record's
//! `last_seen` is refreshed every minute and once more at shutdown, so after a restart the panel says
//! «переподключается» for a station that was here, and «была 3 мин назад» for one that was not. The same
//! «переподключается» follows a voice socket closed by a handshake (a mode switch: the station opens the next by
//! itself, in up to a few minutes); one that broke or was cut silent (code 1006) leaves the station offline at once.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::core::events::{EventBus, GatewayEvent};
use crate::core::stations::presence::{
    closed_by_handshake, presence_of, PresenceInput, PresenceState, StationChannel, StationPresence,
};
use crate::stations::service::{StationStatus, StationsService};
use crate::utils::time::now_ms;

/// How often a station's channels refresh its «last seen» in its record.
const SEEN_EVERY_MS: u64 = 60_000;
/// How often the states are looked at again: a poll that stopped, a restart's window that ran out.
const TICK: Duration = Duration::from_millis(15_000);

#[derive(Default)]
struct Inner {
    /// Live voice sessions by device.
    voice: HashMap<String, HashSet<String>>,
    /// When a device's last voice socket closed by a handshake, none open since: it opens the next by itself.
    voice_closed_at: HashMap<String, u64>,
    microphone: HashSet<String>,
    polled_at: HashMap<String, u64>,
    seen_before_start: HashMap<String, u64>,
    seen_written: HashMap<String, u64>,
    views: HashMap<String, StationPresence>,
}

pub struct PresenceService {
    stations: Arc<StationsService>,
    events: EventBus,
    started_at: u64,
    inner: Mutex<Inner>,
    cancel: CancellationToken,
}

impl PresenceService {
    pub fn new(stations: Arc<StationsService>, events: EventBus) -> PresenceService {
        PresenceService {
            stations,
            events,
            started_at: now_ms(),
            inner: Mutex::new(Inner::default()),
            cancel: CancellationToken::new(),
        }
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Seeds «last seen» from the records, does the first look (synchronously, so the state is there at once), then
    /// follows the station's channels on the bus and looks again every tick. The TS `onApplicationBootstrap`.
    pub fn start(self: &Arc<Self>) {
        {
            let mut inner = self.inner();
            for station in self.stations.list() {
                if let Ok(at) = station.view.last_seen.parse::<chrono::DateTime<chrono::Utc>>() {
                    inner.seen_before_start.insert(station.view.device, at.timestamp_millis().max(0) as u64);
                }
            }
        }
        // Subscribe before the first look: events during it are buffered, none lost.
        let receiver = self.events.subscribe();
        self.refresh(now_ms());
        let me = self.clone();
        let cancel = self.cancel.clone();
        tokio::spawn(async move { me.run(receiver, cancel).await });
    }

    async fn run(self: Arc<Self>, mut receiver: broadcast::Receiver<Arc<GatewayEvent>>, cancel: CancellationToken) {
        let mut ticker = tokio::time::interval(TICK);
        ticker.tick().await; // the immediate first tick
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = ticker.tick() => self.refresh(now_ms()),
                event = receiver.recv() => match event {
                    Ok(event) => self.handle(&event),
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        tracing::warn!(target: "events", "presence missed {missed} event(s)");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            }
        }
    }

    fn handle(&self, event: &GatewayEvent) {
        match event {
            GatewayEvent::StationConnected { session_id, device_tag } => {
                {
                    let mut inner = self.inner();
                    inner.voice.entry(device_tag.clone()).or_default().insert(session_id.clone());
                    inner.voice_closed_at.remove(device_tag);
                }
                self.update(device_tag, now_ms());
            }
            GatewayEvent::StationDisconnected { session_id, device_tag, code } => {
                let now = now_ms();
                {
                    let mut inner = self.inner();
                    let last = inner.voice.get_mut(device_tag).is_none_or(|sessions| {
                        sessions.remove(session_id);
                        sessions.is_empty()
                    });
                    // Its last socket: closed by a handshake, the station comes back by itself (a mode switch);
                    // broken or cut silent (1006), it may be gone.
                    if last {
                        if code.is_some_and(closed_by_handshake) {
                            inner.voice_closed_at.insert(device_tag.clone(), now);
                        } else {
                            inner.voice_closed_at.remove(device_tag);
                        }
                    }
                }
                self.update(device_tag, now);
            }
            GatewayEvent::StationChannel { address, channel, up } => {
                let Some(device) = self.stations.device_at(Some(address)) else { return };
                {
                    let mut inner = self.inner();
                    if channel == "microphone" {
                        if *up {
                            inner.microphone.insert(device.clone());
                        } else {
                            inner.microphone.remove(&device);
                        }
                    } else if *up {
                        inner.polled_at.insert(device.clone(), now_ms());
                    } else {
                        inner.polled_at.remove(&device);
                    }
                }
                self.update(&device, now_ms());
            }
            GatewayEvent::StationUpdated { device, .. } => self.update(device, now_ms()),
            _ => {}
        }
    }

    /// Before the database closes: keep every station with a channel up at «now», so after the restart the panel
    /// shows it reconnecting, not gone.
    pub fn stop(&self) {
        self.cancel.cancel();
        let devices: Vec<String> = {
            let inner = self.inner();
            inner.views.values().filter(|view| !view.channels.is_empty()).map(|view| view.device.clone()).collect()
        };
        let now = now_ms();
        for device in devices {
            self.stations.seen(&device, now);
        }
    }

    /// Every approved station's presence.
    pub fn list(&self) -> Vec<StationPresence> {
        self.inner().views.values().cloned().collect()
    }

    pub fn of(&self, device: &str) -> Option<StationPresence> {
        self.inner().views.get(device).cloned()
    }

    /// Looks at every approved station again: what ran out of time changes without an event of its own.
    pub fn refresh(&self, now: u64) {
        for station in self.stations.list() {
            self.update(&station.view.device, now);
        }
    }

    fn update(&self, device: &str, now: u64) {
        let approved = self.stations.find(device).is_some_and(|station| station.status == StationStatus::Approved);
        if !approved {
            self.inner().views.remove(device);
            return;
        }
        let mut inner = self.inner();
        let (state, channels) = presence_of(&PresenceInput {
            voice: inner.voice.get(device).is_some_and(|sessions| !sessions.is_empty()),
            microphone: inner.microphone.contains(device),
            polled_at: inner.polled_at.get(device).copied(),
            seen_before_start: inner.seen_before_start.get(device).copied(),
            voice_closed_at: inner.voice_closed_at.get(device).copied(),
            started_at: self.started_at,
            now,
        });
        // Heard since the start: the restart's «reconnecting» is over for it — gone later is gone.
        if !channels.is_empty() {
            inner.seen_before_start.remove(device);
        }
        let previous = inner.views.get(device).cloned();
        // The last channel just went: it was here until now — kept at once, not at the next minute.
        let lost = previous.as_ref().is_some_and(|view| !view.channels.is_empty()) && channels.is_empty();
        let written = inner.seen_written.get(device).copied().unwrap_or(0);
        if (!channels.is_empty() && now.saturating_sub(written) >= SEEN_EVERY_MS) || lost {
            inner.seen_written.insert(device.to_string(), now);
            // The database write does not need the presence lock.
            drop(inner);
            self.stations.seen(device, now);
            inner = self.inner();
        }
        let last_seen = if !channels.is_empty() || lost {
            Some(iso_at(now))
        } else {
            self.stations.find(device).map(|station| station.last_seen)
        };
        if let Some(previous) = &previous {
            if previous.state == state && previous.channels == channels {
                if let Some(view) = inner.views.get_mut(device) {
                    view.last_seen = last_seen;
                }
                return;
            }
        }
        let since = match &previous {
            Some(previous) if previous.state == state => previous.since.clone(),
            _ => iso_at(now),
        };
        let view = StationPresence { device: device.to_string(), state, channels: channels.clone(), last_seen, since };
        inner.views.insert(device.to_string(), view.clone());
        let changed_state = previous.as_ref().map(|previous| previous.state) != Some(state);
        drop(inner);
        if changed_state {
            let channels_label = channels.iter().map(channel_label).collect::<Vec<_>>().join(", ");
            let suffix = if channels.is_empty() { String::new() } else { format!(" ({channels_label})") };
            tracing::info!("station {device}: {}{suffix}", state_label(state));
        }
        self.events.emit(GatewayEvent::StationPresenceChanged { presence: view });
    }

    /// The gateway start, for a test to reason about the «reconnecting» window.
    #[cfg(test)]
    fn started_at(&self) -> u64 {
        self.started_at
    }
}

fn iso_at(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(crate::utils::time::iso)
        .unwrap_or_else(crate::utils::time::now_iso)
}

fn state_label(state: PresenceState) -> &'static str {
    match state {
        PresenceState::Voice => "voice",
        PresenceState::Alive => "alive",
        PresenceState::Reconnecting => "reconnecting",
        PresenceState::Offline => "offline",
    }
}

fn channel_label(channel: &StationChannel) -> &'static str {
    match channel {
        StationChannel::Voice => "voice",
        StationChannel::Microphone => "microphone",
        StationChannel::Calls => "calls",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StationPairing;
    use crate::storage::Db;

    /// A collector of `station.presence` emissions, each as `"state channels"`, in order.
    struct Changes {
        _task: tokio::task::JoinHandle<()>,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Changes {
        fn watch(events: &EventBus) -> Changes {
            let log: Arc<Mutex<Vec<String>>> = Arc::default();
            let sink = log.clone();
            let mut receiver = events.subscribe();
            let task = tokio::spawn(async move {
                while let Ok(event) = receiver.recv().await {
                    if let GatewayEvent::StationPresenceChanged { presence } = &*event {
                        let channels = presence.channels.iter().map(channel_label).collect::<Vec<_>>().join("+");
                        let label = format!("{} {channels}", state_label(presence.state));
                        sink.lock().unwrap().push(label.trim().to_string());
                    }
                }
            });
            Changes { _task: task, log }
        }

        fn list(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    fn build(pairing: StationPairing) -> (Arc<StationsService>, EventBus) {
        let events = EventBus::new();
        let stations = Arc::new(StationsService::new(Arc::new(Db::memory()), events.clone(), pairing).unwrap());
        (stations, events)
    }

    /// Waits until `check` holds, letting the presence task run; fails after a short while.
    async fn until(mut check: impl FnMut() -> bool) {
        for _ in 0..200 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("timed out");
    }

    #[tokio::test]
    async fn tells_a_station_by_what_it_opens_then_nothing() {
        let (stations, events) = build(StationPairing::Open);
        stations.admit("kitchen", "::ffff:192.168.1.100");
        let changes = Changes::watch(&events);
        let presence = Arc::new(PresenceService::new(stations.clone(), events.clone()));
        presence.start();

        // Heard just before this gateway started: it is reconnecting, not gone.
        assert_eq!(presence.of("kitchen").map(|view| view.state), Some(PresenceState::Reconnecting));

        events.emit(GatewayEvent::StationConnected { session_id: "s1".into(), device_tag: "kitchen".into() });
        until(|| presence.of("kitchen").map(|view| view.state) == Some(PresenceState::Voice)).await;
        assert_eq!(presence.of("kitchen").unwrap().channels, vec![StationChannel::Voice]);

        // Its microphone bridge, by the address its voice socket came from; a stranger's address is nobody's.
        events.emit(GatewayEvent::StationChannel {
            address: "192.168.1.100".into(),
            channel: "microphone".into(),
            up: true,
        });
        events.emit(GatewayEvent::StationChannel {
            address: "192.168.1.200".into(),
            channel: "calls".into(),
            up: true,
        });
        // Its voice socket broke without a close: it may be gone, and only the bridge is left.
        events.emit(GatewayEvent::StationDisconnected {
            session_id: "s1".into(),
            device_tag: "kitchen".into(),
            code: Some(1006),
        });
        until(|| presence.of("kitchen").map(|view| view.state) == Some(PresenceState::Alive)).await;
        assert_eq!(presence.of("kitchen").unwrap().channels, vec![StationChannel::Microphone]);

        let before = now_ms();
        events.emit(GatewayEvent::StationChannel {
            address: "192.168.1.100".into(),
            channel: "microphone".into(),
            up: false,
        });
        until(|| presence.of("kitchen").map(|view| view.state) == Some(PresenceState::Offline)).await;
        // Gone now: «была только что», kept in its record at once.
        let offline = presence.of("kitchen").unwrap();
        assert!(offline.last_seen.as_deref().map(parse_ms).unwrap_or(0) >= before);
        assert!(parse_ms(&stations.find("kitchen").unwrap().last_seen) >= before);

        until(|| changes.list().len() == 5).await;
        assert_eq!(
            changes.list(),
            vec![
                "reconnecting".to_string(),
                "voice voice".into(),
                "voice voice+microphone".into(),
                "alive microphone".into(),
                "offline".into(),
            ]
        );
        presence.stop();
    }

    #[tokio::test]
    async fn takes_a_call_client_polling_as_the_station_being_there_for_two_minutes() {
        let (stations, events) = build(StationPairing::Open);
        stations.admit("kids", "192.168.1.101");
        let presence = Arc::new(PresenceService::new(stations, events.clone()));
        presence.start();
        events.emit(GatewayEvent::StationChannel {
            address: "192.168.1.101".into(),
            channel: "calls".into(),
            up: true,
        });
        until(|| presence.of("kids").map(|view| view.state) == Some(PresenceState::Alive)).await;
        assert_eq!(presence.of("kids").unwrap().channels, vec![StationChannel::Calls]);
        presence.refresh(presence.started_at() + 121_000);
        assert_eq!(presence.of("kids").map(|view| view.state), Some(PresenceState::Offline));
        presence.stop();
    }

    #[tokio::test]
    async fn waits_for_a_station_whose_socket_closed_by_a_handshake_and_not_for_one_whose_socket_broke() {
        use crate::core::stations::presence::RECONNECT_WINDOW_MS;
        let (stations, events) = build(StationPairing::Open);
        stations.admit("hall", "192.168.1.102");
        let presence = Arc::new(PresenceService::new(stations, events.clone()));
        presence.start();
        let state = || presence.of("hall").map(|view| view.state);
        events.emit(GatewayEvent::StationConnected { session_id: "s1".into(), device_tag: "hall".into() });
        until(|| state() == Some(PresenceState::Voice)).await;
        // A mode switch: we closed its socket and it answered. It opens the next by itself, in seconds to minutes:
        // not «не в сети» meanwhile.
        events.emit(GatewayEvent::StationDisconnected {
            session_id: "s1".into(),
            device_tag: "hall".into(),
            code: Some(1000),
        });
        until(|| state() == Some(PresenceState::Reconnecting)).await;
        // Not back within the window: gone.
        presence.refresh(now_ms() + RECONNECT_WINDOW_MS);
        assert_eq!(state(), Some(PresenceState::Offline));
        // Back; then its socket broke (a reset) or went silent and was cut: it may be gone, offline at once.
        events.emit(GatewayEvent::StationConnected { session_id: "s2".into(), device_tag: "hall".into() });
        until(|| state() == Some(PresenceState::Voice)).await;
        events.emit(GatewayEvent::StationDisconnected {
            session_id: "s2".into(),
            device_tag: "hall".into(),
            code: Some(1006),
        });
        until(|| state() == Some(PresenceState::Offline)).await;
        presence.stop();
    }

    fn parse_ms(iso: &str) -> u64 {
        iso.parse::<chrono::DateTime<chrono::Utc>>().map(|at| at.timestamp_millis().max(0) as u64).unwrap_or(0)
    }
}
