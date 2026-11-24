//! A second station as an ear of the centre — the computer's part, added to «Дом»'s `home::ear`: the centre this
//! computer is the Turbo of, as an ear dials it, and a station of its own joining as that centre's ear.

use std::sync::Arc;

use serde_json::json;

pub use duowave_home::home::ear::*;

use crate::core::events::GatewayEvent;
use crate::platform::http::ApiError;

/// The centre this computer is the Turbo of, as an ear dials it: its address, port, pair key and name.
pub fn centre_for_ear(
    station_gateway: &Arc<crate::placement::watch::StationGateways>,
) -> Result<(String, u16, String, Option<String>), ApiError> {
    let centre = crate::home::devices::centre_elsewhere(station_gateway)
        .ok_or_else(|| ApiError::conflict(crate::tr!("Хаб дома не на колонке: подключать ухом не к чему.")))?;
    let record = centre
        .record()
        .ok_or_else(|| ApiError::conflict(crate::tr!("Хаб дома не на колонке: подключать ухом не к чему.")))?;
    let key = centre.pair_key().ok_or_else(|| ApiError::conflict(crate::tr!("Нет ключа пары хаба.")))?;
    Ok((record.address, record.port, key, centre.name()))
}

/// A station this computer knows (its pair key) becomes the centre's ear: the centre's address and key handed to it.
pub async fn join_station(
    station_gateway: &Arc<crate::placement::watch::StationGateways>,
    events: &crate::core::events::EventBus,
    station: &str,
) -> Result<(), ApiError> {
    let centre = crate::home::devices::centre_elsewhere(station_gateway)
        .ok_or_else(|| ApiError::conflict(crate::tr!("Хаб дома не на колонке: подключать ухом не к чему.")))?;
    if centre.key() == station {
        return Err(ApiError::conflict(crate::tr!("Это и есть хаб дома.")));
    }
    let record = centre
        .record()
        .ok_or_else(|| ApiError::conflict(crate::tr!("Хаб дома не на колонке: подключать ухом не к чему.")))?;
    let key = centre.pair_key().ok_or_else(|| ApiError::conflict(crate::tr!("Нет ключа пары хаба.")))?;
    let found = station_gateway
        .find(station)
        .ok_or_else(|| ApiError::not_found(crate::tr!("Такой колонки у этого компьютера нет.")))?;
    // The same station under a second record (by its device and «at:<address>»): still the centre itself — an ear of
    // itself would send its voice in a circle (Windows test 21.11 offered the centre among «not in the home»).
    if found.record().is_some_and(|other| other.address == record.address) {
        return Err(ApiError::conflict(crate::tr!("Это и есть хаб дома.")));
    }
    let station =
        found.client().ok_or_else(|| ApiError::not_found(crate::tr!("Такой колонки у этого компьютера нет.")))?;
    let body = json!({ "address": record.address, "port": record.port, "key": key, "name": centre.name() });
    station.put("/api/v1/home/ear", body).await.map_err(ApiError::unavailable)?;
    events.emit(GatewayEvent::HomeDevicesChanged { reason: "join".into() });
    Ok(())
}
