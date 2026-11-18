//! The placement of this gateway and its state, as both panels read them: the computer's build is on a computer;
//! the station's is on the station, alone or with a home server (its connector says which). /health's `placement`
//! comes from here — on the station its settings revision, space, memory, home server and where its voice goes,
//! which the computer reads every 30 s.
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use super::availability::PlacementSource;
use super::station_fs::{own_memory, space_of, StationFs};
use crate::config::Config;
use crate::core::availability::Placement;
use crate::core::build::{Profile, PEER_API};
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::platform::http::ApiError;
use crate::secrets::SecretsService;
use crate::settings::definitions::{GatewayState, SafetyCase, SettingsHome, GATEWAY_STATE, HELPERS, STATION_SELF};
use crate::settings::SettingsService;
use crate::storage::Db;

/// Which gateway this is: the computer's, or the one on the station.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Pc,
    Station,
}

/// The home server as the station's gateway sees it (src/home_server): its address, up or down, since when — the old
/// shape of the pair's door; and the helpers (src/helpers).
pub trait HomeServerView: Send + Sync {
    fn view(&self) -> Option<Value>;
    /// `placement.helpers`: the regulator and the helpers in brief.
    fn helpers(&self) -> Option<Value> {
        None
    }
}

/// What starts this gateway again: the program anew in its own process (exec) — the station's service sees no exit,
/// so its safety net counts no failure; a test counts instead.
pub type Restarter = Arc<dyn Fn() + Send + Sync>;

pub fn restart_soon() -> Restarter {
    Arc::new(|| {
        tokio::spawn(async {
            // The answer leaves first.
            tokio::time::sleep(Duration::from_secs(1)).await;
            match std::env::current_exe() {
                Ok(program) => tracing::error!("restart: {}", crate::updates::install::restart(&program)),
                Err(error) => tracing::error!("restart: {error}"),
            }
        });
    })
}

pub struct PlacementService {
    profile: Profile,
    config: Arc<Config>,
    db: Arc<Db>,
    secrets: Arc<SecretsService>,
    settings: Arc<SettingsService>,
    events: EventBus,
    home: OnceLock<Arc<dyn HomeServerView>>,
    /// The computer's view of the station's gateway (src/placement/watch.rs).
    #[cfg(feature = "pc")]
    station: OnceLock<Arc<super::watch::StationGateways>>,
    /// What starts it again; the last one set wins (a test's own over the test support's).
    restarter: Mutex<Option<Restarter>>,
    /// The revision, computed at most every few seconds: the panels and the computer ask /health often.
    revision: Mutex<Option<(Instant, u64)>>,
    /// The pair's key made here, on the station, when its .env had none: the computer took it with the admin's
    /// password (`POST /api/v1/placement/pair`, docs/plans/desk.md, stage 4). Kept as a secret.
    made_key: OnceLock<String>,
}

/// The secret a station keeps the pair's key under when its .env has none.
pub const MADE_PEER_SECRET: &str = "placement:peer-made";

impl PlacementService {
    pub fn new(
        profile: Profile,
        config: Arc<Config>,
        db: Arc<Db>,
        secrets: Arc<SecretsService>,
        settings: Arc<SettingsService>,
        events: EventBus,
    ) -> PlacementService {
        PlacementService {
            profile,
            config,
            db,
            secrets,
            settings,
            events,
            home: OnceLock::new(),
            #[cfg(feature = "pc")]
            station: OnceLock::new(),
            restarter: Mutex::new(None),
            revision: Mutex::new(None),
            made_key: OnceLock::new(),
        }
        .with_made_key()
    }

    /// A station without PEER_KEY in its .env reads the key it made before.
    fn with_made_key(self) -> PlacementService {
        if self.profile.station && self.config.placement.peer_key.is_none() {
            if let Some(key) = self.secrets.get(MADE_PEER_SECRET).filter(|key| key.len() >= 16) {
                let _ = self.made_key.set(key);
            }
        }
        self
    }

    /// The pair's key for the computer that has the admin's password: the .env's, else one made here now and kept.
    pub fn pair_key(&self) -> Result<String, ApiError> {
        if let Some(key) = self.peer_key() {
            return Ok(key.to_string());
        }
        if !self.profile.station {
            return Err(ApiError::conflict(crate::tr!("Ключ пары отдаёт колонка, а это компьютер.")));
        }
        let key = super::peer::new_key();
        self.secrets.set(MADE_PEER_SECRET, Some(&key))?;
        Ok(self.made_key.get_or_init(|| key).clone())
    }

    pub fn set_home_server(&self, home: Arc<dyn HomeServerView>) {
        let _ = self.home.set(home);
    }

    #[cfg(feature = "pc")]
    pub fn set_station(&self, station: Arc<super::watch::StationGateways>) {
        let _ = self.station.set(station);
    }

    #[cfg(feature = "pc")]
    pub fn station(&self) -> Option<&Arc<super::watch::StationGateways>> {
        self.station.get()
    }

    pub fn set_restarter(&self, restarter: Restarter) {
        *self.restarter.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(restarter);
    }

    /// Starts this gateway again: new settings are applied at its start.
    pub fn restart(&self) {
        let restarter = self.restarter.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        match restarter {
            Some(restarter) => restarter(),
            None => restart_soon()(),
        }
    }

    pub fn role(&self) -> Role {
        if self.profile.station {
            Role::Station
        } else {
            Role::Pc
        }
    }

    pub fn peer_api(&self) -> u32 {
        PEER_API
    }

    /// The pair's key this gateway is reached with: the station's .env (PEER_KEY), else the one it made (pair_key).
    pub fn peer_key(&self) -> Option<&str> {
        self.config.placement.peer_key.as_deref().or_else(|| self.made_key.get().map(String::as_str))
    }

    /// The gateway's state record, defaults where nothing is stored.
    pub fn state(&self) -> GatewayState {
        self.settings.get(&GATEWAY_STATE, None).unwrap_or_default()
    }

    fn update_state(
        &self,
        what: &str,
        actor: Option<Actor>,
        edit: impl FnOnce(&mut GatewayState),
    ) -> Result<(), ApiError> {
        let mut state = self.state();
        edit(&mut state);
        self.settings.set(&GATEWAY_STATE, &state, actor.clone())?;
        self.events.emit(GatewayEvent::GatewayStateChanged { what: what.into(), actor });
        Ok(())
    }

    /// Where the main settings live (the station's own record; the computer keeps its own in its station record).
    pub fn set_settings_home(&self, home: SettingsHome, actor: Option<Actor>) -> Result<GatewayState, ApiError> {
        self.update_state("settingsHome", actor, |state| state.settings_home = home)?;
        Ok(self.state())
    }

    /// The safety net on or off: the record, and on the station the service's mark.
    pub fn set_safety(&self, on: bool, actor: Option<Actor>) -> Result<GatewayState, ApiError> {
        if self.role() == Role::Station {
            StationFs::real().set_safety(on).map_err(ApiError::internal)?;
        }
        self.update_state("safety", actor, |state| state.safety = on)?;
        Ok(self.state())
    }

    /// At the start on the station: what the safety net did while the gateway was down goes into the record and the
    /// history; and a new version that has run well lets the one before go.
    pub fn start(self: &Arc<Self>) {
        if self.role() != Role::Station {
            return;
        }
        let fs = StationFs::real();
        if let Some((what, at)) = fs.take_fired() {
            tracing::warn!("safety net: {what} at {at}");
            let case = SafetyCase { what: what.clone(), at: at.clone() };
            let _ = self.update_state("safety", None, |state| state.safety_last = Some(case));
            self.events.emit(GatewayEvent::SafetyFired { what, at });
        }
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(600)).await;
            // The program on the disk: a trial runs from memory, and its way back is not a `.previous`.
            let fs = StationFs::real();
            if fs.settle_update(&fs.program_on_disk()) {
                tracing::info!("update: the new version ran ten minutes; the one before is removed");
            }
        });
    }

    /// The revision is counted anew at the next ask: something that travels may have changed (the home's copy on a
    /// computer takes a new one within seconds, docs/plans/home-and-devices.md, stage 3).
    pub fn forget_revision(&self) {
        *self.revision.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    /// Forgets the counted revision after any change but the busy ones of requests and presence.
    pub fn watch_revision(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.events.on("placement.revision", move |event| {
            let name = event.name();
            let busy = name.starts_with("request.")
                || matches!(
                    name,
                    "station.presence"
                        | "station.channel"
                        | "station.state"
                        | "tasks.changed"
                        | "workshop.changed"
                        | "works.run"
                        | "home.devices.changed"
                        | "router.deal"
                );
            if !busy {
                if let Some(service) = weak.upgrade() {
                    service.forget_revision();
                }
            }
        });
    }

    /// The settings revision: grows with each change of what travels.
    pub fn settings_revision(&self) -> Option<u64> {
        let mut cached = self.revision.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((at, revision)) = *cached {
            if at.elapsed() < Duration::from_secs(5) {
                return Some(revision);
            }
        }
        let revision = super::transfer::revision(&self.db, &self.secrets).ok()?;
        *cached = Some((Instant::now(), revision));
        Some(revision)
    }

    /// Who installs this gateway's updates: DUOWAVE_MANAGED, else the station's mark (pc, yse).
    pub fn managed_by(&self) -> Option<String> {
        self.config.placement.managed_by.clone().or_else(|| match self.role() {
            Role::Station => StationFs::real().managed_by(),
            Role::Pc => None,
        })
    }

    /// The station this gateway runs on, as the computer that moved it here knows it (`station_self`).
    pub fn station_self(&self) -> (Option<String>, Option<String>) {
        station_self(&self.settings, &self.db)
    }

    /// Whether this gateway keeps its main settings itself: a computer always, a station unless they live on the
    /// computer — whose package then replaces them.
    pub fn owns_settings(&self) -> bool {
        self.role() == Role::Pc || self.state().settings_home == SettingsHome::Station
    }

    pub fn health_value(&self) -> Value {
        let state = self.state();
        let mut health = json!({
            "role": self.role(),
            "placement": self.placement(),
            "peerApi": self.peer_api(),
        });
        // The helpers on both roles (a 0.11 computer reads only `homeServer`, below).
        if let Some(helpers) = self.home.get().and_then(|home| home.helpers()) {
            merge(&mut health, json!({ "helpers": helpers }));
        }
        match self.role() {
            Role::Station => {
                let fs = StationFs::real();
                let data = self
                    .config
                    .data_dir
                    .as_deref()
                    .and_then(space_of)
                    .or_else(|| space_of(std::path::Path::new("/data")));
                let extra = json!({
                    "settingsRevision": self.settings_revision(),
                    "data": data,
                    "memory": own_memory(),
                    "homeServer": self.home.get().and_then(|home| home.view()),
                    "voice": fs.voice().ok().map(|voice| json!({
                        "url": voice.url,
                        "pointsTo": crate::stations::connect::quasar_config::points_to(&voice.url),
                        "local": voice == fs.local_addresses(),
                    })),
                    "managedBy": self.managed_by(),
                    "settingsHome": state.settings_home,
                    "settingsCopiedAt": super::transfer::synced_at(&self.db, "copied"),
                    "settingsReceivedAt": super::transfer::synced_at(&self.db, "received"),
                    "safety": state.safety,
                    "safetyLast": state.safety_last,
                    // A trial of a new version from memory now, or the last one that did not make it
                    // (src/updates/trial.rs): the computer's card says so, and does not offer a failed one by itself.
                    "update": json!({
                        "trial": fs.trial().map(|mark| json!({
                            "version": mark.version,
                            "since": crate::updates::trial::rfc3339(mark.started),
                            "until": crate::updates::trial::rfc3339(mark.until()),
                            "appsPaused": mark.apps_paused,
                        })),
                        "trialFailed": fs.trial_failed().map(|failed| json!({
                            "version": failed.version,
                            "at": failed.at,
                            "why": failed.why,
                            "text": failed.text(),
                        })),
                    }),
                });
                merge(&mut health, extra);
                let (device, name) = self.station_self();
                merge(&mut health, json!({ "device": device, "name": name }));
                // An ear of the home's centre (src/home/ear.rs): its computer follows the centre.
                if let Some(ear) = self.settings.get(&crate::home::ear::HOME_EAR, None) {
                    merge(&mut health, json!({ "earOf": { "address": ear.address, "port": ear.port } }));
                }
            }
            Role::Pc => {
                #[cfg(feature = "pc")]
                if let Some(stations) = self.station.get() {
                    let of = |station: &super::watch::StationGateway| {
                        let live = station.live();
                        station.record().map(|record| {
                            json!({
                                "key": station.key(),
                                "name": station.name(),
                                "device": record.device,
                                "placement": record.placement,
                                "address": record.address,
                                "port": record.port,
                                "reachable": live.reachable,
                                "seenAt": live.seen_at,
                                "version": station.station_version(),
                            })
                        })
                    };
                    let all: Vec<Value> = stations.all().iter().filter_map(|station| of(station)).collect();
                    // `station`: the one with a placement here when it is the only one (the tray, older panels).
                    merge(
                        &mut health,
                        json!({
                            "station": stations.pick(None).and_then(|station| of(&station)),
                            "stations": all,
                        }),
                    );
                }
            }
        }
        health
    }
}

/// What a station is called in a list of stations until the household names it (src/stations/service.rs).
pub const DEFAULT_STATION_NAME: &str = "Колонка";

/// The station a gateway runs on, as the computer that moved it there knows it (`STATION_SELF`): its device, and its
/// name — the one the gateway's list of stations gives that device now (renamed since, it follows), else the one the
/// move brought. The default name of a new station («Колонка») does not count. /health says it, and the gateway's own
/// name follows it while nobody names the gateway (src/core/identity/home.rs).
pub fn station_self(settings: &SettingsService, db: &Db) -> (Option<String>, Option<String>) {
    let Some(own) = settings.get(&STATION_SELF, None) else { return (None, None) };
    let listed = own
        .device
        .as_deref()
        .and_then(|device| station_name(db, device))
        .filter(|name| !name.trim().is_empty() && name != DEFAULT_STATION_NAME);
    let name = listed.or(own.name).filter(|name| !name.trim().is_empty());
    (own.device, name)
}

/// A station's name in this gateway's list of stations, by its device.
pub fn station_name(db: &Db, device: &str) -> Option<String> {
    use rusqlite::OptionalExtension;
    db.with(|connection| {
        connection
            .query_row("SELECT name FROM stations WHERE device = ?1", rusqlite::params![device], |row| row.get(0))
            .optional()
    })
    .ok()
    .flatten()
}

fn merge(into: &mut Value, extra: Value) {
    if let (Some(into), Value::Object(extra)) = (into.as_object_mut(), extra) {
        for (key, value) in extra {
            if !value.is_null() {
                into.insert(key, value);
            }
        }
    }
}

/// /health's `placement`: who this is and where it runs; the station's space, memory and home server join it.
impl crate::health_status::service::PlacementHealthSource for PlacementService {
    fn health(&self) -> Value {
        self.health_value()
    }
}

impl PlacementSource for PlacementService {
    /// «ПК помогает колонке» on: the pair's door `home-computer` switched on with its key — the address alone (a move
    /// to the station brings it) is not it, and other helpers do not make it: a 0.11 computer's switch and the
    /// registry's «с компьютера» keep meaning the computer.
    fn placement(&self) -> Placement {
        match self.role() {
            Role::Pc => Placement::Pc,
            Role::Station if self.computer_helps() => Placement::StationServer,
            Role::Station => Placement::Station,
        }
    }
}

impl PlacementService {
    /// The station leans on its computer: the pair's door switched on, with its key (src/helpers/legacy.rs).
    fn computer_helps(&self) -> bool {
        use crate::helpers::{key_secret, HOME_COMPUTER};
        let door =
            self.settings.get(&HELPERS, None).and_then(|list| list.into_iter().find(|entry| entry.id == HOME_COMPUTER));
        door.is_some_and(|entry| entry.enabled) && self.secrets.has(&key_secret(HOME_COMPUTER))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Env;

    fn station(settings: &Arc<SettingsService>, secrets: &Arc<SecretsService>, db: &Arc<Db>) -> PlacementService {
        let config = Arc::new(Config::for_tests(&Env::from_pairs::<[(&str, &str); 0], &str, &str>([])));
        let station = Profile { station: true };
        PlacementService::new(station, config, db.clone(), secrets.clone(), settings.clone(), EventBus::new())
    }

    fn entry(id: &str, url: &str, kind: crate::helpers::HelperKind, enabled: bool) -> crate::helpers::HelperSetting {
        crate::helpers::HelperSetting {
            id: id.into(),
            name: "Турбо".into(),
            url: url.into(),
            kind,
            enabled,
            without: Vec::new(),
            helper_id: None,
        }
    }

    #[test]
    fn the_station_with_its_helpers_off_is_alone() {
        use crate::helpers::HelperKind;
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let secrets = Arc::new(SecretsService::new(db.clone(), None).unwrap());
        let service = station(&settings, &secrets, &db);
        assert_eq!(service.placement(), Placement::Station);
        let door = |enabled| vec![entry("home-computer", "http://192.0.2.10", HelperKind::Home, enabled)];
        settings.set(&HELPERS, &door(false), None).unwrap();
        secrets.set("helper:home-computer:key", Some("vgw_home")).unwrap();
        assert_eq!(service.placement(), Placement::Station, "the move brought the address; the switch is off");
        settings.set(&HELPERS, &door(true), None).unwrap();
        assert_eq!(service.placement(), Placement::StationServer);
        secrets.delete("helper:home-computer:key").unwrap();
        assert_eq!(service.placement(), Placement::Station, "on, without its key: not leaned on");
    }

    #[test]
    fn another_helper_than_the_computer_leaves_the_placement_alone() {
        use crate::helpers::HelperKind;
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let secrets = Arc::new(SecretsService::new(db.clone(), None).unwrap());
        let service = station(&settings, &secrets, &db);
        settings
            .set(&HELPERS, &vec![entry("own-1", "https://helper.example.com", HelperKind::Own, true)], None)
            .unwrap();
        secrets.set("helper:own-1:key", Some("vgw_own")).unwrap();
        assert_eq!(service.placement(), Placement::Station, "«the computer helps» stays the computer's");
    }
}
