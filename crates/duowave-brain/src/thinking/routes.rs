//! Thinking's API (docs/plans/thinking.md, «Everything is a setting», «Numbers for decisions»).
//!
//! - `GET /api/v1/thinking/settings` → `{settings, defaults, mark, schema}`: the setting `thinking` as it holds (the
//!   defaults where nothing is set), the defaults, the mark the turns' lines carry, the schema this gateway reads. For
//!   whoever may change the gateway's settings.
//! - `PUT /api/v1/thinking/settings` — the setting, whole or in part (every field optional); `{}` — back to the
//!   defaults. It holds from the next phrase. The answer is `GET`'s.
//! - `GET /api/v1/thinking/stats?from=&to=&device=&variant=&mode=` → the numbers of the turns' lines (stats.rs);
//!   `from` and `to` are ms since the epoch (the last 14 days by default), `variant` a card's or a rounds policy's,
//!   `mode` `voice` or `text`. For whoever may read the journal.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::Query;
use axum::routing::get;
use axum::Router;
use serde::Serialize;
use serde_json::Value;

use super::settings::{Settings, SCHEMA, THINKING};
use super::stats::{self, Stats};
use super::OurThinking;
use crate::core::identity::permissions::{JOURNAL_VIEW, SETTINGS_EDIT};
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Provides, RouteState, Uses};
use crate::settings::SettingsService;

const DAY_MS: u64 = 86_400_000;

pub fn routes<S>() -> Router<S>
where
    S: RouteState + Provides<Arc<OurThinking>> + Provides<Arc<SettingsService>>,
{
    Router::new()
        .route("/api/v1/thinking/settings", get(settings_view).put(set_settings))
        .route("/api/v1/thinking/stats", get(stats_view))
}

#[derive(Debug, Serialize)]
struct SettingsView {
    settings: Settings,
    defaults: Settings,
    mark: String,
    schema: u32,
}

fn view_of(settings: &SettingsService) -> SettingsView {
    let holds = Settings::of(settings);
    SettingsView { mark: holds.mark(), settings: holds, defaults: Settings::default(), schema: SCHEMA }
}

async fn settings_view(Uses(settings): Uses<Arc<SettingsService>>, caller: Caller) -> ApiResult<SettingsView> {
    caller.permitted(SETTINGS_EDIT)?;
    Ok(ok(view_of(&settings)))
}

async fn set_settings(
    Uses(settings): Uses<Arc<SettingsService>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<SettingsView> {
    caller.permitted(SETTINGS_EDIT)?;
    let empty = body.as_object().is_some_and(|object| object.is_empty()) || body.is_null();
    if empty {
        settings.forget(&THINKING)?;
        return Ok(ok(view_of(&settings)));
    }
    let parsed: Settings = serde_json::from_value(body).map_err(|error| {
        ApiError::bad_request(crate::tr!("Настройку «Мышления» не прочитать: {error}", error = error.to_string()))
    })?;
    if parsed.schema > SCHEMA {
        return Err(ApiError::bad_request(crate::tr!(
            "Схема {schema} новее этой версии шлюза.",
            schema = parsed.schema.to_string()
        )));
    }
    settings.set(&THINKING, &parsed, caller.actor())?;
    Ok(ok(view_of(&settings)))
}

fn number(query: &HashMap<String, String>, key: &str) -> Option<u64> {
    query.get(key).and_then(|value| value.trim().parse().ok())
}

async fn stats_view(
    Uses(thinking): Uses<Arc<OurThinking>>,
    caller: Caller,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Stats> {
    caller.permitted(JOURNAL_VIEW)?;
    let now = crate::utils::time::now_ms();
    let query = stats::Query {
        from: Some(number(&query, "from").unwrap_or(now.saturating_sub(14 * DAY_MS))),
        to: number(&query, "to"),
        device: query.get("device").filter(|device| !device.is_empty()).cloned(),
        variant: query.get("variant").filter(|variant| !variant.is_empty()).cloned(),
        mode: query.get("mode").filter(|mode| !mode.is_empty()).cloned(),
    };
    Ok(ok(thinking.stats(&query)))
}
