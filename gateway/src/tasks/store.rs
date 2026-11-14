//! The tasks' table (`app_tasks`, migration 0010): a task's row is written when it starts, when it ends, and now and
//! then while it runs — enough for the next start of the gateway to say honestly which ones it cut short.

use rusqlite::params;
use serde_json::Value;

use super::{Step, Task, TaskState};
use crate::storage::Db;

/// A task as it was stored, with its steps.
pub struct Stored {
    pub task: Task,
    pub log: Vec<Step>,
}

pub fn save(db: &Db, task: &Task, log: &[Step]) -> rusqlite::Result<()> {
    let result = task.result.as_ref().map(Value::to_string);
    let log = serde_json::to_string(log).unwrap_or_else(|_| "[]".into());
    db.with(|c| {
        c.execute(
            "INSERT INTO app_tasks (id, app, title, state, percent, text, result, error, log, started_ms, updated_ms, ended_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT (id) DO UPDATE SET state = excluded.state, percent = excluded.percent, text = excluded.text,
                 result = excluded.result, error = excluded.error, log = excluded.log, updated_ms = excluded.updated_ms,
                 ended_ms = excluded.ended_ms",
            params![
                task.id,
                task.app,
                task.title,
                task.state.name(),
                task.percent.map(i64::from),
                task.text,
                result,
                task.error,
                log,
                task.started_at as i64,
                task.updated_at as i64,
                task.ended_at.map(|at| at as i64),
            ],
        )
        .map(|_| ())
    })
}

pub fn delete(db: &Db, id: &str) -> rusqlite::Result<()> {
    db.with(|c| c.execute("DELETE FROM app_tasks WHERE id = ?1", params![id]).map(|_| ()))
}

pub fn delete_app(db: &Db, app: &str) -> rusqlite::Result<()> {
    db.with(|c| c.execute("DELETE FROM app_tasks WHERE app = ?1", params![app]).map(|_| ()))
}

/// Every task stored, the oldest first. A row whose state is not one we know is left out.
pub fn load(db: &Db) -> rusqlite::Result<Vec<Stored>> {
    db.with(|c| {
        let mut statement = c.prepare(
            "SELECT id, app, title, state, percent, text, result, error, log, started_ms, updated_ms, ended_ms
             FROM app_tasks ORDER BY started_ms, id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, Option<i64>>(11)?,
            ))
        })?;
        let mut stored = Vec::new();
        for row in rows {
            let (id, app, title, state, percent, text, result, error, log, started, updated, ended) = row?;
            let Some(state) = TaskState::parse(&state) else { continue };
            stored.push(Stored {
                task: Task {
                    id,
                    app,
                    title,
                    state,
                    percent: percent.map(|percent| percent.clamp(0, 100) as u8),
                    text,
                    result: result.and_then(|text| serde_json::from_str(&text).ok()),
                    error,
                    cancel_requested: false,
                    started_at: started.max(0) as u64,
                    updated_at: updated.max(0) as u64,
                    ended_at: ended.map(|at| at.max(0) as u64),
                },
                log: serde_json::from_str(&log).unwrap_or_default(),
            });
        }
        Ok(stored)
    })
}
