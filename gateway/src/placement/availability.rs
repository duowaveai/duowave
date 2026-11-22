//! The registry of what works where (core/availability.rs) applied to this gateway now: its build, its placement,
//! its helpers and the regulator, its cloud, its mode. `/api/v1/availability` answers from here, the mode switch asks
//! here, and when anything changes — a helper comes or goes, the list or the regulator, the mode or a connector
//! changes — the panels hear `availability.changed` and read again.
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

use crate::config::Config;
use crate::core::availability::{
    evaluate, evaluate_all, feature, mode_feature, table, Context, FeatureState, HelperContext, HomeServer, Placement,
    TableRow,
};
use crate::core::build::{ModeGate, Profile};
use crate::core::events::{EventBus, GatewayEvent};
use crate::helpers::contract::{Abilities, Ability};
use crate::operations::networks::is_local_url;
use crate::requests::ModeSource;
use crate::settings::definitions::PanelMode;

/// Where this gateway runs: the computer's build on a computer; the station's on the station, alone or with a home
/// server (the placement service says which).
pub trait PlacementSource: Send + Sync {
    fn placement(&self) -> Placement;
}

/// The helpers as the registry reads them (src/helpers/registry.rs): from what the gateway keeps in memory — the list,
/// the regulator, the last looks —, never the settings table, since the registry is read often: every `/health` (its
/// modes), the mode switch, `/api/v1/availability`, each recheck on an event.
pub trait HelperSource: Send + Sync {
    /// Who is on the line, what the helpers are set to give and what one gives now (`Helpers::pick`), whether the
    /// regulator keeps them all out.
    fn helpers(&self) -> HelperContext;
    /// Whether the regulator «Что может уходить из дома» lets the clouds by keys out: the clouds' ways of hearing,
    /// voice and the brain are there only then.
    fn clouds(&self) -> bool {
        true
    }
    /// What «Свой» on the station would lack as the helpers give now (no hearing or voice, and no cloud for it), said
    /// as the reason; none — nothing.
    fn own_lacks(&self) -> Option<&'static str> {
        None
    }
    /// What a helper of the list is set to give, whether the regulator lets it out and whether it is on the line now:
    /// on a computer such a part is that helper's, never counted as the computer's own (stage 6).
    fn listed(&self) -> Abilities {
        Abilities::NONE
    }
}

/// What the panel reads.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailabilityView {
    pub placement: Placement,
    /// The old view of the station's home server — its computer up or down — for the panels of 0.11.
    pub home_server: HomeServer,
    /// The helpers as the registry read them: on the line, what they are set to give, what they give now, what this
    /// computer gives itself.
    pub helpers: HelperContext,
    pub features: Vec<FeatureState>,
    /// The three placements and what each gives, for the choice.
    pub placements: Vec<PlacementCard>,
    pub table: Vec<TableRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacementCard {
    pub id: Placement,
    pub title: String,
    pub summary: String,
    pub risk: String,
}

pub struct AvailabilityService {
    profile: Profile,
    /// A recognizer and a voice set up in the cloud (not at home): the station's way when the computer sleeps.
    cloud_asr: bool,
    cloud_tts: bool,
    placement: OnceLock<Arc<dyn PlacementSource>>,
    helpers: OnceLock<Arc<dyn HelperSource>>,
    modes: OnceLock<Arc<dyn ModeSource>>,
    /// The plugins' free memory (src/plugins/budget.rs), kept by the plugin host's watch.
    memory: OnceLock<Arc<crate::plugins::budget::MemoryGauge>>,
    /// A transcoder here, and the owner's flag for the station (gateway/media/transcode.rs).
    transcoder: OnceLock<bool>,
    transcode_on_station: std::sync::atomic::AtomicBool,
    /// MIDI keyboards are heard here (src/midi).
    midi: std::sync::atomic::AtomicBool,
    /// A Турбо at home has a MIDI keyboard plugged in now (src/home/turbo_devices.rs).
    turbo_midi: std::sync::atomic::AtomicBool,
    /// The station's cloud hearing and voice as its panel sets them (src/speech/cloud_speech.rs): read live, over the
    /// flags of the start.
    cloud: OnceLock<Arc<crate::speech::cloud_speech::CloudSpeech>>,
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
            helpers: OnceLock::new(),
            modes: OnceLock::new(),
            memory: OnceLock::new(),
            transcoder: OnceLock::new(),
            transcode_on_station: std::sync::atomic::AtomicBool::new(false),
            midi: std::sync::atomic::AtomicBool::new(false),
            turbo_midi: std::sync::atomic::AtomicBool::new(false),
            cloud: OnceLock::new(),
            events,
            told: Mutex::new(Vec::new()),
        }
    }

    pub fn set_placement(&self, source: Arc<dyn PlacementSource>) {
        let _ = self.placement.set(source);
    }

    pub fn set_helpers(&self, source: Arc<dyn HelperSource>) {
        let _ = self.helpers.set(source);
    }

    pub fn set_modes(&self, source: Arc<dyn ModeSource>) {
        let _ = self.modes.set(source);
    }

    /// The station's cloud hearing and voice: a change in its panel (or a new key) counts at once.
    pub fn set_cloud(&self, cloud: Arc<crate::speech::cloud_speech::CloudSpeech>) {
        let _ = self.cloud.set(cloud);
    }

    pub fn set_memory(&self, gauge: Arc<crate::plugins::budget::MemoryGauge>) {
        let _ = self.memory.set(gauge);
    }

    /// Whether streams can be transcoded here: a transcoder found, and the owner's flag for the station.
    pub fn set_transcoding(&self, transcoder: bool, on_station: bool) {
        let _ = self.transcoder.set(transcoder);
        self.transcode_on_station.store(on_station, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether this gateway hears MIDI keyboards (src/midi): its build and `MIDI`.
    pub fn set_midi(&self, midi: bool) {
        self.midi.store(midi, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether a Турбо at home has a MIDI keyboard plugged in now: the centre relays it (docs/plans/turbo-devices.md).
    /// True when it changed.
    pub fn set_turbo_midi(&self, midi: bool) -> bool {
        self.turbo_midi.swap(midi, std::sync::atomic::Ordering::Relaxed) != midi
    }

    /// Where this gateway runs now: for whoever needs only that (the apps' placement, the schedules, the streams), the
    /// helpers not read.
    pub fn placement(&self) -> Placement {
        self.placement.get().map(|source| source.placement()).unwrap_or(self.profile.placement())
    }

    pub fn context(&self) -> Context {
        let placement = self.placement();
        let pc_build = cfg!(feature = "pc") && !self.profile.station;
        let source = self.helpers.get();
        let mut helpers = source.map(|source| source.helpers()).unwrap_or_default();
        // A computer's build gives hearing, voice, a model, … itself: its own heavy part is a helper on the line, and
        // what it gives is the source's `builtin`. A part this computer has none of (stage 6) is not its own: a helper
        // of its list gives it — or would, while it is away or kept out —, as «Где сейчас» says; hearing and voice of
        // its .env at a cloud's address are the cloud's way, under the regulator. Only what nobody is set to give it
        // still counts as its own, as a computer's build always said.
        if pc_build {
            let listed = source.map(|source| source.listed()).unwrap_or(Abilities::NONE);
            let rest = Abilities::V1
                .iter()
                .filter(|ability| {
                    !helpers.up.has(*ability) && !helpers.configured.has(*ability) && !listed.has(*ability)
                })
                .filter(|ability| match ability {
                    Ability::Hearing => !self.cloud_asr,
                    Ability::Voice => !self.cloud_tts,
                    _ => true,
                })
                .collect();
            helpers = helpers.with_builtin(rest);
        }
        // «Свой» leans on the helpers on the station, the computer at home or another; a computer hears and speaks.
        let own_lacks = if placement.on_station() { source.and_then(|source| source.own_lacks()) } else { None };
        Context {
            pc_build,
            placement,
            helpers,
            cloud_asr: self.cloud.get().map_or(self.cloud_asr, |cloud| cloud.hears()),
            cloud_tts: self.cloud.get().map_or(self.cloud_tts, |cloud| cloud.speaks()),
            clouds: source.is_none_or(|source| source.clouds()),
            yandex: self.modes.get().is_none_or(|modes| modes.current() != PanelMode::Own),
            // Memory is short on the station; a computer's is not counted against a feature.
            memory_free_mb: if placement.on_station() { self.memory.get().and_then(|gauge| gauge.get()) } else { None },
            transcoder: self.transcoder.get().copied().unwrap_or(false),
            transcode_on_station: self.transcode_on_station.load(std::sync::atomic::Ordering::Relaxed),
            own_lacks,
            midi: self.midi.load(std::sync::atomic::Ordering::Relaxed),
            turbo_midi: self.turbo_midi.load(std::sync::atomic::Ordering::Relaxed),
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
        let home_server = match context.placement {
            Placement::StationServer if context.helpers.online => HomeServer::Up,
            Placement::StationServer => HomeServer::Down,
            _ => HomeServer::None,
        };
        AvailabilityView {
            placement: context.placement,
            home_server,
            helpers: context.helpers,
            features: evaluate_all(&context),
            placements: Placement::ALL
                .into_iter()
                .map(|id| PlacementCard {
                    id,
                    title: id.title_text(),
                    summary: id.summary_text(),
                    risk: id.risk_text(),
                })
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

    /// What can change it: the mode, a connector, the helpers and the regulator, the gateway's state.
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
                    | GatewayEvent::HelpersChanged { .. }
                    | GatewayEvent::HelperUp { .. }
                    | GatewayEvent::HelperDown { .. }
                    | GatewayEvent::SettingsChanged { .. }
                    | GatewayEvent::PluginsResources { .. }
            ) {
                me.recheck();
            }
        });
    }
}

/// The mode switch asks the registry: «Свой» on the station only with a helper on the line, and only while one gives
/// it hearing and voice (or a cloud the regulator lets out does).
impl ModeGate for AvailabilityService {
    fn check(&self, mode: PanelMode) -> Result<(), String> {
        match self.get(mode_feature(mode)) {
            Some(state) if !state.on() => Err(match mode {
                PanelMode::Own if state.reason.starts_with(['«', '“']) => state.reason,
                PanelMode::Own => {
                    crate::tr!("«Ваш ИИ» сейчас не включить: {reason}.", reason = lowercase_first(&state.reason))
                }
                _ => state.reason,
            }),
            _ => Ok(()),
        }
    }

    fn absent(&self, mode: PanelMode) -> Option<String> {
        let context = self.context();
        // Only a mode with no way at all in this placement: not one that waits for a helper — the ones the regulator
        // keeps out too —, nor for its hearing and voice: a stored «Свой» is kept, and the gateway leaves it by itself
        // while they are not given, and brings it back when they are.
        let structural =
            Context { helpers: context.helpers.all_up(), yandex: true, own_lacks: None, clouds: true, ..context };
        let state = evaluate(feature(mode_feature(mode))?, &structural);
        (!state.on()).then_some(state.reason)
    }
}

fn lowercase_first(text: &str) -> String {
    // «Турбо» is a name in the sentence (docs/plans/home-and-helpers.md): it keeps its capital.
    if text.starts_with("Турбо") {
        return text.to_string();
    }
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
    use crate::core::availability::{Runs, State};

    struct At(Placement);
    impl PlacementSource for At {
        fn placement(&self) -> Placement {
            self.0
        }
    }

    /// A helper set up to give these abilities: on the line or not, kept out by the regulator or not, the clouds let
    /// out or not, and what «Свой» would lack. On a computer, beside what its own heavy part gives (`itself`).
    struct Helper {
        gives: Abilities,
        itself: Abilities,
        online: AtomicBool,
        kept: AtomicBool,
        clouds: AtomicBool,
        lacks: Mutex<Option<&'static str>>,
    }

    impl Helper {
        fn giving(gives: &[Ability]) -> Arc<Helper> {
            Helper::beside(&[], gives)
        }

        /// A computer's own heavy part giving `itself`, and a helper of its list set up to give `gives`.
        fn beside(itself: &[Ability], gives: &[Ability]) -> Arc<Helper> {
            Arc::new(Helper {
                gives: Abilities::of(gives),
                itself: Abilities::of(itself),
                online: AtomicBool::new(true),
                kept: AtomicBool::new(false),
                clouds: AtomicBool::new(true),
                lacks: Mutex::new(None),
            })
        }
    }

    impl HelperSource for Helper {
        /// As `Helpers` reads it: a helper kept out is neither set up nor given, and with only it listed the helpers
        /// are shut; what the computer gives itself comes first.
        fn helpers(&self) -> HelperContext {
            let kept = self.kept.load(Ordering::Relaxed);
            let online = self.online.load(Ordering::Relaxed) && !kept;
            let configured = if kept { Abilities::NONE } else { self.gives };
            let up = if online { self.gives } else { Abilities::NONE };
            let shut = kept && !self.gives.is_empty();
            HelperContext { online, configured, up, shut, ..HelperContext::default() }.with_builtin(self.itself)
        }

        fn clouds(&self) -> bool {
            self.clouds.load(Ordering::Relaxed)
        }

        fn own_lacks(&self) -> Option<&'static str> {
            *self.lacks.lock().unwrap()
        }

        fn listed(&self) -> Abilities {
            self.gives
        }
    }

    /// The features the panels heard of since the last call.
    fn told(heard: &mut tokio::sync::broadcast::Receiver<Arc<GatewayEvent>>) -> Vec<String> {
        let mut changed = Vec::new();
        while let Ok(event) = heard.try_recv() {
            if let GatewayEvent::AvailabilityChanged { features } = &*event {
                changed.extend(features.clone());
            }
        }
        changed
    }

    /// «Только дома» keeps the clouds by keys out: the station's cloud ways of hearing and voice are gone, and with
    /// its helpers away Yandex hears.
    #[test]
    fn the_regulator_takes_the_cloud_ways_of_hearing_and_voice_away() {
        let service = AvailabilityService::with(Profile { station: true }, true, true, EventBus::new());
        service.set_placement(Arc::new(At(Placement::StationServer)));
        let helper = Helper::giving(&Ability::V1);
        helper.online.store(false, Ordering::Relaxed);
        service.set_helpers(helper.clone());
        let runs = |id: &str| service.get(id).unwrap().runs;
        assert_eq!((runs("asr.own"), runs("voice.own")), (Some(Runs::Cloud), Some(Runs::Cloud)));
        helper.clouds.store(false, Ordering::Relaxed);
        assert!(!service.context().clouds);
        assert_eq!((runs("asr.own"), runs("voice.own")), (Some(Runs::Yandex), Some(Runs::Yandex)));
        let brain = service.get("brain.cloud").unwrap();
        assert_eq!((brain.state, brain.reason.as_str()), (State::Off, "Облака закрыты регулятором"));
    }

    #[tokio::test]
    async fn the_station_with_a_helper_takes_svoi_while_it_is_on_the_line_and_says_when_it_changes() {
        let events = EventBus::new();
        let mut heard = events.subscribe();
        let service = Arc::new(AvailabilityService::with(Profile { station: true }, true, false, events.clone()));
        service.set_placement(Arc::new(At(Placement::StationServer)));
        let helper = Helper::giving(&Ability::V1);
        service.set_helpers(helper.clone());
        service.recheck();
        told(&mut heard);

        assert!(service.check(PanelMode::Own).is_ok());
        let asr = service.get("asr.own").unwrap();
        assert_eq!((asr.state, asr.runs, asr.reason.as_str()), (State::Ready, Some(Runs::Helper), "у Турбо"));
        let view = service.view();
        assert_eq!((view.home_server, view.helpers.online, view.helpers.up), (HomeServer::Up, true, Abilities::V1));
        helper.online.store(false, Ordering::Relaxed);
        assert_eq!(service.check(PanelMode::Own), Err("«Ваш ИИ» сейчас не включить: Турбо не на связи.".to_string()));
        assert_eq!(service.absent(PanelMode::Own), None, "only for now: a stored «Свой» is kept");
        assert_eq!(service.get("asr.own").unwrap().reason, "Турбо не на связи — через облако");
        assert_eq!(service.view().home_server, HomeServer::Down, "the old view for the panels of 0.11");
        service.recheck();
        let changed = told(&mut heard);
        assert!(changed.contains(&"mode.own".to_string()) && changed.contains(&"asr.own".to_string()));
        service.recheck();
        assert!(heard.try_recv().is_err(), "nothing new, nothing told");

        // The station by itself (docs/plans/speaker-alone.md): «Свой» through the cloud's hearing and voice, not without.
        let alone = AvailabilityService::with(Profile { station: true }, true, false, EventBus::new());
        assert_eq!(alone.check(PanelMode::Own), Err("«Ваш ИИ» сейчас не включить: нет облачного голоса.".to_string()));
        assert!(alone.absent(PanelMode::Own).is_some(), "a stored «Свой» without a cloud voice is let go");
        let both = AvailabilityService::with(Profile { station: true }, true, true, EventBus::new());
        assert!(both.check(PanelMode::Own).is_ok() && both.absent(PanelMode::Own).is_none());
        assert_eq!(alone.view().home_server, HomeServer::None);
    }

    /// «Своё» на колонке без помощника со слухом — отказ словами, как сейчас: the helper on the line hears nothing and
    /// no cloud hears for it — the switch refuses with why, the panel sees «Свой» off with the same words; a stored one
    /// is kept, the gateway leaves it by itself. The same whether the helper is the station's computer («колонка +
    /// сервер», where 0.11 had it: review G2) or another. The station without a helper at all — the words it always had.
    #[tokio::test]
    async fn svoi_on_the_station_without_a_helper_with_hearing_is_refused_with_words() {
        for placement in [Placement::Station, Placement::StationServer] {
            let events = EventBus::new();
            let mut heard = events.subscribe();
            let service = Arc::new(AvailabilityService::with(Profile { station: true }, false, false, events.clone()));
            service.set_placement(Arc::new(At(placement)));
            let deaf =
                [Ability::Voice, Ability::Model, Ability::Messages, Ability::Apps, Ability::History, Ability::Catalog];
            let helper = Helper::giving(&deaf);
            *helper.lacks.lock().unwrap() = Some(crate::helpers::NO_HEARING);
            service.set_helpers(helper.clone());
            service.recheck();
            told(&mut heard);

            assert_eq!(
                service.check(PanelMode::Own),
                Err("«Ваш ИИ» сейчас не включить: нет Турбо со слухом.".to_string()),
                "{placement:?}"
            );
            let own = service.get("mode.own").unwrap();
            assert_eq!((own.state, own.reason.as_str()), (State::Off, "Нет Турбо со слухом"), "{placement:?}");
            let asr = service.get("asr.own").unwrap();
            assert_eq!((asr.state, asr.reason.as_str()), (State::Fallback, "нет Турбо со слухом — у Яндекса"));
            assert_eq!(service.absent(PanelMode::Own), None, "{placement:?}: a stored «Свой» stays, left by itself");

            // Its hearing given: «Свой» may be chosen, the panels hear it.
            *helper.lacks.lock().unwrap() = None;
            assert!(service.check(PanelMode::Own).is_ok(), "{placement:?}");
            service.recheck();
            assert!(told(&mut heard).contains(&"mode.own".to_string()), "{placement:?}");
        }

        // No helper at all: the station alone, as before the helpers.
        let alone = AvailabilityService::with(Profile { station: true }, false, false, EventBus::new());
        alone.set_placement(Arc::new(At(Placement::Station)));
        // No cloud hearing set up either (docs/plans/speaker-alone.md): the first that is missing is said.
        assert_eq!(
            alone.check(PanelMode::Own),
            Err("«Ваш ИИ» сейчас не включить: нет облачного распознавания.".to_string())
        );
        assert!(alone.absent(PanelMode::Own).is_some(), "a stored «Свой» without the cloud is let go");
    }

    /// A computer's build hears, speaks and thinks itself: its own heavy part counts as a helper on the line, the ways
    /// run here; the regulator still closes its clouds.
    #[cfg(feature = "pc")]
    #[test]
    fn a_computers_build_counts_its_own_heavy_part_as_a_helper_on_the_line() {
        let service = AvailabilityService::with(Profile { station: false }, false, false, EventBus::new());
        let asr = service.get("asr.own").unwrap();
        assert_eq!((asr.state, asr.runs, asr.reason.as_str()), (State::Ready, Some(Runs::Here), "на этом компьютере"));
        let view = service.view();
        assert_eq!(view.helpers, HelperContext::default().with_builtin(Abilities::V1));
        assert_eq!(view.home_server, HomeServer::None);
        assert_eq!(service.get("brain.local").unwrap().runs, Some(Runs::Here));

        // Its own parts all there, its list of helpers away and the regulator at «Только дома»: still here; only the
        // clouds go.
        let helper = Helper::beside(&Ability::V1, &Ability::V1);
        helper.online.store(false, Ordering::Relaxed);
        helper.clouds.store(false, Ordering::Relaxed);
        service.set_helpers(helper);
        assert_eq!(service.get("asr.own").unwrap().runs, Some(Runs::Here));
        assert!(service.check(PanelMode::Own).is_ok());
        assert_eq!(service.get("brain.cloud").unwrap().reason, "Облака закрыты регулятором");
    }

    /// A computer's build that takes a part from a helper of its list — having none of its own (stage 6) — says so: that
    /// way runs «у помощника», as «Где сейчас» says it, while what it gives itself, or what nobody gives, stays «на
    /// этом компьютере», as a computer's build always said.
    #[cfg(feature = "pc")]
    #[test]
    fn a_computers_build_says_a_part_a_helper_of_its_list_gives_is_that_helpers() {
        let service = AvailabilityService::with(Profile { station: false }, false, false, EventBus::new());
        service.set_helpers(Helper::giving(&[Ability::Hearing]));
        let asr = service.get("asr.own").unwrap();
        assert_eq!((asr.state, asr.runs, asr.reason.as_str()), (State::Ready, Some(Runs::Helper), "у Турбо"));
        let voice = service.get("voice.own").unwrap();
        assert_eq!((voice.state, voice.runs), (State::Ready, Some(Runs::Here)));
        assert_eq!(service.get("brain.local").unwrap().runs, Some(Runs::Here), "nobody else gives it: its own build");
        let view = service.view();
        assert!(!view.helpers.builtin.has(Ability::Hearing) && view.helpers.builtin.has(Ability::Voice));
        assert_eq!(view.helpers.up, Abilities::V1, "everything given, by one or the other");
    }

    /// The parts of a computer's own build without its engine or voice server at home.
    #[cfg(feature = "pc")]
    const NO_EARS_OR_VOICE: [Ability; 3] = [Ability::Apps, Ability::History, Ability::Catalog];

    /// A computer whose .env hears and speaks through clouds — no engine, no voice server at home — never hears or
    /// speaks «на этом компьютере» (review of stage 6): its hearing and voice are the clouds' ways, and «Только дома»
    /// keeps them in, as `/health` (asr «off») and «Где сейчас» («Регулятор не пускает облако») say.
    #[cfg(feature = "pc")]
    #[test]
    fn a_computers_cloud_hearing_and_voice_are_the_clouds_and_the_regulator_keeps_them_in() {
        let service = AvailabilityService::with(Profile { station: false }, true, true, EventBus::new());
        let own = Helper::beside(&NO_EARS_OR_VOICE, &[]);
        service.set_helpers(own.clone());
        let said = |id: &str| {
            let state = service.get(id).unwrap();
            (state.state, state.runs, state.reason)
        };
        let cloud = |none: &str| (State::Fallback, Some(Runs::Cloud), format!("{none} — через облако"));
        assert_eq!(said("asr.own"), cloud("нет Турбо со слухом"));
        assert_eq!(said("voice.own"), cloud("нет Турбо с голосом"));
        let view = service.view();
        assert!(!view.helpers.builtin.has(Ability::Hearing) && !view.helpers.builtin.has(Ability::Voice));
        assert!(view.helpers.builtin.has(Ability::Model), "nobody else gives it: its own build, as always");

        own.clouds.store(false, Ordering::Relaxed);
        let yandex = |none: &str| (State::Fallback, Some(Runs::Yandex), format!("{none} — у Яндекса"));
        assert_eq!(said("asr.own"), yandex("нет Турбо со слухом"));
        assert_eq!(said("voice.own"), yandex("нет Турбо с голосом"));
        assert!(service.check(PanelMode::Own).is_ok(), "«Свой» on a computer is not the registry's to refuse");
    }

    /// A part a helper of a computer's list would give stays that helper's while it is away or the regulator keeps it
    /// out (review of stage 6): never «на этом компьютере» meanwhile, as `/health` and «Где сейчас» say it is not there.
    #[cfg(feature = "pc")]
    #[test]
    fn a_part_a_helper_of_a_computers_list_would_give_is_never_its_own_while_that_helper_is_away_or_kept_out() {
        // The computer speaks itself and hears through a helper of its list (ASR_PROVIDER=none).
        let service = AvailabilityService::with(Profile { station: false }, false, false, EventBus::new());
        let mut itself = NO_EARS_OR_VOICE.to_vec();
        itself.push(Ability::Voice);
        let other = Helper::beside(&itself, &[Ability::Hearing]);
        service.set_helpers(other.clone());
        let asr = || {
            let state = service.get("asr.own").unwrap();
            (state.state, state.runs, state.reason)
        };
        assert_eq!(asr(), (State::Ready, Some(Runs::Helper), "у Турбо".to_string()));

        // Its link drops: the helper is away.
        other.online.store(false, Ordering::Relaxed);
        assert_eq!(asr(), (State::Fallback, Some(Runs::Yandex), "Турбо не на связи — у Яндекса".to_string()));
        let view = service.view();
        assert!(!view.helpers.builtin.has(Ability::Hearing) && !view.helpers.up.has(Ability::Hearing));
        assert_eq!(service.get("voice.own").unwrap().runs, Some(Runs::Here), "its own voice stays its own");

        // Back on the line, and kept out by «Только дома»: nobody hears for it, and it does not hear itself.
        other.online.store(true, Ordering::Relaxed);
        other.kept.store(true, Ordering::Relaxed);
        assert_eq!(asr(), (State::Fallback, Some(Runs::Yandex), "нет Турбо со слухом — у Яндекса".to_string()));
        assert!(!service.view().helpers.builtin.has(Ability::Hearing));
        // Let out again: the helper's once more.
        other.kept.store(false, Ordering::Relaxed);
        assert_eq!(asr(), (State::Ready, Some(Runs::Helper), "у Турбо".to_string()));
    }
}
