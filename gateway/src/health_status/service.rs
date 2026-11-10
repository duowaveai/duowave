//! The gateway's health, read live: the mode and the voice change from the panel without a restart, so nothing here
//! is taken at startup. Who sees the details is the controller's question.
//!
//! The status and the mode are filled here. The rest comes from the modules through `HealthSources` (`add_sources`):
//! the speech, the dialog and its brain, the voice and the skills from one source (src/links), the station sessions
//! from the station link's. A field takes the first source that answers it; until one does, it is left out of the
//! answer, and the panel shows «—» for it.

use std::sync::{Arc, RwLock};

use serde::Serialize;
use serde_json::Value;

use crate::core::build::{ModeGate, Profile};
use crate::requests::ModeSource;
use crate::settings::definitions::PanelMode;

/// What anyone may know: the gateway is up. Docker's and launchd's checks need nothing more.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicHealth {
    pub status: &'static str,
}

/// Whether a station is connected, and when one was last seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StationHealth {
    pub connected: bool,
    pub last_seen_at: String,
}

/// What this build of the gateway is and can do (src/core/build.rs): the panel hides the parts it has not (the
/// station's own gateway has no local models, voice lab or calls) and greys out the modes it does not allow now («Свой»
/// on the station alone). The Node gateway sent none: the panel then assumes a PC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfo {
    pub version: &'static str,
    /// The build that runs on the station itself.
    pub station: bool,
    /// models, voiceLab, calls, stationConnect, mcpPrograms: what the build has beyond the core.
    pub features: Vec<&'static str>,
    /// The modes the panel may switch to now: what works where (src/core/availability.rs) says.
    pub modes: Vec<PanelMode>,
    /// The API a computer's gateway and a station's speak to each other.
    pub peer_api: u32,
    /// The settings package's schema this build's import knows (src/placement/transfer.rs): the computer sends the
    /// station's gateway what its import takes. A build from before it was said knows schema 1 (placement/watch.rs).
    pub settings_schema: u32,
}

pub fn build_info(gate: &dyn ModeGate) -> BuildInfo {
    let profile = crate::core::build::Profile::this();
    BuildInfo {
        version: crate::VERSION,
        station: profile.station,
        features: profile.features(),
        modes: crate::settings::definitions::PANEL_MODES
            .iter()
            .copied()
            .filter(|mode| gate.check(*mode).is_ok())
            .collect(),
        peer_api: crate::core::build::PEER_API,
        settings_schema: crate::placement::transfer::SCHEMA,
    }
}

/// Where the gateway runs and what of it the other side needs to know (src/placement): the computer reads the
/// station's, the panels both.
pub trait PlacementHealthSource: Send + Sync {
    fn health(&self) -> Value;
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthStatus {
    pub status: &'static str,
    pub build: BuildInfo,
    /// gateway: we answer the station; relay: its session goes to Yandex and is only journaled.
    pub mode: &'static str,
    /// The panel's three: relay, gateway, or own (the gateway without Yandex).
    pub panel_mode: PanelMode,
    // ↓ From modules ported apart (HealthSources): absent until their part answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_sessions: Option<usize>,
    /// `{vad, asr}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speech: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dialog: Option<String>,
    /// The same model for people: provider, profiles, effort, and whether it is outside the house.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brain: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tts: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub station: Option<StationHealth>,
    /// Gateway mode only: who serves each capability, what is ours, and the counters since the start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Value>,
    /// Where the gateway runs (src/placement): its role, placement, the station's space and memory, its home server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement: Option<Value>,
}

/// The parts of /health that modules ported apart know. Each defaults to None (left out of the answer).
pub trait HealthSources: Send + Sync {
    /// Station sessions open now (src/gateway/uniproxy: the session registry's active count).
    fn active_sessions(&self) -> Option<usize> {
        None
    }
    /// The speech setup, `{vad, asr}` (src/speech: SpeechService::status).
    fn speech(&self) -> Option<Value> {
        None
    }
    /// The dialog's description (src/dialog: DialogService::description).
    fn dialog(&self) -> Option<String> {
        None
    }
    /// The brain as BrainStatus (src/dialog: DialogService::brain).
    fn brain(&self) -> Option<Value> {
        None
    }
    /// Whose voice speaks, after the mode (src/speech: VoiceService::describe).
    fn tts(&self, _yandex_allowed: bool) -> Option<String> {
        None
    }
    /// Whether a station is connected, and when one was last seen (src/operations: StationWatch::status).
    fn station(&self) -> Option<StationHealth> {
        None
    }
    /// `{routes, implemented, served, unsupported}` (src/skills), asked only in gateway mode.
    fn skills(&self) -> Option<Value> {
        None
    }
}

pub struct HealthStatusService {
    modes: Arc<dyn ModeSource>,
    /// In the order they joined: a field takes the first that answers it.
    sources: RwLock<Vec<Arc<dyn HealthSources>>>,
    placement: RwLock<Option<Arc<dyn PlacementHealthSource>>>,
    gate: RwLock<Option<Arc<dyn ModeGate>>>,
}

impl HealthStatusService {
    pub fn new(modes: Arc<dyn ModeSource>) -> HealthStatusService {
        HealthStatusService {
            modes,
            sources: RwLock::new(Vec::new()),
            placement: RwLock::new(None),
            gate: RwLock::new(None),
        }
    }

    pub fn set_placement(&self, source: Arc<dyn PlacementHealthSource>) {
        *self.placement.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(source);
    }

    /// Which modes may be switched to now: the registry; without it, the build alone.
    pub fn set_gate(&self, gate: Arc<dyn ModeGate>) {
        *self.gate.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(gate);
    }

    /// A module's parts join /health, after those that joined before.
    pub fn add_sources(&self, sources: Arc<dyn HealthSources>) {
        self.sources.write().unwrap_or_else(|poisoned| poisoned.into_inner()).push(sources);
    }

    /// The same as `add_sources`: a module joins beside the others.
    pub fn set_sources(&self, sources: Arc<dyn HealthSources>) {
        self.add_sources(sources);
    }

    pub fn status(&self) -> HealthStatus {
        let panel_mode = self.modes.current();
        let mode = if panel_mode == PanelMode::Relay { "relay" } else { "gateway" };
        let sources = self.sources.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let gate = self.gate.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let build = match gate {
            Some(gate) => build_info(gate.as_ref()),
            None => build_info(&Profile::this()),
        };
        let placement = self.placement.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        HealthStatus {
            status: "ok",
            build,
            mode,
            panel_mode,
            active_sessions: first(&sources, |s| s.active_sessions()),
            speech: first(&sources, |s| s.speech()),
            dialog: first(&sources, |s| s.dialog()),
            brain: first(&sources, |s| s.brain()),
            tts: first(&sources, |s| s.tts(panel_mode != PanelMode::Own)),
            station: first(&sources, |s| s.station()),
            skills: if mode == "gateway" { first(&sources, |s| s.skills()) } else { None },
            placement: placement.map(|source| source.health()),
        }
    }
}

/// The first source's answer to a part: the one that joined first wins.
fn first<T>(sources: &[Arc<dyn HealthSources>], read: impl Fn(&dyn HealthSources) -> Option<T>) -> Option<T> {
    sources.iter().find_map(|source| read(source.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    struct Mode(Mutex<PanelMode>);

    impl ModeSource for Mode {
        fn current(&self) -> PanelMode {
            *self.0.lock().unwrap()
        }
    }

    struct Parts;

    impl HealthSources for Parts {
        fn active_sessions(&self) -> Option<usize> {
            Some(1)
        }
        fn tts(&self, yandex_allowed: bool) -> Option<String> {
            Some(if yandex_allowed { "yandex" } else { "own" }.into())
        }
        fn skills(&self) -> Option<Value> {
            Some(json!({"routes": {}}))
        }
    }

    #[test]
    fn reports_the_mode_now_and_leaves_out_what_no_module_answers_yet() {
        let mode = Arc::new(Mode(Mutex::new(PanelMode::Own)));
        let health = HealthStatusService::new(mode.clone());
        let mut bare = serde_json::to_value(health.status()).unwrap();
        let build = bare.as_object_mut().unwrap().remove("build").expect("the build is always said");
        assert_eq!(bare, json!({"status": "ok", "mode": "gateway", "panelMode": "own"}));
        assert_eq!(build["version"], crate::VERSION);
        assert_eq!(build["settingsSchema"], crate::placement::transfer::SCHEMA, "what the computer sends it goes by");
        assert_eq!(build["station"], cfg!(feature = "station"));
        assert_eq!(build["features"].as_array().unwrap().iter().any(|f| f == "models"), cfg!(feature = "pc"));
        health.set_sources(Arc::new(Parts));
        let status = serde_json::to_value(health.status()).unwrap();
        assert_eq!(status["activeSessions"], 1);
        assert_eq!(status["tts"], "own", "«Свой» speaks with our voice");
        assert_eq!(status["skills"], json!({"routes": {}}));
        assert!(status.get("speech").is_none());
        *mode.0.lock().unwrap() = PanelMode::Relay;
        let relay = serde_json::to_value(health.status()).unwrap();
        assert_eq!((relay["mode"].as_str(), relay["tts"].as_str()), (Some("relay"), Some("yandex")));
        assert!(relay.get("skills").is_none(), "skills only in gateway mode");
    }

    /// The station link's sessions beside the modules' parts.
    struct Station;

    impl HealthSources for Station {
        fn active_sessions(&self) -> Option<usize> {
            Some(2)
        }
        fn station(&self) -> Option<StationHealth> {
            Some(StationHealth { connected: true, last_seen_at: "2026-11-06T02:11:00.331Z".into() })
        }
        fn tts(&self, _yandex_allowed: bool) -> Option<String> {
            Some("the station's".into())
        }
    }

    #[test]
    fn takes_each_part_from_the_first_source_that_answers_it() {
        let health = HealthStatusService::new(Arc::new(Mode(Mutex::new(PanelMode::Gateway))));
        health.add_sources(Arc::new(Parts));
        health.set_sources(Arc::new(Station));
        let status = serde_json::to_value(health.status()).unwrap();
        assert_eq!(status["activeSessions"], 1, "the first source that knows it");
        assert_eq!(status["tts"], "yandex");
        assert_eq!(status["station"], json!({"connected": true, "lastSeenAt": "2026-11-06T02:11:00.331Z"}));
        assert_eq!(status["skills"], json!({"routes": {}}));
        assert!(status.get("dialog").is_none(), "no source answers it");
    }
}
