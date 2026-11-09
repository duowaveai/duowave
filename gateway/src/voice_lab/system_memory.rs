//! The voice lab's view of memory: the computer's pool sampled through the lab's probe (so its tests stand in for the
//! system), the paging rate between two samples, and the free video memory of an NVIDIA card. Counting the pool
//! itself is shared with the plugins' budget — `operations/system_memory.rs`.

use std::path::Path;
use std::sync::Mutex;

use crate::operations::system_memory::number;
pub use crate::operations::system_memory::{parse_darwin, parse_linux, windows_counters, Counters, Pressure};

use super::js;
use super::platform::{OsName, ProbeCell, PROBE_TIMEOUT_MS};

#[derive(Debug, Clone, PartialEq)]
pub struct MemorySnapshot {
    pub total_mb: u64,
    /// What can be taken without swapping.
    pub pool_mb: u64,
    pub pressure: Pressure,
    /// Pages coming back from swap, MB/s, since the previous sample; none on the first or where unknown.
    pub swap_in_mbps: Option<f64>,
    /// NVIDIA: free video memory of the first card.
    pub vram_free_mb: Option<f64>,
    /// When, ms since the epoch.
    pub at: u64,
}

const MB: f64 = 1_048_576.0;

/// Something the sampling asks each time: the system it runs on, whether there is an NVIDIA card, the time.
pub type Fact<T> = Box<dyn Fn() -> T + Send + Sync>;

#[derive(Default)]
struct Samples {
    cached: Option<MemorySnapshot>,
    previous: Option<(f64, u64)>,
}

pub struct SystemMemory {
    probe: ProbeCell,
    os: Fact<OsName>,
    nvidia: Fact<bool>,
    now: Fact<u64>,
    samples: Mutex<Samples>,
}

impl SystemMemory {
    pub fn new(probe: ProbeCell, os: Fact<OsName>, nvidia: Fact<bool>, now: Fact<u64>) -> SystemMemory {
        SystemMemory { probe, os, nvidia, now, samples: Mutex::new(Samples::default()) }
    }

    /// For views: one real sample per 5 seconds, however often the panel asks.
    pub async fn snapshot(&self) -> MemorySnapshot {
        let cached = self.samples.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).cached.clone();
        if let Some(cached) = cached {
            if (self.now)().saturating_sub(cached.at) < 5_000 {
                return cached;
            }
        }
        self.sample().await
    }

    /// A fresh sample: what the watchdog decides on.
    pub async fn sample(&self) -> MemorySnapshot {
        let probe = self.probe.get();
        let counters = match self.counters().await {
            Some(counters) => counters,
            None => {
                let (free, total) = probe.memory_bytes();
                windows_counters(free, total)
            }
        };
        let at = (self.now)();
        let mut vram_free_mb = None;
        if (self.nvidia)() {
            let query = ["--query-gpu=memory.free", "--format=csv,noheader,nounits"];
            let smi = probe.run("nvidia-smi", &query, PROBE_TIMEOUT_MS).await;
            let free = number(smi.stdout.split('\n').next().unwrap_or(""));
            if smi.ok() && free.is_finite() {
                vram_free_mb = Some(free);
            }
        }
        let mut samples = self.samples.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut swap_in_mbps = None;
        if let Some(bytes) = counters.swap_in_bytes {
            if let Some((previous_bytes, previous_at)) = samples.previous {
                if at > previous_at {
                    let rate = (bytes - previous_bytes) / MB / ((at - previous_at) as f64 / 1000.0);
                    swap_in_mbps = Some(js::round(rate.max(0.0) * 10.0) / 10.0);
                }
            }
            samples.previous = Some((bytes, at));
        }
        let snapshot = MemorySnapshot {
            total_mb: counters.total_mb,
            pool_mb: counters.pool_mb,
            pressure: counters.pressure,
            swap_in_mbps,
            vram_free_mb,
            at,
        };
        samples.cached = Some(snapshot.clone());
        snapshot
    }

    async fn counters(&self) -> Option<Counters> {
        let probe = self.probe.get();
        match (self.os)() {
            OsName::Darwin => {
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
                let result = probe.run("sysctl", &names, PROBE_TIMEOUT_MS).await;
                if result.ok() {
                    parse_darwin(&result.stdout, probe.total_mb())
                } else {
                    None
                }
            }
            OsName::Linux => {
                let meminfo = probe.read(Path::new("/proc/meminfo"))?;
                let psi = probe.read(Path::new("/proc/pressure/memory"));
                let vmstat = probe.read(Path::new("/proc/vmstat"));
                parse_linux(&meminfo, psi.as_deref(), vmstat.as_deref())
            }
            _ => {
                let (free, total) = probe.memory_bytes();
                Some(windows_counters(free, total))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use futures::future::BoxFuture;

    use super::*;
    use crate::voice_lab::platform::{CommandResult, Probe};

    struct Pages {
        swapins: Arc<AtomicU64>,
    }

    impl Probe for Pages {
        fn platform(&self) -> String {
            "darwin".into()
        }
        fn arch(&self) -> String {
            "arm64".into()
        }
        fn env(&self, _name: &str) -> Option<String> {
            None
        }
        fn run(&self, command: &str, _args: &[&str], _timeout_ms: u64) -> BoxFuture<'static, CommandResult> {
            let stdout = if command == "sysctl" {
                format!("16384\n500000\n0\n0\n0\n0\n1\n{}", self.swapins.load(Ordering::SeqCst))
            } else {
                "10240\n".into()
            };
            Box::pin(async move { CommandResult { code: Some(0), stdout } })
        }
        fn exists(&self, _path: &Path) -> bool {
            false
        }
        fn read(&self, _path: &Path) -> Option<String> {
            None
        }
        fn total_mb(&self) -> u64 {
            36_864
        }
        fn memory_bytes(&self) -> (u64, u64) {
            (0, 0)
        }
        fn cpus(&self) -> (usize, Option<String>) {
            (1, None)
        }
        fn free_disk_mb(&self, _path: &Path) -> u64 {
            0
        }
        fn home(&self) -> PathBuf {
            PathBuf::from("/home/owner")
        }
    }

    #[tokio::test]
    async fn rates_paging_between_two_samples_and_asks_nvidia_smi_for_free_video_memory() {
        let swapins = Arc::new(AtomicU64::new(1_000));
        let now = Arc::new(AtomicU64::new(0));
        let clock = now.clone();
        let memory = SystemMemory::new(
            ProbeCell::new(Arc::new(Pages { swapins: swapins.clone() })),
            Box::new(|| OsName::Darwin),
            Box::new(|| true),
            Box::new(move || clock.load(Ordering::SeqCst)),
        );
        let first = memory.sample().await;
        assert_eq!(first.swap_in_mbps, None);
        assert_eq!(first.vram_free_mb, Some(10_240.0));
        swapins.fetch_add(640, Ordering::SeqCst); // 10 MB of 16 KB pages in 2 s
        now.store(2_000, Ordering::SeqCst);
        assert_eq!(memory.sample().await.swap_in_mbps, Some(5.0));
        now.store(3_000, Ordering::SeqCst);
        assert_eq!(memory.snapshot().await.at, 2_000, "views reuse a sample for 5 s");
    }
}
