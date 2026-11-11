//! The station's «Голос» while the computer gives it its voice (docs/plans/desk-after-station.md, В): the station's
//! build has no model shop, so its panel's window offers «Голос компьютера (имя) / Алиса» here —
//! `GET|POST /api/v1/models/home-voice`. The choice is the station's own (`tts_provider`, each side's own in
//! src/placement/transfer.rs); a move with the computer as the station's server brings the computer's.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::config::TtsProvider;
use crate::core::availability::HomeServer;
use crate::core::identity::permissions::{MODELS_MANAGE, STATION_VIEW};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller};
use crate::settings::definitions::VOICE_PROVIDER;

/// How long the computer's name for its voice is waited for: it answers at home in milliseconds, or it is away.
const ASK_FOR: Duration = Duration::from_secs(3);

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/models/home-voice", get(view).post(choose))
}

/// The choice as the station's «Голос» window shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeVoiceView {
    /// The computer gives this gateway its voice: a station whose home server helps it, the voice not left out
    /// («Что даю колонке»).
    pub offered: bool,
    /// The computer answers now.
    pub up: bool,
    /// The computer's voice by its name («Baya»), as its home server says it; none while it is away, or when it says
    /// no name (a voice of its voice lab is known by an id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// The station's choice: «computer» or «alice».
    pub chosen: &'static str,
    /// The provider chosen: `own` — Yandex's answers in the computer's voice too, `mixed` — ours only.
    pub provider: &'static str,
}

fn offered(app: &App) -> bool {
    let status = app.home_server.status();
    app.voice.has_home() && status.configured && status.enabled && !status.without.iter().any(|part| part == "tts")
}

fn chosen(app: &App) -> &'static str {
    match app.voice.chosen_provider() {
        TtsProvider::Own | TtsProvider::Mixed => "computer",
        TtsProvider::Yandex | TtsProvider::None => "alice",
    }
}

async fn current(app: &App) -> HomeVoiceView {
    let offered = offered(app);
    let up = offered && app.home_server.state() == HomeServer::Up;
    let voice = if up { computers_voice(app).await } else { None };
    HomeVoiceView { offered, up, voice, chosen: chosen(app), provider: app.voice.chosen_provider().as_str() }
}

/// The computer's voice as its home server names it (`/home/v1/status`: «silero/baya»), by name.
async fn computers_voice(app: &App) -> Option<String> {
    let (url, key) = app.home_server.configured()?;
    let answer = crate::utils::http::client()
        .get(format!("{}/home/v1/status", url.trim_end_matches('/')))
        .bearer_auth(key)
        .timeout(ASK_FOR)
        .send()
        .await
        .ok()?;
    let status: Value = answer.json().await.ok()?;
    voice_name(status.get("voice")?.as_str()?)
}

/// A voice's name for the household out of the pair the voice server knows: «silero/baya» — «Baya»,
/// «piper/ru_RU-irina-medium» — «Irina». An id (a voice of the voice lab) is no name: None.
pub fn voice_name(pair: &str) -> Option<String> {
    let voice = pair.rsplit('/').next()?.trim();
    let parts: Vec<&str> = voice.split('-').collect();
    let word = match parts.as_slice() {
        [locale, name, _quality] if locale.contains('_') => *name,
        [name] => *name,
        _ => return None,
    };
    let named = (2..=24).contains(&word.chars().count()) && word.chars().all(|c| c.is_alphabetic() || c == '_');
    if !named {
        return None;
    }
    let word = word.replace('_', " ");
    let mut chars = word.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

async fn view(State(app): State<Arc<App>>, caller: Caller) -> ApiResult<HomeVoiceView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(current(&app).await))
}

/// `{"voice": "computer"}` — the computer's voice (ours in it, Yandex's in Alice's unless the computer gave `own`);
/// `{"voice": "alice"}` — Alice's. Only a station with a home server: the computer's own gateway chooses its voice in
/// the shop.
async fn choose(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<HomeVoiceView> {
    caller.permitted(MODELS_MANAGE)?;
    if !app.voice.has_home() {
        return Err(ApiError::conflict("Голос этого шлюза выбирают в каталоге голосов."));
    }
    let actor = caller.actor();
    match body.get("voice").and_then(Value::as_str) {
        Some("alice") => {
            app.voice.use_yandex();
            app.settings.set(&VOICE_PROVIDER, &TtsProvider::Yandex.as_str().to_string(), actor)?;
            tracing::info!(target: "voice", "the station speaks Alice's voice (its «Голос»)");
        }
        Some("computer") => {
            if !offered(&app) {
                return Err(ApiError::conflict(
                    "Компьютер не даёт колонке голос: включите «Помогать колонке» в пульте компьютера.",
                ));
            }
            let provider = app.voice.use_home().map_err(ApiError::internal)?;
            app.settings.set(&VOICE_PROVIDER, &provider.as_str().to_string(), actor)?;
            tracing::info!(target: "voice", "the station speaks the computer's voice ({}, its «Голос»)", provider.as_str());
        }
        _ => return Err(ApiError::bad_request(r#"body: {"voice": "computer"} или {"voice": "alice"}"#)),
    }
    Ok(created(current(&app).await))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_a_voice_by_its_name_not_by_its_id() {
        assert_eq!(voice_name("silero/baya").as_deref(), Some("Baya"));
        assert_eq!(voice_name("piper/ru_RU-irina-medium").as_deref(), Some("Irina"));
        assert_eq!(voice_name("silero/ксения").as_deref(), Some("Ксения"));
        assert_eq!(voice_name("omnivoice/3f2a9c1e-77b0-4c1d-9a55-0c1d2e3f4a5b"), None, "a voice lab's id");
        assert_eq!(voice_name("cosyvoice3/v42"), None);
    }
}
