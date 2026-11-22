//! The home's devices for the brain (docs/plans/turbo-devices.md, «Мозг»), given the way the screen gives its own
//! (src/screen/tools.rs). The owner's decision 7 (20.11): seeing and using — at once, by the rights of whoever speaks;
//! changing — only with a «да» on the screen; deleting by voice — never.
//!
//! - `home_devices` — what is plugged into the home's computers, by kind, with each computer's state: ready, asleep,
//!   away, nothing plugged in.
//! - `home_device_use` — a device's ability: «сыграй мелодию на клавиатуре», «покажи на экране Mac». A letter on the
//!   bus from the person who speaks; what changes things is refused here — the panel asks its «да».
//! - `home_device_place` — «на ПК»: where a kind's acts go from this room on.
//! - `wake_computer` — a sleeping Турбо woken over the home network, on the person's «да» to the model's own question.
use std::sync::{Arc, Weak};

use futures::future::BoxFuture;
use serde_json::{json, Map, Value};

use super::{HouseDevices, PlaceState, VOICE_WAKE_WAIT};
use crate::bus::address::{self, Address};
use crate::bus::{Level, Trail};
use crate::core::connectors::{
    Connector, ConnectorKind, ConnectorManifest, ConnectorStatus, ToolContext, ToolContribution, ToolRun,
};
use crate::core::identity::permissions::use_of;
use crate::core::identity::principal::{Principal, PrincipalKind};
use crate::dialog::llm::LlmTool;
use crate::skills::capabilities::CapabilityName;

pub struct DevicesConnector {
    house: Weak<HouseDevices>,
    bus: Weak<crate::bus::Bus>,
}

impl DevicesConnector {
    pub fn new(house: &Arc<HouseDevices>, bus: &Arc<crate::bus::Bus>) -> DevicesConnector {
        DevicesConnector { house: Arc::downgrade(house), bus: Arc::downgrade(bus) }
    }

    /// Without a bus (tests): the letters go straight to the home's devices.
    #[cfg(test)]
    pub fn without_bus(house: &Arc<HouseDevices>) -> DevicesConnector {
        DevicesConnector { house: Arc::downgrade(house), bus: Weak::new() }
    }
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

const KIND: &str = "The kind as home_devices names it: midi (a MIDI keyboard), screen (a computer's screen), storage \
                    (flash drives), …";

fn tools() -> Vec<(LlmTool, &'static str, bool)> {
    vec![
        (
            LlmTool {
                name: "home_devices".into(),
                description: "What is plugged into the home's computers (its Турбо and this one): MIDI keyboards, \
                              screens, flash drives — by kind, with each computer's state: ready, asleep (switched \
                              off or sleeping — say so, and offer to wake it with wake_computer when canWake), away \
                              (online from outside the home: its things are not the home's now), unplugged (nothing \
                              of that kind plugged in there now). Use it for «что у меня подключено?», «где \
                              клавиатура?»."
                    .into(),
                parameters: object(json!({}), &[]),
            },
            "list",
            true,
        ),
        (
            LlmTool {
                name: "home_device_use".into(),
                description: "Use one of the home's devices: call an ability home_devices lists for its kind — midi: \
                              play {notes: [[note 0–127, velocity 1–127, start ms, length ms], …]}, stop, keyboards; \
                              screen: windows, close, pause, resume. Where the home has the kind at several \
                              computers the home chooses one and the answer's `said` tells which: say it. Things that \
                              change files are not done by voice: tell the person to do them in the panel."
                    .into(),
                parameters: object(
                    json!({
                        "kind": { "type": "string", "description": KIND },
                        "action": { "type": "string", "description": "The ability's name, as home_devices lists it." },
                        "input": { "type": "object", "description": "The ability's input." },
                        "computer": { "type": "string", "description": "Only when the person named the computer: its name or id." }
                    }),
                    &["kind", "action"],
                ),
            },
            "use",
            false,
        ),
        (
            LlmTool {
                name: "home_device_place".into(),
                description: "Where a kind's acts go from this room on, when the person names a computer for it («на \
                              ПК», «играй на маке»)."
                    .into(),
                parameters: object(
                    json!({
                        "kind": { "type": "string", "description": KIND },
                        "computer": { "type": "string", "description": "The computer's name or id." }
                    }),
                    &["kind", "computer"],
                ),
            },
            "place",
            false,
        ),
        (
            LlmTool {
                name: "wake_computer".into(),
                description: "Wake a sleeping computer of the home over the home network — only after the person \
                              agreed to your question «Разбудить …?». Waits a few seconds and says whether it woke."
                    .into(),
                parameters: object(
                    json!({ "computer": { "type": "string", "description": "Its name or id; the sleeping one when there is one." } }),
                    &[],
                ),
            },
            "wake",
            false,
        ),
    ]
}

/// Who speaks, as a sender on the bus: a person, the voice of the house, a key.
fn speaker(principal: &Principal) -> Address {
    let (kind, path) = match principal.kind {
        PrincipalKind::User => (address::PERSON, principal.id.as_str()),
        PrincipalKind::ApiKey => (address::AGENT, principal.id.as_str()),
        PrincipalKind::System => (address::GATEWAY, "self"),
        PrincipalKind::Household => (address::HOME, "voice"),
    };
    Address::new(kind, path).or_else(|_| Address::new(address::HOME, "voice")).expect("a valid address")
}

/// A place of a kind by the name the person said, or its id.
fn place_named(house: &HouseDevices, kind: &str, named: &str) -> Option<String> {
    let named = named.trim().to_lowercase();
    let places = house.places(kind);
    places
        .iter()
        .find(|place| place.place.to_lowercase() == named)
        .or_else(|| places.iter().find(|place| !named.is_empty() && place.name.to_lowercase().contains(&named)))
        .map(|place| place.place.clone())
}

/// What the model reads of the home's devices.
fn listed(house: &HouseDevices) -> Value {
    let kinds: Vec<Value> = house
        .view()
        .into_iter()
        .map(|view| {
            let places: Vec<Value> = view
                .places
                .iter()
                .map(|place| {
                    let mut seen = json!({
                        "computer": place.name,
                        "state": place.state,
                        "things": place.instances.iter().map(|thing| thing.title.clone()).collect::<Vec<_>>(),
                    });
                    if place.state == PlaceState::Asleep && !house.macs_of(&place.place).is_empty() {
                        seen["canWake"] = json!(true);
                    }
                    seen
                })
                .collect();
            let handles: Vec<Value> = house
                .handles_of(&view.kind)
                .into_iter()
                .filter(|handle| handle.level != Level::Listen)
                .map(|handle| json!({ "action": handle.name, "changes": handle.level == Level::Edit }))
                .collect();
            json!({ "kind": view.kind, "title": view.title, "computers": places, "actions": handles })
        })
        .collect();
    json!({ "kinds": kinds })
}

fn run(house: &Weak<HouseDevices>, bus: &Weak<crate::bus::Bus>, what: &'static str) -> ToolRun {
    let (house, bus) = (house.clone(), bus.clone());
    Arc::new(move |input: Map<String, Value>, context: ToolContext| -> BoxFuture<'static, anyhow::Result<String>> {
        let (house, bus) = (house.clone(), bus.clone());
        Box::pin(async move {
            let house = house.upgrade().ok_or_else(|| anyhow::anyhow!("the gateway is stopping"))?;
            let text = |key: &str| input.get(key).and_then(Value::as_str).unwrap_or_default().trim().to_string();
            let ear = context.origin.as_ref().map(|origin| origin.device.clone()).unwrap_or_default();
            match what {
                "list" => Ok(listed(&house).to_string()),
                "place" => {
                    let kind = text("kind");
                    let choice =
                        house.set_choice(&kind, &ear, &text("computer")).map_err(|error| anyhow::anyhow!("{error}"))?;
                    Ok(json!({ "kind": kind, "computer": choice.name }).to_string())
                }
                "wake" => {
                    let named = text("computer");
                    let helper = if named.is_empty() {
                        house.sleeping_turbo().map(|(id, _)| id)
                    } else {
                        house.view().iter().find_map(|view| place_named(&house, &view.kind, &named))
                    }
                    .ok_or_else(|| {
                        anyhow::anyhow!(crate::tr!("Спящего компьютера, которого можно разбудить, в доме нет."))
                    })?;
                    let name = house.name_of(&helper).unwrap_or_else(|| helper.clone());
                    let woke = house.wake(&helper, VOICE_WAKE_WAIT).await.map_err(|why| anyhow::anyhow!(why))?;
                    Ok(json!({ "computer": name, "awake": woke }).to_string())
                }
                _ => {
                    let kind = text("kind");
                    let action = text("action");
                    let level = house
                        .handles_of(&kind)
                        .into_iter()
                        .find(|handle| handle.name == action)
                        .map(|handle| handle.level);
                    match level {
                        None => return Err(anyhow::anyhow!(crate::tr!("У «{kind}» нет такого: {action}", kind = kind, action = action))),
                        // The owner's decision 7: what changes things waits for a «да» on the screen.
                        Some(Level::Edit) => {
                            return Ok(json!({ "refused": crate::tr!("Это меняет файлы, а голосом я так не делаю. Сделайте это в пульте, там спросят «да».") }).to_string())
                        }
                        Some(_) => {}
                    }
                    let mut letter = input.get("input").cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
                    letter["ear"] = Value::String(ear);
                    let named = text("computer");
                    if !named.is_empty() {
                        let place = place_named(&house, &kind, &named).ok_or_else(|| {
                            anyhow::anyhow!(crate::tr!("Такого компьютера для «{kind}» нет.", kind = kind))
                        })?;
                        letter["place"] = Value::String(place);
                    }
                    let to = Address::new(address::DEVICE, &kind).map_err(|why| anyhow::anyhow!(why))?;
                    let answer = match bus.upgrade() {
                        Some(bus) => {
                            let trail = Trail::principal(&context.principal);
                            bus.call(trail, speaker(&context.principal), to, &action, letter, Vec::new()).await
                        }
                        None => house.letter(&kind, &action, letter).await,
                    };
                    answer.map(|answer| answer.to_string()).map_err(|error| anyhow::anyhow!("{error}"))
                }
            }
        })
    })
}

impl Connector for DevicesConnector {
    fn manifest(&self) -> ConnectorManifest {
        ConnectorManifest {
            id: "home-devices".into(),
            kind: ConnectorKind::Lookup,
            title: crate::tr!("Устройства дома"),
            description: crate::tr!(
                "Что подключено к компьютерам дома: клавиатура, экран, флешки. И как разбудить компьютер"
            ),
            cloud: false,
            env: Vec::new(),
            // The brain's own tools, not a station's skill.
            capabilities: None,
            setup: None,
        }
    }

    fn listed(&self) -> bool {
        false
    }

    fn status(&self) -> BoxFuture<'_, ConnectorStatus> {
        Box::pin(async move {
            match self.house.upgrade() {
                Some(house) if house.anything() => ConnectorStatus::ready(crate::tr!("Устройства дома видны")),
                _ => ConnectorStatus::off(crate::tr!("К компьютерам дома ничего не подключено")),
            }
        })
    }

    fn tools(&self) -> Vec<ToolContribution> {
        let Some(house) = self.house.upgrade() else { return Vec::new() };
        if !house.anything() {
            return Vec::new();
        }
        tools()
            .into_iter()
            .map(|(tool, what, read_only)| ToolContribution {
                tool,
                app: None,
                permission: use_of(CapabilityName::Plugins),
                run: run(&self.house, &self.bus, what),
                read_only,
                outside: false,
                spoken: false,
                kept: None,
            })
            .collect()
    }
}
