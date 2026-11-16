//! The executor «папка» (docs/plans/router-and-works.md, block A, 10): a work in a work folder — an agent in its own
//! copy of the folder, on its own branch — is a work of the kind `folder`, the fourth after the workshop's apps, the
//! background tools and the apps' tasks. Both builds have it; the heavy part — copies, agents, journals — is the
//! helper's (src/works, the computer's build only), and this executor speaks to it only through the port
//! `WorksPort`: begin, add on the way, how it goes, stop, and the helper's messages. No helper with works — «Для работы
//! в папках нужен компьютер».
//!
//! Stage 3 begins a work from the panel («Новая работа»), says how it goes («правит `form.ts`», from its journal,
//! without a model), adds on the way (a note waits for the run's end and goes to the agent in the same session), stops
//! it, and ends it by the road of the background works: «Готово» with the branch, the folder's checks and the dollars
//! (`told`). A plan's limit is the work's open question «подождать или остановить?»: silence waits too. Voice is stage
//! 5. Each run of the agent is a deal of the router, by the key `work:folder:<id>`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};

use futures::future::BoxFuture;
use futures::FutureExt;
use serde::Serialize;

use super::asked::Kind;
use super::executor::{ByVoice, Closed, Doing, Executor, Facts, Put, Report, Reporter, Started, Told};
use super::layer::TalkLayer;
use super::origin::Origin;
use super::pack::{self, fill, FolderWords};
use super::work_tools::Refusal;
use super::works::{End, NewWork, Question, Work, WorkResult, WorkState, FOLDER};
use crate::memory::text::Lang;
use crate::router::Spent;
use crate::utils::time::now_ms;

/// Its name in the records.
pub const FOLDER_EXECUTOR: &str = "folder";
/// A limit's question is taken without the wake word this long; unanswered, the work simply waits.
const ASK_FOR_MS: u64 = 60_000;

/// A work to begin in a folder: the folder's id, the task as the person gave it, its title, the agent and the model
/// asked (none — the folder's), and the words that make an agent go on in its session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderStart {
    pub folder: String,
    pub task: String,
    pub title: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub resume: String,
}

/// What a work in a folder is at, from its journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "is", content = "what", rename_all = "camelCase")]
pub enum FolderDoing {
    Begins,
    Prepares,
    Reads(String),
    Edits(String),
    Runs(String),
    Searches(String),
    Fetches(String),
    Thinks,
    Writes,
    Checks(String),
    Hands,
    /// A plan's limit holds it: until when, if known.
    Waits(Option<u64>),
}

/// A check of the folder after the agent: the command, whether it passed, its code, how long, the end of its output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderCheck {
    pub command: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    pub ms: u64,
    pub tail: String,
}

/// Why a work in a folder ended as it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FolderWhy {
    Auth,
    Billing,
    Budget,
    Turns,
    Refused,
    Limit,
    NoAgent,
    Folder,
    Git,
    Failed,
}

impl FolderWhy {
    pub fn as_str(self) -> &'static str {
        match self {
            FolderWhy::Auth => "auth",
            FolderWhy::Billing => "billing",
            FolderWhy::Budget => "budget",
            FolderWhy::Turns => "turns",
            FolderWhy::Refused => "refused",
            FolderWhy::Limit => "limit",
            FolderWhy::NoAgent => "no-agent",
            FolderWhy::Folder => "folder",
            FolderWhy::Git => "git",
            FolderWhy::Failed => "failed",
        }
    }

    pub fn parse(text: &str) -> Option<FolderWhy> {
        [
            FolderWhy::Auth,
            FolderWhy::Billing,
            FolderWhy::Budget,
            FolderWhy::Turns,
            FolderWhy::Refused,
            FolderWhy::Limit,
            FolderWhy::NoAgent,
            FolderWhy::Folder,
            FolderWhy::Git,
            FolderWhy::Failed,
        ]
        .into_iter()
        .find(|why| why.as_str() == text)
    }
}

/// What a work in a folder came to: the folder and the agent by name, the branch and whether it reached the folder,
/// the checks, the dollars (by a plan — what a key would have cost; none — unknown), the agent's last message, why it
/// ended so and the error in the agent's or git's words.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderTold {
    pub folder: String,
    pub agent: String,
    pub branch: String,
    pub fetched: bool,
    pub changed: bool,
    pub checks: Vec<FolderCheck>,
    pub checks_skipped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usd_micro: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<FolderWhy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// How a work in a folder ended, for the layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderEnd {
    Ready,
    Unfinished,
    Failed,
    Stopped,
}

/// What the helper tells the executor.
#[derive(Debug, Clone, PartialEq)]
pub enum PortEvent {
    /// The notes said on the way went to the agent.
    Taken { id: String },
    /// Who answers it now: the agent's way and model.
    Model { id: String, provider: String, model: String },
    /// A plan's limit holds it: until when, if known, and the agent's name.
    Waits { id: String, until: Option<u64>, agent: String },
    /// The wait is over: it goes on.
    Goes { id: String },
    /// A run ended: its spending, a deal to close.
    Ran { id: String, n: u32, spent: Spent },
    /// The work ended.
    Ended { id: String, end: FolderEnd },
}

pub type PortListener = Arc<dyn Fn(PortEvent) + Send + Sync>;

/// The helper's works in folders, as the rest of the gateway sees them (docs/plans/router-and-works.md, 6–9). Until
/// stage 5 behind it stands the module itself (src/works) — in the computer's build the same program.
pub trait WorksPort: Send + Sync {
    /// A folder's name by its id; None — no such folder.
    fn folder_name(&self, folder: &str) -> Option<String>;
    /// Begins a work: its id at the helper, or why not.
    fn start(&self, start: FolderStart) -> Result<String, String>;
    /// Words added while it runs: they wait for the run's end. False — nothing runs.
    fn add(&self, id: &str, text: &str) -> bool;
    /// Whether a work that ended can go on (its folder is there): its title.
    fn can_again(&self, id: &str) -> Result<String, String>;
    /// A work that ended goes on in its session: the words to the agent (to go on after a cut, `resume`).
    fn again(&self, id: &str, text: &str, resume: &str) -> Result<(), String>;
    /// What it is at now; None — nothing goes on.
    fn doing(&self, id: &str) -> Option<FolderDoing>;
    fn stop(&self, id: &str) -> bool;
    fn told(&self, id: &str) -> Option<FolderTold>;
    /// A run's deal, once the router wrote it.
    fn dealt(&self, id: &str, n: u32, deal: i64);
    fn listen(&self, listener: PortListener);
}

/// Where the runs' deals go (app.rs): closed into the router's journal by `work:folder:<id>` — the deal's id — and
/// who answers a work going on.
#[derive(Clone)]
pub struct FolderDeals {
    pub close: DealCloser,
    pub heard: ModelHeard,
}

/// A run's spending closed into a deal: its id, none when it was not written.
pub type DealCloser = Arc<dyn Fn(&Spent) -> Option<i64> + Send + Sync>;
/// Who answers a work going on: its deal key, the agent's way, the model.
pub type ModelHeard = Arc<dyn Fn(&str, &str, &str) + Send + Sync>;

/// The key a work's deals go by.
pub fn deal_key(id: &str) -> String {
    format!("work:{FOLDER_EXECUTOR}:{id}")
}

pub struct FolderExecutor {
    layer: Weak<TalkLayer>,
    port: RwLock<Option<Arc<dyn WorksPort>>>,
    reporter: OnceLock<Reporter>,
    deals: OnceLock<FolderDeals>,
    /// What a work the layer created begins with, until it starts: by the work's id.
    staged: Mutex<HashMap<String, FolderStart>>,
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl FolderExecutor {
    /// The executor joins the layer; the helper comes with `attach` (none on the station).
    pub fn new(layer: &Arc<TalkLayer>) -> Arc<FolderExecutor> {
        let executor = Arc::new(FolderExecutor {
            layer: Arc::downgrade(layer),
            port: RwLock::new(None),
            reporter: OnceLock::new(),
            deals: OnceLock::new(),
            staged: Mutex::new(HashMap::new()),
        });
        layer.add_executor(executor.clone());
        layer.set_folders(&executor);
        executor
    }

    /// The helper's works: from now on the executor begins works there and hears its messages.
    pub fn attach(self: &Arc<Self>, port: Arc<dyn WorksPort>) {
        let me = Arc::downgrade(self);
        port.listen(Arc::new(move |event| {
            if let Some(executor) = me.upgrade() {
                executor.heard(event);
            }
        }));
        *self.port.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(port);
    }

    pub fn attach_deals(&self, deals: FolderDeals) {
        let _ = self.deals.set(deals);
    }

    pub fn port(&self) -> Option<Arc<dyn WorksPort>> {
        self.port.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    fn report(&self, report: Report) {
        if let Some(reporter) = self.reporter.get() {
            reporter(FOLDER_EXECUTOR, report);
        }
    }

    pub(super) fn stage(&self, work: &str, start: FolderStart) {
        locked(&self.staged).insert(work.to_string(), start);
    }

    fn heard(&self, event: PortEvent) {
        let Some(layer) = self.layer.upgrade() else { return };
        match event {
            PortEvent::Taken { id } => self.report(Report::Read { target: id }),
            PortEvent::Model { id, provider, model } => {
                if let Some(deals) = self.deals.get() {
                    (deals.heard)(&deal_key(&id), &provider, &model);
                }
            }
            PortEvent::Waits { id, until, agent } => layer.folder_waits(&id, until, &agent),
            PortEvent::Goes { id } => layer.folder_goes(&id),
            PortEvent::Ran { id, n, spent } => {
                let usage = crate::dialog::llm::LlmUsage {
                    input: spent.tokens.input,
                    output: spent.tokens.output,
                    cache_read: Some(spent.tokens.cache_read),
                    cache_write: Some(spent.tokens.cache_write),
                };
                layer.spent(FOLDER_EXECUTOR, &id, &usage);
                let deal = self.deals.get().and_then(|deals| (deals.close)(&spent));
                if let (Some(deal), Some(port)) = (deal, self.port()) {
                    port.dealt(&id, n, deal);
                }
            }
            PortEvent::Ended { id, end } => {
                let state = match end {
                    FolderEnd::Ready => WorkState::Ready,
                    FolderEnd::Unfinished => WorkState::Unfinished,
                    FolderEnd::Failed => WorkState::Failed,
                    FolderEnd::Stopped => WorkState::Stopped,
                };
                self.report(Report::Ended { target: id, state, line: None, title: None });
            }
        }
    }

    fn words(&self, work: &Work) -> &'static FolderWords {
        let lang = self.layer.upgrade().map(|layer| layer.lang(work.device())).unwrap_or(Lang::Ru);
        &pack::of(lang).work.folder
    }
}

/// A work's line of how it goes, in the pack's words: «правит form.ts», «ждёт лимита до 18:00».
pub fn doing_line(words: &FolderWords, doing: &FolderDoing) -> String {
    let what = |text: &str, what: &str| fill(text, &[("what", &short_what(what))]);
    match doing {
        FolderDoing::Begins => words.begins.to_string(),
        FolderDoing::Prepares => words.prepares.to_string(),
        FolderDoing::Reads(file) => what(words.reads, file),
        FolderDoing::Edits(file) => what(words.edits, file),
        FolderDoing::Runs(command) => what(words.runs, command),
        FolderDoing::Searches(pattern) => what(words.searches, pattern),
        FolderDoing::Fetches(address) => what(words.fetches, address),
        FolderDoing::Thinks => words.thinks.to_string(),
        FolderDoing::Writes => words.writes.to_string(),
        FolderDoing::Checks(command) => what(words.checks, command),
        FolderDoing::Hands => words.hands.to_string(),
        FolderDoing::Waits(Some(until)) => fill(words.waits_until, &[("until", &clock(*until))]),
        FolderDoing::Waits(None) => words.waits.to_string(),
    }
}

/// A file by its name, a command by its first words: what a line of how it goes can say.
fn short_what(what: &str) -> String {
    let what = what.trim();
    let name = if what.contains('/') && !what.contains(' ') { what.rsplit('/').next().unwrap_or(what) } else { what };
    let words: Vec<&str> = name.split_whitespace().take(4).collect();
    let mut text = words.join(" ");
    if text.chars().count() > 48 {
        text = text.chars().take(47).collect::<String>() + "…";
    }
    text
}

/// A time of the day on this computer's clock: «18:00».
pub fn clock(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|at| at.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

/// Dollars as people read them: «$1.20», «$0.04».
pub fn dollars(usd_micro: u64) -> String {
    let usd = usd_micro as f64 / 1_000_000.0;
    if usd > 0.0 && usd < 0.01 {
        return "<$0.01".into();
    }
    format!("${usd:.2}")
}

/// The result's words: the branch, the checks, the dollars.
pub fn summary_of(words: &FolderWords, told: &FolderTold) -> String {
    let mut parts = Vec::new();
    if told.changed {
        parts.push(fill(words.branch, &[("branch", &told.branch)]));
    } else {
        parts.push(words.no_changes.to_string());
    }
    if told.checks_skipped {
        parts.push(words.checks_skipped.to_string());
    } else if let Some(failed) = told.checks.iter().find(|check| !check.ok) {
        parts.push(fill(words.checks_failed, &[("command", &failed.command)]));
    } else if !told.checks.is_empty() {
        parts.push(words.checks_passed.to_string());
    }
    match (told.usd_micro, told.billing.as_deref()) {
        (Some(usd), Some("plan")) => parts.push(fill(words.usd_plan, &[("usd", &dollars(usd))])),
        (Some(usd), _) => parts.push(fill(words.usd, &[("usd", &dollars(usd))])),
        (None, _) => parts.push(words.price_unknown.to_string()),
    }
    if told.changed && !told.fetched {
        if let Some(error) = &told.error {
            parts.push(fill(words.not_fetched, &[("why", error)]));
        }
    }
    let mut line = parts.join(", ");
    if let Some(first) = line.chars().next() {
        line = first.to_uppercase().chain(line.chars().skip(1)).collect();
    }
    format!("{line}.")
}

/// Why it ended so, in words.
pub fn why_of(words: &FolderWords, told: &FolderTold) -> String {
    let agent = told.agent.as_str();
    let why = told.error.as_deref().unwrap_or_default();
    match told.why {
        Some(FolderWhy::Auth) => fill(words.why_auth, &[("agent", agent)]),
        Some(FolderWhy::Billing) => words.why_billing.to_string(),
        Some(FolderWhy::Budget) => words.why_budget.to_string(),
        Some(FolderWhy::Turns) => words.why_turns.to_string(),
        Some(FolderWhy::Refused) => words.why_refused.to_string(),
        Some(FolderWhy::Limit) => fill(words.why_limit, &[("agent", agent)]),
        Some(FolderWhy::NoAgent) => fill(words.why_agent, &[("why", why)]),
        Some(FolderWhy::Folder) => words.why_folder.to_string(),
        Some(FolderWhy::Git) => fill(words.why_git, &[("why", why)]),
        Some(FolderWhy::Failed) | None => words.why_failed.to_string(),
    }
}

/// A work's title from its task: its first sentence, short, with a capital.
pub fn title_of(task: &str) -> String {
    let first = task.trim().lines().next().unwrap_or_default();
    let sentence = first.split(['.', '!', '?']).next().unwrap_or(first).trim();
    let mut title: String = sentence.chars().take(60).collect();
    if sentence.chars().count() > 60 {
        title = title.trim_end().to_string() + "…";
    }
    let mut chars = title.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Работа".into(),
    }
}

impl Executor for FolderExecutor {
    fn name(&self) -> &'static str {
        FOLDER_EXECUTOR
    }

    fn listen(&self, reports: Reporter) {
        let _ = self.reporter.set(reports);
    }

    fn start(&self, work: &Work) -> Result<Started, String> {
        let words = self.words(work);
        let port = self.port().ok_or_else(|| words.no_computer.to_string())?;
        if let (true, Some(target)) = (work.again, work.target.as_deref()) {
            port.again(target, &work.brief, words.resume_prompt)?;
            return Ok(Started { target: target.to_string(), title: work.title.clone() });
        }
        let start =
            locked(&self.staged).remove(&work.id).ok_or_else(|| "nothing to begin: the work was lost".to_string())?;
        let target = port.start(start)?;
        Ok(Started { target, title: work.title.clone() })
    }

    fn add(&self, work: &Work, text: &str) -> bool {
        let (Some(target), Some(port)) = (work.target.as_deref(), self.port()) else { return false };
        port.add(target, text)
    }

    fn thing(&self, _name: &str) -> Option<Started> {
        None
    }

    /// A work that ended goes on in its session: a new record of the same work.
    fn open<'a>(&'a self, target: &'a str, _copy: bool) -> BoxFuture<'a, Result<Started, Closed>> {
        let port = self.port();
        let lang = self
            .layer
            .upgrade()
            .and_then(|layer| layer.works.of_target(FOLDER_EXECUTOR, target).map(|work| layer.lang(work.device())))
            .unwrap_or(Lang::Ru);
        async move {
            let words = &pack::of(lang).work.folder;
            let port = port.ok_or_else(|| Closed::Failed(words.no_computer.to_string()))?;
            let title = port.can_again(target).map_err(|_| Closed::Failed(words.no_again.to_string()))?;
            Ok(Started { target: target.to_string(), title })
        }
        .boxed()
    }

    fn status(&self, _work: &Work) -> Option<Doing> {
        Some(Doing::Thinking)
    }

    /// Going — what it is at, from its journal; ended — the result's words.
    fn details(&self, work: &Work) -> Option<String> {
        let target = work.target.as_deref()?;
        let port = self.port()?;
        let words = self.words(work);
        if work.going() {
            return port.doing(target).map(|doing| doing_line(words, &doing));
        }
        let told = port.told(target)?;
        Some(told.last.clone().unwrap_or_else(|| summary_of(words, &told)))
    }

    fn facts<'a>(&'a self, _work: &'a Work) -> BoxFuture<'a, Facts> {
        async { Facts::default() }.boxed()
    }

    fn put<'a>(&'a self, _work: &'a Work, _end: End, _by: ByVoice) -> BoxFuture<'a, Result<Put, String>> {
        async { Ok(Put::Shown { open: false }) }.boxed()
    }

    fn stop(&self, work: &Work) -> bool {
        let (Some(target), Some(port)) = (work.target.as_deref(), self.port()) else { return false };
        port.stop(target)
    }

    /// The result for «Готово» and the page: the branch, the checks and the dollars; «подробнее» — the agent's last
    /// message; the data — all of it for the panel.
    fn told(&self, work: &Work) -> Option<Told> {
        let target = work.target.as_deref()?;
        let told = self.port()?.told(target)?;
        let words = self.words(work);
        let error = match work.state {
            WorkState::Failed | WorkState::Unfinished => Some(why_of(words, &told)),
            _ => None,
        };
        Some(Told {
            summary: Some(summary_of(words, &told)),
            more: told.last.clone(),
            data: serde_json::to_string(&told).ok(),
            error,
        })
    }
}

/// A work to begin in a folder, as the panel asks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderAsk {
    pub folder: String,
    pub task: String,
    pub agent: Option<String>,
    pub model: Option<String>,
}

impl TalkLayer {
    /// «Новая работа» in a folder (stage 3, from the panel): a work of the kind `folder`, at once or in the queue — one
    /// work in folders at a time. The words said, and the work.
    pub fn start_folder(&self, origin: Origin, ask: FolderAsk) -> Result<(String, Work), Refusal> {
        let lang = self.lang(&origin.device);
        let words = &pack::of(lang).work.folder;
        let executor = self.folders().ok_or_else(|| Refusal::Failed(words.no_computer.to_string()))?;
        let port = executor.port().ok_or_else(|| Refusal::Failed(words.no_computer.to_string()))?;
        let task = ask.task.trim();
        if task.is_empty() {
            return Err(Refusal::NoText);
        }
        let folder_name = port.folder_name(&ask.folder).ok_or_else(|| Refusal::Failed(words.why_folder.to_string()))?;
        let title = title_of(task);
        let wait = self.works.may_start_kind(FOLDER, &origin, false, now_ms());
        if wait == Err(super::works::Refused::Enough) {
            return Err(Refusal::Enough);
        }
        let work = self.works.create(NewWork {
            kind: FOLDER,
            face: None,
            executor: FOLDER_EXECUTOR.into(),
            target: None,
            title: title.clone(),
            brief: task.to_string(),
            origin,
            again: false,
        });
        executor.stage(
            &work.id,
            FolderStart {
                folder: ask.folder,
                task: task.to_string(),
                title: title.clone(),
                agent: ask.agent,
                model: ask.model,
                resume: words.resume_prompt.to_string(),
            },
        );
        if wait.is_err() {
            self.enqueue(&work, None);
            let work = self.works.get(&work.id).unwrap_or(work);
            return Ok((fill(words.queued, &[("title", &title)]), work));
        }
        let work = self.launch(work)?;
        let said = fill(words.started, &[("title", &work.title), ("folder", &folder_name)]);
        Ok((said, work))
    }

    /// A plan's limit holds a work: its open question «подождать или остановить?» — silence waits too.
    pub(super) fn folder_waits(&self, target: &str, until: Option<u64>, agent: &str) {
        let Some(work) = self.works.of_target(FOLDER_EXECUTOR, target).filter(Work::going) else { return };
        let words = &pack::of(self.lang(work.device())).work.folder;
        let text = match until {
            Some(until) => fill(words.limit_q_until, &[("agent", agent), ("until", &clock(until))]),
            None => fill(words.limit_q, &[("agent", agent)]),
        };
        let changed = self.works.update(&work.id, |work| {
            work.question = Some(Question::new(text.clone(), now_ms() + ASK_FOR_MS, Kind::Wait));
        });
        if let Some(work) = changed {
            self.told(&work, "asks");
        }
    }

    /// The limit's wait is over: the question goes.
    pub(super) fn folder_goes(&self, target: &str) {
        let Some(work) = self.works.of_target(FOLDER_EXECUTOR, target).filter(Work::going) else { return };
        if work.question.as_ref().and_then(|question| question.kind) != Some(Kind::Wait) {
            return;
        }
        if let Some(work) = self.works.update(&work.id, |work| work.question = None) {
            self.told(&work, "working");
        }
    }

    /// «Готово» of a work in a folder: the frame is the pack's, the result in it the branch, the checks and the
    /// dollars — the helper's facts, no model's retelling. Nothing to put anywhere.
    pub(super) fn finish_folder(self: &Arc<Self>, work: Work, executor: Arc<dyn Executor>) {
        let words = &pack::of(self.lang(work.device())).work.folder;
        let told = executor.told(&work).unwrap_or_default();
        let title = work.title.clone();
        let summary = told.summary.clone().unwrap_or_default();
        let why = told.error.clone().unwrap_or_else(|| words.why_failed.to_string());
        let line = match work.state {
            WorkState::Failed => fill(words.failed, &[("title", &title), ("why", &why)]),
            WorkState::Unfinished => fill(words.unfinished, &[("title", &title), ("why", &why), ("summary", &summary)]),
            _ => fill(words.ready, &[("title", &title), ("summary", &summary)]),
        };
        let state = work.state;
        let Some(work) = self.works.update(&work.id, |work| {
            work.result = Some(WorkResult {
                line: line.clone(),
                end: None,
                summary: told.summary.clone(),
                data: told.data.clone(),
            });
            work.question = None;
            work.said_at = None;
            if state == WorkState::Ready {
                work.state = WorkState::Done;
            }
            work.ended_at.get_or_insert(now_ms());
        }) else {
            return;
        };
        self.told(&work, state.as_str());
        if work.device() != super::COMPUTER {
            self.say_on_station(work.id.clone(), true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_of_how_it_goes_names_the_file_and_the_command_shortly() {
        let words = &pack::of(Lang::Ru).work.folder;
        assert_eq!(doing_line(words, &FolderDoing::Edits("src/app/form.ts".into())), "правит form.ts");
        assert_eq!(
            doing_line(words, &FolderDoing::Runs("npm test -- --watch=false --ci".into())),
            "запускает npm test -- --watch=false"
        );
        assert!(doing_line(words, &FolderDoing::Waits(Some(0))).starts_with("ждёт лимита до "));
        assert_eq!(title_of("страница контактов с формой. И ещё валидация."), "Страница контактов с формой");
    }

    #[test]
    fn the_result_says_the_branch_the_checks_and_the_dollars() {
        let words = &pack::of(Lang::Ru).work.folder;
        let mut told = FolderTold {
            folder: "Сайт".into(),
            agent: "Claude Code".into(),
            branch: "duowave/kontakty".into(),
            fetched: true,
            changed: true,
            checks: vec![FolderCheck {
                command: "npm test".into(),
                ok: true,
                code: Some(0),
                ms: 10,
                tail: String::new(),
            }],
            usd_micro: Some(1_200_000),
            billing: Some("key".into()),
            ..FolderTold::default()
        };
        assert_eq!(summary_of(words, &told), "Ветка duowave/kontakty, проверки прошли, $1.20 (оценка).");
        told.billing = Some("plan".into());
        told.checks[0].ok = false;
        assert_eq!(
            summary_of(words, &told),
            "Ветка duowave/kontakty, проверка «npm test» не прошла, ≈ $1.20 по подписке."
        );
        told.usd_micro = None;
        told.checks.clear();
        told.checks_skipped = true;
        assert!(summary_of(words, &told).contains("цена неизвестна"));
        told.why = Some(FolderWhy::Auth);
        assert_eq!(why_of(words, &told), "Claude Code не вошёл");
    }
}
