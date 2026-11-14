//! «Как говорить с DuoWave»: through the station with Alice, or by this computer's microphone. The microphone is
//! «Своё» under a lock — no session with Yandex, no voice or brain of Alice's — so choosing it remembers the mode in
//! force, goes to «Своё» and holds it there: `ModeService::switch` refuses the other two whoever asks. Choosing the
//! station takes the lock off and returns the mode remembered. The choice is the computer's gateway's alone: the
//! station's own gateway never gets it (placement/transfer.rs, `LOCAL_SETTINGS`).

use serde_json::{json, Value};

use crate::core::events::Actor;
use crate::gateway::mode::ModeService;
use crate::platform::http::ApiError;
use crate::settings::definitions::{PanelMode, TalkInput, VoiceInput, VOICE_INPUT};
use crate::settings::SettingsService;

/// What a switch to «Яндекс» or «Вместе» hears while the microphone is chosen.
pub const ONLY_OWN: &str =
    "В режиме «Микрофон» — только «Своё»: сначала выберите «Через колонку с Алисой» в «Настройках».";

/// The station's own gateway has no microphone of a computer.
pub const NOT_HERE: &str = "Микрофон компьютера выбирается у шлюза на компьютере.";

pub fn current(settings: &SettingsService) -> VoiceInput {
    settings.get(&VOICE_INPUT, None).unwrap_or_default()
}

/// Whether the household talks by this computer's microphone: the mode is locked on «Своё», Alice and the station
/// are out of sight.
pub fn on_computer(settings: &SettingsService) -> bool {
    current(settings).input == TalkInput::Computer
}

/// At the start: the microphone holds «Своё» whatever was stored (`UNIPROXY_MODE`, a database edited by hand). On the
/// station's own gateway the choice has no meaning — a database moved from a computer — and is let go.
pub fn hold_at_start(modes: &ModeService, settings: &SettingsService, station: bool) {
    if !on_computer(settings) {
        return;
    }
    if station {
        tracing::warn!(target: "mode", "voice input: the computer's microphone is not for the station's gateway");
        let _ = settings.set(&VOICE_INPUT, &VoiceInput::default(), None);
        return;
    }
    if modes.current() != PanelMode::Own {
        match modes.hold_own() {
            Ok(()) => tracing::info!(target: "mode", "voice input: the computer's microphone — «Своё»"),
            Err(error) => tracing::error!(target: "mode", "voice input: «Своё» not set: {}", error.message),
        }
    }
}

/// The choice: `to` as the body gave it. To the computer — «Своё» first (a gateway that cannot be there keeps the
/// station), then the lock; to the station — the lock off first, then the mode from before, or «Вместе» when that one
/// has no way here now.
pub fn choose(
    modes: &ModeService,
    settings: &SettingsService,
    station: bool,
    to: Option<&Value>,
    actor: Option<Actor>,
) -> Result<VoiceInput, ApiError> {
    let Some(to) = to.and_then(Value::as_str).and_then(TalkInput::parse) else {
        return Err(ApiError::bad_request(crate::tr!("input: station или computer")));
    };
    let now = current(settings);
    match to {
        TalkInput::Computer => {
            if station {
                return Err(ApiError::conflict(NOT_HERE));
            }
            if now.input == TalkInput::Computer {
                if modes.current() != PanelMode::Own {
                    modes.hold_own()?;
                }
                return Ok(now);
            }
            let before = modes.current();
            modes.switch(Some(&json!("own")), actor.clone())?;
            let next = VoiceInput { input: TalkInput::Computer, before: Some(before) };
            settings.set(&VOICE_INPUT, &next, actor)?;
            Ok(next)
        }
        TalkInput::Station => {
            if now.input == TalkInput::Station {
                return Ok(now);
            }
            let next = VoiceInput::default();
            settings.set(&VOICE_INPUT, &next, actor.clone())?;
            let back = now.before.filter(|mode| modes.possible(*mode)).unwrap_or(PanelMode::Gateway);
            if let Err(error) = modes.switch(Some(&json!(back.as_str())), actor) {
                // The station is chosen all the same: the mode stays «Своё» until the switch has a way.
                tracing::warn!(target: "mode", "voice input: back to {} failed: {}", back.as_str(), error.message);
            }
            Ok(next)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::config::{Config, Env};
    use crate::core::build::Profile;
    use crate::core::events::EventBus;
    use crate::settings::definitions::{ModeSetting, MODE};
    use crate::speech::VoiceService;
    use crate::storage::Db;

    fn build(mode: &str, station: bool) -> (ModeService, Arc<SettingsService>) {
        let config = Config::load(&Env::from_pairs([("UNIPROXY_MODE", mode)])).unwrap();
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(Arc::new(Db::memory()), events.clone()));
        let voice = Arc::new(VoiceService::new(&config.gateway, events.clone()));
        let modes =
            ModeService::new(config.gateway.mode, settings.clone(), events, voice, Arc::new(Profile { station }));
        (modes, settings)
    }

    #[test]
    fn the_microphone_takes_svoi_and_locks_it_and_the_station_returns_the_mode_from_before() {
        let (modes, settings) = build("relay", false);
        assert_eq!(current(&settings), VoiceInput::default(), "the station until chosen otherwise");

        let chosen = choose(&modes, &settings, false, Some(&json!("computer")), None).unwrap();
        assert_eq!(chosen, VoiceInput { input: TalkInput::Computer, before: Some(PanelMode::Relay) });
        assert_eq!(modes.current(), PanelMode::Own);
        assert!(on_computer(&settings));

        // Whoever asks — the panel's switch, the emergency words of a station — hears the same refusal.
        for other in ["relay", "gateway"] {
            let refused = modes.switch(Some(&json!(other)), None).unwrap_err();
            assert_eq!((refused.status.as_u16(), refused.message.as_str()), (409, ONLY_OWN));
        }
        assert_eq!(modes.current(), PanelMode::Own);
        assert!(modes.switch(Some(&json!("own")), None).is_ok(), "«Своё» itself is no switch");
        assert!(!modes.leave_by_itself(PanelMode::Own, PanelMode::Gateway, "x").unwrap(), "nor by itself");

        // Chosen again: nothing changes, the mode from before is not lost.
        let again = choose(&modes, &settings, false, Some(&json!("computer")), None).unwrap();
        assert_eq!(again.before, Some(PanelMode::Relay));

        let back = choose(&modes, &settings, false, Some(&json!("station")), None).unwrap();
        assert_eq!(back, VoiceInput::default());
        assert_eq!(modes.current(), PanelMode::Relay, "the mode from before");
        assert!(modes.switch(Some(&json!("gateway")), None).is_ok(), "the lock is off");
    }

    #[test]
    fn holds_svoi_at_the_start_and_is_not_for_the_stations_gateway() {
        // A computer: the stored mode is not «Своё» — it becomes one.
        let (modes, settings) = build("gateway", false);
        settings.set(&VOICE_INPUT, &VoiceInput { input: TalkInput::Computer, before: None }, None).unwrap();
        hold_at_start(&modes, &settings, false);
        assert_eq!(modes.current(), PanelMode::Own);
        assert_eq!(settings.get(&MODE, None), Some(ModeSetting { mode: PanelMode::Own }));
        // Back with no mode remembered: «Вместе».
        choose(&modes, &settings, false, Some(&json!("station")), None).unwrap();
        assert_eq!(modes.current(), PanelMode::Gateway);

        // The station's gateway: the choice is let go, and cannot be made.
        let (modes, settings) = build("gateway", true);
        settings.set(&VOICE_INPUT, &VoiceInput { input: TalkInput::Computer, before: None }, None).unwrap();
        hold_at_start(&modes, &settings, true);
        assert_eq!((modes.current(), on_computer(&settings)), (PanelMode::Gateway, false));
        let refused = choose(&modes, &settings, true, Some(&json!("computer")), None).unwrap_err();
        assert_eq!((refused.status.as_u16(), refused.message.as_str()), (409, NOT_HERE));
    }

    #[test]
    fn refuses_what_is_not_a_way_to_talk() {
        let (modes, settings) = build("gateway", false);
        for wrong in [json!("phone"), json!(1), Value::Null] {
            let error = choose(&modes, &settings, false, Some(&wrong), None).unwrap_err();
            assert_eq!((error.status.as_u16(), error.message.as_str()), (400, "input: station или computer"));
        }
        assert!(choose(&modes, &settings, false, None, None).is_err());
    }
}
