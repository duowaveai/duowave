pub mod looks;
pub mod presence;

use crate::settings::definitions::STATION_SELF;
use crate::settings::SettingsService;
use crate::storage::Db;

/// What a station is called in a list of stations until the household names it (src/stations/service.rs).
pub const DEFAULT_STATION_NAME: &str = "Колонка";

/// The station a gateway runs on, as the computer that moved it there knows it (`STATION_SELF`): its device, and its
/// name — the one the gateway's list of stations gives that device now (renamed since, it follows), else the one the
/// move brought. The default name of a new station («Колонка») does not count. /health says it, and the gateway's own
/// name follows it while nobody names the gateway (src/core/identity/home.rs).
pub fn station_self(settings: &SettingsService, db: &Db) -> (Option<String>, Option<String>) {
    // No computer moved it here (installed from the terminal, stage 1 of docs/plans/home-setup-flow.md): the station
    // whose voice comes from this very machine is its own. Without it a Turbo that joined by the code never learned
    // the station's device, and its panel offered the hub and its ears as «не в доме» (pretend stations, 23.11).
    let Some(own) = settings.get(&STATION_SELF, None) else {
        let device = own_station(db);
        let name = device
            .as_deref()
            .and_then(|device| station_name(db, device))
            .filter(|name| !name.trim().is_empty() && name != DEFAULT_STATION_NAME);
        return (device, name);
    };
    let listed = own
        .device
        .as_deref()
        .and_then(|device| station_name(db, device))
        .filter(|name| !name.trim().is_empty() && name != DEFAULT_STATION_NAME);
    let name = listed.or(own.name).filter(|name| !name.trim().is_empty());
    (own.device, name)
}

/// The approved station whose voice comes from this machine itself (quasar dials 127.0.0.1), the latest seen.
fn own_station(db: &Db) -> Option<String> {
    use rusqlite::OptionalExtension;
    db.with(|connection| {
        connection
            .query_row(
                "SELECT device FROM stations WHERE status = 'approved' AND last_address IN ('127.0.0.1', '::1') \
                 ORDER BY last_seen DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
    })
    .ok()
    .flatten()
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
