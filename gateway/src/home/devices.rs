//! The devices of the home as its centre sees them (docs/plans/home-and-devices.md, stage 1): the centre itself, the
//! stations that hear for it (ears), and its Turbo in their order. Gathered from what the gateway already keeps — its
//! own identity and placement, the stations and their presence, the helpers' list — nothing new is stored.
//!
//! `GET /api/v1/home/devices` → `{home: {id, name}, centre, devices: [...]}`; the centre first, then the ears, then the
//! Turbo by their order. Each device: `id`, `kind` (`station`, `computer`, `server`, `cloud`), `name`, `roles`
//! (`centre`, `turbo`; none — an ear only), `hears` (has a microphone the home listens through), `state`
//! (`online`, `reconnecting`, `asleep` — a computer that sleeps, `offline`, `off` — switched off, `unknown`), `copy`
//! (keeps a copy of the home and can answer by itself; stage 3), and what is its own: a station's `device` and
//! `model`, a gateway's `gatewayId` (a computer finds itself in the list by it), a Turbo's `order`.
//!
//! `home.devices.changed` follows any change of these sources, once per short burst: the panel reads the list again.
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use serde::Serialize;

use crate::app::App;
use crate::core::events::{EventBus, GatewayEvent};
use crate::core::stations::presence::PresenceState;
use crate::helpers::{HelperKind, HelperState, HelperView};
use crate::platform::http::{ok, ApiResult, Caller};
use crate::stations::service::StationStatus;

/// What kind of thing a device is: its picture and its words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Station,
    Computer,
    Server,
    Cloud,
}

/// A role in the home. Hearing is not a role here: it is `hears`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceRole {
    Centre,
    Turbo,
}

/// How a device is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceState {
    Online,
    Reconnecting,
    /// A computer that is not answering: it sleeps or is away.
    Asleep,
    Offline,
    /// Switched off in the home's list.
    Off,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeDevice {
    pub id: String,
    pub kind: DeviceKind,
    pub name: String,
    pub roles: Vec<DeviceRole>,
    pub hears: bool,
    pub state: DeviceState,
    /// Keeps a copy of the home: answers by itself while the centre is away (stage 3).
    pub copy: bool,
    /// The centre that answers this request.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub itself: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
    /// A station's device id and model («plum»).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The gateway's own id (src/core/identity/home.rs): a computer finds itself in the list by it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_id: Option<String>,
    /// A Turbo's place in the order, from 1; and whether the home takes from it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// A Turbo the regulator «Что может уходить из дома» keeps out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept: Option<bool>,
    /// A Turbo that speaks an older contract than this centre (src/helpers/contract.rs): «обновите оба».
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub older: bool,
    /// A centre on a station: how its own voice client is (the station's voice to its own gateway) — one device, one
    /// row, «голос не на связи» on it when its voice went back to Yandex.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<DeviceState>,
}

impl HomeDevice {
    fn new(id: String, kind: DeviceKind, name: String) -> HomeDevice {
        HomeDevice {
            id,
            kind,
            name,
            roles: Vec::new(),
            hears: false,
            state: DeviceState::Unknown,
            copy: false,
            itself: false,
            since: None,
            last_seen: None,
            device: None,
            model: None,
            gateway_id: None,
            order: None,
            enabled: None,
            kept: None,
            older: false,
            voice: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HomeInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HomeDevices {
    pub home: HomeInfo,
    /// The centre's id in `devices`.
    pub centre: String,
    pub devices: Vec<HomeDevice>,
}

/// The centre as it knows itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Centre {
    /// A station's build: the centre is the station it runs on.
    pub on_station: bool,
    pub gateway_id: Option<String>,
    pub name: String,
    /// The station it runs on (a station's build).
    pub device: Option<String>,
    pub model: Option<String>,
}

/// A station from the list of stations.
#[derive(Debug, Clone, PartialEq)]
pub struct StationIn {
    pub device: String,
    pub name: String,
    pub status: StationStatus,
    pub model: Option<String>,
    pub presence: Option<PresenceState>,
    pub since: Option<String>,
    pub last_seen: Option<String>,
    /// Its voice goes to a gateway of its own, not here (a computer's record of a station that runs its own): it is
    /// another centre, not an ear of this one.
    pub elsewhere: bool,
    /// Its own gateway holds a copy of the home (an ear, stage 4).
    pub copy: bool,
    /// Its voice came over loopback: on a station's gateway, the station's own voice client.
    pub local: bool,
}

fn presence_state(presence: Option<PresenceState>) -> DeviceState {
    match presence {
        Some(PresenceState::Voice | PresenceState::Alive) => DeviceState::Online,
        Some(PresenceState::Reconnecting) => DeviceState::Reconnecting,
        Some(PresenceState::Offline) | None => DeviceState::Offline,
    }
}

/// The list, from what the gateway keeps. Pure: the route and the tests feed it.
pub fn gather(
    home: HomeInfo,
    centre: Centre,
    stations: &[StationIn],
    helpers: &[HelperView],
    copies: &HashSet<String>,
) -> HomeDevices {
    let mut centre = centre;
    // A centre on a station that does not know its own device (put there by the one-command install, which names
    // none): its station's voice client is the one that comes over loopback. Without this it was a second row — «Колонка ·
    // слышит · не на связи» beside «Колонка Мини 2 · Центр» (Windows test 21.11), one station read as two.
    let own = centre.on_station.then(|| match &centre.device {
        Some(device) => stations.iter().find(|station| &station.device == device),
        None => stations.iter().find(|station| station.local && station.status == StationStatus::Approved),
    });
    let own = own.flatten();
    if centre.device.is_none() {
        if let Some(own) = own {
            centre.device = Some(own.device.clone());
            centre.model = centre.model.or_else(|| own.model.clone());
        }
    }
    let centre_id = match (&centre.device, centre.on_station) {
        (Some(device), true) => format!("station:{device}"),
        _ => format!("gateway:{}", centre.gateway_id.clone().unwrap_or_default()),
    };
    let mut me = HomeDevice::new(
        centre_id.clone(),
        if centre.on_station { DeviceKind::Station } else { DeviceKind::Computer },
        centre.name.clone(),
    );
    me.roles.push(DeviceRole::Centre);
    // A station hears always; a computer that is the centre hears through its microphone (the panel's button, the
    // «Микрофон» mode).
    me.hears = true;
    me.state = DeviceState::Online;
    me.itself = true;
    me.device = centre.device.clone();
    me.model = centre.model.clone();
    me.gateway_id = centre.gateway_id.clone();
    me.voice = own.map(|own| presence_state(own.presence));
    // The computer's own built-in Turbo is the centre itself: one device, two roles.
    if !centre.on_station && helpers.iter().any(|helper| helper.builtin && helper.enabled) {
        me.roles.push(DeviceRole::Turbo);
    }
    let mut devices = vec![me];

    let mut seen: HashSet<String> = HashSet::new();
    for station in stations {
        if station.status != StationStatus::Approved || station.elsewhere {
            continue;
        }
        if centre.on_station && centre.device.as_deref() == Some(station.device.as_str()) {
            continue;
        }
        if !seen.insert(station.device.clone()) {
            continue;
        }
        let mut ear = HomeDevice::new(format!("station:{}", station.device), DeviceKind::Station, station.name.clone());
        ear.hears = true;
        ear.state = presence_state(station.presence);
        ear.since = station.since.clone();
        ear.copy = station.copy;
        ear.last_seen = station.last_seen.clone();
        ear.device = Some(station.device.clone());
        ear.model = station.model.clone();
        devices.push(ear);
    }

    let mut order = 0;
    for helper in helpers {
        // The computer's own Turbo is the centre itself; a second entry of the same helper answers once.
        if helper.builtin || helper.state == HelperState::Twin {
            continue;
        }
        order += 1;
        let kind = match helper.kind {
            HelperKind::Home => DeviceKind::Computer,
            HelperKind::Own => DeviceKind::Server,
            HelperKind::Cloud => DeviceKind::Cloud,
        };
        let name = helper
            .helper
            .as_ref()
            .map(|identity| identity.name.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| helper.name.clone());
        let mut turbo = HomeDevice::new(format!("turbo:{}", helper.id), kind, name);
        turbo.roles.push(DeviceRole::Turbo);
        // A computer at home hears through its microphone and talks to the centre (stage 1); a server or the cloud
        // has no microphone.
        turbo.hears = kind == DeviceKind::Computer;
        turbo.state = if !helper.enabled {
            DeviceState::Off
        } else {
            match helper.state {
                HelperState::Up | HelperState::Itself => DeviceState::Online,
                HelperState::Down if kind == DeviceKind::Computer => DeviceState::Asleep,
                HelperState::Down => DeviceState::Offline,
                HelperState::Off => DeviceState::Off,
                HelperState::Unknown | HelperState::Twin => DeviceState::Unknown,
            }
        };
        turbo.since = helper.since.clone();
        turbo.gateway_id = helper.helper.as_ref().map(|identity| identity.id.clone()).filter(|id| !id.is_empty());
        turbo.copy = turbo.gateway_id.as_deref().is_some_and(|id| copies.contains(id));
        turbo.order = Some(order);
        turbo.enabled = Some(helper.enabled);
        turbo.kept = (!helper.allowed).then_some(true);
        turbo.older = helper.contract.is_some_and(|version| version < crate::helpers::contract::VERSION);
        devices.push(turbo);
    }

    HomeDevices { home, centre: centre_id, devices }
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/home/devices", get(list))
}

/// Anyone signed in sees the home's devices: the line at the top of every panel shows them. A computer that is the
/// Turbo of a station's home has no home of its own: it asks its centre with the pair's key, and while the centre is
/// away it says what it knows — the centre by its record, itself as Turbo.
async fn list(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<serde_json::Value> {
    caller.signed_in()?;
    // An ear of the home's centre (stage 4): the home is the centre's.
    if let Some((ear, key)) = super::ear::centre(&app) {
        if let Ok(view) = super::ear::client(&ear, &key).get("/api/v1/home/devices").await {
            if view.get("devices").is_some() {
                return Ok(ok(view));
            }
        }
        return Ok(ok(serde_json::to_value(as_ear(&app, &ear)).unwrap_or_default()));
    }
    #[cfg(feature = "pc")]
    if let Some(centre) = centre_elsewhere(&app) {
        if let Some(client) = centre.client() {
            match client.get("/api/v1/home/devices").await {
                Ok(view) if view.get("devices").is_some() => return Ok(ok(view)),
                Ok(_) => tracing::warn!("home devices: the centre answered without a list"),
                Err(error) => tracing::debug!("home devices: the centre is away: {error}"),
            }
        }
        return Ok(ok(serde_json::to_value(as_turbo(&app, &centre)).unwrap_or_default()));
    }
    // The Turbo of a home whose centre is another computer (the link): that home, not one of its own.
    #[cfg(feature = "pc")]
    if let Some(home) = link_home(&app).and_then(|client| as_link_turbo(&app, &client)) {
        return Ok(ok(serde_json::to_value(home).unwrap_or_default()));
    }
    Ok(ok(serde_json::to_value(of_app(&app)).unwrap_or_default()))
}

/// A computer that helps another home over the link and keeps no station of its own (stage 7 of
/// docs/plans/home-setup-flow.md): that home's Turbo, not a centre with a Turbo of its own — «Компьютер · Центр ·
/// Турбо» on one device was the Windows test's puzzle (21.11). Its own home stays in its data, for «Это новый дом».
#[cfg(feature = "pc")]
fn link_home(app: &App) -> Option<crate::helpers::clients::ClientView> {
    if !app.link.helps() || !app.station_gateway.all().is_empty() || !app.stations.list().is_empty() {
        return None;
    }
    app.clients.view(&[]).into_iter().find(|client| client.via == "link" && !client.own_home && client.home.is_some())
}

/// The home this computer is the Turbo of over the link, as /health tells the panel (`placement.centre`, stage 7 of
/// docs/plans/home-setup-flow.md): the key of the pass to its centre (the client's id), the home's name, whether a link
/// to it is up now. Only once its centre said at a link that it takes this computer's own requests (`up`): with an older
/// centre — or one that does not count this computer as a Turbo at home — the panel keeps this computer's own desk.
#[cfg(feature = "pc")]
pub(crate) fn link_centre(app: &App) -> Option<serde_json::Value> {
    let client = link_home(app)?;
    if app.link.takes_up(&client.id) != Some(true) {
        return None;
    }
    let name = client.home.as_ref().map(|home| home.name.clone()).unwrap_or_default();
    let online = app.link.up_of(&client.id).is_some();
    Some(serde_json::json!({ "key": client.id, "name": name, "online": online }))
}

/// That home as its Turbo shows it: the centre by the home's name — its gateways online or not —, itself as Turbo.
#[cfg(feature = "pc")]
fn as_link_turbo(app: &App, client: &crate::helpers::clients::ClientView) -> Option<HomeDevices> {
    let home = client.home.clone()?;
    let id = format!("home:{}", home.id);
    let mut hub =
        HomeDevice::new(id.clone(), DeviceKind::Computer, crate::tr!("Хаб дома «{name}»", name = home.name.clone()));
    hub.roles.push(DeviceRole::Centre);
    hub.state =
        if client.gateways.iter().any(|gateway| gateway.online) { DeviceState::Online } else { DeviceState::Offline };
    let me = app.identity.gateway();
    let mut itself = HomeDevice::new(
        format!("gateway:{}", me.as_ref().map(|me| me.id.clone()).unwrap_or_default()),
        DeviceKind::Computer,
        me.as_ref().map(|me| me.name.clone()).unwrap_or_default(),
    );
    itself.roles.push(DeviceRole::Turbo);
    itself.state = DeviceState::Online;
    itself.itself = true;
    itself.gateway_id = me.map(|me| me.id);
    Some(HomeDevices { home: HomeInfo { id: Some(home.id), name: home.name }, centre: id, devices: vec![hub, itself] })
}

/// What an ear knows of its home while the centre is away: the centre by its address, itself with its copy.
fn as_ear(app: &App, ear: &super::ear::EarOf) -> HomeDevices {
    let id = format!("centre:{}", ear.address);
    let mut hub =
        HomeDevice::new(id.clone(), DeviceKind::Station, ear.name.clone().unwrap_or_else(|| ear.address.clone()));
    hub.roles.push(DeviceRole::Centre);
    hub.hears = true;
    hub.state = DeviceState::Offline;
    let (device, name) = app.placement.station_self();
    let me = app.identity.gateway();
    let mut itself = HomeDevice::new(
        device.as_ref().map(|device| format!("station:{device}")).unwrap_or_else(|| "station:this".into()),
        DeviceKind::Station,
        name.or_else(|| me.as_ref().map(|me| me.name.clone())).unwrap_or_default(),
    );
    itself.hears = true;
    itself.state = DeviceState::Online;
    itself.copy = super::copy::copied_revision(&app.settings).is_some();
    itself.device = device;
    itself.gateway_id = me.map(|me| me.id);
    HomeDevices {
        home: HomeInfo { id: app.identity.home().map(|home| home.id), name: app.identity.home_name() },
        centre: id,
        devices: vec![hub, itself],
    }
}

/// The station whose gateway is this home's centre while this computer is its Turbo: one with its own gateway (not
/// placed on this computer).
#[cfg(feature = "pc")]
pub(crate) fn centre_elsewhere(app: &App) -> Option<Arc<crate::placement::watch::StationGateway>> {
    use crate::core::availability::Placement;
    // Not an ear of it: a second station joined as the centre's ear has its own gateway and a record here too.
    let ear = |station: &crate::placement::watch::StationGateway| {
        station
            .live()
            .health
            .as_ref()
            .and_then(|health| health.pointer("/placement/earOf"))
            .is_some_and(|ear| !ear.is_null())
    };
    app.station_gateway
        .all()
        .into_iter()
        .find(|station| station.record().is_some_and(|record| record.placement != Placement::Pc) && !ear(station))
}

/// What a Turbo computer knows of its home while the centre is away: the centre by its record, then itself.
#[cfg(feature = "pc")]
fn as_turbo(app: &App, centre: &crate::placement::watch::StationGateway) -> HomeDevices {
    let record = centre.record();
    let device = record.as_ref().and_then(|record| record.device.clone());
    let live = centre.live();
    let id = device
        .as_ref()
        .map(|device| format!("station:{device}"))
        .unwrap_or_else(|| format!("station:{}", centre.key()));
    let mut hub =
        HomeDevice::new(id.clone(), DeviceKind::Station, centre.name().unwrap_or_else(|| crate::tr!("Колонка")));
    hub.roles.push(DeviceRole::Centre);
    hub.hears = true;
    hub.state = if live.reachable { DeviceState::Online } else { DeviceState::Offline };
    hub.last_seen = live.seen_at.clone();
    hub.device = device;
    let me = app.identity.gateway();
    let mut turbo = HomeDevice::new(
        format!("gateway:{}", me.as_ref().map(|me| me.id.as_str()).unwrap_or_default()),
        DeviceKind::Computer,
        me.as_ref().map(|me| me.name.clone()).unwrap_or_default(),
    );
    turbo.roles.push(DeviceRole::Turbo);
    turbo.hears = true;
    turbo.copy = super::copy::copied_revision(&app.settings).is_some();
    turbo.state = DeviceState::Online;
    turbo.gateway_id = me.map(|me| me.id);
    turbo.order = Some(1);
    turbo.enabled = Some(true);
    HomeDevices {
        home: HomeInfo { id: app.identity.home().map(|home| home.id), name: app.identity.home_name() },
        centre: id,
        devices: vec![hub, turbo],
    }
}

/// The list as this gateway keeps it now.
pub fn of_app(app: &App) -> HomeDevices {
    let home = HomeInfo { id: app.identity.home().map(|home| home.id), name: app.identity.home_name() };
    let me = app.identity.gateway();
    let on_station = app.placement.role() == crate::placement::service::Role::Station;
    let (device, station_name) = if on_station { app.placement.station_self() } else { (None, None) };
    let model = device
        .as_deref()
        .and_then(|device| app.stations.find(device))
        .and_then(|station| station.model.or(station.device_model));
    let centre = Centre {
        on_station,
        gateway_id: me.as_ref().map(|me| me.id.clone()),
        name: station_name.or_else(|| me.as_ref().map(|me| me.name.clone())).unwrap_or_else(|| home.name.clone()),
        device,
        model,
    };
    let elsewhere = elsewhere(app);
    let stations: Vec<StationIn> = app
        .stations
        .list()
        .into_iter()
        .map(|entry| {
            let view = entry.view;
            let presence = app.presence.of(&view.device);
            StationIn {
                elsewhere: elsewhere.contains(&view.device),
                copy: super::copy::device_holds(&app.settings, &view.device),
                device: view.device,
                name: view.name,
                status: view.status,
                model: view.model.or(view.device_model),
                since: presence.as_ref().map(|presence| presence.since.clone()),
                last_seen: presence.as_ref().and_then(|presence| presence.last_seen.clone()).or(Some(view.last_seen)),
                local: view.last_address.as_deref().is_some_and(is_loopback),
                presence: presence.map(|presence| presence.state),
            }
        })
        .collect();
    let copies: HashSet<String> = app
        .helpers
        .views()
        .iter()
        .filter_map(|view| view.helper.as_ref().map(|identity| identity.id.clone()))
        .filter(|id| super::copy::holds(&app.settings, id))
        .collect();
    gather(home, centre, &stations, &app.helpers.views(), &copies)
}

/// 127.0.0.1, ::1 or localhost: a voice that came from the gateway's own machine.
fn is_loopback(address: &str) -> bool {
    address == "localhost" || address.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The stations whose voice goes to a gateway of their own (a computer's records): they are not this centre's ears.
#[cfg(feature = "pc")]
fn elsewhere(app: &App) -> HashSet<String> {
    use crate::core::availability::Placement;
    app.station_gateway
        .all()
        .iter()
        .filter_map(|station| station.record())
        .filter(|record| record.placement != Placement::Pc)
        .filter_map(|record| record.device)
        .collect()
}

#[cfg(not(feature = "pc"))]
fn elsewhere(_app: &App) -> HashSet<String> {
    HashSet::new()
}

/// The events that move the list.
fn moves_devices(event: &GatewayEvent) -> bool {
    matches!(
        event.name(),
        "station.presence"
            | "station.updated"
            | "station.pending"
            | "helpers.changed"
            | "helpers.up"
            | "helpers.down"
            | "placement.changed"
            | "placement.moved"
            | "stationGateway.updated"
            | "gateway.state"
            | "identity.changed"
    )
}

/// How long a burst of changes is gathered into one `home.devices.changed`.
const BURST: Duration = Duration::from_millis(300);

/// Says `home.devices.changed` after the sources move, once per burst.
pub fn watch(events: &EventBus) {
    let pending = Arc::new(AtomicBool::new(false));
    let bus = events.clone();
    events.on("home.devices", move |event| {
        if !moves_devices(&event) || pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let pending = pending.clone();
        let bus = bus.clone();
        let reason = event.name().to_string();
        tokio::spawn(async move {
            tokio::time::sleep(BURST).await;
            pending.store(false, Ordering::SeqCst);
            bus.emit(GatewayEvent::HomeDevicesChanged { reason });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helpers::contract::HelperIdentity;

    fn home() -> HomeInfo {
        HomeInfo { id: Some("h1".into()), name: "Дом".into() }
    }

    fn station_centre() -> Centre {
        Centre {
            on_station: true,
            gateway_id: Some("g-station".into()),
            name: "Кухня".into(),
            device: Some("d-kitchen".into()),
            model: Some("plum".into()),
        }
    }

    fn station(device: &str, name: &str, presence: Option<PresenceState>) -> StationIn {
        StationIn {
            device: device.into(),
            name: name.into(),
            status: StationStatus::Approved,
            model: None,
            presence,
            since: None,
            last_seen: None,
            elsewhere: false,
            copy: false,
            local: false,
        }
    }

    fn helper(id: &str, kind: HelperKind, state: HelperState, enabled: bool, builtin: bool) -> HelperView {
        HelperView {
            id: id.into(),
            name: format!("name {id}"),
            kind,
            builtin,
            url: None,
            enabled,
            without: Vec::new(),
            key: None,
            state,
            since: None,
            error: None,
            allowed: true,
            contract: None,
            helper: Some(HelperIdentity { id: format!("g-{id}"), name: format!("Компьютер {id}") }),
            version: None,
            abilities: Vec::new(),
            link: None,
        }
    }

    #[test]
    fn a_station_centre_with_a_second_station_and_a_computer() {
        let stations = [
            station("d-kitchen", "Кухня", Some(PresenceState::Voice)),
            station("d-bed", "Спальня", Some(PresenceState::Reconnecting)),
        ];
        let helpers = [helper("mac", HelperKind::Home, HelperState::Down, true, false)];
        let list = gather(home(), station_centre(), &stations, &helpers, &HashSet::from(["g-mac".to_string()]));
        assert_eq!(list.centre, "station:d-kitchen");
        let ids: Vec<&str> = list.devices.iter().map(|device| device.id.as_str()).collect();
        assert_eq!(ids, ["station:d-kitchen", "station:d-bed", "turbo:mac"], "the centre once, then ears, then Turbo");
        assert_eq!(list.devices[0].roles, [DeviceRole::Centre]);
        assert!(list.devices[0].itself && list.devices[0].hears);
        assert!(list.devices[1].roles.is_empty() && list.devices[1].hears, "an ear: no role, hears");
        assert_eq!(list.devices[1].state, DeviceState::Reconnecting);
        let mac = &list.devices[2];
        assert_eq!((mac.kind, mac.state), (DeviceKind::Computer, DeviceState::Asleep), "a computer away sleeps");
        assert_eq!(mac.name, "Компьютер mac", "it is called as it calls itself");
        assert_eq!(mac.gateway_id.as_deref(), Some("g-mac"));
        assert_eq!(mac.order, Some(1));
        assert!(mac.copy, "it holds a copy of the home");
    }

    #[test]
    fn a_computer_alone_is_the_centre_and_its_own_turbo() {
        let centre = Centre {
            on_station: false,
            gateway_id: Some("g-pc".into()),
            name: "MacBook".into(),
            device: None,
            model: None,
        };
        let helpers = [
            helper("builtin", HelperKind::Home, HelperState::Itself, true, true),
            helper("srv", HelperKind::Own, HelperState::Down, true, false),
            helper("off", HelperKind::Own, HelperState::Up, false, false),
        ];
        let list = gather(home(), centre, &[], &helpers, &HashSet::new());
        assert_eq!(list.centre, "gateway:g-pc");
        assert_eq!(list.devices[0].roles, [DeviceRole::Centre, DeviceRole::Turbo]);
        assert_eq!(list.devices[1].state, DeviceState::Offline, "a server away is offline, not asleep");
        assert!(!list.devices[1].hears, "a server has no microphone");
        assert_eq!((list.devices[2].state, list.devices[2].order), (DeviceState::Off, Some(2)));
    }

    #[test]
    fn stations_of_another_centre_and_unapproved_ones_are_not_ears() {
        let centre =
            Centre { on_station: false, gateway_id: Some("g".into()), name: "PC".into(), device: None, model: None };
        let mut own = station("d-own", "Дача", Some(PresenceState::Voice));
        own.elsewhere = true;
        let mut pending = station("d-new", "Колонка", None);
        pending.status = StationStatus::Pending;
        let list = gather(home(), centre, &[own, pending, station("d-ear", "Кухня", None)], &[], &HashSet::new());
        let ids: Vec<&str> = list.devices.iter().map(|device| device.id.as_str()).collect();
        assert_eq!(ids, ["gateway:g", "station:d-ear"]);
        assert_eq!(list.devices[1].state, DeviceState::Offline);
    }

    /// Windows test 21.11: a station put there by the one-command install knows no device of its own, and its own voice
    /// client was a second row. It is the centre's row, with how its voice is.
    #[test]
    fn a_station_centre_is_one_row_with_its_own_voice() {
        let centre = Centre { device: None, model: None, ..station_centre() };
        let mut voice = station("h:957d99e28d", "Колонка", Some(PresenceState::Offline));
        voice.local = true;
        voice.model = Some("yandexmini_2".into());
        let list = gather(home(), centre, &[voice, station("d-bed", "Спальня", None)], &[], &HashSet::new());
        let ids: Vec<&str> = list.devices.iter().map(|device| device.id.as_str()).collect();
        assert_eq!(ids, ["station:h:957d99e28d", "station:d-bed"], "the centre once");
        assert_eq!(list.devices[0].state, DeviceState::Online, "its gateway answers");
        assert_eq!(list.devices[0].voice, Some(DeviceState::Offline), "its voice is away");
        assert_eq!(list.devices[0].model.as_deref(), Some("yandexmini_2"));
        assert_eq!(list.devices[1].voice, None);
        // Known by its device: the same, and a computer centre has no voice of its own here.
        let list = gather(
            home(),
            station_centre(),
            &[station("d-kitchen", "Кухня", Some(PresenceState::Voice))],
            &[],
            &HashSet::new(),
        );
        assert_eq!((list.devices.len(), list.devices[0].voice), (1, Some(DeviceState::Online)));
        let pc = Centre { on_station: false, device: None, ..station_centre() };
        let mut local = station("d-x", "Колонка", None);
        local.local = true;
        assert_eq!(gather(home(), pc, &[local], &[], &HashSet::new()).devices[0].voice, None);
    }

    #[test]
    fn loopback_is_the_machine_itself() {
        assert!(is_loopback("127.0.0.1") && is_loopback("::1") && is_loopback("localhost"));
        assert!(!is_loopback("192.168.50.58"));
    }
}
