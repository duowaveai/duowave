//! `/api/v1/helpers*` (docs/plans/home-and-helpers.md, «API»): the panel's page «Помощники» — the list with «Где
//! сейчас» and the regulator in one answer, adding (checked first, or only checked), a helper's name, switch and
//! abilities, the order, removing, «Проверить сейчас», the regulator. Seeing needs `CONNECTORS_VIEW`, changing
//! `CONNECTORS_MANAGE`, as the old `/api/v1/home-server` did. axum prefers a static segment over `{id}`, so `order` and
//! `away` route right, and `valid_id` keeps those words out of ids. On a station whose settings live on the computer
//! the changes are refused by the placement's guard (placement/routes.rs), but for the station's own entry
//! `home-computer` and the checks.
//!
//! Adding with a connection key pairs over the link (stage 7, link.rs); a computer that helps also shows its key
//! («Подключить шлюз», to an admin signed in with a password only) and who it helps («Помогаю»).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post, put};
use axum::Router;
use serde::Serialize;
use serde_json::{json, Value};

use super::contract::Ability;
use super::now::NowRow;
use super::{AddInput, Away, Change, HelperKind, HelperView};
use crate::app::App;
use crate::core::identity::home::GatewayHealth;
use crate::core::identity::permissions::{CONNECTORS_MANAGE, CONNECTORS_VIEW};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller};

pub fn router() -> Router<Arc<App>> {
    let router = Router::new()
        .route("/api/v1/helpers", get(list).post(add))
        .route("/api/v1/helpers/order", put(order))
        .route("/api/v1/helpers/away", get(away).put(set_away))
        .route("/api/v1/helpers/{id}", put(update).delete(remove))
        .route("/api/v1/helpers/{id}/check", post(check));
    #[cfg(feature = "pc")]
    let router = router
        .route("/api/v1/helpers/key", get(helping::key))
        .route("/api/v1/helpers/key/rotate", post(helping::rotate))
        .route("/api/v1/helpers/key/outside", put(helping::outside))
        .route("/api/v1/helpers/clients", get(helping::clients))
        .route("/api/v1/helpers/clients/{id}", put(helping::set_gives).delete(helping::remove_client));
    router
}

/// The page in one answer: the helpers, «Где сейчас», the regulator, this gateway.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HelpersView {
    pub helpers: Vec<HelperView>,
    pub now: Vec<NowRow>,
    pub away: Away,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<GatewayHealth>,
}

pub fn view(app: &App) -> HelpersView {
    HelpersView {
        helpers: app.helpers.views(),
        now: super::now::rows(app),
        away: app.helpers.away(),
        gateway: app.identity.gateway(),
    }
}

async fn list(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<HelpersView> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(view(&app)))
}

/// A text field of the body: absent or null — None.
fn text(body: &Value, field: &str) -> Result<Option<String>, ApiError> {
    match body.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(ApiError::bad_request(crate::tr!("{name}: нужна строка.", name = field))),
    }
}

/// A list of ability names; the ones this gateway does not know are passed by.
fn abilities(body: &Value, field: &str) -> Option<Vec<Ability>> {
    let list = body.get(field)?.as_array()?;
    Some(list.iter().filter_map(Value::as_str).filter_map(Ability::parse).collect())
}

/// `{url, key, name?, kind?, without?, dryRun?, replace?}`: a helper checked first and added last in the list
/// (201 with the page and the entry's `id`); `dryRun: true` only checks it — 200 `{helper}`, nothing stored or spent.
/// A connection key pairs over the link; a `vgw_` key is a plain helper at home. Both answers name the entry a
/// re-pairing renews (`matched`): the one `replace` names («Подключить заново»), or — for a connection key — the one
/// of the same helper with the very certificate pinned.
async fn add(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_MANAGE)?;
    let kind = match text(&body, "kind")? {
        None => None,
        Some(kind) => Some(
            HelperKind::parse(&kind).ok_or_else(|| ApiError::bad_request(crate::tr!("kind: home, own или cloud.")))?,
        ),
    };
    let input = AddInput {
        url: text(&body, "url")?.unwrap_or_default(),
        key: text(&body, "key")?.unwrap_or_default(),
        name: text(&body, "name")?,
        kind,
        without: abilities(&body, "without").unwrap_or_default(),
        replace: text(&body, "replace")?,
    };
    let dry = body.get("dryRun").and_then(Value::as_bool) == Some(true);
    let key = input.key.trim().to_string();
    if let Some(connection) = super::link::ConnectionKey::parse(&key) {
        return pair(&app, &caller, input, connection, dry).await;
    }
    if !key.is_empty() && !key.starts_with("vgw_") {
        return Err(ApiError::bad_request(crate::tr!("Ключ подключения — 26 букв и цифр, как его показывает Турбо.")));
    }
    if dry {
        let helper = app.helpers.probe(&input).await?;
        let mut answer = json!({ "helper": helper });
        if let Some(replace) = &input.replace {
            answer["matched"] = json!(replace);
        }
        return Ok(ok(answer));
    }
    let replace = input.replace.clone();
    let id = app.helpers.add(input, caller.actor()).await?;
    let mut answer = serde_json::to_value(view(&app))?;
    answer["id"] = json!(id);
    if let Some(replace) = replace {
        answer["matched"] = json!(replace);
    }
    Ok(created(answer))
}

/// A helper's address for its link: `https://host:port`, the port always written (`http://host` — port 80; a bare
/// `host:port` — that port; `https://host` — 443), no path.
fn link_url(text: &str) -> Option<String> {
    let text = text.trim();
    let (scheme, rest) = match text.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => ("".to_string(), text),
    };
    let default = match scheme.as_str() {
        "https" => 443,
        "http" => 80,
        "" => 443,
        _ => return None,
    };
    let parsed = url::Url::parse(&format!("https://{rest}")).ok()?;
    if !parsed.username().is_empty() || parsed.password().is_some() || parsed.query().is_some() {
        return None;
    }
    let host = match parsed.host()? {
        url::Host::Domain(name) => super::link::bound_host(name),
        url::Host::Ipv4(ip) => ip.to_string(),
        url::Host::Ipv6(ip) => format!("[{ip}]"),
    };
    let port = parsed.port().unwrap_or(default);
    Some(format!("https://{host}:{port}"))
}

/// Into the home by a code (src/home/join.rs, docs/plans/home-setup-flow.md stage 2): a computer that asked in with its
/// own connection key and its link's address is paired here as a Turbo at home — as «Подключить Турбо» with that key
/// pairs it, the household's code (or an admin's «Разрешить») in place of the key typed by hand. The entry's id.
pub async fn pair_with_key(
    app: &Arc<App>,
    url: &str,
    key: &str,
    name: Option<&str>,
    actor: Option<crate::core::events::Actor>,
) -> Result<String, ApiError> {
    let connection = super::link::ConnectionKey::parse(key.trim()).ok_or_else(|| {
        ApiError::bad_request(crate::tr!("Ключ подключения — 26 букв и цифр, как его показывает Турбо."))
    })?;
    let url = link_url(url)
        .ok_or_else(|| ApiError::bad_request(crate::tr!("Адрес Турбо: http://адрес:порт или https://…")))?;
    if !crate::operations::networks::is_local_url(Some(&url)) {
        return Err(ApiError::bad_request(crate::tr!("Турбо по адресу вне дома не может быть «дома».")));
    }
    let name = name.map(super::helper_name).transpose()?;
    let me = super::remote::Me::of(&app.identity);
    let home_key = app.secrets.get(crate::core::identity::home::HOME_KEY).unwrap_or_default();
    let check = app.helpers.pair_check(None);
    let timing = app.helpers.link_timing();
    let paired = super::link::pair(&url, &connection, &me, &home_key, &timing, false, &check)
        .await
        .map_err(|error| error.into_api(&url))?;
    let (id, _) = app.helpers.add_linked(&url, name, Some(HelperKind::Home), Vec::new(), None, paired, actor).await?;
    Ok(id)
}

/// The pairing: the helper's link, the connection key, this gateway's home proven, `pair_check` before anything is
/// spent; a probe stops there. Every check of the input comes first: once the helper confirms, its key is spent.
async fn pair(
    app: &Arc<App>,
    caller: &Caller,
    mut input: super::AddInput,
    key: super::link::ConnectionKey,
    dry: bool,
) -> ApiResult<Value> {
    let url = link_url(&input.url)
        .ok_or_else(|| ApiError::bad_request(crate::tr!("Адрес Турбо: http://адрес:порт или https://…")))?;
    if input.kind == Some(HelperKind::Home) && !crate::operations::networks::is_local_url(Some(&url)) {
        return Err(ApiError::bad_request(crate::tr!("Турбо по адресу вне дома не может быть «дома».")));
    }
    input.name = input.name.as_deref().map(super::helper_name).transpose()?;
    let me = super::remote::Me::of(&app.identity);
    let home_key = app.secrets.get(crate::core::identity::home::HOME_KEY).unwrap_or_default();
    let check = app.helpers.pair_check(input.replace.clone());
    let timing = app.helpers.link_timing();
    let paired = super::link::pair(&url, &key, &me, &home_key, &timing, dry, &check)
        .await
        .map_err(|error| error.into_api(&url))?;
    if dry {
        let helper = app.helpers.paired_view(&url, input.name.as_deref(), input.kind, &paired);
        let mut answer = json!({ "helper": helper });
        if let Some(matched) = app.helpers.matched(input.replace.as_deref(), &paired)? {
            answer["matched"] = json!(matched);
        }
        return Ok(ok(answer));
    }
    let (id, matched) = app
        .helpers
        .add_linked(&url, input.name, input.kind, input.without, input.replace, paired, caller.actor())
        .await?;
    let mut answer = serde_json::to_value(view(app))?;
    answer["id"] = json!(id);
    if let Some(matched) = matched {
        answer["matched"] = json!(matched);
    }
    Ok(created(answer))
}

/// `{name?, enabled?, without?}`. «Модель» is not let go while «Мозг» answers with the model reached through it.
async fn update(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<HelpersView> {
    caller.permitted(CONNECTORS_MANAGE)?;
    let enabled = match body.get("enabled") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(on)) => Some(*on),
        Some(_) => return Err(ApiError::bad_request(crate::tr!("enabled: true или false"))),
    };
    let change = Change { name: text(&body, "name")?, enabled, without: abilities(&body, "without") };
    if app.helpers.entry(&id).is_none() {
        return Err(ApiError::not_found(crate::tr!("Нет такого Турбо.")));
    }
    if let Some(without) = &change.without {
        crate::home_server::check_model_kept(&app.helpers, &app.brain, &id, without)?;
    }
    app.helpers.update(&id, change, caller.actor())?;
    Ok(ok(view(&app)))
}

/// `{ids}`: every stored helper once, in the new order.
async fn order(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<HelpersView> {
    caller.permitted(CONNECTORS_MANAGE)?;
    let ids: Vec<String> = body
        .get("ids")
        .and_then(Value::as_array)
        .map(|ids| ids.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .ok_or_else(|| ApiError::bad_request(crate::tr!("ids: каждое Турбо списка по одному разу.")))?;
    app.helpers.order(&ids, caller.actor())?;
    Ok(ok(view(&app)))
}

async fn remove(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<HelpersView> {
    caller.permitted(CONNECTORS_MANAGE)?;
    app.helpers.remove(&id, caller.actor())?;
    Ok(ok(view(&app)))
}

/// «Проверить сейчас»: one look at the helper now, a switched-off one too; one the regulator keeps out is not looked
/// at.
async fn check(State(app): State<Arc<App>>, caller: Caller, Path(id): Path<String>) -> ApiResult<HelpersView> {
    caller.permitted(CONNECTORS_VIEW)?;
    app.helpers.check(&id).await?;
    Ok(ok(view(&app)))
}

async fn away(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(json!({ "away": app.helpers.away() })))
}

/// `{away: "home" | "mine" | "any"}`: «Что может уходить из дома».
async fn set_away(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<HelpersView> {
    caller.permitted(CONNECTORS_MANAGE)?;
    let away = body
        .get("away")
        .and_then(Value::as_str)
        .and_then(Away::parse)
        .ok_or_else(|| ApiError::bad_request(crate::tr!("away: home, mine или any.")))?;
    app.helpers.set_away(away, caller.actor())?;
    Ok(ok(view(&app)))
}

/// This computer as a helper (pc): its connection key for «Подключить шлюз» and who it helps («Помогаю»). Only on a
/// computer that helps — one in the station role helps nobody (404).
#[cfg(feature = "pc")]
mod helping {
    use std::sync::Arc;

    use axum::extract::{Path, State};
    use axum::http::{header, HeaderValue};
    use axum::response::{IntoResponse, Response};
    use serde::Serialize;
    use serde_json::{json, Value};

    use crate::app::App;
    use crate::core::events::GatewayEvent;
    use crate::core::identity::permissions::{CONNECTORS_MANAGE, CONNECTORS_VIEW, USERS_MANAGE};
    use crate::core::identity::principal::PrincipalKind;
    use crate::core::identity::roles::Role;
    use crate::helpers::clients::ClientView;
    use crate::helpers::contract::Ability;
    use crate::operations::networks::in_networks;
    use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller};
    use crate::settings::definitions::{HelperOutside, HELPER_OUTSIDE};

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
    fn admin(app: &App, caller: &Caller) -> Result<(), ApiError> {
        if !app.link.helps() {
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

    /// This computer's home IPv4 addresses with the link's port, always written.
    fn home_addresses(port: u16) -> Vec<String> {
        let home = ["10.0.0.0/8".to_string(), "192.168.0.0/16".to_string(), "172.16.0.0/12".to_string()];
        crate::stations::connect::station_connect::host_interfaces()
            .into_iter()
            .filter(|entry| entry.ipv4 && !entry.internal && in_networks(&entry.address, &home))
            .map(|entry| format!("https://{}:{port}", entry.address))
            .collect()
    }

    /// The sheet of «Подключить шлюз». In a container (`contained`) the link's port and the addresses this gateway sees
    /// are the container's own, not the machine's — compose.yaml publishes the port as another one —, so neither is
    /// shown: the key alone, with «Адрес снаружи» when it is set.
    fn sheet(app: &App, key: &crate::helpers::link::ConnectionKey, contained: bool) -> HelperKeyView {
        let server = &app.link;
        let outside = app.settings.get(&HELPER_OUTSIDE, None).map(|outside| outside.url);
        let port = server.port().filter(|_| !contained);
        let mut addresses: Vec<String> = outside.iter().cloned().collect();
        if let Some(port) = port {
            addresses.extend(home_addresses(port));
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
            name: app.identity.gateway().map(|me| me.name).unwrap_or_default(),
            armed_until: server.armed_until(),
            port,
            problem: server.problem(),
        }
    }

    fn key_view(app: &App, key: &crate::helpers::link::ConnectionKey) -> Response {
        let contained = crate::stations::connect::station_connect::in_container();
        let mut response = ok(sheet(app, key, contained)).into_response();
        response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }

    /// The connection key, made when there is none, taken for the 15 minutes that opening the sheet starts. Read again
    /// meanwhile — the sheet re-reads it as the panel changes — it is the same key with the same `armedUntil`, never
    /// moved on; read after that, a new key for a new quarter of an hour. A pairing that spends it, or «Сменить ключ»,
    /// shows a new one at once.
    pub async fn key(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
        admin(&app, &caller)?;
        let key = app.link.connection_key(true);
        Ok(key_view(&app, &key))
    }

    /// «Сменить ключ».
    pub async fn rotate(State(app): State<Arc<App>>, caller: Caller) -> Result<Response, ApiError> {
        admin(&app, &caller)?;
        let key = app.link.rotate(caller.actor());
        tracing::info!("link: the connection key changed");
        Ok(key_view(&app, &key))
    }

    /// «Адрес снаружи»: `https://` with a name or an address and an optional port; null removes it.
    pub async fn outside(
        State(app): State<Arc<App>>,
        caller: Caller,
        Body(body): Body<Value>,
    ) -> Result<Response, ApiError> {
        admin(&app, &caller)?;
        let wrong = || ApiError::bad_request(crate::tr!("Адрес снаружи: https://имя или https://адрес:порт."));
        // Removed: the setting forgotten, its change told (and logged, with who) as a write would tell it.
        let forget = || -> Result<(), ApiError> {
            if app.settings.get(&HELPER_OUTSIDE, None).is_some() {
                app.settings.forget(&HELPER_OUTSIDE)?;
                app.events.emit(GatewayEvent::SettingsChanged {
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
                app.settings.set(&HELPER_OUTSIDE, &HelperOutside { url }, caller.actor())?;
            }
            Some(_) => return Err(wrong()),
        }
        let key = app.link.connection_key(false);
        Ok(key_view(&app, &key))
    }

    fn views(app: &App) -> Vec<ClientView> {
        let placements: Vec<(String, Option<String>)> = app
            .station_gateway
            .all()
            .iter()
            .filter_map(|station| station.record().map(|record| (station.key().to_string(), record.home_key_id)))
            .collect();
        app.link.clients.view(&placements)
    }

    /// «Помогаю»: who this computer helps.
    pub async fn clients(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<Value> {
        caller.permitted(CONNECTORS_VIEW)?;
        if !app.link.helps() {
            return Err(not_here());
        }
        Ok(ok(json!({ "clients": views(&app) })))
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
        State(app): State<Arc<App>>,
        caller: Caller,
        Path(id): Path<String>,
        Body(body): Body<Value>,
    ) -> ApiResult<Value> {
        caller.permitted(CONNECTORS_MANAGE)?;
        if !app.link.helps() {
            return Err(not_here());
        }
        link_client(&id)?;
        let gives: Vec<Ability> = body
            .get("gives")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Value::as_str).filter_map(Ability::parse).collect())
            .ok_or_else(|| ApiError::bad_request(crate::tr!("gives: список умений.")))?;
        app.link.clients.set_gives(&id, gives, caller.actor())?;
        Ok(ok(json!({ "clients": views(&app) })))
    }

    /// A home removed: its key forgotten, its links cut.
    pub async fn remove_client(
        State(app): State<Arc<App>>,
        caller: Caller,
        Path(id): Path<String>,
    ) -> ApiResult<Value> {
        caller.permitted(CONNECTORS_MANAGE)?;
        if !app.link.helps() {
            return Err(not_here());
        }
        link_client(&id)?;
        app.link.clients.remove(&id, caller.actor())?;
        Ok(ok(json!({ "clients": views(&app) })))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::helpers::builtin::tests::computer;

        /// Inside a container the sheet shows neither the container's link port nor its addresses — compose.yaml
        /// publishes the port as another one —: the key alone to copy, «Адрес снаружи» first when it is set. Outside
        /// a container, the port it listens on.
        #[tokio::test]
        async fn in_a_container_the_key_sheet_names_no_port_of_its_own() {
            let (app, _dir) = computer(&[]);
            app.link.set_port(3001);
            let key = app.link.connection_key(true);
            let sheet_in = sheet(&app, &key, true);
            assert_eq!((sheet_in.port, sheet_in.addresses.len()), (None, 0), "{sheet_in:?}");
            assert_eq!(sheet_in.payload, key.shown(), "the key alone");
            let shown = serde_json::to_value(&sheet_in).unwrap();
            assert!(shown.get("port").is_none(), "{shown}");
            assert_eq!(sheet(&app, &key, false).port, Some(3001));

            let outside = HelperOutside { url: "https://helper.example.com".into() };
            app.settings.set(&HELPER_OUTSIDE, &outside, None).unwrap();
            let sheet_in = sheet(&app, &key, true);
            assert_eq!(sheet_in.addresses, ["https://helper.example.com"]);
            assert_eq!(sheet_in.payload, format!("https://helper.example.com {}", key.shown()));
            assert_eq!(sheet_in.port, None);
        }
    }
}
