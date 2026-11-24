//! The home's devices — the computer's part, added to «Дом»'s `home::devices`: the home as this computer sees it when
//! it is a Турбо — its centre elsewhere (a station with a gateway of its own) by its record, the home it helps over the
//! link.

use std::sync::Arc;

pub use duowave_home::home::devices::*;

use super::computer::Computer;
use super::Parts;

/// A computer that helps another home over the link and keeps no station of its own (stage 7 of
/// docs/plans/home-setup-flow.md): that home's Turbo, not a centre with a Turbo of its own — «Компьютер · Центр ·
/// Турбо» on one device was the Windows test's puzzle (21.11). Its own home stays in its data, for «Это новый дом».
pub fn link_home(pc: &Computer, parts: &Parts) -> Option<crate::helpers::clients::ClientView> {
    if !pc.link.helps() || !pc.station_gateway.all().is_empty() || !parts.stations.list().is_empty() {
        return None;
    }
    pc.clients.view(&[]).into_iter().find(|client| client.via == "link" && !client.own_home && client.home.is_some())
}

/// The home this computer is the Turbo of over the link, as /health tells the panel (`placement.centre`, stage 7 of
/// docs/plans/home-setup-flow.md): the key of the pass to its centre (the client's id), the home's name, whether a link
/// to it is up now. Only once its centre said at a link that it takes this computer's own requests (`up`): with an older
/// centre — or one that does not count this computer as a Turbo at home — the panel keeps this computer's own desk.
pub fn link_centre(pc: &Computer, parts: &Parts) -> Option<serde_json::Value> {
    let client = link_home(pc, parts)?;
    if pc.link.takes_up(&client.id) != Some(true) {
        return None;
    }
    let name = client.home.as_ref().map(|home| home.name.clone()).unwrap_or_default();
    let online = pc.link.up_of(&client.id).is_some();
    Some(serde_json::json!({ "key": client.id, "name": name, "online": online }))
}

/// That home as its Turbo shows it: the centre by the home's name — its gateways online or not —, itself as Turbo.
pub fn as_link_turbo(parts: &Parts, client: &crate::helpers::clients::ClientView) -> Option<HomeDevices> {
    let home = client.home.clone()?;
    let id = format!("home:{}", home.id);
    let mut hub =
        HomeDevice::new(id.clone(), DeviceKind::Computer, crate::tr!("Хаб дома «{name}»", name = home.name.clone()));
    hub.roles.push(DeviceRole::Centre);
    hub.state =
        if client.gateways.iter().any(|gateway| gateway.online) { DeviceState::Online } else { DeviceState::Offline };
    let me = parts.identity.gateway();
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

/// The station whose gateway is this home's centre while this computer is its Turbo: one with its own gateway (not
/// placed on this computer).
pub fn centre_elsewhere(
    station_gateway: &Arc<crate::placement::watch::StationGateways>,
) -> Option<Arc<crate::placement::watch::StationGateway>> {
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
    station_gateway
        .all()
        .into_iter()
        .find(|station| station.record().is_some_and(|record| record.placement != Placement::Pc) && !ear(station))
}

/// What a Turbo computer knows of its home while the centre is away: the centre by its record, then itself.
pub fn as_turbo(
    identity: &Arc<crate::core::identity::home::Identity>,
    settings: &Arc<crate::settings::SettingsService>,
    centre: &crate::placement::watch::StationGateway,
) -> HomeDevices {
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
    let me = identity.gateway();
    let mut turbo = HomeDevice::new(
        format!("gateway:{}", me.as_ref().map(|me| me.id.as_str()).unwrap_or_default()),
        DeviceKind::Computer,
        me.as_ref().map(|me| me.name.clone()).unwrap_or_default(),
    );
    turbo.roles.push(DeviceRole::Turbo);
    turbo.hears = true;
    turbo.copy = crate::home::copy::copied_revision(settings).is_some();
    turbo.state = DeviceState::Online;
    turbo.gateway_id = me.map(|me| me.id);
    turbo.order = Some(1);
    turbo.enabled = Some(true);
    HomeDevices {
        home: HomeInfo { id: identity.home().map(|home| home.id), name: identity.home_name() },
        centre: id,
        devices: vec![hub, turbo],
    }
}
