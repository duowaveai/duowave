//! The home's Telegram (docs/plans/home-and-devices.md, stage 2): a connection of the home, kept at its centre — the
//! token and the contacts are set in «Подключения» from any panel. A centre without a bot of its own (the station's
//! build) hands it to the Турбо that gives the messages (`POST /home/v1/messenger/bot`, src/home_server/serve.rs),
//! whenever it changes and whenever that Турбо comes back; the bot runs there, and «передай маме…» goes through it as
//! before (src/home_server/messenger.rs).
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::Parts;
use crate::core::events::EventBus;
use crate::helpers::contract::Ability;
use crate::platform::http::Uses;

fn tells(name: &str, connector: Option<&str>) -> bool {
    match name {
        "connector.changed" => connector == Some("telegram"),
        "helpers.up" | "helpers.changed" => true,
        _ => false,
    }
}

/// Starts handing the home's Telegram to the Турбо that runs the bot.
pub fn watch(events: &EventBus, parts: impl Fn() -> Option<Parts> + Clone + Send + Sync + 'static) {
    events.on("home.telegram", move |event| {
        let connector = match event.as_ref() {
            crate::core::events::GatewayEvent::ConnectorChanged { connector, .. } => Some(connector.as_str()),
            _ => None,
        };
        if !tells(event.name(), connector) {
            return;
        }
        let Some(home) = parts() else { return };
        // A computer runs a bot of its own (crates/duowave-pc): it keeps its Telegram to itself.
        if home.computer.is_some() {
            return;
        }
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Err(error) = hand(&home.helpers, &home.connector_settings).await {
                tracing::info!(target: "home", "Telegram to the Турбо: {error}");
            }
        });
    });
}

/// Hands the home's Telegram to the Турбо that gives the messages, when one is on the line.
pub async fn hand(
    helpers: &Arc<crate::helpers::Helpers>,
    connector_settings: &Arc<crate::connectors::settings::ConnectorSettings>,
) -> Result<(), String> {
    let Some(remote) = helpers.pick_remote(Ability::Messages) else { return Ok(()) };
    let (setting, token) = connector_settings.telegram_for_turbo();
    if token.is_none() && setting.contacts.is_empty() {
        return Ok(());
    }
    let mut body = json!(setting);
    body["token"] = json!(token);
    let response = crate::utils::http::client()
        .post(format!("{}/home/v1/messenger/bot", remote.base().trim_end_matches('/')))
        .bearer_auth(remote.credential())
        .json(&body)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    tracing::info!(target: "home", "the home's Telegram is with the Турбо");
    Ok(())
}

/// The routes: a copy holder's bot the home takes when it has none (`adopt`).
pub fn routes<S: crate::home::RouteNeeds>() -> axum::Router<S> {
    axum::Router::new().route("/api/v1/home/telegram-adopt", axum::routing::post(adopt))
}

/// At the centre, from a device of the home with the pair's key, when its first copy is taken: the Telegram bot that
/// device ran before it joined becomes the home's — only while the home has none; one the home has stays.
async fn adopt(
    Uses(parts): Uses<Parts>,
    caller: crate::platform::http::Caller,
    crate::platform::http::Body(body): crate::platform::http::Body<serde_json::Value>,
) -> crate::platform::http::ApiResult<serde_json::Value> {
    use crate::platform::http::{ok, ApiError};
    if !crate::placement::peer::from_peer(&caller, parts.placement.peer_key()) {
        return Err(ApiError::forbidden(crate::tr!("Бот дома передаёт только устройство этого дома с ключом пары.")));
    }
    let (_, home_token) = parts.connector_settings.telegram_for_turbo();
    if home_token.is_some_and(|token| !token.trim().is_empty()) {
        return Err(ApiError::conflict(crate::tr!("У дома уже есть свой бот Telegram: он и остаётся.")));
    }
    let token = body.get("token").and_then(serde_json::Value::as_str).unwrap_or_default().trim().to_string();
    if token.is_empty() {
        return Err(ApiError::bad_request("token".to_string()));
    }
    let setting: crate::settings::definitions::TelegramSetting =
        serde_json::from_value(body.clone()).unwrap_or_default();
    parts.connector_settings.take_home_telegram(&setting, Some(&token))?;
    tracing::info!(target: "home", "the home took the Telegram bot of a device that joined it");
    Ok(ok(json!({ "taken": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_goes_on_its_own_change_and_when_the_turbo_comes() {
        assert!(tells("connector.changed", Some("telegram")));
        assert!(!tells("connector.changed", Some("weather")));
        assert!(tells("helpers.up", None) && tells("helpers.changed", None));
        assert!(!tells("persona.changed", None));
    }
}
