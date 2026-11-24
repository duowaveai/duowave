//! The station's «Голос» while a helper gives it its voice (docs/plans/desk-after-station.md, В; the helpers,
//! docs/plans/home-and-helpers.md): the station's build has no model shop, so its panel's window offers «Голос
//! компьютера (имя) / Алиса» here — `GET|POST /api/v1/models/home-voice`. The choice is the station's own
//! (`tts_provider`, each side's own in src/placement/transfer.rs); a move with the computer as the station's server
//! brings the computer's. Without a helper, with a cloud voice set up (src/speech/cloud_speech.rs,
//! docs/plans/speaker-alone.md): «Облачный голос / Алиса». In block 1 the helper that gives a station its voice is a
//! computer: the words say so.

use std::sync::Arc;

use axum::routing::get;
use axum::Router;
use serde::Serialize;
use serde_json::Value;

use super::contract::Ability;
use super::{Giver, Helpers};
use crate::config::TtsProvider;
use crate::core::events::EventBus;
use crate::core::identity::permissions::{MODELS_MANAGE, STATION_VIEW};
use crate::platform::http::{created, ok, ApiError, ApiResult, Body, Caller, Uses};
use crate::settings::definitions::{HOME_VOICE_FOLLOW, PERSONA, VOICE_PROVIDER};
use crate::settings::SettingsService;
use crate::speech::VoiceService;

crate::route_needs!(HomeVoice);

/// «Голос» of a station whose home server gives it the computer's voice, and the panel's choice (the gateway adds them).
pub fn routes<S: RouteNeeds>() -> Router<S> {
    Router::new().route("/api/v1/models/home-voice", get(view).post(choose))
}

/// What the home's voice works with: the voice, the helpers that give one, the settings its marks live in, and a
/// character's own voice. The gateway makes it (src/links/http.rs) for the routes and the watch.
#[derive(Clone)]
pub struct HomeVoice {
    pub voice: Arc<VoiceService>,
    pub helpers: Arc<Helpers>,
    pub settings: Arc<SettingsService>,
    /// The package of a character's own voice, by the character's id: asked of the characters (a floor above,
    /// gateway/layers.json) through the gateway.
    pub package_of: PackageOf,
}

/// A character's id → the package of its own voice.
pub type PackageOf = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The choice as the station's «Голос» window shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeVoiceView {
    /// A helper gives this gateway its voice: a station whose helpers would give it (the voice not left out).
    pub offered: bool,
    /// The helper gives it now.
    pub up: bool,
    /// A helper that would give it is on the line, its voice ready or not: «на связи, а голоса ещё нет» is no «не на
    /// связи» (clean-path check 18.11, item 12: the Mac was on the line without a voice of its own).
    pub reached: bool,
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

pub fn offered(home: &HomeVoice) -> bool {
    home.voice.has_home() && matches!(home.helpers.offer(Ability::Voice), Some(Giver::Helper(_)))
}

fn chosen(home: &HomeVoice) -> &'static str {
    match home.voice.chosen_provider() {
        TtsProvider::Own | TtsProvider::Mixed if !offered(home) && home.voice.cloud_speech().is_some() => "cloud",
        TtsProvider::Own | TtsProvider::Mixed => "computer",
        TtsProvider::Yandex | TtsProvider::None => "alice",
    }
}

pub fn current(home: &HomeVoice) -> HomeVoiceView {
    let offered = offered(home);
    let giving = home.helpers.pick_remote(Ability::Voice).filter(|_| offered);
    let reached =
        giving.is_some() || (offered && home.helpers.offering(Ability::Voice).iter().any(|helper| helper.up()));
    // Its voice as its last status named it: no extra request.
    let voice = giving.as_ref().and_then(|helper| helper.status()).and_then(|status| {
        let pair = status.contract.get(Ability::Voice)?.details.get("voice")?.as_str()?.to_string();
        voice_name(&pair)
    });
    HomeVoiceView {
        offered,
        up: giving.is_some(),
        reached,
        voice,
        cloud: home.voice.cloud_speech().is_some(),
        chosen: chosen(home),
        provider: home.voice.chosen_provider().as_str(),
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

/// The follow mark: the character it is for (and its package), and whether it still waits to switch.
fn follow_mark(home: &HomeVoice) -> (Option<String>, Option<String>, bool) {
    let mark = home.settings.get(&HOME_VOICE_FOLLOW, None).unwrap_or_default();
    (mark.get("character").cloned(), mark.get("package").cloned(), mark.get("armed").map(String::as_str) == Some("1"))
}

fn set_follow(home: &HomeVoice, character: Option<&str>, package: Option<&str>, armed: bool) {
    let mut mark = std::collections::BTreeMap::new();
    if let Some(character) = character {
        mark.insert("character".to_string(), character.to_string());
    }
    if let Some(package) = package.filter(|package| !package.is_empty()) {
        mark.insert("package".to_string(), package.to_string());
    }
    mark.insert("armed".to_string(), if armed { "1" } else { "0" }.to_string());
    if let Err(error) = home.settings.set(&HOME_VOICE_FOLLOW, &mark, None) {
        tracing::warn!(target: "voice", "the voice's follow mark is not kept: {}", error.message);
    }
}

/// The home's character changed (clean-path check 18.11, item 70): one with a voice of its own (a package) — the
/// centre will switch to the computer's voice once that voice is up on the Турбо; the same character again (its
/// manner changed) leaves the mark as it is, so the owner's own «Алиса» after the choice stays.
pub fn character_chosen(home: &HomeVoice, preset_id: Option<&str>) {
    let (marked, _, _) = follow_mark(home);
    if marked.as_deref() == preset_id {
        return;
    }
    let package = preset_id.and_then(|id| (home.package_of)(id));
    follow(home, preset_id, package.as_deref());
}

/// A new mark: a character with a package waits for its voice; one without (a built-in) — nothing to wait for.
pub fn follow(home: &HomeVoice, character: Option<&str>, package: Option<&str>) {
    set_follow(home, character, package, package.is_some_and(|package| !package.is_empty()));
}

/// The owner chose the home's voice by hand: the centre does not switch it any more for this character.
pub fn chosen_by_hand(home: &HomeVoice) {
    let (marked, package, armed) = follow_mark(home);
    if armed {
        set_follow(home, marked.as_deref(), package.as_deref(), false);
    }
}

/// Switches the station to the computer's voice when the home's character was chosen with its voice and that voice
/// speaks on the Турбо now (its status names the character's own voice) — the owner did not choose a voice by hand
/// since (clean-path check 18.11, item 70: the voice came up after the first steps had ended, and the station stayed on
/// Alice's until the owner switched it). true — switched.
pub fn follow_voice(home: &HomeVoice) -> bool {
    let (Some(id), Some(package), true) = follow_mark(home) else { return false };
    if !home.voice.has_home() {
        return false;
    }
    // Still the home's character: another one was chosen since — its own mark decides.
    let current = home.settings.get(&PERSONA, None).and_then(|persona| persona.preset_id);
    if current.as_deref() != Some(id.as_str()) {
        return false;
    }
    if chosen(home) == "computer" {
        set_follow(home, Some(&id), Some(&package), false);
        return false;
    }
    if !offered(home) {
        return false;
    }
    let Some(giving) = home.helpers.pick_remote(Ability::Voice) else { return false };
    let pair = giving
        .status()
        .and_then(|status| status.contract.get(Ability::Voice)?.details.get("voice")?.as_str().map(str::to_string));
    let theirs = format!("c-{package}-");
    let Some(pair) = pair.filter(|pair| pair.contains(&theirs)) else { return false };
    let provider = match home.voice.use_home() {
        Ok(provider) => provider,
        Err(error) => {
            tracing::warn!(target: "voice", "the character's voice is up, the computer's voice not taken: {error}");
            return false;
        }
    };
    if let Err(error) = home.settings.set(&VOICE_PROVIDER, &provider.as_str().to_string(), None) {
        tracing::warn!(target: "voice", "the computer's voice is not kept: {}", error.message);
    }
    set_follow(home, Some(&id), Some(&package), false);
    tracing::info!(
        target: "voice",
        "the station speaks the computer's voice ({}): the character {id}'s voice is up on the Турбо ({pair}) — switched by itself",
        provider.as_str()
    );
    true
}

/// Watches for the moment to switch: the home's character changing (arms), and the Турбо's voice — at each of its
/// looks (a new contract, up) and every few seconds besides (a voice that changed in its status alone). `home` gives
/// the parts while the gateway lives (it holds the gateway weakly: the bus it listens on is the gateway's).
pub fn watch_follow(events: &EventBus, home: impl Fn() -> Option<HomeVoice> + Clone + Send + Sync + 'static) {
    let parts = home.clone();
    events.on("home.voice.follow", move |event| {
        let Some(home) = parts() else { return };
        if let crate::core::events::GatewayEvent::PersonaChanged { preset_id, .. } = event.as_ref() {
            character_chosen(&home, preset_id.as_deref());
        }
        if matches!(event.name(), "persona.changed" | "helpers.up" | "helpers.changed") {
            follow_voice(&home);
        }
    });
    tokio::spawn(async move {
        let mut every = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            every.tick().await;
            let Some(home) = home() else { return };
            follow_voice(&home);
        }
    });
}

async fn view(Uses(home): Uses<HomeVoice>, caller: Caller) -> ApiResult<HomeVoiceView> {
    caller.permitted(STATION_VIEW)?;
    Ok(ok(current(&home)))
}

/// `{"voice": "computer"}` — the computer's voice (ours in it, Yandex's in Alice's unless the computer gave `own`);
/// `{"voice": "alice"}` — Alice's. Only a station with a home server: the computer's own gateway chooses its voice in
/// the shop.
async fn choose(Uses(home): Uses<HomeVoice>, caller: Caller, Body(body): Body<Value>) -> ApiResult<HomeVoiceView> {
    caller.permitted(MODELS_MANAGE)?;
    if !home.voice.has_home() {
        return Err(ApiError::conflict(crate::tr!("Голос здесь выбирают в каталоге голосов.")));
    }
    let actor = caller.actor();
    // The owner's own choice: the centre does not switch it for the character any more (item 70).
    if matches!(body.get("voice").and_then(Value::as_str), Some("cloud" | "alice" | "computer")) {
        chosen_by_hand(&home);
    }
    match body.get("voice").and_then(Value::as_str) {
        Some("cloud") => {
            let provider = home.voice.use_cloud().map_err(|_| {
                ApiError::conflict(crate::tr!("Облачный голос не настроен: выберите сервис и ключ ниже."))
            })?;
            home.settings.set(&VOICE_PROVIDER, &provider.as_str().to_string(), actor)?;
            tracing::info!(target: "voice", "the station speaks its cloud voice ({}, its «Голос»)", provider.as_str());
        }
        Some("alice") => {
            home.voice.use_yandex();
            home.settings.set(&VOICE_PROVIDER, &TtsProvider::Yandex.as_str().to_string(), actor)?;
            tracing::info!(target: "voice", "the station speaks Alice's voice (its «Голос»)");
        }
        Some("computer") => {
            if !offered(&home) {
                return Err(ApiError::conflict(crate::tr!(
                    "Компьютер не даёт колонке голос: в его пульте включите «Помогать» у этой колонки («Настройки» → «Этот компьютер» → «Помогаю»)."
                )));
            }
            let provider = home.voice.use_home().map_err(ApiError::internal)?;
            home.settings.set(&VOICE_PROVIDER, &provider.as_str().to_string(), actor)?;
            tracing::info!(target: "voice", "the station speaks the computer's voice ({}, its «Голос»)", provider.as_str());
        }
        _ => {
            return Err(ApiError::bad_request(crate::tr!(
                "body: {\"voice\": \"computer\"}, {\"voice\": \"cloud\"} или {\"voice\": \"alice\"}"
            )))
        }
    }
    Ok(created(current(&home)))
}
