//! How much memory the machine can give without swapping and how hard it is pressed — each system counted its own
//! way, for everyone who needs it: the voice lab's watchdog (a PC's), the plugins' budget (docs/plans/plugins.md,
//! «Память и место на колонке» — on the station above all, where there is no swap and the OOM killer takes Alice's
//! `quasar` along with the gateway).
//!
//! - macOS: kern.memorystatus_level is useless here (75 % «free» with 11 GB of swap in use, 05.11); the pool is free +
//!   speculative + purgeable + reusable pages and half the file-backed ones.
//! - Linux (the station): MemAvailable and PSI.
//! - Windows: the available physical memory.
//!
//! The parsers are pure; `read_counters` reads this machine directly (the voice lab reads through its probe, so its
//! tests can stand in for the system).

use std::path::Path;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Pressure {
    Normal,
    Warn,
    Critical,
}

impl Pressure {
    pub fn as_str(self) -> &'static str {
        match self {
            Pressure::Normal => "normal",
            Pressure::Warn => "warn",
            Pressure::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Counters {
    pub total_mb: u64,
    pub pool_mb: u64,
    pub pressure: Pressure,
    /// A monotonic count of swapped-in bytes, for the rate.
    pub swap_in_bytes: Option<f64>,
}

const MB: f64 = 1_048_576.0;

/// JavaScript's trim, the byte order mark included.
fn trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// Math.round: a half goes up.
fn round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// Number(text): "" is 0, anything else must be a whole decimal number.
pub fn number(text: &str) -> f64 {
    let text = trim(text);
    if text.is_empty() {
        return 0.0;
    }
    text.parse::<f64>().ok().filter(|value| value.is_finite()).unwrap_or(f64::NAN)
}

/// `sysctl -n hw.pagesize vm.page_*_count kern.memorystatus_vm_pressure_level vm.compressor.swapper.swapins_total`.
pub fn parse_darwin(stdout: &str, total_mb: u64) -> Option<Counters> {
    let values: Vec<f64> = trim(stdout).split('\n').map(number).collect();
    if values.len() < 8 || values.iter().any(|value| !value.is_finite()) {
        return None;
    }
    let [page, free, speculative, purgeable, reusable, external, level, swapins] =
        [values[0], values[1], values[2], values[3], values[4], values[5], values[6], values[7]];
    let pool = (free + speculative + purgeable + reusable) * page + (external * page) / 2.0;
    Some(Counters {
        total_mb,
        pool_mb: round(pool / MB).max(0.0) as u64,
        pressure: if level >= 4.0 {
            Pressure::Critical
        } else if level >= 2.0 {
            Pressure::Warn
        } else {
            Pressure::Normal
        },
        swap_in_bytes: Some(swapins * page),
    })
}

/// `^name:\s+(\d+)` in /proc/meminfo, in bytes.
fn meminfo_field(meminfo: &str, name: &str) -> Option<f64> {
    meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix(name)?.strip_prefix(':')?;
        let value = rest.trim_start();
        if value.len() == rest.len() {
            return None; // no white space after the colon
        }
        let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
        (!digits.is_empty()).then(|| number(&digits) * 1024.0)
    })
}

/// `^kind avg10=([\d.]+)` in /proc/pressure/memory.
fn psi_average(psi: &str, kind: &str) -> f64 {
    psi.lines()
        .find_map(|line| {
            let value = line.strip_prefix(kind)?.strip_prefix(" avg10=")?;
            let figure: String = value.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            (!figure.is_empty()).then(|| number(&figure))
        })
        .unwrap_or(0.0)
}

/// /proc/meminfo, /proc/pressure/memory (PSI) and /proc/vmstat's pswpin (4 KB pages).
pub fn parse_linux(meminfo: &str, psi: Option<&str>, vmstat: Option<&str>) -> Option<Counters> {
    let total = meminfo_field(meminfo, "MemTotal").filter(|total| *total != 0.0)?;
    let available = meminfo_field(meminfo, "MemAvailable").or_else(|| meminfo_field(meminfo, "MemFree"))?;
    let share = available / total;
    let mut pressure = if share < 0.05 {
        Pressure::Critical
    } else if share < 0.1 {
        Pressure::Warn
    } else {
        Pressure::Normal
    };
    if let Some(psi) = psi {
        pressure = if psi_average(psi, "full") >= 10.0 {
            Pressure::Critical
        } else if psi_average(psi, "some") >= 20.0 {
            Pressure::Warn
        } else {
            Pressure::Normal
        };
    }
    let swapins = vmstat.and_then(|vmstat| {
        vmstat.lines().find_map(|line| {
            let value = line.strip_prefix("pswpin")?;
            let digits = value.trim_start();
            if digits.len() == value.len() {
                return None;
            }
            let digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
            (!digits.is_empty()).then_some(digits)
        })
    });
    Some(Counters {
        total_mb: round(total / MB) as u64,
        pool_mb: round(available / MB) as u64,
        pressure,
        swap_in_bytes: swapins.map(|pages| number(&pages) * 4096.0),
    })
}

/// Windows: the available physical memory; pressure by its share.
pub fn windows_counters(free_bytes: u64, total_bytes: u64) -> Counters {
    let share = free_bytes as f64 / total_bytes.max(1) as f64;
    Counters {
        total_mb: round(total_bytes as f64 / MB) as u64,
        pool_mb: round(free_bytes as f64 / MB) as u64,
        pressure: if share < 0.05 {
            Pressure::Critical
        } else if share < 0.1 {
            Pressure::Warn
        } else {
            Pressure::Normal
        },
        swap_in_bytes: None,
    }
}

/// `VmRSS:` of /proc/<pid>/status, in MB.
pub fn parse_rss(status: &str) -> Option<u64> {
    meminfo_field(status, "VmRSS").map(|bytes| round(bytes / MB) as u64)
}

/// This machine's counters now, read directly: /proc on Linux, `sysctl` on macOS. None where it cannot tell.
pub fn read_counters() -> Option<Counters> {
    if cfg!(target_os = "linux") {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        let psi = std::fs::read_to_string("/proc/pressure/memory").ok();
        let vmstat = std::fs::read_to_string("/proc/vmstat").ok();
        return parse_linux(&meminfo, psi.as_deref(), vmstat.as_deref());
    }
    if cfg!(target_os = "macos") {
        let total = sysctl(&["-n", "hw.memsize"]).map(|text| number(&text))?;
        let names = [
            "-n",
            "hw.pagesize",
            "vm.page_free_count",
            "vm.page_speculative_count",
            "vm.page_purgeable_count",
            "vm.page_reusable_count",
            "vm.page_pageable_external_count",
            "kern.memorystatus_vm_pressure_level",
            "vm.compressor.swapper.swapins_total",
        ];
        return parse_darwin(&sysctl(&names)?, round(total / MB) as u64);
    }
    None
}

fn sysctl(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("sysctl").args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// How much memory a process holds now (its resident set), MB: this gateway's when `pid` is none. /proc on Linux,
/// `ps` elsewhere; None where it cannot tell.
pub fn process_rss_mb(pid: Option<u32>) -> Option<u64> {
    if cfg!(target_os = "linux") {
        let file = match pid {
            Some(pid) => format!("/proc/{pid}/status"),
            None => "/proc/self/status".into(),
        };
        return parse_rss(&std::fs::read_to_string(file).ok()?);
    }
    if cfg!(unix) {
        let pid = pid.unwrap_or_else(std::process::id).to_string();
        let output = std::process::Command::new("ps").args(["-o", "rss=", "-p", &pid]).output().ok()?;
        let kb = number(&String::from_utf8_lossy(&output.stdout));
        return (output.status.success() && kb.is_finite()).then(|| round(kb / 1024.0) as u64);
    }
    None
}

/// Free room on the disk that holds `path`, MB; None where it cannot tell.
#[cfg(unix)]
pub fn free_disk_mb(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    // The folder may not exist yet: its nearest parent that does is on the same disk.
    let existing = path.ancestors().find(|dir| dir.exists())?;
    let name = std::ffi::CString::new(existing.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a valid C string and a zeroed struct the call fills.
    if unsafe { libc::statvfs(name.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    let bytes = stats.f_bavail as u64 * stats.f_frsize as u64;
    Some(bytes / 1_048_576)
}

#[cfg(not(unix))]
pub fn free_disk_mb(_path: &Path) -> Option<u64> {
    None
}

/// What a folder holds, its files counted, MB rounded up.
pub fn folder_mb(path: &Path) -> u64 {
    fn walk(path: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(path) else { return 0 };
        entries
            .filter_map(Result::ok)
            .map(|entry| match entry.file_type() {
                Ok(kind) if kind.is_dir() => walk(&entry.path()),
                Ok(kind) if kind.is_file() => entry.metadata().map(|meta| meta.len()).unwrap_or(0),
                _ => 0,
            })
            .sum()
    }
    walk(path).div_ceil(1_048_576)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_a_mac_pool_from_its_pages_not_from_memorystatus_level_05_11_75_free_while_swapping() {
        // The counters of 06.11 00:05, page 16 KB: free, speculative, purgeable, reusable, file-backed, level,
        // swap-ins.
        let counters = parse_darwin("16384\n287115\n6239\n12839\n61438\n224580\n1\n18248758", 36_864);
        assert_eq!(
            counters,
            Some(Counters {
                total_mb: 36_864,
                pool_mb: 7_499,
                pressure: Pressure::Normal,
                swap_in_bytes: Some(18_248_758.0 * 16_384.0),
            })
        );
        assert_eq!(parse_darwin("16384\n1\n1\n1\n1\n1\n4\n0", 36_864).map(|c| c.pressure), Some(Pressure::Critical));
        assert_eq!(parse_darwin("garbage", 36_864), None);
    }

    #[test]
    fn takes_linux_memavailable_and_psi_without_psi_the_share_of_free_memory() {
        let meminfo = "MemTotal:       16384000 kB\nMemFree:          512000 kB\nMemAvailable:    8192000 kB\n";
        let psi = "some avg10=1.20 avg60=0.5 avg300=0.1 total=1\nfull avg10=0.00 avg60=0 avg300=0 total=0";
        assert_eq!(
            parse_linux(meminfo, Some(psi), Some("pswpin 1000\n")),
            Some(Counters {
                total_mb: 16_000,
                pool_mb: 8_000,
                pressure: Pressure::Normal,
                swap_in_bytes: Some(4_096_000.0)
            })
        );
        let warn = "some avg10=35.00 avg60=1 avg300=1 total=1\nfull avg10=2.00 avg60=0 avg300=0 total=0";
        assert_eq!(parse_linux(meminfo, Some(warn), None).map(|c| c.pressure), Some(Pressure::Warn));
        let critical = "some avg10=80.00\nfull avg10=25.00";
        assert_eq!(parse_linux(meminfo, Some(critical), None).map(|c| c.pressure), Some(Pressure::Critical));
        let short = "MemTotal: 16384000 kB\nMemAvailable: 600000 kB\n";
        assert_eq!(parse_linux(short, None, None).map(|c| c.pressure), Some(Pressure::Critical));
    }

    #[test]
    fn takes_windows_available_memory_pressure_by_its_share() {
        assert_eq!(
            windows_counters(8 * 1_048_576 * 1024, 32 * 1_048_576 * 1024),
            Counters { total_mb: 32_768, pool_mb: 8_192, pressure: Pressure::Normal, swap_in_bytes: None }
        );
        assert_eq!(windows_counters(1_048_576 * 1024, 32 * 1_048_576 * 1024).pressure, Pressure::Critical);
    }

    #[test]
    fn reads_a_processs_resident_memory_and_this_machines_disk() {
        // The station's gateway (Lite 2, 07.11): 13,6 MB resident.
        let status = "Name:\tduowave\nVmPeak:\t   40000 kB\nVmRSS:\t   13940 kB\nThreads:\t9\n";
        assert_eq!(parse_rss(status), Some(14));
        assert_eq!(parse_rss("Name:\tduowave\n"), None);
        assert!(process_rss_mb(None).is_some_and(|mb| mb > 0), "this test's own process holds memory");
        let dir = tempfile::tempdir().unwrap();
        assert!(free_disk_mb(&dir.path().join("not/yet")).is_some(), "a folder to be is on its parent's disk");
        std::fs::write(dir.path().join("a"), vec![0u8; 1_500_000]).unwrap();
        assert_eq!(folder_mb(dir.path()), 2);
    }
}
