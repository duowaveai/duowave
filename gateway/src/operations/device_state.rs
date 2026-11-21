//! The state of the device a gateway runs on, for the apps (`system/device`, Host API 2): its temperature, memory,
//! storage, processor load and uptime, its name and its role in the home — and the other devices of the home as the
//! links already bring them: the computer's watch reads the station's /health every 30 s (placement/watch.rs), the
//! centre looks at its Turbo's `/home/v1/status` every 15 s (helpers/remote.rs). Nothing new travels for it.
//!
//! Cheap on the station (about 123 MB of memory, 31 MB of /data): one reading serves every letter for a few seconds,
//! however many widgets ask; one sampler for the whole gateway, started by the first letter, keeps a few hours of
//! history for graphs and stops after a day without letters — nothing per app, nothing while nobody asks.
//!
//! - Linux (the station, a Linux computer): /sys/class/thermal (or hwmon), /proc/meminfo, /proc/stat, /proc/loadavg,
//!   /proc/uptime.
//! - macOS: no temperature without the administrator's rights — said so, not made up; memory as the plugins' budget
//!   counts it, the processor by `ps`, the load by getloadavg, the uptime by kern.boottime.
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use super::system_memory;
use crate::utils::time::now_iso;

/// The participant of the bus.
pub const ADDRESS: &str = "system/device";

/// A reading is given again this long: many widgets, one reading.
const FRESH: Duration = Duration::from_secs(5);
/// The history: one sample a minute, six hours of them, for this many devices at most.
pub const EVERY: Duration = Duration::from_secs(60);
const KEEP: usize = 360;
const DEVICES_MAX: usize = 8;
/// The sampler stops after this long without a letter; the next letter starts it again.
const IDLE: Duration = Duration::from_secs(24 * 3600);
/// The processor's counters are read twice this far apart when there is no earlier reading to compare with.
const CPU_GAP: Duration = Duration::from_millis(300);
/// An earlier reading older than this gives no percent: it would be the average of too long.
const CPU_STALE: Duration = Duration::from_secs(600);

/// Who this gateway is in the home, as an app reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Me {
    /// The gateway's own id (src/core/identity/home.rs).
    pub id: String,
    /// The name people see.
    pub name: String,
    /// `station` or `computer`.
    pub kind: &'static str,
    /// In the home: `centre`, `ear` (a station that hears for the centre), `turbo` (the computer that helps it).
    pub roles: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Zone {
    pub name: String,
    pub celsius: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Temperature {
    /// The hottest sensor.
    pub celsius: f64,
    pub zones: Vec<Zone>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub total_mb: u64,
    /// What can be taken without swapping (MemAvailable).
    pub free_mb: u64,
    pub used_percent: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Storage {
    /// Which disk: `/data` on the station; a computer's own path is not told.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub of: Option<String>,
    pub total_mb: u64,
    pub free_mb: u64,
    pub used_percent: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cpu {
    /// The share of time the processor was busy since the reading before, 0–100.
    pub percent: Option<f64>,
    /// The load average over 1, 5 and 15 minutes.
    pub load: Option<[f64; 3]>,
    pub cores: usize,
    /// The cores' clock: a station that runs hot slows it down.
    pub mhz: Option<Mhz>,
}

/// The processor's clock, MHz: the average of the cores now and the highest any of them can run at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Mhz {
    pub now: u64,
    pub max: Option<u64>,
}

/// The wireless link the device is on.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Wifi {
    /// `wlan0` and the like.
    pub interface: String,
    /// The signal, dBm: −30 is right next to the router, −80 is about to drop.
    pub dbm: Option<f64>,
    /// The link's quality, 0–100.
    pub percent: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gateway {
    /// The gateway's own resident memory.
    pub memory_mb: Option<u64>,
    pub uptime_s: u64,
}

/// What the machine says of itself now.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub temperature: Option<Temperature>,
    pub memory: Option<Memory>,
    pub storage: Option<Storage>,
    pub cpu: Option<Cpu>,
    pub wifi: Option<Wifi>,
    pub uptime_s: Option<u64>,
    pub gateway: Gateway,
    /// What could not be read, and why, by its field: an honest «нет данных».
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub missing: BTreeMap<&'static str, String>,
}

/// Where a reading comes from: this machine, or a test's.
pub trait Probe: Send + Sync {
    fn read(&self) -> Reading;
    /// Reads the processor's counters when there is no fresh earlier reading to compare with; true — wait a moment
    /// before `read`, so the percent is there.
    fn prime(&self) -> bool {
        false
    }
}

/// The processor's time since boot: busy and all, in ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticks {
    pub busy: u64,
    pub total: u64,
}

/// The first line of /proc/stat: `cpu  user nice system idle iowait irq softirq steal …`; waiting for the disk is idle.
pub fn parse_proc_stat(text: &str) -> Option<Ticks> {
    let line = text.lines().find(|line| line.starts_with("cpu "))?;
    let numbers: Vec<u64> = line.split_whitespace().skip(1).take(8).filter_map(|n| n.parse().ok()).collect();
    if numbers.len() < 4 {
        return None;
    }
    let total: u64 = numbers.iter().sum();
    let idle = numbers[3] + numbers.get(4).copied().unwrap_or(0);
    Some(Ticks { busy: total.saturating_sub(idle), total })
}

/// The busy share between two readings, 0–100; none when no time passed.
pub fn busy_percent(before: Ticks, now: Ticks) -> Option<f64> {
    let total = now.total.checked_sub(before.total)?;
    if total == 0 {
        return None;
    }
    let busy = now.busy.saturating_sub(before.busy) as f64;
    Some(round1((busy / total as f64 * 100.0).clamp(0.0, 100.0)))
}

/// A sensor's `temp`: millidegrees as the kernel writes them (some drivers write degrees). None for what no device
/// reads: below −40° or above 150°, a sensor not connected.
pub fn sensor_celsius(text: &str) -> Option<f64> {
    let raw: f64 = text.trim().parse().ok()?;
    let celsius = if raw.abs() >= 1000.0 { raw / 1000.0 } else { raw };
    (-40.0..=150.0).contains(&celsius).then(|| round1(celsius))
}

/// The thermal zones under `root` (/sys/class/thermal): `thermal_zone*/type` and `temp`.
pub fn thermal_zones(root: &Path) -> Vec<Zone> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("thermal_zone"))
        })
        .collect();
    dirs.sort();
    dirs.iter()
        .filter_map(|dir| {
            let celsius = sensor_celsius(&std::fs::read_to_string(dir.join("temp")).ok()?)?;
            let name = std::fs::read_to_string(dir.join("type"))
                .ok()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| dir.file_name().unwrap_or_default().to_string_lossy().into_owned());
            Some(Zone { name, celsius })
        })
        .collect()
}

/// The hardware monitors under `root` (/sys/class/hwmon), where a Linux computer has no thermal zones:
/// `hwmon*/name` and its `temp*_input`.
pub fn hwmon_sensors(root: &Path) -> Vec<Zone> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = entries.filter_map(Result::ok).map(|entry| entry.path()).collect();
    dirs.sort();
    let mut zones = Vec::new();
    for dir in dirs {
        let name = std::fs::read_to_string(dir.join("name")).map(|name| name.trim().to_string()).unwrap_or_default();
        let Ok(files) = std::fs::read_dir(&dir) else { continue };
        let mut inputs: Vec<PathBuf> = files
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("temp") && name.ends_with("_input"))
            })
            .collect();
        inputs.sort();
        for input in inputs {
            if let Some(celsius) = std::fs::read_to_string(&input).ok().as_deref().and_then(sensor_celsius) {
                zones.push(Zone { name: if name.is_empty() { "hwmon".into() } else { name.clone() }, celsius });
            }
        }
    }
    zones
}

/// The sensors as one temperature: the hottest of them.
pub fn temperature_of(zones: Vec<Zone>) -> Option<Temperature> {
    let celsius = zones
        .iter()
        .map(|zone| zone.celsius)
        .fold(None, |hottest: Option<f64>, one| Some(hottest.map_or(one, |hottest| hottest.max(one))))?;
    Some(Temperature { celsius, zones })
}

/// The cores' clock under `root` (/sys/devices/system/cpu): each `cpu<N>/cpufreq` — `scaling_cur_freq` (or
/// `cpuinfo_cur_freq`) and `cpuinfo_max_freq`, in kHz. Cores without cpufreq, or a kernel without it, give none.
pub fn cpu_mhz(root: &Path) -> Option<Mhz> {
    let Ok(entries) = std::fs::read_dir(root) else { return None };
    let khz = |path: PathBuf| -> Option<u64> {
        std::fs::read_to_string(path).ok()?.trim().parse::<u64>().ok().filter(|khz| *khz > 0)
    };
    let (mut now, mut max): (Vec<u64>, Vec<u64>) = (Vec::new(), Vec::new());
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(number) = name.to_str().and_then(|name| name.strip_prefix("cpu")) else { continue };
        if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let freq = entry.path().join("cpufreq");
        if let Some(cur) = khz(freq.join("scaling_cur_freq")).or_else(|| khz(freq.join("cpuinfo_cur_freq"))) {
            now.push(cur);
        }
        if let Some(top) = khz(freq.join("cpuinfo_max_freq")) {
            max.push(top);
        }
    }
    if now.is_empty() {
        return None;
    }
    let average = now.iter().sum::<u64>() / now.len() as u64;
    Some(Mhz { now: average / 1000, max: max.iter().max().map(|khz| khz / 1000) })
}

/// A number of /proc/net/wireless: `70.`, `-33.`, `190` — the dot is the kernel's mark of an updated value.
fn wireless_number(text: &str) -> Option<f64> {
    text.trim_end_matches('.').parse::<f64>().ok().filter(|value| value.is_finite())
}

/// /proc/net/wireless: two header lines, then `wlan0: 0000   70.  -33.  -256  …` — the interface, its status, the
/// link's quality, the signal and the noise. The first interface with a link wins (`p2p0` and the like have none).
/// A signal some drivers give unsigned (`190` is −66 dBm) is read as such; −256 and 0 are «no value».
pub fn parse_wireless(text: &str) -> Option<Wifi> {
    text.lines().skip(2).find_map(|line| {
        let (interface, rest) = line.split_once(':')?;
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let link = fields.get(1).and_then(|text| wireless_number(text)).unwrap_or(0.0);
        let level = fields.get(2).and_then(|text| wireless_number(text));
        let dbm = level
            .map(|level| if level > 0.0 && level <= 255.0 { level - 256.0 } else { level })
            .filter(|dbm| (-150.0..0.0).contains(dbm) && *dbm > -256.0);
        if link <= 0.0 && dbm.is_none() {
            return None;
        }
        // The quality by the signal where there is one (drivers scale their own «link» differently), else by the
        // link out of the usual 70.
        let percent = match dbm {
            Some(dbm) => Some((2.0 * (dbm + 100.0)).clamp(0.0, 100.0)),
            None => Some((link / 70.0 * 100.0).clamp(0.0, 100.0)),
        }
        .map(round1);
        Some(Wifi { interface: interface.trim().to_string(), dbm: dbm.map(round1), percent })
    })
}

/// The used share of `total`, 0–100, one decimal.
fn used_percent(total: u64, free: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    round1(total.saturating_sub(free) as f64 / total as f64 * 100.0)
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// `/proc/loadavg`: `0.52 0.41 0.30 1/123 4567`.
pub fn parse_loadavg(text: &str) -> Option<[f64; 3]> {
    let numbers: Vec<f64> = text.split_whitespace().take(3).filter_map(|n| n.parse().ok()).collect();
    (numbers.len() == 3).then(|| [round2(numbers[0]), round2(numbers[1]), round2(numbers[2])])
}

/// `kern.boottime` as sysctl prints it: `{ sec = 1795163759, usec = 255914 } Fri Nov 20 …`.
pub fn parse_boottime(text: &str) -> Option<u64> {
    let after = text.split("sec =").nth(1)?;
    after.trim_start().split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// This machine.
pub struct Machine {
    /// The disk whose room is told: the gateway's data.
    storage: PathBuf,
    /// What the disk is called for people: `/data` on the station; none on a computer (its path is personal).
    storage_label: Option<String>,
    started: Instant,
    /// The processor's counters last read, and when (Linux).
    ticks: Mutex<Option<(Instant, Ticks)>>,
}

impl Machine {
    pub fn new(storage: PathBuf, storage_label: Option<String>, started: Instant) -> Machine {
        Machine { storage, storage_label, started, ticks: Mutex::new(None) }
    }

    /// The processor's share since the reading before (Linux), or by `ps` (macOS).
    fn cpu_percent(&self) -> Option<f64> {
        if cfg!(target_os = "linux") {
            let now = parse_proc_stat(&std::fs::read_to_string("/proc/stat").ok()?)?;
            let mut last = self.ticks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let percent =
                last.filter(|(at, _)| at.elapsed() <= CPU_STALE).and_then(|(_, before)| busy_percent(before, now));
            *last = Some((Instant::now(), now));
            return percent;
        }
        if cfg!(target_os = "macos") {
            let output = std::process::Command::new("ps").args(["-A", "-o", "%cpu="]).output().ok()?;
            if !output.status.success() {
                return None;
            }
            let sum: f64 = String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.trim().replace(',', ".").parse::<f64>().ok())
                .sum();
            let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
            return Some(round1((sum / cores).clamp(0.0, 100.0)));
        }
        None
    }

    fn load(&self) -> Option<[f64; 3]> {
        if cfg!(target_os = "linux") {
            return parse_loadavg(&std::fs::read_to_string("/proc/loadavg").ok()?);
        }
        bsd_load()
    }

    fn uptime_s(&self) -> Option<u64> {
        if cfg!(target_os = "linux") {
            let text = std::fs::read_to_string("/proc/uptime").ok()?;
            return text.split_whitespace().next()?.parse::<f64>().ok().map(|seconds| seconds as u64);
        }
        if cfg!(target_os = "macos") {
            static BOOT: OnceLock<Option<u64>> = OnceLock::new();
            let boot = (*BOOT.get_or_init(|| {
                let output = std::process::Command::new("sysctl").args(["-n", "kern.boottime"]).output().ok()?;
                parse_boottime(&String::from_utf8_lossy(&output.stdout))
            }))?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
            return now.checked_sub(boot);
        }
        None
    }
}

#[cfg(target_os = "macos")]
fn bsd_load() -> Option<[f64; 3]> {
    let mut load = [0f64; 3];
    // SAFETY: an array of three the call fills.
    let got = unsafe { libc::getloadavg(load.as_mut_ptr(), 3) };
    (got == 3).then(|| load.map(round2))
}

#[cfg(not(target_os = "macos"))]
fn bsd_load() -> Option<[f64; 3]> {
    None
}

impl Probe for Machine {
    fn prime(&self) -> bool {
        if !cfg!(target_os = "linux") {
            return false;
        }
        let fresh = self
            .ticks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some_and(|(at, _)| at.elapsed() <= CPU_STALE);
        if fresh {
            return false;
        }
        self.cpu_percent();
        true
    }

    fn read(&self) -> Reading {
        let mut missing = BTreeMap::new();
        let temperature = if cfg!(target_os = "linux") {
            let mut zones = thermal_zones(Path::new("/sys/class/thermal"));
            if zones.is_empty() {
                zones = hwmon_sensors(Path::new("/sys/class/hwmon"));
            }
            temperature_of(zones)
        } else {
            None
        };
        if temperature.is_none() {
            missing.insert(
                "temperature",
                if cfg!(target_os = "macos") {
                    crate::tr!("macOS не открывает температуру датчиков другим программам")
                } else if cfg!(target_os = "linux") {
                    crate::tr!("у этого устройства нет датчиков температуры, открытых для чтения")
                } else {
                    crate::tr!("температуру этой системы шлюз не читает")
                },
            );
        }
        let memory = system_memory::read_counters().map(|counters| Memory {
            total_mb: counters.total_mb,
            free_mb: counters.pool_mb,
            used_percent: used_percent(counters.total_mb, counters.pool_mb),
        });
        if memory.is_none() {
            missing.insert("memory", crate::tr!("память этой системы шлюз не читает"));
        }
        let storage = system_memory::disk_mb(&self.storage).map(|(total_mb, free_mb)| Storage {
            of: self.storage_label.clone(),
            total_mb,
            free_mb,
            used_percent: used_percent(total_mb, free_mb),
        });
        if storage.is_none() {
            missing.insert("storage", crate::tr!("место на диске этой системы шлюз не читает"));
        }
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        let mhz = if cfg!(target_os = "linux") { cpu_mhz(Path::new("/sys/devices/system/cpu")) } else { None };
        let cpu = Cpu { percent: self.cpu_percent(), load: self.load(), cores, mhz };
        let wifi = if cfg!(target_os = "linux") {
            std::fs::read_to_string("/proc/net/wireless").ok().as_deref().and_then(parse_wireless)
        } else {
            None
        };
        if wifi.is_none() {
            missing.insert(
                "wifi",
                if cfg!(target_os = "linux") {
                    crate::tr!("нет подключения по Wi-Fi или ядро не говорит его сигнал")
                } else {
                    crate::tr!("сигнал Wi-Fi этой системы шлюз не читает")
                },
            );
        }
        let uptime_s = self.uptime_s();
        Reading {
            temperature,
            memory,
            storage,
            cpu: Some(cpu),
            wifi,
            uptime_s,
            gateway: Gateway {
                memory_mb: system_memory::process_rss_mb(None),
                uptime_s: self.started.elapsed().as_secs(),
            },
            missing,
        }
    }
}

/// Who this gateway is now (its name may change): set by the app.
pub type WhoAmI = Arc<dyn Fn() -> Me + Send + Sync>;
/// The other devices of the home as the links last brought them: each one's snapshot with its `id`, `name`, `kind`,
/// `roles`, `at`, and `reachable` — whether it answered the last look.
pub type Others = Arc<dyn Fn() -> Vec<Value> + Send + Sync>;

/// One point of a device's history: what a graph draws.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub at: String,
    pub temperature: Option<f64>,
    pub cpu: Option<f64>,
    pub memory_used: Option<f64>,
    pub memory_free_mb: Option<u64>,
    pub storage_free_mb: Option<u64>,
    pub cpu_mhz: Option<u64>,
    pub wifi_dbm: Option<f64>,
}

impl Sample {
    /// A device's snapshot as a point.
    pub fn of(snapshot: &Value) -> Sample {
        let number =
            |path: &[&str]| path.iter().try_fold(snapshot, |value, key| value.get(key)).and_then(Value::as_f64);
        Sample {
            at: snapshot.get("at").and_then(Value::as_str).map(str::to_string).unwrap_or_else(now_iso),
            temperature: number(&["temperature", "celsius"]),
            cpu: number(&["cpu", "percent"]),
            memory_used: number(&["memory", "usedPercent"]),
            memory_free_mb: number(&["memory", "freeMb"]).map(|mb| mb as u64),
            storage_free_mb: number(&["storage", "freeMb"]).map(|mb| mb as u64),
            cpu_mhz: number(&["cpu", "mhz", "now"]).map(|mhz| mhz as u64),
            wifi_dbm: number(&["wifi", "dbm"]),
        }
    }
}

/// The device state for the apps: this gateway's reading, the others', and the history.
pub struct DeviceState {
    probe: Box<dyn Probe>,
    who: RwLock<Option<WhoAmI>>,
    others: RwLock<Option<Others>>,
    /// The last snapshot of this device, and when it was read.
    cache: Mutex<Option<(Instant, Value)>>,
    /// Each device's samples by its id, oldest first.
    history: Mutex<HashMap<String, VecDeque<Sample>>>,
    /// The last letter; the sampler stops a day after it.
    asked: Mutex<Instant>,
    sampling: AtomicBool,
    me: Weak<DeviceState>,
}

static SHARED: OnceLock<Arc<DeviceState>> = OnceLock::new();

/// The gateway's own: set at start (app.rs), or this machine by itself where nothing set it (the workshop's terminal).
pub fn shared() -> Arc<DeviceState> {
    SHARED
        .get_or_init(|| {
            let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            DeviceState::new(Box::new(Machine::new(here, None, Instant::now())))
        })
        .clone()
}

/// The gateway's own, once at start; false when one already stands.
pub fn set_shared(state: Arc<DeviceState>) -> bool {
    SHARED.set(state).is_ok()
}

impl DeviceState {
    pub fn new(probe: Box<dyn Probe>) -> Arc<DeviceState> {
        Arc::new_cyclic(|me| DeviceState {
            probe,
            who: RwLock::new(None),
            others: RwLock::new(None),
            cache: Mutex::new(None),
            history: Mutex::new(HashMap::new()),
            asked: Mutex::new(Instant::now()),
            sampling: AtomicBool::new(false),
            me: me.clone(),
        })
    }

    pub fn set_who(&self, who: WhoAmI) {
        *self.who.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(who);
    }

    pub fn set_others(&self, others: Others) {
        *self.others.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(others);
    }

    fn who(&self) -> Me {
        match self.who.read().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref() {
            Some(who) => who(),
            None => Me {
                id: String::new(),
                name: crate::tr!("Это устройство"),
                kind: if crate::core::build::Profile::this().station { "station" } else { "computer" },
                roles: Vec::new(),
            },
        }
    }

    /// This device now, from the reading of the last few seconds or a new one — without waiting: the percent of the
    /// processor is since the reading before (/health asks this way).
    pub fn snapshot(&self) -> Value {
        let cached = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .filter(|(at, _)| at.elapsed() < FRESH)
            .map(|(_, value)| value.clone());
        if let Some(value) = cached {
            return value;
        }
        let value = self.compose(self.probe.read());
        *self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((Instant::now(), value.clone()));
        value
    }

    fn compose(&self, reading: Reading) -> Value {
        let me = self.who();
        let mut value = json!({
            "id": me.id,
            "name": me.name,
            "kind": me.kind,
            "roles": me.roles,
            "itself": true,
            "at": now_iso(),
        });
        if let (Some(fields), Ok(Value::Object(reading))) = (value.as_object_mut(), serde_json::to_value(reading)) {
            fields.extend(reading);
        }
        value
    }

    /// The same for a letter: the first one waits a moment for the processor's percent, the reading itself off the
    /// gateway's own threads.
    pub async fn snapshot_now(self: &Arc<Self>) -> Value {
        // Outside the gateway's runtime (a sandbox's own executor) the reading is made right here.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            if self.probe.prime() {
                std::thread::sleep(CPU_GAP);
                self.forget();
            }
            return self.snapshot();
        };
        let state = self.clone();
        if runtime.spawn_blocking(move || state.probe.prime()).await.unwrap_or(false) {
            tokio::time::sleep(CPU_GAP).await;
            self.forget();
        }
        let state = self.clone();
        runtime.spawn_blocking(move || state.snapshot()).await.unwrap_or(Value::Null)
    }

    /// The next snapshot is read anew.
    fn forget(&self) {
        *self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    fn others(&self) -> Vec<Value> {
        let others = self.others.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        others
            .map(|others| others())
            .unwrap_or_default()
            .into_iter()
            .filter(|other| other.is_object())
            .map(|mut other| {
                other["itself"] = Value::Bool(false);
                other
            })
            .collect()
    }

    /// Every device: this one first, then the others the links bring.
    fn devices(&self, mine: Value) -> Vec<Value> {
        let id = |device: &Value| device.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let mut all = vec![mine];
        for other in self.others() {
            // The same gateway reached by two roads answers once.
            let other_id = id(&other);
            if other_id.is_empty() || all.iter().any(|known| id(known) == other_id) {
                continue;
            }
            all.push(other);
        }
        all.truncate(DEVICES_MAX);
        all
    }

    /// A letter to `system/device`: `state {device?}`, `devices`, `history {device?, minutes?}`.
    pub async fn answer(self: &Arc<Self>, handle: &str, input: &Value) -> Result<Value, String> {
        self.touch();
        let wanted = input.get("device").and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty());
        match handle {
            "state" => {
                let all = self.devices(self.snapshot_now().await);
                pick(&all, wanted).cloned().ok_or_else(|| not_found(wanted, &all))
            }
            "devices" => Ok(Value::Array(self.devices(self.snapshot_now().await))),
            "history" => {
                let all = self.devices(self.snapshot_now().await);
                let device = pick(&all, wanted).ok_or_else(|| not_found(wanted, &all))?;
                let id = device.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                let minutes = input.get("minutes").and_then(Value::as_u64).unwrap_or(60).clamp(1, KEEP as u64 * 60);
                let mut history = self.history.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let samples = history.entry(id).or_default();
                // A graph asked for the first time has a point at once.
                if samples.is_empty() {
                    samples.push_back(Sample::of(device));
                }
                let take = (minutes * 60 / EVERY.as_secs()).max(1) as usize;
                let from = samples.len().saturating_sub(take);
                let samples: Vec<&Sample> = samples.iter().skip(from).collect();
                Ok(json!({
                    "device": { "id": device["id"], "name": device["name"], "kind": device["kind"] },
                    "everyS": EVERY.as_secs(),
                    "samples": samples,
                }))
            }
            other => Err(crate::tr!("у system/device нет умения «{handle}»", handle = other)),
        }
    }

    /// A letter came: the sampler runs (started when it did not).
    fn touch(&self) {
        *self.asked.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Instant::now();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        if self.sampling.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.me.clone();
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(EVERY).await;
                let Some(state) = me.upgrade() else { return };
                if state.asked.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).elapsed() > IDLE {
                    state.sampling.store(false, Ordering::SeqCst);
                    return;
                }
                let reader = state.clone();
                let mine = tokio::task::spawn_blocking(move || reader.snapshot()).await.unwrap_or(Value::Null);
                state.record(&state.devices(mine));
            }
        });
    }

    /// One sample of each device; another device's reading not renewed since the last sample (it did not answer the
    /// look, or the look is slower than a minute) is not counted twice.
    pub fn record(&self, devices: &[Value]) {
        let mut history = self.history.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for device in devices {
            let Some(id) = device.get("id").and_then(Value::as_str) else { continue };
            if !history.contains_key(id) && history.len() >= DEVICES_MAX {
                continue;
            }
            let samples = history.entry(id.to_string()).or_default();
            let sample = Sample::of(device);
            if samples.back().is_some_and(|last| last.at == sample.at) {
                continue;
            }
            samples.push_back(sample);
            while samples.len() > KEEP {
                samples.pop_front();
            }
        }
    }
}

/// The device a letter names: none or `self` — this one; `station` or `computer` — the first of that kind (this one
/// first); else its id or name.
pub fn pick<'a>(all: &'a [Value], wanted: Option<&str>) -> Option<&'a Value> {
    let field = |device: &Value, key: &str| device.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    match wanted.map(str::to_lowercase).as_deref() {
        None | Some("self") | Some("this") => all.first(),
        Some(kind @ ("station" | "computer")) => all.iter().find(|device| field(device, "kind") == kind),
        Some(name) => all
            .iter()
            .find(|device| field(device, "id").to_lowercase() == name || field(device, "name").to_lowercase() == name),
    }
}

fn not_found(wanted: Option<&str>, all: &[Value]) -> String {
    let known: Vec<String> = all
        .iter()
        .map(|device| {
            format!(
                "{} ({})",
                device.get("name").and_then(Value::as_str).unwrap_or_default(),
                device.get("kind").and_then(Value::as_str).unwrap_or_default()
            )
        })
        .collect();
    crate::tr!(
        "устройства «{device}» не видно; видны: {known}",
        device = wanted.unwrap_or_default(),
        known = known.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Reading);

    impl Probe for Fixed {
        fn read(&self) -> Reading {
            self.0.clone()
        }
    }

    fn fixed() -> Arc<DeviceState> {
        let reading = Reading {
            temperature: temperature_of(vec![Zone { name: "soc".into(), celsius: 51.5 }]),
            memory: Some(Memory { total_mb: 123, free_mb: 40, used_percent: used_percent(123, 40) }),
            storage: Some(Storage { of: Some("/data".into()), total_mb: 31, free_mb: 12, used_percent: 61.3 }),
            cpu: Some(Cpu {
                percent: Some(12.5),
                load: Some([0.5, 0.4, 0.3]),
                cores: 4,
                mhz: Some(Mhz { now: 1200, max: Some(1200) }),
            }),
            wifi: Some(Wifi { interface: "wlan0".into(), dbm: Some(-33.0), percent: Some(100.0) }),
            uptime_s: Some(3600),
            gateway: Gateway { memory_mb: Some(30), uptime_s: 60 },
            missing: BTreeMap::new(),
        };
        let state = DeviceState::new(Box::new(Fixed(reading)));
        state.set_who(Arc::new(|| Me {
            id: "g-station".into(),
            name: "Колонка".into(),
            kind: "station",
            roles: vec!["centre"],
        }));
        state
    }

    #[test]
    fn reads_the_kernels_numbers() {
        assert_eq!(sensor_celsius("47250\n"), Some(47.3));
        assert_eq!(sensor_celsius("52"), Some(52.0));
        assert_eq!(sensor_celsius("-273000"), None, "a sensor not connected");
        assert_eq!(sensor_celsius("x"), None);
        let before = parse_proc_stat("cpu  100 0 100 700 100 0 0 0 0 0\ncpu0 1 2 3").unwrap();
        assert_eq!(before, Ticks { busy: 200, total: 1000 });
        let now = parse_proc_stat("cpu  150 0 150 800 100 0 0 0 0 0\n").unwrap();
        assert_eq!(busy_percent(before, now), Some(50.0));
        assert_eq!(busy_percent(now, now), None);
        assert_eq!(parse_loadavg("0.52 0.41 0.30 1/123 4567\n"), Some([0.52, 0.41, 0.3]));
        assert_eq!(parse_boottime("{ sec = 1795163759, usec = 255914 } Fri Nov 20 16:35:59 2026"), Some(1795163759));
        assert_eq!(used_percent(123, 40), 67.5);
    }

    /// Stations differ: what one kernel gives, another does not — each part is read on its own, nothing missing
    /// breaks the rest. The numbers are a Station Lite 2's (Amlogic, 21.11.2026).
    #[test]
    fn reads_the_wifi_and_the_clock_and_takes_what_is_not_there_calmly() {
        let lite = "Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE\n \
                     face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22\n \
                    wlan0: 0000   70.  -33.  -256        0      0      0    165      0        0\n  \
                     p2p0: 0000    0     0     0        0      0      0      0      0        0\n";
        let wifi = parse_wireless(lite).unwrap();
        assert_eq!((wifi.interface.as_str(), wifi.dbm, wifi.percent), ("wlan0", Some(-33.0), Some(100.0)));
        // An unsigned signal, the station's interface named otherwise, the first one down.
        let other = "h1\nh2\n p2p0: 0000 0 0 0 0 0 0 0 0 0\n mlan0: 0000 40. 190. 0 0 0 0 0 0 0\n";
        let wifi = parse_wireless(other).unwrap();
        assert_eq!((wifi.interface.as_str(), wifi.dbm, wifi.percent), ("mlan0", Some(-66.0), Some(68.0)));
        // A link without a signal; nothing connected; only headers; nonsense.
        assert_eq!(parse_wireless("h\nh\n wlan0: 0000 35. 0 0\n").unwrap().percent, Some(50.0));
        assert!(parse_wireless("h\nh\n wlan0: 0000 0 -256 -256\n").is_none());
        assert!(parse_wireless("h\nh\n").is_none() && parse_wireless("").is_none());
        assert!(parse_wireless("h\nh\n wlan0: x y z\n").is_none());

        let root = tempfile::tempdir().unwrap();
        assert_eq!(cpu_mhz(root.path()), None, "a kernel without cpufreq");
        for (cpu, cur, max) in [("cpu0", "1200000", "1200000"), ("cpu1", "600000\n", "1800000"), ("cpu2", "", "")] {
            std::fs::create_dir_all(root.path().join(cpu).join("cpufreq")).unwrap();
            std::fs::write(root.path().join(cpu).join("cpufreq/scaling_cur_freq"), cur).unwrap();
            std::fs::write(root.path().join(cpu).join("cpufreq/cpuinfo_max_freq"), max).unwrap();
        }
        // Not cores: the kernel's own files beside them.
        std::fs::create_dir_all(root.path().join("cpufreq")).unwrap();
        std::fs::create_dir_all(root.path().join("cpuidle")).unwrap();
        assert_eq!(cpu_mhz(root.path()), Some(Mhz { now: 900, max: Some(1800) }));
    }

    #[test]
    fn finds_the_thermal_zones_and_takes_the_hottest() {
        let root = tempfile::tempdir().unwrap();
        for (zone, kind, temp) in [("thermal_zone0", "soc_thermal", "48000"), ("thermal_zone1", "ddr", "55500")] {
            std::fs::create_dir(root.path().join(zone)).unwrap();
            std::fs::write(root.path().join(zone).join("type"), format!("{kind}\n")).unwrap();
            std::fs::write(root.path().join(zone).join("temp"), temp).unwrap();
        }
        std::fs::create_dir(root.path().join("cooling_device0")).unwrap();
        let temperature = temperature_of(thermal_zones(root.path())).unwrap();
        assert_eq!(temperature.celsius, 55.5);
        assert_eq!(temperature.zones[0], Zone { name: "soc_thermal".into(), celsius: 48.0 });
        assert!(temperature_of(thermal_zones(&root.path().join("none"))).is_none());
    }

    #[tokio::test]
    async fn a_letter_gets_this_device_and_the_others_by_kind_or_name() {
        let state = fixed();
        state.set_others(Arc::new(|| {
            vec![json!({ "id": "g-mac", "name": "Мак", "kind": "computer", "roles": ["turbo"], "reachable": true,
                         "cpu": { "percent": 30.0 } })]
        }));
        let me = state.answer("state", &json!({})).await.unwrap();
        assert_eq!((me["name"].as_str(), me["itself"].as_bool()), (Some("Колонка"), Some(true)));
        assert_eq!(me["temperature"]["celsius"], 51.5);
        assert_eq!(me["memory"]["freeMb"], 40);
        assert_eq!(me["storage"]["of"], "/data");
        assert_eq!((me["wifi"]["dbm"].as_f64(), me["cpu"]["mhz"]["now"].as_u64()), (Some(-33.0), Some(1200)));
        assert_eq!(state.answer("state", &json!({ "device": "station" })).await.unwrap()["id"], "g-station");
        let mac = state.answer("state", &json!({ "device": "computer" })).await.unwrap();
        assert_eq!((mac["id"].as_str(), mac["itself"].as_bool()), (Some("g-mac"), Some(false)));
        assert_eq!(state.answer("state", &json!({ "device": "мак" })).await.unwrap()["id"], "g-mac");
        let all = state.answer("devices", &json!({})).await.unwrap();
        assert_eq!(all.as_array().map(Vec::len), Some(2));
        let none = state.answer("state", &json!({ "device": "телевизор" })).await.unwrap_err();
        assert!(none.contains("Мак"), "{none}");
        assert!(state.answer("reboot", &json!({})).await.is_err());
    }

    #[tokio::test]
    async fn a_graph_has_a_point_at_once_and_keeps_six_hours() {
        let state = fixed();
        let history = state.answer("history", &json!({})).await.unwrap();
        assert_eq!(history["everyS"], 60);
        assert_eq!(history["samples"][0]["temperature"], 51.5);
        assert_eq!(history["samples"][0]["memoryFreeMb"], 40);
        assert_eq!(
            (history["samples"][0]["wifiDbm"].as_f64(), history["samples"][0]["cpuMhz"].as_u64()),
            (Some(-33.0), Some(1200))
        );
        let mut mine = state.snapshot();
        state.record(std::slice::from_ref(&mine));
        let again = state.answer("history", &json!({})).await.unwrap();
        assert_eq!(again["samples"].as_array().map(Vec::len), Some(1), "the same reading is not counted twice");
        for minute in 0..KEEP + 10 {
            mine["at"] = json!(format!("minute {minute}"));
            state.record(std::slice::from_ref(&mine));
        }
        let all = state.answer("history", &json!({ "minutes": 100000 })).await.unwrap();
        assert_eq!(all["samples"].as_array().map(Vec::len), Some(KEEP));
        let hour = state.answer("history", &json!({})).await.unwrap();
        assert_eq!(hour["samples"].as_array().map(Vec::len), Some(60));
    }

    #[test]
    fn many_widgets_one_reading() {
        struct Counting(std::sync::atomic::AtomicUsize);
        impl Probe for Counting {
            fn read(&self) -> Reading {
                self.0.fetch_add(1, Ordering::SeqCst);
                Reading::default()
            }
        }
        let probe = Arc::new(Counting(std::sync::atomic::AtomicUsize::new(0)));
        struct Shared(Arc<Counting>);
        impl Probe for Shared {
            fn read(&self) -> Reading {
                self.0.read()
            }
        }
        let state = DeviceState::new(Box::new(Shared(probe.clone())));
        for _ in 0..20 {
            state.snapshot();
        }
        assert_eq!(probe.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn this_machine_reads_without_a_fuss() {
        let reading = Machine::new(std::env::temp_dir(), None, Instant::now()).read();
        assert!(reading.memory.is_some() || cfg!(windows));
        assert!(reading.temperature.is_some() || reading.missing.contains_key("temperature"));
    }
}
