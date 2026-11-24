//! What the placement asks of the computer (`placement::computer::PlacementComputer`, «Дом»): the computer's answer,
//! over its stations' watch (watch.rs), its moves (moves.rs) and the addresses a station dials it at.

use std::sync::{Arc, OnceLock};

use futures::future::BoxFuture;
use serde_json::{json, Map, Value};

pub use duowave_home::placement::computer::*;

use super::moves::MovesService;
use super::watch::{StationGateway, StationGateways};
use crate::midi::routes::StationPair;
use crate::platform::http::ApiError;
use crate::stations::connect::station_connect::StationConnectService;

/// What /health says of the home this computer is the Turbo of over the link (`placement.centre`); none when it is no
/// such Turbo.
pub type Centre = Arc<dyn Fn() -> Option<Value> + Send + Sync>;

/// The computer's parts the placement works with.
pub struct Computer {
    pub station_gateway: Arc<StationGateways>,
    pub moves: Arc<MovesService>,
    pub station_connect: Arc<StationConnectService>,
    /// The home this computer is the Turbo of over the link, as /health tells it (`home::devices::link_centre`): set
    /// once the gateway is built.
    centre: OnceLock<Centre>,
}

impl Computer {
    pub fn new(
        station_gateway: Arc<StationGateways>,
        moves: Arc<MovesService>,
        station_connect: Arc<StationConnectService>,
    ) -> Self {
        Self { station_gateway, moves, station_connect, centre: OnceLock::new() }
    }

    pub fn set_centre(&self, centre: Centre) {
        let _ = self.centre.set(centre);
    }
}

/// A station's gateway as /health tells it.
fn station_health(station: &StationGateway) -> Option<Value> {
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
}

impl PlacementComputer for Computer {
    fn health(&self) -> Value {
        let stations = &self.station_gateway;
        let all: Vec<Value> = stations.all().iter().filter_map(|station| station_health(station)).collect();
        // `station`: the one with a placement here when it is the only one (the tray, older panels).
        let mut health = json!({
            "station": stations.pick(None).and_then(|station| station_health(&station)),
            "stations": all,
        });
        // The home's centre when this computer is its Turbo over the link (stage 7 of home-setup-flow): the panel goes
        // to that home's panel through the pass with its key.
        if let Some(centre) = self.centre.get().and_then(|centre| centre()) {
            health["centre"] = centre;
        }
        // macOS keeps this program off the home network (no «Local Network» for DuoWave): stations and the home's other
        // computers do not see it, and the panel says where to allow it (step 11 check, finding 3).
        if cfg!(target_os = "macos") && crate::stations::connect::discovery::OFF_NETWORK.now() {
            health["localNetwork"] = json!("blocked");
        }
        health
    }

    fn view(&self, station: Option<&str>) -> Map<String, Value> {
        let mut view = Map::new();
        let mut put = |name: &str, value: Value| {
            view.insert(name.to_string(), value);
        };
        if let Some(station) = self.station_gateway.pick(station) {
            put("station", serde_json::to_value(station.view()).unwrap_or_default());
        }
        let stations = self.station_gateway.views();
        if !stations.is_empty() {
            put("stations", serde_json::to_value(stations).unwrap_or_default());
        }
        if let Some(job) = self.moves.running() {
            put("job", serde_json::to_value(job).unwrap_or_default());
        }
        let addresses = self.station_connect.home_addresses(
            &crate::stations::connect::station_connect::host_interfaces(),
            crate::stations::connect::station_connect::in_container(),
        );
        if !addresses.is_empty() {
            put("homeAddresses", json!(addresses));
        }
        view
    }

    fn safety<'a>(&'a self, station: Option<&'a str>, on: bool) -> BoxFuture<'a, Result<(), ApiError>> {
        Box::pin(async move {
            let pick = super::routes::Pick { station: station.map(str::to_string) };
            let client = super::routes::chosen(&self.station_gateway, &pick)?
                .client()
                .ok_or_else(|| ApiError::conflict(crate::tr!("Хаб не на колонке.")))?;
            client.post("/api/v1/placement/safety", json!({ "on": on })).await.map_err(ApiError::unavailable)?;
            Ok(())
        })
    }

    fn pair_of(&self, station: &str) -> StationPair {
        let Some(gateway) = self.station_gateway.find(station) else {
            return StationPair::Unknown;
        };
        match gateway.client() {
            Some(peer) => StationPair::Paired { base: peer.base().to_string(), key: peer.key().to_string() },
            None => StationPair::Unpaired,
        }
    }

    fn turbo(&self) -> bool {
        !self.station_gateway.all().is_empty()
    }

    fn device_states(&self) -> Vec<Value> {
        let mut found = Vec::new();
        for station in self.station_gateway.all() {
            let live = station.live();
            let state = live.health.as_ref().and_then(|health| health.get("deviceState")).cloned();
            if let Some(mut state) = state.filter(Value::is_object) {
                state["reachable"] = live.reachable.into();
                found.push(state);
            }
        }
        found
    }
}
