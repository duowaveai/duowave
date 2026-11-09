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
use crate::settings::definitions::{GatewayState, SafetyCase, SettingsHome, CONNECTOR_HOME_SERVER, GATEWAY_STATE};
use crate::settings::SettingsService;
use crate::storage::Db;

/// Which gateway this is: the computer's, or the one on the station.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Pc,
    Station,
}

/// The home server as the station's gateway sees it (src/home_server): its address, up or down, since when.
pub trait HomeServerView: Send + Sync {
    fn view(&self) -> Option<Value>;
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
    restarter: OnceLock<Restarter>,
    /// The revision, computed at most every few seconds: the panels and the computer ask /health often.
    revision: Mutex<Option<(Instant, u64)>>,
}

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
            restarter: OnceLock::new(),
            revision: Mutex::new(None),
        }
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
        let _ = self.restarter.set(restarter);
    }

    /// Starts this gateway again: new settings are applied at its start.
    pub fn restart(&self) {
        match self.restarter.get() {
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

    /// The pair's key this gateway is reached with: the station's .env (PEER_KEY).
    pub fn peer_key(&self) -> Option<&str> {
        self.config.placement.peer_key.as_deref()
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
            if let Ok(program) = std::env::current_exe() {
                if StationFs::real().settle_update(&program) {
                    tracing::info!("update: the new version ran ten minutes; the one before is removed");
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

    pub fn health_value(&self) -> Value {
        let state = self.state();
        let mut health = json!({
            "role": self.role(),
            "placement": self.placement(),
            "peerApi": self.peer_api(),
        });
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
                });
                merge(&mut health, extra);
            }
            Role::Pc => {
                #[cfg(feature = "pc")]
                if let Some(stations) = self.station.get() {
                    let of = |station: &super::watch::StationGateway| {
                        let live = station.live();
                        station.record().map(|record| {
                            json!({
                                "key": station.key(),
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
    fn placement(&self) -> Placement {
        match self.role() {
            Role::Pc => Placement::Pc,
            // «ПК помогает колонке» on: the address and the key alone (a move to the station brings them) are not it.
            Role::Station if self.settings.get(&CONNECTOR_HOME_SERVER, None).is_some_and(|home| home.enabled) => {
                Placement::StationServer
            }
            Role::Station => Placement::Station,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Env;
    use crate::settings::definitions::HomeServerSetting;

    #[test]
    fn the_station_with_the_computer_known_but_not_leaned_on_is_alone() {
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let secrets = Arc::new(SecretsService::new(db.clone(), None).unwrap());
        let config = Arc::new(Config::for_tests(&Env::from_pairs::<[(&str, &str); 0], &str, &str>([])));
        let service =
            PlacementService::new(Profile { station: true }, config, db, secrets, settings.clone(), EventBus::new());
        assert_eq!(service.placement(), Placement::Station);
        let home = |enabled| HomeServerSetting { url: "http://192.0.2.10".into(), enabled };
        settings.set(&CONNECTOR_HOME_SERVER, &home(false), None).unwrap();
        assert_eq!(service.placement(), Placement::Station, "the move brought the address; the switch is off");
        settings.set(&CONNECTOR_HOME_SERVER, &home(true), None).unwrap();
        assert_eq!(service.placement(), Placement::StationServer);
    }
}
