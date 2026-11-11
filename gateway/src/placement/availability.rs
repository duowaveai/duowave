//! The registry of what works where (core/availability.rs) applied to this gateway now: its build, its placement,
//! its home server, its cloud, its mode. `/api/v1/availability` answers from here, the mode switch asks here, and
//! when anything changes — the home server comes or goes, the mode or a connector changes — the panels hear
//! `availability.changed` and read again.

use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

use crate::config::Config;
use crate::core::availability::{
    evaluate, evaluate_all, feature, mode_feature, table, Context, FeatureState, HomeServer, Placement, TableRow,
};
use crate::core::build::{ModeGate, Profile};
use crate::core::events::{EventBus, GatewayEvent};
use crate::operations::networks::is_local_url;
use crate::requests::ModeSource;
use crate::settings::definitions::PanelMode;

/// Where this gateway runs: the computer's build on a computer; the station's on the station, alone or with a home
/// server (the placement service says which).
pub trait PlacementSource: Send + Sync {
    fn placement(&self) -> Placement;
}

/// The home server as the station's gateway last saw it.
pub trait HomeServerSource: Send + Sync {
    fn home_server(&self) -> HomeServer;
    /// What «Свой» would lack as the computer gives now («Что даю колонке» without hearing or voice, and no cloud for
    /// it), said as the reason; none — nothing.
    fn own_lacks(&self) -> Option<&'static str> {
        None
    }
}

/// What the panel reads.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailabilityView {
    pub placement: Placement,
    pub home_server: HomeServer,
    pub features: Vec<FeatureState>,
    /// The three placements and what each gives, for the choice.
    pub placements: Vec<PlacementCard>,
    pub table: Vec<TableRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacementCard {
    pub id: Placement,
    pub title: &'static str,
    pub summary: &'static str,
    pub risk: &'static str,
}

pub struct AvailabilityService {
    profile: Profile,
    /// A recognizer and a voice set up in the cloud (not at home): the station's way when the computer sleeps.
    cloud_asr: bool,
    cloud_tts: bool,
    placement: OnceLock<Arc<dyn PlacementSource>>,
    home: OnceLock<Arc<dyn HomeServerSource>>,
    modes: OnceLock<Arc<dyn ModeSource>>,
    /// The plugins' free memory (src/plugins/budget.rs), kept by the plugin host's watch.
    memory: OnceLock<Arc<crate::plugins::budget::MemoryGauge>>,
    /// A transcoder here, and the owner's flag for the station (gateway/media/transcode.rs).
    transcoder: OnceLock<bool>,
    transcode_on_station: std::sync::atomic::AtomicBool,
    events: EventBus,
    /// The states last told: `availability.changed` only when one changed.
    told: Mutex<Vec<FeatureState>>,
}

impl AvailabilityService {
    pub fn new(config: &Config, profile: Profile, events: EventBus) -> Arc<AvailabilityService> {
        let asr = &config.speech.asr;
        let cloud_asr = asr.enabled && !is_local_url(Some(&asr.base_url));
        let cloud_tts = config.gateway.tts.own.as_ref().is_some_and(|own| !is_local_url(Some(&own.base_url)));
        let service = Arc::new(AvailabilityService::with(profile, cloud_asr, cloud_tts, events));
        *service.told.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = service.features();
        service
    }

    pub fn with(profile: Profile, cloud_asr: bool, cloud_tts: bool, events: EventBus) -> AvailabilityService {
        AvailabilityService {
            profile,
            cloud_asr,
            cloud_tts,
            placement: OnceLock::new(),
            home: OnceLock::new(),
            modes: OnceLock::new(),
            memory: OnceLock::new(),
            transcoder: OnceLock::new(),
            transcode_on_station: std::sync::atomic::AtomicBool::new(false),
            events,
            told: Mutex::new(Vec::new()),
        }
    }

    pub fn set_placement(&self, source: Arc<dyn PlacementSource>) {
        let _ = self.placement.set(source);
    }

    pub fn set_home_server(&self, source: Arc<dyn HomeServerSource>) {
        let _ = self.home.set(source);
    }

    pub fn set_modes(&self, source: Arc<dyn ModeSource>) {
        let _ = self.modes.set(source);
    }

    pub fn set_memory(&self, gauge: Arc<crate::plugins::budget::MemoryGauge>) {
        let _ = self.memory.set(gauge);
    }

    /// Whether streams can be transcoded here: a transcoder found, and the owner's flag for the station.
    pub fn set_transcoding(&self, transcoder: bool, on_station: bool) {
        let _ = self.transcoder.set(transcoder);
        self.transcode_on_station.store(on_station, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn context(&self) -> Context {
        let placement = self.placement.get().map(|source| source.placement()).unwrap_or(self.profile.placement());
        let home_server = match placement {
            Placement::StationServer => self.home.get().map(|source| source.home_server()).unwrap_or(HomeServer::Down),
            _ => HomeServer::None,
        };
        let own_lacks = match placement {
            Placement::StationServer => self.home.get().and_then(|source| source.own_lacks()),
            _ => None,
        };
        Context {
            pc_build: cfg!(feature = "pc") && !self.profile.station,
            placement,
            home_server,
            cloud_asr: self.cloud_asr,
            cloud_tts: self.cloud_tts,
            yandex: self.modes.get().is_none_or(|modes| modes.current() != PanelMode::Own),
            // Memory is short on the station; a computer's is not counted against a feature.
            memory_free_mb: if placement.on_station() { self.memory.get().and_then(|gauge| gauge.get()) } else { None },
            transcoder: self.transcoder.get().copied().unwrap_or(false),
            transcode_on_station: self.transcode_on_station.load(std::sync::atomic::Ordering::Relaxed),
            own_lacks,
        }
    }

    pub fn features(&self) -> Vec<FeatureState> {
        evaluate_all(&self.context())
    }

    /// One feature now; an unknown id is off.
    pub fn get(&self, id: &str) -> Option<FeatureState> {
        feature(id).map(|feature| evaluate(feature, &self.context()))
    }

    pub fn view(&self) -> AvailabilityView {
        let context = self.context();
        AvailabilityView {
            placement: context.placement,
            home_server: context.home_server,
            features: evaluate_all(&context),
            placements: Placement::ALL
                .into_iter()
                .map(|id| PlacementCard { id, title: id.title(), summary: id.summary(), risk: id.risk() })
                .collect(),
            table: table(),
        }
    }

    /// Reads it all again and tells the panels when a feature changed.
    pub fn recheck(&self) {
        let now = self.features();
        let changed: Vec<String> = {
            let mut told = self.told.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let changed = now
                .iter()
                .filter(|state| told.iter().find(|before| before.id == state.id) != Some(*state))
                .map(|state| state.id.to_string())
                .collect();
            *told = now;
            changed
        };
        if !changed.is_empty() {
            self.events.emit(GatewayEvent::AvailabilityChanged { features: changed });
        }
    }

    /// What can change it: the mode, a connector (the home server is one), the gateway's state.
    pub fn start(self: &Arc<Self>, events: &EventBus) {
        let me = self.clone();
        events.on("availability", move |event| {
            if matches!(
                &*event,
                GatewayEvent::ModeChanged { .. }
                    | GatewayEvent::ConnectorChanged { .. }
                    | GatewayEvent::HomeServerChanged { .. }
                    | GatewayEvent::HomeServerUp { .. }
                    | GatewayEvent::HomeServerDown { .. }
                    | GatewayEvent::SettingsChanged { .. }
                    | GatewayEvent::PluginsResources { .. }
            ) {
                me.recheck();
            }
        });
    }
}

/// The mode switch asks the registry: «Свой» on the station only with its home server up, and only while it gives
/// the station hearing and voice (or the cloud does).
impl ModeGate for AvailabilityService {
    fn check(&self, mode: PanelMode) -> Result<(), String> {
        match self.get(mode_feature(mode)) {
            Some(state) if !state.on() => Err(match mode {
                PanelMode::Own if state.reason.starts_with('«') => state.reason,
                PanelMode::Own => format!("«Своё» сейчас не включить: {}.", lowercase_first(&state.reason)),
                _ => state.reason,
            }),
            _ => Ok(()),
        }
    }

    fn absent(&self, mode: PanelMode) -> Option<String> {
        let context = self.context();
        // Only a mode with no way at all in this placement: not one that waits for the home server, nor for its hearing
        // and voice — a stored «Свой» is kept, and the gateway leaves it by itself while they are not given.
        let structural = Context { home_server: HomeServer::Up, yandex: true, own_lacks: None, ..context };
        let state = evaluate(feature(mode_feature(mode))?, &structural);
        (!state.on()).then_some(state.reason)
    }
}

fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::core::availability::{State, OWN_ON_STATION};

    struct At(Placement);
    impl PlacementSource for At {
        fn placement(&self) -> Placement {
            self.0
        }
    }

    /// The home server: up or not, and what «Свой» would lack.
    struct Home(AtomicBool, Mutex<Option<&'static str>>);
    impl HomeServerSource for Home {
        fn home_server(&self) -> HomeServer {
            if self.0.load(Ordering::Relaxed) {
                HomeServer::Up
            } else {
                HomeServer::Down
            }
        }

        fn own_lacks(&self) -> Option<&'static str> {
            *self.1.lock().unwrap()
        }
    }

    #[tokio::test]
    async fn the_station_with_a_server_takes_svoi_while_the_server_is_up_and_says_when_it_changes() {
        let events = EventBus::new();
        let mut heard = events.subscribe();
        let service = Arc::new(AvailabilityService::with(Profile { station: true }, true, false, events.clone()));
        service.set_placement(Arc::new(At(Placement::StationServer)));
        let home = Arc::new(Home(AtomicBool::new(true), Mutex::new(None)));
        service.set_home_server(home.clone());
        service.recheck();
        while heard.try_recv().is_ok() {}

        assert!(service.check(PanelMode::Own).is_ok());
        assert_eq!(service.get("asr.own").unwrap().state, State::Ready);
        home.0.store(false, Ordering::Relaxed);
        assert_eq!(service.check(PanelMode::Own), Err("«Своё» сейчас не включить: компьютер не на связи.".to_string()));
        assert_eq!(service.absent(PanelMode::Own), None, "only for now: a stored «Свой» is kept");
        assert_eq!(service.get("asr.own").unwrap().reason, "компьютер не на связи — через облако");
        service.recheck();
        let mut changed = Vec::new();
        while let Ok(event) = heard.try_recv() {
            if let GatewayEvent::AvailabilityChanged { features } = &*event {
                changed.extend(features.clone());
            }
        }
        assert!(changed.contains(&"mode.own".to_string()) && changed.contains(&"asr.own".to_string()));
        service.recheck();
        assert!(heard.try_recv().is_err(), "nothing new, nothing told");

        let alone = AvailabilityService::with(Profile { station: true }, true, false, EventBus::new());
        assert_eq!(alone.check(PanelMode::Own), Err(OWN_ON_STATION.to_string()));
        assert_eq!(alone.absent(PanelMode::Own), Some(OWN_ON_STATION.to_string()));
    }

    /// Review G2: the switch knows what the home server does not give — «Свой» without the computer's hearing (no
    /// cloud for it) is refused with why, and the panel sees it off with the same words; a stored one is kept.
    #[tokio::test]
    async fn svoi_waits_for_the_computers_hearing_and_voice_and_says_so() {
        let events = EventBus::new();
        let mut heard = events.subscribe();
        let service = Arc::new(AvailabilityService::with(Profile { station: true }, false, false, events.clone()));
        service.set_placement(Arc::new(At(Placement::StationServer)));
        let home = Arc::new(Home(AtomicBool::new(true), Mutex::new(None)));
        service.set_home_server(home.clone());
        service.recheck();
        while heard.try_recv().is_ok() {}
        assert!(service.check(PanelMode::Own).is_ok());

        *home.1.lock().unwrap() = Some("слух компьютера выключен");
        assert_eq!(
            service.check(PanelMode::Own),
            Err("«Своё» сейчас не включить: слух компьютера выключен.".to_string())
        );
        let own = service.get("mode.own").unwrap();
        assert_eq!((own.state, own.reason.as_str()), (State::Off, "Слух компьютера выключен"));
        assert_eq!(service.absent(PanelMode::Own), None, "a stored «Свой» stays: the gateway leaves it by itself");
        service.recheck();
        let told = heard.try_recv().unwrap();
        let GatewayEvent::AvailabilityChanged { features } = &*told else { panic!("{told:?}") };
        assert!(features.contains(&"mode.own".to_string()), "the panels hear «Свой» is off");

        *home.1.lock().unwrap() = None;
        assert!(service.check(PanelMode::Own).is_ok(), "given again: «Свой» may be chosen");
    }
}
