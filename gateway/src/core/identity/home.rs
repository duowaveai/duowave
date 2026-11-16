//! Who this gateway is (docs/plans/home-and-helpers.md, block 1, stage 2): the home it belongs to — one for every
//! gateway of a family, with a shared key — and itself, with an id and a name of its own. Both are made at the first
//! start when missing (`Identity::ensure`). The home travels with the settings package (src/placement/transfer.rs), so a
//! station whose settings the computer keeps joins the computer's home with the first package, while each gateway keeps
//! its own id and name. The home's key is never shown: /health says the home's id only. «Код дома» — the key in words —
//! comes with «Сейф» (block 2).
//!
//! Unlike the rest of the core, `Identity` keeps state — the settings, the secrets, the household's list of stations —
//! at the place the plan gives it; the views and the constants beside it are pure.

use std::sync::Arc;

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::core::build::Profile;
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::helpers::contract::HelperIdentity;
use crate::platform::http::ApiError;
use crate::secrets::SecretsService;
use crate::settings::definitions::{GatewaySelf, HomeIdentity, GATEWAY_SELF, HOME_IDENTITY};
use crate::settings::SettingsService;
use crate::stations::looks::{look_of, Chosen, Reported};
use crate::storage::Db;
use crate::utils::ids::{random_hex, uuid};
use crate::utils::time::now_iso;

/// The secret that holds the home's key. It travels with the settings package (`transfer::TRAVELLING_SECRETS`).
pub const HOME_KEY: &str = "home:key";
/// The home's key: this many random bytes, kept as hex (the secrets hold text).
pub const HOME_KEY_BYTES: usize = 32;
/// A person's name for a gateway: at most this many characters.
pub const NAME_MAX: usize = 40;
/// The kind of the state table's row that marks this gateway's own id beside `gateway_self` (`Identity::ensure`); its
/// device is empty. No settings package carries it (of the state table only the apps' schedules travel): a setting
/// brought by another gateway's package is told apart by it.
const OWN_ID: &str = "gateway-self";
/// The kind of the state table's row that marks the home id this gateway's key belongs to: `{home, key}` — the home's
/// id and the SHA-256 of `home:key` as hex. No package carries it either: a home id that a release before the block
/// wrote over (`ensure`) is told apart from the key's own.
const OWN_HOME: &str = "home-of-key";

/// The home id a key was marked with, and the key's hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
struct HomeMark {
    home: String,
    key: String,
}

/// A home key's mark: its SHA-256 as hex — the key itself is kept by the secrets alone.
fn key_hash(key: &str) -> String {
    hex::encode(crate::helpers::link::sha256(key.trim().as_bytes()))
}

/// What a gateway runs on, for its default name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Station,
    Computer,
}

impl DeviceKind {
    /// The device a gateway of this profile runs on: the station's build, or a computer's playing it, on a station.
    pub fn of(profile: Profile) -> DeviceKind {
        if profile.station {
            DeviceKind::Station
        } else {
            DeviceKind::Computer
        }
    }
}

/// The home as /health and the panels see it: its id, never its key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HomeView {
    pub id: String,
}

/// This gateway as /health says it: its own id and the name people see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GatewayHealth {
    pub id: String,
    pub name: String,
}

/// This gateway as the panel's rename reads it (`GET|PUT /api/v1/gateway/self`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayView {
    pub id: String,
    /// The name people see: a person's, else the default.
    pub name: String,
    /// What it is called while nobody names it: the hint in the panel's empty field.
    pub default_name: String,
    /// A person named it.
    pub named: bool,
    /// The home it is in; null while it has none.
    pub home: Option<HomeView>,
}

/// Who this gateway is: its home (the setting `home_identity` and the secret `home:key`) and itself (the setting
/// `gateway_self`, each gateway's own).
pub struct Identity {
    settings: Arc<SettingsService>,
    secrets: Arc<SecretsService>,
    db: Arc<Db>,
    kind: DeviceKind,
    events: EventBus,
}

impl Identity {
    pub fn new(
        settings: Arc<SettingsService>,
        secrets: Arc<SecretsService>,
        db: Arc<Db>,
        kind: DeviceKind,
        events: EventBus,
    ) -> Identity {
        Identity { settings, secrets, db, kind, events }
    }

    /// Makes what is missing, at the start — after the package the other gateway left was applied (app.rs), which may
    /// have brought the home. A failed read is an error, never «missing»: it makes no second home.
    ///
    /// - No home, or a stored one that does not read as one: a new home, its key first — a home is never kept without
    ///   its key.
    /// - A home without its key here (its `secrets.key` lost, a package from a writer that lost it, a move to a 0.11
    ///   station that kept `home_identity` and passed the `home:` secret by): a new home, a new key and a new id. A home
    ///   id without its key is no home: two gateways must never hold one home id with two keys — the helpers prove the
    ///   home with it (stage 7) and «Сейф» is sealed with it (block 2). While the settings live on the computer, its
    ///   next package brings its home back.
    /// - A home whose id is not the one its key was marked with (`OWN_HOME`): another gateway's home id, brought by a
    ///   package of a release that knows nothing of it — a station put back on 0.13.0 takes the computer's settings
    ///   whole, `home_identity` among them, while its own `home:key` stays (such a package carries no `home:` secret).
    ///   The key's own id comes back; the other's name goes. Two gateways must never hold one home id with two keys.
    ///   A key that is not the marked one came with its id (a package of the block brings both): marked anew.
    /// - No record of this gateway: a new id, no name — or its own id back, when its mark (`OWN_ID`) keeps one.
    /// - A record that is not its own: another gateway's id, brought by a package of a release that knows nothing of it
    ///   (0.13.0 and earlier send every setting they do not know, `gateway_self` among them — a computer put back on
    ///   0.13.0 that moves to a station). Its own id comes back from its mark, the other's name with it gone; without a
    ///   mark, a record that came with a home without its key is taken as such too: a new id. Two gateways with one id
    ///   would each take the other for itself, and the station would pass its computer by.
    pub fn ensure(&self) -> Result<(), ApiError> {
        let mut keyless = false;
        match self.settings.peek(&HOME_IDENTITY)? {
            None => self.new_home()?,
            Some(home) if !self.secrets.has(HOME_KEY) => {
                tracing::warn!(
                    "identity: the home {} has no key here: a home of its own until the owner's comes",
                    home.id
                );
                keyless = true;
                self.new_home()?;
            }
            Some(home) => self.keep_home_of_key(&home)?,
        }
        let own = self.own_id()?;
        match (self.settings.peek(&GATEWAY_SELF)?, own) {
            (Some(me), Some(own)) if me.id == own => {}
            (Some(me), Some(own)) => {
                tracing::warn!(
                    "identity: {} is another gateway's id, brought by a package: this gateway is {own} again",
                    me.id
                );
                self.settings.set(&GATEWAY_SELF, &GatewaySelf { id: own, name: None }, None)?;
            }
            (None, Some(own)) => {
                tracing::info!("identity: this gateway's record is gone: it is {own} again");
                self.settings.set(&GATEWAY_SELF, &GatewaySelf { id: own, name: None }, None)?;
            }
            (Some(me), None) if keyless => {
                tracing::warn!(
                    "identity: {} came with a home without its key: another gateway's id, a new one here",
                    me.id
                );
                self.new_self()?;
            }
            // Made by this line before the mark: its own.
            (Some(me), None) => self.keep_own_id(&me.id)?,
            (None, None) => self.new_self()?,
        }
        Ok(())
    }

    /// A new id for this gateway, its mark with it.
    fn new_self(&self) -> Result<(), ApiError> {
        let me = GatewaySelf { id: uuid(), name: None };
        self.keep_own_id(&me.id)?;
        self.settings.set(&GATEWAY_SELF, &me, None)?;
        tracing::info!("identity: this gateway is {}", me.id);
        Ok(())
    }

    /// This gateway's own id as its mark keeps it: a row of the state table that no release's settings package carries
    /// (src/placement/transfer.rs). None — no mark yet.
    fn own_id(&self) -> Result<Option<String>, ApiError> {
        let stored: Option<String> = self.db.with(|c| {
            c.query_row("SELECT items FROM state WHERE kind = ?1 AND device = ''", params![OWN_ID], |row| row.get(0))
                .optional()
        })?;
        Ok(stored.and_then(|text| serde_json::from_str::<String>(&text).ok()).filter(|id| !id.is_empty()))
    }

    fn keep_own_id(&self, id: &str) -> Result<(), ApiError> {
        let text = serde_json::to_string(id).unwrap_or_default();
        self.db.with(|c| {
            c.execute(
                "INSERT INTO state (kind, device, items, updated_at) VALUES (?1, '', ?2, ?3)
                 ON CONFLICT (kind, device) DO UPDATE SET items = excluded.items, updated_at = excluded.updated_at",
                params![OWN_ID, text, now_iso()],
            )
        })?;
        Ok(())
    }

    /// A new home: its key first, then its id — a home is never stored without its key —, then their mark.
    fn new_home(&self) -> Result<(), ApiError> {
        let key = random_hex(HOME_KEY_BYTES);
        self.secrets.set(HOME_KEY, Some(&key))?;
        let home = HomeIdentity { id: uuid(), name: None };
        self.settings.set(&HOME_IDENTITY, &home, None)?;
        self.mark_home(&home.id, &key)?;
        tracing::info!("identity: a new home, {}", home.id);
        Ok(())
    }

    /// The stored home against its key's mark: the key's own id back when another's came without its key; else the
    /// pair as it is now, marked.
    fn keep_home_of_key(&self, home: &HomeIdentity) -> Result<(), ApiError> {
        let Some(key) = self.secrets.get(HOME_KEY) else { return Ok(()) };
        let hash = key_hash(&key);
        match self.home_mark()? {
            Some(mark) if mark.key == hash && mark.home == home.id => Ok(()),
            Some(mark) if mark.key == hash => {
                tracing::warn!(
                    "identity: {} is another home's id, brought without its key by a package: this home is {} again",
                    home.id,
                    mark.home
                );
                self.settings.set(&HOME_IDENTITY, &HomeIdentity { id: mark.home, name: None }, None)?;
                Ok(())
            }
            _ => self.mark_home(&home.id, &key),
        }
    }

    fn home_mark(&self) -> Result<Option<HomeMark>, ApiError> {
        let stored: Option<String> = self.db.with(|c| {
            c.query_row("SELECT items FROM state WHERE kind = ?1 AND device = ''", params![OWN_HOME], |row| row.get(0))
                .optional()
        })?;
        Ok(stored.and_then(|text| serde_json::from_str::<HomeMark>(&text).ok()).filter(|mark| !mark.home.is_empty()))
    }

    fn mark_home(&self, home: &str, key: &str) -> Result<(), ApiError> {
        let text = serde_json::to_string(&HomeMark { home: home.to_string(), key: key_hash(key) }).unwrap_or_default();
        self.db.with(|c| {
            c.execute(
                "INSERT INTO state (kind, device, items, updated_at) VALUES (?1, '', ?2, ?3)
                 ON CONFLICT (kind, device) DO UPDATE SET items = excluded.items, updated_at = excluded.updated_at",
                params![OWN_HOME, text, now_iso()],
            )
        })?;
        Ok(())
    }

    /// The home this gateway is in; none while it has none (its start could not make it).
    pub fn home(&self) -> Option<HomeView> {
        self.settings.get(&HOME_IDENTITY, None).map(|home| HomeView { id: home.id })
    }

    /// The home's name: the one the household gave, else «Дом». Not shown in block 1; a helper hears it at pairing.
    pub fn home_name(&self) -> String {
        self.settings
            .get(&HOME_IDENTITY, None)
            .and_then(|home| home.name)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| crate::tr!("Дом"))
    }

    /// This gateway as a helper names itself, and as it names itself to its helpers: its own id and the name people
    /// see. No record yet — an empty id, read as unknown, and its default name.
    pub fn as_helper(&self) -> HelperIdentity {
        match self.gateway() {
            Some(me) => HelperIdentity { id: me.id, name: me.name },
            None => HelperIdentity { id: String::new(), name: self.default_name() },
        }
    }

    /// This gateway as /health says it; none while it has no record.
    pub fn gateway(&self) -> Option<GatewayHealth> {
        let me = self.settings.get(&GATEWAY_SELF, None)?;
        let name = me.name.filter(|name| !name.trim().is_empty()).unwrap_or_else(|| self.default_name());
        Some(GatewayHealth { id: me.id, name })
    }

    /// This gateway as the panel's rename reads it.
    pub fn view(&self) -> Result<GatewayView, ApiError> {
        let me = self.me()?;
        let default_name = self.default_name();
        let given = me.name.filter(|name| !name.trim().is_empty());
        Ok(GatewayView {
            id: me.id,
            named: given.is_some(),
            name: given.unwrap_or_else(|| default_name.clone()),
            default_name,
            home: self.home(),
        })
    }

    /// A person's name for this gateway (`clean_name`), at most `NAME_MAX` characters; empty or none — the default
    /// again.
    /// The panels hear of it (`gateway.state`).
    pub fn rename(&self, name: Option<&str>, actor: Option<Actor>) -> Result<GatewayView, ApiError> {
        let name = name.map(clean_name).filter(|name| !name.is_empty());
        if name.as_ref().is_some_and(|name| name.chars().count() > NAME_MAX) {
            return Err(ApiError::bad_request(crate::tr!("Имя шлюза — не длиннее {max} знаков.", max = NAME_MAX)));
        }
        let me = GatewaySelf { name, ..self.me()? };
        self.settings.set(&GATEWAY_SELF, &me, actor.clone())?;
        self.events.emit(GatewayEvent::GatewayStateChanged { what: "name".into(), actor });
        self.view()
    }

    /// The name nobody gave, in the reader's language: on a station the household's name of it (a move brings it, the
    /// list of stations renames it), else «Колонка» and its model as its own sync told it, else «Колонка»; on a computer
    /// «Компьютер» and its system's family. Worked out when read, never kept: what is learned later — the station's first
    /// sync, a new name in the household's list — shows at once.
    pub fn default_name(&self) -> String {
        match self.kind {
            DeviceKind::Station => {
                use crate::placement::service::{station_self, DEFAULT_STATION_NAME};
                let (device, name) = station_self(&self.settings, &self.db);
                if let Some(name) = name.filter(|name| name != DEFAULT_STATION_NAME) {
                    return name;
                }
                match station_model(&self.db, device.as_deref()) {
                    Some(model) => crate::tr!("Колонка {model}", model = model),
                    None => crate::tr!("Колонка"),
                }
            }
            DeviceKind::Computer => match computer_model() {
                Some(model) => crate::tr!("Компьютер {model}", model = model),
                None => crate::tr!("Компьютер"),
            },
        }
    }

    /// This gateway's record; made now when its start could not make it.
    fn me(&self) -> Result<GatewaySelf, ApiError> {
        if let Some(me) = self.settings.peek(&GATEWAY_SELF)? {
            return Ok(me);
        }
        self.ensure()?;
        self.settings.peek(&GATEWAY_SELF)?.ok_or_else(|| ApiError::internal("identity: gateway_self is not kept"))
    }
}

/// A name as a person typed it, to keep or to send: control characters as spaces, the bidi format characters (which
/// turn the text around another's name, U+200E, U+200F, U+202A–U+202E, U+2066–U+2069) out, the spaces around it out
/// and those inside one.
pub fn clean_name(name: &str) -> String {
    let plain: String =
        name.chars().filter(|c| !bidi_format(*c)).map(|c| if c.is_control() { ' ' } else { c }).collect();
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn bidi_format(c: char) -> bool {
    matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// The family of a station's model as its own sync told it, in the reader's language («Лайт 2», "Lite 2"): the row of
/// its device; its device not known (a gateway put on the station by hand), the only station with a model. None — not
/// told yet: the station's first sync comes after the gateway's first start. A known device never takes another
/// station's model: the household's list travels whole, the other stations' rows with it.
fn station_model(db: &Db, device: Option<&str>) -> Option<String> {
    let told: Vec<(String, String)> = db
        .with(|connection| {
            connection
                .prepare("SELECT device, device_model FROM stations WHERE coalesce(device_model, '') != ''")?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .unwrap_or_default();
    let (_, code) = match device {
        Some(device) => told.iter().find(|(own, _)| own == device),
        None => (told.len() == 1).then(|| &told[0]),
    }?;
    let reported = Reported { device_model: Some(code.as_str()), device_color: None };
    look_of(reported, Chosen::default()).map(|look| look.title.to_string())
}

/// The words of a computer's model: on Linux the product's name its firmware says (`product_name`), else its
/// system's family — never its host name, which often carries a person's name; macOS gives only an identifier such as
/// `Mac14,2`, which tells a person nothing.
fn computer_model() -> Option<String> {
    match std::env::consts::OS {
        "macos" => Some("Mac".into()),
        "windows" => Some("Windows".into()),
        "linux" => {
            let firmware = std::fs::read_to_string("/sys/devices/virtual/dmi/id/product_name").ok();
            Some(firmware.as_deref().and_then(product_name).unwrap_or_else(|| "Linux".into()))
        }
        _ => None,
    }
}

/// The product's name as a computer's firmware says it (Linux's DMI `product_name`), when it is a name: trimmed, at
/// most `NAME_MAX` characters, never one of the placeholders boards ship with.
fn product_name(text: &str) -> Option<String> {
    const PLACEHOLDERS: [&str; 7] = [
        "to be filled by o.e.m.",
        "system product name",
        "default string",
        "not applicable",
        "not specified",
        "none",
        "o.e.m.",
    ];
    let name = clean_name(text);
    let lower = name.to_lowercase();
    let placeholder = PLACEHOLDERS.contains(&lower.as_str()) || lower.chars().all(|c| !c.is_alphanumeric());
    (!name.is_empty() && !placeholder && name.chars().count() <= NAME_MAX).then_some(name)
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use super::*;
    use crate::settings::definitions::{StationSelf, STATION_SELF};

    fn identity(kind: DeviceKind) -> Identity {
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let secrets = Arc::new(SecretsService::new(db.clone(), None).unwrap());
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        Identity::new(settings, secrets, db, kind, events)
    }

    fn uuid_v4(text: &str) -> bool {
        uuid::Uuid::parse_str(text).is_ok_and(|id| id.get_version_num() == 4)
    }

    fn home_key(identity: &Identity) -> String {
        identity.secrets.get(HOME_KEY).expect("the home's key")
    }

    #[test]
    fn a_new_gateway_makes_its_home_its_key_and_its_own_id_once() {
        let identity = identity(DeviceKind::Computer);
        assert_eq!((identity.home(), identity.gateway()), (None, None), "nothing before the start");
        identity.ensure().unwrap();
        let (home, me) = (identity.home().unwrap(), identity.gateway().unwrap());
        assert!(uuid_v4(&home.id) && uuid_v4(&me.id) && home.id != me.id, "{home:?} {me:?}");
        let key = home_key(&identity);
        assert!(key.len() == 2 * HOME_KEY_BYTES && key.chars().all(|c| c.is_ascii_hexdigit()), "32 bytes as hex");
        assert_eq!(identity.settings.raw("gateway_self"), Some(serde_json::json!({ "id": me.id })), "no name kept");

        identity.ensure().unwrap();
        assert_eq!((identity.home().unwrap(), identity.gateway().unwrap().id), (home, me.id), "made once");
        assert_eq!(home_key(&identity), key);
    }

    /// A home id without its key is no home (review, round two): two gateways must never hold one home id with two
    /// keys. The gateway makes a home of its own, a new id with a new key, and stays itself.
    #[test]
    fn a_home_without_its_key_becomes_a_new_home() {
        let identity = identity(DeviceKind::Computer);
        identity.ensure().unwrap();
        let (home, key, me) = (identity.home().unwrap(), home_key(&identity), identity.gateway().unwrap());
        identity.secrets.delete(HOME_KEY).unwrap();
        identity.ensure().unwrap();
        let (new_home, new_key) = (identity.home().unwrap(), home_key(&identity));
        assert!(new_home != home && uuid_v4(&new_home.id), "a new home: {new_home:?}");
        assert!(new_key.len() == 2 * HOME_KEY_BYTES && new_key != key, "a key of its own");
        assert_eq!(identity.gateway().unwrap().id, me.id, "this gateway stays itself");
        identity.ensure().unwrap();
        assert_eq!((identity.home().unwrap(), home_key(&identity)), (new_home, new_key), "made once");
    }

    #[test]
    fn a_lost_home_makes_a_new_home_with_a_new_key() {
        let identity = identity(DeviceKind::Computer);
        identity.ensure().unwrap();
        let (home, key, me) = (identity.home().unwrap(), home_key(&identity), identity.gateway().unwrap());
        identity.settings.forget(&HOME_IDENTITY).unwrap();
        identity.ensure().unwrap();
        let (new_home, new_key) = (identity.home().unwrap(), home_key(&identity));
        assert!(new_home != home && new_key != key, "a new home, a key of its own");
        assert_eq!(identity.gateway().unwrap(), me, "this gateway stays itself");

        // A stored record that does not read as a home is replaced the same way.
        identity
            .db
            .with(|c| c.execute("UPDATE settings SET value = '\"broken\"' WHERE key = 'home_identity'", []))
            .unwrap();
        identity.ensure().unwrap();
        assert!(identity.home().is_some_and(|home| home != new_home) && home_key(&identity) != new_key);
    }

    /// A read that fails is not a home missing: nothing is made, and the next start finds the home it had.
    #[test]
    fn a_failed_read_makes_no_second_home() {
        let identity = identity(DeviceKind::Computer);
        identity.ensure().unwrap();
        let (home, key) = (identity.home().unwrap(), home_key(&identity));
        identity.db.with(|c| c.execute("ALTER TABLE settings RENAME TO settings_away", [])).unwrap();
        assert!(identity.ensure().is_err());
        identity.db.with(|c| c.execute("ALTER TABLE settings_away RENAME TO settings", [])).unwrap();
        identity.ensure().unwrap();
        assert_eq!((identity.home().unwrap(), home_key(&identity)), (home, key));
    }

    /// A release before the block sends every setting it does not know: a computer put back on 0.13.0 that moves to a
    /// station gives it `gateway_self` (review of the merge with main, 16.11). The id a package brought gives way to the
    /// gateway's own, which its mark in the state table keeps (a table no package carries); a record gone comes back.
    #[test]
    fn another_gateways_id_from_an_old_package_gives_way_to_its_own() {
        let identity = identity(DeviceKind::Station);
        identity.ensure().unwrap();
        let me = identity.gateway().unwrap().id;
        let theirs = GatewaySelf { id: "gateway-of-the-computer".into(), name: Some("Компьютер Mac".into()) };
        identity.settings.set(&GATEWAY_SELF, &theirs, None).unwrap();
        identity.ensure().unwrap();
        assert_eq!(identity.settings.raw("gateway_self"), Some(serde_json::json!({ "id": me })), "its own, no name");
        identity.settings.forget(&GATEWAY_SELF).unwrap();
        identity.ensure().unwrap();
        assert_eq!(identity.gateway().unwrap().id, me, "a record gone comes back");
        identity.rename(Some("Кухня"), None).unwrap();
        identity.ensure().unwrap();
        assert_eq!(identity.gateway().unwrap().name, "Кухня", "its own record is kept as it is");
    }

    /// A station of the block put back on 0.13.0 took the computer's settings whole: the computer's home id, while its
    /// own home key stayed (no such package carries `home:`). Back on the block, the key's own home id comes back. A
    /// package of the block brings a home with its key: that pair is kept and marked anew.
    #[test]
    fn another_homes_id_without_its_key_gives_way_to_the_keys_own() {
        let station = identity(DeviceKind::Station);
        station.ensure().unwrap();
        let (own, key) = (station.home().unwrap().id, home_key(&station));
        let theirs = HomeIdentity { id: "home-of-the-computer".into(), name: Some("Квартира".into()) };
        station.settings.set(&HOME_IDENTITY, &theirs, None).unwrap();
        station.ensure().unwrap();
        assert_eq!(station.home().unwrap().id, own, "the key's own home again");
        assert_eq!(home_key(&station), key, "the key is kept");
        assert_eq!(
            station.settings.raw("home_identity"),
            Some(serde_json::json!({ "id": own })),
            "the other's name gone"
        );

        // The computer's home with its key, as a package of the block brings it.
        station.secrets.set(HOME_KEY, Some("ab".repeat(32).as_str())).unwrap();
        station.settings.set(&HOME_IDENTITY, &theirs, None).unwrap();
        station.ensure().unwrap();
        assert_eq!(station.home().unwrap().id, "home-of-the-computer", "a home that came with its key stays");
        station.settings.set(&HOME_IDENTITY, &HomeIdentity { id: own.clone(), name: None }, None).unwrap();
        station.ensure().unwrap();
        assert_eq!(station.home().unwrap().id, "home-of-the-computer", "marked anew with that key");

        // A gateway of the block from before the mark: its pair as it is, marked.
        let earlier = identity(DeviceKind::Computer);
        earlier.ensure().unwrap();
        let home = earlier.home().unwrap().id;
        earlier.db.with(|c| c.execute("DELETE FROM state WHERE kind = ?1", params![OWN_HOME])).unwrap();
        earlier.ensure().unwrap();
        earlier.settings.set(&HOME_IDENTITY, &theirs, None).unwrap();
        earlier.ensure().unwrap();
        assert_eq!(earlier.home().unwrap().id, home);
    }

    /// No mark yet. A station that never ran the block took the computer's package from 0.13.0: the computer's id with
    /// the computer's home, whose key no such package carries — another gateway's id, a new one here. A gateway of this
    /// line from before the mark, its home with its key, keeps its id and is marked.
    #[test]
    fn without_a_mark_an_id_that_came_with_a_keyless_home_is_anothers() {
        let station = identity(DeviceKind::Station);
        let home = HomeIdentity { id: "home-of-the-computer".into(), name: None };
        station.settings.set(&HOME_IDENTITY, &home, None).unwrap();
        let theirs = GatewaySelf { id: "gateway-of-the-computer".into(), name: Some("Компьютер Mac".into()) };
        station.settings.set(&GATEWAY_SELF, &theirs, None).unwrap();
        station.ensure().unwrap();
        let me = station.gateway().unwrap();
        assert!(uuid_v4(&me.id) && me.name == "Колонка", "a new id, the default name: {me:?}");
        assert_ne!(station.home().unwrap().id, "home-of-the-computer", "a home of its own until the owner's comes");
        station.ensure().unwrap();
        assert_eq!(station.gateway().unwrap().id, me.id, "made once");

        let earlier = identity(DeviceKind::Computer);
        earlier.ensure().unwrap();
        let own = GatewaySelf { id: "made-before-the-mark".into(), name: Some("Кабинет".into()) };
        earlier.settings.set(&GATEWAY_SELF, &own, None).unwrap();
        earlier.db.with(|c| c.execute("DELETE FROM state WHERE kind = ?1", params![OWN_ID])).unwrap();
        earlier.ensure().unwrap();
        assert_eq!(earlier.gateway().unwrap(), GatewayHealth { id: own.id.clone(), name: "Кабинет".into() });
        earlier.settings.set(&GATEWAY_SELF, &theirs, None).unwrap();
        earlier.ensure().unwrap();
        assert_eq!(earlier.gateway().unwrap().id, own.id, "marked now");
    }

    #[test]
    fn the_default_name_is_the_stations_household_name_else_its_model_else_its_kind() {
        let station = identity(DeviceKind::Station);
        station.ensure().unwrap();
        assert_eq!(station.default_name(), "Колонка", "nothing known at the first start");
        // The station's own sync told its model: its family's name, at once — the default is never kept.
        station
            .db
            .with(|c| {
                c.execute(
                    "INSERT INTO stations (device, name, status, first_seen, last_seen, device_model)
                     VALUES ('h:kids', 'Колонка', 'approved', 'then', 'then', 'plum')",
                    [],
                )
            })
            .unwrap();
        assert_eq!(station.gateway().unwrap().name, "Колонка Лайт 2");
        // A move brought the household's name of it — «Колонка» is no name.
        let brought = |name: &str| StationSelf { device: Some("h:kids".into()), name: Some(name.into()) };
        station.settings.set(&STATION_SELF, &brought("Колонка"), None).unwrap();
        assert_eq!(station.default_name(), "Колонка Лайт 2");
        station.settings.set(&STATION_SELF, &brought("Детская"), None).unwrap();
        assert_eq!(station.default_name(), "Детская");
        // Renamed in the household's list since: that name.
        station.db.with(|c| c.execute("UPDATE stations SET name = 'Спальня'", [])).unwrap();
        assert_eq!(station.default_name(), "Спальня");

        let computer = identity(DeviceKind::Computer);
        let family = if cfg!(target_os = "macos") {
            " Mac"
        } else if cfg!(target_os = "windows") {
            " Windows"
        } else if cfg!(target_os = "linux") {
            " Linux"
        } else {
            ""
        };
        if !cfg!(target_os = "linux") {
            assert_eq!(computer.default_name(), format!("Компьютер{family}"));
        }
        // On Linux the firmware's product name, when it is a name; a board's placeholder is none.
        assert_eq!(product_name("ThinkPad X1 Carbon Gen 11\n").as_deref(), Some("ThinkPad X1 Carbon Gen 11"));
        assert_eq!(product_name("To Be Filled By O.E.M."), None);
        assert_eq!(product_name("System Product Name"), None);
        assert_eq!(product_name(" Default string "), None);
        assert_eq!(product_name("  \n"), None);
        assert_eq!(product_name(&"Ж".repeat(NAME_MAX + 1)), None, "too long to be a name");
    }

    /// The household's list travels whole: a station whose device is known never takes another station's model for its
    /// name (review, round one) — «Колонка» until its own sync tells its own.
    #[test]
    fn a_known_station_without_a_model_never_takes_anothers() {
        let station = identity(DeviceKind::Station);
        station.ensure().unwrap();
        station
            .db
            .with(|c| {
                c.execute_batch(
                    "INSERT INTO stations (device, name, status, first_seen, last_seen, device_model)
                     VALUES ('h:hall', 'Колонка', 'approved', 'then', 'then', 'plum');
                     INSERT INTO stations (device, name, status, first_seen, last_seen)
                     VALUES ('h:kids', 'Колонка', 'approved', 'then', 'then');",
                )
            })
            .unwrap();
        let own = StationSelf { device: Some("h:kids".into()), name: None };
        station.settings.set(&STATION_SELF, &own, None).unwrap();
        assert_eq!(station.default_name(), "Колонка", "the hall's model is not its");
        station
            .db
            .with(|c| c.execute("UPDATE stations SET device_model = 'yandexmini' WHERE device = 'h:kids'", []))
            .unwrap();
        assert_eq!(station.default_name(), "Колонка Мини");
    }

    #[test]
    fn a_persons_name_wins_and_an_empty_one_brings_the_default_back() {
        let identity = identity(DeviceKind::Station);
        identity.ensure().unwrap();
        let mut heard = identity.events.subscribe();
        let view = identity.rename(Some("  Кухня\tу окна \n"), None).unwrap();
        assert_eq!((view.name.as_str(), view.default_name.as_str(), view.named), ("Кухня у окна", "Колонка", true));
        assert_eq!(
            (identity.gateway().unwrap().name, view.home.clone()),
            ("Кухня у окна".to_string(), identity.home())
        );
        let mut names = Vec::new();
        while let Ok(event) = heard.try_recv() {
            names.push(event.name());
        }
        assert_eq!(names, ["settings.changed", "gateway.state"], "the panels read it again");

        let back = identity.rename(Some(" \n"), None).unwrap();
        assert_eq!((back.name.as_str(), back.named, &back.id), ("Колонка", false, &view.id), "the same gateway");
        identity.rename(Some("Кухня"), None).unwrap();
        assert!(!identity.rename(None, None).unwrap().named);
        assert_eq!(identity.settings.raw("gateway_self"), Some(serde_json::json!({ "id": view.id })));

        let error = identity.rename(Some(&"я".repeat(NAME_MAX + 1)), None).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(identity.gateway().unwrap().name, "Колонка", "nothing kept");
        assert_eq!(identity.rename(Some(&"я".repeat(NAME_MAX)), None).unwrap().name.chars().count(), NAME_MAX);
        // A character that turns the text around (U+202E) never comes into a name.
        let turned = identity.rename(Some("Кухня\u{202E}\u{2066}тёмная\u{200F}"), None).unwrap();
        assert_eq!(turned.name, "Кухнятёмная");
    }

    #[test]
    fn this_gateway_names_itself_to_its_helpers_by_its_id_and_the_name_people_see() {
        let identity = identity(DeviceKind::Computer);
        let computer = |name: &str| name.starts_with("Компьютер") || name.starts_with("Computer");
        let unknown = identity.as_helper();
        assert!(unknown.id.is_empty() && computer(&unknown.name), "no record yet: {unknown:?}");
        identity.ensure().unwrap();
        let me = identity.as_helper();
        assert_eq!(me.id, identity.gateway().unwrap().id);
        identity.rename(Some("Кабинет"), None).unwrap();
        assert_eq!(identity.as_helper(), HelperIdentity { id: me.id.clone(), name: "Кабинет".into() });
        // The home's name: the household's, else «Дом».
        assert!(["Дом", "Home"].contains(&identity.home_name().as_str()), "{}", identity.home_name());
        let id = identity.home().unwrap().id;
        identity.settings.set(&HOME_IDENTITY, &HomeIdentity { id, name: Some("Дача".into()) }, None).unwrap();
        assert_eq!(identity.home_name(), "Дача");
    }
}
