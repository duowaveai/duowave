//! The budgets (docs/plans/router-and-works.md, block A, 5; decision 10): how much a work may spend. A budget has a
//! measure — dollars or tokens —, a limit, and steps: what to do at which share of it. `cheaper` — the next run of the
//! work asks the table for a cheaper model; `ask` — the person is asked «добавить ещё или остановить?» before the next
//! run; `stop` — the work ends «не доделано». The first version has three scopes: all works (the setting `budgets`,
//! each work on its own), a work folder (its field `budget`) and one work (`work_budgets`). The narrowest one holds:
//! a work's own over its folder's, a folder's over the general one. The default: five dollars a work, a cheaper model
//! at 80%, the question at 100%.
//!
//! A work's spending is what its deals came to since it began (`Deals::of_work`): the dollars of the priced ones —
//! an agent that names no price spends no dollars here, only tokens — and every token, fresh, out and the cache's.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::Tokens;
use crate::storage::Db;

/// What a budget counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Measure {
    #[default]
    Usd,
    Tokens,
}

/// What a step does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Act {
    Cheaper,
    Ask,
    Stop,
}

impl Act {
    pub fn as_str(self) -> &'static str {
        match self {
            Act::Cheaper => "cheaper",
            Act::Ask => "ask",
            Act::Stop => "stop",
        }
    }

    pub fn parse(text: &str) -> Option<Act> {
        [Act::Cheaper, Act::Ask, Act::Stop].into_iter().find(|act| act.as_str() == text)
    }
}

/// A step of a budget: at this share of the limit, do this.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub at: f64,
    #[serde(rename = "do")]
    pub act: Act,
}

/// A budget: its measure, its limit in it, its steps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    #[serde(default)]
    pub measure: Measure,
    pub limit: f64,
    #[serde(default = "default_steps")]
    pub steps: Vec<Step>,
}

/// The steps of the default: a cheaper model at 80%, the question at 100%.
pub fn default_steps() -> Vec<Step> {
    vec![Step { at: 0.8, act: Act::Cheaper }, Step { at: 1.0, act: Act::Ask }]
}

/// The most a limit may be: a million dollars, or a hundred billion tokens.
const LIMIT_MAX: f64 = 1e11;
/// A budget has at most this many steps.
const STEPS_MAX: usize = 8;

impl Budget {
    /// Dollars, with the default steps.
    pub fn usd(limit: f64) -> Budget {
        Budget { measure: Measure::Usd, limit, steps: default_steps() }
    }

    /// Whether it makes sense: a positive limit, steps at shares between 0 and 10, not too many. Why not, in words.
    pub fn check(&self) -> Result<(), String> {
        if !self.limit.is_finite() || self.limit <= 0.0 || self.limit > LIMIT_MAX {
            return Err(crate::tr!("Предел бюджета — число больше нуля."));
        }
        if self.steps.len() > STEPS_MAX {
            return Err(crate::tr!("Ступеней бюджета не больше {max}.", max = STEPS_MAX));
        }
        if self.steps.iter().any(|step| !step.at.is_finite() || step.at <= 0.0 || step.at > 10.0) {
            return Err(crate::tr!("Ступень бюджета — доля предела от 0 до 10 (0,8 — это 80%)."));
        }
        Ok(())
    }

    /// The steps in the order of their shares.
    fn ordered(&self) -> Vec<Step> {
        let mut steps = self.steps.clone();
        steps.sort_by(|a, b| a.at.total_cmp(&b.at));
        steps
    }

    /// How much of it is spent, in its measure.
    pub fn used(&self, spent: &Spending) -> f64 {
        match self.measure {
            Measure::Usd => spent.usd_micro as f64 / 1_000_000.0,
            Measure::Tokens => spent.tokens() as f64,
        }
    }

    /// The share of it spent.
    pub fn share(&self, spent: &Spending) -> f64 {
        self.used(spent) / self.limit
    }

    /// The step reached: the last whose share is spent. None — none yet.
    pub fn step(&self, spent: &Spending) -> Option<Step> {
        let share = self.share(spent);
        self.ordered().into_iter().rfind(|step| share >= step.at)
    }

    /// Where the person is asked or the work stops: the first `ask` or `stop` step, as an amount of the measure. None
    /// — the budget never stops a work.
    pub fn stop_at(&self) -> Option<f64> {
        self.ordered().into_iter().find(|step| step.act != Act::Cheaper).map(|step| step.at * self.limit)
    }

    /// What one more run may spend in dollars before the person is asked: what is left to the first `ask` or `stop`
    /// step. None — not in dollars, or nothing stops it. A run's ceiling (an agent's `--max-budget-usd`).
    pub fn ceiling_usd(&self, spent: &Spending) -> Option<f64> {
        if self.measure != Measure::Usd {
            return None;
        }
        self.stop_at().map(|at| (at - self.used(spent)).max(0.0))
    }

    /// The same again on top: «добавить ещё» — the limit grows by what it was.
    pub fn raised(&self, by: &Budget) -> Budget {
        Budget { limit: (self.limit + by.limit).min(LIMIT_MAX), ..self.clone() }
    }
}

/// The setting `budgets`: the general one, on every work. More scopes — a seat, a person, an app, a month — come in
/// block Г as new fields of the same setting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Budgets {
    #[serde(default = "default_works")]
    pub works: Budget,
}

impl Default for Budgets {
    fn default() -> Self {
        Budgets { works: default_works() }
    }
}

fn default_works() -> Budget {
    Budget::usd(5.0)
}

/// Which budget holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Works,
    Folder,
    Work,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Works => "works",
            Scope::Folder => "folder",
            Scope::Work => "work",
        }
    }
}

/// The narrowest budget there is: the work's own, else its folder's, else the general one.
pub fn narrowest(work: Option<&Budget>, folder: Option<&Budget>, general: &Budget) -> (Budget, Scope) {
    match (work, folder) {
        (Some(work), _) => (work.clone(), Scope::Work),
        (None, Some(folder)) => (folder.clone(), Scope::Folder),
        (None, None) => (general.clone(), Scope::Works),
    }
}

/// What a work spent so far, by its deals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Spending {
    pub usd_micro: u64,
    pub tokens: Tokens,
    /// Deals without a price: their dollars are not in `usd_micro`.
    pub unpriced: u64,
}

impl Spending {
    /// Every token: fresh, out, read from the cache and written to it.
    pub fn tokens(&self) -> u64 {
        let tokens = &self.tokens;
        tokens.input + tokens.output + tokens.cache_read + tokens.cache_write
    }
}

/// The budgets of single works (`work_budgets`, migration 0018): by the work's id and when it began — ids start over
/// once old works are gone, so a budget holds only for the work it was given to.
pub struct WorkBudgets {
    db: std::sync::Arc<Db>,
}

impl WorkBudgets {
    pub fn new(db: std::sync::Arc<Db>) -> WorkBudgets {
        WorkBudgets { db }
    }

    /// A work's own budget, if it has one.
    pub fn get(&self, work: &str, since: u64) -> Option<Budget> {
        let text: Option<String> = self
            .db
            .with(|c| {
                c.query_row(
                    "SELECT budget FROM work_budgets WHERE work = ?1 AND since = ?2",
                    params![work, since as i64],
                    |row| row.get(0),
                )
                .optional()
            })
            .unwrap_or_else(|error| {
                tracing::warn!(target: "router", "the budget of the work {work} not read: {error}");
                None
            });
        text.and_then(|text| serde_json::from_str(&text).ok())
    }

    /// A work's own budget given, or taken away (None).
    pub fn set(&self, work: &str, since: u64, budget: Option<&Budget>, now: u64) -> rusqlite::Result<()> {
        self.db.with(|c| {
            match budget {
                Some(budget) => {
                    let text = serde_json::to_string(budget).unwrap_or_default();
                    c.execute(
                        "INSERT INTO work_budgets (work, since, budget, at) VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT (work) DO UPDATE SET since = excluded.since, budget = excluded.budget,
                             at = excluded.at",
                        params![work, since as i64, text, now as i64],
                    )?;
                }
                None => {
                    c.execute("DELETE FROM work_budgets WHERE work = ?1", params![work])?;
                }
            }
            Ok(())
        })
    }

    /// The budgets given before `before` (ms): their works are gone. How many.
    pub fn prune(&self, before: u64) -> rusqlite::Result<usize> {
        self.db.with(|c| c.execute("DELETE FROM work_budgets WHERE at < ?1", params![before as i64]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usd(usd: f64) -> Spending {
        Spending { usd_micro: (usd * 1_000_000.0) as u64, ..Spending::default() }
    }

    #[test]
    fn the_step_reached_is_the_last_whose_share_is_spent() {
        let budget = Budget::usd(5.0);
        assert_eq!(budget.step(&usd(3.9)), None);
        assert_eq!(budget.step(&usd(4.0)).map(|step| step.act), Some(Act::Cheaper));
        assert_eq!(budget.step(&usd(5.0)).map(|step| step.act), Some(Act::Ask));
        assert_eq!(budget.step(&usd(9.0)).map(|step| step.act), Some(Act::Ask));
        // Steps given in any order.
        let mixed = Budget {
            measure: Measure::Usd,
            limit: 2.0,
            steps: vec![Step { at: 1.2, act: Act::Stop }, Step { at: 0.5, act: Act::Cheaper }],
        };
        assert_eq!(mixed.step(&usd(1.5)).map(|step| step.act), Some(Act::Cheaper));
        assert_eq!(mixed.step(&usd(2.4)).map(|step| step.act), Some(Act::Stop));
    }

    #[test]
    fn a_runs_ceiling_is_what_is_left_to_the_question() {
        let budget = Budget::usd(5.0);
        assert_eq!(budget.ceiling_usd(&usd(1.0)), Some(4.0));
        assert_eq!(budget.ceiling_usd(&usd(6.0)), Some(0.0));
        let tokens = Budget { measure: Measure::Tokens, ..Budget::usd(1000.0) };
        assert_eq!(tokens.ceiling_usd(&usd(1.0)), None);
        let never = Budget { steps: vec![Step { at: 0.8, act: Act::Cheaper }], ..Budget::usd(1.0) };
        assert_eq!(never.ceiling_usd(&usd(0.2)), None);
    }

    #[test]
    fn tokens_count_every_token() {
        let spent = Spending {
            tokens: Tokens { input: 10, output: 20, cache_read: 300, cache_write: 4 },
            ..Spending::default()
        };
        let budget = Budget { measure: Measure::Tokens, ..Budget::usd(1000.0) };
        assert_eq!(budget.used(&spent), 334.0);
    }

    #[test]
    fn the_narrowest_holds() {
        let (work, folder, general) = (Budget::usd(2.0), Budget::usd(3.0), Budget::usd(5.0));
        assert_eq!(narrowest(Some(&work), Some(&folder), &general), (work.clone(), Scope::Work));
        assert_eq!(narrowest(None, Some(&folder), &general), (folder, Scope::Folder));
        assert_eq!(narrowest(None, None, &general), (general, Scope::Works));
    }

    #[test]
    fn a_budget_reads_from_the_setting_and_says_what_is_wrong() {
        let read: Budgets = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(read, Budgets::default());
        let read: Budget = serde_json::from_value(serde_json::json!({"limit": 2})).unwrap();
        assert_eq!(read, Budget::usd(2.0));
        let read: Budget = serde_json::from_value(serde_json::json!({
            "measure": "tokens", "limit": 100000, "steps": [{"at": 1, "do": "stop"}]
        }))
        .unwrap();
        assert_eq!((read.measure, read.steps[0].act), (Measure::Tokens, Act::Stop));
        assert!(Budget::usd(0.0).check().is_err());
        assert!(Budget { steps: vec![Step { at: -1.0, act: Act::Ask }], ..Budget::usd(1.0) }.check().is_err());
        assert!(Budget::usd(1.0).check().is_ok());
        assert_eq!(Budget::usd(2.0).raised(&Budget::usd(5.0)).limit, 7.0);
    }

    #[test]
    fn a_works_budget_holds_only_for_the_work_it_was_given_to() {
        let budgets = WorkBudgets::new(std::sync::Arc::new(Db::memory()));
        budgets.set("7", 1000, Some(&Budget::usd(2.0)), 1000).unwrap();
        assert_eq!(budgets.get("7", 1000), Some(Budget::usd(2.0)));
        // A later work with the same id: not its budget.
        assert_eq!(budgets.get("7", 5000), None);
        budgets.set("7", 1000, None, 2000).unwrap();
        assert_eq!(budgets.get("7", 1000), None);
        budgets.set("8", 1000, Some(&Budget::usd(2.0)), 1000).unwrap();
        assert_eq!(budgets.prune(1500).unwrap(), 1);
    }

    /// The project's rule for a migration (CLAUDE.md, «Миграции БД в worktree»): 0018 on a copy of a real database,
    /// taken with `sqlite3 <db> ".backup '<copy>'"` into a folder of its own, never the live one. Opened by the same
    /// code as the gateway's start. `DUOWAVE_DB_COPY=<the copy's state folder> cargo test -p duowave-brain --lib
    /// router::budgets -- --ignored`.
    #[test]
    #[ignore = "on a copy of a live database: DUOWAVE_DB_COPY=<its state folder>"]
    fn the_budgets_migration_on_a_copy_of_a_live_database() {
        let Some(dir) = std::env::var_os("DUOWAVE_DB_COPY") else { return };
        let dir = std::path::PathBuf::from(dir);
        let db = std::sync::Arc::new(Db::open(Some(&dir)).expect("the copy opens and migrates"));
        let one = |sql: &str| -> i64 { db.with(|c| c.query_row(sql, [], |row| row.get(0))).unwrap() };
        let columns = |table: &str| -> Vec<String> {
            db.with(|c| {
                c.prepare(&format!("PRAGMA table_info({table})"))?
                    .query_map([], |row| row.get::<_, String>(1))?
                    .collect()
            })
            .unwrap()
        };
        assert!(one("SELECT MAX(version) FROM schema_migrations") >= 18, "0018 applied");
        assert_eq!(columns("work_budgets"), ["work", "since", "budget", "at"]);
        assert!(columns("folder_works").contains(&"session_model".to_string()));
        // The data before is there and reads.
        let (settings, users) = (one("SELECT COUNT(*) FROM settings"), one("SELECT COUNT(*) FROM users"));
        assert!(settings > 0 && users > 0, "settings {settings}, people {users}");
        let works = one("SELECT COUNT(*) FROM folder_works");
        // A work's budget is kept, read and taken away on it, and leaves nothing behind.
        let budgets = WorkBudgets::new(db.clone());
        budgets.set("copy-check", 1, Some(&Budget::usd(2.0)), 2).unwrap();
        assert_eq!(budgets.get("copy-check", 1), Some(Budget::usd(2.0)));
        assert_eq!(budgets.get("copy-check", 3), None, "another work by the same id");
        budgets.set("copy-check", 1, None, 4).unwrap();
        assert_eq!(one("SELECT COUNT(*) FROM work_budgets WHERE work = 'copy-check'"), 0);
        eprintln!("copy: 0018 applied; settings {settings}, people {users}, works in folders {works}; budgets work");
    }
}
