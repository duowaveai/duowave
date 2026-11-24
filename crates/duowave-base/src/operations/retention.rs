//! Journals and recorded audio grow by the day: RECORDER_RETENTION_DAYS keeps that many days of session folders
//! (data/sessions/<day>) and removes older ones, once at start and then daily. 0 keeps everything.
//!
//! RECORDER_MAX_MB caps the whole journal: over it, whole days go oldest first, then the oldest sessions of the
//! newest day; the newest session — the one being written — stays. The station's /data has some 60 MB, and a
//! journal kept a week fills it in days (06.11 on the station: 0.8 MB in five minutes), so the station build caps
//! it at 10 MB; with a cap the sweep runs every ten minutes.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use regex::Regex;
use tokio_util::sync::CancellationToken;

use crate::utils::time::now_ms;

const DAY_MS: u64 = 86_400_000;
/// How often a capped journal is swept: a day of talk is well under the cap's ten minutes of growth.
const CAPPED_SWEEP_MS: u64 = 600_000;

static DAY_FOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").expect("the pattern compiles"));

pub struct RetentionService {
    dir: PathBuf,
    days: u64,
    /// The journal's whole size in bytes; 0 is no cap.
    cap: u64,
    cancel: CancellationToken,
}

impl RetentionService {
    pub fn new(dir: PathBuf, days: u64, cap: u64) -> RetentionService {
        RetentionService { dir, days, cap, cancel: CancellationToken::new() }
    }

    /// Sweeps once now and then daily (the TS `onApplicationBootstrap`), every ten minutes with a cap; nothing when
    /// neither is on.
    pub fn start(self: &Arc<Self>) {
        if self.days == 0 && self.cap == 0 {
            return;
        }
        self.sweep(now_ms());
        let me = self.clone();
        let cancel = self.cancel.clone();
        let every = if self.cap > 0 { CAPPED_SWEEP_MS } else { DAY_MS };
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(every));
            ticker.tick().await; // the immediate first tick; the start already swept
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = ticker.tick() => { me.sweep(now_ms()); }
                }
            }
        });
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }

    /// Removes day folders (YYYY-MM-DD) older than the retention, then what is over the cap; returns what it removed.
    pub fn sweep(&self, now: u64) -> Vec<String> {
        let mut removed = if self.days > 0 { self.expire(now) } else { Vec::new() };
        if self.cap > 0 {
            removed.extend(self.trim());
        }
        removed
    }

    fn expire(&self, now: u64) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut names: Vec<String> =
            entries.filter_map(|entry| entry.ok()).filter_map(|entry| entry.file_name().into_string().ok()).collect();
        // Deterministic order, so the removed list reads oldest-first as the panel and the log expect.
        names.sort();
        let mut removed = Vec::new();
        let cutoff = now.saturating_sub(self.days.saturating_mul(DAY_MS));
        for name in names {
            if !DAY_FOLDER.is_match(&name) {
                continue;
            }
            let folder = self.dir.join(&name);
            let Some(day) = day_ms(&name) else { continue };
            let too_new = day + DAY_MS > cutoff;
            let is_dir = folder.is_dir();
            if too_new || !is_dir {
                continue;
            }
            if std::fs::remove_dir_all(&folder).is_ok() {
                removed.push(name);
            }
        }
        if !removed.is_empty() {
            tracing::info!("journals older than {} days removed: {}", self.days, removed.join(", "));
        }
        removed
    }

    /// Keeps the journal under the cap: whole days oldest first, then the newest day's sessions oldest first
    /// (their names start with the time); the newest session stays.
    fn trim(&self) -> Vec<String> {
        let mut total = size_of(&self.dir);
        if total <= self.cap {
            return Vec::new();
        }
        let mut days = folders(&self.dir);
        days.retain(|name| DAY_FOLDER.is_match(name));
        let Some(newest) = days.pop() else { return Vec::new() };
        let mut candidates: Vec<String> = days;
        let mut sessions = folders(&self.dir.join(&newest));
        sessions.pop(); // the one being written
        candidates.extend(sessions.into_iter().map(|session| format!("{newest}/{session}")));
        let mut removed = Vec::new();
        for name in candidates {
            if total <= self.cap {
                break;
            }
            let path = self.dir.join(&name);
            let size = size_of(&path);
            if std::fs::remove_dir_all(&path).is_ok() {
                total = total.saturating_sub(size);
                removed.push(name);
            }
        }
        if !removed.is_empty() {
            tracing::info!("journal over {} MB: removed {}", self.cap / 1_048_576, removed.join(", "));
        }
        removed
    }
}

/// The folders in `dir`, by name.
fn folders(dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// Bytes under `path`, files of every depth; links are not followed.
fn size_of(path: &std::path::Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if !meta.is_dir() {
        return meta.len();
    }
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries.filter_map(|entry| entry.ok()).map(|entry| size_of(&entry.path())).sum()
}

/// Midnight UTC of a `YYYY-MM-DD` day, in ms; None when it is not a date.
fn day_ms(name: &str) -> Option<u64> {
    let at = format!("{name}T00:00:00Z").parse::<chrono::DateTime<chrono::Utc>>().ok()?;
    Some(at.timestamp_millis().max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_journal_days_older_than_the_retention_and_nothing_else() {
        let dir = tempfile::Builder::new().prefix("journals-").tempdir().unwrap();
        for day in ["2026-10-20", "2026-10-28", "2026-10-31", "not-a-day"] {
            std::fs::create_dir(dir.path().join(day)).unwrap();
        }
        let service = RetentionService::new(dir.path().to_path_buf(), 3, 0);
        let now = "2026-11-01T12:00:00Z".parse::<chrono::DateTime<chrono::Utc>>().unwrap().timestamp_millis() as u64;
        assert_eq!(service.sweep(now), vec!["2026-10-20".to_string(), "2026-10-28".into()]);
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok().and_then(|entry| entry.file_name().into_string().ok()))
            .collect();
        left.sort();
        assert_eq!(left, vec!["2026-10-31".to_string(), "not-a-day".into()]);
    }

    #[test]
    fn keeps_the_journal_under_its_cap_oldest_first_and_the_newest_session_always() {
        let dir = tempfile::Builder::new().prefix("journals-").tempdir().unwrap();
        let put = |path: &str, bytes: usize| {
            let file = dir.path().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, vec![b'x'; bytes]).unwrap();
        };
        put("2026-11-04/0901_h_a/events.jsonl", 3000);
        put("2026-11-05/1002_h_b/events.jsonl", 3000);
        put("2026-11-06/1100_h_c/events.jsonl", 2000);
        put("2026-11-06/1200_h_d/events.jsonl", 2000);
        put("2026-11-06/1300_h_e/events.jsonl", 3000);
        let now = "2026-11-06T13:30:00Z".parse::<chrono::DateTime<chrono::Utc>>().unwrap().timestamp_millis() as u64;

        // Under the cap: nothing goes; retention off.
        assert!(RetentionService::new(dir.path().to_path_buf(), 0, 20_000).sweep(now).is_empty());
        // Over it: the old days first, then the newest day's sessions, until it fits.
        assert_eq!(
            RetentionService::new(dir.path().to_path_buf(), 0, 6000).sweep(now),
            vec!["2026-11-04".to_string(), "2026-11-05".into(), "2026-11-06/1100_h_c".into()]
        );
        // A cap smaller than the newest session: everything else goes, the session being written stays.
        assert_eq!(
            RetentionService::new(dir.path().to_path_buf(), 0, 100).sweep(now),
            vec!["2026-11-06/1200_h_d".to_string()]
        );
        assert_eq!(folders(&dir.path().join("2026-11-06")), vec!["1300_h_e".to_string()]);
    }
}
