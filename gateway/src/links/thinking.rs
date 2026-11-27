//! Thinking behind the door (docs/plans/thinking.md): the gateway makes `OurThinking` once, from the talk's parts, the
//! characters and the setting `thinking`, and every door the audio side talks to stands on the switch
//! (`thinking::Switch`): per phrase ours, or the talk layer's stand-in, as the setting `thinking.implementation` says;
//! a second agent's own Thinking takes the phrases said to it (stage M10). The home's devices it asks about are the
//! stations on the line and this computer (`HomeDevices`).

use std::sync::Arc;

use crate::access::AccessService;
use crate::app::App;
use crate::core::identity::principal::user_principal;
use crate::door::{Device, Kind};
use crate::talk::thinking::People;
use crate::thinking::{Deps, OurThinking, Switch};
use duowave_brain::door::Thinking;

/// A station's name nobody chose (stations/service.rs, `first_name`): not a room.
#[cfg(feature = "yandex-station")]
const NO_ROOM: &str = "Колонка";

/// The home's devices as Thinking asks them (`door::Devices`): the stations on the line now, each with the room its
/// name says («Кухня»), its hands and whether someone talks with it or it rings; and this computer, where the
/// gateway runs on one. A station relayed to Yandex (the mode «Алиса») is Yandex's: not among them.
pub struct HomeDevices {
    stations: Arc<crate::stations::StationsService>,
    computer: bool,
    #[cfg(feature = "yandex-station")]
    sessions: std::sync::OnceLock<std::sync::Weak<crate::gateway::uniproxy::SessionRegistry>>,
}

impl HomeDevices {
    pub fn new(stations: Arc<crate::stations::StationsService>, computer: bool) -> Arc<HomeDevices> {
        Arc::new(HomeDevices {
            stations,
            computer,
            #[cfg(feature = "yandex-station")]
            sessions: std::sync::OnceLock::new(),
        })
    }

    /// The stations' sessions, once they are built (after Thinking: they take its door).
    #[cfg(feature = "yandex-station")]
    pub fn set_sessions(&self, sessions: &Arc<crate::gateway::uniproxy::SessionRegistry>) {
        let _ = self.sessions.set(Arc::downgrade(sessions));
    }

    #[cfg(feature = "yandex-station")]
    fn stations(&self, devices: &mut Vec<Device>) {
        let Some(sessions) = self.sessions.get().and_then(std::sync::Weak::upgrade) else { return };
        let turns = sessions.turns();
        for snapshot in sessions.list() {
            let relayed = snapshot.mode.is_some();
            let device = snapshot.device_tag;
            if snapshot.closed_at.is_some() || relayed || devices.iter().any(|known| known.device == device) {
                continue;
            }
            let room = self.stations.find(&device).map(|station| station.name).filter(|name| name != NO_ROOM);
            devices.push(Device {
                busy: turns.busy(&device),
                device,
                room,
                kind: Kind::Station,
                screen: false,
                speaker: true,
                hands: crate::ear::hands::station(),
            });
        }
    }
}

impl crate::door::Devices for HomeDevices {
    fn devices(&self) -> Vec<Device> {
        let mut devices = Vec::new();
        #[cfg(feature = "yandex-station")]
        self.stations(&mut devices);
        #[cfg(not(feature = "yandex-station"))]
        let _ = &self.stations;
        if self.computer {
            devices.push(Device {
                device: crate::core::origin::COMPUTER.to_string(),
                room: None,
                kind: Kind::Computer,
                screen: true,
                speaker: crate::ear::port::port().is_some(),
                hands: crate::ear::hands::computer(),
                busy: false,
            });
        }
        devices
    }
}

/// Who speaks into a device: the person signed in on its panel, by their id; nobody — the household's voice.
pub fn people(access: &Arc<AccessService>) -> People {
    let (users, household) = (access.users.clone(), access.household.clone());
    Arc::new(move |id| {
        id.and_then(|id| users.get(id).ok())
            .filter(|person| person.status == crate::access::users_repository::UserStatus::Active)
            .map(|person| user_principal(&person.id, &person.name, person.role, Some(&person.restrictions)))
            .unwrap_or_else(|| household.speaker(None))
    })
}

/// Our Thinking, made once for the gateway.
#[allow(clippy::too_many_arguments)]
pub fn ours(
    parts: crate::talk::Parts,
    personas: Arc<crate::dialog::PersonaService>,
    access: &Arc<AccessService>,
    settings: Arc<crate::settings::SettingsService>,
    lines: Arc<dyn crate::core::requests::RequestLines>,
    titles: Option<crate::thinking::catalog::AppTitles>,
    characters: Option<crate::thinking::agent::Characters>,
    devices: Option<Arc<dyn crate::door::Devices>>,
) -> Arc<OurThinking> {
    Arc::new(OurThinking::new(Deps {
        parts,
        personas,
        people: people(access),
        settings,
        devices,
        lines: Some(lines),
        titles,
        characters,
    }))
}

/// The door for an audio side: the switch in front of ours and the talk layer's stand-in.
pub fn door(app: &App, talk: Arc<dyn Thinking>) -> Arc<dyn Thinking> {
    Arc::new(Switch::new(talk, app.thinking.clone(), app.settings.clone()))
}
