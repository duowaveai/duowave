//! Devices on the bus (`device/<name>`, docs/plans/bus.md, «Устройства»): what is plugged into the computer the
//! gateway runs on. The gateway owns a device — a browser may not, and the app's window has no way to — and an app
//! reaches it through the bus, by its links, with a person's «да» the first time.
//!
//! A new device is one `impl Device` added to the registry: a name, a title, its handles (a step of the ladder each,
//! a danger when it has one) and what it does on a letter. The MIDI keyboard is the first one.
//!
//! The same interface is what a computer gives its home as a Турбо (docs/plans/turbo-devices.md): a device's nature
//! (a thing at hand, or data), whether it is sensitive (off for the home until ticked on the computer), and the things
//! of its kind plugged in now. The home's devices (src/home/turbo_devices.rs) gather this gateway's and its Турбо's.
use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::bus::address::{self, Address};
use crate::bus::provider::{BusError, Handle, Letter, Participant, Provider};
use crate::bus::Level;

/// What a device is to the home (owner 20.11, 6): a thing at hand — a keyboard, a screen, a flash drive — is the
/// home's only while its computer is at home; data — a folder — reaches from afar too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Nature {
    Physical,
    Data,
}

/// One thing of a kind plugged in now: «Keystation 49». Its id is the computer's own (`in-2`); the home adds the
/// place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub title: String,
}

/// One device the gateway owns.
pub trait Device: Send + Sync {
    /// Its name: `midi` is `device/midi`.
    fn name(&self) -> &'static str;
    /// For people: «MIDI-клавиатура».
    fn title(&self) -> String;
    /// What it can do; news it tells are handles of the step `listen`.
    fn handles(&self) -> Vec<Handle>;
    /// Whether it can be reached here now (the build has it, it is not switched off).
    fn available(&self) -> bool {
        true
    }
    /// Carries out a letter the bus let through.
    fn call(&self, handle: &str, input: Value) -> BoxFuture<'static, Result<Value, BusError>>;
    /// A thing at hand or data: what the home may reach while its computer is away.
    fn nature(&self) -> Nature {
        Nature::Physical
    }
    /// A camera, a microphone, a keyboard-like thing that hears what is typed: a computer gives it to its home only
    /// when ticked on the computer (owner 20.11, 3).
    fn sensitive(&self) -> bool {
        false
    }
    /// The things of its kind plugged in now, as people name them.
    fn instances(&self) -> Vec<Instance> {
        Vec::new()
    }
}

/// The devices of this gateway: a provider of `device/…`.
#[derive(Default)]
pub struct Devices {
    devices: RwLock<Vec<Arc<dyn Device>>>,
}

impl Devices {
    pub fn new() -> Devices {
        Devices::default()
    }

    /// A device from now on; one of the same name before it goes.
    pub fn add(&self, device: Arc<dyn Device>) {
        let mut devices = self.devices.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        devices.retain(|known| known.name() != device.name());
        devices.push(device);
    }

    fn device(&self, address: &Address) -> Option<Arc<dyn Device>> {
        if address.kind() != address::DEVICE {
            return None;
        }
        self.get(address.name()?)
    }

    /// The device of this name, while it can be reached.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Device>> {
        self.devices
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|device| device.name() == name && device.available())
            .cloned()
    }

    /// Every device that can be reached now, in the order they were added.
    pub fn list(&self) -> Vec<Arc<dyn Device>> {
        self.devices
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|device| device.available())
            .cloned()
            .collect()
    }
}

impl Provider for Devices {
    fn kind(&self) -> &'static str {
        address::DEVICE
    }

    fn participants(&self) -> Vec<Participant> {
        self.devices
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|device| device.available())
            .filter_map(|device| {
                Some(Participant::new(Address::new(address::DEVICE, device.name()).ok()?, &device.title()))
            })
            .collect()
    }

    fn participant(&self, address: &Address) -> Option<Participant> {
        let device = self.device(address)?;
        Some(Participant::new(address.clone(), &device.title()))
    }

    fn handles(&self, address: &Address) -> Vec<Handle> {
        self.device(address).map(|device| device.handles()).unwrap_or_default()
    }

    fn owns(&self, _from: &Address, _to: &Address) -> bool {
        false
    }

    fn call(&self, letter: Letter) -> BoxFuture<'static, Result<Value, BusError>> {
        match self.device(&letter.to) {
            Some(device) => device.call(&letter.handle, letter.input),
            None => Box::pin(async move { Err(BusError::NotFound(letter.to.to_string())) }),
        }
    }
}

/// The MIDI keyboard plugged into the computer (src/midi, docs/plans/midi.md). Its notes go to the app's page by
/// the keyboard's own channel (`/api/v1/midi/ws`) — never through a backend nor a journal; the bus decides which app's
/// page may open it (`notes`, the step `listen`).
pub struct MidiKeyboard {
    midi: Arc<crate::midi::MidiService>,
}

impl MidiKeyboard {
    pub fn new(midi: Arc<crate::midi::MidiService>) -> MidiKeyboard {
        MidiKeyboard { midi }
    }
}

impl Device for MidiKeyboard {
    fn name(&self) -> &'static str {
        "midi"
    }

    fn title(&self) -> String {
        crate::tr!("MIDI-клавиатура")
    }

    fn handles(&self) -> Vec<Handle> {
        vec![
            Handle::new("keyboards", Level::See).titled(&crate::tr!("какие клавиатуры подключены")),
            Handle::new("notes", Level::Listen).titled(&crate::tr!("слышать ноты на своей странице")),
            Handle::new("play", Level::Call).titled(&crate::tr!("сыграть мелодию на клавиатуре")),
            Handle::new("stop", Level::Call).titled(&crate::tr!("остановить мелодию на клавиатуре")),
        ]
    }

    fn available(&self) -> bool {
        self.midi.available()
    }

    fn instances(&self) -> Vec<Instance> {
        self.midi
            .devices()
            .into_iter()
            .filter_map(|device| {
                let id = device.input.clone().or(device.output.clone())?;
                Some(Instance { id, title: device.name })
            })
            .collect()
    }

    fn call(&self, handle: &str, input: Value) -> BoxFuture<'static, Result<Value, BusError>> {
        let midi = self.midi.clone();
        let handle = handle.to_string();
        Box::pin(async move {
            match handle.as_str() {
                "keyboards" => Ok(json!(midi.devices())),
                // The notes themselves go by the keyboard's channel: the letter only says the page may open it.
                "notes" => Ok(json!({ "channel": "/api/v1/midi/ws" })),
                // A melody with no page (the brain's «сыграй на клавиатуре»): on the keyboard named, or the first one
                // that sounds.
                "play" => {
                    let steps = crate::midi::wire::melody(input.get("notes").unwrap_or(&Value::Null))
                        .map_err(BusError::Failed)?;
                    let asked = input.get("device").and_then(Value::as_str).unwrap_or_default();
                    let port = midi.output_for(asked).ok_or_else(|| {
                        BusError::Failed(crate::tr!("Клавиатуры, которая звучит, сейчас нет: её отключили?"))
                    })?;
                    midi.play_alone(&port, steps).map_err(BusError::Failed)?;
                    Ok(json!({ "playing": port }))
                }
                "stop" => {
                    midi.stop_alone();
                    Ok(json!({ "stopped": true }))
                }
                other => Err(BusError::NotFound(format!("device/midi: {other}"))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lamp;

    impl Device for Lamp {
        fn name(&self) -> &'static str {
            "lamp"
        }
        fn title(&self) -> String {
            "Лампа на столе".into()
        }
        fn handles(&self) -> Vec<Handle> {
            vec![Handle::new("on", Level::Call), Handle::new("pressed", Level::Listen)]
        }
        fn call(&self, handle: &str, _input: Value) -> BoxFuture<'static, Result<Value, BusError>> {
            let handle = handle.to_string();
            Box::pin(async move { Ok(json!({ "did": handle })) })
        }
    }

    #[tokio::test]
    async fn a_new_device_is_one_impl_and_one_add() {
        let devices = Devices::new();
        devices.add(Arc::new(Lamp));
        let lamp = Address::parse("device/lamp").unwrap();
        assert_eq!(devices.participants()[0].title, "Лампа на столе");
        assert_eq!(devices.handles(&lamp).len(), 2);
        let letter = Letter {
            id: 1,
            from: Address::app("x"),
            to: lamp.clone(),
            handle: "on".into(),
            input: json!({}),
            files: vec![],
            trail: crate::bus::Trail::app("x"),
        };
        assert_eq!(devices.call(letter).await.unwrap(), json!({"did": "on"}));
        assert!(devices.participant(&Address::parse("device/none").unwrap()).is_none());
    }
}
