//! The station's mode — Яндекс (relay), Шлюз (gateway), Свой (own) — the one source of truth for it. Relay and
//! gateway are different sessions with the station, so a switch between them closes the live connections and the
//! station reconnects into the new one (the session registry hears `mode.changed`). «Свой» is the gateway with
//! Yandex forbidden: no session with Yandex opens and no route step goes to it. It is an overlay — the routes stay
//! the household's preference and come back as they were on the way out of «Свой».

use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;

use crate::config::UniproxyMode;
use crate::core::build::ModeGate;
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::platform::http::ApiError;
use crate::requests::ModeSource;
use crate::settings::definitions::{AutoSwitch, GatewayState, ModeBy, ModeSetting, PanelMode, GATEWAY_STATE, MODE};
use crate::settings::SettingsService;
use crate::speech::voice_service::Whose;
use crate::speech::VoiceService;

fn note(mode: PanelMode) -> &'static str {
    match mode {
        PanelMode::Relay => "Всё отвечает Яндекс, как до шлюза. Звонки по ссылке и помощник не работают.",
        PanelMode::Gateway => "Сначала свой, Яндекс подхватывает то, чего мы не умеем.",
        PanelMode::Own => "Без Яндекса. Радио, сказки, напоминания и навыки пока не умеем.",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModeView {
    pub mode: PanelMode,
    pub note: &'static str,
    /// Whose voice is chosen: what «Свой» warns about when it is Alice's (it cannot speak without Yandex).
    pub voice: Whose,
    /// Whose voice speaks now, after the mode: in «Свой» Alice's gives way to ours.
    pub speaking: Whose,
    /// Who chose the mode in force: a person, or the gateway itself.
    pub by: ModeBy,
    /// The gateway's own switch, while it holds: from which mode and why — it goes back by itself when the reason
    /// passes, unless a person decides first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto: Option<AutoSwitch>,
}

/// Which session a connecting station gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// Relayed to Yandex.
    Relay,
    /// Answered by us.
    Gateway,
}

pub struct ModeService {
    settings: Arc<SettingsService>,
    events: EventBus,
    voice: Arc<VoiceService>,
    /// What may be switched to now: the registry of what works where (src/core/availability.rs) — no «Свой» on the
    /// station alone, on the station with a home server only while it is up.
    gate: Arc<dyn ModeGate>,
    mode: Mutex<PanelMode>,
}

impl ModeService {
    /// The mode UNIPROXY_MODE names, then the panel's stored choice over it (the TS `onModuleInit`). The first
    /// panel's routes kept aside by its «Свой» are a Node data format and are not read here (gateway/PORTING.md).
    /// A mode that has no way at all here — «Свой» kept in a database moved onto the station alone — becomes «Шлюз»,
    /// and the setting with it, so everything that reads the setting agrees. One only out of reach for now («Свой» on
    /// the station while its home server is not up yet) stays: the gateway's state decides about it.
    pub fn new(
        configured: UniproxyMode,
        settings: Arc<SettingsService>,
        events: EventBus,
        voice: Arc<VoiceService>,
        gate: Arc<dyn ModeGate>,
    ) -> ModeService {
        let mut mode = match configured {
            UniproxyMode::Gateway => PanelMode::Gateway,
            UniproxyMode::Relay => PanelMode::Relay,
        };
        if let Some(ModeSetting { mode: stored }) = settings.get(&MODE, None) {
            if stored != mode {
                tracing::info!(target: "mode", "mode: {} (from the settings)", stored.as_str());
            }
            mode = stored;
        }
        if let Some(why) = gate.absent(mode) {
            tracing::warn!(target: "mode", "mode: {} is not for this gateway, gateway instead — {why}", mode.as_str());
            mode = PanelMode::Gateway;
            let _ = settings.set(&MODE, &ModeSetting { mode }, None);
        }
        ModeService { settings, events, voice, gate, mode: Mutex::new(mode) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PanelMode> {
        self.mode.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn current(&self) -> PanelMode {
        *self.lock()
    }

    pub fn view(&self) -> ModeView {
        let mode = self.current();
        let state = self.state();
        ModeView {
            mode,
            note: note(mode),
            voice: self.voice.whose(true),
            speaking: self.voice.whose(mode != PanelMode::Own),
            by: state.mode_by,
            auto: state.auto,
        }
    }

    /// The gateway's state record: who chose the mode, and its own switch.
    pub fn state(&self) -> GatewayState {
        self.settings.get(&GATEWAY_STATE, None).unwrap_or_default()
    }

    fn record(&self, by: ModeBy, auto: Option<AutoSwitch>, actor: Option<Actor>) -> Result<(), ApiError> {
        let mut state = self.state();
        if state.mode_by == by && state.auto == auto {
            return Ok(());
        }
        state.mode_by = by;
        state.auto = auto;
        self.settings.set(&GATEWAY_STATE, &state, actor.clone())?;
        self.events.emit(GatewayEvent::GatewayStateChanged { what: "mode".into(), actor });
        Ok(())
    }

    /// Which session a connecting station gets: relayed to Yandex, or answered by us.
    pub fn session_kind(&self) -> SessionKind {
        if self.current() == PanelMode::Relay {
            SessionKind::Relay
        } else {
            SessionKind::Gateway
        }
    }

    /// Whether anything may go to Yandex: not in «Свой».
    pub fn yandex_allowed(&self) -> bool {
        self.current() != PanelMode::Own
    }

    /// The panel's switch: `to` as the body gave it, anything but one of the three refused. A person's choice ends
    /// the gateway's own switch: no going back by itself after it.
    pub fn switch(&self, to: Option<&Value>, actor: Option<Actor>) -> Result<ModeView, ApiError> {
        let Some(to) = to.and_then(Value::as_str).and_then(PanelMode::parse) else {
            return Err(ApiError::bad_request("mode: relay, gateway или own"));
        };
        self.gate.check(to).map_err(ApiError::conflict)?;
        self.change(to, actor.clone(), None)?;
        self.record(ModeBy::Person, None, actor)?;
        Ok(self.view())
    }

    /// «Остаться»: the mode in force becomes the person's choice; the gateway will not switch back by itself.
    pub fn keep(&self, actor: Option<Actor>) -> Result<ModeView, ApiError> {
        self.record(ModeBy::Person, None, actor)?;
        Ok(self.view())
    }

    /// The gateway leaves a mode by itself (its home server went): to `to`, kept as its own switch, with why.
    /// Nothing happens when the mode is not `from` any more.
    pub fn leave_by_itself(&self, from: PanelMode, to: PanelMode, reason: &str) -> Result<bool, ApiError> {
        if self.current() != from {
            return Ok(false);
        }
        self.gate.check(to).map_err(ApiError::conflict)?;
        self.change(to, Some(Actor::from(&crate::core::identity::principal::system())), Some(reason.to_string()))?;
        let auto = AutoSwitch { from, to, reason: reason.to_string(), at: crate::utils::time::now_iso() };
        self.record(ModeBy::Auto, Some(auto), None)?;
        Ok(true)
    }

    /// The reason passed (the home server is back): the mode the gateway left by itself comes back — only while that
    /// switch still holds, a person's choice since is never undone.
    pub fn return_by_itself(&self, reason: &str) -> Result<bool, ApiError> {
        let state = self.state();
        let Some(auto) = state.auto.filter(|_| state.mode_by == ModeBy::Auto) else { return Ok(false) };
        if self.current() != auto.to || self.gate.check(auto.from).is_err() {
            return Ok(false);
        }
        self.change(
            auto.from,
            Some(Actor::from(&crate::core::identity::principal::system())),
            Some(reason.to_string()),
        )?;
        self.record(ModeBy::Person, None, None)?;
        Ok(true)
    }

    fn change(&self, to: PanelMode, actor: Option<Actor>, auto: Option<String>) -> Result<(), ApiError> {
        let from = {
            let mut mode = self.lock();
            let from = *mode;
            if to == from {
                return Ok(());
            }
            *mode = to;
            from
        };
        self.settings.set(&MODE, &ModeSetting { mode: to }, actor.clone())?;
        let why = auto.as_deref().map(|why| format!(" by itself: {why}")).unwrap_or_default();
        self.events.emit(GatewayEvent::ModeChanged { from: from.as_str().into(), to: to.as_str().into(), actor, auto });
        tracing::info!(target: "mode", "mode: {} → {}{why}; the station reconnects into it", from.as_str(), to.as_str());
        Ok(())
    }
}

/// The mode for the requests' journal, health and the connectors: the panel's mode now.
impl ModeSource for ModeService {
    fn current(&self) -> PanelMode {
        ModeService::current(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Env, TtsProvider};
    use crate::core::build::Profile;
    use crate::storage::Db;
    use serde_json::json;

    fn build(provider: &str, own: bool) -> (ModeService, Arc<VoiceService>, EventBus) {
        let mut pairs = vec![("TTS_PROVIDER", provider), ("UNIPROXY_MODE", "gateway")];
        if own {
            pairs.push(("TTS_OWN_URL", "http://127.0.0.1:8091/v1"));
        }
        let config = Config::load(&Env::from_pairs(pairs)).unwrap();
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(Arc::new(Db::memory()), events.clone()));
        let voice = Arc::new(VoiceService::new(&config.gateway, events.clone()));
        (
            ModeService::new(
                config.gateway.mode,
                settings,
                events.clone(),
                voice.clone(),
                Arc::new(Profile { station: false }),
            ),
            voice,
            events,
        )
    }

    #[tokio::test]
    async fn takes_svoi_as_the_gateway_without_yandex_no_session_with_it_and_our_voice_for_alices() {
        let (modes, voice, events) = build("yandex", true);
        let mut changes = events.subscribe();
        assert!(modes.yandex_allowed());
        assert_eq!(voice.provider(modes.yandex_allowed()), TtsProvider::Yandex);

        let view = modes.switch(Some(&json!("own")), None).unwrap();
        assert_eq!(serde_json::to_value(&view).unwrap()["mode"], "own");
        assert_eq!((view.voice, view.speaking), (Whose::Yandex, Whose::Own));
        assert_eq!(modes.session_kind(), SessionKind::Gateway);
        assert!(!modes.yandex_allowed());
        assert_eq!(voice.provider(modes.yandex_allowed()), TtsProvider::Mixed);
        assert!(!voice.markup(modes.yandex_allowed()));
        let mut seen = Vec::new();
        while let Ok(event) = changes.try_recv() {
            if let GatewayEvent::ModeChanged { from, to, .. } = &*event {
                seen.push(format!("{from}→{to}"));
            }
        }
        assert_eq!(seen, vec!["gateway→own"]);
    }

    #[test]
    fn goes_silent_rather_than_pretend_when_svoi_has_no_voice_of_ours() {
        let (modes, voice, _) = build("yandex", false);
        modes.switch(Some(&json!("own")), None).unwrap();
        assert_eq!(voice.provider(modes.yandex_allowed()), TtsProvider::None);
    }

    #[test]
    fn refuses_what_is_not_a_mode_the_object_prototypes_names_included() {
        let (modes, _, _) = build("yandex", true);
        for wrong in [json!("constructor"), json!("__proto__"), json!(1), Value::Null] {
            let error = modes.switch(Some(&wrong), None).unwrap_err();
            assert_eq!((error.status.as_u16(), error.message.as_str()), (400, "mode: relay, gateway или own"));
        }
        assert!(modes.switch(None, None).is_err());
    }

    #[test]
    fn keeps_the_panels_choice_across_a_restart() {
        // The TS spec's last case gave back the first panel's routes kept aside by its «Свой»: a Node data format,
        // not read by the Rust gateway (PORTING.md). What is left of it: the stored mode wins over UNIPROXY_MODE.
        let config = Config::load(&Env::from_pairs([("UNIPROXY_MODE", "relay")])).unwrap();
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(Arc::new(Db::memory()), events.clone()));
        settings.set(&MODE, &ModeSetting { mode: PanelMode::Own }, None).unwrap();
        let voice = Arc::new(VoiceService::new(&config.gateway, events.clone()));
        let modes =
            ModeService::new(config.gateway.mode, settings, events, voice, Arc::new(Profile { station: false }));
        assert_eq!(modes.current(), PanelMode::Own);
        assert_eq!(ModeSource::current(&modes), PanelMode::Own);
    }

    #[test]
    fn the_station_refuses_svoi_and_turns_a_stored_one_into_the_gateway() {
        let config = Config::load(&Env::from_pairs([("UNIPROXY_MODE", "relay")])).unwrap();
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(Arc::new(Db::memory()), events.clone()));
        settings.set(&MODE, &ModeSetting { mode: PanelMode::Own }, None).unwrap();
        let voice = Arc::new(VoiceService::new(&config.gateway, events.clone()));
        let modes =
            ModeService::new(config.gateway.mode, settings.clone(), events, voice, Arc::new(Profile { station: true }));
        assert_eq!(modes.current(), PanelMode::Gateway, "a database moved from a PC");
        assert_eq!(settings.get(&MODE, None), Some(ModeSetting { mode: PanelMode::Gateway }), "the setting agrees");

        let error = modes.switch(Some(&json!("own")), None).unwrap_err();
        assert_eq!((error.status.as_u16(), error.message.as_str()), (409, crate::core::build::OWN_ON_STATION));
        assert_eq!(modes.current(), PanelMode::Gateway);
        assert_eq!(serde_json::to_value(modes.switch(Some(&json!("relay")), None).unwrap()).unwrap()["mode"], "relay");
    }

    /// The station with a home server: «Свой» only while the server is up.
    struct Server(std::sync::atomic::AtomicBool);

    impl ModeGate for Server {
        fn check(&self, mode: PanelMode) -> Result<(), String> {
            if mode == PanelMode::Own && !self.0.load(std::sync::atomic::Ordering::Relaxed) {
                return Err("«Свой» сейчас не включить: компьютер не на связи.".into());
            }
            Ok(())
        }
        fn absent(&self, _mode: PanelMode) -> Option<String> {
            None
        }
    }

    fn with_server() -> (ModeService, Arc<Server>, Arc<SettingsService>) {
        let config = Config::load(&Env::from_pairs([("UNIPROXY_MODE", "gateway")])).unwrap();
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(Arc::new(Db::memory()), events.clone()));
        let voice = Arc::new(VoiceService::new(&config.gateway, events.clone()));
        let server = Arc::new(Server(std::sync::atomic::AtomicBool::new(true)));
        let modes = ModeService::new(config.gateway.mode, settings.clone(), events, voice, server.clone());
        (modes, server, settings)
    }

    #[test]
    fn leaves_svoi_by_itself_when_the_server_goes_and_comes_back_only_if_nobody_chose_since() {
        let (modes, server, settings) = with_server();
        modes.switch(Some(&json!("own")), None).unwrap();
        server.0.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(modes.leave_by_itself(PanelMode::Own, PanelMode::Gateway, "компьютер не на связи").unwrap());
        let view = modes.view();
        assert_eq!((view.mode, view.by), (PanelMode::Gateway, ModeBy::Auto));
        assert_eq!(
            view.auto.as_ref().map(|auto| (auto.from, auto.reason.as_str())),
            Some((PanelMode::Own, "компьютер не на связи"))
        );
        assert_eq!(settings.get(&GATEWAY_STATE, None).unwrap().mode_by, ModeBy::Auto, "kept in the database");
        assert!(!modes.return_by_itself("компьютер снова на связи").unwrap(), "not while the server is away");
        server.0.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(modes.return_by_itself("компьютер снова на связи").unwrap());
        assert_eq!((modes.current(), modes.view().by, modes.view().auto), (PanelMode::Own, ModeBy::Person, None));

        // A person decides while it holds: the choice stays.
        server.0.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(modes.leave_by_itself(PanelMode::Own, PanelMode::Gateway, "компьютер не на связи").unwrap());
        modes.switch(Some(&json!("relay")), None).unwrap();
        server.0.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(!modes.return_by_itself("компьютер снова на связи").unwrap());
        assert_eq!(modes.current(), PanelMode::Relay);

        // «Остаться в «Шлюзе»»: the same, without switching.
        modes.switch(Some(&json!("own")), None).unwrap();
        assert!(modes.leave_by_itself(PanelMode::Own, PanelMode::Gateway, "компьютер не на связи").unwrap());
        modes.keep(None).unwrap();
        assert!(!modes.return_by_itself("компьютер снова на связи").unwrap());
        assert_eq!((modes.current(), modes.view().by), (PanelMode::Gateway, ModeBy::Person));
        assert!(!modes.leave_by_itself(PanelMode::Own, PanelMode::Gateway, "x").unwrap(), "not in «Свой»: nothing");
    }
}
