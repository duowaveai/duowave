//! The tests of `ear/mode.rs` that need a floor above its own (the placement's availability): its crate does not
//! have it, the program does.

use std::sync::Arc;

use serde_json::json;

use crate::config::{Config, Env};
use crate::core::build::Profile;
use crate::core::events::EventBus;
use crate::ear::mode::*;
use crate::settings::definitions::{ModeSetting, PanelMode, MODE};
use crate::settings::SettingsService;
use crate::speech::VoiceService;
use crate::storage::Db;

/// The station without a cloud hearing and voice (docs/plans/speaker-alone.md): «Свой» would be deaf and mute.
#[test]
fn the_station_without_a_cloud_refuses_svoi_and_turns_a_stored_one_into_the_gateway() {
    let config = Config::load(&Env::from_pairs([("UNIPROXY_MODE", "relay")])).unwrap();
    let events = EventBus::new();
    let settings = Arc::new(SettingsService::new(Arc::new(Db::memory()), events.clone()));
    settings.set(&MODE, &ModeSetting { mode: PanelMode::Own }, None).unwrap();
    let voice = Arc::new(VoiceService::new(&config.gateway, events.clone()));
    let gate = Arc::new(crate::placement::availability::AvailabilityService::with(
        Profile { station: true },
        false,
        false,
        events.clone(),
    ));
    let modes = ModeService::new(config.gateway.mode, settings.clone(), events, voice, gate);
    assert_eq!(modes.current(), PanelMode::Gateway, "a database moved from a PC");
    assert_eq!(settings.get(&MODE, None), Some(ModeSetting { mode: PanelMode::Gateway }), "the setting agrees");

    let error = modes.switch(Some(&json!("own")), None).unwrap_err();
    assert_eq!(
        (error.status.as_u16(), error.message.as_str()),
        (409, "«Ваш ИИ» сейчас не включить: нет облачного распознавания.")
    );
    assert_eq!(modes.current(), PanelMode::Gateway);
    assert_eq!(serde_json::to_value(modes.switch(Some(&json!("relay")), None).unwrap()).unwrap()["mode"], "relay");
}
