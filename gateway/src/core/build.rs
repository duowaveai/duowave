//! What this build is — the gateway for a PC or the one on the station itself — in one place: the parts it has. The
//! station build has none of the heavy parts (local model servers, the voice lab, calls, the station connect over
//! SSH, MCP servers as programs). What works where, the modes included, is the registry's (core/availability.rs):
//! the build is one of its inputs, the placement and the home server the others.

use crate::core::availability::{evaluate, feature, mode_feature, Context, HomeServer, Placement};
use crate::settings::definitions::PanelMode;

pub use crate::core::availability::OWN_ON_STATION;

/// The version of the API a computer's gateway and a station's speak to each other (/health `build.peerApi`): a
/// computer offers to update a station that speaks another before it moves anything onto it. 2 (0.5.1): the station
/// takes its update as the archive itself (`POST /api/v1/updates/archive`), no longer as base64 in JSON.
pub const PEER_API: u32 = 2;

/// The build as the gateway sees it. `station` comes from the cargo feature; tests build either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    pub station: bool,
}

impl Profile {
    /// This build.
    pub const fn this() -> Profile {
        Profile { station: cfg!(feature = "station") }
    }

    /// The heavy parts the build has beyond the core: models, voiceLab, calls, stationConnect, mcpPrograms. They are
    /// compiled in or out (the `pc` feature), so they follow the build, not the profile's flag.
    pub fn features(self) -> Vec<&'static str> {
        if cfg!(feature = "pc") {
            vec!["models", "voiceLab", "calls", "stationConnect", "mcpPrograms"]
        } else {
            Vec::new()
        }
    }

    /// This build, or the role DUOWAVE_ROLE names (a computer's build playing the station's gateway).
    pub fn for_role(role: Option<&str>) -> Profile {
        match role {
            Some("station") => Profile { station: true },
            Some("pc") => Profile { station: false },
            _ => Profile::this(),
        }
    }

    /// Where a gateway of this build runs when nothing else is known: the station's on the station alone.
    pub fn placement(self) -> Placement {
        if self.station {
            Placement::Station
        } else {
            Placement::Pc
        }
    }
}

/// Whether a mode may be switched to now, and why not: the mode switch asks it (the registry, in the gateway).
pub trait ModeGate: Send + Sync {
    fn check(&self, mode: PanelMode) -> Result<(), String>;
    /// Whether the mode has no way at all here (not «not now»): a stored one is then let go at the start.
    fn absent(&self, mode: PanelMode) -> Option<String> {
        self.check(mode).err()
    }
}

/// The build alone, for tests and tools: the station without a home server.
impl ModeGate for Profile {
    fn check(&self, mode: PanelMode) -> Result<(), String> {
        let context = Context {
            pc_build: !self.station,
            placement: self.placement(),
            home_server: HomeServer::None,
            cloud_asr: true,
            cloud_tts: true,
            yandex: true,
            memory_free_mb: None,
            transcoder: false,
            transcode_on_station: false,
            own_lacks: None,
        };
        let state = evaluate(feature(mode_feature(mode)).expect("every mode is a feature"), &context);
        if state.on() {
            Ok(())
        } else {
            Err(state.reason)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_station_alone_refuses_svoi_a_pc_takes_all_three() {
        let station = Profile { station: true };
        assert_eq!(station.check(PanelMode::Own), Err(OWN_ON_STATION.to_string()));
        assert!(station.check(PanelMode::Gateway).is_ok() && station.check(PanelMode::Relay).is_ok());
        let pc = Profile { station: false };
        for mode in [PanelMode::Relay, PanelMode::Gateway, PanelMode::Own] {
            assert!(pc.check(mode).is_ok());
        }
    }
}
