//! A MIDI keyboard for apps (docs/plans/midi.md): the gateway on a computer hears the keyboard plugged into it and
//! plays it, for the apps that have the right `midi`. The keyboard is open only while someone listens: an input is
//! opened for its first listener and let go 10 s after the last, an output 30 s after its last message. Notes go only
//! to the page that asked (the channel of notes, socket.rs) — never to a journal, the database, a plugin's backend or
//! the workshop's agent.
//!
//! Who listens is an «owner»: one page of one app. A socket carries an owner; the owner outlives its socket for
//! `LINGER`, so a page that renews its socket (every 15 minutes, as the live channel) takes it over whole — what it
//! listens to, the notes it holds, the melody it plays — and nothing is lost on the joint. An owner gone for good has
//! every note it turned on let go, «all notes off» and the pedal up on the channels it touched: no note sounds forever.

pub mod file;
pub mod ports;
pub mod routes;
pub mod socket;
pub mod wire;

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, Notify};

use self::ports::{InputLink, OutputLink, PortInfo, Ports};
use self::wire::{Message, Step};
use crate::core::events::{EventBus, GatewayEvent};
use crate::platform::http::Zone;

/// An input stays open this long after its last listener: a page reloaded does not open it anew.
pub const INPUT_HOLD: Duration = Duration::from_secs(10);
/// An output stays open this long after its last message.
pub const OUTPUT_HOLD: Duration = Duration::from_secs(30);
/// An owner without its socket waits this long for the page to take it again, then lets everything go.
pub const LINGER: Duration = Duration::from_secs(15);
/// Windows and Linux are asked for their keyboards this often while someone listens (a Mac tells by itself).
pub const POLL: Duration = Duration::from_secs(2);
/// Pages that hear the keyboard at once, on this gateway.
pub const MAX_OWNERS: usize = 16;
/// Messages an owner sends the keyboard in a second, at most.
pub const SEND_RATE: u32 = 1000;
/// Events kept for a listener that falls behind; past it it loses the oldest and is told so.
const HEARD_QUEUE: usize = 1024;

/// What a keyboard played, as the listeners hear it: the input port, the message, the gateway's time in ms and a
/// number that only grows — a page sees no event twice and knows when it lost some.
#[derive(Debug, Clone)]
pub struct Heard {
    pub port: String,
    pub message: Message,
    pub at: u64,
    pub seq: u64,
}

impl Heard {
    pub fn frame(&self) -> Value {
        let mut fields = self.message.fields();
        fields.insert("d".into(), json!(self.port));
        fields.insert("at".into(), json!(self.at));
        fields.insert("s".into(), json!(self.seq));
        Value::Object(fields)
    }
}

/// A keyboard: its input and output ports glued by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Device {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// Ports as keyboards: an input and an output of one name are one keyboard.
pub fn devices_of(ports: &[PortInfo]) -> Vec<Device> {
    let mut devices: Vec<Device> = Vec::new();
    for port in ports {
        let at = devices.iter().position(|device| {
            device.name == port.name && if port.input { device.input.is_none() } else { device.output.is_none() }
        });
        let device = match at {
            Some(at) => &mut devices[at],
            None => {
                devices.push(Device { name: port.name.clone(), input: None, output: None });
                devices.last_mut().expect("just pushed")
            }
        };
        if port.input {
            device.input = Some(port.id.clone());
        } else {
            device.output = Some(port.id.clone());
        }
    }
    devices
}

/// Who listens now, for the panel: the app, where its page is, which keyboards.
#[derive(Debug, Clone, Serialize)]
pub struct ListenerView {
    pub package: String,
    pub zone: Zone,
    pub devices: Vec<String>,
}

/// `GET /api/v1/midi`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiView {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    pub devices: Vec<Device>,
    pub listeners: Vec<ListenerView>,
}

/// May this app's page hear the keyboard: the app is on here, and its version in force has the right `midi`.
pub type Gate = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;
/// A line in an app's own journal: (app, line).
pub type Journal = Arc<dyn Fn(&str, &str) + Send + Sync>;

struct InputHold {
    _link: Box<dyn InputLink>,
    name: String,
    users: usize,
    idle_since: Option<Instant>,
}

struct OutputHold {
    link: Box<dyn OutputLink>,
    last: Instant,
}

struct Owner {
    package: String,
    zone: Zone,
    token: String,
    /// The page's socket, while it has one: frames that are not notes (a melody's progress, an error).
    outbox: Option<mpsc::Sender<Value>>,
    detached_at: Option<Instant>,
    listening: BTreeSet<String>,
    /// Notes it turned on and has not let go: (output, channel, note).
    held: BTreeSet<(String, u8, u8)>,
    /// Channels it touched, and those with the pedal down: let go when it goes.
    touched: BTreeSet<(String, u8)>,
    pedal: BTreeSet<(String, u8)>,
    play: Option<(String, tokio::task::AbortHandle)>,
    second: (Instant, u32),
}

#[derive(Default)]
struct State {
    ports: Vec<PortInfo>,
    devices: Vec<Device>,
    inputs: HashMap<String, InputHold>,
    outputs: HashMap<String, OutputHold>,
    owners: HashMap<u64, Owner>,
    next_owner: u64,
}

pub struct MidiService {
    ports: Arc<dyn Ports>,
    /// Why there is no MIDI here: the build, or `MIDI=0`.
    off: Option<String>,
    /// `MIDI_REMOTE=1`: pages from outside home hear the keyboard too (the owner's decision 2: no by default).
    remote: bool,
    started: Instant,
    seq: Arc<AtomicU64>,
    heard: broadcast::Sender<Heard>,
    changes: broadcast::Sender<Vec<Device>>,
    state: Mutex<State>,
    events: OnceLock<EventBus>,
    gate: OnceLock<Gate>,
    journal: OnceLock<Journal>,
    /// The system tells of keyboards plugged in or out by itself (a Mac): no polling.
    watching: AtomicBool,
    nudge: Arc<Notify>,
    stopped: tokio_util::sync::CancellationToken,
    /// The channel of notes' tickets (socket.rs).
    pub tickets: socket::Tickets,
}

impl MidiService {
    /// `off` — why MIDI is off here though the build has it (`MIDI=0`).
    pub fn new(ports: Arc<dyn Ports>, off: Option<String>, remote: bool) -> Arc<MidiService> {
        let off = ports.absent().or(off);
        Arc::new(MidiService {
            ports,
            off,
            remote,
            started: Instant::now(),
            seq: Arc::new(AtomicU64::new(0)),
            heard: broadcast::channel(HEARD_QUEUE).0,
            changes: broadcast::channel(16).0,
            state: Mutex::new(State::default()),
            events: OnceLock::new(),
            gate: OnceLock::new(),
            journal: OnceLock::new(),
            watching: AtomicBool::new(false),
            nudge: Arc::new(Notify::new()),
            stopped: tokio_util::sync::CancellationToken::new(),
            tickets: socket::Tickets::default(),
        })
    }

    pub fn set_events(&self, events: EventBus) {
        let _ = self.events.set(events);
    }

    pub fn set_gate(&self, gate: Gate) {
        let _ = self.gate.set(gate);
    }

    pub fn set_journal(&self, journal: Journal) {
        let _ = self.journal.set(journal);
    }

    pub fn available(&self) -> bool {
        self.off.is_none()
    }

    /// Why there is no MIDI here, if there is none.
    pub fn why_off(&self) -> Option<&str> {
        self.off.as_deref()
    }

    /// Milliseconds since the service started: the time of the events.
    pub fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn note(&self, package: &str, line: &str) {
        if let Some(journal) = self.journal.get() {
            journal(package, line);
        }
    }

    /// The system's news of keyboards first (a Mac: before any other client), then the list, then the keeper: idle
    /// ports let go, owners without a page let go, keyboards asked for again where the system does not tell.
    pub fn start(self: &Arc<Self>) {
        if self.off.is_some() {
            return;
        }
        let nudge = self.nudge.clone();
        let watching = self.ports.watch(Arc::new(move || nudge.notify_one()));
        self.watching.store(watching, Ordering::Relaxed);
        if watching {
            tracing::info!(target: "midi", "the system tells of keyboards plugged in or out");
        }
        self.refresh();
        let me = Arc::downgrade(self);
        let (nudge, stopped) = (self.nudge.clone(), self.stopped.clone());
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            let mut polled = Instant::now();
            loop {
                tokio::select! {
                    _ = stopped.cancelled() => return,
                    _ = nudge.notified() => {
                        let Some(service) = me.upgrade() else { return };
                        service.refresh();
                    }
                    _ = tick.tick() => {
                        let Some(service) = me.upgrade() else { return };
                        service.keep(Instant::now());
                        let listened = !service.lock().owners.is_empty();
                        if !service.watching.load(Ordering::Relaxed) && listened && polled.elapsed() >= POLL {
                            polled = Instant::now();
                            service.refresh();
                        }
                    }
                }
            }
        });
    }

    /// The gateway stops: every owner lets its notes go.
    pub fn stop(&self) {
        self.stopped.cancel();
        let owners: Vec<u64> = self.lock().owners.keys().copied().collect();
        for owner in owners {
            self.release(owner, "шлюз остановился");
        }
    }

    /// Read the keyboards again; a change goes to the listeners and the panels.
    pub fn refresh(&self) -> Vec<Device> {
        if self.off.is_some() {
            return Vec::new();
        }
        let ports = match self.ports.list() {
            Ok(ports) => ports,
            Err(error) => {
                tracing::warn!(target: "midi", "the keyboards are not read: {error}");
                return self.lock().devices.clone();
            }
        };
        let devices = devices_of(&ports);
        let changed = {
            let mut state = self.lock();
            state.ports = ports;
            let changed = state.devices != devices;
            state.devices = devices.clone();
            changed
        };
        if changed {
            let names: Vec<String> = devices.iter().map(|device| device.name.clone()).collect();
            tracing::info!(target: "midi", "keyboards: {}", if names.is_empty() { "none".into() } else { names.join(", ") });
            let _ = self.changes.send(devices.clone());
            if let Some(events) = self.events.get() {
                events.emit(GatewayEvent::MidiDevices { devices: names });
            }
        }
        devices
    }

    pub fn devices(&self) -> Vec<Device> {
        self.lock().devices.clone()
    }

    /// For the panel: the keyboards read now, and who listens.
    pub fn view(&self) -> MidiView {
        let devices = self.refresh();
        let state = self.lock();
        let mut listeners: Vec<ListenerView> = state
            .owners
            .values()
            .filter(|owner| owner.detached_at.is_none() && !owner.listening.is_empty())
            .map(|owner| ListenerView {
                package: owner.package.clone(),
                zone: owner.zone,
                devices: owner
                    .listening
                    .iter()
                    .map(|port| state.inputs.get(port).map(|hold| hold.name.clone()).unwrap_or_else(|| port.clone()))
                    .collect(),
            })
            .collect();
        listeners.sort_by(|a, b| a.package.cmp(&b.package));
        MidiView { available: self.off.is_none(), why: self.off.clone(), devices, listeners }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Heard> {
        self.heard.subscribe()
    }

    pub fn device_changes(&self) -> broadcast::Receiver<Vec<Device>> {
        self.changes.subscribe()
    }

    /// May a page of this app, from this zone, hear the keyboard: MIDI here, home only (decision 2), the app's right.
    pub fn may_hear(&self, package: &str, zone: Zone) -> Result<(), String> {
        if let Some(why) = &self.off {
            return Err(why.clone());
        }
        if zone == Zone::Remote && !self.remote {
            return Err("Клавиатуру слышно только дома: пульт вне дома её не слышит (MIDI_REMOTE).".into());
        }
        match self.gate.get() {
            Some(gate) => gate(package),
            None => Err("приложения здесь не запущены".into()),
        }
    }

    /// A page of an app comes: a new owner, and its token to take it again from a new socket.
    pub fn attach(&self, package: &str, zone: Zone, outbox: mpsc::Sender<Value>) -> Result<(u64, String), String> {
        let mut state = self.lock();
        if state.owners.len() >= MAX_OWNERS {
            return Err(format!("клавиатуру уже слушают {MAX_OWNERS} страниц — закройте лишние"));
        }
        state.next_owner += 1;
        let id = state.next_owner;
        let token = crate::utils::ids::random_hex(16);
        state.owners.insert(
            id,
            Owner {
                package: package.to_string(),
                zone,
                token: token.clone(),
                outbox: Some(outbox),
                detached_at: None,
                listening: BTreeSet::new(),
                held: BTreeSet::new(),
                touched: BTreeSet::new(),
                pedal: BTreeSet::new(),
                play: None,
                second: (Instant::now(), 0),
            },
        );
        Ok((id, token))
    }

    /// A new socket of the same page takes the owner it had: everything moves to `owner` (the socket's new one), the
    /// old one is gone without letting anything go. False when the token is no one's of this app.
    pub fn resume(&self, owner: u64, token: &str) -> bool {
        let mut state = self.lock();
        let Some(package) = state.owners.get(&owner).map(|owner| owner.package.clone()) else { return false };
        let Some(old_id) = state
            .owners
            .iter()
            .find(|(id, old)| **id != owner && old.token == token && old.package == package)
            .map(|(id, _)| *id)
        else {
            return false;
        };
        let old = state.owners.remove(&old_id).expect("found above");
        let new = state.owners.get_mut(&owner).expect("checked above");
        // The new owner keeps its own token: the next socket takes it with that one.
        new.listening.extend(old.listening);
        new.held.extend(old.held);
        new.touched.extend(old.touched);
        new.pedal.extend(old.pedal);
        if new.play.is_none() {
            new.play = old.play;
        } else if let Some((_, handle)) = old.play {
            handle.abort();
        }
        true
    }

    /// Whether the owner is still this socket's and may still hear: false — the socket closes.
    pub fn still(&self, owner: u64) -> Result<(), String> {
        let package = {
            let state = self.lock();
            match state.owners.get(&owner) {
                Some(owner) => owner.package.clone(),
                None => return Err("канал перешёл на новую страницу".into()),
            }
        };
        match self.gate.get().map(|gate| gate(&package)) {
            Some(Err(why)) => {
                self.release(owner, &why);
                Err(why)
            }
            _ => Ok(()),
        }
    }

    /// The socket is gone: the owner waits `LINGER` for the page to take it again.
    pub fn detach(&self, owner: u64) {
        if let Some(owner) = self.lock().owners.get_mut(&owner) {
            owner.outbox = None;
            owner.detached_at = Some(Instant::now());
        }
    }

    /// Whether the owner listens to this input.
    pub fn hears(&self, owner: u64, port: &str) -> bool {
        self.lock().owners.get(&owner).is_some_and(|owner| owner.listening.contains(port))
    }

    /// Listen to an input (`*` — every keyboard): opened for its first listener.
    pub fn listen(&self, owner: u64, port: &str) -> Result<Vec<String>, String> {
        let wanted: Vec<String> = {
            let state = self.lock();
            if port == "*" {
                state.ports.iter().filter(|info| info.input).map(|info| info.id.clone()).collect()
            } else {
                vec![port.to_string()]
            }
        };
        let mut opened = Vec::new();
        for port in wanted {
            self.listen_one(owner, &port)?;
            opened.push(port);
        }
        Ok(opened)
    }

    fn listen_one(&self, owner: u64, port: &str) -> Result<(), String> {
        let mut state = self.lock();
        let Some(package) = state.owners.get(&owner).map(|owner| owner.package.clone()) else {
            return Err("канала нет".into());
        };
        if state.owners.get(&owner).is_some_and(|owner| owner.listening.contains(port)) {
            return Ok(());
        }
        if !state.inputs.contains_key(port) {
            let name = state
                .ports
                .iter()
                .find(|info| info.input && info.id == port)
                .map(|info| info.name.clone())
                .ok_or_else(|| "Такой клавиатуры нет: её отключили?".to_string())?;
            let link = self.ports.open_input(port, self.sink(port))?;
            tracing::info!(target: "midi", "keyboard «{name}» opened");
            state.inputs.insert(port.to_string(), InputHold { _link: link, name, users: 0, idle_since: None });
        }
        let hold = state.inputs.get_mut(port).expect("opened above");
        hold.users += 1;
        hold.idle_since = None;
        let name = hold.name.clone();
        state.owners.get_mut(&owner).expect("checked above").listening.insert(port.to_string());
        drop(state);
        self.note(&package, &format!("MIDI: открыта клавиатура «{name}»"));
        Ok(())
    }

    /// What an open input hands on: a channel message, numbered and timed, to every listener at once. It never
    /// waits — the system's thread calls it.
    fn sink(&self, port: &str) -> ports::Sink {
        let (port, heard, seq, started) = (port.to_string(), self.heard.clone(), self.seq.clone(), self.started);
        Box::new(move |_stamp, bytes| {
            if let Some(message) = Message::parse(bytes) {
                let seq = seq.fetch_add(1, Ordering::Relaxed) + 1;
                let at = started.elapsed().as_millis() as u64;
                let _ = heard.send(Heard { port: port.clone(), message, at, seq });
            }
        })
    }

    pub fn unlisten(&self, owner: u64, port: &str) {
        let mut state = self.lock();
        let ports: Vec<String> = match state.owners.get_mut(&owner) {
            Some(owner) if port == "*" => std::mem::take(&mut owner.listening).into_iter().collect(),
            Some(owner) => owner.listening.take(port).into_iter().collect(),
            None => return,
        };
        let now = Instant::now();
        for port in ports {
            if let Some(hold) = state.inputs.get_mut(&port) {
                hold.users = hold.users.saturating_sub(1);
                if hold.users == 0 {
                    hold.idle_since = Some(now);
                }
            }
        }
    }

    /// A message to a keyboard from an owner: counted against its rate, its notes and pedal remembered to be let go.
    pub fn send(&self, owner: u64, port: &str, message: Message) -> Result<(), String> {
        let mut state = self.lock();
        let now = Instant::now();
        {
            let Some(who) = state.owners.get_mut(&owner) else { return Err("канала нет".into()) };
            if now.duration_since(who.second.0) >= Duration::from_secs(1) {
                who.second = (now, 0);
            }
            who.second.1 += 1;
            if who.second.1 > SEND_RATE {
                return Err(format!("больше {SEND_RATE} сообщений в секунду — лишнее отброшено"));
            }
        }
        if !state.outputs.contains_key(port) {
            if !state.ports.iter().any(|info| !info.input && info.id == port) {
                return Err("Такой клавиатуры нет: её отключили?".into());
            }
            let link = self.ports.open_output(port)?;
            state.outputs.insert(port.to_string(), OutputHold { link, last: now });
        }
        let hold = state.outputs.get_mut(port).expect("opened above");
        hold.last = now;
        hold.link.send(&message.bytes())?;
        let who = state.owners.get_mut(&owner).expect("checked above");
        let channel = message.channel();
        who.touched.insert((port.to_string(), channel));
        match message {
            Message::On { note, .. } => {
                who.held.insert((port.to_string(), channel, note));
            }
            Message::Off { note, .. } => {
                who.held.remove(&(port.to_string(), channel, note));
            }
            Message::Control { control: 64, value, .. } if value >= 64 => {
                who.pedal.insert((port.to_string(), channel));
            }
            Message::Control { control: 64, .. } => {
                who.pedal.remove(&(port.to_string(), channel));
            }
            Message::Control { control: 120 | 123, .. } => {
                who.held.retain(|(held_port, held_channel, _)| !(held_port == port && *held_channel == channel));
            }
            _ => {}
        }
        Ok(())
    }

    /// A melody for the keyboard, played by the gateway's clock: the page may go to the background. A new one of the
    /// same owner takes the place of the old; the page hears `playing` per note and `played` at the end.
    pub fn play(self: &Arc<Self>, owner: u64, id: &str, port: &str, steps: Vec<Step>) -> Result<(), String> {
        self.stop_play(owner);
        if !self.lock().ports.iter().any(|info| !info.input && info.id == port) {
            return Err("Такой клавиатуры нет: её отключили?".into());
        }
        // Every note's start and end, in time order; at one moment the ends go first.
        let mut moments: Vec<(u64, bool, usize)> = Vec::with_capacity(steps.len() * 2);
        for (i, step) in steps.iter().enumerate() {
            moments.push((step.at_ms, true, i));
            moments.push((step.at_ms + step.length_ms.max(1), false, i));
        }
        moments.sort_by_key(|(at, on, _)| (*at, *on));
        let me = Arc::downgrade(self);
        let (id, port) = (id.to_string(), port.to_string());
        let task_id = id.clone();
        let task = tokio::spawn(async move {
            let start = tokio::time::Instant::now();
            for (at, on, i) in moments {
                tokio::time::sleep_until(start + Duration::from_millis(at)).await;
                let Some(service) = me.upgrade() else { return };
                let step = steps[i];
                let message = if on {
                    Message::On { channel: step.channel, note: step.note, velocity: step.velocity }
                } else {
                    Message::Off { channel: step.channel, note: step.note, velocity: 0 }
                };
                if let Err(why) = service.send(owner, &port, message) {
                    service.tell(owner, json!({ "t": "why", "text": why, "id": task_id }));
                    return;
                }
                if on {
                    service.tell(owner, json!({ "t": "playing", "id": task_id, "i": i }));
                }
            }
            if let Some(service) = me.upgrade() {
                if let Some(who) = service.lock().owners.get_mut(&owner) {
                    who.play = None;
                }
                service.tell(owner, json!({ "t": "played", "id": task_id }));
            }
        });
        match self.lock().owners.get_mut(&owner) {
            Some(who) => who.play = Some((id, task.abort_handle())),
            None => task.abort(),
        }
        Ok(())
    }

    /// Stop the owner's melody and let its notes go.
    pub fn stop_play(&self, owner: u64) {
        let play = self.lock().owners.get_mut(&owner).and_then(|who| who.play.take());
        if let Some((_, handle)) = play {
            handle.abort();
            self.let_go(owner);
        }
    }

    /// A frame to the owner's page, if it has one now; a page that cannot take it loses it.
    fn tell(&self, owner: u64, frame: Value) {
        let outbox = self.lock().owners.get(&owner).and_then(|who| who.outbox.clone());
        if let Some(outbox) = outbox {
            let _ = outbox.try_send(frame);
        }
    }

    /// Every note the owner holds let go, the pedal up.
    fn let_go(&self, owner: u64) {
        let (held, pedal) = {
            let mut state = self.lock();
            let Some(who) = state.owners.get_mut(&owner) else { return };
            (std::mem::take(&mut who.held), std::mem::take(&mut who.pedal))
        };
        for (port, channel, note) in held {
            let _ = self.raw(&port, Message::Off { channel, note, velocity: 0 });
        }
        for (port, channel) in pedal {
            let _ = self.raw(&port, Message::Control { channel, control: 64, value: 0 });
        }
    }

    /// A message past the owners' counting: the service's own, to let go.
    fn raw(&self, port: &str, message: Message) -> Result<(), String> {
        let mut state = self.lock();
        let hold = state.outputs.get_mut(port).ok_or_else(|| "выход закрыт".to_string())?;
        hold.last = Instant::now();
        hold.link.send(&message.bytes())
    }

    /// The owner goes for good: its melody stops, its notes are let go, «all notes off» and the pedal up on the
    /// channels it touched, its inputs left.
    pub fn release(&self, owner: u64, why: &str) {
        let play = self.lock().owners.get_mut(&owner).and_then(|who| who.play.take());
        if let Some((_, handle)) = play {
            handle.abort();
        }
        self.let_go(owner);
        let touched = self.lock().owners.get(&owner).map(|who| who.touched.clone()).unwrap_or_default();
        for (port, channel) in touched {
            let _ = self.raw(&port, Message::Control { channel, control: 123, value: 0 });
            let _ = self.raw(&port, Message::Control { channel, control: 64, value: 0 });
        }
        self.unlisten(owner, "*");
        let gone = self.lock().owners.remove(&owner);
        if let Some(gone) = gone {
            tracing::debug!(target: "midi", "{}: a page let the keyboard go ({why})", gone.package);
            self.note(&gone.package, "MIDI: клавиатура закрыта");
        }
    }

    /// Every page of an app lets the keyboard go: the app was switched off, removed, or lost the right.
    pub fn release_package(&self, package: &str, why: &str) {
        let owners: Vec<u64> =
            self.lock().owners.iter().filter(|(_, owner)| owner.package == package).map(|(id, _)| *id).collect();
        for owner in owners {
            self.release(owner, why);
        }
    }

    /// The keeper's second: owners without a page past `LINGER` let go, idle inputs and outputs closed.
    pub fn keep(&self, now: Instant) {
        let gone: Vec<u64> = self
            .lock()
            .owners
            .iter()
            .filter(|(_, owner)| owner.detached_at.is_some_and(|at| now.duration_since(at) >= LINGER))
            .map(|(id, _)| *id)
            .collect();
        for owner in gone {
            self.release(owner, "страница не вернулась");
        }
        let mut state = self.lock();
        state.inputs.retain(|_, hold| hold.idle_since.is_none_or(|since| now.duration_since(since) < INPUT_HOLD));
        let used: BTreeSet<String> =
            state.owners.values().flat_map(|owner| owner.held.iter().map(|(port, _, _)| port.clone())).collect();
        state.outputs.retain(|port, hold| used.contains(port) || now.duration_since(hold.last) < OUTPUT_HOLD);
    }

    /// Whether an input is open now (tests, the panel's line).
    pub fn input_open(&self, port: &str) -> bool {
        self.lock().inputs.contains_key(port)
    }
}

#[cfg(test)]
mod tests {
    use super::ports::{Fake, FakePorts};
    use super::*;

    fn service() -> (Arc<MidiService>, Arc<FakePorts>) {
        let fake = FakePorts::new();
        fake.plug("1", "Keystation 49");
        let service = MidiService::new(Arc::new(Fake(fake.clone())), None, false);
        service.set_gate(Arc::new(
            |package: &str| {
                if package == "teacher" {
                    Ok(())
                } else {
                    Err("нет права midi".into())
                }
            },
        ));
        service.refresh();
        (service, fake)
    }

    fn owner(service: &MidiService) -> (u64, mpsc::Receiver<Value>) {
        let (outbox, frames) = mpsc::channel(64);
        (service.attach("teacher", Zone::Local, outbox).unwrap().0, frames)
    }

    #[test]
    fn inputs_and_outputs_of_one_name_are_one_keyboard() {
        let ports = [
            PortInfo { id: "a".into(), name: "Keys".into(), input: true },
            PortInfo { id: "b".into(), name: "Keys".into(), input: false },
            PortInfo { id: "c".into(), name: "Pads".into(), input: true },
            PortInfo { id: "d".into(), name: "Keys".into(), input: true },
        ];
        let devices = devices_of(&ports);
        assert_eq!(devices.len(), 3);
        assert_eq!(devices[0], Device { name: "Keys".into(), input: Some("a".into()), output: Some("b".into()) });
        assert_eq!(devices[2].input.as_deref(), Some("d"), "a second keyboard of the same name");
    }

    #[test]
    fn who_may_hear() {
        let (service, _) = service();
        assert!(service.may_hear("teacher", Zone::Local).is_ok());
        assert!(service.may_hear("teacher", Zone::Home).is_ok());
        assert!(service.may_hear("teacher", Zone::Remote).unwrap_err().contains("только дома"));
        assert!(service.may_hear("weather", Zone::Local).is_err());
        let absent = MidiService::new(Arc::new(ports::Absent("нет MIDI".into())), None, true);
        assert_eq!(absent.may_hear("teacher", Zone::Local).unwrap_err(), "нет MIDI");
        let off = MidiService::new(Arc::new(Fake(FakePorts::new())), Some("MIDI=0".into()), true);
        assert!(!off.available());
    }

    #[tokio::test]
    async fn notes_reach_the_listeners_of_that_keyboard_only() {
        let (service, fake) = service();
        let (one, _) = owner(&service);
        let (two, _) = owner(&service);
        let mut heard = service.subscribe();
        assert!(!fake.press("in-1", 0, &[0x90, 60, 90]), "closed until someone listens");
        service.listen(one, "in-1").unwrap();
        service.listen(two, "*").unwrap();
        assert_eq!(fake.opened(), ["in-1"], "opened once for two listeners");
        fake.press("in-1", 0, &[0xF8]);
        fake.press("in-1", 0, &[0x90, 60, 90]);
        let event = heard.recv().await.unwrap();
        assert_eq!(event.message, Message::On { channel: 1, note: 60, velocity: 90 }, "the clock was dropped");
        assert!(service.hears(one, "in-1") && service.hears(two, "in-1"));
        assert_eq!(event.frame()["d"], "in-1");
        fake.press("in-1", 0, &[0x90, 60, 0]);
        let next = heard.recv().await.unwrap();
        assert!(next.seq > event.seq);
        assert!(matches!(next.message, Message::Off { .. }));
    }

    #[test]
    fn an_input_is_let_go_ten_seconds_after_its_last_listener() {
        let (service, fake) = service();
        let (one, _) = owner(&service);
        service.listen(one, "in-1").unwrap();
        service.unlisten(one, "in-1");
        let now = Instant::now();
        service.keep(now + INPUT_HOLD - Duration::from_secs(1));
        assert!(fake.is_open("in-1"), "a reload does not close it");
        service.listen(one, "in-1").unwrap();
        service.unlisten(one, "in-1");
        service.keep(Instant::now() + INPUT_HOLD);
        assert!(!fake.is_open("in-1"));
        assert!(!service.input_open("in-1"));
    }

    #[test]
    fn a_page_gone_lets_its_notes_and_pedal_go() {
        let (service, fake) = service();
        let (one, _) = owner(&service);
        service.send(one, "out-1", Message::On { channel: 1, note: 60, velocity: 90 }).unwrap();
        service.send(one, "out-1", Message::On { channel: 1, note: 64, velocity: 90 }).unwrap();
        service.send(one, "out-1", Message::Off { channel: 1, note: 64, velocity: 0 }).unwrap();
        service.send(one, "out-1", Message::Control { channel: 1, control: 64, value: 127 }).unwrap();
        service.detach(one);
        service.keep(Instant::now() + LINGER - Duration::from_secs(1));
        let before = fake.sent().len();
        assert_eq!(before, 4, "nothing let go while the page may come back");
        service.keep(Instant::now() + LINGER);
        let sent: Vec<Vec<u8>> = fake.sent().into_iter().skip(before).map(|(_, bytes)| bytes).collect();
        assert!(sent.contains(&vec![0x80, 60, 0]), "the held note let go");
        assert!(!sent.contains(&vec![0x80, 64, 0]), "only what was held");
        assert!(sent.contains(&vec![0xB0, 64, 0]), "the pedal up");
        assert!(sent.contains(&vec![0xB0, 123, 0]), "all notes off on the channel touched");
        assert!(service.view().listeners.is_empty());
    }

    #[test]
    fn a_new_socket_takes_the_owner_whole() {
        let (service, fake) = service();
        let (outbox, _frames) = mpsc::channel(8);
        let (old, token) = service.attach("teacher", Zone::Local, outbox).unwrap();
        service.listen(old, "in-1").unwrap();
        service.send(old, "out-1", Message::On { channel: 1, note: 60, velocity: 90 }).unwrap();
        let (new, _) = owner(&service);
        assert!(!service.resume(new, "not-the-token"));
        assert!(service.resume(new, &token));
        assert!(service.hears(new, "in-1"));
        assert!(service.still(old).is_err(), "the old socket closes");
        service.keep(Instant::now() + LINGER);
        assert_eq!(fake.sent().len(), 1, "nothing let go on the joint");
        assert!(fake.is_open("in-1"));
        service.release(new, "закрыли");
        assert!(fake.sent().iter().any(|(_, bytes)| bytes == &vec![0x80, 60, 0]));
    }

    #[test]
    fn the_rate_has_a_limit() {
        let (service, _) = service();
        let (one, _) = owner(&service);
        for _ in 0..SEND_RATE {
            service.send(one, "out-1", Message::Control { channel: 1, control: 1, value: 1 }).unwrap();
        }
        assert!(service.send(one, "out-1", Message::Control { channel: 1, control: 1, value: 1 }).is_err());
    }

    #[test]
    fn a_keyboard_unplugged_or_busy_says_so() {
        let (service, fake) = service();
        let (one, _) = owner(&service);
        fake.hold("in-1");
        assert!(service.listen(one, "in-1").unwrap_err().contains("занята"));
        assert!(service.listen(one, "in-9").unwrap_err().contains("отключили"));
        fake.unplug("1");
        assert!(service.refresh().is_empty());
        assert!(service.send(one, "out-1", Message::On { channel: 1, note: 60, velocity: 1 }).is_err());
    }

    #[test]
    fn an_app_switched_off_lets_go() {
        let (service, _) = service();
        let (one, _) = owner(&service);
        service.listen(one, "in-1").unwrap();
        assert_eq!(service.view().listeners.len(), 1);
        service.release_package("teacher", "выключили");
        assert!(service.view().listeners.is_empty());
        assert!(service.still(one).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn a_melody_plays_by_the_gateways_clock() {
        let (service, fake) = service();
        let (one, mut frames) = owner(&service);
        let steps = wire::melody(&json!([[60, 90, 0, 400], [62, 90, 500, 400]])).unwrap();
        service.play(one, "demo", "out-1", steps).unwrap();
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let sent: Vec<Vec<u8>> = fake.sent().into_iter().map(|(_, bytes)| bytes).collect();
        assert_eq!(sent, [vec![0x90, 60, 90], vec![0x80, 60, 0], vec![0x90, 62, 90], vec![0x80, 62, 0]]);
        let mut kinds = Vec::new();
        while let Ok(frame) = frames.try_recv() {
            kinds.push(frame["t"].as_str().unwrap().to_string());
        }
        assert_eq!(kinds, ["playing", "playing", "played"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_melody_stopped_lets_its_note_go() {
        let (service, fake) = service();
        let (one, _) = owner(&service);
        let steps = wire::melody(&json!([[60, 90, 0, 5000]])).unwrap();
        service.play(one, "demo", "out-1", steps).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        service.stop_play(one);
        let sent: Vec<Vec<u8>> = fake.sent().into_iter().map(|(_, bytes)| bytes).collect();
        assert_eq!(sent, [vec![0x90, 60, 90], vec![0x80, 60, 0]]);
    }
}
