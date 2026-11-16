//! The station's «Голос» while a helper gives it its voice (docs/plans/desk-after-station.md, В; the helpers,
//! docs/plans/home-and-helpers.md): the station's build has no model shop, so its panel's window offers «Голос
//! компьютера (имя) / Алиса» here — `GET|POST /api/v1/models/home-voice`. The choice is the station's own
//! (`tts_provider`, each side's own in src/placement/transfer.rs); a move with the computer as the station's server
//! brings the computer's. Without a helper, with a cloud voice set up (src/speech/cloud_speech.rs,
//! docs/plans/speaker-alone.md): «Облачный голос / Алиса». In block 1 the helper that gives a station its voice is a
//! computer: the words say so.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::config::TtsProvider;
use crate::core::identity::permissions::{MODELS_MANAGE, STATION_VIEW};
use crate::helpers::contract::Ability;
use crate::helpers::Giver;
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller};
use crate::settings::definitions::VOICE_PROVIDER;

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/models/home-voice", get(view).post(choose))
}

/// The choice as the station's «Голос» window shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeVoiceView {
    /// A helper gives this gateway its voice: a station whose helpers would give it (the voice not left out).
    pub offered: bool,
    /// The helper gives it now.
    pub up: bool,
    /// The helper's voice by its name («Baya»), as its last status said it; none while it is away, or when it says no
    /// name (a voice of its voice lab is known by an id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// A cloud voice is set up on the station (src/speech/cloud_speech.rs): without the computer it speaks.
    pub cloud: bool,
    /// The station's choice: «computer», «cloud» or «alice».
    pub chosen: &'static str,
    /// The provider chosen: `own` — Yandex's answers in the helper's voice too, `mixed` — ours only.
    pub provider: &'static str,
}

fn offered(app: &App) -> bool {
    app.voice.has_home() && matches!(app.helpers.offer(Ability::Voice), Some(Giver::Helper(_)))
}

fn chosen(app: &App) -> &'static str {
    match app.voice.chosen_provider() {
        TtsProvider::Own | TtsProvider::Mixed if !offered(app) && app.voice.cloud_speech().is_some() => "cloud",
        TtsProvider::Own | TtsProvider::Mixed => "computer",
        TtsProvider::Yandex | TtsProvider::None => "alice",
    }
}

fn current(app: &App) -> HomeVoiceView {
    let offered = offered(app);
    let giving = app.helpers.pick_remote(Ability::Voice).filter(|_| offered);
    // Its voice as its last status named it: no extra request.
    let voice = giving.as_ref().and_then(|helper| helper.status()).and_then(|status| {
        let pair = status.contract.get(Ability::Voice)?.details.get("voice")?.as_str()?.to_string();
        voice_name(&pair)
    });
    HomeVoiceView {
        offered,
        up: giving.is_some(),
        voice,
        cloud: app.voice.cloud_speech().is_some(),
        chosen: chosen(app),
        provider: app.voice.chosen_provider().as_str(),
    }
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
    Ok(ok(current(&app)))
}

/// `{"voice": "computer"}` — the computer's voice (ours in it, Yandex's in Alice's unless the computer gave `own`);
/// `{"voice": "alice"}` — Alice's. Only a station with a home server: the computer's own gateway chooses its voice in
/// the shop.
async fn choose(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Value>) -> ApiResult<HomeVoiceView> {
    caller.permitted(MODELS_MANAGE)?;
    if !app.voice.has_home() {
        return Err(ApiError::conflict(crate::tr!("Голос этого шлюза выбирают в каталоге голосов.")));
    }
    let actor = caller.actor();
    match body.get("voice").and_then(Value::as_str) {
        Some("cloud") => {
            let provider = app.voice.use_cloud().map_err(|_| {
                ApiError::conflict(crate::tr!("Облачный голос не настроен: выберите сервис и ключ ниже."))
            })?;
            app.settings.set(&VOICE_PROVIDER, &provider.as_str().to_string(), actor)?;
            tracing::info!(target: "voice", "the station speaks its cloud voice ({}, its «Голос»)", provider.as_str());
        }
        Some("alice") => {
            app.voice.use_yandex();
            app.settings.set(&VOICE_PROVIDER, &TtsProvider::Yandex.as_str().to_string(), actor)?;
            tracing::info!(target: "voice", "the station speaks Alice's voice (its «Голос»)");
        }
        Some("computer") => {
            if !offered(&app) {
                return Err(ApiError::conflict(crate::tr!(
                    "Компьютер не даёт колонке голос: в его пульте включите «Помогать» у этой колонки («Настройки» → «Этот компьютер» → «Помогаю»)."
                )));
            }
            let provider = app.voice.use_home().map_err(ApiError::internal)?;
            app.settings.set(&VOICE_PROVIDER, &provider.as_str().to_string(), actor)?;
            tracing::info!(target: "voice", "the station speaks the computer's voice ({}, its «Голос»)", provider.as_str());
        }
        _ => {
            return Err(ApiError::bad_request(crate::tr!(
                "body: {\"voice\": \"computer\"}, {\"voice\": \"cloud\"} или {\"voice\": \"alice\"}"
            )))
        }
    }
    Ok(created(current(&app)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env};
    use crate::helpers::tests::{stand, KEY};
    use crate::helpers::{AddInput, HelperKind};

    fn gateway(dir: &std::path::Path, role: &str) -> Arc<App> {
        let data = dir.to_string_lossy().into_owned();
        let env = Env::from_pairs([
            ("STATE_DIR", ""),
            ("DATA_DIR", data.as_str()),
            ("PLUGINS_BUNDLED", "false"),
            ("DUOWAVE_ROLE", role),
        ]);
        App::build(Config::for_tests(&env)).unwrap()
    }

    /// The window offers the voice of the helper that gives it — not only the old «computer» — and names it as its
    /// last status did; without a helper it offers nothing; a computer never leans on its helpers for its voice.
    #[tokio::test]
    async fn the_voice_window_offers_the_helper_that_gives_the_voice() {
        let status = serde_json::json!({
            "version": "0.12.0", "peerApi": 2,
            "contract": { "version": 1, "helper": { "id": "srv", "name": "Мой сервер" }, "abilities": [
                { "id": "voice", "ready": true, "details": { "voice": "silero/baya" } },
                { "id": "hearing", "ready": true }
            ] }
        });
        let server = stand(status).await;
        let dir = tempfile::tempdir().unwrap();
        let station = gateway(dir.path(), "station");
        let alone = current(&station);
        assert!(!alone.offered && !alone.up && alone.voice.is_none(), "no helper, no voice to offer");
        let own =
            AddInput { url: server.url.clone(), key: KEY.into(), kind: Some(HelperKind::Own), ..AddInput::default() };
        station.helpers.add(own, None).await.unwrap();
        assert!(station.helpers.entry("home-computer").is_none(), "one's own server, not the computer");
        let view = current(&station);
        assert_eq!((view.offered, view.up, view.voice.as_deref()), (true, true, Some("Baya")));

        let other = tempfile::tempdir().unwrap();
        let computer = gateway(other.path(), "pc");
        computer.voice.set_home(computer.helpers.clone());
        assert!(!computer.voice.has_home() && !offered(&computer), "its own voice window, as before");
    }

    #[test]
    fn names_a_voice_by_its_name_not_by_its_id() {
        assert_eq!(voice_name("silero/baya").as_deref(), Some("Baya"));
        assert_eq!(voice_name("piper/ru_RU-irina-medium").as_deref(), Some("Irina"));
        assert_eq!(voice_name("silero/ксения").as_deref(), Some("Ксения"));
        assert_eq!(voice_name("omnivoice/3f2a9c1e-77b0-4c1d-9a55-0c1d2e3f4a5b"), None, "a voice lab's id");
        assert_eq!(voice_name("cosyvoice3/v42"), None);
    }
}
