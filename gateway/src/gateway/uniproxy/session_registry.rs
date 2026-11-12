//! Live station sessions plus the last closed ones, for the debug API and health. A station connection is answered
//! by us (gateway mode) or relayed to Yandex (relay mode), as the mode says when it connects.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use axum::extract::ws::WebSocket;
use futures::future::{BoxFuture, FutureExt};
use tokio_util::sync::CancellationToken;

use super::relay_session::{RelayDeps, RelaySession};
use super::spoken_answer::AnswerOutcome;
use super::station_session::{SayOutcome, SessionDeps, SessionSnapshot, StationSession};
use super::uniproxy_server::Handshake;
use crate::connectors::builtin::StationSessions;
use crate::core::events::{EventBus, GatewayEvent};
use crate::gateway::mode::{ModeService, SessionKind};
use crate::gateway::observability::session_recorder::SessionEventRecord;
use crate::health_status::{HealthSources, StationHealth};
use crate::operations::station_watch::ActiveSessions;
use crate::operations::StationWatch;
use crate::plugins::schedule::ScheduleStation;
use crate::plugins::skill::TurnStation;
use crate::skills::capabilities::Intent;
use crate::skills::own::reminders_skill::{AudioClip, Speaker};

const RECENT_CLOSED: usize = 20;

/// A station connection: answered by us (gateway mode) or relayed to Yandex (relay mode).
#[derive(Clone)]
pub enum AnySession {
    Station(Arc<StationSession>),
    Relay(Arc<RelaySession>),
}

impl AnySession {
    pub fn id(&self) -> &str {
        match self {
            AnySession::Station(session) => &session.id,
            AnySession::Relay(session) => &session.id,
        }
    }

    pub fn device_tag(&self) -> &str {
        match self {
            AnySession::Station(session) => &session.device_tag,
            AnySession::Relay(session) => &session.device_tag,
        }
    }

    pub fn remote_address(&self) -> &str {
        match self {
            AnySession::Station(session) => &session.remote_address,
            AnySession::Relay(session) => &session.remote_address,
        }
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        match self {
            AnySession::Station(session) => session.snapshot(),
            AnySession::Relay(session) => session.snapshot(),
        }
    }

    pub fn recent_events(&self, limit: usize) -> Vec<SessionEventRecord> {
        match self {
            AnySession::Station(session) => session.recent_events(limit),
            AnySession::Relay(session) => session.recent_events(limit),
        }
    }

    fn close_socket(&self, code: u16, reason: &str) {
        match self {
            AnySession::Station(session) => session.close_socket(code, reason),
            AnySession::Relay(session) => session.close_socket(code, reason),
        }
    }

    fn same(&self, other: &AnySession) -> bool {
        match (self, other) {
            (AnySession::Station(a), AnySession::Station(b)) => Arc::ptr_eq(a, b),
            (AnySession::Relay(a), AnySession::Relay(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

#[derive(Default)]
struct State {
    /// In the order they connected.
    active: Vec<AnySession>,
    /// Sockets let through the upgrade and not yet served, by slot and address: they count against the limits from
    /// the upgrade on, as ws's clients did.
    reserved: Vec<(u64, String)>,
    /// The last closed ones, newest first.
    closed: VecDeque<AnySession>,
}

pub struct SessionRegistry {
    deps: Arc<SessionDeps>,
    modes: Arc<ModeService>,
    events: EventBus,
    state: Mutex<State>,
    next_slot: AtomicU64,
    /// The gateway stops: every station socket is cut, as the TS terminated them before the server closed.
    stopping: CancellationToken,
    me: Weak<SessionRegistry>,
}

/// A socket's place among the limits, taken with the upgrade's count: it counts until its session is served, and
/// goes back when the upgrade never completes (dropped).
pub struct SocketSlot {
    registry: Weak<SessionRegistry>,
    id: u64,
}

impl Drop for SocketSlot {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            registry.lock().reserved.retain(|(id, _)| *id != self.id);
        }
    }
}

impl SessionRegistry {
    pub fn new(deps: SessionDeps) -> Arc<SessionRegistry> {
        Arc::new_cyclic(|me| SessionRegistry {
            modes: deps.modes.clone(),
            events: deps.events.clone(),
            deps: Arc::new(deps),
            state: Mutex::new(State::default()),
            next_slot: AtomicU64::new(1),
            stopping: CancellationToken::new(),
            me: me.clone(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Our reminders speak through the live station of their device; a mode switch reconnects the station. Inside
    /// the runtime.
    pub fn start(self: &Arc<Self>) {
        self.deps.skills.attach_speaker(Arc::new(RegistrySpeaker(self.me.clone())));
        // Delayed commands run as new requests on the live station of their device.
        self.deps.skills.later.attach_runner(Arc::new(RegistrySpeaker(self.me.clone())));
        // The player's retries through the gateway go to the live station of their device the same way.
        self.deps.skills.player_watch.set_pusher(Arc::new(RegistrySpeaker(self.me.clone())));
        let me = self.me.clone();
        self.events.on("sessions", move |event| {
            if let GatewayEvent::ModeChanged { .. } = &*event {
                if let Some(registry) = me.upgrade() {
                    registry.drop_all("mode switch");
                }
            }
        });
        tracing::info!(target: "uniproxy", "station voice endpoint: ws://<host>:<port>{}", self.deps.config.uniproxy_path);
    }

    /// The gateway stops: what would hold the server's close ends now.
    pub fn stop(&self) {
        self.stopping.cancel();
    }

    /// A station's socket, upgraded: its session in the mode the gateway has now, until it closes.
    pub async fn serve(self: Arc<Self>, socket: WebSocket, handshake: Handshake, slot: SocketSlot) {
        let (session, end) = match self.modes.session_kind() {
            SessionKind::Relay => {
                let deps = RelayDeps {
                    codec: self.deps.codec.clone(),
                    config: self.deps.config.clone(),
                    events: self.events.clone(),
                    requests: self.deps.requests.clone(),
                };
                let (session, outgoing) = RelaySession::new(&handshake, deps);
                self.opened(AnySession::Relay(session.clone()), slot);
                let end = session.run(socket, outgoing, handshake, self.stopping.clone()).await;
                (AnySession::Relay(session), end)
            }
            SessionKind::Gateway => {
                let (session, outgoing) = StationSession::new(&handshake, self.deps.clone());
                self.opened(AnySession::Station(session.clone()), slot);
                let end = session.run(socket, outgoing, handshake, self.stopping.clone()).await;
                (AnySession::Station(session), end)
            }
        };
        self.closed(session, end.code);
    }

    /// The session takes its slot's place in one stretch: the count neither drops nor doubles.
    fn opened(&self, session: AnySession, slot: SocketSlot) {
        let (session_id, device_tag) = (session.id().to_string(), session.device_tag().to_string());
        {
            let mut state = self.lock();
            state.active.push(session);
            state.reserved.retain(|(id, _)| *id != slot.id);
        }
        drop(slot);
        self.events.emit(GatewayEvent::StationConnected { session_id, device_tag });
    }

    /// Its socket ended with `code`: the presence tells a close handshake (the station comes back by itself) from a
    /// socket that broke or was cut (1006: it may be gone).
    fn closed(&self, session: AnySession, code: u16) {
        {
            let mut state = self.lock();
            state.active.retain(|active| !active.same(&session));
            state.closed.push_front(session.clone());
            state.closed.truncate(RECENT_CLOSED);
        }
        self.events.emit(GatewayEvent::StationDisconnected {
            session_id: session.id().to_string(),
            device_tag: session.device_tag().to_string(),
            code: Some(code),
        });
    }

    /// Closes the live station connections: the station reconnects at once, and the next session opens in the mode
    /// the gateway has now (the panel's Яндекс/Шлюз/Свой switch).
    pub fn drop_all(&self, reason: &str) {
        let active = self.lock().active.clone();
        for session in active {
            session.close_socket(1000, reason);
        }
    }

    pub fn active_count(&self) -> usize {
        self.lock().active.len()
    }

    /// A place for a socket from `address`, counted with the sockets open now and those let through before it, in
    /// one stretch: refused with those counts (in all, from the address) when either limit is reached.
    pub fn reserve(&self, address: &str, per_address: usize, in_all: usize) -> Result<SocketSlot, (usize, usize)> {
        let mut state = self.lock();
        let all = state.active.len() + state.reserved.len();
        let from_here = state.active.iter().filter(|session| session.remote_address() == address).count()
            + state.reserved.iter().filter(|(_, reserved)| reserved == address).count();
        if all >= in_all || from_here >= per_address {
            return Err((all, from_here));
        }
        let id = self.next_slot.fetch_add(1, Ordering::Relaxed);
        state.reserved.push((id, address.to_string()));
        Ok(SocketSlot { registry: self.me.clone(), id })
    }

    pub fn list(&self) -> Vec<SessionSnapshot> {
        let sessions: Vec<AnySession> = {
            let state = self.lock();
            state.active.iter().chain(state.closed.iter()).cloned().collect()
        };
        sessions.iter().map(AnySession::snapshot).collect()
    }

    /// Finds a session by full id or by its first characters (at least four), as printed in the log.
    pub fn find(&self, id_or_prefix: &str) -> Option<AnySession> {
        if id_or_prefix.chars().count() < 4 {
            return None;
        }
        let state = self.lock();
        state.active.iter().chain(state.closed.iter()).find(|session| session.id().starts_with(id_or_prefix)).cloned()
    }

    /// The newest live session that can speak: the station's main connection, not a short LogSpotter one.
    pub fn latest_speaking(&self) -> Option<Arc<StationSession>> {
        self.speaking_on("*")
    }

    /// The turn timer's way to the stations (plugins/skill.rs): «время вышло» as the answer to a push.
    pub fn turns(&self) -> Arc<dyn TurnStation> {
        Arc::new(RegistrySpeaker(self.me.clone()))
    }

    /// The plugins' schedules' way to the stations (plugins/schedule.rs): speech, whether one is busy, its zone.
    pub fn schedule_station(&self) -> Arc<dyn ScheduleStation> {
        Arc::new(RegistrySpeaker(self.me.clone()))
    }

    /// The newest live session of a device (`*`: any) that can speak.
    pub fn speaking_on(&self, device_tag: &str) -> Option<Arc<StationSession>> {
        let active = self.lock().active.clone();
        active.into_iter().rev().find_map(|session| match session {
            AnySession::Station(station)
                if (device_tag == "*" || station.device_tag == device_tag) && station.can_speak() =>
            {
                Some(station)
            }
            _ => None,
        })
    }
}

/// How the skills speak (reminders, calls announced): through the live station of the device.
struct RegistrySpeaker(Weak<SessionRegistry>);

impl Speaker for RegistrySpeaker {
    fn say(&self, device_tag: String, text: String, clip: Option<AudioClip>) -> BoxFuture<'static, bool> {
        let session = self.0.upgrade().and_then(|registry| registry.speaking_on(&device_tag));
        async move {
            match session {
                Some(session) => session.say(text, clip).await == SayOutcome::Answer(AnswerOutcome::Spoken),
                None => false,
            }
        }
        .boxed()
    }
}

/// The turn timer's station: the live station of the device answers a push with our plugins' skill.
impl TurnStation for RegistrySpeaker {
    fn ask(
        &self,
        device: &str,
        intent: Intent,
        label: String,
        still: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> BoxFuture<'static, bool> {
        let session = self.0.upgrade().and_then(|registry| registry.speaking_on(device));
        async move {
            match session {
                Some(session) => session.answer_pushed(intent, label, still).await,
                None => false,
            }
        }
        .boxed()
    }

    fn hearing(&self, device: &str) -> bool {
        self.0.upgrade().and_then(|registry| registry.speaking_on(device)).is_some_and(|session| session.hearing())
    }

    fn ringing(&self, device: &str) -> bool {
        self.0.upgrade().and_then(|registry| registry.speaking_on(device)).is_some_and(|session| session.ringing())
    }

    /// Ringing, words heard, a conversation going on, or music playing: «Готово» waits for a free station.
    fn busy(&self, device: &str) -> bool {
        self.0
            .upgrade()
            .and_then(|registry| registry.speaking_on(device))
            .is_some_and(|session| session.busy() || session.playing())
    }
}

/// The delayed commands' station: the words as a new request on the device's live station.
impl crate::skills::own::later_skill::CommandRunner for RegistrySpeaker {
    fn busy(&self, device: &str) -> bool {
        self.0.upgrade().and_then(|registry| registry.speaking_on(device)).is_some_and(|session| session.busy())
    }

    fn run(&self, device: &str, command: String) -> BoxFuture<'static, bool> {
        let session = self.0.upgrade().and_then(|registry| registry.speaking_on(device));
        async move {
            match session {
                Some(session) => session.run_words(command).await,
                None => false,
            }
        }
        .boxed()
    }
}

/// The schedules' station: speech on the device's live station (`*`: the newest connected), busy, its zone.
impl ScheduleStation for RegistrySpeaker {
    fn say(&self, device: &str, text: String) -> BoxFuture<'static, bool> {
        Speaker::say(self, device.to_string(), text, None)
    }

    fn busy(&self, device: &str) -> bool {
        self.0.upgrade().and_then(|registry| registry.speaking_on(device)).is_some_and(|session| session.busy())
    }

    fn timezone(&self) -> Option<String> {
        self.0.upgrade().and_then(|registry| registry.latest_speaking()).and_then(|session| session.zone())
    }
}

impl crate::skills::media::playback::Pusher for RegistrySpeaker {
    fn push(&self, device: &str, directive: crate::gateway::protocol::protocol_types::Directive) -> bool {
        match self.0.upgrade().and_then(|registry| registry.speaking_on(device)) {
            Some(session) => {
                session.push(directive);
                true
            }
            None => false,
        }
    }
}

/// The offline watch counts the live sessions.
impl ActiveSessions for SessionRegistry {
    fn active_count(&self) -> usize {
        SessionRegistry::active_count(self)
    }
}

/// The Yandex connector's view: the stations connected, and whether our session with Yandex is up for one.
impl StationSessions for SessionRegistry {
    fn open(&self) -> usize {
        self.active_count()
    }

    fn yandex_connected(&self) -> bool {
        let active = self.lock().active.clone();
        active.iter().any(|session| matches!(session, AnySession::Station(station) if station.yandex_connected()))
    }
}

/// The station link's part of /health: the sessions open now, and whether a station is connected (the offline
/// watch's view).
pub struct StationLinkHealth {
    pub sessions: Arc<SessionRegistry>,
    pub watch: Arc<StationWatch>,
}

impl HealthSources for StationLinkHealth {
    fn active_sessions(&self) -> Option<usize> {
        Some(self.sessions.active_count())
    }

    fn station(&self) -> Option<StationHealth> {
        let status = self.watch.status();
        Some(StationHealth { connected: status.connected, last_seen_at: status.last_seen_at })
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::config::{Config, Env};

    #[tokio::test]
    async fn counts_a_socket_from_its_upgrade_on_until_its_session_is_served_or_its_slot_goes() {
        let app = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
        let sessions = &app.sessions;
        let first = sessions.reserve("192.168.1.50", 2, 3).unwrap();
        let _second = sessions.reserve("192.168.1.50", 2, 3).unwrap();
        // Neither is served yet, and a third from that address is already one too many.
        assert_eq!(sessions.reserve("192.168.1.50", 2, 3).err(), Some((2, 2)));
        let _other = sessions.reserve("192.168.1.51", 2, 3).unwrap();
        assert_eq!(sessions.reserve("192.168.1.52", 2, 3).err(), Some((3, 0)), "in all");
        // An upgrade that never completed gives its place back.
        drop(first);
        assert!(sessions.reserve("192.168.1.50", 2, 3).is_ok());
    }

    #[tokio::test]
    async fn lets_through_no_more_than_the_limit_of_upgrades_that_come_at_once() {
        let app = App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
        let sessions = &app.sessions;
        let slots: Vec<_> = std::thread::scope(|scope| {
            let burst: Vec<_> =
                (0..40).map(|_| scope.spawn(|| sessions.reserve("192.168.1.60", 16, 64).ok())).collect();
            burst.into_iter().map(|upgrade| upgrade.join().unwrap()).collect()
        });
        assert_eq!(slots.iter().flatten().count(), 16);
    }
}
