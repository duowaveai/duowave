//! The plugins' memory and room (docs/plans/plugins.md, «Память и место на колонке», stage 3). On a computer there is
//! plenty of both; on the station (Lite 2: 238 MB of memory, about 137 free, no swap, about 60 MB free in /data) a lack
//! of memory brings the OOM killer, which may take Alice's `quasar` along with the gateway. So the budget is counted
//! before, not after a fall:
//!
//! ```text
//! available to the station (MemAvailable, now)   ≈ 137 MB  — the gateway's own memory is already out of it
//! + what the plugins' engines hold now           (they give it back when unloaded)
//! − the reserve for the system and Alice           40 MB   (PLUGINS_RESERVE_MB)
//! = the plugins' budget                          ≈  80 MB
//! ```
//!
//! - Each plugin that is on takes its declared `budget.memoryMb`, not what it holds now: the budget must not lie at a
//!   plugin's peak.
//! - Switching one on is checked against what is free; on the station a plugin that does not fit stays off, and the
//!   panel says how much it needs and how much is free.
//! - Room on the disk the same way: what a package unpacks into and its store, with a reserve kept for the gateway's
//!   own update (PLUGINS_STORAGE_RESERVE_MB, 15 MB: `.previous` alone is 12).
//! - Under pressure (MemAvailable below the reserve, or the system's own pressure at «warn»): the contexts of plugins
//!   idle longer than a minute are unloaded and their work in the background waits. At «critical» on the station the
//!   plugin switched on last is switched off (PLUGINS_PRESSURE_ACTION=warn only says so: the owner's open decision 3).
//!   The built-in skills and the voice path are never touched.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use crate::core::availability::Placement;
use crate::operations::system_memory::{self, Pressure};

/// What a plugin that is idle this long gives back under pressure.
pub const PRESSED_IDLE_MS: u64 = 60_000;
/// Background calls kept while they wait for the pressure to ease; older ones go first.
pub const DEFERRED_MAX: usize = 50;

/// What the machine says of its memory now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryNow {
    pub total_mb: u64,
    /// What can be taken without swapping (MemAvailable on Linux).
    pub available_mb: u64,
    pub pressure: Pressure,
    /// This gateway's resident memory, when it can tell.
    pub gateway_mb: Option<u64>,
}

/// Where the numbers come from: the machine itself, or a test's.
pub trait MemoryProbe: Send + Sync {
    fn now(&self) -> Option<MemoryNow>;
    fn free_disk_mb(&self, path: &Path) -> Option<u64>;
}

/// This machine: /proc on the station, sysctl and ps on a Mac.
pub struct MachineMemory;

impl MemoryProbe for MachineMemory {
    fn now(&self) -> Option<MemoryNow> {
        let counters = system_memory::read_counters()?;
        Some(MemoryNow {
            total_mb: counters.total_mb,
            available_mb: counters.pool_mb,
            pressure: counters.pressure,
            gateway_mb: system_memory::process_rss_mb(None),
        })
    }

    fn free_disk_mb(&self, path: &Path) -> Option<u64> {
        system_memory::free_disk_mb(path)
    }
}

/// What to do at «critical» on the station.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PressureAction {
    /// Switch the plugin switched on last off (the plan's choice).
    Disable,
    /// Only say so: the log and the panel.
    Warn,
}

impl PressureAction {
    pub fn parse(text: &str) -> Option<PressureAction> {
        match text.trim() {
            "disable" => Some(PressureAction::Disable),
            "warn" => Some(PressureAction::Warn),
            _ => None,
        }
    }
}

/// The plugins' free memory as the availability registry reads it (`Need::Memory`): kept by the host's watch, read
/// without waiting. Unknown until the first look.
#[derive(Debug)]
pub struct MemoryGauge(AtomicU64);

impl Default for MemoryGauge {
    fn default() -> MemoryGauge {
        MemoryGauge(AtomicU64::new(u64::MAX))
    }
}

impl MemoryGauge {
    pub fn set(&self, free_mb: Option<u64>) {
        self.0.store(free_mb.unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    pub fn get(&self) -> Option<u64> {
        Some(self.0.load(Ordering::Relaxed)).filter(|free| *free != u64::MAX)
    }
}

/// A plugin in the count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginShare {
    pub id: String,
    pub title: String,
    pub enabled: bool,
    pub loaded: bool,
    /// What it declared: what it takes of the budget while on.
    pub budget_mb: u64,
    /// What its engine holds now, while loaded.
    pub actual_mb: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskView {
    pub free_mb: Option<u64>,
    pub reserve_mb: u64,
    /// What the packages' folder holds.
    pub packages_mb: u64,
}

/// `GET /api/v1/packages/resources`: the plugins' memory and room, for «Память колонки» and the plugins' list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesView {
    pub placement: Placement,
    /// On the station the budget is kept (a plugin that does not fit stays off); on a computer it is shown.
    pub enforced: bool,
    /// Whether the machine told its memory at all.
    pub measured: bool,
    pub total_mb: u64,
    pub available_mb: u64,
    pub reserve_mb: u64,
    pub gateway_mb: Option<u64>,
    /// What the loaded plugins' engines hold now.
    pub plugins_mb: u64,
    /// The plugins' budget: what they may take at most, together.
    pub budget_mb: u64,
    /// What the plugins that are on declared.
    pub committed_mb: u64,
    pub free_mb: u64,
    pub pressure: Pressure,
    /// Under pressure the plugins' work in the background waits.
    pub deferring: bool,
    pub action: PressureAction,
    pub disk: DiskView,
    pub plugins: Vec<PluginShare>,
    /// «Можно поставить ещё плагины примерно на 60 МБ».
    pub note: String,
}

/// The count itself, from the machine's numbers and the plugins'.
pub struct Count {
    pub budget_mb: u64,
    pub committed_mb: u64,
    pub free_mb: u64,
    pub plugins_mb: u64,
}

pub fn count(memory: Option<MemoryNow>, reserve_mb: u64, plugins: &[PluginShare]) -> Count {
    let plugins_mb: u64 = plugins.iter().filter_map(|plugin| plugin.actual_mb).sum();
    let committed_mb: u64 = plugins.iter().filter(|plugin| plugin.enabled).map(|plugin| plugin.budget_mb).sum();
    let budget_mb = memory.map(|memory| (memory.available_mb + plugins_mb).saturating_sub(reserve_mb)).unwrap_or(0);
    Count { budget_mb, committed_mb, free_mb: budget_mb.saturating_sub(committed_mb), plugins_mb }
}

/// What the panel says under the bar.
pub fn note(free_mb: u64, measured: bool, pressure: Pressure) -> String {
    if !measured {
        return "Сколько памяти свободно, шлюз здесь сказать не может.".into();
    }
    let room = match free_mb {
        0 => "Новых плагинов больше не поставить: память занята.".to_string(),
        free => format!("Можно поставить ещё плагины примерно на {free} МБ."),
    };
    match pressure {
        Pressure::Normal => room,
        Pressure::Warn => format!("{room} Памяти мало: простаивающие плагины выгружаются, фоновая работа ждёт."),
        Pressure::Critical => format!("{room} Памяти очень мало: плагины выгружаются."),
    }
}

/// Why a plugin does not fit, or nothing when it does.
pub fn does_not_fit(need_mb: u64, free_mb: u64) -> Option<String> {
    (need_mb > free_mb).then(|| {
        format!(
            "не хватит памяти колонки: нужно {need_mb} МБ, свободно {free_mb}. Выключите другой плагин или перенесите этот на домашний сервер"
        )
    })
}

/// Why a package does not fit on the disk, or nothing when it does.
pub fn no_room(need_mb: u64, free_mb: Option<u64>, reserve_mb: u64) -> Option<String> {
    let free = free_mb?;
    (need_mb + reserve_mb > free).then(|| {
        format!(
            "не хватит места: нужно {need_mb} МБ и ещё {reserve_mb} МБ про запас на обновление шлюза, свободно {free}"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share(id: &str, enabled: bool, budget_mb: u64, actual_mb: Option<u64>) -> PluginShare {
        PluginShare { id: id.into(), title: id.into(), enabled, loaded: actual_mb.is_some(), budget_mb, actual_mb }
    }

    #[test]
    fn counts_the_plans_example_by_what_plugins_declare_not_what_they_hold() {
        // The station of 07.11: 137 MB available with the gateway running, the reserve 40.
        let memory = MemoryNow { total_mb: 238, available_mb: 137, pressure: Pressure::Normal, gateway_mb: Some(14) };
        let plugins =
            [share("radio", true, 4, Some(1)), share("weather", true, 8, None), share("off", false, 30, None)];
        let count = count(Some(memory), 40, &plugins);
        assert_eq!(count.plugins_mb, 1);
        assert_eq!(count.budget_mb, 98, "what the loaded radio holds comes back to the budget");
        assert_eq!(count.committed_mb, 12, "on: 4 + 8 declared; off takes nothing");
        assert_eq!(count.free_mb, 86);
        assert_eq!(note(86, true, Pressure::Normal), "Можно поставить ещё плагины примерно на 86 МБ.");
        assert!(note(0, true, Pressure::Warn).contains("Памяти мало"));
        let starving = MemoryNow { available_mb: 30, ..memory };
        assert_eq!(super::count(Some(starving), 40, &plugins).free_mb, 0, "below the reserve nothing is free");
        assert_eq!(super::count(None, 40, &plugins).budget_mb, 0);
    }

    #[test]
    fn says_how_much_is_needed_and_how_much_is_free() {
        assert_eq!(does_not_fit(4, 7), None);
        let why = does_not_fit(12, 7).unwrap();
        assert!(why.contains("нужно 12 МБ, свободно 7"), "{why}");
        assert_eq!(no_room(2, Some(60), 15), None);
        assert!(no_room(50, Some(60), 15).unwrap().contains("свободно 60"));
        assert_eq!(no_room(50, None, 15), None, "a disk that does not tell is not refused");
        let gauge = MemoryGauge::default();
        assert_eq!(gauge.get(), None);
        gauge.set(Some(80));
        assert_eq!(gauge.get(), Some(80));
        assert_eq!(PressureAction::parse("warn"), Some(PressureAction::Warn));
        assert_eq!(PressureAction::parse("panic"), None);
    }
}

/// A machine of a test's own: its memory as the test sets it.
#[cfg(test)]
pub mod fake {
    use std::sync::{Arc, Mutex};

    use super::*;

    pub struct FixedMemory {
        pub now: Mutex<Option<MemoryNow>>,
        pub disk_mb: Option<u64>,
    }

    impl FixedMemory {
        pub fn set(&self, now: MemoryNow) {
            *self.now.lock().unwrap() = Some(now);
        }
    }

    impl MemoryProbe for FixedMemory {
        fn now(&self) -> Option<MemoryNow> {
            *self.now.lock().unwrap()
        }

        fn free_disk_mb(&self, _path: &Path) -> Option<u64> {
            self.disk_mb
        }
    }

    /// A computer with memory to spare.
    pub fn plenty() -> Arc<FixedMemory> {
        let now = MemoryNow { total_mb: 16_384, available_mb: 8_192, pressure: Pressure::Normal, gateway_mb: Some(40) };
        Arc::new(FixedMemory { now: Mutex::new(Some(now)), disk_mb: Some(100_000) })
    }
}
