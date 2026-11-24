//! What this build is — the gateway for a PC or the one on the station itself — in one place: the parts it has. The
//! station build has none of the heavy parts (local model servers, the voice lab, calls, the station connect over
//! SSH, MCP servers as programs). What works where, the modes included, is the registry's (core/availability.rs):
//! the build is one of its inputs, the placement and the helpers the others.

use crate::core::availability::{evaluate, feature, mode_feature, Context, HelperContext, Placement};
use crate::core::helpers::contract::Abilities;
use crate::settings::definitions::PanelMode;

pub use crate::core::availability::OWN_ON_STATION;

/// The version of the API a computer's gateway and a station's speak to each other (/health `build.peerApi`): a
/// computer offers to update a station that speaks another before it moves anything onto it. 2 (0.5.1): the station
/// takes its update as the archive itself (`POST /api/v1/updates/archive`), no longer as base64 in JSON.
pub const PEER_API: u32 = 2;

/// Whether the computer's floor (crates/duowave-pc) is in this program: the computer's build says so at its start,
/// before the configuration is read (`duowave_pc::install`); the station's never does. What only a computer has —
/// the shop's models, the voice lab, calls, the station connect, MCP programs — follows it, not the profile's flag.
pub fn computer() -> bool {
    COMPUTER.load(std::sync::atomic::Ordering::Relaxed)
}

/// The computer's floor is in this program (`duowave_pc::install`).
pub fn set_computer() {
    COMPUTER.store(true, std::sync::atomic::Ordering::Relaxed);
}

static COMPUTER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
    /// the computer's floor's (crates/duowave-pc), in the program or not (`computer`), so they follow the program, not
    /// the profile's flag.
    pub fn features(self) -> Vec<&'static str> {
        if computer() {
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

/// The build alone, for tests and tools: the station without a helper; a computer gives its heavy parts itself.
impl ModeGate for Profile {
    fn check(&self, mode: PanelMode) -> Result<(), String> {
        let helpers =
            if self.station { HelperContext::default() } else { HelperContext::default().with_builtin(Abilities::V1) };
        let context = Context {
            pc_build: !self.station,
            placement: self.placement(),
            helpers,
            cloud_asr: true,
            cloud_tts: true,
            clouds: true,
            yandex: true,
            memory_free_mb: None,
            transcoder: false,
            transcode_on_station: false,
            own_lacks: None,
            midi: false,
            turbo_midi: false,
        };
        let state = evaluate(feature(mode_feature(mode)).expect("every mode is a feature"), &context);
        if state.on() {
            Ok(())
        } else {
            Err(state.reason)
        }
    }
}

/// The room on the station's /data an update never takes: Alice and the system write there too, and so does our
/// database's journal. A /data filled to the last byte breaks Alice, not only us.
pub const STATION_FLOOR_KB: u64 = 5 * 1024;

/// What the next version of a program this size may add: the gateway grows from release to release (0.7.1 → 0.8.0:
/// 17.5 → 20.4 MB) — 15 % of it, never under 2 MB.
pub fn growth_kb(program_bytes: u64) -> u64 {
    (program_bytes / 1024 * 15 / 100).max(2 * 1024)
}

/// The room on the station's disk kept for the gateway's own next update while apps are put there: its growth and
/// the floor. The new version is tried from memory and takes the old one's place (`move_to_disk`), so the update
/// needs no room for a second program — unless memory is short too, and then it says how much to free.
pub fn update_reserve_kb(program_bytes: u64) -> u64 {
    growth_kb(program_bytes) + STATION_FLOOR_KB
}

/// Where the gateway's program lies on the station's disk: the one `gateway-bin` names (placement/station_fs.rs), not
/// the running program — a trial runs from memory. The station's layout gives it (app.rs); none until then.
static STATION_PROGRAM: std::sync::OnceLock<fn() -> std::path::PathBuf> = std::sync::OnceLock::new();

/// The station's layout says where its program lies; the first call counts.
pub fn station_program_by(program: fn() -> std::path::PathBuf) {
    let _ = STATION_PROGRAM.set(program);
}

/// The gateway's program on the station's disk, once the layout said where it lies.
pub fn station_program() -> Option<std::path::PathBuf> {
    STATION_PROGRAM.get().map(|program| program())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The build alone counts a cloud's hearing and voice as set up: the station's build can take «Свой» through them.
    #[test]
    fn the_station_takes_svoi_through_the_cloud_a_pc_takes_all_three() {
        let station = Profile { station: true };
        assert!(station.check(PanelMode::Own).is_ok());
        assert!(station.check(PanelMode::Gateway).is_ok() && station.check(PanelMode::Relay).is_ok());
        let pc = Profile { station: false };
        for mode in [PanelMode::Relay, PanelMode::Gateway, PanelMode::Own] {
            assert!(pc.check(mode).is_ok());
        }
    }
}
