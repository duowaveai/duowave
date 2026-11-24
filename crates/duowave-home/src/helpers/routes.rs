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

use axum::extract::Path;
use axum::routing::{get, post, put};
use axum::Router;
use serde::Serialize;
use serde_json::{json, Value};

use super::contract::Ability;
use super::now::NowRow;
pub use super::{AddInput, Away, Change, HelperKind, HelperView, Parts};
use crate::core::identity::home::{GatewayHealth, Identity};
use crate::core::identity::permissions::{CONNECTORS_MANAGE, CONNECTORS_VIEW};
use crate::helpers::Helpers;
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Uses};
use crate::secrets::SecretsService;

pub fn router<S: crate::helpers::RouteNeeds>() -> Router<S> {
    let router = Router::new()
        .route("/api/v1/helpers", get(list).post(add))
        .route("/api/v1/helpers/order", put(order))
        .route("/api/v1/helpers/away", get(away).put(set_away))
        .route("/api/v1/helpers/{id}", put(update).delete(remove))
        .route("/api/v1/helpers/{id}/check", post(check));
    // A computer that helps adds its key and its clients (crates/duowave-pc, helpers::routes).
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

pub fn view(parts: &Parts) -> HelpersView {
    HelpersView {
        helpers: parts.helpers.views(),
        now: super::now::rows(parts),
        away: parts.helpers.away(),
        gateway: parts.identity.gateway(),
    }
}

async fn list(Uses(parts): Uses<Parts>, caller: Caller) -> ApiResult<HelpersView> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(view(&parts)))
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
async fn add(Uses(parts): Uses<Parts>, caller: Caller, Body(body): Body<Value>) -> ApiResult<Value> {
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
        return pair(&parts, &caller, input, connection, dry).await;
    }
    if !key.is_empty() && !key.starts_with("vgw_") {
        return Err(ApiError::bad_request(crate::tr!("Ключ подключения — 26 букв и цифр, как его показывает Турбо.")));
    }
    if dry {
        let helper = parts.helpers.probe(&input).await?;
        let mut answer = json!({ "helper": helper });
        if let Some(replace) = &input.replace {
            answer["matched"] = json!(replace);
        }
        return Ok(ok(answer));
    }
    let replace = input.replace.clone();
    let id = parts.helpers.add(input, caller.actor()).await?;
    let mut answer = serde_json::to_value(view(&parts))?;
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
    (identity, secrets, helpers): (&Identity, &SecretsService, &Helpers),
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
    let me = super::remote::Me::of(identity);
    let home_key = secrets.get(crate::core::identity::home::HOME_KEY).unwrap_or_default();
    let check = helpers.pair_check(None);
    let timing = helpers.link_timing();
    let paired = super::link::pair(&url, &connection, &me, &home_key, &timing, false, &check)
        .await
        .map_err(|error| error.into_api(&url))?;
    let (id, _) = helpers.add_linked(&url, name, Some(HelperKind::Home), Vec::new(), None, paired, actor).await?;
    Ok(id)
}

/// The pairing: the helper's link, the connection key, this gateway's home proven, `pair_check` before anything is
/// spent; a probe stops there. Every check of the input comes first: once the helper confirms, its key is spent.
async fn pair(
    parts: &Parts,
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
    let me = super::remote::Me::of(&parts.identity);
    let home_key = parts.secrets.get(crate::core::identity::home::HOME_KEY).unwrap_or_default();
    let check = parts.helpers.pair_check(input.replace.clone());
    let timing = parts.helpers.link_timing();
    let paired = super::link::pair(&url, &key, &me, &home_key, &timing, dry, &check)
        .await
        .map_err(|error| error.into_api(&url))?;
    if dry {
        let helper = parts.helpers.paired_view(&url, input.name.as_deref(), input.kind, &paired);
        let mut answer = json!({ "helper": helper });
        if let Some(matched) = parts.helpers.matched(input.replace.as_deref(), &paired)? {
            answer["matched"] = json!(matched);
        }
        return Ok(ok(answer));
    }
    let (id, matched) = parts
        .helpers
        .add_linked(&url, input.name, input.kind, input.without, input.replace, paired, caller.actor())
        .await?;
    let mut answer = serde_json::to_value(view(parts))?;
    answer["id"] = json!(id);
    if let Some(matched) = matched {
        answer["matched"] = json!(matched);
    }
    Ok(created(answer))
}

/// `{name?, enabled?, without?}`. «Модель» is not let go while «Мозг» answers with the model reached through it.
async fn update(
    Uses(parts): Uses<Parts>,
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
    if parts.helpers.entry(&id).is_none() {
        return Err(ApiError::not_found(crate::tr!("Нет такого Турбо.")));
    }
    if let Some(without) = &change.without {
        crate::home_server::check_model_kept(&parts.helpers, &parts.brain, &id, without)?;
    }
    parts.helpers.update(&id, change, caller.actor())?;
    Ok(ok(view(&parts)))
}

/// `{ids}`: every stored helper once, in the new order.
async fn order(Uses(parts): Uses<Parts>, caller: Caller, Body(body): Body<Value>) -> ApiResult<HelpersView> {
    caller.permitted(CONNECTORS_MANAGE)?;
    let ids: Vec<String> = body
        .get("ids")
        .and_then(Value::as_array)
        .map(|ids| ids.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .ok_or_else(|| ApiError::bad_request(crate::tr!("ids: каждое Турбо списка по одному разу.")))?;
    parts.helpers.order(&ids, caller.actor())?;
    Ok(ok(view(&parts)))
}

async fn remove(Uses(parts): Uses<Parts>, caller: Caller, Path(id): Path<String>) -> ApiResult<HelpersView> {
    caller.permitted(CONNECTORS_MANAGE)?;
    parts.helpers.remove(&id, caller.actor())?;
    Ok(ok(view(&parts)))
}

/// «Проверить сейчас»: one look at the helper now, a switched-off one too; one the regulator keeps out is not looked
/// at.
async fn check(Uses(parts): Uses<Parts>, caller: Caller, Path(id): Path<String>) -> ApiResult<HelpersView> {
    caller.permitted(CONNECTORS_VIEW)?;
    parts.helpers.check(&id).await?;
    Ok(ok(view(&parts)))
}

async fn away(Uses(helpers): Uses<Arc<Helpers>>, caller: Caller) -> ApiResult<Value> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(json!({ "away": helpers.away() })))
}

/// `{away: "home" | "mine" | "any"}`: «Что может уходить из дома».
async fn set_away(Uses(parts): Uses<Parts>, caller: Caller, Body(body): Body<Value>) -> ApiResult<HelpersView> {
    caller.permitted(CONNECTORS_MANAGE)?;
    let away = body
        .get("away")
        .and_then(Value::as_str)
        .and_then(Away::parse)
        .ok_or_else(|| ApiError::bad_request(crate::tr!("away: home, mine или any.")))?;
    parts.helpers.set_away(away, caller.actor())?;
    Ok(ok(view(&parts)))
}
