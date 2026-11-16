//! The new version tried from memory on the station (docs/plans/station-update-space.md, variant «Б»).
//!
//! The station's /data has some 30 MB to spare, and a running program keeps its room on the disk even once its file is
//! removed: an update that writes the new program beside the running one needs room for both. So on the station an
//! update's program is unpacked into /tmp (memory), asked its version, and the gateway becomes it (exec: the same
//! process for the station's service) — while the program on the disk stays as it was. That is the way back, and the
//! station's own service takes it by itself, with the gateway.sh every station already has: after any end of the
//! gateway — a fall, a hang the watch cuts short, a reboot — it starts the program on the disk. The old version,
//! started so, finds the trial's mark, notes that the new one did not make it (`TrialFailed`) and frees the memory.
//!
//! After `TRIAL` of good work the new program takes the old one's place on the disk (`install::move_to_disk`); after
//! a quiet minute — nobody talks to the station, nothing plays — the gateway becomes the program on the disk, and the
//! memory is free again. Short of memory, the station's apps are switched off for the trial (not as the owner's
//! choice: the restart from the disk brings them back), and with memory short and the disk roomy the update goes
//! beside the old program as before (`Way::Beside`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::install::{same_file, STATION_FLOOR_KB};

/// How long the new version works from memory before it takes the old one's place.
pub const TRIAL: Duration = Duration::from_secs(600);
/// How often a trial that waits — for a quiet minute, for room on the disk — looks again.
pub const LOOK_EVERY: Duration = Duration::from_secs(15);
/// Looks in a row with the station quiet before the gateway restarts from the disk: a minute.
pub const QUIET_LOOKS: u32 = 4;
/// A move to the disk that failed is tried again after this long.
pub const RETRY_MOVE: Duration = Duration::from_secs(300);
/// How long the new program has to name its version.
pub const ANSWER_WAIT: Duration = Duration::from_secs(20);

/// The trial's mark (`/data/opt/duowave/trying`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trying {
    pub version: String,
    /// The new program in memory.
    pub program: PathBuf,
    pub sha256: String,
    /// Unix seconds.
    pub started: i64,
    /// The boot it began in: a mark of another boot is a trial a restart of the station ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot: Option<String>,
    /// The station's apps were switched off for it: the trial does not start them.
    #[serde(default)]
    pub apps_paused: bool,
}

impl Trying {
    /// Unix seconds when its trial is over.
    pub fn until(&self) -> i64 {
        self.started + TRIAL.as_secs() as i64
    }
}

/// A trial that ended without its move to the disk (`/data/opt/duowave/trial-failed`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialFailed {
    pub version: String,
    /// RFC 3339.
    pub at: String,
    /// `crashed`: the new version fell or hung, and the one before came back; `rebooted`: the station restarted
    /// during it.
    pub why: String,
}

impl TrialFailed {
    /// For people.
    pub fn text(&self) -> String {
        if self.why == "rebooted" {
            crate::tr!(
                "Проба версии {version} прервалась: колонку перезагрузили, работает прежняя.",
                version = self.version
            )
        } else {
            crate::tr!(
                "Версия {version} не прошла пробу: упала или зависла, вернулась прежняя.",
                version = self.version
            )
        }
    }

    /// Why, after «Версия X не поставилась:».
    pub fn reason(&self) -> String {
        if self.why == "rebooted" {
            crate::tr!("колонку перезагрузили во время пробы — работает прежняя")
        } else {
            crate::tr!("упала или зависла на пробе — вернулась прежняя")
        }
    }

    /// Whether the new version itself failed (not a power cut): the computer does not offer it by itself again.
    pub fn crashed(&self) -> bool {
        self.why != "rebooted"
    }
}

/// What a start of the gateway on the station finds of a trial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtStart {
    None,
    /// This process is the trial: the new program from memory.
    Trying(Trying),
    /// The trial ended without its move to the disk: the program on the disk runs again. Why: `crashed`, `rebooted`.
    Ended(Trying, &'static str),
    /// The move to the disk was done and the gateway started from it; only the mark was left.
    Done(Trying),
}

/// What a trial's mark found at a start means: `running` is the running program, `program` the one on the disk,
/// `version` the running one's, `boot` this boot's id.
pub fn at_start(mark: Option<Trying>, running: &Path, program: &Path, version: &str, boot: Option<&str>) -> AtStart {
    let Some(mark) = mark else { return AtStart::None };
    if same_file(running, &mark.program) {
        return AtStart::Trying(mark);
    }
    if same_version(&mark.version, version) && same_file(running, program) {
        return AtStart::Done(mark);
    }
    let rebooted = matches!((mark.boot.as_deref(), boot), (Some(then), Some(now)) if then != now);
    AtStart::Ended(mark, if rebooted { "rebooted" } else { "crashed" })
}

fn same_version(a: &str, b: &str) -> bool {
    a.trim_start_matches('v') == b.trim_start_matches('v')
}

/// How an update goes on the station, from the room it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// Tried from memory, then put in the old program's place.
    Trial,
    /// Beside the old program on the disk, as before: memory short, the disk roomy.
    Beside,
    /// Tried from memory once the station's apps give their memory back.
    PauseApps,
    /// Not even the trial's way fits the disk.
    NoRoom,
}

/// The room an update has on the station, KB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Room {
    /// The new program.
    pub new_kb: u64,
    /// The one on the disk now.
    pub old_kb: u64,
    /// Free on the disk, with what this update removes counted in (a kept `.previous`).
    pub free_kb: Option<u64>,
    /// Memory available now (MemAvailable).
    pub available_kb: Option<u64>,
    /// What stays for the system and Alice (PLUGINS_RESERVE_MB).
    pub reserve_kb: u64,
}

impl Room {
    /// The disk a trial needs: the program's growth, and the floor — the new program takes the old one's place.
    pub fn disk_for_trial(&self) -> u64 {
        self.new_kb.saturating_sub(self.old_kb) + STATION_FLOOR_KB
    }

    /// The disk an update beside the old program needs.
    pub fn disk_beside(&self) -> u64 {
        self.new_kb + STATION_FLOOR_KB
    }

    /// The memory a trial needs: the program, and the reserve the system and Alice keep.
    pub fn memory_for_trial(&self) -> u64 {
        self.new_kb + self.reserve_kb
    }

    pub fn memory_fits(&self) -> bool {
        self.available_kb.is_none_or(|available| available >= self.memory_for_trial())
    }

    fn free(&self) -> u64 {
        self.free_kb.unwrap_or(u64::MAX)
    }

    /// The way: the trial when it fits; beside when memory does not and the disk does; the trial with the apps
    /// switched off when only that fits; else nothing.
    pub fn way(&self) -> Way {
        let disk_for_trial = self.free() >= self.disk_for_trial();
        if disk_for_trial && self.memory_fits() {
            Way::Trial
        } else if self.free() >= self.disk_beside() {
            Way::Beside
        } else if disk_for_trial {
            Way::PauseApps
        } else {
            Way::NoRoom
        }
    }

    /// Why there is no room, for people.
    pub fn no_room(&self) -> String {
        let (need, free) = (self.disk_for_trial(), self.free_kb.unwrap_or(0));
        crate::tr!(
            "На колонке свободно {free} МБ, а обновлению нужно {need} МБ. Освободите {short} МБ — например, удалите приложение.",
            free = free / 1024,
            need = need.div_ceil(1024),
            short = need.saturating_sub(free).div_ceil(1024).max(1)
        )
    }

    /// Why the new program does not fit beside the old one (an update from the console, which does not restart the
    /// gateway into a trial), for people.
    pub fn no_room_beside(&self) -> String {
        let (need, free) = (self.disk_beside(), self.free_kb.unwrap_or(0));
        crate::tr!(
            "На колонке свободно {free} МБ, а новой версии рядом со старой нужно {need} МБ. Освободите {short} МБ.",
            free = free / 1024,
            need = need.div_ceil(1024),
            short = need.saturating_sub(free).div_ceil(1024).max(1)
        )
    }

    /// Why the trial does not fit memory even with the apps switched off, for people.
    pub fn no_memory(&self) -> String {
        let free = self.free_kb.unwrap_or(0);
        crate::tr!(
            "Колонке не хватает ОЗУ на пробу новой версии: нужно {need} МБ, свободно {available} МБ даже без приложений. Освободите на диске {short} МБ — тогда версия встанет рядом со старой без пробы.",
            need = self.memory_for_trial().div_ceil(1024),
            available = self.available_kb.unwrap_or(0) / 1024,
            short = self.disk_beside().saturating_sub(free).div_ceil(1024).max(1)
        )
    }
}

/// The trial as the panel shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialView {
    pub version: String,
    /// RFC 3339: when it began, when it is over.
    pub since: String,
    pub until: String,
    /// `trying` — works from memory; `waiting` — on the disk, waits for a quiet minute to restart from it; `stuck` —
    /// could not go onto the disk yet (`message` says why).
    pub phase: &'static str,
    pub apps_paused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl TrialView {
    pub fn of(mark: &Trying) -> TrialView {
        TrialView {
            version: mark.version.clone(),
            since: rfc3339(mark.started),
            until: rfc3339(mark.until()),
            phase: "trying",
            apps_paused: mark.apps_paused,
            message: None,
        }
    }
}

pub fn rfc3339(seconds: i64) -> String {
    chrono::DateTime::from_timestamp(seconds, 0)
        .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

pub fn now_seconds() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(dir: &Path) -> Trying {
        Trying {
            version: "0.13.0".into(),
            program: dir.join("duowave-0.13.0"),
            sha256: "ab".into(),
            started: 1_700_000_000,
            boot: Some("boot-1".into()),
            apps_paused: false,
        }
    }

    #[test]
    fn a_start_tells_the_trial_from_its_end() {
        let dir = tempfile::tempdir().unwrap();
        let disk = dir.path().join("duowave");
        let memory = dir.path().join("duowave-0.13.0");
        std::fs::write(&disk, b"old").unwrap();
        std::fs::write(&memory, b"new").unwrap();
        let mark = mark(dir.path());
        assert_eq!(at_start(None, &disk, &disk, "0.12.0", Some("boot-1")), AtStart::None);
        assert_eq!(
            at_start(Some(mark.clone()), &memory, &disk, "0.13.0", Some("boot-1")),
            AtStart::Trying(mark.clone())
        );
        // The program on the disk runs again: the new one fell (same boot) or the station restarted.
        assert_eq!(
            at_start(Some(mark.clone()), &disk, &disk, "0.12.0", Some("boot-1")),
            AtStart::Ended(mark.clone(), "crashed")
        );
        assert_eq!(
            at_start(Some(mark.clone()), &disk, &disk, "0.12.0", Some("boot-2")),
            AtStart::Ended(mark.clone(), "rebooted")
        );
        // The new version on the disk with the mark still there: the move was done.
        assert_eq!(at_start(Some(mark.clone()), &disk, &disk, "v0.13.0", Some("boot-1")), AtStart::Done(mark));
    }

    #[test]
    fn the_way_follows_the_room() {
        // The station of 14.11: 20.4 MB of program, a new one a little bigger; 123 MB of memory, 40 kept.
        let room = Room {
            new_kb: 21 * 1024,
            old_kb: 20 * 1024,
            free_kb: Some(10 * 1024),
            available_kb: Some(123 * 1024),
            reserve_kb: 40 * 1024,
        };
        assert_eq!(room.disk_for_trial(), 6 * 1024, "the growth and the floor");
        assert_eq!(room.way(), Way::Trial);
        let short_of_memory = Room { available_kb: Some(50 * 1024), ..room };
        assert_eq!(short_of_memory.way(), Way::PauseApps, "the disk is tight: the apps give their memory");
        let roomy = Room { free_kb: Some(30 * 1024), ..short_of_memory };
        assert_eq!(roomy.way(), Way::Beside, "the disk has room for both: nothing is switched off");
        let full = Room { free_kb: Some(5 * 1024), ..room };
        assert_eq!(full.way(), Way::NoRoom);
        assert!(full.no_room().contains("Освободите 1 МБ"), "{}", full.no_room());
        assert_eq!(Room { available_kb: None, ..room }.way(), Way::Trial, "memory unknown: the trial");
    }
}
