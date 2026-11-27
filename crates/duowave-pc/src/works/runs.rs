//! The runs of a work and their journals (docs/plans/router-and-works.md, block A, 9; decisions 17 and 20). A run is
//! one call of the agent — the work, a note added on the way, a continuation — a row of `work_runs`. Its journal is two
//! files in the data folder: `works/<work>/runs/<n>.jsonl`, what the agent printed as it came («Скачать как есть»), and
//! `<n>.steps.jsonl`, the common steps one per line with their number, time and the call of the agent's helper they
//! belong to. The panel reads the steps from a place `after`; a run going on is read on as it grows. A run's journal
//! is at most 20 MB: past it only the end is written. The journal is local, the admin's only: it holds the task, the
//! files' names and pieces of code — never in a deal, never in the settings package. Kept 30 days, as the work's record.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

use super::harness::{Out, Step};
use crate::storage::Db;
use crate::utils::time::now_ms;

/// A run's journal at most: the raw file and the steps together.
pub const RUN_LIMIT: u64 = 20 * 1024 * 1024;
/// Steps a read gives at most.
pub const READ_MAX: usize = 1000;

/// What a run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum What {
    Work,
    Note,
    Resume,
    /// «С планом»: the plan, in a read-only session (stage 6).
    Plan,
    /// «С проверкой»: the review, in a read-only session on another model.
    Review,
}

impl What {
    pub fn as_str(self) -> &'static str {
        match self {
            What::Work => "work",
            What::Note => "note",
            What::Resume => "resume",
            What::Plan => "plan",
            What::Review => "review",
        }
    }

    fn parse(text: &str) -> What {
        match text {
            "note" => What::Note,
            "resume" => What::Resume,
            "plan" => What::Plan,
            "review" => What::Review,
            _ => What::Work,
        }
    }
}

/// A run as `work_runs` keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub n: u32,
    pub what: What,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    /// `ok`, `failed`, `limit`, `budget`, `stopped`, `interrupted`, `cut`; none — going.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deal: Option<i64>,
    pub cut: bool,
}

pub fn folder_of(dir: &Path, work: &str) -> PathBuf {
    dir.join(work)
}

fn runs_dir(dir: &Path, work: &str) -> PathBuf {
    folder_of(dir, work).join("runs")
}

pub fn raw_path(dir: &Path, work: &str, n: u32) -> PathBuf {
    runs_dir(dir, work).join(format!("{n}.jsonl"))
}

fn steps_path(dir: &Path, work: &str, n: u32) -> PathBuf {
    runs_dir(dir, work).join(format!("{n}.steps.jsonl"))
}

/// A new run's row: its number is the next of the work's.
pub fn begin(db: &Db, work: &str, what: What, model: Option<&str>, session: Option<&str>) -> rusqlite::Result<Run> {
    let started_at = now_ms();
    db.with(|c| {
        let last: Option<u32> = c
            .query_row("SELECT MAX(n) FROM work_runs WHERE work = ?1", params![work], |row| row.get(0))
            .optional()?
            .flatten();
        let n = last.unwrap_or(0) + 1;
        c.execute(
            "INSERT INTO work_runs (work, n, what, model, session, started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![work, n, what.as_str(), model, session, started_at as i64],
        )?;
        Ok(Run {
            n,
            what,
            model: model.map(str::to_string),
            session: session.map(str::to_string),
            started_at,
            ended_at: None,
            outcome: None,
            deal: None,
            cut: false,
        })
    })
}

/// A run ended: how, with which model and session, whether its journal was cut.
pub fn end(db: &Db, work: &str, n: u32, outcome: &str, model: Option<&str>, session: Option<&str>, cut: bool) {
    let done = db.with(|c| {
        c.execute(
            "UPDATE work_runs SET ended_at = ?3, outcome = ?4, model = COALESCE(?5, model), session = COALESCE(?6, session),
                 cut = ?7 WHERE work = ?1 AND n = ?2",
            params![work, n, now_ms() as i64, outcome, model, session, cut],
        )
    });
    if let Err(error) = done {
        tracing::warn!(target: "works", "the run {work}/{n} is not kept: {error}");
    }
}

/// The run's deal, once the router wrote it.
pub fn dealt(db: &Db, work: &str, n: u32, deal: i64) {
    let _ = db.with(|c| c.execute("UPDATE work_runs SET deal = ?3 WHERE work = ?1 AND n = ?2", params![work, n, deal]));
}

pub fn list(db: &Db, work: &str) -> rusqlite::Result<Vec<Run>> {
    db.with(|c| {
        let mut statement = c.prepare(
            "SELECT n, what, model, session, started_at, ended_at, outcome, deal, cut FROM work_runs WHERE work = ?1
             ORDER BY n",
        )?;
        let rows = statement.query_map(params![work], |row| {
            Ok(Run {
                n: row.get(0)?,
                what: What::parse(&row.get::<_, String>(1)?),
                model: row.get(2)?,
                session: row.get(3)?,
                started_at: row.get::<_, i64>(4)?.max(0) as u64,
                ended_at: row.get::<_, Option<i64>>(5)?.map(|at| at.max(0) as u64),
                outcome: row.get(6)?,
                deal: row.get(7)?,
                cut: row.get(8)?,
            })
        })?;
        rows.collect()
    })
}

/// The runs going when the gateway stopped: interrupted.
pub fn interrupt_all(db: &Db) -> usize {
    db.with(|c| {
        c.execute(
            "UPDATE work_runs SET outcome = 'interrupted', ended_at = ?1 WHERE ended_at IS NULL",
            params![now_ms() as i64],
        )
    })
    .unwrap_or(0)
}

/// A run's journal being written: the raw lines and the steps, up to `RUN_LIMIT`; `heard` hears each step (what the
/// work is at, the panel's events).
pub struct RunLog {
    raw: Mutex<Option<File>>,
    steps: Mutex<Option<File>>,
    count: AtomicU64,
    bytes: AtomicU64,
    cut: AtomicBool,
    limit: u64,
    heard: Box<dyn Fn(&Step) + Send + Sync>,
}

impl RunLog {
    pub fn create(
        dir: &Path,
        work: &str,
        n: u32,
        heard: Box<dyn Fn(&Step) + Send + Sync>,
    ) -> std::io::Result<Arc<RunLog>> {
        RunLog::create_with_limit(dir, work, n, RUN_LIMIT, heard)
    }

    pub fn create_with_limit(
        dir: &Path,
        work: &str,
        n: u32,
        limit: u64,
        heard: Box<dyn Fn(&Step) + Send + Sync>,
    ) -> std::io::Result<Arc<RunLog>> {
        std::fs::create_dir_all(runs_dir(dir, work))?;
        let open = |path: PathBuf| OpenOptions::new().create(true).append(true).open(path);
        let raw = open(raw_path(dir, work, n))?;
        let steps = open(steps_path(dir, work, n))?;
        let bytes = raw.metadata().map(|m| m.len()).unwrap_or(0) + steps.metadata().map(|m| m.len()).unwrap_or(0);
        let count = count_lines(&steps_path(dir, work, n));
        Ok(Arc::new(RunLog {
            raw: Mutex::new(Some(raw)),
            steps: Mutex::new(Some(steps)),
            count: AtomicU64::new(count),
            bytes: AtomicU64::new(bytes),
            cut: AtomicBool::new(false),
            limit,
            heard,
        }))
    }

    /// The journal reached its limit.
    pub fn was_cut(&self) -> bool {
        self.cut.load(Ordering::Relaxed)
    }

    fn write(&self, file: &Mutex<Option<File>>, line: &str) {
        let mut file = file.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(file) = file.as_mut() {
            let _ = file.write_all(line.as_bytes()).and_then(|()| file.write_all(b"\n"));
        }
        self.bytes.fetch_add(line.len() as u64 + 1, Ordering::Relaxed);
    }

    fn write_step(&self, parent: Option<&str>, step: &Step) {
        let n = self.count.fetch_add(1, Ordering::Relaxed);
        let mut value = serde_json::to_value(step).unwrap_or(Value::Null);
        if let Value::Object(map) = &mut value {
            map.insert("n".into(), n.into());
            map.insert("at".into(), now_ms().into());
            if let Some(parent) = parent {
                map.insert("parent".into(), parent.into());
            }
        }
        self.write(&self.steps, &value.to_string());
    }

    /// Over the limit now: the step `Cut` once, then only the end.
    fn over(&self) -> bool {
        if self.bytes.load(Ordering::Relaxed) < self.limit {
            return false;
        }
        if !self.cut.swap(true, Ordering::Relaxed) {
            self.write_step(None, &Step::Cut);
        }
        true
    }
}

impl Out for RunLog {
    fn raw(&self, line: &str) {
        if !self.over() {
            self.write(&self.raw, line);
        }
    }

    fn step(&self, parent: Option<&str>, step: Step) {
        (self.heard)(&step);
        if matches!(step, Step::End { .. }) || !self.over() {
            self.write_step(parent, &step);
        }
    }
}

fn count_lines(path: &Path) -> u64 {
    File::open(path).map(|file| BufReader::new(file).lines().count() as u64).unwrap_or(0)
}

/// Steps of a run's journal from the place `after` (how many were read before), at most `limit`; the place to read
/// on from. Only the troubles (`trouble`) and those with a word (`find`, any case) when asked.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Read {
    pub steps: Vec<Value>,
    pub next: u64,
}

pub fn read(dir: &Path, work: &str, n: u32, after: u64, limit: usize, trouble: bool, find: Option<&str>) -> Read {
    let path = steps_path(dir, work, n);
    let Ok(file) = File::open(&path) else { return Read { steps: Vec::new(), next: after } };
    let find = find.map(str::to_lowercase).filter(|find| !find.trim().is_empty());
    let mut steps = Vec::new();
    let mut next = after;
    for (at, line) in BufReader::new(file).lines().enumerate() {
        let Ok(line) = line else { break };
        // A line still being written has no end yet: read it the next time.
        if !line.ends_with('}') {
            break;
        }
        if (at as u64) < after {
            continue;
        }
        if steps.len() >= limit.min(READ_MAX) {
            break;
        }
        next = at as u64 + 1;
        let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
        if trouble && !is_trouble(&value) {
            continue;
        }
        if let Some(find) = &find {
            if !line.to_lowercase().contains(find.as_str()) {
                continue;
            }
        }
        steps.push(value);
    }
    Read { steps, next }
}

fn is_trouble(value: &Value) -> bool {
    serde_json::from_value::<Step>(value.clone()).map(|step| step.is_trouble()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::works::harness::{ToolKind, ToolState};

    #[test]
    fn a_journal_is_read_from_its_place_and_cut_at_its_limit_with_the_end_kept() {
        let dir = tempfile::tempdir().unwrap();
        let heard = Arc::new(Mutex::new(0usize));
        let counted = heard.clone();
        let log =
            RunLog::create_with_limit(dir.path(), "w1", 1, 2000, Box::new(move |_| *counted.lock().unwrap() += 1))
                .unwrap();
        log.raw(r#"{"type":"system","subtype":"init"}"#);
        log.step(None, Step::Says { text: "Начинаю".into() });
        let tool = |state| Step::Tool {
            id: "t1".into(),
            kind: ToolKind::Execute,
            title: "npm test".into(),
            path: None,
            command: Some("npm test".into()),
            output: None,
            diff: Vec::new(),
            state,
        };
        log.step(Some("t0"), tool(ToolState::Failed));
        let first = read(dir.path(), "w1", 1, 0, 100, false, None);
        assert_eq!(first.steps.len(), 2);
        assert_eq!(first.steps[1]["parent"], "t0");
        assert_eq!(first.next, 2);
        let only = read(dir.path(), "w1", 1, 0, 100, true, None);
        assert_eq!(only.steps.len(), 1, "only the failed call");
        assert_eq!(read(dir.path(), "w1", 1, 0, 100, false, Some("НАЧИНАЮ")).steps.len(), 1);
        for _ in 0..50 {
            log.step(None, Step::Says { text: "x".repeat(100) });
        }
        assert!(log.was_cut());
        log.step(
            None,
            Step::End {
                outcome: "ok".into(),
                text: Some("Готово".into()),
                error: None,
                usd_micro: None,
                unknown: None,
            },
        );
        let all = read(dir.path(), "w1", 1, 2, 1000, false, None);
        let types: Vec<&str> = all.steps.iter().filter_map(|step| step["type"].as_str()).collect();
        assert_eq!(types.last(), Some(&"end"), "the end is written past the limit");
        assert!(types.contains(&"cut"));
        assert!(std::fs::metadata(raw_path(dir.path(), "w1", 1)).unwrap().len() < 4000);
        assert_eq!(*heard.lock().unwrap(), 53);
    }
}
