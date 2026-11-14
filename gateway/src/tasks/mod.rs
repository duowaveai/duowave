//! Long tasks of apps (docs/plans/feed.md, stage 1; docs/plans/big-apps.md, point 2). A call of an app waits for its
//! answer two seconds, thirty in the background (plugins/runtime.rs); work that takes minutes — a series filmed, a
//! copy made for sending, an archive read — is a task instead: the app starts it, reports how far it got in percent
//! and in words, and ends it with a result or an error. The panel follows it on the live channel (`tasks.changed`)
//! and may ask to cancel it; the app learns of that with its next report, or by waiting for it.
//!
//! What is kept and for how long:
//!
//! - an app runs one task at a time unless its manifest asks for more (`budget.tasks`, up to four);
//! - a task that ended is kept a day, the last twenty of an app, then forgotten with its result;
//! - a task the app was asked to cancel and did not end in thirty seconds is closed as cancelled by the gateway;
//! - a task whose app said nothing for half an hour, whose app fell or was switched off, is closed as interrupted;
//! - the gateway restarted: what ran is marked interrupted at the start. Nothing restores a task — the app decides
//!   whether to begin again;
//! - a task never travels: its table is not in the settings package, a move to another computer leaves it behind.
//!
//! Who uses it: an app's program through its Host API (`/api/v1/apps/host/tasks…`, apps/host_http.rs), the panel
//! through `/api/v1/tasks` (routes.rs), an app's window through `useTask` (panel/src/frame).

pub mod routes;
mod store;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::core::events::{EventBus, GatewayEvent};
use crate::storage::Db;
use crate::utils::time::now_ms;

/// Tasks an app runs at once unless its manifest says otherwise (`budget.tasks`), and the most it may ask for.
pub const DEFAULT_AT_ONCE: u32 = 1;
pub const MAX_AT_ONCE: u32 = 4;

/// What a task keeps of its words.
const TITLE: usize = 120;
const TEXT: usize = 200;
const ERROR: usize = 500;
/// A result is an answer, not a file: what is bigger goes to the app's folder, and the result names it.
pub const RESULT_BYTES: usize = 64 * 1024;
/// Steps kept per task.
const LOG_KEPT: usize = 100;
/// The panel is told of a task's progress at most this often; how it started and ended — at once.
const TOLD_EVERY_MS: u64 = 500;
/// A running task's row is written at most this often.
const SAVED_EVERY_MS: u64 = 5_000;
/// How often the limits below are looked at.
const WATCH_EVERY: Duration = Duration::from_secs(5);

fn restarted() -> String {
    crate::tr!("Шлюз перезапустился, пока задача шла.")
}
fn forgotten() -> String {
    crate::tr!("Приложение удалено.")
}

/// The limits in time and number; the tests set their own.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// A task that ended is kept this long.
    pub result_kept_ms: u64,
    /// Ended tasks kept per app.
    pub kept_per_app: usize,
    /// After «Отменить» the app has this long to end the task itself.
    pub cancel_grace_ms: u64,
    /// A running task whose app said nothing this long is closed as interrupted.
    pub silence_ms: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            result_kept_ms: 24 * 60 * 60 * 1000,
            kept_per_app: 20,
            cancel_grace_ms: 30_000,
            silence_ms: 30 * 60 * 1000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskState {
    Running,
    Done,
    Failed,
    /// Stopped on a person's word.
    Cancelled,
    /// Cut short by something else: a restart of the gateway, the app's fall, its silence.
    Interrupted,
}

impl TaskState {
    pub fn name(self) -> &'static str {
        match self {
            TaskState::Running => "running",
            TaskState::Done => "done",
            TaskState::Failed => "failed",
            TaskState::Cancelled => "cancelled",
            TaskState::Interrupted => "interrupted",
        }
    }

    pub fn parse(name: &str) -> Option<TaskState> {
        [TaskState::Running, TaskState::Done, TaskState::Failed, TaskState::Cancelled, TaskState::Interrupted]
            .into_iter()
            .find(|state| state.name() == name)
    }

    /// In words, for a refusal.
    fn words(self) -> String {
        match self {
            TaskState::Running => crate::tr!("идёт"),
            TaskState::Done => crate::tr!("готова"),
            TaskState::Failed => crate::tr!("не получилась"),
            TaskState::Cancelled => crate::tr!("отменена"),
            TaskState::Interrupted => crate::tr!("прервалась"),
        }
    }
}

/// A line of a task's journal: what it was at, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    /// Milliseconds since the epoch.
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    /// Whose it is: the app's id.
    pub app: String,
    /// What it does, in words for people: «Снимаю серию 3».
    pub title: String,
    pub state: TaskState,
    /// How far it got, 0–100; none while the app cannot tell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    /// What it is at now: «Озвучка».
    pub text: String,
    /// What a task that is done gave.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Why it failed or was interrupted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// «Отменить» was pressed and the app has not ended the task yet.
    pub cancel_requested: bool,
    /// Milliseconds since the epoch.
    pub started_at: u64,
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
}

/// A task with its journal.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskLog {
    #[serde(flatten)]
    pub task: Task,
    pub log: Vec<Step>,
}

/// How the app ends its task.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Done(Option<Value>),
    Failed(String),
    /// It stopped as asked.
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskError {
    TooMany {
        title: String,
        limit: u32,
    },
    NotFound,
    /// A report on a task that is over: the app learns how it ended.
    Ended(TaskState),
    Invalid(String),
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskError::TooMany { title, limit } => out.write_str(&crate::tr!(
                "У приложения уже идёт задача «{title}»: сразу — не больше {limit}.",
                title = title,
                limit = limit
            )),
            TaskError::NotFound => out.write_str(&crate::tr!("Нет такой задачи.")),
            TaskError::Ended(state) => {
                out.write_str(&crate::tr!("Задача уже закончена: {state}.", state = state.words()))
            }
            TaskError::Invalid(text) => out.write_str(text),
        }
    }
}

impl std::error::Error for TaskError {}

struct Entry {
    task: Task,
    log: VecDeque<Step>,
    /// Cancelled when a person asks to cancel the task and when it ends: who waits for either wakes.
    cancel: CancellationToken,
    cancel_asked_at: Option<u64>,
    saved_at: u64,
    told_at: u64,
    /// Its progress changed since the panel was last told, and a word of that is on its way.
    tell_pending: bool,
}

impl Entry {
    fn of(task: Task, log: VecDeque<Step>, now: u64) -> Entry {
        let cancel = CancellationToken::new();
        if task.state != TaskState::Running {
            cancel.cancel();
        }
        Entry { task, log, cancel, cancel_asked_at: None, saved_at: now, told_at: 0, tell_pending: false }
    }

    fn note(&mut self, at: u64, percent: Option<u8>, text: &str) {
        match self.log.back_mut() {
            // The same step going on: its line moves, no new one.
            Some(last) if last.text == text => {
                last.at = at;
                last.percent = percent;
            }
            _ => {
                self.log.push_back(Step { at, percent, text: text.to_string() });
                while self.log.len() > LOG_KEPT {
                    self.log.pop_front();
                }
            }
        }
    }

    /// Ends the task: its state, what it gave or why not, the last line of its journal.
    fn close(&mut self, state: TaskState, result: Option<Value>, error: Option<String>, now: u64) {
        let line = match state {
            TaskState::Done => crate::tr!("Готово"),
            TaskState::Cancelled => crate::tr!("Отменена"),
            _ => error.clone().unwrap_or_else(|| state.words()),
        };
        if state == TaskState::Done {
            self.task.percent = Some(100);
        }
        self.task.state = state;
        self.task.result = result;
        self.task.error = error;
        self.task.cancel_requested = false;
        self.task.updated_at = now;
        self.task.ended_at = Some(now);
        self.cancel_asked_at = None;
        self.tell_pending = false;
        self.note(now, self.task.percent, &line);
        self.cancel.cancel();
    }
}

fn cut(text: &str, chars: usize) -> String {
    text.trim().chars().take(chars).collect()
}

/// A percent as the app gave it: any number, kept within 0–100.
fn percent_of(value: f64) -> Option<u8> {
    value.is_finite().then(|| value.clamp(0.0, 100.0).round() as u8)
}

pub struct Tasks {
    db: Arc<Db>,
    events: EventBus,
    limits: Limits,
    entries: Mutex<HashMap<String, Entry>>,
    me: Weak<Tasks>,
}

impl Tasks {
    /// The tasks as the database has them; what ran when the gateway stopped is marked interrupted.
    pub fn new(db: Arc<Db>, events: EventBus) -> Arc<Tasks> {
        Tasks::with_limits(db, events, Limits::default())
    }

    pub fn with_limits(db: Arc<Db>, events: EventBus, limits: Limits) -> Arc<Tasks> {
        let now = now_ms();
        let mut entries = HashMap::new();
        match store::load(&db) {
            Ok(stored) => {
                for store::Stored { task, log } in stored {
                    let mut entry = Entry::of(task, log.into(), now);
                    if entry.task.state == TaskState::Running {
                        entry.close(TaskState::Interrupted, None, Some(restarted()), now);
                        save(&db, &mut entry, now);
                        tracing::info!(target: "tasks", "{}: «{}» was cut short by the restart", entry.task.app, entry.task.title);
                    }
                    entries.insert(entry.task.id.clone(), entry);
                }
            }
            Err(error) => tracing::warn!(target: "tasks", "the tasks were not read: {error}"),
        }
        let tasks = Arc::new_cyclic(|me| Tasks { db, events, limits, entries: Mutex::new(entries), me: me.clone() });
        tasks.sweep_at(now);
        tasks
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn tell(&self, app: &str, task: &str, change: &str) {
        self.events.emit(GatewayEvent::TasksChanged { app: app.into(), task: task.into(), change: change.into() });
    }

    /// The limits looked at now and then, until the gateway stops. Inside the runtime.
    pub fn watch(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let me = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(WATCH_EVERY);
            loop {
                ticks.tick().await;
                let Some(tasks) = me.upgrade() else { return };
                tasks.sweep_at(now_ms());
            }
        })
    }

    /// An app starts a task: `at_once` is how many it may run at the same time (its manifest's `budget.tasks`).
    pub fn start(
        &self,
        app: &str,
        title: &str,
        text: Option<&str>,
        percent: Option<f64>,
        at_once: u32,
    ) -> Result<Task, TaskError> {
        let title = cut(title, TITLE);
        if title.is_empty() {
            return Err(TaskError::Invalid(crate::tr!(
                "У задачи нет названия (title): что она делает, словами для человека."
            )));
        }
        let limit = at_once.clamp(1, MAX_AT_ONCE);
        let now = now_ms();
        let task = {
            let mut entries = self.entries();
            let mut running =
                entries.values().filter(|entry| entry.task.app == app && entry.task.state == TaskState::Running);
            if let Some(first) = running.next() {
                if running.count() + 1 >= limit as usize {
                    return Err(TaskError::TooMany { title: first.task.title.clone(), limit });
                }
            }
            let task = Task {
                id: uuid::Uuid::new_v4().simple().to_string(),
                app: app.to_string(),
                title,
                state: TaskState::Running,
                percent: percent.and_then(percent_of),
                text: text.map(|text| cut(text, TEXT)).unwrap_or_default(),
                result: None,
                error: None,
                cancel_requested: false,
                started_at: now,
                updated_at: now,
                ended_at: None,
            };
            let mut entry = Entry::of(task.clone(), VecDeque::new(), now);
            let line = if task.text.is_empty() { task.title.clone() } else { task.text.clone() };
            entry.note(now, task.percent, &line);
            entry.told_at = now;
            save(&self.db, &mut entry, now);
            entries.insert(task.id.clone(), entry);
            task
        };
        self.sweep_at(now);
        self.tell(app, &task.id, "started");
        Ok(task)
    }

    /// The app says how far its task got; the answer — whether a person asked to cancel it.
    pub fn progress(&self, app: &str, id: &str, percent: Option<f64>, text: Option<&str>) -> Result<bool, TaskError> {
        let now = now_ms();
        let (asked, tell_now, tell_in) = {
            let mut entries = self.entries();
            let entry = entries.get_mut(id).filter(|entry| entry.task.app == app).ok_or(TaskError::NotFound)?;
            if entry.task.state != TaskState::Running {
                return Err(TaskError::Ended(entry.task.state));
            }
            if let Some(percent) = percent.and_then(percent_of) {
                entry.task.percent = Some(percent);
            }
            if let Some(text) = text {
                entry.task.text = cut(text, TEXT);
            }
            entry.task.updated_at = now;
            if !entry.task.text.is_empty() {
                let (percent, text) = (entry.task.percent, entry.task.text.clone());
                entry.note(now, percent, &text);
            }
            if now.saturating_sub(entry.saved_at) >= SAVED_EVERY_MS {
                save(&self.db, entry, now);
            }
            // The panel reads the tasks again on each word: a burst of reports is one word, the last one told too.
            let since = now.saturating_sub(entry.told_at);
            let (tell_now, tell_in) = if since >= TOLD_EVERY_MS {
                entry.told_at = now;
                (true, None)
            } else if entry.tell_pending {
                (false, None)
            } else {
                entry.tell_pending = true;
                (false, Some(TOLD_EVERY_MS - since))
            };
            (entry.task.cancel_requested, tell_now, tell_in)
        };
        if tell_now {
            self.tell(app, id, "progress");
        }
        if let Some(wait) = tell_in {
            self.tell_later(app, id, wait);
        }
        Ok(asked)
    }

    /// The last report of a burst, told once the pause between two words is over.
    fn tell_later(&self, app: &str, id: &str, wait_ms: u64) {
        let (me, app, id) = (self.me.clone(), app.to_string(), id.to_string());
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            // No runtime to wait in: told at once.
            self.told_late(&app, &id);
            return;
        };
        runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(wait_ms)).await;
            if let Some(tasks) = me.upgrade() {
                tasks.told_late(&app, &id);
            }
        });
    }

    fn told_late(&self, app: &str, id: &str) {
        let pending = {
            let mut entries = self.entries();
            match entries.get_mut(id) {
                Some(entry) if entry.tell_pending => {
                    entry.tell_pending = false;
                    entry.told_at = now_ms();
                    true
                }
                _ => false,
            }
        };
        if pending {
            self.tell(app, id, "progress");
        }
    }

    /// The app ends its task: done with what it gave, failed with why, or stopped as asked.
    pub fn finish(&self, app: &str, id: &str, outcome: Outcome) -> Result<Task, TaskError> {
        let (state, result, error) = match outcome {
            Outcome::Done(result) => {
                let result = result.filter(|value| !value.is_null());
                if result.as_ref().is_some_and(|value| value.to_string().len() > RESULT_BYTES) {
                    return Err(TaskError::Invalid(crate::tr!(
                        "Результат больше {size} КБ: положите его в файл своей папки и верните имя файла.",
                        size = RESULT_BYTES / 1024
                    )));
                }
                (TaskState::Done, result, None)
            }
            Outcome::Failed(error) => {
                let error = cut(&error, ERROR);
                (TaskState::Failed, None, Some(if error.is_empty() { crate::tr!("не получилось") } else { error }))
            }
            Outcome::Cancelled => (TaskState::Cancelled, None, None),
        };
        let now = now_ms();
        let task = {
            let mut entries = self.entries();
            let entry = entries.get_mut(id).filter(|entry| entry.task.app == app).ok_or(TaskError::NotFound)?;
            if entry.task.state != TaskState::Running {
                return Err(TaskError::Ended(entry.task.state));
            }
            entry.close(state, result, error, now);
            save(&self.db, entry, now);
            entry.task.clone()
        };
        self.tell(app, id, state.name());
        Ok(task)
    }

    /// A person asks to cancel a task. The app is told with its next report (or wakes from its wait) and ends the
    /// task itself; one that does not within the grace is closed by the gateway (`sweep_at`). A task already over
    /// is answered as it is.
    pub fn cancel(&self, id: &str) -> Result<Task, TaskError> {
        let (task, asked) = {
            let mut entries = self.entries();
            let entry = entries.get_mut(id).ok_or(TaskError::NotFound)?;
            let asked = entry.task.state == TaskState::Running && !entry.task.cancel_requested;
            if asked {
                entry.task.cancel_requested = true;
                entry.cancel_asked_at = Some(now_ms());
                entry.cancel.cancel();
            }
            (entry.task.clone(), asked)
        };
        if asked {
            self.tell(&task.app, id, "cancelling");
        }
        Ok(task)
    }

    /// The app's own task, as it is now.
    pub fn own(&self, app: &str, id: &str) -> Result<Task, TaskError> {
        self.entries()
            .get(id)
            .filter(|entry| entry.task.app == app)
            .map(|entry| entry.task.clone())
            .ok_or(TaskError::NotFound)
    }

    /// The app's own task once a person asked to cancel it or it ended — or as it is when `wait` is over: an app
    /// that reports rarely (a long answer of a model) waits here instead of asking again and again.
    pub async fn wait(&self, app: &str, id: &str, wait: Duration) -> Result<Task, TaskError> {
        let token = {
            let entries = self.entries();
            let entry = entries.get(id).filter(|entry| entry.task.app == app).ok_or(TaskError::NotFound)?;
            entry.cancel.clone()
        };
        let _ = tokio::time::timeout(wait, token.cancelled()).await;
        self.own(app, id)
    }

    /// A task with its journal.
    pub fn get(&self, id: &str) -> Option<TaskLog> {
        self.entries()
            .get(id)
            .map(|entry| TaskLog { task: entry.task.clone(), log: entry.log.iter().cloned().collect() })
    }

    /// The tasks, of one app or of all: what runs first, then the newest.
    pub fn list(&self, app: Option<&str>) -> Vec<Task> {
        let mut tasks: Vec<Task> = self
            .entries()
            .values()
            .filter(|entry| app.is_none_or(|app| entry.task.app == app))
            .map(|entry| entry.task.clone())
            .collect();
        tasks.sort_by(|a, b| {
            (b.state == TaskState::Running)
                .cmp(&(a.state == TaskState::Running))
                .then(b.started_at.cmp(&a.started_at))
                .then(a.id.cmp(&b.id))
        });
        tasks
    }

    /// How many tasks of the app run now: an app at work is not unloaded as idle.
    pub fn running(&self, app: &str) -> usize {
        self.entries().values().filter(|entry| entry.task.app == app && entry.task.state == TaskState::Running).count()
    }

    /// The app's program is gone — it fell, was switched off or unloaded: what it ran is interrupted, with the reason.
    pub fn interrupt_app(&self, app: &str, why: &str) {
        let now = now_ms();
        let closed: Vec<String> = {
            let mut entries = self.entries();
            entries
                .values_mut()
                .filter(|entry| entry.task.app == app && entry.task.state == TaskState::Running)
                .map(|entry| {
                    entry.close(TaskState::Interrupted, None, Some(cut(why, ERROR)), now);
                    save(&self.db, entry, now);
                    entry.task.id.clone()
                })
                .collect()
        };
        for id in closed {
            tracing::info!(target: "tasks", "{app}: a task was interrupted: {why}");
            self.tell(app, &id, TaskState::Interrupted.name());
        }
    }

    /// The gateway stops: what runs is cut short by it. The next start would say the same of a gateway that fell.
    pub fn shutdown(&self) {
        let apps: Vec<String> = {
            let entries = self.entries();
            let mut apps: Vec<String> = entries
                .values()
                .filter(|entry| entry.task.state == TaskState::Running)
                .map(|entry| entry.task.app.clone())
                .collect();
            apps.sort();
            apps.dedup();
            apps
        };
        for app in apps {
            self.interrupt_app(&app, &restarted());
        }
    }

    /// The app is removed: its tasks go with it.
    pub fn forget_app(&self, app: &str) {
        let gone: Vec<String> = {
            let mut entries = self.entries();
            let ids: Vec<String> =
                entries.values().filter(|entry| entry.task.app == app).map(|entry| entry.task.id.clone()).collect();
            for id in &ids {
                if let Some(mut entry) = entries.remove(id) {
                    if entry.task.state == TaskState::Running {
                        entry.close(TaskState::Interrupted, None, Some(forgotten()), now_ms());
                    }
                }
            }
            ids
        };
        if let Err(error) = store::delete_app(&self.db, app) {
            tracing::warn!(target: "tasks", "{app}: its tasks were not removed: {error}");
        }
        if !gone.is_empty() {
            self.tell(app, "", "forgotten");
        }
    }

    /// The limits at the moment `now`: a cancel the app did not answer, an app that fell silent, results kept long
    /// enough.
    pub fn sweep_at(&self, now: u64) {
        let limits = self.limits;
        let mut told: Vec<(String, String, &'static str)> = Vec::new();
        {
            let mut entries = self.entries();
            for entry in entries.values_mut().filter(|entry| entry.task.state == TaskState::Running) {
                let closed = if entry.cancel_asked_at.is_some_and(|at| now.saturating_sub(at) >= limits.cancel_grace_ms)
                {
                    entry.close(TaskState::Cancelled, None, None, now);
                    true
                } else if now.saturating_sub(entry.task.updated_at) >= limits.silence_ms {
                    let minutes = (limits.silence_ms / 60_000).max(1);
                    entry.close(
                        TaskState::Interrupted,
                        None,
                        Some(crate::tr!("Приложение молчало {minutes} мин: задача закрыта.", minutes = minutes)),
                        now,
                    );
                    true
                } else {
                    false
                };
                if closed {
                    save(&self.db, entry, now);
                    told.push((entry.task.app.clone(), entry.task.id.clone(), entry.task.state.name()));
                }
            }
            // What ended long ago, and what is over the number kept per app — the oldest first.
            let mut ended: Vec<(String, String, u64)> = entries
                .values()
                .filter_map(|entry| entry.task.ended_at.map(|at| (entry.task.app.clone(), entry.task.id.clone(), at)))
                .collect();
            ended.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)));
            let mut kept: HashMap<String, usize> = HashMap::new();
            let mut forgotten: Vec<String> = Vec::new();
            for (app, id, at) in ended {
                let seen = kept.entry(app.clone()).or_default();
                *seen += 1;
                if now.saturating_sub(at) >= limits.result_kept_ms || *seen > limits.kept_per_app {
                    entries.remove(&id);
                    if let Err(error) = store::delete(&self.db, &id) {
                        tracing::warn!(target: "tasks", "{app}: an old task was not removed: {error}");
                    }
                    if !forgotten.contains(&app) {
                        forgotten.push(app);
                    }
                }
            }
            told.extend(forgotten.into_iter().map(|app| (app, String::new(), "forgotten")));
        }
        for (app, id, change) in told {
            self.tell(&app, &id, change);
        }
    }
}

fn save(db: &Db, entry: &mut Entry, now: u64) {
    entry.saved_at = now;
    let log: Vec<Step> = entry.log.iter().cloned().collect();
    if let Err(error) = store::save(db, &entry.task, &log) {
        tracing::warn!(target: "tasks", "{}: a task was not written: {error}", entry.task.app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tasks() -> (Arc<Tasks>, EventBus, Arc<Db>) {
        let (db, events) = (Arc::new(Db::memory()), EventBus::new());
        (Tasks::new(db.clone(), events.clone()), events, db)
    }

    /// The changes told since the listener was made, as `task-less` words: `started`, `progress`…
    fn told(listener: &mut tokio::sync::broadcast::Receiver<Arc<GatewayEvent>>) -> Vec<String> {
        let mut changes = Vec::new();
        while let Ok(event) = listener.try_recv() {
            if let GatewayEvent::TasksChanged { change, .. } = event.as_ref() {
                changes.push(change.clone());
            }
        }
        changes
    }

    #[test]
    fn a_task_starts_tells_its_progress_and_ends_with_its_result() {
        let (tasks, events, _) = tasks();
        let mut listener = events.subscribe();
        let task = tasks.start("feed", "  Снимаю серию 3  ", Some("Сценарий"), None, 1).unwrap();
        assert_eq!((task.title.as_str(), task.state, task.percent), ("Снимаю серию 3", TaskState::Running, None));
        assert_eq!(tasks.progress("feed", &task.id, Some(40.4), Some("Озвучка")), Ok(false));
        assert_eq!(tasks.progress("feed", &task.id, Some(250.0), None), Ok(false), "a percent stays within 0–100");
        let now = tasks.own("feed", &task.id).unwrap();
        assert_eq!((now.percent, now.text.as_str()), (Some(100), "Озвучка"));
        let done = tasks.finish("feed", &task.id, Outcome::Done(Some(json!({ "file": "s1e3.mp4" })))).unwrap();
        assert_eq!((done.state, done.percent), (TaskState::Done, Some(100)));
        assert_eq!(done.result, Some(json!({ "file": "s1e3.mp4" })));
        assert!(done.ended_at.is_some());
        let changes = told(&mut listener);
        assert_eq!(changes.first().map(String::as_str), Some("started"));
        assert_eq!(changes.last().map(String::as_str), Some("done"));
        // Its journal: a line a step, the same step moving its line.
        let log: Vec<String> = tasks.get(&task.id).unwrap().log.into_iter().map(|step| step.text).collect();
        assert_eq!(log, ["Сценарий", "Озвучка", "Готово"]);
        // A report on a task that is over says how it ended.
        assert_eq!(tasks.progress("feed", &task.id, Some(10.0), None), Err(TaskError::Ended(TaskState::Done)));
        assert_eq!(tasks.finish("feed", &task.id, Outcome::Cancelled), Err(TaskError::Ended(TaskState::Done)));
    }

    #[test]
    fn an_app_runs_one_task_at_a_time_unless_its_manifest_asks_for_more() {
        let (tasks, _, _) = tasks();
        let first = tasks.start("feed", "Серия 1", None, None, 1).unwrap();
        let refused = tasks.start("feed", "Серия 2", None, None, 1).unwrap_err();
        assert_eq!(refused, TaskError::TooMany { title: "Серия 1".into(), limit: 1 });
        assert!(refused.to_string().contains("«Серия 1»"), "{refused}");
        // Another app's task is its own business.
        tasks.start("archive", "Разбор", None, None, 1).unwrap();
        assert_eq!((tasks.running("feed"), tasks.running("archive"), tasks.running("nobody")), (1, 1, 0));
        // Two at once for the app that asked for two, never a third; the most is four whatever it asks.
        tasks.start("feed", "Серия 2", None, None, 2).unwrap();
        assert!(matches!(tasks.start("feed", "Серия 3", None, None, 2), Err(TaskError::TooMany { limit: 2, .. })));
        assert!(tasks.start("feed", "Серия 3", None, None, 99).is_ok());
        assert!(tasks.start("feed", "Серия 4", None, None, 99).is_ok());
        assert!(matches!(tasks.start("feed", "Серия 5", None, None, 99), Err(TaskError::TooMany { limit: 4, .. })));
        // One ended: the next may start.
        tasks.finish("feed", &first.id, Outcome::Failed("нет связи".into())).unwrap();
        assert!(tasks.start("feed", "Серия 5", None, None, 4).is_ok());
        assert!(matches!(tasks.start("feed", "  ", None, None, 1), Err(TaskError::Invalid(_))), "a task has a name");
    }

    #[test]
    fn a_cancel_reaches_the_app_with_its_next_report_and_the_app_ends_the_task() {
        let (tasks, events, _) = tasks();
        let task = tasks.start("feed", "Копия для отправки", None, Some(0.0), 1).unwrap();
        let mut listener = events.subscribe();
        let asked = tasks.cancel(&task.id).unwrap();
        assert!(asked.cancel_requested && asked.state == TaskState::Running, "the app ends it, not the panel");
        assert_eq!(told(&mut listener), ["cancelling"]);
        tasks.cancel(&task.id).unwrap();
        assert_eq!(told(&mut listener), Vec::<String>::new(), "asked once");
        assert_eq!(tasks.progress("feed", &task.id, Some(30.0), None), Ok(true));
        let ended = tasks.finish("feed", &task.id, Outcome::Cancelled).unwrap();
        assert_eq!((ended.state, ended.cancel_requested, ended.error), (TaskState::Cancelled, false, None));
        // Cancelling what is over changes nothing and is no error: the button may be pressed late.
        assert_eq!(tasks.cancel(&task.id).unwrap().state, TaskState::Cancelled);
        assert_eq!(tasks.cancel("no-such-task"), Err(TaskError::NotFound));
    }

    #[test]
    fn an_app_that_does_not_answer_a_cancel_or_falls_silent_has_its_task_closed() {
        let (db, events) = (Arc::new(Db::memory()), EventBus::new());
        let limits = Limits { cancel_grace_ms: 30_000, silence_ms: 60_000, ..Limits::default() };
        let tasks = Tasks::with_limits(db, events, limits);
        let deaf = tasks.start("feed", "Глухая", None, None, 4).unwrap();
        let silent = tasks.start("feed", "Молчит", None, None, 4).unwrap();
        tasks.cancel(&deaf.id).unwrap();
        let now = now_ms();
        tasks.sweep_at(now + 10_000);
        assert_eq!(tasks.own("feed", &deaf.id).unwrap().state, TaskState::Running, "the grace is not over");
        tasks.sweep_at(now + 31_000);
        assert_eq!(tasks.own("feed", &deaf.id).unwrap().state, TaskState::Cancelled);
        assert_eq!(tasks.own("feed", &silent.id).unwrap().state, TaskState::Running);
        // The app wakes up late: its report is refused with how the task ended.
        assert_eq!(tasks.finish("feed", &deaf.id, Outcome::Done(None)), Err(TaskError::Ended(TaskState::Cancelled)));
        tasks.sweep_at(now + 61_000);
        let closed = tasks.own("feed", &silent.id).unwrap();
        assert_eq!(closed.state, TaskState::Interrupted);
        assert!(closed.error.unwrap().contains("молчало 1 мин"));
    }

    #[test]
    fn a_restart_marks_what_ran_as_interrupted_and_keeps_what_ended() {
        let (db, events) = (Arc::new(Db::memory()), EventBus::new());
        let before = Tasks::new(db.clone(), events.clone());
        let ran = before.start("feed", "Снимаю серию 3", Some("Рендер"), Some(60.0), 2).unwrap();
        let done = before.start("feed", "Копия", None, None, 2).unwrap();
        before.finish("feed", &done.id, Outcome::Done(Some(json!({ "file": "copy.mp4" })))).unwrap();
        drop(before);
        // The gateway starts again over the same database.
        let after = Tasks::new(db.clone(), events.clone());
        let cut = after.get(&ran.id).unwrap();
        assert_eq!((cut.task.state, cut.task.percent), (TaskState::Interrupted, Some(60)));
        assert_eq!(cut.task.error.as_deref(), Some(restarted().as_str()));
        assert!(cut.task.ended_at.is_some());
        assert_eq!(cut.log.last().map(|step| step.text.as_str()), Some(restarted().as_str()));
        let kept = after.own("feed", &done.id).unwrap();
        assert_eq!((kept.state, kept.result), (TaskState::Done, Some(json!({ "file": "copy.mp4" }))));
        // Nothing restores it: the app's slot is free, and a third start reads the same.
        assert_eq!(after.running("feed"), 0);
        assert!(after.start("feed", "Снимаю серию 3", None, None, 1).is_ok());
        let again = Tasks::new(db, events);
        assert_eq!(again.own("feed", &ran.id).unwrap().error.as_deref(), Some(restarted().as_str()));
    }

    #[test]
    fn what_ended_is_kept_a_day_and_a_few_per_app() {
        let (db, events) = (Arc::new(Db::memory()), EventBus::new());
        let day = Limits::default().result_kept_ms;
        let limits = Limits { kept_per_app: 3, silence_ms: 2 * day, ..Limits::default() };
        let tasks = Tasks::with_limits(db.clone(), events, limits);
        let mut ids = Vec::new();
        for n in 0..5 {
            let task = tasks.start("feed", &format!("Серия {n}"), None, None, 1).unwrap();
            tasks.finish("feed", &task.id, Outcome::Done(None)).unwrap();
            ids.push(task.id);
            std::thread::sleep(Duration::from_millis(2));
        }
        let running = tasks.start("feed", "Идёт", None, None, 1).unwrap();
        let listed = tasks.list(Some("feed"));
        assert_eq!(listed.len(), 4, "three ended and the one that runs");
        assert_eq!(listed[0].id, running.id, "what runs comes first");
        assert!(tasks.get(&ids[0]).is_none() && tasks.get(&ids[4]).is_some(), "the oldest went");
        assert_eq!(store::load(&db).unwrap().len(), 4, "in the database too");
        // A day later the ended ones are forgotten; what runs stays.
        tasks.sweep_at(now_ms() + day + 1000);
        let left: Vec<String> = tasks.list(None).into_iter().map(|task| task.title).collect();
        assert_eq!(left, ["Идёт"]);
        assert_eq!(store::load(&db).unwrap().len(), 1);
    }

    #[test]
    fn a_task_is_its_apps_own_and_goes_with_the_app() {
        let (tasks, events, db) = tasks();
        let task = tasks.start("feed", "Снимаю", None, None, 1).unwrap();
        assert_eq!(tasks.progress("other", &task.id, Some(5.0), None), Err(TaskError::NotFound));
        assert_eq!(tasks.finish("other", &task.id, Outcome::Done(None)), Err(TaskError::NotFound));
        assert_eq!(tasks.own("other", &task.id), Err(TaskError::NotFound));
        // The app's program fell: what it ran is interrupted, with the reason.
        let mut listener = events.subscribe();
        tasks.interrupt_app("feed", "Приложение упало.");
        let cut = tasks.own("feed", &task.id).unwrap();
        assert_eq!((cut.state, cut.error.as_deref()), (TaskState::Interrupted, Some("Приложение упало.")));
        assert_eq!(told(&mut listener), ["interrupted"]);
        tasks.interrupt_app("feed", "ещё раз");
        assert_eq!(told(&mut listener), Vec::<String>::new(), "nothing runs: nothing to interrupt");
        // The app removed: nothing of its tasks stays.
        tasks.forget_app("feed");
        assert!(tasks.list(None).is_empty());
        assert!(store::load(&db).unwrap().is_empty());
    }

    #[test]
    fn a_result_is_an_answer_not_a_file() {
        let (tasks, _, _) = tasks();
        let task = tasks.start("feed", "Снимаю", None, None, 1).unwrap();
        let big = json!({ "data": "x".repeat(RESULT_BYTES) });
        assert!(matches!(tasks.finish("feed", &task.id, Outcome::Done(Some(big))), Err(TaskError::Invalid(_))));
        assert_eq!(tasks.own("feed", &task.id).unwrap().state, TaskState::Running, "refused, the task goes on");
        let failed = tasks.finish("feed", &task.id, Outcome::Failed("  ".into())).unwrap();
        assert_eq!((failed.state, failed.error.as_deref()), (TaskState::Failed, Some("не получилось")));
    }

    #[tokio::test]
    async fn a_burst_of_reports_is_told_once_and_its_last_word_too() {
        let (tasks, events, _) = tasks();
        let task = tasks.start("feed", "Копия", None, None, 1).unwrap();
        let mut listener = events.subscribe();
        for percent in 1..=50 {
            tasks.progress("feed", &task.id, Some(f64::from(percent)), None).unwrap();
        }
        assert_eq!(told(&mut listener), Vec::<String>::new(), "the start was told just now");
        tokio::time::sleep(Duration::from_millis(TOLD_EVERY_MS + 200)).await;
        assert_eq!(told(&mut listener), ["progress"], "the last report of the burst");
        assert_eq!(tasks.own("feed", &task.id).unwrap().percent, Some(50));
    }

    #[tokio::test]
    async fn an_app_that_waits_wakes_when_a_person_cancels() {
        let (tasks, _, _) = tasks();
        let task = tasks.start("feed", "Жду модель", None, None, 1).unwrap();
        let still = tasks.wait("feed", &task.id, Duration::from_millis(50)).await.unwrap();
        assert!(!still.cancel_requested, "the wait is over, nobody asked");
        let waiting = {
            let (tasks, id) = (tasks.clone(), task.id.clone());
            tokio::spawn(async move { tasks.wait("feed", &id, Duration::from_secs(30)).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        tasks.cancel(&task.id).unwrap();
        let woke = tokio::time::timeout(Duration::from_secs(2), waiting).await.unwrap().unwrap().unwrap();
        assert!(woke.cancel_requested);
        assert_eq!(tasks.wait("other", &task.id, Duration::from_millis(10)).await, Err(TaskError::NotFound));
    }
}
