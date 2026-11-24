//! This computer as a helper (pc): its connection key for «Подключить шлюз» and who it helps («Помогаю»). Only on a
//! computer that helps — one in the station role helps nobody (404).

use std::sync::Arc;

use axum::extract::Path;
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{json, Value};

use crate::core::events::EventBus;
use crate::core::events::GatewayEvent;
use crate::core::identity::home::Identity;
use crate::core::identity::permissions::{CONNECTORS_MANAGE, CONNECTORS_VIEW, USERS_MANAGE};
use crate::core::identity::principal::PrincipalKind;
use crate::core::identity::roles::Role;
use crate::helpers::clients::ClientView;
use crate::helpers::contract::Ability;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Uses};
use crate::settings::definitions::{HelperOutside, HELPER_OUTSIDE};
use crate::settings::SettingsService;
use crate::stations::connect::station_connect::{home_ranked, host_interfaces, Iface};

/// The key sheet («Подключить шлюз»).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperKeyView {
    pub key: String,
    /// Where other gateways reach the link: «Адрес снаружи» first, then this computer's home addresses.
    pub addresses: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outside: Option<String>,
    /// `<the first address> <key>`: the text and the QR a gateway takes.
    pub payload: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub armed_until: Option<String>,
    /// The link's port as other gateways dial it; none before it is bound, and none in a container, where the
    /// machine publishes it as another port.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

fn not_here() -> ApiError {
    ApiError::not_found(crate::tr!("Этот хаб не помогает другим домам: он не Турбо."))
}

/// The key only to an admin person signed in with a password: never a key, never the pair's pass.
fn admin(link: &Arc<crate::helpers::link_serve::LinkServer>, caller: &Caller) -> Result<(), ApiError> {
    if !link.helps() {
        return Err(not_here());
    }
    let principal = caller.permitted(USERS_MANAGE)?;
    if principal.kind != PrincipalKind::User || principal.role != Some(Role::Admin) || principal.id == "helper" {
        return Err(ApiError::forbidden(crate::tr!(
            "Ключ подключения показывается только администратору, вошедшему по паролю."
        )));
    }
    Ok(())
}

/// This computer's home IPv4 addresses with the link's port, always written: the likeliest first, as the station
/// wizard ranks them — the first goes into the text and the QR, so it must be one the home's other machines reach.
fn home_addresses(interfaces: &[Iface], port: u16) -> Vec<String> {
    home_ranked(interfaces).iter().map(|entry| format!("https://{}:{port}", entry.address)).collect()
}

/// The sheet of «Подключить шлюз». In a container (`contained`) the link's port and the addresses this gateway sees
/// are the container's own, not the machine's — compose.yaml publishes the port as another one —, so neither is
/// shown: the key alone, with «Адрес снаружи» when it is set.
pub fn sheet(
    link: &Arc<crate::helpers::link_serve::LinkServer>,
    settings: &Arc<SettingsService>,
    identity: &Arc<Identity>,
    key: &crate::helpers::link::ConnectionKey,
    contained: bool,
) -> HelperKeyView {
    let server = &link;
    let outside = settings.get(&HELPER_OUTSIDE, None).map(|outside| outside.url);
    let port = server.port().filter(|_| !contained);
    let mut addresses: Vec<String> = outside.iter().cloned().collect();
    if let Some(port) = port {
        addresses.extend(home_addresses(&host_interfaces(), port));
    }
    let shown = key.shown();
    let payload = match addresses.first() {
        Some(address) => format!("{address} {shown}"),
        None => shown.clone(),
    };
    HelperKeyView {
        key: shown,
        addresses,
        outside,
        payload,
        since: server.key_since(),
        name: identity.gateway().map(|me| me.name).unwrap_or_default(),
        armed_until: server.armed_until(),
        port,
        problem: server.problem(),
    }
}

fn key_view(
    link: &Arc<crate::helpers::link_serve::LinkServer>,
    settings: &Arc<SettingsService>,
    identity: &Arc<Identity>,
    key: &crate::helpers::link::ConnectionKey,
) -> Response {
    let contained = crate::stations::connect::station_connect::in_container();
    let mut response = ok(sheet(link, settings, identity, key, contained)).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The connection key, made when there is none, taken for the 15 minutes that opening the sheet starts. Read again
/// meanwhile — the sheet re-reads it as the panel changes — it is the same key with the same `armedUntil`, never
/// moved on; read after that, a new key for a new quarter of an hour. A pairing that spends it, or «Сменить ключ»,
/// shows a new one at once.
pub async fn key(
    Uses(link): Uses<Arc<crate::helpers::link_serve::LinkServer>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    Uses(identity): Uses<Arc<Identity>>,
    caller: Caller,
) -> Result<Response, ApiError> {
    admin(&link, &caller)?;
    let key = link.connection_key(true);
    Ok(key_view(&link, &settings, &identity, &key))
}

/// «Сменить ключ».
pub async fn rotate(
    Uses(link): Uses<Arc<crate::helpers::link_serve::LinkServer>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    Uses(identity): Uses<Arc<Identity>>,
    caller: Caller,
) -> Result<Response, ApiError> {
    admin(&link, &caller)?;
    let key = link.rotate(caller.actor());
    tracing::info!("link: the connection key changed");
    Ok(key_view(&link, &settings, &identity, &key))
}

/// «Адрес снаружи»: `https://` with a name or an address and an optional port; null removes it.
pub async fn outside(
    Uses(link): Uses<Arc<crate::helpers::link_serve::LinkServer>>,
    Uses(settings): Uses<Arc<SettingsService>>,
    Uses(events): Uses<EventBus>,
    Uses(identity): Uses<Arc<Identity>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> Result<Response, ApiError> {
    admin(&link, &caller)?;
    let wrong = || ApiError::bad_request(crate::tr!("Адрес снаружи: https://имя или https://адрес:порт."));
    // Removed: the setting forgotten, its change told (and logged, with who) as a write would tell it.
    let forget = || -> Result<(), ApiError> {
        if settings.get(&HELPER_OUTSIDE, None).is_some() {
            settings.forget(&HELPER_OUTSIDE)?;
            events.emit(GatewayEvent::SettingsChanged {
                key: HELPER_OUTSIDE.key.into(),
                user_id: None,
                actor: caller.actor(),
            });
        }
        Ok(())
    };
    match body.get("url") {
        None | Some(Value::Null) => forget()?,
        Some(Value::String(text)) if text.trim().is_empty() => forget()?,
        Some(Value::String(text)) => {
            let parsed = url::Url::parse(text.trim()).map_err(|_| wrong())?;
            let host = parsed.host_str().filter(|host| !host.is_empty()).ok_or_else(wrong)?;
            if parsed.scheme() != "https"
                || !matches!(parsed.path(), "" | "/")
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || !parsed.username().is_empty()
            {
                return Err(wrong());
            }
            let port = parsed.port().map(|port| format!(":{port}")).unwrap_or_default();
            let url = format!("https://{}{port}", host.to_ascii_lowercase());
            settings.set(&HELPER_OUTSIDE, &HelperOutside { url }, caller.actor())?;
        }
        Some(_) => return Err(wrong()),
    }
    let key = link.connection_key(false);
    Ok(key_view(&link, &settings, &identity, &key))
}

fn views(
    station_gateway: &Arc<crate::placement::watch::StationGateways>,
    link: &Arc<crate::helpers::link_serve::LinkServer>,
) -> Vec<ClientView> {
    let placements: Vec<(String, Option<String>)> = station_gateway
        .all()
        .iter()
        .filter_map(|station| station.record().map(|record| (station.key().to_string(), record.home_key_id)))
        .collect();
    link.clients.view(&placements)
}

/// «Помогаю»: who this computer helps.
pub async fn clients(
    Uses(link): Uses<Arc<crate::helpers::link_serve::LinkServer>>,
    Uses(station_gateway): Uses<Arc<crate::placement::watch::StationGateways>>,
    caller: Caller,
) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_VIEW)?;
    if !link.helps() {
        return Err(not_here());
    }
    Ok(ok(json!({ "clients": views(&station_gateway, &link) })))
}

fn link_client(id: &str) -> Result<(), ApiError> {
    if id.starts_with("key:") {
        return Err(ApiError::conflict(crate::tr!(
            "Это ключ из «Доступ» → «Ключи»: что он даёт и как его отозвать — там."
        )));
    }
    Ok(())
}

/// `{gives}`: what a home over the link is given.
pub async fn set_gives(
    Uses(link): Uses<Arc<crate::helpers::link_serve::LinkServer>>,
    Uses(station_gateway): Uses<Arc<crate::placement::watch::StationGateways>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_MANAGE)?;
    if !link.helps() {
        return Err(not_here());
    }
    link_client(&id)?;
    let gives: Vec<Ability> = body
        .get("gives")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).filter_map(Ability::parse).collect())
        .ok_or_else(|| ApiError::bad_request(crate::tr!("gives: список умений.")))?;
    link.clients.set_gives(&id, gives, caller.actor())?;
    Ok(ok(json!({ "clients": views(&station_gateway, &link) })))
}

/// A home removed: its key forgotten, its links cut.
pub async fn remove_client(
    Uses(link): Uses<Arc<crate::helpers::link_serve::LinkServer>>,
    Uses(station_gateway): Uses<Arc<crate::placement::watch::StationGateways>>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_MANAGE)?;
    if !link.helps() {
        return Err(not_here());
    }
    link_client(&id)?;
    link.clients.remove(&id, caller.actor())?;
    Ok(ok(json!({ "clients": views(&station_gateway, &link) })))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Step 11 check: on a Mac with the pretend stations' aliases on lo0, listed before the Wi-Fi, the sheet's first
    /// address — the one in the text and the QR — was https://10.77.0.1:7781, and the hub could not reach it.
    #[test]
    fn the_sheets_first_address_is_the_home_networks_not_the_loopbacks() {
        let iface = |name: &str, address: &str, internal: bool| Iface {
            ipv4: true,
            internal,
            address: address.into(),
            name: name.into(),
            tunnel: name.starts_with("utun"),
        };
        let interfaces = vec![
            iface("lo0", "127.0.0.1", true),
            iface("lo0", "10.77.0.1", false),
            iface("utun4", "10.8.0.2", false),
            iface("en0", "192.168.1.110", false),
        ];
        assert_eq!(home_addresses(&interfaces, 7781), ["https://192.168.1.110:7781", "https://10.8.0.2:7781"]);
    }
}
