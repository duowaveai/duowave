//! The bus's journal (docs/plans/bus.md, decision 13): one line a letter — when, which chain, who started it, from
//! whom to whom, which handle, how it went — never what the letter carried. Only appended to; past the place's limit
//! the oldest lines go. The panel reads it («Шина: что просили»), a person with the right to the journal.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use rusqlite::params;
use serde::Serialize;

use crate::storage::Db;
use crate::utils::time::now_iso;

/// Lines kept on the station and on a computer.
pub const STATION_LINES: u32 = 2_000;
pub const COMPUTER_LINES: u32 = 20_000;
/// The oldest lines go once this many were added since the last time.
const TRIM_EVERY: u32 = 50;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub seq: i64,
    pub at: String,
    pub trail: i64,
    pub root: String,
    /// `call`, `tell`, `ask`, `list`, `answer`.
    pub kind: String,
    pub from: String,
    pub to: String,
    pub handle: String,
    /// `ok`, `failed`, `asked`, `denied`, `notFound`, `fuse`, `yes`, `no`.
    pub outcome: String,
    pub ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A line to write.
#[derive(Debug, Clone, Default)]
pub struct Entry<'a> {
    pub trail: u64,
    pub root: &'a str,
    pub kind: &'a str,
    pub from: &'a str,
    pub to: &'a str,
    pub handle: &'a str,
    pub outcome: &'a str,
    pub ms: u64,
    pub note: Option<&'a str>,
}

pub struct Journal {
    db: Arc<Db>,
    keep: u32,
    added: AtomicU32,
}

impl Journal {
    pub fn new(db: Arc<Db>, keep: u32) -> Journal {
        Journal { db, keep: keep.max(100), added: AtomicU32::new(0) }
    }

    /// Writes a line; a failure is logged, never the caller's.
    pub fn write(&self, entry: Entry<'_>) {
        let written = self.db.with(|c| {
            c.execute(
                "INSERT INTO bus_journal (at, trail, root, kind, from_addr, to_addr, handle, outcome, ms, note)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    now_iso(),
                    entry.trail as i64,
                    entry.root,
                    entry.kind,
                    entry.from,
                    entry.to,
                    entry.handle,
                    entry.outcome,
                    entry.ms as i64,
                    entry.note.map(|note| note.chars().take(300).collect::<String>()),
                ],
            )
        });
        if let Err(error) = written {
            tracing::warn!(target: "bus", "journal: a line was not written: {error}");
            return;
        }
        if self.added.fetch_add(1, Ordering::Relaxed) + 1 >= TRIM_EVERY {
            self.added.store(0, Ordering::Relaxed);
            self.trim();
        }
    }

    /// Keeps the newest lines within the limit.
    pub fn trim(&self) {
        let keep = self.keep;
        let trimmed = self.db.with(|c| {
            c.execute(
                "DELETE FROM bus_journal WHERE seq <= (SELECT MAX(seq) FROM bus_journal) - ?1",
                params![keep as i64],
            )
        });
        if let Err(error) = trimmed {
            tracing::warn!(target: "bus", "journal: not trimmed: {error}");
        }
    }

    /// The newest lines first, older than `before` when given; only those about `about` (either side) when given.
    pub fn read(&self, about: Option<&str>, before: Option<i64>, limit: u32) -> Vec<Line> {
        let limit = limit.clamp(1, 500) as i64;
        let before = before.unwrap_or(i64::MAX);
        let read = self.db.with(|c| {
            let mut statement = c.prepare(
                "SELECT seq, at, trail, root, kind, from_addr, to_addr, handle, outcome, ms, note FROM bus_journal
                 WHERE seq < ?1 AND (?2 IS NULL OR from_addr = ?2 OR to_addr = ?2 OR from_addr LIKE ?3 OR to_addr LIKE ?3)
                 ORDER BY seq DESC LIMIT ?4",
            )?;
            let like = about.map(|about| format!("{about}/%"));
            let lines = statement
                .query_map(params![before, about, like, limit], |row| {
                    Ok(Line {
                        seq: row.get(0)?,
                        at: row.get(1)?,
                        trail: row.get(2)?,
                        root: row.get(3)?,
                        kind: row.get(4)?,
                        from: row.get(5)?,
                        to: row.get(6)?,
                        handle: row.get(7)?,
                        outcome: row.get(8)?,
                        ms: row.get(9)?,
                        note: row.get(10)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(lines)
        });
        read.unwrap_or_else(|error| {
            tracing::warn!(target: "bus", "journal: not read: {error}");
            Vec::new()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry<'a>(from: &'a str, to: &'a str, outcome: &'a str) -> Entry<'a> {
        Entry { trail: 7, root: "household", kind: "call", from, to, handle: "list", outcome, ..Entry::default() }
    }

    #[test]
    fn keeps_the_newest_lines_and_reads_about_one_participant() {
        let journal = Journal::new(Arc::new(Db::open(None).unwrap()), 100);
        for _ in 0..160 {
            journal.write(entry("app/a", "app/b", "ok"));
        }
        journal.write(entry("app/c", "files/app/c/tmp/x", "denied"));
        journal.trim();
        let all = journal.read(None, None, 500);
        assert_eq!(all.len(), 100);
        assert_eq!(all[0].outcome, "denied");
        let about = journal.read(Some("files/app/c"), None, 10);
        assert_eq!(about.len(), 1);
        assert_eq!(journal.read(Some("app/b"), None, 5).len(), 5);
        let older = journal.read(None, Some(all[0].seq), 1);
        assert_eq!(older[0].seq, all[1].seq);
    }
}
