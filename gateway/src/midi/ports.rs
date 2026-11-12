//! The keyboards' ports (docs/plans/midi.md, «Служба MIDI»): what the system has, opening an input or an output.
//! `Midir` — the real ones through midir (CoreMIDI on a Mac, WinMM on Windows, ALSA on Linux), in a build with the
//! feature `midi`; `Absent` — a build without it; `FakePorts` — the tests', a keyboard pressed by code.
//!
//! A Mac tells of a keyboard plugged in later only on the run loop of the thread its first CoreMIDI client was made
//! on: midir has no such news, and without a run loop its list stays as it was at the start. So `watch`, called
//! before any other client, makes the first one on a thread of its own that runs a `CFRunLoop` and passes the news
//! on. Windows and Linux are asked again every few seconds instead (`MidiService`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// One port: a keyboard's input (what it plays) or output (what it is told to play).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    pub id: String,
    pub name: String,
    pub input: bool,
}

/// What an open input hands on: the time in microseconds (the system's, any start), and the message's bytes. Called
/// on the system's own thread: it must never wait.
pub type Sink = Box<dyn FnMut(u64, &[u8]) + Send + 'static>;

/// An open input: closed when dropped.
pub trait InputLink: Send {}

/// An open output.
pub trait OutputLink: Send {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String>;
}

pub trait Ports: Send + Sync {
    /// Why this gateway has no MIDI at all, if it has none.
    fn absent(&self) -> Option<String> {
        None
    }
    fn list(&self) -> Result<Vec<PortInfo>, String>;
    fn open_input(&self, id: &str, sink: Sink) -> Result<Box<dyn InputLink>, String>;
    fn open_output(&self, id: &str) -> Result<Box<dyn OutputLink>, String>;
    /// Hear of keyboards plugged in or out from the system itself; false — it cannot, ask again by time. Called once,
    /// before anything else of MIDI.
    fn watch(&self, _changed: Arc<dyn Fn() + Send + Sync>) -> bool {
        false
    }
}

/// A gateway without MIDI: the station's build, the Linux program, or `MIDI=0`.
pub struct Absent(pub String);

impl Ports for Absent {
    fn absent(&self) -> Option<String> {
        Some(self.0.clone())
    }
    fn list(&self) -> Result<Vec<PortInfo>, String> {
        Err(self.0.clone())
    }
    fn open_input(&self, _id: &str, _sink: Sink) -> Result<Box<dyn InputLink>, String> {
        Err(self.0.clone())
    }
    fn open_output(&self, _id: &str) -> Result<Box<dyn OutputLink>, String> {
        Err(self.0.clone())
    }
}

/// Why a build has no MIDI: said to people.
pub const NOT_IN_BUILD: &str = "В этой сборке шлюза MIDI нет: клавиатуру слышит приложение DuoWave на компьютере \
(Mac, Windows, Linux) и программа duowave для Mac и Windows.";

/// The ports this build has: midir's with the feature `midi`, none without it.
pub fn system() -> Arc<dyn Ports> {
    #[cfg(feature = "midi")]
    {
        Arc::new(Midir)
    }
    #[cfg(not(feature = "midi"))]
    {
        Arc::new(Absent(NOT_IN_BUILD.into()))
    }
}

#[cfg(feature = "midi")]
pub use real::Midir;

#[cfg(feature = "midi")]
mod real {
    use std::sync::Arc;

    use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};

    use super::{InputLink, OutputLink, PortInfo, Ports, Sink};

    const CLIENT: &str = "DuoWave";

    /// The system's ports through midir.
    pub struct Midir;

    struct Input(#[allow(dead_code)] MidiInputConnection<()>);
    impl InputLink for Input {}

    struct Output(MidiOutputConnection);
    impl OutputLink for Output {
        fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.0.send(bytes).map_err(|error| format!("клавиатура не приняла: {error}"))
        }
    }

    fn unopened(error: impl std::fmt::Display) -> String {
        format!("MIDI не открылся: {error}")
    }

    impl Ports for Midir {
        fn list(&self) -> Result<Vec<PortInfo>, String> {
            let mut ports = Vec::new();
            let input = MidiInput::new(CLIENT).map_err(unopened)?;
            for port in input.ports() {
                if let Ok(name) = input.port_name(&port) {
                    ports.push(PortInfo { id: port.id(), name, input: true });
                }
            }
            let output = MidiOutput::new(CLIENT).map_err(unopened)?;
            for port in output.ports() {
                if let Ok(name) = output.port_name(&port) {
                    ports.push(PortInfo { id: port.id(), name, input: false });
                }
            }
            Ok(ports)
        }

        fn open_input(&self, id: &str, mut sink: Sink) -> Result<Box<dyn InputLink>, String> {
            let mut input = MidiInput::new(CLIENT).map_err(unopened)?;
            // SysEx, the clock and active sensing never reach us (docs/plans/midi.md, «Фильтр»).
            input.ignore(Ignore::All);
            let port = input.find_port_by_id(id).ok_or_else(|| "Клавиатуры нет: её отключили?".to_string())?;
            let connection = input
                .connect(&port, "duowave-in", move |stamp, bytes, _| sink(stamp, bytes), ())
                .map_err(|error| busy_or(error.kind().to_string()))?;
            Ok(Box::new(Input(connection)))
        }

        fn open_output(&self, id: &str) -> Result<Box<dyn OutputLink>, String> {
            let output = MidiOutput::new(CLIENT).map_err(unopened)?;
            let port = output.find_port_by_id(id).ok_or_else(|| "Клавиатуры нет: её отключили?".to_string())?;
            let connection = output.connect(&port, "duowave-out").map_err(|error| busy_or(error.kind().to_string()))?;
            Ok(Box::new(Output(connection)))
        }

        fn watch(&self, changed: Arc<dyn Fn() + Send + Sync>) -> bool {
            watch(changed)
        }
    }

    /// On Windows a port is one program's: another one holding it is the usual reason it does not open.
    fn busy_or(error: String) -> String {
        if cfg!(windows) {
            format!("Клавиатура занята другой программой или отключена ({error}).")
        } else {
            format!("Клавиатуру не открыть: {error}")
        }
    }

    /// A Mac: the first CoreMIDI client, on a thread of its own with a run loop — the system's news of keyboards
    /// plugged in or out comes there, and midir's lists are fresh after it.
    #[cfg(target_os = "macos")]
    fn watch(changed: Arc<dyn Fn() + Send + Sync>) -> bool {
        use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop, CFRunLoopRunResult};
        use coremidi::{Client, Notification};

        let (made, told) = std::sync::mpsc::channel();
        let started = std::thread::Builder::new().name("midi-devices".into()).spawn(move || {
            let client = Client::new_with_notifications("DuoWave devices", move |notification: &Notification| {
                if matches!(
                    notification,
                    Notification::SetupChanged | Notification::ObjectAdded(_) | Notification::ObjectRemoved(_)
                ) {
                    changed();
                }
            });
            let alive = client.is_ok();
            let _ = made.send(alive);
            if !alive {
                return;
            }
            let _client = client;
            loop {
                // A run loop with nothing on it ends at once: then it waits a little, never spins.
                let result =
                    CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, std::time::Duration::from_secs(60), false);
                if matches!(result, CFRunLoopRunResult::Finished) {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
        });
        started.is_ok() && told.recv_timeout(std::time::Duration::from_secs(5)).unwrap_or(false)
    }

    #[cfg(not(target_os = "macos"))]
    fn watch(_changed: Arc<dyn Fn() + Send + Sync>) -> bool {
        false
    }
}

/// The tests' keyboards: plugged and pressed by code; what the gateway sent them is kept.
#[derive(Default)]
pub struct FakePorts {
    state: Mutex<FakeState>,
}

#[derive(Default)]
struct FakeState {
    ports: Vec<PortInfo>,
    sinks: HashMap<String, Arc<Mutex<Option<Sink>>>>,
    opened: Vec<String>,
    sent: Vec<(String, Vec<u8>)>,
    busy: Vec<String>,
}

struct FakeInput {
    sink: Arc<Mutex<Option<Sink>>>,
}
impl InputLink for FakeInput {}
impl Drop for FakeInput {
    fn drop(&mut self) {
        *self.sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

struct FakeOutput {
    id: String,
    ports: Arc<FakePorts>,
}
impl OutputLink for FakeOutput {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.ports.lock().sent.push((self.id.clone(), bytes.to_vec()));
        Ok(())
    }
}

impl FakePorts {
    pub fn new() -> Arc<FakePorts> {
        Arc::new(FakePorts::default())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A keyboard plugged in: its input and output with these ids (`in-…`, `out-…`).
    pub fn plug(&self, id: &str, name: &str) {
        let mut state = self.lock();
        state.ports.push(PortInfo { id: format!("in-{id}"), name: name.into(), input: true });
        state.ports.push(PortInfo { id: format!("out-{id}"), name: name.into(), input: false });
    }

    pub fn unplug(&self, id: &str) {
        let mut state = self.lock();
        state.ports.retain(|port| port.id != format!("in-{id}") && port.id != format!("out-{id}"));
        if let Some(sink) = state.sinks.remove(&format!("in-{id}")) {
            *sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        }
    }

    /// Opening this port fails, as a port held by another program does.
    pub fn hold(&self, port: &str) {
        self.lock().busy.push(port.into());
    }

    /// A key on the keyboard: what its open input hands on. False when nobody has it open.
    pub fn press(&self, port: &str, stamp_us: u64, bytes: &[u8]) -> bool {
        let sink = self.lock().sinks.get(port).cloned();
        let Some(sink) = sink else { return false };
        let mut sink = sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match sink.as_mut() {
            Some(sink) => {
                sink(stamp_us, bytes);
                true
            }
            None => false,
        }
    }

    /// Whether an input is open now.
    pub fn is_open(&self, port: &str) -> bool {
        let sink = self.lock().sinks.get(port).cloned();
        sink.is_some_and(|sink| sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).is_some())
    }

    /// How many times ports were opened, in order.
    pub fn opened(&self) -> Vec<String> {
        self.lock().opened.clone()
    }

    /// What the gateway sent the keyboards.
    pub fn sent(&self) -> Vec<(String, Vec<u8>)> {
        self.lock().sent.clone()
    }
}

/// The tests hold the fake ports by `Arc`; the service takes them as its `Ports`.
pub struct Fake(pub Arc<FakePorts>);

impl Ports for Fake {
    fn list(&self) -> Result<Vec<PortInfo>, String> {
        Ok(self.0.lock().ports.clone())
    }

    fn open_input(&self, id: &str, sink: Sink) -> Result<Box<dyn InputLink>, String> {
        let mut state = self.0.lock();
        if state.busy.iter().any(|busy| busy == id) {
            return Err("Клавиатура занята другой программой".into());
        }
        if !state.ports.iter().any(|port| port.id == id && port.input) {
            return Err("Клавиатуры нет: её отключили?".into());
        }
        state.opened.push(id.to_string());
        let slot = Arc::new(Mutex::new(Some(sink)));
        state.sinks.insert(id.to_string(), slot.clone());
        Ok(Box::new(FakeInput { sink: slot }))
    }

    fn open_output(&self, id: &str) -> Result<Box<dyn OutputLink>, String> {
        let mut state = self.0.lock();
        if !state.ports.iter().any(|port| port.id == id && !port.input) {
            return Err("Клавиатуры нет: её отключили?".into());
        }
        state.opened.push(id.to_string());
        Ok(Box::new(FakeOutput { id: id.to_string(), ports: self.0.clone() }))
    }
}

#[cfg(all(test, feature = "midi"))]
mod tests {
    use super::*;

    /// This computer's own keyboards, through CoreMIDI or the system's: by hand only (`cargo test --features midi --
    /// --ignored the_systems_keyboards --nocapture`) — it touches the real MIDI of the machine, nothing else.
    #[test]
    #[ignore]
    fn the_systems_keyboards() {
        let ports = Midir;
        let watching = ports.watch(Arc::new(|| println!("the system told of a change")));
        println!("watching: {watching}");
        let list = ports.list().expect("the system's MIDI reads");
        for port in &list {
            println!("{} {} — {}", if port.input { "in " } else { "out" }, port.id, port.name);
        }
        if cfg!(target_os = "macos") {
            assert!(watching, "a Mac tells of keyboards plugged in later");
        }
    }
}
