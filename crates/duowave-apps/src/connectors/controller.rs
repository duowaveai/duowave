//! The panel's routes for the connectors it sets up («Дом» → «Коннекторы»). Their settings, a change, «Очистить»,
//! «Проверить», who wrote to the bot, a feed looked at before it is added: a person with the right of connectors,
//! from home — as the MCP servers (a token is someone's account). The news catalog: whoever sees the connectors.

use std::sync::Arc;

use axum::extract::Path;
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::checks::{Checker, FeedPreview};
use super::settings::{ConnectorId, ConnectorSettings, ConnectorSettingsView};
use crate::config::Config;
use crate::core::identity::permissions::{CONNECTORS_MANAGE, CONNECTORS_VIEW};
use crate::core::identity::PrincipalKind;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Uses, Zone};
use crate::skills::lookups::news_catalog::{NewsCatalog, NEWS_CATALOG};

pub fn routes<S: super::RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/connectors/news/catalog", get(catalog))
        .route("/api/v1/connectors/news/preview", post(preview))
        .route("/api/v1/connectors/telegram/chats", get(chats))
        .route("/api/v1/connectors/{id}/settings", get(view).put(update).delete(clear))
        .route("/api/v1/connectors/{id}/check", post(check))
}

/// From home, with the right of connectors, a person: never a script's key.
fn manager(caller: &Caller) -> Result<(), ApiError> {
    caller.zone_at_least(Zone::Home)?;
    if caller.permitted(CONNECTORS_MANAGE)?.kind != PrincipalKind::User {
        return Err(ApiError::forbidden(crate::tr!("Это для людей, не для ключей.")));
    }
    Ok(())
}

fn connector(id: &str) -> Result<ConnectorId, ApiError> {
    ConnectorId::parse(id).ok_or_else(|| ApiError::not_found(crate::tr!("Нет такого подключения.")))
}

/// What a connector's sheet works with: its settings, the configuration (the weather's checks), and on a computer the
/// calls a change would end (crates/duowave-pc, calls). The gateway makes it (src/links/http.rs).
#[derive(Clone)]
pub struct Parts {
    pub connector_settings: Arc<ConnectorSettings>,
    pub config: Arc<Config>,
    /// The computer's calls; the station's build has none.
    pub calls: Option<Arc<dyn CallsSheet>>,
}

/// The calls on a computer as a connector's sheet sees them (crates/duowave-pc, calls, implements it; links/http.rs
/// gives it).
pub trait CallsSheet: Send + Sync {
    /// Whether a call goes on through the connector whose parties' ids start so (`sip:`, `livekit:`): a change would
    /// end it.
    fn busy_through(&self, prefix: &str) -> bool;
    /// Who wrote to the bot lately and is no contact, as the panel sees them: a name and the chat's last digits.
    fn strangers(&self) -> Vec<Value>;
    /// The chat of one of them, by the panel's handle.
    fn stranger_chat(&self, handle: &str) -> Option<String>;
}

/// A connector's settings, and whether a call goes on through it now (a change would end it).
#[derive(Debug, Serialize)]
pub struct Settings {
    #[serde(flatten)]
    view: ConnectorSettingsView,
    busy: bool,
}

fn answer(parts: &Parts, id: ConnectorId) -> Settings {
    Settings { view: parts.connector_settings.view(id), busy: busy(parts, id) }
}

fn busy(parts: &Parts, id: ConnectorId) -> bool {
    let Some(calls) = &parts.calls else { return false };
    match id {
        ConnectorId::Sip => calls.busy_through("sip:"),
        ConnectorId::LiveKit => calls.busy_through("livekit:"),
        _ => false,
    }
}

pub async fn view(Uses(parts): Uses<Parts>, caller: Caller, Path(id): Path<String>) -> ApiResult<Settings> {
    manager(&caller)?;
    Ok(ok(answer(&parts, connector(&id)?)))
}

pub async fn update(
    Uses(parts): Uses<Parts>,
    caller: Caller,
    Path(id): Path<String>,
    Body(body): Body<Value>,
) -> ApiResult<Settings> {
    manager(&caller)?;
    let id = connector(&id)?;
    let Value::Object(mut input) = body else {
        return Err(ApiError::bad_request(crate::tr!("Нужен объект с полями.")));
    };
    if id == ConnectorId::Telegram {
        strangers_to_chats(&parts, &mut input)?;
    }
    parts.connector_settings.update(id, &input, caller.actor())?;
    Ok(ok(answer(&parts, id)))
}

/// A contact made of someone who wrote to the bot (`{name, stranger}`): their chat, which the panel never sees. The
/// station's build has no bot of its own to have heard them.
fn strangers_to_chats(parts: &Parts, input: &mut Map<String, Value>) -> Result<(), ApiError> {
    let Some(calls) = &parts.calls else { return Ok(()) };
    let Some(Value::Array(contacts)) = input.get_mut("contacts") else { return Ok(()) };
    for contact in contacts.iter_mut().filter_map(Value::as_object_mut) {
        let Some(stranger) = contact.remove("stranger") else { continue };
        let found = stranger.as_str().and_then(|handle| calls.stranger_chat(handle));
        let Some(chat) = found else {
            return Err(ApiError::bad_request(crate::tr!(
                "Этого чата уже нет среди написавших боту: откройте лист заново."
            )));
        };
        contact.insert("to".into(), Value::String(chat));
    }
    Ok(())
}

pub async fn clear(Uses(parts): Uses<Parts>, caller: Caller, Path(id): Path<String>) -> ApiResult<Settings> {
    manager(&caller)?;
    let id = connector(&id)?;
    parts.connector_settings.clear(id, caller.actor())?;
    Ok(ok(answer(&parts, id)))
}

async fn check(Uses(parts): Uses<Parts>, caller: Caller, Path(id): Path<String>) -> ApiResult<Settings> {
    manager(&caller)?;
    let id = connector(&id)?;
    Checker::new(parts.config.skills.weather.clone()).check(&parts.connector_settings, id).await;
    Ok(ok(answer(&parts, id)))
}

/// Who wrote to the bot lately and is no contact: a name and the chat's last digits.
async fn chats(Uses(parts): Uses<Parts>, caller: Caller) -> ApiResult<Vec<Value>> {
    manager(&caller)?;
    Ok(ok(parts.calls.as_ref().map(|calls| calls.strangers()).unwrap_or_default()))
}

pub async fn catalog(caller: Caller) -> ApiResult<NewsCatalog> {
    caller.permitted(CONNECTORS_VIEW)?;
    Ok(ok(NEWS_CATALOG.clone()))
}

#[derive(Debug, Default, Deserialize)]
struct PreviewBody {
    #[serde(default)]
    url: String,
}

async fn preview(
    Uses(config): Uses<Arc<Config>>,
    caller: Caller,
    Body(body): Body<PreviewBody>,
) -> ApiResult<FeedPreview> {
    manager(&caller)?;
    Ok(ok(Checker::new(config.skills.weather.clone()).preview(&body.url).await?))
}
