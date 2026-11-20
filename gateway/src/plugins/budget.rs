//! The plugins' memory and room (docs/plans/plugins.md, «Память и место на колонке»). On a computer there is plenty of
//! both; on the station (Lite 2: 238 MB of memory, no swap, about 60 MB free in /data; 21.11 with the wake word and the
//! VPN on, MemAvailable 88.7 MB, the gateway 51.6 MB resident) a lack of memory brings the OOM killer, which may take
//! Alice's `quasar` along with the gateway. So the budget is counted before a plugin's context loads, not after a fall:
//!
//! ```text
//! available to the station (MemAvailable, now)   ≈  89 MB  — the gateway's and the apps' own memory already out of it
//! + what the plugins' engines hold now           (they give it back when unloaded)
//! − the reserve for the system and Alice           40 MB   (a sixth of the memory, 24–40; PLUGINS_RESERVE_MB)
//! = the plugins' budget                          ≈  49 MB
//! − what the loaded contexts take                          (idle: what they hold with a margin; in a call: declared)
//! = free: what one more context may take
//! ```
//!
//! - A context takes its plugin's declared `budget.memoryMb` while it is loaded — the engine's own hard limit
//!   (quickjs.rs), so the budget does not lie at a plugin's peak — and nothing while it is not. A plugin that is on but
//!   not loaded takes nothing, and idle contexts unload after PLUGINS_IDLE_UNLOAD_S: switching a plugin on is never
//!   refused for memory. (Until 10.11.2026 every plugin that was on took its share: the ten built-ins declared 42 MB
//!   and held about 2, and a small plugin from the workshop did not go on — «нужно 4 МБ, свободно 0».)
//! - The check is at the load — a call, an event, a voice route: the context's declared budget must fit what is free.
//!   If it does not, idle contexts unload first, the longest idle first, one in a call never; if it still does not,
//!   the call fails and says what it needs and what is free. On the station only: on a computer the numbers are shown,
//!   not kept. An app's program is checked the same way at its start; it is a process of its own, counted by what it
//!   holds (out of MemAvailable), not by a share.
//! - Room on the disk is checked at the install: what a package unpacks into and its store, with a reserve kept for the
//!   gateway's own update — its program's growth and a floor, some 8 MB (`PluginHost::storage_reserve_mb`; the new
//!   version is tried from memory and takes the old one's place, docs/plans/station-update-space.md), or
//!   PLUGINS_STORAGE_RESERVE_MB when it is set.
//! - Under pressure (MemAvailable below the reserve, or the system's own pressure at «warn»): the contexts of plugins
//!   idle longer than a minute are unloaded and their work in the background waits. At «critical» on the station the
//!   app (else the plugin) switched on last among those loaded falls asleep: unloaded, still on, and it wakes by
//!   itself once the memory is back with room to spare (PLUGINS_PRESSURE_ACTION=warn only says so); one that holds
//!   nothing frees nothing and stays as it is. The built-in skills and the voice
//!   path are never touched: a plugin's voice route that does not fit fails as any failed call does, and a built-in
//!   skill takes the phrase when there is one.
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use crate::core::availability::Placement;
use crate::operations::system_memory::{self, Pressure};

/// The reserve for the system and Alice at most and at least, when PLUGINS_RESERVE_MB does not set it: a sixth of the
/// station's memory between these (`reserve_mb`).
pub const RESERVE_MAX_MB: u64 = 40;
pub const RESERVE_MIN_MB: u64 = 24;

/// The memory kept for the system and Alice out of the plugins' budget: PLUGINS_RESERVE_MB when it is set, else a sixth
/// of the machine's memory, between 24 and 40 MB. 40 MB was chosen on a Lite 2 (237 MB in all) and has held there — a
/// sixth of 237 is that, so the Lite 2 keeps it. A station with less memory runs a firmware made for less, its own
/// processes smaller: its reserve shrinks with it, so its apps are not left with nothing (Windows test 21.11: a Station
/// Mini 2's widgets «нужно 2 МБ, свободно 0»), but never below 24 MB — three fifths of the reserve that has held, for the
/// kernel's own watermarks and Alice's peaks. More memory needs no more: 40 has held. Unknown memory (0): 40.
pub fn reserve_mb(total_mb: u64, override_mb: Option<u64>) -> u64 {
    if let Some(mb) = override_mb {
        return mb;
    }
    if total_mb == 0 {
        return RESERVE_MAX_MB;
    }
    total_mb.div_ceil(6).clamp(RESERVE_MIN_MB, RESERVE_MAX_MB)
}

/// The least an idle context is charged over what its engine holds: its own room to breathe between two looks.
pub const IDLE_MARGIN_KB: u64 = 512;

/// What a loaded context takes of the plugins' budget (Windows test 21.11: widgets of 2–4 MB declared took all of it
/// while they held a fraction — «нужно 2 МБ, свободно 0»). Busy — in a call or loading, when a context grows — or not
/// measured yet: all it declared, its engine's hard limit. Idle: what its engine holds, with a margin of a quarter of
/// its declared budget (at least `IDLE_MARGIN_KB`), never more than declared. An idle context runs no code: it grows
/// only in a call, and charged its whole budget then.
pub fn charge_mb(declared_mb: u64, held_kb: u64, busy: bool) -> u64 {
    if declared_mb == 0 {
        return 0;
    }
    if busy || held_kb == 0 {
        return declared_mb;
    }
    let declared_kb = declared_mb * 1024;
    let margin_kb = (declared_kb / 4).max(IDLE_MARGIN_KB);
    (held_kb + margin_kb).min(declared_kb).div_ceil(1024)
}

/// An app asleep for want of memory wakes when this much is free over the reserve and its own budget: not into the
/// same want again.
pub const WAKE_MARGIN_MB: u64 = 15;

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
    /// The one switched on last falls asleep: unloaded, still on, and it wakes by itself once the memory is back
    /// (the owner's choice, 16.11.2026: «засыпает и само просыпается»). `disable`, the word before, means it too.
    Sleep,
    /// Only say so: the log and the panel.
    Warn,
}

impl PressureAction {
    pub fn parse(text: &str) -> Option<PressureAction> {
        match text.trim() {
            "sleep" | "disable" => Some(PressureAction::Sleep),
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
    /// Its context is loaded, or loading.
    pub loaded: bool,
    /// In a call now, or loading: its context is not unloaded to make room.
    pub busy: bool,
    /// What it declared: its engine's hard limit, and what its context needs free to load.
    pub budget_mb: u64,
    /// What it takes of the budget now (`charge_mb`): busy or not measured yet — what it declared; idle — what it
    /// holds with a margin; nothing while not loaded.
    pub taken_mb: u64,
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

/// `GET /api/v1/packages/resources`: the plugins' memory and room, for «Ресурсы колонки» and the plugins' list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesView {
    pub placement: Placement,
    /// On the station the budget is kept (a context that does not fit does not load); on a computer it is shown.
    pub enforced: bool,
    /// Whether the machine told its memory at all.
    pub measured: bool,
    pub total_mb: u64,
    pub available_mb: u64,
    pub reserve_mb: u64,
    pub gateway_mb: Option<u64>,
    /// What the loaded plugins' engines hold now.
    pub plugins_mb: u64,
    /// The plugins' budget: what their loaded contexts may take at most, together.
    pub budget_mb: u64,
    /// In use now: what the loaded contexts take (`charge_mb`).
    pub committed_mb: u64,
    /// What one more context may take now, nothing unloaded.
    pub free_mb: u64,
    /// What one more context could get: the free and what the idle contexts give back when they unload.
    pub room_mb: u64,
    /// The plugins whose contexts are loaded now.
    pub loaded: usize,
    pub pressure: Pressure,
    /// Under pressure the plugins' work in the background waits.
    pub deferring: bool,
    pub action: PressureAction,
    pub disk: DiskView,
    pub plugins: Vec<PluginShare>,
    /// «Загружено плагинов: 3, под них занято 12 МБ. Можно загрузить ещё 68 МБ.»
    pub note: String,
}

/// The count itself, from the machine's numbers and the plugins'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Count {
    pub budget_mb: u64,
    /// What the loaded contexts take (`charge_mb`).
    pub committed_mb: u64,
    /// What one more context may take now.
    pub free_mb: u64,
    /// What one more context could get once the idle contexts unload: the busy ones keep theirs.
    pub room_mb: u64,
    pub plugins_mb: u64,
    pub loaded: usize,
}

pub fn count(memory: Option<MemoryNow>, reserve_mb: u64, plugins: &[PluginShare]) -> Count {
    let plugins_mb: u64 = plugins.iter().filter_map(|plugin| plugin.actual_mb).sum();
    let committed_mb: u64 = plugins.iter().map(|plugin| plugin.taken_mb).sum();
    let busy_mb: u64 = plugins.iter().filter(|plugin| plugin.busy).map(|plugin| plugin.taken_mb).sum();
    let budget_mb = memory.map(|memory| (memory.available_mb + plugins_mb).saturating_sub(reserve_mb)).unwrap_or(0);
    Count {
        budget_mb,
        committed_mb,
        free_mb: budget_mb.saturating_sub(committed_mb),
        room_mb: budget_mb.saturating_sub(busy_mb),
        plugins_mb,
        loaded: plugins.iter().filter(|plugin| plugin.loaded).count(),
    }
}

/// What the panel says under the bar: what is in use now and how much more can be loaded.
pub fn note(count: &Count, measured: bool, pressure: Pressure) -> String {
    if !measured {
        return crate::tr!("Сколько ОЗУ свободно, DuoWave здесь сказать не может.");
    }
    let used = match count.loaded {
        0 => crate::tr!("Сейчас ни одно приложение не загружено."),
        loaded => crate::tr!(
            "Загружено приложений: {loaded}, под них занято {mb} МБ.",
            loaded = loaded,
            mb = count.committed_mb
        ),
    };
    let more = if count.free_mb > 0 {
        crate::tr!("Можно загрузить ещё {mb} МБ.", mb = count.free_mb)
    } else if count.budget_mb == 0 {
        crate::tr!("Приложениям ОЗУ не осталось: свободного меньше запаса для системы и Алисы.")
    } else if count.room_mb > 0 {
        crate::tr!("Свободного ОЗУ нет: следующее приложение сначала выгрузит простаивающие.")
    } else {
        crate::tr!("Больше не загрузить, пока работают загруженные.")
    };
    let room = format!("{used} {more}");
    match pressure {
        Pressure::Normal => room,
        Pressure::Warn => {
            crate::tr!("{room} ОЗУ мало: простаивающие приложения выгружаются, фоновая работа ждёт.", room = room)
        }
        Pressure::Critical => crate::tr!("{room} ОЗУ очень мало: приложения выгружаются.", room = room),
    }
}

/// Why a plugin's context does not load, or nothing when it fits: which side is short — the apps' whole budget (the
/// station's own memory below the reserve for the system and Alice) or what the loaded apps take, and which of them —
/// so a person sees what to switch off (Windows test 21.11: «нужно 2 МБ, свободно 0» read as «a widget ate it all»).
pub fn does_not_fit(
    need_mb: u64,
    free_mb: u64,
    count: &Count,
    reserve_mb: u64,
    taking: &[(String, u64)],
) -> Option<String> {
    if need_mb <= free_mb {
        return None;
    }
    if count.budget_mb < need_mb {
        return Some(crate::tr!(
            "колонке не хватает ОЗУ, чтобы загрузить приложение: нужно {need_mb} МБ, а приложениям всего {budget_mb} МБ — остальное занято системой и Алисой (запас {reserve_mb} МБ). Помогут меньше приложений на колонке или перезагрузка колонки.",
            need_mb = need_mb,
            budget_mb = count.budget_mb,
            reserve_mb = reserve_mb
        ));
    }
    let mut taking: Vec<&(String, u64)> = taking.iter().filter(|(_, mb)| *mb > 0).collect();
    taking.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let named: Vec<String> = taking.iter().take(4).map(|(title, mb)| format!("«{title}» {mb}")).collect();
    let more = taking.len().saturating_sub(4);
    let mut list = named.join(", ");
    if more > 0 {
        list.push_str(&crate::tr!(" и ещё {more}", more = more));
    }
    Some(crate::tr!(
        "колонке не хватает ОЗУ, чтобы загрузить приложение: нужно {need_mb} МБ, свободно {free_mb} из {budget_mb} МБ для приложений; сейчас заняли (МБ): {list}.",
        need_mb = need_mb,
        free_mb = free_mb,
        budget_mb = count.budget_mb,
        list = list
    ))
}

/// A loaded context that may be unloaded to make room: none of its calls is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Idle {
    pub id: String,
    /// What it takes of the budget.
    pub taken_mb: u64,
    /// How long since its last call.
    pub idle_ms: u64,
}

/// What to unload so that a context of `need_mb` fits what is free: the idle contexts, the longest idle first, only as
/// many as it takes; nothing when it fits already. When even all of them would not make room, nothing is unloaded and
/// the answer is what the context could get at most.
pub fn to_unload(need_mb: u64, free_mb: u64, idle: &[Idle]) -> Result<Vec<String>, u64> {
    if need_mb <= free_mb {
        return Ok(Vec::new());
    }
    let mut order: Vec<&Idle> = idle.iter().collect();
    order.sort_by_key(|idle| std::cmp::Reverse(idle.idle_ms));
    let could = free_mb + order.iter().map(|idle| idle.taken_mb).sum::<u64>();
    if could < need_mb {
        return Err(could);
    }
    let mut free = free_mb;
    let mut chosen = Vec::new();
    for idle in order {
        if free >= need_mb {
            break;
        }
        free += idle.taken_mb;
        chosen.push(idle.id.clone());
    }
    Ok(chosen)
}

/// Why a package does not fit on the disk, or nothing when it does.
pub fn no_room(need_mb: u64, free_mb: Option<u64>, reserve_mb: u64) -> Option<String> {
    let free = free_mb?;
    (need_mb + reserve_mb > free).then(|| {
        crate::tr!(
            "не хватит памяти: нужно {need_mb} МБ и ещё {reserve_mb} МБ про запас на обновление DuoWave, свободно {free}",
            need_mb = need_mb,
            reserve_mb = reserve_mb,
            free = free
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plugin that is on in the count: its context loaded (holding `actual_mb`) or not, in a call or idle.
    fn share(id: &str, budget_mb: u64, loaded: bool, busy: bool, actual_mb: Option<u64>) -> PluginShare {
        PluginShare {
            id: id.into(),
            title: id.into(),
            enabled: true,
            loaded,
            busy,
            budget_mb,
            taken_mb: if loaded { budget_mb } else { 0 },
            actual_mb,
        }
    }

    #[test]
    fn counts_only_the_loaded_contexts_by_what_they_declare() {
        // The station of 10.11: 120 MB available with the gateway running, the reserve 40; ten plugins on, two loaded.
        let memory = MemoryNow { total_mb: 238, available_mb: 120, pressure: Pressure::Normal, gateway_mb: Some(33) };
        let mut plugins = vec![share("radio", 4, true, false, Some(1)), share("calendar", 8, true, true, Some(1))];
        plugins.extend((0..8).map(|n| share(&format!("idle-{n}"), 4, false, false, None)));
        let count = count(Some(memory), 40, &plugins);
        assert_eq!(count.plugins_mb, 2);
        assert_eq!(count.budget_mb, 82, "what the loaded contexts hold comes back to the budget");
        assert_eq!(count.committed_mb, 12, "loaded: 4 + 8 declared; on but not loaded takes nothing");
        assert_eq!((count.free_mb, count.room_mb, count.loaded), (70, 74, 2), "the idle radio would give its 4 back");
        assert_eq!(
            note(&count, true, Pressure::Normal),
            "Загружено приложений: 2, под них занято 12 МБ. Можно загрузить ещё 70 МБ."
        );

        // Nothing loaded: the whole budget is free, however many plugins are on.
        let unloaded: Vec<PluginShare> = (0..10).map(|n| share(&format!("p{n}"), 4, false, false, None)).collect();
        let idle = super::count(Some(memory), 40, &unloaded);
        assert_eq!((idle.committed_mb, idle.free_mb), (0, 80));
        assert_eq!(
            note(&idle, true, Pressure::Normal),
            "Сейчас ни одно приложение не загружено. Можно загрузить ещё 80 МБ."
        );

        // Full of idle contexts: nothing free, room once they unload.
        let tight = super::count(Some(MemoryNow { available_mb: 50, ..memory }), 40, &plugins);
        assert_eq!((tight.budget_mb, tight.free_mb, tight.room_mb), (12, 0, 4));
        assert!(note(&tight, true, Pressure::Normal).ends_with("следующее приложение сначала выгрузит простаивающие."));

        let starving = super::count(Some(MemoryNow { available_mb: 30, ..memory }), 40, &plugins);
        assert_eq!((starving.free_mb, starving.room_mb), (0, 0), "below the reserve nothing is free");
        let said = note(&starving, true, Pressure::Warn);
        assert!(said.contains("свободного меньше запаса") && said.contains("ОЗУ мало"), "{said}");
        assert_eq!(super::count(None, 40, &plugins).budget_mb, 0);
    }

    #[test]
    fn makes_room_from_the_longest_idle_and_only_as_much_as_it_takes() {
        let idle = |id: &str, taken_mb: u64, idle_ms: u64| Idle { id: id.into(), taken_mb, idle_ms };
        let loaded = [idle("weather", 4, 5_000), idle("radio", 4, 90_000), idle("news", 4, 30_000)];
        assert_eq!(to_unload(4, 4, &loaded), Ok(Vec::new()), "it fits: nothing unloads");
        assert_eq!(to_unload(4, 1, &loaded), Ok(vec!["radio".to_string()]), "the longest idle goes first");
        assert_eq!(to_unload(8, 1, &loaded), Ok(vec!["radio".to_string(), "news".to_string()]));
        assert_eq!(to_unload(20, 1, &loaded), Err(13), "all of them would not do: nothing unloads");
        assert_eq!(to_unload(4, 0, &[]), Err(0));
    }

    #[test]
    fn says_how_much_is_needed_and_how_much_is_free() {
        let count = Count { budget_mb: 20, committed_mb: 13, free_mb: 7, room_mb: 20, plugins_mb: 3, loaded: 2 };
        let taking = [("Погода".to_string(), 2), ("Радио".to_string(), 11), ("Пусто".to_string(), 0)];
        assert_eq!(does_not_fit(4, 7, &count, 40, &taking), None);
        let why = does_not_fit(12, 7, &count, 40, &taking).unwrap();
        assert!(why.contains("нужно 12 МБ, свободно 7 из 20 МБ для приложений"), "{why}");
        assert!(why.contains("«Радио» 11, «Погода» 2."), "the biggest first, none that takes nothing: {why}");
        assert!(!why.contains("Выключите"), "switching others off is no longer the way: {why}");
        // The apps' whole budget is short: the system and Alice hold the rest — no list of apps then.
        let none = Count { budget_mb: 1, committed_mb: 0, free_mb: 1, room_mb: 1, plugins_mb: 0, loaded: 0 };
        let why = does_not_fit(2, 1, &none, 40, &[]).unwrap();
        assert!(why.contains("приложениям всего 1 МБ") && why.contains("запас 40 МБ"), "{why}");
        assert_eq!(no_room(2, Some(60), 15), None);
        assert!(no_room(50, Some(60), 15).unwrap().contains("свободно 60"));
        assert_eq!(no_room(50, None, 15), None, "a disk that does not tell is not refused");
        let gauge = MemoryGauge::default();
        assert_eq!(gauge.get(), None);
        gauge.set(Some(80));
        assert_eq!(gauge.get(), Some(80));
        assert_eq!(PressureAction::parse("warn"), Some(PressureAction::Warn));
        assert_eq!(PressureAction::parse("sleep"), Some(PressureAction::Sleep));
        assert_eq!(PressureAction::parse("disable"), Some(PressureAction::Sleep), "the word before");
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

    /// Windows test 21.11: a Station Mini 2's widgets — «нужно 2 МБ, свободно 0» — while widgets of 2–4 MB hold a
    /// fraction of that. An idle context is charged what it holds with a margin; a busy or unmeasured one all it declared.
    #[test]
    fn an_idle_context_is_charged_what_it_holds_and_a_busy_one_all_it_declared() {
        assert_eq!(charge_mb(0, 300, false), 0, "not loaded: nothing");
        assert_eq!(charge_mb(4, 0, false), 4, "not measured yet: all it declared");
        assert_eq!(charge_mb(4, 300, true), 4, "in a call it may grow to its limit");
        assert_eq!(charge_mb(4, 300, false), 2, "300 KB and a quarter of 4 MB: 1.3 MB, counted 2");
        assert_eq!(charge_mb(2, 300, false), 1, "300 KB and the least margin of 512 KB");
        assert_eq!(charge_mb(4, 4000, false), 4, "never more than declared");
        // Ten idle widgets of 4 MB holding 300 KB each take 20 MB, not 40.
        assert_eq!((0..10).map(|_| charge_mb(4, 300, false)).sum::<u64>(), 20);
    }

    /// The reserve for the system and Alice follows the station's memory: the Lite 2 keeps its 40 MB, a smaller one
    /// keeps less but never under 24; PLUGINS_RESERVE_MB still sets it.
    #[test]
    fn the_reserve_follows_the_stations_memory() {
        assert_eq!(reserve_mb(237, None), 40, "the Lite 2: as before");
        assert_eq!(reserve_mb(238, None), 40);
        assert_eq!(reserve_mb(512, None), 40, "more memory needs no more");
        assert_eq!(reserve_mb(180, None), 30);
        assert_eq!(reserve_mb(120, None), 24, "never below 24");
        assert_eq!(reserve_mb(0, None), 40, "memory unknown");
        assert_eq!(reserve_mb(120, Some(50)), 50, "PLUGINS_RESERVE_MB wins");
    }
}
