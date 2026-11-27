//! The voice channel's routes (docs/plans/audio-interface-3.md, «К2», «API»):
//!
//! - `GET /api/v1/talk/voice` — whether this gateway has the computer's ear (`port`), whether this panel is on this
//!   computer (`here`), what the channel does, its switches, the key and whether another program holds it, the echo
//!   check's result, the microphone's access and whether a call holds it.
//! - `PUT /api/v1/talk/voice` — the switches (`sounds`, `bargeIn`, `strip`, `word`) and the key (`key`); the admin's
//!   (`station.mode`), as the choice of «Как говорить».
//! - `POST /api/v1/talk/voice/press`, `…/release` (`{heldMs}`), `…/stop` — only from this computer (zone `Local`) and
//!   with the panel's microphone right (`station.say`): a phone cannot open the computer's microphone.
//! - `POST /api/v1/talk/voice/check` — the echo check: from this computer, the admin's.
//! - `POST /api/v1/talk/voice/panel` — the strip asks for the panel's window (from this computer).
//!
//! The person who presses «Сказать» on this computer's panel, or opens its settings, is who speaks into its microphone
//! from then on (decision 3): their talk and their memory. Nobody — the household's voice.

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use serde::Serialize;
use serde_json::Value;

use super::{State, View, VoiceChannel};
use crate::core::identity::permissions::{STATION_MODE, STATION_SAY, STATION_VIEW};
use crate::core::identity::PrincipalKind;
use crate::ear::port::Access;
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller, Uses, Zone};
use crate::settings::definitions::{EchoCheck, KeyActivation, TalkVoice, DEFAULT_TALK_KEY, TALK_KEY, TALK_VOICE};
use crate::settings::SettingsService;

/// The gateway's voice channel, when its app gave it an ear.
#[derive(Clone, Default)]
pub struct VoiceHandle(pub Option<Arc<VoiceChannel>>);

crate::route_needs!(VoiceHandle, Arc<SettingsService>);

pub fn routes<S: RouteNeeds>() -> Router<S> {
    Router::new()
        .route("/api/v1/talk/voice", get(view).put(set))
        .route("/api/v1/talk/voice/press", post(press))
        .route("/api/v1/talk/voice/release", post(release))
        .route("/api/v1/talk/voice/stop", post(stop))
        .route("/api/v1/talk/voice/check", post(check))
        .route("/api/v1/talk/voice/panel", post(panel))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceView {
    /// The gateway has the computer's ear (the app's port).
    pub port: bool,
    /// This panel is on this computer: the ear and the voice are the app's, the panel only shows.
    pub here: bool,
    pub state: View,
    pub key: String,
    /// Another program holds the key: another one is chosen.
    pub key_taken: bool,
    pub sounds: bool,
    pub barge_in: bool,
    pub strip: bool,
    pub word: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub echo: Option<EchoCheck>,
    /// The system cancels the computer's own sound in the capture.
    pub cancels_echo: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
    /// Another program holds the microphone: a call.
    pub busy: bool,
    /// An activator app stands on this computer: «Слушать слово» can be switched on.
    pub word_app: bool,
    /// How the key drives a thought.
    pub activation: KeyActivation,
    /// A thought the key does not hold ends when the person is done.
    pub auto_end: bool,
}

fn no_ear() -> ApiError {
    ApiError::conflict(crate::tr!("У этого шлюза нет уха: компьютер слышит и говорит сам в приложении DuoWave на Mac."))
}

fn channel_of(handle: &VoiceHandle) -> Result<&Arc<VoiceChannel>, ApiError> {
    handle.0.as_ref().ok_or_else(no_ear)
}

/// A person signed in on this computer's panel is who speaks into its microphone from now on.
fn note_person(channel: &VoiceChannel, caller: &Caller) {
    if caller.zone != Zone::Local {
        return;
    }
    if let Some(principal) = caller.principal().filter(|principal| principal.kind == PrincipalKind::User) {
        channel.set_person(&principal.id, &principal.name);
    }
}

fn view_of(handle: &VoiceHandle, settings: &SettingsService, caller: &Caller) -> VoiceView {
    let voice: TalkVoice = settings.get(&TALK_VOICE, None).unwrap_or_default();
    let key = settings.get(&TALK_KEY, None).unwrap_or_else(|| DEFAULT_TALK_KEY.to_string());
    match &handle.0 {
        Some(channel) => VoiceView {
            port: true,
            here: caller.zone == Zone::Local,
            state: channel.view(),
            key,
            key_taken: channel.key_taken(),
            sounds: voice.sounds,
            barge_in: voice.barge_in,
            strip: voice.strip,
            word: voice.word,
            echo: voice.echo,
            cancels_echo: channel.port().cancels_echo(),
            access: Some(channel.port().access()),
            busy: channel.port().mic_busy(),
            word_app: channel.word_app(),
            activation: voice.activation,
            auto_end: voice.auto_end,
        },
        None => VoiceView {
            port: false,
            here: false,
            state: View { state: State::Quiet, ..View::default() },
            key,
            key_taken: false,
            sounds: voice.sounds,
            barge_in: voice.barge_in,
            strip: voice.strip,
            word: voice.word,
            echo: voice.echo,
            cancels_echo: false,
            access: None,
            busy: false,
            word_app: false,
            activation: voice.activation,
            auto_end: voice.auto_end,
        },
    }
}

async fn view(
    Uses(handle): Uses<VoiceHandle>,
    Uses(settings): Uses<Arc<SettingsService>>,
    caller: Caller,
) -> ApiResult<VoiceView> {
    caller.permitted(STATION_VIEW)?;
    if let Some(channel) = &handle.0 {
        note_person(channel, &caller);
    }
    Ok(ok(view_of(&handle, &settings, &caller)))
}

/// What a key may be called: a modifier or more and a key, `+` between (`Alt+Space`, `Control+Alt+K`).
pub fn valid_key(key: &str) -> bool {
    let parts: Vec<&str> = key.split('+').map(str::trim).collect();
    let modifiers = ["Alt", "Option", "Control", "Ctrl", "Shift", "Command", "Cmd", "Super", "CommandOrControl"];
    parts.len() >= 2
        && parts.len() <= 4
        && parts[..parts.len() - 1].iter().all(|part| modifiers.contains(part))
        && parts.last().is_some_and(|last| {
            !last.is_empty() && !modifiers.contains(last) && last.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

async fn set(
    Uses(handle): Uses<VoiceHandle>,
    Uses(settings): Uses<Arc<SettingsService>>,
    caller: Caller,
    Body(body): Body<Value>,
) -> ApiResult<VoiceView> {
    caller.permitted(STATION_MODE)?;
    let mut voice: TalkVoice = settings.get(&TALK_VOICE, None).unwrap_or_default();
    let flag = |name: &str| body.get(name).and_then(Value::as_bool);
    if let Some(sounds) = flag("sounds") {
        voice.sounds = sounds;
    }
    if let Some(barge_in) = flag("bargeIn") {
        voice.barge_in = barge_in;
    }
    if let Some(strip) = flag("strip") {
        voice.strip = strip;
    }
    if let Some(word) = flag("word") {
        voice.word = word;
    }
    if let Some(auto_end) = flag("autoEnd") {
        voice.auto_end = auto_end;
    }
    if let Some(activation) = body.get("activation") {
        voice.activation = serde_json::from_value(activation.clone())
            .map_err(|_| ApiError::bad_request(crate::tr!("Нет такого способа держать клавишу.")))?;
    }
    settings.set(&TALK_VOICE, &voice, caller.actor())?;
    if let Some(key) = body.get("key").and_then(Value::as_str).map(str::trim) {
        if !valid_key(key) {
            return Err(ApiError::bad_request(crate::tr!(
                "Такую клавишу не выбрать: нужно сочетание, например ⌥Пробел (Alt+Space)."
            )));
        }
        settings.set(&TALK_KEY, &key.to_string(), caller.actor())?;
        if let Some(channel) = &handle.0 {
            channel.key_changed(key);
        }
    }
    if let Some(channel) = &handle.0 {
        channel.settings_changed();
    }
    Ok(ok(view_of(&handle, &settings, &caller)))
}

/// The computer's microphone is opened from this computer only, with the panel's microphone right.
fn local_say<'a>(handle: &'a VoiceHandle, caller: &Caller) -> Result<&'a Arc<VoiceChannel>, ApiError> {
    caller.zone_at_least(Zone::Local)?;
    caller.permitted(STATION_SAY)?;
    let channel = channel_of(handle)?;
    note_person(channel, caller);
    Ok(channel)
}

#[derive(Debug, Serialize)]
pub struct Done {
    pub state: View,
}

async fn press(Uses(handle): Uses<VoiceHandle>, caller: Caller) -> ApiResult<Done> {
    let channel = local_say(&handle, &caller)?;
    channel.press();
    Ok(ok(Done { state: channel.view() }))
}

async fn release(Uses(handle): Uses<VoiceHandle>, caller: Caller, Body(body): Body<Value>) -> ApiResult<Done> {
    let channel = local_say(&handle, &caller)?;
    channel.release(body.get("heldMs").and_then(Value::as_u64).unwrap_or(0));
    Ok(ok(Done { state: channel.view() }))
}

async fn stop(Uses(handle): Uses<VoiceHandle>, caller: Caller) -> ApiResult<Done> {
    let channel = local_say(&handle, &caller)?;
    channel.stop();
    Ok(ok(Done { state: channel.view() }))
}

async fn check(Uses(handle): Uses<VoiceHandle>, caller: Caller) -> ApiResult<super::Checked> {
    caller.zone_at_least(Zone::Local)?;
    caller.permitted(STATION_MODE)?;
    let channel = channel_of(&handle)?;
    let checked = channel.check().await.map_err(ApiError::conflict)?;
    Ok(ok(checked))
}

async fn panel(Uses(handle): Uses<VoiceHandle>, caller: Caller) -> ApiResult<Done> {
    caller.zone_at_least(Zone::Local)?;
    let channel = channel_of(&handle)?;
    channel.open_panel();
    Ok(ok(Done { state: channel.view() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_a_modifier_and_a_key() {
        for key in ["Alt+Space", "Control+Alt+K", "CommandOrControl+Shift+1"] {
            assert!(valid_key(key), "{key}");
        }
        for key in ["Space", "Alt+", "Alt+Shift", "Alt+Пробел", "Alt+Space+X+Y+Z", ""] {
            assert!(!valid_key(key), "{key}");
        }
    }
}
