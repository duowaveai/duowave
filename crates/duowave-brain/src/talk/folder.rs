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
use super::works::{End, NewWork, Offer, Question, Work, WorkResult, WorkState, FOLDER};
use crate::memory::text::Lang;
use crate::router::budgets::{Budget, Measure, Spending};
use crate::router::{Spent, WorkerAsk, WorkerDeal};
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
    /// The set chosen for this work (router/sets.rs): said in the phrase or picked in the panel.
    pub set: Option<String>,
    /// How our system does it (stage 6): `review`, `plan`; none — at once, one run.
    pub way: Option<String>,
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
    /// Its budget is used up: it waits for «добавить» or «остановить».
    WaitsBudget,
    /// «С планом»: the plan waits for «да».
    WaitsPlan,
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
    /// The regulator «Что может уходить из дома» keeps the agent's model in.
    Closed,
    /// «С проверкой»: the review's blocking remarks were not fixed in its rounds.
    Review,
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
            FolderWhy::Closed => "closed",
            FolderWhy::Review => "review",
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
            FolderWhy::Closed,
            FolderWhy::Review,
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
    /// «С проверкой»: `clean`, `fixed:<n>`, or the remarks left.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
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
    /// Its budget is used up before the next run: the person is asked to add as much again or stop; the work waits.
    Budget { id: String, deal: Box<WorkerDeal> },
    /// «С планом» (stage 6 of docs/plans/work-systems.md): the plan is ready; the work waits for «да».
    Plan { id: String, plan: String },
    /// A run ended: its spending, a deal to close.
    Ran { id: String, n: u32, spent: Spent },
    /// The work ended.
    Ended { id: String, end: FolderEnd },
}

pub type PortListener = Arc<dyn Fn(PortEvent) + Send + Sync>;

/// The helper's works in folders, as the rest of the gateway sees them (docs/plans/router-and-works.md, 6–9). Behind it
/// stands the built-in helper's ability `works` (src/works, helpers/builtin.rs) — in the computer's build the same
/// program.
pub trait WorksPort: Send + Sync {
    /// A folder's name by its id; None — no such folder.
    fn folder_name(&self, folder: &str) -> Option<String>;
    /// The work folders, by id and name, in their order: a folder said by voice is found among them.
    fn folders(&self) -> Vec<(String, String)>;
    /// The agents there are, by id and name: Claude Code first, then the cards'. Whether one is ready is its run's to
    /// say.
    fn agents(&self) -> Vec<(String, String)>;
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
    /// A work waiting on its budget's question: its budget grew, it asks the door again. False — it does not wait.
    fn nudge(&self, id: &str) -> bool;
    fn told(&self, id: &str) -> Option<FolderTold>;
    /// A run's deal, once the router wrote it.
    fn dealt(&self, id: &str, n: u32, deal: i64);
    fn listen(&self, listener: PortListener);
    /// The cheaper set a work's agent goes on with after a limit (stage 5 of docs/plans/work-systems.md): its id, when
    /// that set gives the agent another model.
    fn cheaper(&self, _id: &str) -> Option<String> {
        None
    }
    /// The work goes on with that set, now: its model is the set's for its agent.
    fn switch_set(&self, _id: &str, _set: &str) -> Result<(), String> {
        Err("no sets here".into())
    }
    /// The work folder a work is in, by its id.
    fn folder_of(&self, _id: &str) -> Option<String> {
        None
    }
}

/// Where the runs' deals go (app.rs): closed into the router's journal by `work:folder:<id>` — the deal's id — and
/// who answers a work going on.
#[derive(Clone)]
pub struct FolderDeals {
    pub close: DealCloser,
    pub heard: ModelHeard,
    /// A work's own budget given: by the work's id and when it began. False — not kept.
    pub raise: BudgetGiver,
}

/// A work's own budget given (the router's `work_budgets`): the work, when it began, the budget.
pub type BudgetGiver = Arc<dyn Fn(&str, u64, &Budget) -> bool + Send + Sync>;

/// The router's door for a run of a work in a folder (app.rs): by the work's deal key `work:folder:<id>`, the ask with
/// its agent, model and folder's budget; the door adds what the work spent and its own budget.
pub type WorkerDoor = Arc<dyn Fn(&str, WorkerAsk) -> WorkerDeal + Send + Sync>;

/// A run's spending closed into a deal: its id, none when it was not written.
pub type DealCloser = Arc<dyn Fn(&Spent) -> Option<i64> + Send + Sync>;
/// Who answers a work going on: its deal key, the agent's way, the model.
pub type ModelHeard = Arc<dyn Fn(&str, &str, &str) + Send + Sync>;

/// Who gives `works` now (app.rs: the helper the helpers pick — this computer's built-in one): its port, none — no
/// helper does works in folders here.
pub type WorksGiver = Arc<dyn Fn() -> Option<Arc<dyn WorksPort>> + Send + Sync>;

/// How much of the agent's last message is read aloud for «подробнее»: the rest is in the panel.
const MORE_CHARS: usize = 700;

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
    /// The budget that held when a work asked «добавить ещё или остановить?»: by the work's id at the helper.
    asked_budgets: Mutex<HashMap<String, Budget>>,
    /// Who gives `works` now; unset — the port attached (a test's).
    giver: OnceLock<WorksGiver>,
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
            asked_budgets: Mutex::new(HashMap::new()),
            giver: OnceLock::new(),
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

    /// Who gives `works`: asked before each new work.
    pub fn set_giver(&self, giver: WorksGiver) {
        let _ = self.giver.set(giver);
    }

    /// Where a new work begins: the helper that gives `works` now. The works going on are followed through the port
    /// attached, whoever gives the next.
    pub fn giving(&self) -> Option<Arc<dyn WorksPort>> {
        match self.giver.get() {
            Some(giver) => giver(),
            None => self.port(),
        }
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
            PortEvent::Budget { id, deal } => {
                locked(&self.asked_budgets).insert(id.clone(), deal.budget.clone());
                layer.folder_budget_asks(&id, &deal);
            }
            PortEvent::Plan { id, plan } => layer.folder_plan_asks(&id, &plan),
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
                locked(&self.asked_budgets).remove(&id);
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

    /// «Добавить» to a work's budget question: its own budget is the one that held, as much again on top; the work
    /// asks the door again. The budget added, or None — nothing asked, or not kept.
    pub(super) fn raise(&self, work: &Work) -> Option<Budget> {
        let target = work.target.as_deref()?;
        let held = locked(&self.asked_budgets).get(target).cloned()?;
        let raised = held.raised(&held);
        let given = self.deals.get().is_some_and(|deals| (deals.raise)(&work.id, work.started_at, &raised));
        if !given {
            return None;
        }
        locked(&self.asked_budgets).remove(target);
        if let Some(port) = self.port() {
            port.nudge(target);
        }
        Some(held)
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
        FolderDoing::WaitsBudget => words.waits_budget.to_string(),
        FolderDoing::WaitsPlan => words.waits_plan.to_string(),
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

/// An amount of a budget's measure as people read it: «$5.00», «200000 токенов».
pub fn amount(words: &FolderWords, measure: Measure, value: f64) -> String {
    match measure {
        Measure::Usd => dollars((value.max(0.0) * 1_000_000.0).round() as u64),
        Measure::Tokens => fill(words.tokens, &[("n", &format!("{}", value.max(0.0).round() as u64))]),
    }
}

/// What a work spent, in a budget's measure.
fn spent_amount(words: &FolderWords, budget: &Budget, spent: &Spending) -> String {
    amount(words, budget.measure, budget.used(spent))
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
    // «С проверкой» (stage 6): what the review came to.
    match told.review.as_deref() {
        Some("clean") => parts.push(words.review_clean.to_string()),
        Some(fixed) => {
            if let Some(n) = fixed.strip_prefix("fixed:") {
                parts.push(fill(words.review_fixed, &[("n", n)]));
            }
        }
        None => {}
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
        Some(FolderWhy::Closed) => fill(words.why_closed, &[("agent", agent)]),
        Some(FolderWhy::Turns) => words.why_turns.to_string(),
        Some(FolderWhy::Refused) => words.why_refused.to_string(),
        Some(FolderWhy::Limit) => fill(words.why_limit, &[("agent", agent)]),
        Some(FolderWhy::NoAgent) => fill(words.why_agent, &[("why", why)]),
        Some(FolderWhy::Folder) => words.why_folder.to_string(),
        Some(FolderWhy::Git) => fill(words.why_git, &[("why", why)]),
        Some(FolderWhy::Review) => fill(words.why_review, &[("why", why)]),
        Some(FolderWhy::Failed) | None => words.why_failed.to_string(),
    }
}

/// A set said in a phrase (stage 5): its id, and the task without its words. None — no set said.
pub(super) fn set_said(pack: &super::pack::Pack, task: &str) -> (Option<String>, String) {
    let words = &pack.work.folder;
    let found = [(crate::router::sets::ECONOMY, words.set_economy), (crate::router::sets::QUALITY, words.set_quality)];
    for (id, patterns) in found {
        for pattern in patterns {
            let Ok(rule) = regex::Regex::new(&format!("(?i){pattern}")) else { continue };
            if !rule.is_match(task) {
                continue;
            }
            let left = rule.replace_all(task, " ");
            let left = left.split_whitespace().collect::<Vec<_>>().join(" ").replace(" ,", ",");
            let left = left.trim_matches(|c: char| c == ',' || c.is_whitespace()).to_string();
            return (Some(id.to_string()), if left.is_empty() { task.to_string() } else { left });
        }
    }
    (None, task.to_string())
}

/// A way said in a phrase (stage 6): «с проверкой» — `review`, «сначала план» — `plan`; the task without its words.
pub(super) fn way_said(pack: &super::pack::Pack, task: &str) -> (Option<String>, String) {
    let words = &pack.work.folder;
    for (id, patterns) in [("plan", words.way_plan), ("review", words.way_review)] {
        for pattern in patterns {
            let Ok(rule) = regex::Regex::new(&format!("(?i){pattern}")) else { continue };
            if !rule.is_match(task) {
                continue;
            }
            let left = rule.replace_all(task, " ");
            let left = left.split_whitespace().collect::<Vec<_>>().join(" ").replace(" ,", ",");
            let left = left.trim_matches(|c: char| c == ',' || c.is_whitespace()).to_string();
            return (Some(id.to_string()), if left.is_empty() { task.to_string() } else { left });
        }
    }
    (None, task.to_string())
}

/// The answers of a question in a row: «Подождать, остановить или передать bb».
fn choices(words: &FolderWords, all: Vec<String>) -> String {
    match all.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{}{}{last}", rest.join(", "), words.choice_or),
        Some((last, _)) => last.clone(),
        None => String::new(),
    }
}

/// A set's title as said: the bundled ones in the language's words, one's own by its id.
fn set_title<'a>(words: &'a FolderWords, set: &'a str) -> &'a str {
    match set {
        crate::router::sets::ECONOMY => words.set_titles[0],
        crate::router::sets::USUAL => words.set_titles[1],
        crate::router::sets::QUALITY => words.set_titles[2],
        own => own,
    }
}

/// A work's title from its task: its first sentence, short, with a capital.
pub fn title_of(task: &str) -> String {
    let first = task.trim().lines().next().unwrap_or_default();
    // A sentence ends at a stop followed by a space or the end: «poems2.md» is a word (the hand check of 27.11).
    let chars: Vec<(usize, char)> = first.char_indices().collect();
    let end = chars
        .iter()
        .enumerate()
        .find(|(at, (_, c))| {
            matches!(c, '.' | '!' | '?') && chars.get(at + 1).is_none_or(|(_, next)| next.is_whitespace())
        })
        .map_or(first.len(), |(_, (byte, _))| *byte);
    let sentence = first[..end].trim();
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

/// Words as a phrase compares them: lower case, «ё» as «е», letters and digits only.
fn plain(text: &str) -> String {
    let text = text.to_lowercase().replace('ё', "е");
    let words: Vec<String> =
        text.split(|c: char| !c.is_alphanumeric()).filter(|word| !word.is_empty()).map(str::to_string).collect();
    words.join(" ")
}

/// A name said is the name there: the same words, one inside the other, or alike by their stems («в сайте» —
/// «Сайт»).
fn names_the(said: &str, id: &str, name: &str) -> bool {
    let (said, name_plain) = (plain(said), plain(name));
    if said.is_empty() {
        return false;
    }
    if said == name_plain || said == plain(id) {
        return true;
    }
    let padded = |text: &str| format!(" {text} ");
    if !name_plain.is_empty()
        && (padded(&said).contains(&padded(&name_plain)) || padded(&name_plain).contains(&padded(&said)))
    {
        return true;
    }
    let (wanted, there) = (super::work_tools::stems(&said), super::work_tools::stems(&name_plain));
    !wanted.is_empty() && !there.is_empty() && super::work_tools::alike(&there, &wanted)
}

/// Names as a list said aloud: «„Сайт“, „Бот“ или „Блог“».
fn listed(names: &[String], pack: &pack::Pack) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("«{name}»")).collect();
    match quoted.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{}{}{last}", rest.join(", "), pack.work.say.or),
        Some((last, _)) => last.clone(),
        None => String::new(),
    }
}

/// The folder a phrase means: by its name; no name — the only one there is, else «в какой папке?».
fn find_folder(folders: &[(String, String)], said: Option<&str>, pack: &pack::Pack) -> Result<String, Refusal> {
    let words = &pack.work.folder;
    if folders.is_empty() {
        return Err(Refusal::Said(words.no_folders.to_string()));
    }
    let names: Vec<String> = folders.iter().map(|(_, name)| name.clone()).collect();
    let said = said.map(str::trim).filter(|said| !said.is_empty());
    let Some(said) = said else {
        return match folders {
            [(id, _)] => Ok(id.clone()),
            _ => Err(Refusal::Said(fill(words.which_folder, &[("folders", &listed(&names, pack))]))),
        };
    };
    let found: Vec<&(String, String)> = folders.iter().filter(|(id, name)| names_the(said, id, name)).collect();
    match found.as_slice() {
        [(id, _)] => Ok(id.clone()),
        // Two alike: the one named exactly, else the question.
        [_, ..] => found
            .iter()
            .find(|(id, name)| plain(name) == plain(said) || plain(id) == plain(said))
            .map(|(id, _)| id.clone())
            .ok_or_else(|| {
                let alike: Vec<String> = found.iter().map(|(_, name)| name.clone()).collect();
                Refusal::Said(fill(words.which_folder, &[("folders", &listed(&alike, pack))]))
            }),
        [] => Err(Refusal::Said(fill(words.no_folder, &[("name", said), ("folders", &listed(&names, pack))]))),
    }
}

/// The agent a phrase asks for, by its name («клод», «дроид» — a word of it is enough).
fn find_agent(agents: &[(String, String)], said: &str, pack: &pack::Pack) -> Result<String, Refusal> {
    let wanted = plain(said);
    let found = agents.iter().find(|(id, title)| {
        let (id, title) = (plain(id), plain(title));
        wanted == id
            || wanted == title
            || title.split(' ').any(|word| word.len() > 2 && wanted.split(' ').any(|said| said == word))
    });
    match found {
        Some((id, _)) => Ok(id.clone()),
        None => {
            let names: Vec<String> = agents.iter().map(|(_, title)| title.clone()).collect();
            Err(Refusal::Said(fill(pack.work.folder.no_agent, &[("name", said), ("agents", &listed(&names, pack))])))
        }
    }
}

/// An agent's message as it reads aloud: no marks of formatting, no code, the first `MORE_CHARS` at a sentence's end.
fn spoken(text: &str) -> String {
    let mut lines = Vec::new();
    let mut code = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            code = !code;
            continue;
        }
        if code || trimmed.is_empty() {
            continue;
        }
        let line = trimmed.trim_start_matches(['#', '-', '*', '>', ' ']).trim();
        let line: String = line.chars().filter(|c| !matches!(c, '*' | '`' | '_' | '#')).collect();
        if !line.is_empty() {
            lines.push(line);
        }
    }
    // A heading or a list's line without its own end is a sentence of its own aloud.
    let sentences: Vec<String> = lines
        .into_iter()
        .map(|line| if line.ends_with(['.', '!', '?', ':', ';', '…']) { line } else { format!("{line}.") })
        .collect();
    let text = sentences.join(" ");
    if text.chars().count() <= MORE_CHARS {
        return text;
    }
    let cut: String = text.chars().take(MORE_CHARS).collect();
    match cut.rfind(['.', '!', '?']) {
        Some(end) if end > MORE_CHARS / 3 => cut[..=end].to_string(),
        _ => format!("{}…", cut.trim_end()),
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
            return Ok(Started { target: target.to_string(), title: work.title.clone(), note: None });
        }
        let start =
            locked(&self.staged).remove(&work.id).ok_or_else(|| "nothing to begin: the work was lost".to_string())?;
        let target = port.start(start)?;
        Ok(Started { target, title: work.title.clone(), note: None })
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
            Ok(Started { target: target.to_string(), title, note: None })
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
    /// The set chosen for this work: said in the phrase, or the panel's switch.
    pub set: Option<String>,
    /// How our system does it: `review`, `plan`; none — at once.
    pub way: Option<String>,
}

impl TalkLayer {
    /// «Новая работа» in a folder (stage 3, from the panel): a work of the kind `folder`, at once or in the queue — one
    /// work in folders at a time. The words said, and the work.
    pub fn start_folder(&self, origin: Origin, ask: FolderAsk) -> Result<(String, Work), Refusal> {
        let lang = self.lang(&origin.device);
        let words = &pack::of(lang).work.folder;
        let executor = self.folders().ok_or_else(|| Refusal::Said(words.no_computer.to_string()))?;
        let port = executor.giving().ok_or_else(|| Refusal::Said(words.no_computer.to_string()))?;
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
                set: ask.set,
                way: ask.way,
            },
        );
        if let Err(refused) = wait {
            // Another work in folders goes on — or, by voice, a new work began on this device a minute ago.
            let (wait, said) = match refused {
                super::works::Refused::TooSoon(wait) => {
                    let say = &pack::of(lang).work.say;
                    (Some(wait), fill(say.queued_in, &[("in", &say.duration(wait)), ("title", &title)]))
                }
                _ => (None, fill(words.queued, &[("title", &title)])),
            };
            self.enqueue(&work, wait);
            let work = self.works.get(&work.id).unwrap_or(work);
            return Ok((said, work));
        }
        let work = self.launch(work)?;
        let said = fill(words.started, &[("title", &work.title), ("folder", &folder_name)]);
        Ok((said, work))
    }

    /// A work in a folder begun by a phrase (stage 5): `work_start` with a folder, the reader's or the rules'. The
    /// folder and the agent are found by the names said; no folder said — the only one, or «в какой папке?». Only the
    /// home's admin begins one, and only while a helper gives `works`. Then as from the panel: at once or in the queue,
    /// the day's new works by voice counted with the things'.
    pub fn start_folder_said(
        &self,
        origin: Origin,
        task: &str,
        folder: Option<&str>,
        agent: Option<&str>,
    ) -> Result<String, Refusal> {
        let pack = pack::of(self.lang(&origin.device));
        let words = &pack.work.folder;
        let task = task.trim();
        if task.is_empty() {
            return Err(Refusal::NoBrief);
        }
        let Some(port) = self.folders().and_then(|executor| executor.giving()) else {
            // A station as the hub (stage 9): the folders are a Турбо's, its works led by its own systems over the
            // home link.
            return self.start_on_turbo(origin, task, folder, agent);
        };
        if !origin.admin() {
            return Err(Refusal::Said(words.only_admin.to_string()));
        }
        let folder = find_folder(&port.folders(), folder, pack)?;
        // A set said in the phrase («сделай подешевле …», stage 5 of docs/plans/work-systems.md): the work's own, and
        // its words leave the task.
        let (set, task) = set_said(pack, task);
        let (way, task) = way_said(pack, &task);
        // The model may drop these words from the task (the hand check of 27.11: «сделай подешевле» began on opus):
        // the person's phrase itself says them too.
        let phrase =
            self.last_heard.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&origin.device).cloned();
        let set = set.or_else(|| phrase.as_deref().and_then(|phrase| set_said(pack, phrase).0));
        let way = way.or_else(|| phrase.as_deref().and_then(|phrase| way_said(pack, phrase).0));
        // «Сначала план, потом поправь …»: the model may keep «потом» in the task (the hand check of 27.11).
        let task = match way.as_deref() {
            Some("plan") => {
                let lowered = task.to_lowercase();
                ["потом ", "затем ", "после этого ", "then "]
                    .iter()
                    .find_map(|word| lowered.starts_with(word).then(|| task[word.len()..].trim().to_string()))
                    .filter(|rest| !rest.is_empty())
                    .unwrap_or(task)
            }
            _ => task,
        };
        let task = task.as_str();
        // Who gets it (docs/plans/work-systems.md, owner decision 8): a system named for this one work, the folder's
        // own, the home's default; ours when none of them, or when the name said is one of our agents.
        let mut task = task.to_string();
        if let Some(systems) = self.systems() {
            let said = agent.map(str::trim).filter(|agent| !agent.is_empty());
            // The model may leave the system in the task — «T3 Code: допиши …», «дай T3 Code: …» (the hand check of
            // 27.11): a system named at its head is the one named, and leaves the task.
            let in_task = match said {
                None => system_in_task(&systems, &task).or_else(|| {
                    // Nor in the task: one named in the person's phrase itself.
                    let heard = self.last_heard.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    let phrase = heard.get(&origin.device)?;
                    systems.named_in(phrase).map(|id| (id, task.clone()))
                }),
                Some(_) => None,
            };
            let named = match in_task {
                Some((id, rest)) => {
                    task = rest;
                    Some(id)
                }
                None => said.and_then(|said| systems.named(said)),
            };
            let our_agent = said.is_some() && named.is_none();
            let who = systems.who_gets(named.as_deref(), &folder);
            if !our_agent && who != super::system::DUOWAVE {
                let folder_name = port.folder_name(&folder).unwrap_or_default();
                return self.start_in_system(origin, &who, &folder, &folder_name, &task, false);
            }
        }
        let task = task.as_str();
        let agent = match agent.map(str::trim).filter(|agent| !agent.is_empty()) {
            Some(said) => Some(find_agent(&port.agents(), said, pack)?),
            None => None,
        };
        let ask = FolderAsk { folder, task: task.to_string(), agent, model: None, set, way };
        self.start_folder(origin, ask).map(|(said, _)| said)
    }

    /// A work said to a hub with no folders of its own (stage 9 of docs/plans/work-systems.md): the Турбо's folder by
    /// its name, its system by that computer's setup (a system named in the phrase over it), begun there over the home
    /// link. No Турбо gives works — «нужен компьютер».
    fn start_on_turbo(
        &self,
        origin: Origin,
        task: &str,
        folder: Option<&str>,
        agent: Option<&str>,
    ) -> Result<String, Refusal> {
        let pack = pack::of(self.lang(&origin.device));
        let words = &pack.work.folder;
        let systems = self.systems().ok_or_else(|| Refusal::Said(words.no_computer.to_string()))?;
        let remote = systems.remote_folders();
        if remote.is_empty() {
            return Err(Refusal::Said(words.no_computer.to_string()));
        }
        if !origin.admin() {
            return Err(Refusal::Said(words.only_admin.to_string()));
        }
        let named: Vec<(String, String)> = remote.iter().map(|kept| (kept.folder.clone(), kept.name.clone())).collect();
        let folder = find_folder(&named, folder, pack)?;
        let kept = remote.iter().find(|kept| kept.folder == folder).cloned().unwrap_or_default();
        let (set, task) = set_said(pack, task);
        let _ = set;
        let said = agent.map(str::trim).filter(|agent| !agent.is_empty());
        // A system named in the phrase, of the same Турбо: «а это дай T3» — `t3code@<entry>`.
        let named = said.and_then(|said| systems.named(said)).filter(|id| id.ends_with(&format!("@{}", kept.entry)));
        let who = named.unwrap_or(kept.system.clone());
        self.start_in_system(origin, &who, &folder, &kept.name, &task, false)
    }

    /// The folders' names said in a phrase's reading: those there are, while a helper gives `works`.
    pub(super) fn folder_names(&self) -> Vec<String> {
        let port = self.folders().and_then(|executor| executor.giving());
        port.map(|port| port.folders().into_iter().map(|(_, name)| name).collect()).unwrap_or_default()
    }

    /// «Подробнее» to a work's «Готово»: a work in a folder — the agent's last message, its beginning, as it reads
    /// aloud; a background work — the rest of its retelling.
    pub(super) fn more_of(&self, work: &Work) -> Option<String> {
        if matches!(work.kind.as_str(), FOLDER | super::works::SYSTEM) {
            let told = self.executor(&work.executor)?.told(work)?;
            return told.more.as_deref().map(spoken).filter(|more| !more.is_empty());
        }
        self.background().and_then(|executor| executor.more(work))
    }

    /// A plan's limit holds a work: its open question «подождать или остановить?» — silence waits too.
    pub(super) fn folder_waits(&self, target: &str, until: Option<u64>, agent: &str) {
        let Some(work) = self.works.of_target(FOLDER_EXECUTOR, target).filter(Work::going) else { return };
        let words = &pack::of(self.lang(work.device())).work.folder;
        let (more, offers): (Vec<String>, Vec<Offer>) = self.more_choices(&work, target, true).into_iter().unzip();
        let text = match until {
            _ if !more.is_empty() => {
                let until = until.map(|until| fill(words.limit_until, &[("until", &clock(until))])).unwrap_or_default();
                let all = [vec![words.choice_wait.to_string(), words.choice_stop.to_string()], more].concat();
                fill(words.limit_q_choices, &[("agent", agent), ("until", &until), ("choices", &choices(words, all))])
            }
            Some(until) => fill(words.limit_q_until, &[("agent", agent), ("until", &clock(until))]),
            None => fill(words.limit_q, &[("agent", agent)]),
        };
        let changed = self.works.update(&work.id, |work| {
            let mut question = Question::new(text.clone(), now_ms() + ASK_FOR_MS, Kind::Wait);
            question.offers = offers.clone();
            work.question = Some(question);
        });
        if let Some(work) = changed {
            self.told(&work, "asks");
        }
    }

    /// A work's budget is used up before its next run: its open question «добавить ещё или остановить?». Silence
    /// leaves it waiting — nothing is spent while nobody answers.
    pub(super) fn folder_budget_asks(&self, target: &str, deal: &WorkerDeal) {
        let Some(work) = self.works.of_target(FOLDER_EXECUTOR, target).filter(Work::going) else { return };
        let words = &pack::of(self.lang(work.device())).work.folder;
        let limit = amount(words, deal.budget.measure, deal.budget.limit);
        let spent = spent_amount(words, &deal.budget, &deal.spent);
        // A cheaper set does not help a budget used up: only another system is offered.
        let (more, offers): (Vec<String>, Vec<Offer>) = self.more_choices(&work, target, false).into_iter().unzip();
        let text = if more.is_empty() {
            fill(words.budget_q, &[("title", &work.title), ("limit", &limit), ("spent", &spent)])
        } else {
            let first = vec![fill(words.choice_raise, &[("limit", &limit)]), words.choice_stop.to_string()];
            let all = choices(words, [first, more].concat());
            fill(
                words.budget_q_choices,
                &[("title", &work.title), ("limit", &limit), ("spent", &spent), ("choices", &all)],
            )
        };
        let changed = self.works.update(&work.id, |work| {
            let mut question = Question::new(text.clone(), now_ms() + ASK_FOR_MS, Kind::Budget);
            question.offers = offers.clone();
            work.question = Some(question);
        });
        if let Some(work) = changed {
            self.told(&work, "asks");
        }
    }

    /// What a limit's question offers beyond its first two answers (stage 5): the cheaper set when it gives the work's
    /// agent another model (`switch`), and the first other system that can take the work now. Nothing more — none.
    fn more_choices(&self, work: &Work, target: &str, switch: bool) -> Vec<(String, Offer)> {
        let words = &pack::of(self.lang(work.device())).work.folder;
        let mut more = Vec::new();
        let port = self.folders().and_then(|executor| executor.port()).filter(|_| switch);
        if let Some(set) = port.as_ref().and_then(|port| port.cheaper(target)) {
            let title = set_title(words, &set).to_string();
            let offer = Offer { kind: "switch".into(), id: set.clone(), title: title.clone() };
            more.push((fill(words.choice_switch, &[("set", &title)]), offer));
        }
        if let Some((id, title)) = self.systems().and_then(|systems| systems.takers().into_iter().next()) {
            let said = fill(words.choice_hand, &[("system", &title)]);
            more.push((said, Offer { kind: "hand".into(), id, title }));
        }
        more
    }

    /// «Перейти на «Экономно»» to a limit's question: the work goes on now with the cheaper set. The words said; none —
    /// nothing cheaper.
    pub(super) fn folder_switch(&self, id: &str) -> Option<String> {
        let work = self.works.get(id).filter(Work::going)?;
        let target = work.target.clone()?;
        let words = &pack::of(self.lang(work.device())).work.folder;
        let port = self.folders()?.port()?;
        let set = port.cheaper(&target)?;
        port.switch_set(&target, &set).ok()?;
        if let Some(work) = self.works.update(id, |work| work.question = None) {
            self.told(&work, "answered");
        }
        Some(fill(words.switched, &[("set", set_title(words, &set))]))
    }

    /// «Передай bb» to a limit's question (stage 5): a new work in that system, with this one's task and what is done;
    /// this one stops. The words said.
    pub(super) async fn hand_over(&self, origin: Origin, id: &str, to: &str) -> Result<String, Refusal> {
        let work = self.works.get(id).ok_or_else(|| Refusal::Failed("no such work".into()))?;
        let pack = pack::of(self.lang(work.device()));
        let words = &pack.work.folder;
        if !origin.admin() {
            return Err(Refusal::Said(words.only_admin.to_string()));
        }
        let none = || Refusal::Said(fill(words.no_system, &[("name", to)]));
        let systems = self.systems().ok_or_else(none)?;
        let who = systems.named(to).ok_or_else(none)?;
        let port = self.folders().and_then(|executor| executor.port()).ok_or_else(none)?;
        let target = work.target.clone().unwrap_or_default();
        let folder = port.folder_of(&target).ok_or_else(|| Refusal::Failed(words.why_folder.to_string()))?;
        let folder_name = port.folder_name(&folder).unwrap_or_default();
        let mut task = work.brief.clone();
        if let Some(last) = port.told(&target).and_then(|told| told.last).filter(|last| !last.trim().is_empty()) {
            let last: String = last.chars().take(2000).collect();
            task = format!("{task}\n\n{}: {last}", words.handed_note);
        }
        let label = format!("{}{id}", pack.work.label);
        if work.going() {
            self.act(&origin, "stop", Some(&label)).await?;
        }
        self.start_in_system(origin, &who, &folder, &folder_name, &task, true)?;
        let title = systems.title_of(&who);
        Ok(fill(words.handed, &[("system", &title), ("title", &work.title)]))
    }

    /// «С планом»: the plan is said with «Делаю?»; the work waits for the answer (silence waits too).
    pub(super) fn folder_plan_asks(&self, target: &str, plan: &str) {
        let Some(work) = self.works.of_target(FOLDER_EXECUTOR, target).filter(Work::going) else { return };
        let words = &pack::of(self.lang(work.device())).work.folder;
        let plan = spoken(plan);
        let plan: String = plan.chars().take(400).collect();
        let text = fill(words.plan_q, &[("title", &work.title), ("plan", plan.trim())]);
        let changed = self.works.update(&work.id, |work| {
            work.question = Some(Question::new(text.clone(), now_ms() + ASK_FOR_MS, Kind::Plan));
        });
        if let Some(work) = changed {
            self.told(&work, "asks");
        }
    }

    /// «Да» to the plan: the question goes, the work goes on by the plan. The words said.
    pub(super) fn folder_plan_go(&self, id: &str) -> Option<String> {
        let work = self.works.get(id).filter(Work::going)?;
        let words = &pack::of(self.lang(work.device())).work.folder;
        let target = work.target.clone()?;
        self.folders()?.port()?.nudge(&target).then_some(())?;
        if let Some(work) = self.works.update(id, |work| work.question = None) {
            self.told(&work, "answered");
        }
        Some(words.plan_go.to_string())
    }

    /// «Добавить» to a work's budget question: the question goes, the work goes on. The words said.
    pub(super) fn folder_raise(&self, id: &str) -> Option<String> {
        let work = self.works.get(id).filter(Work::going)?;
        let words = &pack::of(self.lang(work.device())).work.folder;
        let held = self.folders()?.raise(&work)?;
        if let Some(work) = self.works.update(id, |work| work.question = None) {
            self.told(&work, "answered");
        }
        Some(fill(words.budget_added, &[("limit", &amount(words, held.measure, held.limit))]))
    }

    /// A work's own budget was set from the panel: one waiting on its budget's question asks the door again — it
    /// goes on if the new budget lets it, or asks again.
    pub fn budget_changed(&self, id: &str) {
        let Some(work) = self.works.get(id).filter(Work::going) else { return };
        if work.question.as_ref().and_then(|question| question.kind) != Some(Kind::Budget) {
            return;
        }
        let (Some(target), Some(executor)) = (work.target.as_deref(), self.folders()) else { return };
        locked(&executor.asked_budgets).remove(target);
        if let Some(work) = self.works.update(id, |work| work.question = None) {
            self.told(&work, "answered");
        }
        if let Some(port) = executor.port() {
            port.nudge(target);
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
        // Its end with the agent's last message asks whether to read it: «подробнее» reads it (stage 5).
        let more = told.more.as_deref().is_some_and(|more| !spoken(more).is_empty());
        let (line, kind) = match work.state {
            WorkState::Failed => (fill(words.failed, &[("title", &title), ("why", &why)]), None),
            WorkState::Unfinished => {
                let line = fill(words.unfinished, &[("title", &title), ("why", &why), ("summary", &summary)]);
                if more {
                    (format!("{line} {}", words.more_q), Some(Kind::More))
                } else {
                    (format!("{line} {}", words.in_panel), None)
                }
            }
            _ => {
                let line = fill(words.ready, &[("title", &title), ("summary", &summary)]);
                if more {
                    (format!("{line} {}", words.more_q), Some(Kind::More))
                } else {
                    (format!("{line} {}", words.in_panel), None)
                }
            }
        };
        let until = now_ms() + super::asked::ANSWER_FOR.as_millis() as u64;
        let question = kind.map(|kind| Question::new(line.clone(), until, kind));
        let state = work.state;
        let Some(work) = self.works.update(&work.id, |work| {
            work.result = Some(WorkResult {
                line: line.clone(),
                end: None,
                summary: told.summary.clone(),
                data: told.data.clone(),
            });
            work.question = question.clone();
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

/// A system named at the head of a task — «T3 Code: допиши …», «дай bb, …» —: its id and the task without it.
fn system_in_task(systems: &super::systems::SystemExecutor, task: &str) -> Option<(String, String)> {
    let (head, rest) = task.split_once([':', ','])?;
    let head = head.trim();
    let lowered = head.to_lowercase();
    let name = ["дай ", "отдай ", "поручи ", "передай ", "попроси ", "give ", "ask "]
        .iter()
        .find_map(|verb| lowered.strip_prefix(verb).map(|_| head[verb.len()..].trim()))
        .unwrap_or(head);
    let id = systems.named(name)?;
    let rest = rest.trim();
    (!rest.is_empty()).then(|| (id, rest.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A helper's works as a test sees them: two folders, two agents; what it was asked to begin; its result.
    #[derive(Default)]
    struct Port {
        started: Mutex<Vec<FolderStart>>,
        told: Mutex<Option<FolderTold>>,
        listener: Mutex<Option<PortListener>>,
        /// The cheaper set a limit may offer (stage 5), and the switches asked.
        cheaper: Mutex<Option<String>>,
        switched: Mutex<Vec<(String, String)>>,
    }

    impl WorksPort for Port {
        fn folder_name(&self, folder: &str) -> Option<String> {
            self.folders().into_iter().find(|(id, _)| id == folder).map(|(_, name)| name)
        }
        fn folders(&self) -> Vec<(String, String)> {
            vec![("sayt".into(), "Сайт".into()), ("bot".into(), "Телеграм-бот".into())]
        }
        fn agents(&self) -> Vec<(String, String)> {
            vec![("claude-code".into(), "Claude Code".into()), ("droid".into(), "Factory Droid".into())]
        }
        fn start(&self, start: FolderStart) -> Result<String, String> {
            let mut started = locked(&self.started);
            started.push(start);
            Ok(format!("w{}", started.len()))
        }
        fn add(&self, _id: &str, _text: &str) -> bool {
            true
        }
        fn can_again(&self, _id: &str) -> Result<String, String> {
            Err("no".into())
        }
        fn again(&self, _id: &str, _text: &str, _resume: &str) -> Result<(), String> {
            Ok(())
        }
        fn doing(&self, _id: &str) -> Option<FolderDoing> {
            Some(FolderDoing::Waits(None))
        }
        fn stop(&self, _id: &str) -> bool {
            true
        }
        fn nudge(&self, _id: &str) -> bool {
            true
        }
        fn told(&self, _id: &str) -> Option<FolderTold> {
            locked(&self.told).clone()
        }
        fn dealt(&self, _id: &str, _n: u32, _deal: i64) {}
        fn listen(&self, listener: PortListener) {
            *locked(&self.listener) = Some(listener);
        }
        fn cheaper(&self, _id: &str) -> Option<String> {
            locked(&self.cheaper).clone()
        }
        fn switch_set(&self, id: &str, set: &str) -> Result<(), String> {
            locked(&self.switched).push((id.to_string(), set.to_string()));
            Ok(())
        }
        fn folder_of(&self, id: &str) -> Option<String> {
            let at: usize = id.trim_start_matches('w').parse().ok()?;
            locked(&self.started).get(at.checked_sub(1)?).map(|start| start.folder.clone())
        }
    }

    /// Ours, as the contract sees it: the two work folders as places.
    struct Ours;

    #[async_trait::async_trait]
    impl crate::talk::system::WorkSystem for Ours {
        fn id(&self) -> String {
            crate::talk::system::DUOWAVE.into()
        }
        fn about(&self) -> crate::talk::system::About {
            crate::talk::system::About {
                id: crate::talk::system::DUOWAVE.into(),
                title: "DuoWave".into(),
                names: Vec::new(),
                plug: "ours".into(),
                protocol: None,
                version: None,
            }
        }
        async fn ready(&self) -> crate::talk::system::SystemReady {
            crate::talk::system::SystemReady::yes()
        }
        fn can(&self) -> crate::talk::system::Can {
            crate::talk::system::Can { places: true, ..crate::talk::system::Can::default() }
        }
        async fn places(&self) -> Vec<crate::talk::system::Place> {
            use crate::talk::system::Place;
            vec![
                Place { id: "sayt".into(), title: "Сайт".into(), path: Some("/work/site".into()) },
                Place { id: "bot".into(), title: "Телеграм-бот".into(), path: Some("/work/bot".into()) },
            ]
        }
    }

    async fn works_of(system: &crate::talk::system::stand_in::StandIn, n: usize) -> Vec<crate::talk::system::Seen> {
        use crate::talk::system::WorkSystem;
        for _ in 0..200 {
            let seen = system.list().await;
            if seen.len() >= n {
                return seen;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("never {n} works in {}", system.id);
    }

    fn said_by(role: Option<crate::core::identity::Role>) -> Origin {
        let mut origin = Origin::computer(super::super::origin::Activator::Button);
        origin.role = role;
        origin
    }

    fn admin() -> Origin {
        said_by(Some(crate::core::identity::Role::Admin))
    }

    async fn until(what: &str, check: impl Fn() -> bool) {
        for _ in 0..200 {
            if check() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("never: {what}");
    }

    /// Stage 5: a phrase begins a work in a folder found by its name — only the admin's, only while a helper gives
    /// `works`; no folder named — the only one, or «в какой папке?»; an agent by its name.
    #[tokio::test]
    async fn a_phrase_begins_a_work_in_the_folder_it_names_only_the_admins_and_only_with_a_helper() {
        let layer = TalkLayer::new(None, crate::core::events::EventBus::new());
        let executor = FolderExecutor::new(&layer);
        let pack = pack::of(Lang::Ru);
        let words = &pack.work.folder;
        let said = |origin: Origin, folder: Option<&str>, agent: Option<&str>| {
            layer
                .start_folder_said(origin, "поправь опечатку в README", folder, agent)
                .unwrap_or_else(|refusal| refusal.for_person(pack))
        };
        assert_eq!(said(admin(), Some("сайт"), None), words.no_computer, "no helper with works here");
        let port = Arc::new(Port::default());
        executor.attach(port.clone());
        let gives = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let (gives, port) = (gives.clone(), port.clone());
            executor.set_giver(Arc::new(move || {
                gives.load(std::sync::atomic::Ordering::Relaxed).then(|| port.clone() as Arc<dyn WorksPort>)
            }));
        }
        assert_eq!(said(admin(), Some("сайт"), None), words.no_computer, "the helpers do not pick one for works");
        gives.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(said(said_by(Some(crate::core::identity::Role::Guest)), Some("сайт"), None), words.only_admin);
        assert_eq!(said(said_by(Some(crate::core::identity::Role::Member)), Some("сайт"), None), words.only_admin);
        assert_eq!(said(said_by(None), Some("сайт"), None), words.only_admin, "nobody known");
        assert_eq!(said(admin(), None, None), "В какой папке: «Сайт» или «Телеграм-бот»?");
        assert_eq!(said(admin(), Some("блог"), None), "Папки «блог» нет. Есть: «Сайт» или «Телеграм-бот».");
        assert_eq!(
            said(admin(), Some("сайт"), Some("codex")),
            "Агента «codex» нет. Есть: «Claude Code» или «Factory Droid»."
        );
        assert!(locked(&port.started).is_empty(), "nothing began");

        let begun = said(admin(), Some("Телеграм бот"), Some("droid"));
        assert_eq!(
            begun,
            "Взяла в работу в папке «Телеграм-бот»: «Поправь опечатку в README». Скажу, когда будет готово."
        );
        let started = locked(&port.started)[0].clone();
        assert_eq!((started.folder.as_str(), started.agent.as_deref()), ("bot", Some("droid")));
        let work = layer.works.of_target(FOLDER_EXECUTOR, "w1").expect("the work");
        assert_eq!((work.kind.as_str(), work.origin.via), (FOLDER, super::super::origin::Via::Voice));
        // One work in folders at a time: the next waits for its turn.
        let next = said(admin(), Some("сайт"), None);
        assert_eq!(
            next,
            "«Поправь опечатку в README»: в очереди — в папках сейчас идёт другая работа. Скажу, когда будет готово."
        );
        assert_eq!(locked(&port.started).len(), 1);
    }

    /// Who gets a work (docs/plans/work-systems.md, stage 4, owner decision 8): ours out of the box; a folder whose
    /// system is bb gives a plain task to bb, in bb's project at the folder's path; a system named in the phrase takes
    /// one work, an agent of ours named keeps it ours; a work of the system going on in the folder asks «Добавить туда
    /// или начать новую?», and both answers do what they say.
    #[tokio::test]
    async fn a_plain_task_goes_to_the_folders_system_and_a_name_gives_one_work_to_another() {
        use crate::talk::system::stand_in::StandIn;
        use crate::talk::system::WorkSystem;

        let layer = TalkLayer::new(None, crate::core::events::EventBus::new());
        let executor = FolderExecutor::new(&layer);
        let port = Arc::new(Port::default());
        executor.attach(port.clone());
        let systems = crate::talk::systems::Systems::new();
        systems.add(Arc::new(Ours));
        let (bb, t3) = (Arc::new(StandIn::full("bb")), Arc::new(StandIn::full("t3")));
        systems.add(bb.clone());
        systems.add(t3.clone());
        let system_executor = crate::talk::systems::SystemExecutor::new(&layer, &systems, false);
        let settings = Arc::new(crate::settings::SettingsService::new(
            Arc::new(crate::storage::Db::memory()),
            crate::core::events::EventBus::new(),
        ));
        system_executor.set_settings(settings.clone());

        layer.start_folder_said(admin(), "поправь опечатку", Some("сайт"), None).unwrap();
        assert_eq!(locked(&port.started).len(), 1, "ours out of the box, as before");

        // «Работы в его проектах»: the folder at bb's project's path is bb's.
        let mut setup = crate::router::who::WorkSystemsSetup::default();
        crate::router::who::assign(&mut setup, "bb", &crate::router::who::Assign::Places(vec!["sayt".into()]));
        settings.set(&crate::router::who::WORK_SYSTEMS, &setup, None).unwrap();
        let said = layer.start_folder_said(admin(), "сделай тёмную тему", Some("сайт"), None).unwrap();
        assert_eq!(
            said,
            "Взяла в работу в папке «Сайт», у Stand-in bb: «Сделай тёмную тему». Скажу, когда будет готово."
        );
        let begun = works_of(&bb, 1).await;
        assert_eq!(begun[0].place.as_deref(), Some("p1"), "bb's project at the folder's path");
        assert_eq!(locked(&port.started).len(), 1, "nothing more for ours");

        // Another folder is still ours (in our queue: one work in folders at a time); a name gives one work to another
        // system; our agent's name keeps it ours.
        let said = layer.start_folder_said(admin(), "обнови зависимости", Some("телеграм бот"), None).unwrap();
        assert!(said.starts_with("«Обнови зависимости»: в очереди"), "{said}");
        let said = layer.start_folder_said(admin(), "проверь ссылки", Some("сайт"), Some("t3")).unwrap();
        assert!(said.contains("у Stand-in t3"), "{said}");
        works_of(&t3, 1).await;
        // The model left the system in the task: it is the one named (the hand check of 27.11) — here asked as t3's
        // work going on in the folder.
        let said = layer.start_folder_said(admin(), "дай t3: проверь картинки", Some("сайт"), None).unwrap();
        assert_eq!(said, "Добавить в «Проверь ссылки» или начать новую?");
        // Nor in the task: the system named in the person's phrase (the model dropped it, the hand check of 27.11).
        let device = admin().device.clone();
        layer.last_heard.lock().unwrap().insert(device, "в папке сайт дай t3 проверь шрифты".into());
        let said = layer.start_folder_said(admin(), "проверь шрифты", Some("сайт"), None).unwrap();
        assert_eq!(said, "Добавить в «Проверь ссылки» или начать новую?", "t3's work goes on there");
        layer.last_heard.lock().unwrap().clear();
        let said = layer.start_folder_said(admin(), "поправь README", Some("сайт"), Some("droid")).unwrap();
        assert!(said.starts_with("«Поправь README»: в очереди"), "{said}");
        assert_eq!((bb.list().await.len(), t3.list().await.len()), (1, 1), "nothing more in the systems");

        // A work of bb goes on in «Сайт»: «Добавить туда или начать новую?»
        let asked = layer.start_folder_said(admin(), "добавь футер", Some("сайт"), None).unwrap();
        assert_eq!(asked, "Добавить в «Сделай тёмную тему» или начать новую?");
        let computer = super::super::origin::COMPUTER;
        let question = layer.asked.open(computer).expect("the question");
        assert_eq!(question.kind, Kind::Similar);
        let rules = super::super::rules::TalkRules::new(&layer);
        let answer = |text: &str| match rules.answer_on(text, computer) {
            Some(super::super::rules::Heard::Answer { answer, .. }) => Some(answer),
            _ => None,
        };
        assert_eq!(answer("добавь туда"), Some(super::super::ready::Answer::Edit));
        assert_eq!(answer("начни новую"), Some(super::super::ready::Answer::New));
        let said = layer.answer(admin(), &question.work, Kind::Similar, false, super::super::ready::Answer::Edit).await;
        assert!(!said.is_empty());
        let first = begun[0].id.clone();
        until("the words reach bb's work", || bb.told(&first) == ["добавь футер"]).await;

        layer.start_folder_said(admin(), "сделай страницу «О нас»", Some("сайт"), None).unwrap();
        let question = layer.asked.open(computer).expect("asked again");
        let said = layer.answer(admin(), &question.work, Kind::Similar, false, super::super::ready::Answer::New).await;
        assert!(said.starts_with("Взяла в работу в папке «Сайт», у Stand-in bb"), "{said}");
        works_of(&bb, 2).await;
    }

    /// Stage 5 of docs/plans/work-systems.md: a set said in the phrase is the work's, and its words leave the task; a
    /// limit's question offers only what is possible — a cheaper set when there is one, another system when one can
    /// take the work; «перейди на экономно» switches, «передай bb» gives the work to bb with what is done and stops it
    /// here.
    /// The model dropped «подешевле» and «сначала план» from the task: the person's phrase says them (the hand check
    /// of 27.11).
    #[tokio::test]
    async fn a_set_and_a_way_the_model_dropped_are_read_from_the_phrase() {
        let layer = TalkLayer::new(None, crate::core::events::EventBus::new());
        let executor = FolderExecutor::new(&layer);
        let port = Arc::new(Port::default());
        executor.attach(port.clone());
        let phrase = "в папке сайт сначала план сделай подешевле футер";
        layer.last_heard.lock().unwrap().insert(admin().device.clone(), phrase.into());
        layer.start_folder_said(admin(), "потом сделай футер", Some("сайт"), None).unwrap();
        let started = locked(&port.started)[0].clone();
        assert_eq!((started.set.as_deref(), started.way.as_deref()), (Some("economy"), Some("plan")));
        assert_eq!(started.task, "сделай футер", "«потом» left with the plan");
    }

    #[tokio::test]
    async fn a_set_said_is_the_works_and_a_limit_offers_only_what_is_possible() {
        use crate::talk::system::stand_in::StandIn;
        let layer = TalkLayer::new(None, crate::core::events::EventBus::new());
        let executor = FolderExecutor::new(&layer);
        let port = Arc::new(Port::default());
        executor.attach(port.clone());
        let said = layer.start_folder_said(admin(), "сделай подешевле тёмную тему", Some("сайт"), None).unwrap();
        let started = locked(&port.started)[0].clone();
        assert_eq!((started.set.as_deref(), started.task.as_str()), (Some("economy"), "сделай тёмную тему"));
        assert!(said.contains("«Сделай тёмную тему»"), "{said}");
        let quality = set_said(pack::of(Lang::Ru), "по максимуму проверь ссылки, и почини");
        assert_eq!(quality, (Some("quality".to_string()), "проверь ссылки, и почини".to_string()));
        assert_eq!(set_said(pack::of(Lang::Ru), "проверь ссылки").0, None);

        // Nothing more is possible: the question as before.
        layer.folder_waits("w1", None, "Claude Code");
        let id = layer.works.of_target(FOLDER_EXECUTOR, "w1").unwrap().id;
        let question = |layer: &TalkLayer| {
            layer.works.get(&id).and_then(|work| work.question).map(|question| question.text).unwrap_or_default()
        };
        assert_eq!(question(&layer), "Лимит Claude Code кончился: подождать или остановить?");

        // A cheaper set for its agent, and a system that can take the work.
        *locked(&port.cheaper) = Some(crate::router::sets::ECONOMY.into());
        let systems = crate::talk::systems::Systems::new();
        systems.add(Arc::new(Ours));
        let bb = Arc::new(StandIn::full("bb"));
        systems.add(bb.clone());
        let mut off = StandIn::full("t3");
        off.ready = false;
        systems.add(Arc::new(off));
        crate::talk::systems::SystemExecutor::new(&layer, &systems, false);
        layer.folder_waits("w1", None, "Claude Code");
        assert_eq!(
            question(&layer),
            "Лимит Claude Code кончился. Подождать, остановить, перейти на «Экономно» или передать Stand-in bb?",
            "t3 is not there: not offered"
        );
        let computer = super::super::origin::COMPUTER;
        layer.act(&admin(), "status", None).await.unwrap();
        let rules = super::super::rules::TalkRules::new(&layer);
        let answer = |text: &str| match rules.answer_on(text, computer) {
            Some(super::super::rules::Heard::Answer { answer, .. }) => Some(answer),
            _ => None,
        };
        assert_eq!(answer("перейди на экономно"), Some(super::super::ready::Answer::Switch));
        assert_eq!(answer("давай подешевле"), Some(super::super::ready::Answer::Switch));
        assert_eq!(answer("передай bb"), Some(super::super::ready::Answer::HandTo { to: "bb".into() }));
        assert_eq!(answer("подожди"), Some(super::super::ready::Answer::Wait));

        let said = layer.answer(admin(), &id, Kind::Wait, false, super::super::ready::Answer::Switch).await;
        assert_eq!(said, "Перешла на «Экономно», продолжаю.");
        assert_eq!(locked(&port.switched).as_slice(), [("w1".to_string(), "economy".to_string())]);

        // «Передай bb»: a guest may not; the admin's starts bb's work with what is done, and this one stops.
        layer.folder_waits("w1", None, "Claude Code");
        *locked(&port.told) = Some(FolderTold {
            last: Some("Тёмная тема готова наполовину.".into()),
            ..FolderTold::default()
        });
        let guest = said_by(Some(crate::core::identity::Role::Guest));
        let hand = || super::super::ready::Answer::HandTo { to: "bb".into() };
        let said = layer.answer(guest, &id, Kind::Wait, false, hand()).await;
        assert_eq!(said, pack::of(Lang::Ru).work.folder.only_admin);
        let said = layer.answer(admin(), &id, Kind::Wait, false, hand()).await;
        assert_eq!(said, "Передала Stand-in bb: «Сделай тёмную тему». Здесь работу остановила.");
        let begun = works_of(&bb, 1).await;
        assert_eq!(begun[0].place.as_deref(), Some("p1"));
        let given = layer.works.all(None, None).into_iter().find(|work| work.kind == super::super::works::SYSTEM);
        let brief = given.map(|work| work.brief).unwrap_or_default();
        assert_eq!(brief, "сделай тёмную тему\n\nЧто уже сделано: Тёмная тема готова наполовину.");
        let said = layer
            .answer(admin(), &id, Kind::Wait, false, super::super::ready::Answer::HandTo { to: "t9".into() })
            .await;
        assert_eq!(said, "Системы «t9» нет.");
    }

    /// Stage 6 of docs/plans/work-systems.md: «сначала план» and «с проверкой» in a phrase choose the way and leave the
    /// task; the plan comes back as «План для «…»: … Делаю?», and «да» lets the work go on by it.
    #[tokio::test]
    async fn a_way_said_is_the_works_and_its_plan_is_asked_with_do_it() {
        let pack = pack::of(Lang::Ru);
        assert_eq!(way_said(pack, "сначала план, поправь опечатку"), (Some("plan".into()), "поправь опечатку".into()));
        assert_eq!(way_said(pack, "поправь опечатку с проверкой"), (Some("review".into()), "поправь опечатку".into()));
        assert_eq!(way_said(pack, "поправь опечатку").0, None);
        let layer = TalkLayer::new(None, crate::core::events::EventBus::new());
        let executor = FolderExecutor::new(&layer);
        let port = Arc::new(Port::default());
        executor.attach(port.clone());
        layer.start_folder_said(admin(), "сначала план, поправь опечатку", Some("сайт"), None).unwrap();
        let started = locked(&port.started)[0].clone();
        assert_eq!((started.way.as_deref(), started.task.as_str()), (Some("plan"), "поправь опечатку"));
        layer.folder_plan_asks("w1", "1. Найти опечатку.\n2. Поправить её.");
        let id = layer.works.of_target(FOLDER_EXECUTOR, "w1").unwrap().id;
        let question = layer.works.get(&id).unwrap().question.unwrap();
        assert_eq!(question.kind, Some(Kind::Plan));
        assert_eq!(question.text, "План для «Поправь опечатку»: 1. Найти опечатку. 2. Поправить её. Делаю?");
        let said = layer.answer(admin(), &id, Kind::Plan, false, super::super::ready::Answer::Yes).await;
        assert_eq!(said, "Делаю по плану.");
        assert!(layer.works.get(&id).unwrap().question.is_none());
    }

    /// «Как там …» while a plan's limit holds it asks «подождать или остановить?» aloud: «подожди» and «стоп» answer
    /// that question and no other.
    #[tokio::test]
    async fn how_it_goes_under_a_limit_asks_and_wait_and_stop_answer_only_that() {
        let layer = TalkLayer::new(None, crate::core::events::EventBus::new());
        let executor = FolderExecutor::new(&layer);
        let port = Arc::new(Port::default());
        executor.attach(port.clone());
        layer.start_folder_said(admin(), "поправь опечатку", Some("сайт"), None).unwrap();
        let rules = super::super::rules::TalkRules::new(&layer);
        let computer = super::super::origin::COMPUTER;
        assert_eq!(rules.answer_on("подожди", computer), None, "nothing is asked");
        layer.folder_waits("w1", None, "Claude Code");
        let line = layer.act(&admin(), "status", None).await.unwrap();
        assert!(line.ends_with(": ждёт лимита. Подождать или остановить?"), "{line}");
        assert_eq!(layer.asked.open(computer).map(|question| question.kind), Some(Kind::Wait));
        let answer = |text: &str| match rules.answer_on(text, computer) {
            Some(super::super::rules::Heard::Answer { answer, .. }) => Some(answer),
            _ => None,
        };
        assert_eq!(answer("подожди"), Some(super::super::ready::Answer::Wait));
        assert_eq!(answer("пусть ждёт"), Some(super::super::ready::Answer::Wait));
        assert_eq!(answer("остановить"), Some(super::super::ready::Answer::Stop));
        let id = layer.works.of_target(FOLDER_EXECUTOR, "w1").unwrap().id;
        let said = layer.answer(admin(), &id, Kind::Wait, false, super::super::ready::Answer::Wait).await;
        assert_eq!(said, pack::of(Lang::Ru).work.folder.waiting);
        // Another question open: «подожди» and «стоп» are not its answers.
        layer.asked.ask(computer, &id, Kind::Ready, false);
        assert_eq!((answer("подожди"), answer("стоп")), (None, None));
        assert_eq!(answer("да"), Some(super::super::ready::Answer::Yes));
        assert_eq!(answer("добавь"), None, "«добавь» is the budget's answer");
        // The budget's question: «добавь» and «стоп» answer it, «подожди» does not.
        layer.asked.ask(computer, &id, Kind::Budget, false);
        assert_eq!(answer("добавь ещё"), Some(super::super::ready::Answer::Raise));
        assert_eq!(answer("стоп"), Some(super::super::ready::Answer::Stop));
        assert_eq!(answer("подожди"), None);
    }

    /// «Готово» says the result and asks whether to read what the agent wrote; «подробнее» reads its last message
    /// aloud — no marks, no code.
    #[tokio::test]
    async fn ready_asks_to_read_the_agents_message_and_more_reads_it_aloud() {
        let layer = TalkLayer::new(None, crate::core::events::EventBus::new());
        let executor = FolderExecutor::new(&layer);
        let port = Arc::new(Port::default());
        executor.attach(port.clone());
        layer.start_folder_said(admin(), "поправь опечатку", Some("сайт"), None).unwrap();
        *locked(&port.told) = Some(FolderTold {
            folder: "Сайт".into(),
            agent: "Claude Code".into(),
            branch: "duowave/opechatka".into(),
            fetched: true,
            changed: true,
            usd_micro: Some(400_000),
            billing: Some("plan".into()),
            last: Some("## Сделано\n- Поправила `README.md`\n```sh\ngit diff\n```\nТесты прошли.".into()),
            ..FolderTold::default()
        });
        let listener = locked(&port.listener).clone().expect("the executor listens");
        listener(PortEvent::Ended { id: "w1".into(), end: FolderEnd::Ready });
        let id = layer.works.of_target(FOLDER_EXECUTOR, "w1").unwrap().id;
        until("its «Готово»", || layer.works.get(&id).is_some_and(|work| work.result.is_some())).await;
        let work = layer.works.get(&id).unwrap();
        assert_eq!(
            work.result.as_ref().unwrap().line,
            "Поправь опечатку: готово. Ветка duowave/opechatka, ≈ $0.40 по подписке. Рассказать, что написал агент?"
        );
        assert_eq!(work.question.as_ref().and_then(|question| question.kind), Some(Kind::More));
        let more = layer.answer(admin(), &id, Kind::More, false, super::super::ready::Answer::Details).await;
        assert_eq!(more, "Сделано. Поправила README.md. Тесты прошли.");
        // Without a message of the agent: the details are in the panel, nothing asked.
        assert_eq!(spoken("```\nonly code\n```"), "");
    }

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
        assert_eq!(title_of("напиши в файл poems2.md пять стихов"), "Напиши в файл poems2.md пять стихов");
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
