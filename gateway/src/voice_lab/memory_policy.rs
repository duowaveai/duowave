//! The own voices' memory rules, as pure functions: whether a heavy program may start, the ceiling it runs under,
//! when it must stop. The household's recognition and voice come first, and the guard is automatic: there is no
//! number for the owner to pick (06.11: «не больше 4 ГБ», set for an imprint, kept the voice from starting — nobody
//! can know that a voice on the video card wants 6.3 GB).
//!  - Before a start: never a refusal, only a warning the owner may start past — while the computer is short already
//!    (pressure, paging, video memory) or short of room beside the household's reserve. 13.11: a refusal on pressure,
//!    read wrong on macOS 15, left a 48 GB Mac with 90 % free no way to take an imprint.
//!  - While an imprint runs: the watchdog stops it when the computer starts paging or pressing (MemoryWatch). 05.11:
//!    training beside the gateway swapped the Mac, and recognition missed 8 s. Started on «всё равно», a lasting warning
//!    alone does not stop it: the owner saw that warning and chose; critical pressure and paging still do.
//!  - Always: a ceiling well above the program's need, against a runaway (a leak, a bug). Past it the program is
//!    killed, not the gateway: Linux's OOM killer would otherwise pick the largest process, recognition included.

use serde::Serialize;

use super::js::{gb, round};
use super::system_memory::{MemorySnapshot, Pressure};

const GB: u64 = 1024;

/// Kept for the household: max(2 GB, 6 % of the memory), or VOICE_LAB_RESERVE_MB. The household's servers are
/// already resident; the reserve is their room to breathe (a long utterance, a long answer). Less than this left: a
/// warning.
pub fn reserve_mb(total_mb: u64, override_mb: Option<u64>) -> u64 {
    if let Some(mb) = override_mb {
        return mb;
    }
    (2 * GB).max(round(total_mb as f64 * 0.06) as u64)
}

/// A program's need: its measured peak with a margin, or the engine's default until it is measured.
pub fn need_mb(measured_peak_mb: Option<f64>, fallback_mb: u64) -> u64 {
    match measured_peak_mb.filter(|peak| *peak != 0.0 && !peak.is_nan()) {
        Some(peak) => round(peak * 1.15).max(0.0) as u64,
        None => fallback_mb,
    }
}

/// The ceiling a program runs under: half again its need, at least a gigabyte above it — only a runaway reaches it.
pub fn ceiling_mb(need: u64) -> u64 {
    (round(need as f64 * 1.5) as u64).max(need + GB)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AdmissionVerdict {
    /// There is room.
    Go,
    /// Short of room, or the computer short already: it starts only on the owner's «всё равно».
    Tight,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    pub verdict: AdmissionVerdict,
    pub need_mb: u64,
    /// What it runs under once started.
    pub ceiling_mb: u64,
    /// The refusal or the warning, in the owner's words.
    pub reason: Option<String>,
}

/// Paging in faster than this: the system is already short.
pub const START_SWAP_MBPS: f64 = 2.0;

/// Whether a program may start now, or the warning in the owner's words the start waits on.
pub fn admit(snapshot: &MemorySnapshot, reserve: u64, need: u64, video_need_mb: Option<u64>) -> Admission {
    let verdict = |verdict: AdmissionVerdict, reason: Option<String>| Admission {
        verdict,
        need_mb: need,
        ceiling_mb: ceiling_mb(need),
        reason,
    };
    if snapshot.pressure != Pressure::Normal {
        return verdict(
            AdmissionVerdict::Tight,
            Some(crate::tr!("Системе уже не хватает памяти — лучше подождать или закрыть тяжёлые программы.")),
        );
    }
    if snapshot.swap_in_mbps.unwrap_or(0.0) > START_SWAP_MBPS {
        return verdict(
            AdmissionVerdict::Tight,
            Some(crate::tr!("Компьютер сейчас подкачивает память с диска — лучше подождать минуту.")),
        );
    }
    if let (Some(video), Some(free)) = (video_need_mb, snapshot.vram_free_mb) {
        if free < video as f64 {
            return verdict(
                AdmissionVerdict::Tight,
                Some(crate::tr!(
                    "Видеопамяти свободно {free} ГБ, а нужно около {need} ГБ.",
                    free = gb(free),
                    need = gb(video as f64)
                )),
            );
        }
    }
    let pool = snapshot.pool_mb as f64;
    if pool - reserve as f64 >= need as f64 {
        return verdict(AdmissionVerdict::Go, None);
    }
    let reason = if snapshot.pool_mb >= need {
        crate::tr!(
            "Памяти впритык: свободно {free} ГБ, нужно около {need} ГБ, а {reserve} ГБ держу для колонки — она может подтормаживать.",
            free = gb(pool),
            need = gb(need as f64),
            reserve = gb(reserve as f64)
        )
    } else {
        crate::tr!(
            "Памяти мало: свободно {free} ГБ, а нужно около {need} ГБ — Мак начнёт подкачивать память с диска, колонка будет подтормаживать.",
            free = gb(pool),
            need = gb(need as f64)
        )
    };
    verdict(AdmissionVerdict::Tight, Some(reason))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StopReason {
    Memory,
    Pressure,
    Swap,
}

/// Milliseconds a warning state must last before a job is stopped for it.
const SUSTAINED_MS: u64 = 6_000;
/// Paging in this fast for SUSTAINED_MS: the job is pushing the household out of memory.
pub const STOP_SWAP_MBPS: f64 = 10.0;

/// One look at a running job: its own memory (as it reports it), the system's pressure and paging, when.
#[derive(Debug, Clone, Copy)]
pub struct WatchSample {
    pub job_mb: Option<f64>,
    pub pressure: Pressure,
    pub swap_in_mbps: Option<f64>,
    pub at: u64,
}

/// The watchdog's judgement over a running job's samples: its own memory against its ceiling, the system's pressure,
/// and paging. Stateful: a warning has to last before it stops anything.
pub struct MemoryWatch {
    limit: u64,
    /// Started on «всё равно»: a lasting warning alone does not stop it.
    anyway: bool,
    warn_since: Option<u64>,
    warn_start_mb: f64,
    swap_since: Option<u64>,
}

impl MemoryWatch {
    pub fn new(limit: u64, anyway: bool) -> MemoryWatch {
        MemoryWatch { limit, anyway, warn_since: None, warn_start_mb: 0.0, swap_since: None }
    }

    pub fn observe(&mut self, sample: WatchSample) -> Option<StopReason> {
        if sample.job_mb.is_some_and(|mb| mb > self.limit as f64) {
            return Some(StopReason::Memory);
        }
        if sample.pressure == Pressure::Critical {
            return Some(StopReason::Pressure);
        }
        if sample.pressure == Pressure::Warn && !self.anyway {
            match self.warn_since {
                None => {
                    self.warn_since = Some(sample.at);
                    self.warn_start_mb = sample.job_mb.unwrap_or(0.0);
                }
                Some(since) => {
                    if sample.at.saturating_sub(since) >= SUSTAINED_MS
                        && sample.job_mb.unwrap_or(0.0) > self.warn_start_mb
                    {
                        return Some(StopReason::Pressure);
                    }
                }
            }
        } else {
            self.warn_since = None;
        }
        if sample.swap_in_mbps.unwrap_or(0.0) > STOP_SWAP_MBPS {
            let since = *self.swap_since.get_or_insert(sample.at);
            if sample.at.saturating_sub(since) >= SUSTAINED_MS {
                return Some(StopReason::Swap);
            }
        } else {
            self.swap_since = None;
        }
        None
    }
}

/// Why a job was stopped, for the panel.
pub fn stop_message(reason: StopReason, limit: u64) -> String {
    match reason {
        StopReason::Memory => {
            crate::tr!(
                "Остановлено: модель заняла больше {limit} ГБ — намного больше обычного, похоже на сбой.",
                limit = gb(limit as f64)
            )
        }
        StopReason::Pressure => crate::tr!("Остановлено: системе стало не хватать памяти — колонка важнее."),
        StopReason::Swap => {
            crate::tr!("Остановлено: компьютер начал подкачивать память с диска — колонка важнее.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(pool_mb: u64) -> MemorySnapshot {
        MemorySnapshot {
            total_mb: 36_864,
            pool_mb,
            pressure: Pressure::Normal,
            swap_in_mbps: None,
            vram_free_mb: None,
            at: 0,
        }
    }

    fn sample(job_mb: f64, pressure: Pressure, swap_in_mbps: Option<f64>, at: u64) -> WatchSample {
        WatchSample { job_mb: Some(job_mb), pressure, swap_in_mbps, at }
    }

    #[test]
    fn keeps_max_2_gb_6_for_the_household_unless_told_otherwise() {
        assert_eq!(reserve_mb(36_864, None), 2_212);
        assert_eq!(reserve_mb(16_384, None), 2_048);
        assert_eq!(reserve_mb(131_072, None), 7_864);
        assert_eq!(reserve_mb(36_864, Some(5_000)), 5_000);
    }

    #[test]
    fn needs_the_measured_peak_with_a_margin_the_default_before_any_measurement() {
        assert_eq!(need_mb(None, 3_500), 3_500);
        assert_eq!(need_mb(Some(2_955.0), 3_500), 3_398);
    }

    #[test]
    fn sets_the_ceiling_well_above_the_need_only_a_runaway_reaches_it() {
        assert_eq!(ceiling_mb(3_526), 5_289);
        assert_eq!(ceiling_mb(6_500), 9_750);
        assert_eq!(ceiling_mb(1_500), 2_524);
    }

    #[test]
    fn starts_when_the_program_fits_beside_the_households_reserve_under_its_ceiling() {
        assert_eq!(
            admit(&snapshot(11_400), 2_212, 6_500, None),
            Admission { verdict: AdmissionVerdict::Go, need_mb: 6_500, ceiling_mb: 9_750, reason: None }
        );
        assert_eq!(admit(&snapshot(5_712), 2_212, 3_500, None).verdict, AdmissionVerdict::Go);
    }

    #[test]
    fn only_warns_when_room_is_tight_in_numbers_the_owner_may_start_anyway_06_11_a_4_gb_limit_kept_the_voice_off() {
        let tight = admit(&snapshot(5_120), 2_212, 3_500, None);
        assert_eq!((tight.verdict, tight.need_mb, tight.ceiling_mb), (AdmissionVerdict::Tight, 3_500, 5_250));
        assert_eq!(
            tight.reason.as_deref(),
            Some("Памяти впритык: свободно 5,0 ГБ, нужно около 3,4 ГБ, а 2,2 ГБ держу для колонки — она может подтормаживать.")
        );
        assert_eq!(
            admit(&snapshot(3_000), 2_212, 3_500, None).reason.as_deref(),
            Some(
                "Памяти мало: свободно 2,9 ГБ, а нужно около 3,4 ГБ — Мак начнёт подкачивать память с диска, колонка \
                 будет подтормаживать."
            )
        );
    }

    #[test]
    fn only_warns_while_the_computer_is_short_already_pressure_paging_video_memory_13_11_never_a_refusal() {
        let pressed = admit(&MemorySnapshot { pressure: Pressure::Warn, ..snapshot(20_000) }, 2_212, 3_500, None);
        assert_eq!(pressed.verdict, AdmissionVerdict::Tight);
        assert!(pressed.reason.unwrap().contains("не хватает памяти"));
        let critical = admit(&MemorySnapshot { pressure: Pressure::Critical, ..snapshot(20_000) }, 2_212, 3_500, None);
        assert_eq!(critical.verdict, AdmissionVerdict::Tight);
        let paging = admit(&MemorySnapshot { swap_in_mbps: Some(5.0), ..snapshot(20_000) }, 2_212, 3_500, None);
        assert_eq!(paging.verdict, AdmissionVerdict::Tight);
        assert!(paging.reason.unwrap().contains("подкачивает"));
        let video =
            admit(&MemorySnapshot { vram_free_mb: Some(1_000.0), ..snapshot(20_000) }, 2_212, 3_500, Some(3_000));
        assert_eq!(video.verdict, AdmissionVerdict::Tight);
        assert!(video.reason.unwrap().contains("Видеопамяти"));
        let calm = admit(&MemorySnapshot { swap_in_mbps: Some(1.0), ..snapshot(20_000) }, 2_212, 3_500, None);
        assert_eq!(calm.verdict, AdmissionVerdict::Go);
    }

    #[test]
    fn stops_a_job_past_its_ceiling_at_once_on_critical_pressure_at_once_on_lasting_warnings_and_paging_after_6_s() {
        assert_eq!(
            MemoryWatch::new(4_000, false).observe(sample(4_100.0, Pressure::Normal, None, 0)),
            Some(StopReason::Memory)
        );
        assert_eq!(
            MemoryWatch::new(4_000, false).observe(sample(1_000.0, Pressure::Critical, None, 0)),
            Some(StopReason::Pressure)
        );

        let mut warned = MemoryWatch::new(4_000, false);
        assert_eq!(warned.observe(sample(1_000.0, Pressure::Warn, None, 0)), None);
        assert_eq!(warned.observe(sample(1_000.0, Pressure::Warn, None, 7_000)), None, "not growing: not the job");
        assert_eq!(warned.observe(sample(1_500.0, Pressure::Warn, None, 8_000)), Some(StopReason::Pressure));

        let mut chosen = MemoryWatch::new(4_000, true);
        assert_eq!(chosen.observe(sample(1_000.0, Pressure::Warn, None, 0)), None);
        assert_eq!(chosen.observe(sample(1_500.0, Pressure::Warn, None, 8_000)), None, "the owner chose «всё равно»");
        assert_eq!(chosen.observe(sample(1_500.0, Pressure::Critical, None, 9_000)), Some(StopReason::Pressure));

        let mut paging = MemoryWatch::new(4_000, false);
        assert_eq!(paging.observe(sample(1_000.0, Pressure::Normal, Some(40.0), 0)), None);
        assert_eq!(paging.observe(sample(1_000.0, Pressure::Normal, Some(0.0), 3_000)), None, "a burst");
        assert_eq!(paging.observe(sample(1_000.0, Pressure::Normal, Some(40.0), 4_000)), None);
        assert_eq!(paging.observe(sample(1_000.0, Pressure::Normal, Some(40.0), 10_000)), Some(StopReason::Swap));
    }

    #[test]
    fn tells_why_it_stopped() {
        assert_eq!(
            stop_message(StopReason::Memory, 5_289),
            "Остановлено: модель заняла больше 5,2 ГБ — намного больше обычного, похоже на сбой."
        );
        assert!(stop_message(StopReason::Swap, 4_096).contains("подкачивать"));
    }
}
