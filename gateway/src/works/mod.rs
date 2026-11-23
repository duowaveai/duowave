//! The works in folders, the helper's heavy part (docs/plans/router-and-works.md, block A, 6–9): the work folders, the
//! socket of agents (Claude Code built in), the work copies, the run of a work and its journal. Only in the
//! computer's build. The rest of the gateway sees it through the port `WorksPort` (talk/folder.rs): until stage 5 the
//! port is this module itself, in the same program.
//!
//! A work: a copy of the folder on its own branch → the folder's preparation → the agent's run with the task → a
//! commit if the agent left changes → the branch into the folder → the folder's checks → its end. Notes said on the
//! way wait for the run's end and go to the agent in the same session, a new run; so does a continuation after a
//! restart. A plan's limit is waited out (the person may stop it). One work in folders at a time — the talk layer's
//! queue (decision 16). The gateway itself merges nothing into the person's branches and pushes nothing; the agent
//! pushes only when the folder's allowed commands list `git push` (decision 21).

pub mod agents;
pub mod folders;
pub mod git;
pub mod harness;
pub mod routes;
mod runner;
pub mod runs;
pub mod workspace;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::core::events::{EventBus, GatewayEvent};
use crate::secrets::SecretsService;
use crate::settings::SettingsService;
use crate::storage::Db;
use crate::talk::folder::{
    FolderCheck, FolderDoing, FolderStart, FolderTold, FolderWhy, PortEvent, PortListener, WorksPort,
};
use crate::utils::time::now_ms;
use harness::claude_code::{ClaudeCode, CLAUDE_CODE};
use harness::Harness;
use runs::What;
use workspace::{GitWorkspace, Workspace};

crate::route_needs!(
    Arc<crate::settings::SettingsService>,
    Option<Arc<FolderWorks>>,
    Arc<crate::talk::TalkLayer>,
    Arc<crate::secrets::SecretsService>
);

/// A work's record and its journals are kept this long after its end, as the talk's record.
pub const KEPT_MS: u64 = 30 * 24 * 60 * 60_000;
/// A work copy is kept this long after the work's last run: «вдогонку» and «продолжай» go on in it; later the copy is
/// made again from the work's branch in the folder.
pub const COPY_KEPT_MS: u64 = 7 * 24 * 60 * 60_000;
/// How often the old works, journals and copies are cleaned.
const CLEAN_EVERY: Duration = Duration::from_secs(6 * 3600);

/// Where a work in a folder is, as the helper keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Working,
    Waiting,
    Ready,
    Unfinished,
    Failed,
    Stopped,
    Interrupted,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Working => "working",
            State::Waiting => "waiting",
            State::Ready => "ready",
            State::Unfinished => "unfinished",
            State::Failed => "failed",
            State::Stopped => "stopped",
            State::Interrupted => "interrupted",
        }
    }

    fn parse(text: &str) -> State {
        match text {
            "working" | "queued" => State::Working,
            "waiting" => State::Waiting,
            "ready" => State::Ready,
            "unfinished" => State::Unfinished,
            "failed" => State::Failed,
            "stopped" => State::Stopped,
            _ => State::Interrupted,
        }
    }
}

/// A row of `folder_works`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: String,
    pub folder: String,
    pub folder_name: String,
    pub agent: String,
    pub model: Option<String>,
    pub title: String,
    pub task: String,
    pub branch: String,
    pub base: Option<String>,
    pub base_commit: Option<String>,
    pub copy: Option<String>,
    pub billing: Option<String>,
    pub session: Option<String>,
    pub state: State,
    pub why: Option<String>,
    pub wait_until: Option<u64>,
    pub last: Option<String>,
    pub checks: Option<Vec<FolderCheck>>,
    pub checks_skipped: bool,
    pub committed: bool,
    pub fetched: bool,
    pub changed: bool,
    pub error: Option<String>,
    pub usd_micro: Option<u64>,
    pub runs: u32,
    pub created_at: u64,
    pub updated_at: u64,
    pub ended_at: Option<u64>,
}

/// What `checks` keeps as JSON: the checks, whether they were skipped, the changes and the error of handing over.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Kept {
    #[serde(default)]
    checks: Option<Vec<KeptCheck>>,
    #[serde(default)]
    skipped: bool,
    #[serde(default)]
    changed: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeptCheck {
    command: String,
    ok: bool,
    code: Option<i32>,
    ms: u64,
    tail: String,
}

const COLUMNS: &str = "id, folder, folder_name, agent, model, title, task, branch, base, base_commit, copy, billing, \
     session, state, why, wait_until, last, checks, committed, fetched, usd_micro, runs, created_at, updated_at, ended_at";

fn record_of(row: &Row<'_>) -> rusqlite::Result<Record> {
    let kept: Kept =
        row.get::<_, Option<String>>(17)?.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    let maybe = |value: Option<i64>| value.map(|value| value.max(0) as u64);
    Ok(Record {
        id: row.get(0)?,
        folder: row.get(1)?,
        folder_name: row.get(2)?,
        agent: row.get(3)?,
        model: row.get(4)?,
        title: row.get(5)?,
        task: row.get(6)?,
        branch: row.get(7)?,
        base: row.get(8)?,
        base_commit: row.get(9)?,
        copy: row.get(10)?,
        billing: row.get(11)?,
        session: row.get(12)?,
        state: State::parse(&row.get::<_, String>(13)?),
        why: row.get(14)?,
        wait_until: maybe(row.get(15)?),
        last: row.get(16)?,
        checks: kept.checks.map(|checks| {
            checks
                .into_iter()
                .map(|check| FolderCheck {
                    command: check.command,
                    ok: check.ok,
                    code: check.code,
                    ms: check.ms,
                    tail: check.tail,
                })
                .collect()
        }),
        checks_skipped: kept.skipped,
        committed: row.get(18)?,
        fetched: row.get(19)?,
        changed: kept.changed,
        error: kept.error,
        usd_micro: maybe(row.get(20)?),
        runs: row.get(21)?,
        created_at: row.get::<_, i64>(22)?.max(0) as u64,
        updated_at: row.get::<_, i64>(23)?.max(0) as u64,
        ended_at: maybe(row.get(24)?),
    })
}

fn save(db: &Db, record: &Record) {
    let kept = Kept {
        checks: record.checks.as_ref().map(|checks| {
            checks
                .iter()
                .map(|check| KeptCheck {
                    command: check.command.clone(),
                    ok: check.ok,
                    code: check.code,
                    ms: check.ms,
                    tail: check.tail.clone(),
                })
                .collect()
        }),
        skipped: record.checks_skipped,
        changed: record.changed,
        error: record.error.clone(),
    };
    let done = db.with(|c| {
        c.execute(
            &format!(
                "INSERT INTO folder_works ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                     ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)
                 ON CONFLICT (id) DO UPDATE SET folder_name = excluded.folder_name, model = excluded.model,
                     branch = excluded.branch, base = excluded.base, base_commit = excluded.base_commit,
                     copy = excluded.copy, billing = excluded.billing, session = excluded.session, state = excluded.state,
                     why = excluded.why, wait_until = excluded.wait_until, last = excluded.last, checks = excluded.checks,
                     committed = excluded.committed, fetched = excluded.fetched, usd_micro = excluded.usd_micro,
                     runs = excluded.runs, updated_at = excluded.updated_at, ended_at = excluded.ended_at"
            ),
            params![
                record.id,
                record.folder,
                record.folder_name,
                record.agent,
                record.model,
                record.title,
                record.task,
                record.branch,
                record.base,
                record.base_commit,
                record.copy,
                record.billing,
                record.session,
                record.state.as_str(),
                record.why,
                record.wait_until.map(|at| at as i64),
                record.last,
                serde_json::to_string(&kept).ok(),
                record.committed,
                record.fetched,
                record.usd_micro.map(|usd| usd as i64),
                record.runs,
                record.created_at as i64,
                record.updated_at as i64,
                record.ended_at.map(|at| at as i64),
            ],
        )
    });
    if let Err(error) = done {
        tracing::warn!(target: "works", "the work {} is not kept: {error}", record.id);
    }
}

/// A work with a job going: its stop, the notes waiting for the run's end, what it is at.
pub(crate) struct Going {
    pub(crate) stop: CancellationToken,
    pub(crate) notes: Mutex<Vec<String>>,
    pub(crate) doing: Mutex<Option<FolderDoing>>,
    /// When the panel last heard a step of it: at most once a second.
    told_at: AtomicU64,
}

impl Going {
    fn new() -> Arc<Going> {
        Arc::new(Going {
            stop: CancellationToken::new(),
            notes: Mutex::new(Vec::new()),
            doing: Mutex::new(Some(FolderDoing::Begins)),
            told_at: AtomicU64::new(0),
        })
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct FolderWorks {
    me: Weak<FolderWorks>,
    pub(crate) db: Arc<Db>,
    /// `<data>/works`: a folder per work — its copy and its runs' journals.
    pub(crate) dir: PathBuf,
    pub(crate) settings: Arc<SettingsService>,
    pub(crate) claude: Arc<ClaudeCode>,
    /// The agents of the cards from MagicPlace (stage 3б).
    pub agents: agents::Agents,
    pub(crate) workspace: Arc<dyn Workspace>,
    /// Where the gateway keeps its own things: never a work folder.
    pub(crate) guarded: Vec<PathBuf>,
    events: EventBus,
    going: Mutex<HashMap<String, Arc<Going>>>,
    listener: RwLock<Option<PortListener>>,
    cancel: CancellationToken,
    /// The gateway is closing: what its stop cuts is interrupted, not stopped — «Продолжить» goes on in the session.
    closing: std::sync::atomic::AtomicBool,
}

impl FolderWorks {
    /// The works kept in the table: what went on when the gateway stopped is interrupted now — «продолжай» goes on
    /// in the same session.
    pub fn new(
        db: Arc<Db>,
        dir: PathBuf,
        guarded: Vec<PathBuf>,
        settings: Arc<SettingsService>,
        secrets: Arc<SecretsService>,
        events: EventBus,
    ) -> Arc<FolderWorks> {
        let cut = db
            .with(|c| {
                c.execute(
                    "UPDATE folder_works SET state = 'interrupted', ended_at = ?1, updated_at = ?1
                     WHERE state IN ('working', 'waiting', 'queued')",
                    params![now_ms() as i64],
                )
            })
            .unwrap_or(0);
        let runs = runs::interrupt_all(&db);
        if cut + runs > 0 {
            tracing::info!(target: "works", "{cut} work(s) in folders and {runs} run(s) were going on when the gateway stopped: interrupted");
        }
        let claude = Arc::new(ClaudeCode::new(settings.clone(), secrets));
        let agents = agents::Agents::new(settings.clone());
        Arc::new_cyclic(|me| FolderWorks {
            me: me.clone(),
            db,
            dir,
            settings,
            claude,
            agents,
            workspace: Arc::new(GitWorkspace),
            guarded,
            events,
            going: Mutex::new(HashMap::new()),
            listener: RwLock::new(None),
            cancel: CancellationToken::new(),
            closing: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// The cleaning: now, then every six hours — works older than a month with their journals; copies a week after
    /// their last run.
    pub fn start_cleaning(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(CLEAN_EVERY);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = ticker.tick() => match me.upgrade() {
                        Some(works) => works.clean(now_ms()).await,
                        None => break,
                    },
                }
            }
        });
    }

    /// Stops every job: its agent ends its turn and what it did is handed over (the gateway is closing). The works are
    /// interrupted, as a restart leaves them; the talk's records learn it at the next start.
    pub fn stop_all(&self) {
        self.closing.store(true, Ordering::Relaxed);
        self.cancel.cancel();
        for going in locked(&self.going).values() {
            going.stop.cancel();
        }
    }

    pub(crate) fn closing(&self) -> bool {
        self.closing.load(Ordering::Relaxed)
    }

    pub(crate) fn tell(&self, event: PortEvent) {
        let listener = self.listener.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        if let Some(listener) = listener {
            listener(event);
        }
    }

    /// The panel hears a run change: begun, a step (at most once a second), ended.
    pub(crate) fn told(&self, id: &str, run: u32, change: &str) {
        if change == "step" {
            let Some(going) = locked(&self.going).get(id).cloned() else { return };
            let now = now_ms();
            let last = going.told_at.load(Ordering::Relaxed);
            if now.saturating_sub(last) < 1000 {
                return;
            }
            going.told_at.store(now, Ordering::Relaxed);
        }
        self.events.emit(GatewayEvent::WorksRun { work: id.to_string(), run, change: change.to_string() });
    }

    /// An agent by its id: Claude Code, built in, or one of a card from MagicPlace.
    pub fn harness(&self, agent: &str) -> Option<Arc<dyn Harness>> {
        match agent {
            CLAUDE_CODE => Some(self.claude.clone() as Arc<dyn Harness>),
            other => self.agents.get(other).map(|agent| agent as Arc<dyn Harness>),
        }
    }

    /// Every agent there is: Claude Code first, then the cards.
    pub fn harnesses(&self) -> Vec<Arc<dyn Harness>> {
        let mut all = vec![self.claude.clone() as Arc<dyn Harness>];
        all.extend(self.agents.list().into_iter().map(|agent| agent as Arc<dyn Harness>));
        all
    }

    pub fn record(&self, id: &str) -> Option<Record> {
        self.db
            .with(|c| {
                c.query_row(&format!("SELECT {COLUMNS} FROM folder_works WHERE id = ?1"), params![id], record_of)
                    .optional()
            })
            .ok()
            .flatten()
    }

    pub(crate) fn keep(&self, record: &mut Record) {
        record.updated_at = now_ms();
        save(&self.db, record);
    }

    /// The branches the other works of a folder took.
    async fn taken_branches(&self, folder: &str, but: &str) -> Vec<String> {
        self.db
            .with(|c| {
                let mut statement = c.prepare("SELECT branch FROM folder_works WHERE folder = ?1 AND id != ?2")?;
                let rows = statement.query_map(params![folder, but], |row| row.get::<_, String>(0))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default()
    }

    /// The work's folder: `<data>/works/<id>`.
    pub fn folder_of(&self, id: &str) -> PathBuf {
        runs::folder_of(&self.dir, id)
    }

    pub fn copy_of(&self, id: &str) -> PathBuf {
        self.folder_of(id).join("copy")
    }

    fn going_of(&self, id: &str) -> Option<Arc<Going>> {
        locked(&self.going).get(id).cloned()
    }

    /// A job for a work: its first prompt and what it is; the job goes on its own task.
    fn spawn(&self, id: &str, first: String, what: What, resume: String) -> Result<(), String> {
        let me = self.me.upgrade().ok_or("the helper is closing")?;
        let handle = tokio::runtime::Handle::try_current().map_err(|_| "no runtime for a work")?;
        let going = Going::new();
        {
            let mut all = locked(&self.going);
            if all.contains_key(id) {
                return Err("the work goes on already".into());
            }
            all.insert(id.to_string(), going.clone());
        }
        let id = id.to_string();
        handle.spawn(async move {
            me.clone().job(&id, first, what, resume, going).await;
            locked(&me.going).remove(&id);
        });
        Ok(())
    }

    /// Old works with their journals; copies a week after their last run.
    async fn clean(&self, now: u64) {
        let old: Vec<(String, Option<i64>, Option<String>)> = self
            .db
            .with(|c| {
                let mut statement = c.prepare(
                    "SELECT id, ended_at, copy FROM folder_works WHERE ended_at IS NOT NULL AND state NOT IN ('working', 'waiting')",
                )?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default();
        for (id, ended, copy) in old {
            if self.going_of(&id).is_some() {
                continue;
            }
            let ended = ended.unwrap_or(0).max(0) as u64;
            if now.saturating_sub(ended) > KEPT_MS {
                let _ = tokio::fs::remove_dir_all(self.folder_of(&id)).await;
                let _ = self.db.with(|c| {
                    c.execute("DELETE FROM work_runs WHERE work = ?1", params![id])?;
                    c.execute("DELETE FROM folder_works WHERE id = ?1", params![id])
                });
            } else if copy.is_some() && now.saturating_sub(ended) > COPY_KEPT_MS {
                let _ = tokio::fs::remove_dir_all(self.copy_of(&id)).await;
                if let Some(mut record) = self.record(&id) {
                    record.copy = None;
                    self.keep(&mut record);
                }
            }
        }
    }

    /// A fresh id: `w` and eight hex digits.
    fn new_id(&self) -> String {
        loop {
            let id = format!("w{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
            if self.record(&id).is_none() {
                return id;
            }
        }
    }

    /// What the record tells of its end.
    fn told_of(&self, record: &Record) -> FolderTold {
        let agent = self.harness(&record.agent).map(|harness| harness.title()).unwrap_or_else(|| record.agent.clone());
        FolderTold {
            folder: record.folder_name.clone(),
            agent,
            branch: record.branch.clone(),
            fetched: record.fetched,
            changed: record.changed,
            checks: record.checks.clone().unwrap_or_default(),
            checks_skipped: record.checks_skipped,
            usd_micro: record.usd_micro,
            billing: record.billing.clone(),
            last: record.last.clone(),
            why: record.why.as_deref().and_then(FolderWhy::parse),
            error: record.error.clone(),
        }
    }
}

impl WorksPort for FolderWorks {
    fn folder_name(&self, folder: &str) -> Option<String> {
        folders::get(&self.settings, folder).map(|folder| folder.name)
    }

    fn start(&self, start: FolderStart) -> Result<String, String> {
        let folder = folders::get(&self.settings, &start.folder).ok_or("no such work folder")?;
        let agent = start
            .agent
            .clone()
            .or(folder.agent.clone())
            .filter(|agent| !agent.trim().is_empty())
            .unwrap_or_else(|| CLAUDE_CODE.to_string());
        if self.harness(&agent).is_none() {
            return Err(format!("no agent {agent}"));
        }
        let model = start.model.clone().or(folder.model.clone()).filter(|model| !model.trim().is_empty());
        let now = now_ms();
        let mut record = Record {
            id: self.new_id(),
            folder: folder.id.clone(),
            folder_name: folder.name.clone(),
            agent,
            model,
            title: start.title.clone(),
            task: start.task.clone(),
            branch: format!("duowave/{}", git::slug(&start.title)),
            base: None,
            base_commit: None,
            copy: None,
            billing: None,
            session: None,
            state: State::Working,
            why: None,
            wait_until: None,
            last: None,
            checks: None,
            checks_skipped: false,
            committed: false,
            fetched: false,
            changed: false,
            error: None,
            usd_micro: None,
            runs: 0,
            created_at: now,
            updated_at: now,
            ended_at: None,
        };
        self.keep(&mut record);
        self.spawn(&record.id, start.task, What::Work, start.resume)?;
        tracing::info!(target: "works", "work {} began in the folder {} with {}", record.id, folder.id, record.agent);
        Ok(record.id)
    }

    fn add(&self, id: &str, text: &str) -> bool {
        let Some(going) = self.going_of(id) else { return false };
        locked(&going.notes).push(text.trim().to_string());
        true
    }

    fn can_again(&self, id: &str) -> Result<String, String> {
        let record = self.record(id).ok_or("no such work")?;
        folders::get(&self.settings, &record.folder).ok_or("its folder is gone")?;
        Ok(record.title)
    }

    fn again(&self, id: &str, text: &str, resume: &str) -> Result<(), String> {
        if self.add(id, text) {
            return Ok(());
        }
        let mut record = self.record(id).ok_or("no such work")?;
        folders::get(&self.settings, &record.folder).ok_or("its folder is gone")?;
        // After a cut — a restart, a stop, a limit — it is a continuation; otherwise words added after the end.
        let cut = matches!(record.state, State::Interrupted | State::Stopped | State::Waiting) || text == resume;
        let what = if cut { What::Resume } else { What::Note };
        record.state = State::Working;
        record.why = None;
        record.error = None;
        record.wait_until = None;
        record.ended_at = None;
        self.keep(&mut record);
        self.spawn(id, text.to_string(), what, resume.to_string())
    }

    fn doing(&self, id: &str) -> Option<FolderDoing> {
        self.going_of(id).and_then(|going| locked(&going.doing).clone())
    }

    fn stop(&self, id: &str) -> bool {
        match self.going_of(id) {
            Some(going) => {
                going.stop.cancel();
                true
            }
            None => false,
        }
    }

    fn told(&self, id: &str) -> Option<FolderTold> {
        self.record(id).map(|record| self.told_of(&record))
    }

    fn dealt(&self, id: &str, n: u32, deal: i64) {
        runs::dealt(&self.db, id, n, deal);
    }

    fn listen(&self, listener: PortListener) {
        *self.listener.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(listener);
    }
}

/// The home folder of this computer, for the folders' checks.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

#[cfg(test)]
mod tests;
