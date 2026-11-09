//! The stations the gateway serves. The station's voice socket cannot sign in, and anything on the home network
//! could open it (the uniproxy server): so a station is approved before it is answered. The first one that connects
//! is trusted (the household's own, at install time); a later one waits as `pending` until the admin approves it in
//! the panel, and the owner is told. A browser cannot set the station's device header, so a web page never passes
//! as one. The station's own HTTP (its call client, its microphone bridge) is believed only from the address an
//! approved station connects from.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::access::js_len;
use crate::config::StationPairing;
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::platform::http::ApiError;
use crate::stations::looks::{look_of, look_problem, Chosen, LookSource, Reported, StationLook};
use crate::storage::Db;
use crate::utils::time::{now_iso, now_ms};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StationStatus {
    Approved,
    Pending,
    Blocked,
}

impl StationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            StationStatus::Approved => "approved",
            StationStatus::Pending => "pending",
            StationStatus::Blocked => "blocked",
        }
    }

    fn parse(value: &str) -> Option<StationStatus> {
        match value {
            "approved" => Some(StationStatus::Approved),
            "pending" => Some(StationStatus::Pending),
            "blocked" => Some(StationStatus::Blocked),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StationView {
    pub device: String,
    pub name: String,
    pub status: StationStatus,
    pub last_address: Option<String>,
    pub first_seen: String,
    pub last_seen: String,
    /// What it said about itself at its last sync: «plum», «blue», its firmware.
    pub device_model: Option<String>,
    pub device_color: Option<String>,
    pub firmware: Option<String>,
    /// Its look as a person chose it (looks.rs); None — the default.
    pub model: Option<String>,
    pub color: Option<String>,
    /// Wears a sticker (only models sold with them offer it); the panel reads it from the look.
    #[serde(skip)]
    pub sticker: Option<bool>,
}

/// A station for the panel: the record and the look in force — what the data settles and what a person chose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StationEntry {
    #[serde(flatten)]
    pub view: StationView,
    pub look: Option<StationLook>,
    /// Whether the gateway was tried with its firmware; absent when nothing is known of its model's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware_tried: Option<bool>,
}

/// On a station's voice socket: may it be served, and what it is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Admission {
    pub ok: bool,
    pub status: StationStatus,
}

/// A station's tag when its handshake names no device: never trusted on first use.
pub const UNKNOWN_DEVICE: &str = "unknown";

pub struct StationsService {
    db: Arc<Db>,
    events: EventBus,
    pairing: StationPairing,
    records: Mutex<BTreeMap<String, StationView>>,
    /// One change at a time: a change's read, its decision and both its writes (memory, then the database) are never
    /// interleaved with another's, as the TS ran each on its one thread — no stale copy is written back over a block
    /// or a removal, and two first stations are never both trusted. Readers do not wait for it.
    writing: Mutex<()>,
    /// Addresses the admin is connecting from the panel: their first connection is trusted until the time here (ms).
    invited: Mutex<HashMap<String, u64>>,
}

impl StationsService {
    /// Loads the records it already has: a failed read stops the start, as the TS bootstrap did — with none known,
    /// the first stranger would pass as the household's own. `start` then listens for a station's sync.
    pub fn new(db: Arc<Db>, events: EventBus, pairing: StationPairing) -> anyhow::Result<StationsService> {
        let records = load(&db).context("stations not loaded")?;
        let approved = records.values().filter(|station| station.status == StationStatus::Approved).count();
        let first = pairing == StationPairing::Tofu && approved == 0;
        tracing::info!(
            "stations: {approved} approved, pairing {}{}",
            pairing_label(pairing),
            if first { " (the first to connect is trusted)" } else { "" }
        );
        Ok(StationsService {
            db,
            events,
            pairing,
            records: Mutex::new(records),
            writing: Mutex::new(()),
            invited: Mutex::new(HashMap::new()),
        })
    }

    /// Listens for a station naming itself at sync. The listener does not keep the service alive.
    pub fn start(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        self.events.on("stations", move |event| {
            if let GatewayEvent::StationSynced { device_tag, device_model, device_color, firmware } = &*event {
                if let Some(me) = me.upgrade() {
                    me.reported(device_tag, device_model.as_deref(), device_color.as_deref(), firmware.as_deref());
                }
            }
        });
    }

    fn records(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, StationView>> {
        self.records.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The right to change the records, held for the whole change.
    fn writing(&self) -> std::sync::MutexGuard<'_, ()> {
        self.writing.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// On a station's voice socket: may it be served. Synchronous — the upgrade decides at once; the write follows.
    pub fn admit(&self, device: &str, address: &str) -> Admission {
        // A handshake without a device id is no station of ours (a browser cannot even send one): not kept, not asked.
        if device == UNKNOWN_DEVICE && self.pairing != StationPairing::Open {
            return Admission { ok: false, status: StationStatus::Blocked };
        }
        let now = now_iso();
        let _writing = self.writing();
        let known = self.records().get(device).cloned();
        let invited = self.invitation(address);
        if let Some(known) = known {
            if known.status == StationStatus::Pending && invited {
                self.save(StationView {
                    status: StationStatus::Approved,
                    last_address: Some(plain(address)),
                    last_seen: now,
                    ..known
                });
                tracing::info!("station {device} from {address}: approved — the admin connected it from the panel");
                return Admission { ok: true, status: StationStatus::Approved };
            }
            if known.status == StationStatus::Approved {
                self.save(StationView { last_address: Some(plain(address)), last_seen: now, ..known });
                return Admission { ok: true, status: StationStatus::Approved };
            }
            if self.pairing == StationPairing::Open {
                self.save(StationView {
                    status: StationStatus::Approved,
                    last_address: Some(plain(address)),
                    last_seen: now,
                    ..known
                });
                return Admission { ok: true, status: StationStatus::Approved };
            }
            let status = known.status;
            self.save(StationView { last_address: Some(plain(address)), last_seen: now, ..known });
            return Admission { ok: false, status };
        }
        let first = self.pairing == StationPairing::Tofu
            && device != UNKNOWN_DEVICE
            && !self.records().values().any(|station| station.status == StationStatus::Approved);
        let status = if self.pairing == StationPairing::Open || first || invited {
            StationStatus::Approved
        } else {
            StationStatus::Pending
        };
        let record = StationView {
            device: device.to_string(),
            name: "Колонка".into(),
            status,
            last_address: Some(plain(address)),
            first_seen: now.clone(),
            last_seen: now,
            device_model: None,
            device_color: None,
            firmware: None,
            model: None,
            color: None,
            sticker: None,
        };
        self.save(record);
        if status == StationStatus::Approved {
            let how = if first {
                " (trusted on first use)"
            } else if invited {
                " — the admin connected it from the panel"
            } else {
                ""
            };
            tracing::info!("station {device} from {address}: approved{how}");
        } else {
            tracing::warn!("station {device} from {address} waits for the admin's approval");
            self.events.emit(GatewayEvent::StationPending { device: device.to_string(), address: plain(address) });
        }
        Admission { ok: status == StationStatus::Approved, status }
    }

    /// The admin connects the station at this address from the panel (connect): its first connection is approved
    /// without asking again, for `ms`. A blocked station stays blocked.
    pub fn expect(&self, address: &str, ms: u64) {
        self.invited.lock().unwrap_or_else(|p| p.into_inner()).insert(plain(address), now_ms() + ms);
    }

    /// Whether an address is invited now; an invitation is used once.
    fn invitation(&self, address: &str) -> bool {
        let key = plain(address);
        let mut invited = self.invited.lock().unwrap_or_else(|p| p.into_inner());
        match invited.remove(&key) {
            Some(until) => until > now_ms(),
            None => false,
        }
    }

    /// The approved station that connects from an address: whose its own clients are (the microphone, the calls).
    pub fn device_at(&self, address: Option<&str>) -> Option<String> {
        let wanted = plain(address.unwrap_or(""));
        self.records()
            .values()
            .find(|station| {
                station.status == StationStatus::Approved && station.last_address.as_deref() == Some(&*wanted)
            })
            .map(|station| station.device.clone())
    }

    /// It was heard at `at` (ms): kept with its record, so a restart remembers when.
    pub fn seen(&self, device: &str, at: u64) {
        let _writing = self.writing();
        let known = self.records().get(device).cloned();
        if let Some(known) = known {
            self.save(StationView { last_seen: iso_at(at), ..known });
        }
    }

    /// Whether an address is an approved station's (its call client, its microphone bridge); any, with `open`.
    pub fn trusted_address(&self, address: Option<&str>) -> bool {
        if self.pairing == StationPairing::Open {
            return true;
        }
        let wanted = plain(address.unwrap_or(""));
        self.records().values().any(|station| {
            station.status == StationStatus::Approved && station.last_address.as_deref() == Some(&*wanted)
        })
    }

    /// What the station says about itself at sync — its model and colour, kept with its record: they settle its
    /// look in the panel (looks.rs) — and its firmware: another one than at the last sync is an update from Yandex,
    /// said in the journal (station.firmware). A sync that names nothing keeps what an earlier one said.
    pub fn reported(
        &self,
        device: &str,
        device_model: Option<&str>,
        device_color: Option<&str>,
        firmware: Option<&str>,
    ) {
        let _writing = self.writing();
        let known = match self.records().get(device).cloned() {
            Some(known) => known,
            None => return,
        };
        let device_model = word(device_model).or_else(|| known.device_model.clone());
        let device_color = word(device_color).or_else(|| known.device_color.clone());
        let firmware = word(firmware).or_else(|| known.firmware.clone());
        if device_model == known.device_model && device_color == known.device_color && firmware == known.firmware {
            return;
        }
        if let (Some(from), Some(to)) = (&known.firmware, &firmware) {
            if from != to {
                let tried = super::firmware::tried(device_model.as_deref(), Some(to));
                tracing::warn!(
                    "station {device}: firmware {from} → {to}: updated by Yandex{}",
                    match tried {
                        Some(false) => "; the gateway was not tried with it",
                        _ => "",
                    }
                );
                self.events.emit(GatewayEvent::StationFirmwareChanged {
                    device: device.to_string(),
                    from: from.clone(),
                    to: to.clone(),
                    tried,
                });
            }
        }
        tracing::info!(
            "station {device}: {}, {}, firmware {}",
            device_model.as_deref().unwrap_or("model unknown"),
            device_color.as_deref().unwrap_or("colour unknown"),
            firmware.as_deref().unwrap_or("unknown")
        );
        self.save(StationView { device_model, device_color, firmware, ..known });
    }

    /// One station's record, as it is now.
    pub fn find(&self, device: &str) -> Option<StationView> {
        self.records().get(device).cloned()
    }

    /// The station (any status) whose last address is this one: the connect flow, to name the station at an address.
    pub fn device_for_address(&self, address: &str) -> Option<String> {
        self.records()
            .values()
            .find(|station| station.last_address.as_deref() == Some(address))
            .map(|station| station.device.clone())
    }

    pub fn list(&self) -> Vec<StationEntry> {
        let mut records: Vec<StationView> = self.records().values().cloned().collect();
        records.sort_by(|a, b| a.first_seen.cmp(&b.first_seen));
        records.into_iter().map(with_look).collect()
    }

    pub fn update(
        &self,
        device: &str,
        input: &serde_json::Value,
        actor: Option<Actor>,
    ) -> Result<StationEntry, ApiError> {
        let empty = serde_json::Map::new();
        let input = input.as_object().unwrap_or(&empty);
        let writing = self.writing();
        let known = self.find(device).ok_or_else(|| ApiError::not_found("Нет такой колонки."))?;
        let mut next = known.clone();
        if let Some(status) = input.get("status") {
            match status.as_str().and_then(StationStatus::parse) {
                Some(status) => next.status = status,
                None => return Err(ApiError::bad_request("status: approved, pending или blocked")),
            }
        }
        if let Some(name) = input.get("name") {
            let name = name.as_str().map(str::trim).unwrap_or("");
            // JavaScript's length, in UTF-16 units: an emoji counts two.
            let length = js_len(name);
            if !(1..=40).contains(&length) {
                return Err(ApiError::bad_request("Имя колонки — до 40 символов."));
            }
            next.name = name.to_string();
        }
        if input.contains_key("model") || input.contains_key("color") {
            let model = choice_of(input.get("model"), "model", known.model.clone())?;
            let mut color = choice_of(input.get("color"), "color", known.color.clone())?;
            // A new variant drops a chosen colour it is not sold in: the variant's first stands.
            if !input.contains_key("color") && color.is_some() {
                let look = look_of(reported(&known), Chosen { model: model.as_deref(), color: color.as_deref() });
                if look.map(|look| look.color_from) != Some(LookSource::Chosen) {
                    color = None;
                }
            }
            let problem = look_problem(
                reported(&known),
                Chosen {
                    model: if input.contains_key("model") { model.as_deref() } else { None },
                    color: if input.contains_key("color") { color.as_deref() } else { None },
                },
            );
            if let Some(problem) = problem {
                return Err(ApiError::bad_request(problem));
            }
            next.model = model;
            next.color = color;
        }
        if let Some(sticker) = input.get("sticker") {
            let sticker = match sticker {
                serde_json::Value::Null => None,
                serde_json::Value::Bool(on) => Some(*on),
                _ => return Err(ApiError::bad_request("sticker: true, false или null")),
            };
            let offered = look_of(reported(&known), Chosen::default()).is_some_and(|look| look.sticker.is_some());
            if sticker.is_some() && !offered {
                return Err(ApiError::bad_request("Наклейки бывают только у Лайт 2."));
            }
            next.sticker = sticker;
        }
        self.save(next.clone());
        drop(writing);
        self.events.emit(GatewayEvent::StationUpdated {
            device: device.to_string(),
            status: next.status.as_str().to_string(),
            name: Some(next.name.clone()),
            model: Some(next.model.clone()),
            color: Some(next.color.clone()),
            actor,
        });
        Ok(with_look(next))
    }

    pub fn remove(&self, device: &str, actor: Option<Actor>) -> Result<(), ApiError> {
        let writing = self.writing();
        if self.records().remove(device).is_none() {
            return Err(ApiError::not_found("Нет такой колонки."));
        }
        self.db.with(|c| c.execute("DELETE FROM stations WHERE device = ?1", params![device]))?;
        drop(writing);
        self.events.emit(GatewayEvent::StationUpdated {
            device: device.to_string(),
            status: "removed".into(),
            name: None,
            model: None,
            color: None,
            actor,
        });
        Ok(())
    }

    /// Keeps a record in memory, then in the database. Only under `writing`: then memory and the database take the
    /// changes in the same order.
    fn save(&self, record: StationView) {
        self.records().insert(record.device.clone(), record.clone());
        let result = self.db.with(|c| {
            c.execute(
                "INSERT INTO stations
                     (device, name, status, last_address, first_seen, last_seen, device_model, device_color, model, color, sticker,
                      firmware)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT (device) DO UPDATE SET
                     name = excluded.name, status = excluded.status, last_address = excluded.last_address,
                     last_seen = excluded.last_seen, device_model = excluded.device_model,
                     device_color = excluded.device_color, model = excluded.model, color = excluded.color,
                     sticker = excluded.sticker, firmware = excluded.firmware",
                params![
                    record.device,
                    record.name,
                    record.status.as_str(),
                    record.last_address,
                    record.first_seen,
                    record.last_seen,
                    record.device_model,
                    record.device_color,
                    record.model,
                    record.color,
                    record.sticker,
                    record.firmware,
                ],
            )
        });
        if let Err(error) = result {
            tracing::warn!("station {} not saved: {error}", record.device);
        }
    }
}

fn load(db: &Db) -> rusqlite::Result<BTreeMap<String, StationView>> {
    let stations = db.with(|c| {
        let mut statement = c.prepare(
            "SELECT device, name, status, last_address, first_seen, last_seen, device_model, device_color, model, color,
                    sticker, firmware
             FROM stations",
        )?;
        let rows = statement.query_map([], |row| {
            let status: String = row.get(2)?;
            Ok(StationView {
                device: row.get(0)?,
                name: row.get(1)?,
                status: StationStatus::parse(&status).unwrap_or(StationStatus::Pending),
                last_address: row.get(3)?,
                first_seen: row.get(4)?,
                last_seen: row.get(5)?,
                device_model: row.get(6)?,
                device_color: row.get(7)?,
                model: row.get(8)?,
                color: row.get(9)?,
                sticker: row.get(10)?,
                firmware: row.get(11)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })?;
    Ok(stations.into_iter().map(|station| (station.device.clone(), station)).collect())
}

fn with_look(record: StationView) -> StationEntry {
    let look = look_of(reported(&record), Chosen { model: record.model.as_deref(), color: record.color.as_deref() })
        .map(|look| StationLook { sticker: look.sticker.map(|_| record.sticker.unwrap_or(false)), ..look });
    let firmware_tried = super::firmware::tried(record.device_model.as_deref(), record.firmware.as_deref());
    StationEntry { view: record, look, firmware_tried }
}

fn reported(record: &StationView) -> Reported<'_> {
    Reported { device_model: record.device_model.as_deref(), device_color: record.device_color.as_deref() }
}

/// A station's word about itself, if it is one: short, plain letters (JavaScript `/^[\w .-]{1,40}$/`).
fn word(value: Option<&str>) -> Option<String> {
    let text = value.unwrap_or("").trim();
    let fits = (1..=40).contains(&text.chars().count())
        && text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ' ' | '.' | '-'));
    fits.then(|| text.to_string())
}

/// A look choice from the panel: an id, null to go back to the default, or what is kept when it is not sent.
fn choice_of(value: Option<&serde_json::Value>, field: &str, kept: Option<String>) -> Result<Option<String>, ApiError> {
    match value {
        None => Ok(kept),
        Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(text)) if text.is_empty() => Ok(None),
        Some(serde_json::Value::String(text))
            if (1..=24).contains(&text.len())
                && text.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') =>
        {
            Ok(Some(text.clone()))
        }
        Some(_) => Err(ApiError::bad_request(format!("{field}: id варианта или null"))),
    }
}

/// "::ffff:192.168.1.5" and "192.168.1.5" are one address.
fn plain(address: &str) -> String {
    match address.get(..7) {
        Some(prefix) if prefix.eq_ignore_ascii_case("::ffff:") => address[7..].to_string(),
        _ => address.to_string(),
    }
}

/// An ISO time from a ms timestamp.
fn iso_at(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64).map(crate::utils::time::iso).unwrap_or_else(now_iso)
}

fn pairing_label(pairing: StationPairing) -> &'static str {
    match pairing {
        StationPairing::Tofu => "tofu",
        StationPairing::Closed => "closed",
        StationPairing::Open => "open",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(pairing: StationPairing) -> Arc<StationsService> {
        Arc::new(StationsService::new(Arc::new(Db::memory()), EventBus::new(), pairing).unwrap())
    }

    #[test]
    fn tofu_trusts_the_first_and_asks_about_later_ones() {
        let stations = service(StationPairing::Tofu);
        assert_eq!(stations.admit("kitchen", "192.168.1.10").status, StationStatus::Approved);
        // The first is approved; a second waits.
        assert_eq!(stations.admit("bedroom", "192.168.1.11"), Admission { ok: false, status: StationStatus::Pending });
        // The known approved one keeps being served; the pending one keeps waiting.
        assert!(stations.admit("kitchen", "192.168.1.10").ok);
        assert!(!stations.admit("bedroom", "192.168.1.11").ok);
        // A handshake with no device id is never a station of ours.
        assert_eq!(stations.admit(UNKNOWN_DEVICE, "192.168.1.9").status, StationStatus::Blocked);
    }

    #[test]
    fn an_invitation_approves_the_next_connection_once() {
        let stations = service(StationPairing::Tofu);
        stations.admit("kitchen", "192.168.1.10"); // trusts the first, so bedroom would otherwise wait
        stations.expect("::ffff:192.168.1.20", 60_000);
        // The invited address is approved at once, by its plain form.
        assert!(stations.admit("bedroom", "192.168.1.20").ok);
        // The invitation is spent: a brand-new station from it would wait again.
        assert!(!stations.admit("studio", "192.168.1.20").ok);
    }

    #[test]
    fn closed_admits_only_the_already_approved_and_open_admits_any() {
        let closed = service(StationPairing::Closed);
        assert_eq!(closed.admit("kitchen", "192.168.1.10"), Admission { ok: false, status: StationStatus::Pending });
        let open = service(StationPairing::Open);
        assert!(open.admit("anything", "192.168.1.10").ok);
        assert!(open.admit(UNKNOWN_DEVICE, "192.168.1.10").ok, "open takes an unknown device too");
        assert!(open.trusted_address(Some("10.0.0.1")), "open trusts any address");
    }

    #[test]
    fn binds_the_clients_to_the_address_an_approved_station_connects_from() {
        let stations = service(StationPairing::Open);
        stations.admit("kitchen", "::ffff:192.168.1.100");
        assert_eq!(stations.device_at(Some("192.168.1.100")).as_deref(), Some("kitchen"));
        assert!(stations.trusted_address(Some("192.168.1.100")));
        assert_eq!(stations.device_at(Some("192.168.1.200")), None);
    }

    #[test]
    fn a_sync_settles_the_look_and_persists_across_a_reload() {
        let db = Arc::new(Db::memory());
        let stations = Arc::new(StationsService::new(db.clone(), EventBus::new(), StationPairing::Open).unwrap());
        stations.admit("kitchen", "192.168.1.100");
        stations.reported("kitchen", Some("plum"), Some("green"), None);
        let entry = stations.list().into_iter().find(|entry| entry.view.device == "kitchen").unwrap();
        let look = entry.look.unwrap();
        assert_eq!(look.model, "plain");
        assert_eq!(look.color, "green");
        // A reload reads the reported model and colour back.
        let again = StationsService::new(db, EventBus::new(), StationPairing::Open).unwrap();
        let reloaded = again.find("kitchen").unwrap();
        assert_eq!(reloaded.device_model.as_deref(), Some("plum"));
        assert_eq!(reloaded.device_color.as_deref(), Some("green"));
    }

    #[test]
    fn puts_a_sticker_on_a_lite_2_keeps_it_across_a_reload_and_refuses_it_elsewhere() {
        let db = Arc::new(Db::memory());
        let stations = Arc::new(StationsService::new(db.clone(), EventBus::new(), StationPairing::Open).unwrap());
        stations.admit("kitchen", "192.168.1.100");
        stations.admit("hall", "192.168.1.101");
        stations.reported("kitchen", Some("plum"), Some("blue"), None);
        stations.reported("hall", Some("yandexmini_2"), None, None);
        assert_eq!(
            stations.list().iter().find(|e| e.view.device == "kitchen").unwrap().look.as_ref().unwrap().sticker,
            Some(false)
        );
        let worn = stations.update("kitchen", &serde_json::json!({ "sticker": true }), None).unwrap();
        assert_eq!(worn.look.unwrap().sticker, Some(true));
        let again = StationsService::new(db, EventBus::new(), StationPairing::Open).unwrap();
        assert_eq!(again.find("kitchen").unwrap().sticker, Some(true));
        let refused = stations.update("hall", &serde_json::json!({ "sticker": true }), None).unwrap_err();
        assert_eq!(refused.message, "Наклейки бывают только у Лайт 2.");
        assert!(stations.update("kitchen", &serde_json::json!({ "sticker": "yes" }), None).is_err());
        assert_eq!(
            stations.update("kitchen", &serde_json::json!({ "sticker": null }), None).unwrap().look.unwrap().sticker,
            Some(false)
        );
    }

    #[test]
    fn names_a_station_and_refuses_a_look_the_data_has_settled() {
        let stations = service(StationPairing::Open);
        stations.admit("kitchen", "192.168.1.100");
        stations.reported("kitchen", Some("plum"), Some("green"), None);
        let named = stations.update("kitchen", &serde_json::json!({ "name": "Кухня" }), None).unwrap();
        assert_eq!(named.view.name, "Кухня");
        let problem = stations.update("kitchen", &serde_json::json!({ "model": "clock" }), None).unwrap_err();
        assert_eq!(problem.message, "Модель колонка сообщила сама.");
        assert!(stations.update("absent", &serde_json::json!({}), None).is_err());
    }

    #[test]
    fn counts_a_name_as_javascript_does() {
        let stations = service(StationPairing::Open);
        stations.admit("kitchen", "192.168.1.100");
        // 30 letters and 6 emoji: 36 characters, 42 UTF-16 units — over the 40 the TS allowed.
        let long = format!("{}{}", "Я".repeat(30), "🎵".repeat(6));
        let refused = stations.update("kitchen", &serde_json::json!({ "name": long }), None).unwrap_err();
        assert_eq!(refused.message, "Имя колонки — до 40 символов.");
        let fits = format!("{}{}", "Я".repeat(30), "🎵".repeat(5));
        assert_eq!(stations.update("kitchen", &serde_json::json!({ "name": fits }), None).unwrap().view.name, fits);
    }

    #[test]
    fn a_failed_read_of_the_records_stops_the_start() {
        let db = Arc::new(Db::memory());
        db.with(|c| c.execute_batch("DROP TABLE stations")).unwrap();
        // With no station known, the first stranger would be trusted as the household's own.
        let error = StationsService::new(db, EventBus::new(), StationPairing::Tofu).err().expect("no start");
        assert!(format!("{error:#}").starts_with("stations not loaded: "), "{error:#}");
    }

    /// Waits until `check` holds, letting the listeners run; fails after a short while.
    async fn until(mut check: impl FnMut() -> bool) {
        for _ in 0..200 {
            if check() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("timed out");
    }

    #[test]
    fn says_when_yandex_updated_a_station_and_whether_the_gateway_was_tried_with_it() {
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let mut heard = events.subscribe();
        let stations = StationsService::new(db.clone(), events.clone(), StationPairing::Open).unwrap();
        stations.admit("kitchen", "192.168.1.100");
        let tried = "0.369.1.50.4794654278.20261021.133";
        // The first sync names it: kept, nothing to say.
        stations.reported("kitchen", Some("plum"), None, Some(tried));
        assert_eq!(stations.list()[0].firmware_tried, Some(true));
        let firmware_said = |event: &Arc<GatewayEvent>| matches!(&**event, GatewayEvent::StationFirmwareChanged { .. });
        assert!(!std::iter::from_fn(|| heard.try_recv().ok()).any(|event| firmware_said(&event)));
        // Another one later: an update, said in the journal.
        stations.reported("kitchen", Some("plum"), None, Some("0.370.1.1.4900000000.20261201.1"));
        let changed = std::iter::from_fn(|| heard.try_recv().ok()).find(firmware_said).expect("station.firmware");
        let GatewayEvent::StationFirmwareChanged { device, from, to, tried: was_tried } = &*changed else {
            unreachable!()
        };
        assert_eq!((device.as_str(), from.as_str(), was_tried), ("kitchen", tried, &Some(false)));
        assert!(to.starts_with("0.370."));
        assert_eq!(stations.list()[0].firmware_tried, Some(false), "the panel says it was not tried");
        // A sync that names nothing keeps it; and it is kept across a restart.
        stations.reported("kitchen", None, None, None);
        let again = StationsService::new(db, EventBus::new(), StationPairing::Open).unwrap();
        assert!(again.find("kitchen").unwrap().firmware.unwrap().starts_with("0.370."));
    }

    #[tokio::test]
    async fn keeps_what_a_syncing_station_says_about_itself_and_lets_a_person_pick_only_what_that_leaves_open() {
        let events = EventBus::new();
        let stations =
            Arc::new(StationsService::new(Arc::new(Db::memory()), events.clone(), StationPairing::Open).unwrap());
        stations.start();
        stations.admit("kitchen", "192.168.1.100");
        events.emit(GatewayEvent::StationSynced {
            device_tag: "kitchen".into(),
            device_model: Some("plum".into()),
            device_color: Some("blue".into()),
            firmware: None,
        });
        until(|| stations.find("kitchen").is_some_and(|station| station.device_color.is_some())).await;
        let entry = stations.list().remove(0);
        assert_eq!(entry.view.device_model.as_deref(), Some("plum"));
        assert_eq!(entry.view.device_color.as_deref(), Some("blue"));
        assert_eq!((entry.view.model.as_deref(), entry.view.color.as_deref()), (None, None));
        // The Lite 2 says its colour, not whether it has the clock: blue comes in both, so the clock is a person's choice.
        let look = entry.look.unwrap();
        assert_eq!((look.family, look.model, look.color), ("lite2", "clock", "azure"));
        assert_eq!((look.model_from, look.color_from), (LookSource::Default, LookSource::Station));

        let input = serde_json::json!({ "model": "plain", "name": "Кухня" });
        let picked = stations.update("kitchen", &input, None).unwrap();
        assert_eq!((picked.view.name.as_str(), picked.view.model.as_deref()), ("Кухня", Some("plain")));
        let look = picked.look.unwrap();
        assert_eq!((look.model, look.clock, look.model_from), ("plain", false, LookSource::Chosen));
        let refused = stations.update("kitchen", &serde_json::json!({ "color": "pink" }), None).unwrap_err();
        assert_eq!(refused.message, "Цвет колонка сообщила сама.");
        assert!(stations.update("kitchen", &serde_json::json!({ "model": "big" }), None).is_err());
        let reset = stations.update("kitchen", &serde_json::json!({ "model": null }), None).unwrap().look.unwrap();
        assert_eq!((reset.model, reset.model_from), ("clock", LookSource::Default));
    }

    #[test]
    fn lets_a_later_station_in_only_once_the_admin_approves_it_and_not_once_blocked() {
        let stations = service(StationPairing::Tofu);
        assert!(stations.admit("KITCHEN01", "192.168.1.10").ok);
        assert!(stations.admit("KITCHEN01", "192.168.1.10").ok);
        assert_eq!(
            stations.admit("STRANGER9", "192.168.1.11"),
            Admission { ok: false, status: StationStatus::Pending }
        );
        assert!(!stations.admit(UNKNOWN_DEVICE, "192.168.1.12").ok);
        let statuses: Vec<StationStatus> = stations.list().into_iter().map(|entry| entry.view.status).collect();
        assert_eq!(statuses, vec![StationStatus::Approved, StationStatus::Pending]);
        let approve = serde_json::json!({ "status": "approved", "name": "Детская" });
        stations.update("STRANGER9", &approve, None).unwrap();
        assert!(stations.admit("STRANGER9", "192.168.1.11").ok);
        stations.update("STRANGER9", &serde_json::json!({ "status": "blocked" }), None).unwrap();
        assert_eq!(
            stations.admit("STRANGER9", "192.168.1.11"),
            Admission { ok: false, status: StationStatus::Blocked }
        );
    }

    #[tokio::test]
    async fn its_sync_listener_does_not_keep_it_alive() {
        let stations = service(StationPairing::Open);
        stations.start();
        let left = Arc::downgrade(&stations);
        drop(stations);
        assert!(left.upgrade().is_none(), "a restarted engine leaves the old records behind");
    }

    #[test]
    fn two_new_stations_at_once_on_a_fresh_gateway_trust_only_one() {
        for _ in 0..20 {
            let stations = service(StationPairing::Tofu);
            let start = Arc::new(std::sync::Barrier::new(6));
            let admissions: Vec<_> = (0..6)
                .map(|n| {
                    let (stations, start) = (stations.clone(), start.clone());
                    std::thread::spawn(move || {
                        start.wait();
                        stations.admit(&format!("station-{n}"), &format!("192.168.1.{}", 10 + n))
                    })
                })
                .collect();
            let trusted = admissions.into_iter().map(|admission| admission.join().unwrap()).filter(|a| a.ok).count();
            assert_eq!(trusted, 1, "the first is trusted, the others wait for the admin");
        }
    }

    /// Marks the station seen, again and again, until told to stop: what presence does on its own task.
    fn seen_meanwhile(
        stations: &Arc<StationsService>,
    ) -> (Arc<std::sync::atomic::AtomicBool>, std::thread::JoinHandle<()>) {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (stations, stopped) = (stations.clone(), stop.clone());
        let seen = std::thread::spawn(move || {
            while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
                stations.seen("kitchen", now_ms());
            }
        });
        (stop, seen)
    }

    #[test]
    fn a_change_is_never_undone_by_the_station_being_seen_meanwhile() {
        let db = Arc::new(Db::memory());
        let stations = Arc::new(StationsService::new(db.clone(), EventBus::new(), StationPairing::Tofu).unwrap());
        stations.admit("kitchen", "192.168.1.100");
        let (stop, seen) = seen_meanwhile(&stations);
        for n in 0..300 {
            let name = format!("Кухня {n}");
            stations.update("kitchen", &serde_json::json!({ "name": name }), None).unwrap();
            assert_eq!(stations.find("kitchen").unwrap().name, name, "a stale copy was written back over the change");
        }
        stations.update("kitchen", &serde_json::json!({ "status": "blocked" }), None).unwrap();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        seen.join().unwrap();
        assert_eq!(stations.find("kitchen").unwrap().status, StationStatus::Blocked);
        // The database took the changes in the same order.
        let reloaded = StationsService::new(db, EventBus::new(), StationPairing::Tofu).unwrap();
        assert_eq!(reloaded.find("kitchen"), stations.find("kitchen"));
    }

    #[test]
    fn a_station_forgotten_while_it_is_seen_stays_forgotten() {
        let db = Arc::new(Db::memory());
        let stations = Arc::new(StationsService::new(db.clone(), EventBus::new(), StationPairing::Open).unwrap());
        stations.admit("kitchen", "192.168.1.100");
        let (stop, seen) = seen_meanwhile(&stations);
        for _ in 0..300 {
            stations.remove("kitchen", None).unwrap();
            assert_eq!(stations.find("kitchen"), None, "a stale copy brought the station back");
            stations.admit("kitchen", "192.168.1.100");
        }
        stations.remove("kitchen", None).unwrap();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        seen.join().unwrap();
        assert_eq!(stations.find("kitchen"), None);
        let reloaded = StationsService::new(db, EventBus::new(), StationPairing::Open).unwrap();
        assert_eq!(reloaded.find("kitchen"), None, "not back in the database either");
    }

    #[test]
    fn forgets_a_station() {
        let stations = service(StationPairing::Open);
        stations.admit("kitchen", "192.168.1.100");
        assert!(stations.remove("kitchen", None).is_ok());
        assert_eq!(stations.find("kitchen"), None);
        assert!(stations.remove("kitchen", None).is_err());
    }
}
