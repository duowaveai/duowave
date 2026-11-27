//! The router (docs/plans/router-and-works.md): which model takes a call, and what it cost. Stage 1 is the journal of
//! deals and the prices. Every answer of a model leaves a deal: the seat that asked, the model that answered and why,
//! its tokens, its dollars at the price of the day and that price itself (`deals.rs`). A price comes, in this order,
//! from the panel, from the provider's own list, from the snapshot built into the program — or there is none
//! (`pool.rs`). In both builds, small: no network, no crates of its own.
//!
//! Stage 2 is the door (`Router::open`): an ask (`policy::Ask` — the seat, why, the agent, the budget's step) and the
//! models offered for it go through the frames (`frames.rs`: the regulator «Что может уходить из дома»), then the
//! policy (`policy.rs`: the table of rules, the first that fits) — the model taken and why. The conversation, the
//! workshop, the memory, the apps and the background works ask it for their seat's model (`Router::slot`): with the
//! default table each keeps the model «Мозг» stands on the seat, as before the door, and only the deal says more. The
//! seat «Исполнитель» (`seats.rs`) gives a work in a folder its agent and model; before each run of it the door
//! counts what the work spent against the narrowest budget (`budgets.rs`) and says the step — a cheaper model, a
//! question, a stop — and what the run may spend (`Router::worker`). The door reads no network and waits for no
//! database on a conversation's way: the table and the regulator are kept in memory, the deal is written after.
//!
//! An answer's rounds are summed by its `Tally`: the dialog asks it before each round and tells it each round's
//! spending; the tally passes every round on to the sink (the journal's request, the work) and, once the answer is
//! over — ended, failed or dropped — closes it into one deal per model that answered: one, or two when «Запасная»
//! took over midway. The deal is written after the answer, never on its way.

pub mod budgets;
pub mod deals;
pub mod frames;
pub mod policy;
pub mod pool;
pub mod routes;
pub mod seats;
pub mod sets;
pub mod settings;
pub mod who;

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::config::{DialogProvider, ModelProfile};
use crate::core::events::{EventBus, GatewayEvent};
use crate::dialog::brain::presets::HELPER_PRESET;
use crate::dialog::llm::brain::ModelSlot;
use crate::dialog::llm::LlmUsage;
use crate::settings::SettingsService;
use crate::storage::Db;
use crate::utils::time::now_ms;

use budgets::{Act, Budget, Budgets, Scope, Spending, WorkBudgets};
pub use deals::{Answered, Deal, Deals};
use policy::{Ask, ModelRef, Offer, Policy, Ranked, Table, TablePolicy};
pub use pool::{Pool, Price};

/// How often the old deals and days are cleaned: once at the start, then daily.
const CLEAN_EVERY: Duration = Duration::from_secs(24 * 3600);

/// The seat a model answers on: the conversation's two (by the rule of the phrase), «Запасная» once it took over, the
/// workshop's agent, the memory's night, an app, a background work's round (`DialogService::background_round`), the
/// agent of a work in a folder («Исполнитель», stage 3). The planner's and the validator's come with the work by parts
/// (set aside, decision 18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Seat {
    Fast,
    Smart,
    Backup,
    Workshop,
    Memory,
    App,
    Background,
    Worker,
}

impl Seat {
    pub const ALL: [Seat; 8] = [
        Seat::Fast,
        Seat::Smart,
        Seat::Backup,
        Seat::Workshop,
        Seat::Memory,
        Seat::App,
        Seat::Background,
        Seat::Worker,
    ];
    /// The five a model is put on: «Разговор» (fast), «Умная», «Запасная», «Мастерская», «Исполнитель». The other three
    /// take what «Мозг» or their own settings have; people see them only in «Расход».
    pub const PLACED: [Seat; 5] = [Seat::Fast, Seat::Smart, Seat::Backup, Seat::Workshop, Seat::Worker];

    pub fn as_str(self) -> &'static str {
        match self {
            Seat::Fast => "fast",
            Seat::Smart => "smart",
            Seat::Backup => "backup",
            Seat::Workshop => "workshop",
            Seat::Memory => "memory",
            Seat::App => "app",
            Seat::Background => "background",
            Seat::Worker => "worker",
        }
    }

    pub fn parse(text: &str) -> Option<Seat> {
        Seat::ALL.into_iter().find(|seat| seat.as_str() == text)
    }

    /// The conversation's seat of a profile: «Быстрая» or «Умная».
    pub fn of(profile: ModelProfile) -> Seat {
        match profile {
            ModelProfile::Fast => Seat::Fast,
            ModelProfile::Smart => Seat::Smart,
        }
    }
}

/// Why this model: the conversation's rule by the phrase (`skills/quick_commands.rs`, `choose_profile`) — a command, a
/// hard question, a short one; what stands on the seat; «Запасная», which took over from a model that gave no answer;
/// a rule of the router's table (`table:<rule>`); a step of a budget (`budget:cheaper`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    RuleCommand,
    RuleHard,
    RuleShort,
    Seat,
    Backup,
    Table(String),
    Budget(Act),
}

impl Reason {
    pub fn as_str(&self) -> Cow<'static, str> {
        match self {
            Reason::RuleCommand => "rule:command".into(),
            Reason::RuleHard => "rule:hard".into(),
            Reason::RuleShort => "rule:short".into(),
            Reason::Seat => "seat".into(),
            Reason::Backup => "backup".into(),
            Reason::Table(rule) => format!("table:{rule}").into(),
            Reason::Budget(step) => format!("budget:{}", step.as_str()).into(),
        }
    }
}

/// How a deal is paid: by a key; by a plan's sign-in (ChatGPT's, Claude Code's: the dollars are what a key would have
/// cost); at home; by an agent's own account (an agent of a card, stage 3б — how it pays is its business).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Billing {
    Key,
    Plan,
    Local,
    Agent,
}

impl Billing {
    pub fn as_str(self) -> &'static str {
        match self {
            Billing::Key => "key",
            Billing::Plan => "plan",
            Billing::Local => "local",
            Billing::Agent => "agent",
        }
    }

    pub fn parse(text: &str) -> Option<Billing> {
        match text {
            "key" => Some(Billing::Key),
            "plan" => Some(Billing::Plan),
            "local" => Some(Billing::Local),
            "agent" => Some(Billing::Agent),
            _ => None,
        }
    }
}

/// Where the model is: at home, in one's own cloud (the regulator's class of «Дом и помощники», stage 3: a helper's
/// model away from home — the hidden preset `helper` of «Мозг»), in someone else's cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Home,
    Mine,
    Cloud,
}

impl Place {
    pub fn as_str(self) -> &'static str {
        match self {
            Place::Home => "home",
            Place::Mine => "mine",
            Place::Cloud => "cloud",
        }
    }
}

/// How an answer ended: whole; failed; on a plan's or a service's limit; cut by its rounds or time (the last round
/// asked for words); stopped (cancelled, or dropped in the middle of a round); on a run's ceiling of dollars (an
/// agent's `--max-budget-usd`: what the work's budget left it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Failed,
    Limit,
    Cut,
    Stopped,
    Budget,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Failed => "failed",
            Outcome::Limit => "limit",
            Outcome::Cut => "cut",
            Outcome::Stopped => "stopped",
            Outcome::Budget => "budget",
        }
    }

    pub fn parse(text: &str) -> Outcome {
        match text {
            "ok" => Outcome::Ok,
            "limit" => Outcome::Limit,
            "cut" => Outcome::Cut,
            "stopped" => Outcome::Stopped,
            "budget" => Outcome::Budget,
            _ => Outcome::Failed,
        }
    }

    /// A failure's outcome: a limit when the provider said so (ChatGPT's plan, HTTP 429), else a failure.
    pub fn of_error(error: &anyhow::Error) -> Outcome {
        let text = format!("{error:#}").to_lowercase();
        let limit = ["usage_limit", "rate_limit", "rate limit", "http 429", "status 429", "429 too many"]
            .iter()
            .any(|sign| text.contains(sign));
        if limit {
            Outcome::Limit
        } else {
            Outcome::Failed
        }
    }
}

/// Tokens, summed over rounds. `input` is the fresh input — neither read from the cache nor written to it — whatever
/// the provider counted: the whole input is the three together.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Tokens {
    /// One round as its provider counted it: OpenAI's prompt tokens hold the cached ones, Claude's input does not.
    pub fn of(usage: &LlmUsage, kind: DialogProvider) -> Tokens {
        let cache_read = usage.cache_read.unwrap_or(0);
        let cache_write = usage.cache_write.unwrap_or(0);
        let input = match kind {
            DialogProvider::Anthropic => usage.input,
            _ => usage.input.saturating_sub(cache_read.saturating_add(cache_write)),
        };
        Tokens { input, output: usage.output, cache_read, cache_write }
    }

    pub fn add(&mut self, other: Tokens) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

/// The model a round goes to, as a deal names it: the provider's id in «Мозг» and the model as sent, how its provider
/// counts tokens, whether it is away from home, and the seat and the reason it answers by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answerer {
    pub provider: String,
    pub model: String,
    pub kind: DialogProvider,
    pub cloud: bool,
    pub seat: Seat,
    pub reason: Reason,
}

impl Answerer {
    pub fn of(slot: &ModelSlot, seat: Seat, reason: Reason) -> Answerer {
        Answerer {
            provider: slot.provider.clone(),
            model: slot.model.clone(),
            kind: slot.kind,
            cloud: slot.cloud,
            seat,
            reason,
        }
    }

    /// Who answers a slot's round now: «Запасная» once it took over this answer, else the slot's own model as asked.
    pub fn now(slot: &ModelSlot, asked: &Answerer) -> Answerer {
        match slot.backup.as_deref().filter(|backup| backup.taken()) {
            Some(backup) => Answerer::of(&backup.slot, Seat::Backup, Reason::Backup),
            None => asked.clone(),
        }
    }
}

/// One answer's spending, its rounds summed: what the dialog closes into a deal. No text of the phrase or the answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Spent {
    /// The conversation it was in — a device's, `panel:<id>`, `workshop:make:<draft>`, `work:<executor>:<target>`;
    /// empty — none.
    pub key: String,
    pub by: Answerer,
    pub tokens: Tokens,
    pub rounds: u32,
    pub ms: u64,
    pub outcome: Outcome,
    /// A run of an agent (a work in a folder): how it went and paid, and the dollars it said itself — the gateway
    /// does not count them by its prices. None — the gateway called the model itself.
    pub bill: Option<Bill>,
}

/// What an agent's run says of its own paying: the way of the call (`harness:claude-code`, `agent:<card>`), how it
/// paid, its own estimate of the dollars and whose estimate it is; none — the price is unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bill {
    pub via: String,
    pub billing: Billing,
    pub usd_micro: Option<u64>,
    pub from: pool::PriceSource,
}

/// What the model spent, as the dialog's sink hears it: each round as it ends, with the provider and the seat; and
/// each answer once it is over, its rounds summed — the deal to close.
#[derive(Clone, Copy)]
pub enum Usage<'a> {
    Round { key: &'a str, provider: &'a str, model: &'a str, seat: Seat, usage: &'a LlmUsage },
    Answer(&'a Spent),
}

/// Where the model's spending goes (`DialogService::attach_usage`, the memory's night).
pub type UsageSink = Arc<dyn Fn(Usage<'_>) + Send + Sync>;

/// One answer's rounds as they end, closed into deals once: when the answer is over or dropped. Shared by the pieces
/// of one answer (a command's choice and the words after it): the last of them to go closes it.
pub struct Tally {
    sink: Option<UsageSink>,
    key: String,
    asked: Answerer,
    began: Instant,
    state: Mutex<TallyState>,
}

#[derive(Default)]
struct TallyState {
    /// Per model that answered a round, in order.
    parts: Vec<Part>,
    /// A round was asked at all.
    asked: bool,
    /// A round is going: an answer dropped now was stopped.
    going: bool,
    /// The last round asked for words: the rounds or the time ran out.
    cut: bool,
    ended: Option<Outcome>,
}

struct Part {
    by: Answerer,
    tokens: Tokens,
    rounds: u32,
}

impl Tally {
    /// An answer of `asked` (its seat and reason) in the conversation `key`; without a sink it counts for nobody. Nor
    /// do the words said in a model's place when none may be asked (dialog/llm/unreachable.rs, kind `none`): no model
    /// answered, so there is no deal.
    pub fn new(sink: Option<UsageSink>, key: &str, asked: Answerer) -> Arc<Tally> {
        let sink = sink.filter(|_| asked.kind != DialogProvider::None);
        Arc::new(Tally {
            sink,
            key: key.to_string(),
            asked,
            began: Instant::now(),
            state: Mutex::new(TallyState::default()),
        })
    }

    /// The model as the answer asked for it, on its seat.
    pub fn asked(&self) -> &Answerer {
        &self.asked
    }

    /// A round is asked now.
    pub fn asking(&self) {
        let mut state = self.lock();
        state.asked = true;
        state.going = true;
    }

    /// The round about to be asked is the last, for words: the rounds or the time ran out.
    pub fn cut(&self) {
        self.lock().cut = true;
    }

    /// A round ended, answered by `by`, with what it spent: summed, and told to the sink as it is.
    pub fn round(&self, by: &Answerer, usage: Option<&LlmUsage>) {
        {
            let mut state = self.lock();
            state.going = false;
            let tokens = usage.map(|usage| Tokens::of(usage, by.kind)).unwrap_or_default();
            match state.parts.iter_mut().find(|part| part.by == *by) {
                Some(part) => {
                    part.tokens.add(tokens);
                    part.rounds += 1;
                }
                None => state.parts.push(Part { by: by.clone(), tokens, rounds: 1 }),
            }
        }
        if let (Some(sink), Some(usage)) = (&self.sink, usage) {
            sink(Usage::Round { key: &self.key, provider: &by.provider, model: &by.model, seat: by.seat, usage });
        }
    }

    /// The answer is over: how. The first word stays; a whole answer whose last round was for words is `cut`.
    pub fn end(&self, outcome: Outcome) {
        let mut state = self.lock();
        state.going = false;
        if state.ended.is_none() {
            state.ended = Some(if outcome == Outcome::Ok && state.cut { Outcome::Cut } else { outcome });
        }
    }

    /// The answer failed: on a limit, cancelled, or otherwise.
    pub fn failed(&self, error: &anyhow::Error, cancel: Option<&CancellationToken>) {
        let outcome = if cancel.is_some_and(CancellationToken::is_cancelled) {
            Outcome::Stopped
        } else {
            Outcome::of_error(error)
        };
        self.end(outcome);
    }

    fn lock(&self) -> MutexGuard<'_, TallyState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// What the answer spent, one per model that answered; the model asked when none answered a round. Nothing when
    /// no round was asked.
    fn spent(&self, state: &TallyState) -> Vec<Spent> {
        if !state.asked {
            return Vec::new();
        }
        let ended = state.ended.unwrap_or(if state.going {
            Outcome::Stopped
        } else if state.cut {
            Outcome::Cut
        } else {
            Outcome::Ok
        });
        let ms = self.began.elapsed().as_millis() as u64;
        let spent = |by: &Answerer, tokens: Tokens, rounds: u32| Spent {
            key: self.key.clone(),
            by: by.clone(),
            tokens,
            rounds,
            ms,
            outcome: ended,
            bill: None,
        };
        if state.parts.is_empty() {
            return vec![spent(&self.asked, Tokens::default(), 0)];
        }
        state.parts.iter().map(|part| spent(&part.by, part.tokens, part.rounds)).collect()
    }
}

impl Drop for Tally {
    fn drop(&mut self) {
        let Some(sink) = self.sink.take() else { return };
        let state = self.state.get_mut().unwrap_or_else(|poisoned| poisoned.into_inner());
        let spent = {
            let state = std::mem::take(state);
            self.spent(&state)
        };
        for spent in &spent {
            sink(Usage::Answer(spent));
        }
    }
}

/// What a deal belongs to: the journal's request, the work and its part.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Links {
    pub request: Option<String>,
    pub work: Option<String>,
    pub part: Option<String>,
}

/// The journal of deals and the prices behind it, the door with its table, frames and budgets, as the gateway holds
/// them.
pub struct Router {
    pub deals: Deals,
    pub pool: Pool,
    /// The budgets of single works.
    pub budgets: WorkBudgets,
    settings: Arc<SettingsService>,
    events: EventBus,
    cancel: CancellationToken,
    /// Who answered each work's last round, by the work's id: the page «Работы» names the model of a work going on
    /// before its deal is written — the workshop's agent closes one deal at the end of its run. Not kept.
    heard: Mutex<HashMap<String, (Answered, u64)>>,
    /// The table and the regulator as the door reads them: from the settings once, again after they change.
    view: RwLock<Option<Arc<View>>>,
}

/// What the door reads on every ask, kept in memory.
struct View {
    table: Table,
    away: crate::core::helpers::Away,
}

/// The door's answer: the offer taken and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub offer: usize,
    pub reason: Reason,
}

/// A work in a folder before a run, as the door is asked: its agent and the model it is on (none — the agent's own),
/// whether the agent's model is away from home (Claude Code always; a card's agent unless its model goes through
/// DuoWave), its folder's budget and its own, what it spent so far.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkerAsk {
    pub agent: String,
    pub model: Option<String>,
    pub cloud: bool,
    pub folder: Option<Budget>,
    pub work: Option<Budget>,
    pub spent: Spending,
}

/// The same model of «Мозг»: its provider and its name.
fn same(a: &ModelSlot, b: &ModelSlot) -> bool {
    a.provider == b.provider && a.model == b.model
}

impl WorkerDeal {
    /// No door to ask (the gateway is closing): the work's own model, no budget's step.
    pub fn none(asked: &WorkerAsk) -> WorkerDeal {
        WorkerDeal {
            model: asked.model.clone(),
            reason: Reason::Seat,
            step: None,
            budget: Budgets::default().works,
            scope: Scope::Works,
            spent: asked.spent,
            ceiling_usd: None,
            closed: false,
        }
    }
}

/// The door's answer for a run: the model (none — the agent's own) and why; the budget that holds, where it comes
/// from and the step reached; what was spent; what the run may spend in dollars before the person is asked (an
/// agent's `--max-budget-usd`); `closed` — the regulator keeps the agent's model in: the run does not go.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerDeal {
    pub model: Option<String>,
    pub reason: Reason,
    pub step: Option<Act>,
    pub budget: Budget,
    pub scope: Scope,
    pub spent: Spending,
    pub ceiling_usd: Option<f64>,
    pub closed: bool,
}

/// A work's rounds heard are remembered for this many works: the oldest go first.
const HEARD_MAX: usize = 200;

impl Router {
    pub fn new(db: Arc<Db>, settings: Arc<SettingsService>, events: EventBus, env: pool::EnvModel) -> Arc<Router> {
        Arc::new(Router {
            deals: Deals::new(db.clone()),
            budgets: WorkBudgets::new(db.clone()),
            pool: Pool::new(db, env),
            settings,
            events,
            cancel: CancellationToken::new(),
            heard: Mutex::new(HashMap::new()),
            view: RwLock::new(None),
        })
    }

    // ───────── The door ─────────

    /// The table and the regulator now.
    fn view(&self) -> Arc<View> {
        if let Some(view) = self.view.read().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref() {
            return view.clone();
        }
        let view = Arc::new(View {
            table: self.table(),
            away: self.settings.get(&crate::settings::definitions::AWAY, None).unwrap_or_default(),
        });
        *self.view.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(view.clone());
        view
    }

    /// The table or the regulator changed: read again at the next ask.
    pub fn forget_view(&self) {
        *self.view.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    /// The table: the setting `router_table`, else the default built in — also when the stored one is not ours to
    /// read (a newer schema from a settings package).
    pub fn table(&self) -> Table {
        self.settings
            .get(&crate::router::settings::ROUTER_TABLE, None)
            .filter(|table| table.check().is_ok())
            .unwrap_or_default()
    }

    /// The seats of the setting `router_seats`.
    pub fn seats(&self) -> seats::Seats {
        self.settings.get(&crate::router::settings::ROUTER_SEATS, None).unwrap_or_default()
    }

    /// The general budgets: the setting `budgets`, else five dollars a work.
    pub fn general(&self) -> Budgets {
        self.settings.get(&crate::router::settings::BUDGETS, None).unwrap_or_default()
    }

    /// The regulator «Что может уходить из дома» now.
    pub fn away(&self) -> crate::core::helpers::Away {
        self.view().away
    }

    /// The door: the offers the frames let through, in the policy's order — the first is taken, by the reason the
    /// policy gave it, else the ask's own. None — the frames let nothing through.
    pub fn open(&self, ask: &Ask, offers: &[Offer]) -> Option<Opened> {
        let view = self.view();
        self.open_in(&view, ask, offers)
    }

    fn open_in(&self, view: &View, ask: &Ask, offers: &[Offer]) -> Option<Opened> {
        let through = frames::let_through(view.away, offers);
        let first = *through.first()?;
        let ranked = TablePolicy(&view.table).rank(ask, offers);
        let taken = ranked
            .into_iter()
            .find(|ranked| through.contains(&ranked.offer))
            .unwrap_or(Ranked { offer: first, reason: None });
        Some(Opened { offer: taken.offer, reason: taken.reason.unwrap_or_else(|| ask.reason.clone()) })
    }

    /// A seat's model of «Мозг» through the door: the slot «Мозг» stands on the seat, and the models the table names
    /// for the ask — a bare name is a model of the seat's own provider — as `resolve` finds them in «Мозг». The slot
    /// taken, with its seat and reason for the deal. With the default table — nothing named — it is the seat's slot,
    /// as before the door.
    pub fn slot(
        &self,
        ask: &Ask,
        slot: ModelSlot,
        resolve: &dyn Fn(&str, &str) -> Option<ModelSlot>,
    ) -> (ModelSlot, Answerer) {
        let view = self.view();
        let named = view.table.named(ask);
        if named.is_empty() {
            let answerer = Answerer::of(&slot, ask.seat, ask.reason.clone());
            return (slot, answerer);
        }
        let mut slots = vec![slot];
        for model in named {
            let found = match &model {
                ModelRef::Named(name) => resolve(&slots[0].provider, name.trim()),
                ModelRef::Pick { provider, model } => resolve(provider, model),
            };
            if let Some(found) = found.filter(|found| !slots.iter().any(|slot| same(slot, found))) {
                slots.push(found);
            }
        }
        let offers: Vec<Offer> = slots
            .iter()
            .enumerate()
            .map(|(at, slot)| Offer {
                provider: slot.provider.clone(),
                model: slot.model.clone(),
                place: if at == 0 { self.cheap_place(slot) } else { self.place_of(&slot.provider, slot.cloud) },
                price: None,
                seat: at == 0,
                framed: at == 0,
            })
            .collect();
        let opened = self.open_in(&view, ask, &offers).unwrap_or(Opened { offer: 0, reason: ask.reason.clone() });
        let slot = slots.swap_remove(opened.offer);
        let answerer = Answerer::of(&slot, ask.seat, opened.reason);
        (slot, answerer)
    }

    /// Where a model is, without asking the database: at home, or away — the seat's model, held by «Мозг» already.
    fn cheap_place(&self, slot: &ModelSlot) -> Place {
        if slot.cloud {
            Place::Cloud
        } else {
            Place::Home
        }
    }

    /// Where a model is, as the regulator counts it: at home, the household's own cloud (a helper's model away from
    /// home), someone else's cloud.
    pub fn place_of(&self, provider: &str, cloud: bool) -> Place {
        if !cloud {
            return Place::Home;
        }
        if self.pool.book(provider).preset.as_deref() == Some(HELPER_PRESET) {
            Place::Mine
        } else {
            Place::Cloud
        }
    }

    /// A run of a work in a folder through the door: the narrowest budget and the step the work's spending reached,
    /// the model for the run — the table's at the step (`cheaper`: Claude Code on `haiku`), else the work's own —, the
    /// ceiling of the run, and whether the regulator lets the agent's model out at all.
    pub fn worker(&self, asked: &WorkerAsk) -> WorkerDeal {
        let general = self.general();
        let (budget, scope) = budgets::narrowest(asked.work.as_ref(), asked.folder.as_ref(), &general.works);
        let step = budget.step(&asked.spent).map(|step| step.act);
        let ask = Ask { agent: Some(asked.agent.clone()), step, ..Ask::of(Seat::Worker, Reason::Seat) };
        let view = self.view();
        // The agent's model goes away unless it goes through DuoWave — then it is held as the conversation is.
        let offer = |model: &str, seat: bool| Offer {
            provider: asked.agent.clone(),
            model: model.to_string(),
            place: if asked.cloud { Place::Cloud } else { Place::Home },
            price: None,
            seat,
            framed: !asked.cloud,
        };
        let own = asked.model.clone().unwrap_or_default();
        let mut offers = vec![offer(&own, true)];
        for model in view.table.named(&ask) {
            if let ModelRef::Named(name) = model {
                let name = name.trim();
                if !name.eq_ignore_ascii_case(&own) && !offers.iter().any(|offer| offer.model == name) {
                    offers.push(offer(name, false));
                }
            }
        }
        let opened = self.open_in(&view, &ask, &offers);
        let model = match &opened {
            Some(opened) if !offers[opened.offer].seat => Some(offers[opened.offer].model.clone()),
            _ => asked.model.clone(),
        };
        WorkerDeal {
            model,
            reason: opened.as_ref().map(|opened| opened.reason.clone()).unwrap_or(Reason::Seat),
            step,
            ceiling_usd: budget.ceiling_usd(&asked.spent),
            budget,
            scope,
            spent: asked.spent,
            closed: opened.is_none(),
        }
    }

    /// What a work spent since it began, by its deals.
    pub fn spending(&self, work: &str, since: u64) -> Spending {
        match self.deals.of_work(work, since) {
            Ok(of) => Spending { usd_micro: of.sums.usd_micro, tokens: of.sums.tokens, unpriced: of.sums.unpriced },
            Err(error) => {
                tracing::warn!(target: "router", "the deals of the work {work} not read: {error}");
                Spending::default()
            }
        }
    }

    /// A round of a work ended: who answered it (the usage sink, app.rs).
    pub fn heard(&self, work: &str, provider: &str, model: &str, seat: Seat) {
        let mut heard = self.heard.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let answered = Answered {
            provider: provider.to_string(),
            model: model.to_string(),
            seat: seat.as_str().into(),
            place: None,
        };
        heard.insert(work.to_string(), (answered, now_ms()));
        if heard.len() > HEARD_MAX {
            if let Some(oldest) = heard.iter().min_by_key(|(_, (_, at))| *at).map(|(work, _)| work.clone()) {
                heard.remove(&oldest);
            }
        }
    }

    /// Who answered a work's last round heard since this start of the gateway, if any.
    pub fn last_heard(&self, work: &str) -> Option<Answered> {
        let heard = self.heard.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        heard.get(work).map(|(answered, _)| answered.clone())
    }

    /// An answer's spending closed into a deal: priced, written with its day, told (`router.deal`). None when the
    /// database refused it (logged).
    pub fn close(&self, spent: &Spent, links: Links) -> Option<Deal> {
        let deal = self.price(spent, links, now_ms());
        match self.deals.write(&deal) {
            Ok(id) => {
                let deal = Deal { id, ..deal };
                self.events.emit(GatewayEvent::RouterDeal {
                    id,
                    seat: deal.seat.clone(),
                    provider: deal.provider.clone(),
                    model: deal.model.clone(),
                    usd_micro: deal.usd_micro,
                });
                Some(deal)
            }
            Err(error) => {
                tracing::warn!(target: "router", "a deal of {} ({}) not written: {error}", spent.by.model, spent.by.seat.as_str());
                None
            }
        }
    }

    /// The deal of an answer, not written yet: where its model is, how it is paid, the price and the dollars.
    pub fn price(&self, spent: &Spent, links: Links, at: u64) -> Deal {
        let by = &spent.by;
        if let Some(bill) = &spent.bill {
            // An agent's run: its own dollars, its way and its paying; away from home.
            let price = bill.usd_micro.map(|_| Price::estimate(bill.from));
            return Deal {
                id: 0,
                at,
                seat: by.seat.as_str().to_string(),
                via: bill.via.clone(),
                provider: by.provider.clone(),
                model: by.model.clone(),
                place: if by.cloud { Place::Cloud } else { Place::Home }.as_str().to_string(),
                reason: by.reason.as_str().to_string(),
                request: links.request,
                work: links.work,
                part: links.part,
                tokens: spent.tokens,
                rounds: spent.rounds,
                ms: spent.ms,
                usd_micro: bill.usd_micro,
                price,
                billing: bill.billing.as_str().to_string(),
                outcome: spent.outcome.as_str().to_string(),
                check: None,
            };
        }
        let (place, billing, price) = if by.cloud {
            let book = self.pool.book(&by.provider);
            let billing = if book.plan { Billing::Plan } else { Billing::Key };
            // A helper's model away from home is the household's own cloud (a helper is never a cloud by keys).
            let place = if book.preset.as_deref() == Some(HELPER_PRESET) { Place::Mine } else { Place::Cloud };
            (place, billing, self.pool.price(&by.provider, book.preset.as_deref(), &by.model))
        } else {
            (Place::Home, Billing::Local, Some(Price::local()))
        };
        let usd_micro = price.as_ref().and_then(|price| price.cost(&spent.tokens));
        Deal {
            id: 0,
            at,
            seat: by.seat.as_str().to_string(),
            via: deals::VIA_BRAIN.to_string(),
            provider: by.provider.clone(),
            model: by.model.clone(),
            place: place.as_str().to_string(),
            reason: by.reason.as_str().to_string(),
            request: links.request,
            work: links.work,
            part: links.part,
            tokens: spent.tokens,
            rounds: spent.rounds,
            ms: spent.ms,
            usd_micro,
            price,
            billing: billing.as_str().to_string(),
            outcome: spent.outcome.as_str().to_string(),
            check: None,
        }
    }

    /// The cleaning: now, then daily — deals older than 90 days, days older than a year.
    pub fn start(self: &Arc<Self>) {
        self.clean(now_ms());
        // The door reads the table and the regulator again once either changed: by the API, the panel, a package.
        let me = Arc::downgrade(self);
        self.events.on("router-view", move |event| {
            let changed = matches!(&*event, GatewayEvent::SettingsChanged { key, .. }
                if key == crate::router::settings::ROUTER_TABLE.key
                    || key == crate::settings::definitions::AWAY.key);
            if let Some(router) = me.upgrade().filter(|_| changed) {
                router.forget_view();
            }
        });
        let me = Arc::downgrade(self);
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(CLEAN_EVERY);
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = ticker.tick() => match me.upgrade() {
                        Some(router) => router.clean(now_ms()),
                        None => break,
                    },
                }
            }
        });
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }

    fn clean(&self, now: u64) {
        match self.deals.prune(now) {
            Ok((0, 0)) => {}
            Ok((deals, days)) => tracing::info!(target: "router", "cleaned {deals} old deal(s) and {days} old day(s)"),
            Err(error) => tracing::warn!(target: "router", "the old deals not cleaned: {error}"),
        }
        // A work's own budget is kept as long as its deals.
        if let Err(error) = self.budgets.prune(now.saturating_sub(deals::KEEP_DEALS_DAYS * 86_400_000)) {
            tracing::warn!(target: "router", "the old budgets of works not cleaned: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    fn answerer(model: &str, seat: Seat, reason: Reason) -> Answerer {
        Answerer {
            provider: "openrouter".into(),
            model: model.into(),
            kind: DialogProvider::OpenAi,
            cloud: true,
            seat,
            reason,
        }
    }

    fn usage(input: u64, output: u64, cache_read: Option<u64>) -> LlmUsage {
        LlmUsage { input, output, cache_read, cache_write: None }
    }

    /// What a sink keeps.
    type Kept<T> = Arc<StdMutex<Vec<T>>>;

    /// A sink that keeps what it hears: the rounds' keys and models, the answers.
    fn sink() -> (UsageSink, Kept<String>, Kept<Spent>) {
        let (rounds, answers) = (Arc::new(StdMutex::new(Vec::new())), Arc::new(StdMutex::new(Vec::new())));
        let (r, a) = (rounds.clone(), answers.clone());
        let sink: UsageSink = Arc::new(move |heard: Usage<'_>| match heard {
            Usage::Round { key, model, seat, usage, .. } => {
                r.lock().unwrap().push(format!("{key} {model} {} {}", seat.as_str(), usage.input))
            }
            Usage::Answer(spent) => a.lock().unwrap().push(spent.clone()),
        });
        (sink, rounds, answers)
    }

    #[test]
    fn counts_the_fresh_input_whatever_the_provider_counted() {
        let openai = Tokens::of(&usage(100, 20, Some(80)), DialogProvider::OpenAi);
        assert_eq!(openai, Tokens { input: 20, output: 20, cache_read: 80, cache_write: 0 });
        let claude = LlmUsage { input: 12, output: 3, cache_read: Some(2000), cache_write: Some(500) };
        assert_eq!(
            Tokens::of(&claude, DialogProvider::Anthropic),
            Tokens { input: 12, output: 3, cache_read: 2000, cache_write: 500 }
        );
    }

    #[test]
    fn sums_an_answers_rounds_into_one_deal_and_tells_each_round() {
        let (sink, rounds, answers) = sink();
        let asked = answerer("m", Seat::Smart, Reason::RuleHard);
        let tally = Tally::new(Some(sink), "kitchen", asked.clone());
        for _ in 0..2 {
            tally.asking();
            tally.round(&asked, Some(&usage(100, 20, Some(80))));
        }
        tally.end(Outcome::Ok);
        assert!(answers.lock().unwrap().is_empty(), "closed only when the answer goes");
        drop(tally);
        assert_eq!(*rounds.lock().unwrap(), ["kitchen m smart 100", "kitchen m smart 100"]);
        let answers = answers.lock().unwrap();
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].tokens, Tokens { input: 40, output: 40, cache_read: 160, cache_write: 0 });
        assert_eq!((answers[0].rounds, answers[0].outcome), (2, Outcome::Ok));
        assert_eq!((answers[0].by.seat, answers[0].by.reason.clone()), (Seat::Smart, Reason::RuleHard));
    }

    #[test]
    fn a_backup_that_took_over_midway_has_a_deal_of_its_own() {
        let (sink, _, answers) = sink();
        let asked = answerer("first", Seat::Fast, Reason::RuleShort);
        let backup = answerer("backup", Seat::Backup, Reason::Backup);
        let tally = Tally::new(Some(sink), "kitchen", asked.clone());
        tally.asking();
        tally.round(&asked, Some(&usage(10, 1, None)));
        tally.asking();
        tally.round(&backup, Some(&usage(30, 3, None)));
        tally.end(Outcome::Ok);
        drop(tally);
        let answers = answers.lock().unwrap();
        let models: Vec<(&str, u64, Seat)> =
            answers.iter().map(|spent| (spent.by.model.as_str(), spent.tokens.input, spent.by.seat)).collect();
        assert_eq!(models, [("first", 10, Seat::Fast), ("backup", 30, Seat::Backup)]);
    }

    #[test]
    fn says_how_the_answer_ended() {
        let asked = answerer("m", Seat::Fast, Reason::Seat);
        let ended = |play: &dyn Fn(&Tally)| {
            let (sink, _, answers) = sink();
            let tally = Tally::new(Some(sink), "", asked.clone());
            play(&tally);
            drop(tally);
            let answers = answers.lock().unwrap();
            answers.first().map(|spent| (spent.outcome, spent.rounds))
        };
        // Nothing asked: no deal.
        assert_eq!(ended(&|_| {}), None);
        // Dropped in the middle of a round: stopped; between rounds: whole.
        assert_eq!(ended(&|tally| tally.asking()), Some((Outcome::Stopped, 0)));
        assert_eq!(
            ended(&|tally| {
                tally.asking();
                tally.round(&asked, None);
            }),
            Some((Outcome::Ok, 1))
        );
        // The last round for words: cut.
        assert_eq!(
            ended(&|tally| {
                tally.cut();
                tally.asking();
                tally.round(&asked, None);
                tally.end(Outcome::Ok);
            }),
            Some((Outcome::Cut, 1))
        );
        // A failure: on a limit, cancelled, or else; the first word stays.
        let limit = anyhow::anyhow!("ChatGPT: HTTP 429: subscription_sharing_usage_limit_exceeded");
        assert_eq!(
            ended(&|tally| {
                tally.asking();
                tally.failed(&limit, None);
                tally.end(Outcome::Ok);
            }),
            Some((Outcome::Limit, 0))
        );
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            ended(&|tally| {
                tally.asking();
                tally.failed(&anyhow::anyhow!("aborted"), Some(&cancel));
            }),
            Some((Outcome::Stopped, 0))
        );
        assert_eq!(Outcome::of_error(&anyhow::anyhow!("no answer from m in 100 ms")), Outcome::Failed);
    }

    #[test]
    fn names_the_seats_and_the_reasons() {
        assert_eq!(
            Seat::ALL.map(Seat::as_str),
            ["fast", "smart", "backup", "workshop", "memory", "app", "background", "worker"]
        );
        assert_eq!(Seat::parse("memory"), Some(Seat::Memory));
        assert_eq!(Seat::parse("planner"), None);
        assert_eq!((Seat::of(ModelProfile::Fast), Seat::of(ModelProfile::Smart)), (Seat::Fast, Seat::Smart));
        let reasons = [
            Reason::RuleCommand,
            Reason::RuleHard,
            Reason::RuleShort,
            Reason::Seat,
            Reason::Backup,
            Reason::Table("night".into()),
            Reason::Budget(Act::Cheaper),
        ];
        assert_eq!(
            reasons.map(|reason| reason.as_str().into_owned()),
            ["rule:command", "rule:hard", "rule:short", "seat", "backup", "table:night", "budget:cheaper"]
        );
    }

    /// «Мозг» with no model that may be asked answers in honest words (docs/plans/home-and-helpers.md, stage 4): they
    /// are no model's answer, so they leave no deal — none with an empty model in «Расход» or in the journal's request.
    #[test]
    fn the_words_in_a_models_place_leave_no_deal() {
        let (sink, rounds, answers) = sink();
        let words = Answerer {
            provider: "words".into(),
            model: String::new(),
            kind: DialogProvider::None,
            cloud: false,
            seat: Seat::Fast,
            reason: Reason::Seat,
        };
        let tally = Tally::new(Some(sink), "kitchen", words);
        tally.asking();
        tally.failed(&anyhow::anyhow!("Компьютер дома не на связи, а запасной модели нет."), None);
        drop(tally);
        assert!(rounds.lock().unwrap().is_empty() && answers.lock().unwrap().is_empty());
    }

    fn door() -> (Arc<Router>, Arc<SettingsService>) {
        let db = Arc::new(Db::memory());
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let env = pool::EnvModel { kind: DialogProvider::None, base_url: None };
        (Router::new(db, settings.clone(), EventBus::new(), env), settings)
    }

    fn model(provider: &str, model: &str, cloud: bool) -> ModelSlot {
        ModelSlot {
            client: Arc::new(crate::dialog::llm::echo::EchoLlm),
            model: model.into(),
            provider: provider.into(),
            kind: DialogProvider::OpenAi,
            title: provider.into(),
            cloud,
            backup: None,
        }
    }

    /// The conversation, the workshop, the memory, the apps and the background works keep the seat's model with the
    /// default table — the door changes nothing but the deal's words.
    #[test]
    fn with_the_default_table_every_seat_keeps_its_model_and_reason() {
        let (router, _) = door();
        let asked = std::cell::Cell::new(0);
        let resolve = |_: &str, _: &str| {
            asked.set(asked.get() + 1);
            None
        };
        for (seat, reason) in [
            (Seat::Fast, Reason::RuleShort),
            (Seat::Smart, Reason::RuleHard),
            (Seat::Fast, Reason::RuleCommand),
            (Seat::Workshop, Reason::Seat),
            (Seat::Memory, Reason::Seat),
            (Seat::App, Reason::Seat),
            (Seat::Background, Reason::Seat),
        ] {
            let ask = policy::Ask::of(seat, reason.clone());
            let (slot, answerer) = router.slot(&ask, model("openrouter", "m", true), &resolve);
            assert_eq!((slot.provider.as_str(), slot.model.as_str()), ("openrouter", "m"));
            assert_eq!(answerer, Answerer::of(&slot, seat, reason));
        }
        assert_eq!(asked.get(), 0, "nothing is looked up for the default");
    }

    /// A table replaced holds from the next ask: a rule for «Умная» takes the model it names, by its rule; a bare
    /// name is the seat provider's model; the regulator keeps a cloud a rule names in.
    #[test]
    fn a_table_set_holds_from_the_next_ask_and_the_regulator_holds_it() {
        let (router, settings) = door();
        let table: Table = serde_json::from_value(serde_json::json!({"schema": 1, "rules": [
            {"id": "deep", "seat": "smart", "when": {"tools": true}, "model": "deep-model"},
            {"id": "home", "seat": "smart", "model": {"provider": "ollama", "model": "qwen"}}
        ]}))
        .unwrap();
        let resolve = |provider: &str, name: &str| Some(model(provider, name, provider != "ollama"));
        let ask = policy::Ask::of(Seat::Smart, Reason::RuleHard);
        let (before, _) = router.slot(&ask, model("openrouter", "m", true), &resolve);
        assert_eq!(before.model, "m");
        settings.set(&crate::router::settings::ROUTER_TABLE, &table, None).unwrap();
        router.forget_view();
        let (slot, answerer) = router.slot(&ask, model("openrouter", "m", true), &resolve);
        assert_eq!((slot.provider.as_str(), slot.model.as_str()), ("ollama", "qwen"));
        assert_eq!(answerer.reason, Reason::Table("home".into()));
        let tools = policy::Ask { tools: true, ..ask.clone() };
        let (slot, answerer) = router.slot(&tools, model("openrouter", "m", true), &resolve);
        assert_eq!((slot.provider.as_str(), slot.model.as_str()), ("openrouter", "deep-model"));
        assert_eq!(answerer.reason.as_str(), "table:deep");
        // «Только дома»: the cloud the rule names is not offered; the seat's own model is «Мозг»'s to hold.
        settings.set(&crate::settings::definitions::AWAY, &crate::core::helpers::Away::Home, None).unwrap();
        router.forget_view();
        let (slot, _) = router.slot(&tools, model("openrouter", "m", true), &resolve);
        assert_eq!(slot.model, "qwen");
        // A table that is not ours to read: the default holds.
        settings.set(&crate::router::settings::ROUTER_TABLE, &Table { schema: 9, rules: Vec::new() }, None).unwrap();
        router.forget_view();
        assert_eq!(router.table(), Table::default());
    }

    fn spent(usd: f64) -> Spending {
        Spending { usd_micro: (usd * 1_000_000.0) as u64, ..Spending::default() }
    }

    fn claude(model: Option<&str>, usd: f64) -> WorkerAsk {
        WorkerAsk {
            agent: seats::CLAUDE_CODE.into(),
            model: model.map(str::to_string),
            cloud: true,
            spent: spent(usd),
            ..WorkerAsk::default()
        }
    }

    /// The budget's step `cheaper` at 0.8 gives a cheaper model through the table for the next run; `ask` — the
    /// question; the run's ceiling is what is left to it.
    #[test]
    fn the_budgets_steps_give_a_cheaper_model_then_the_question() {
        let (router, _) = door();
        let fresh = router.worker(&claude(Some("sonnet"), 1.0));
        assert_eq!((fresh.model.as_deref(), fresh.step, fresh.reason.clone()), (Some("sonnet"), None, Reason::Seat));
        assert_eq!((fresh.ceiling_usd, fresh.scope, fresh.closed), (Some(4.0), Scope::Works, false));
        let cheaper = router.worker(&claude(Some("sonnet"), 4.2));
        assert_eq!(cheaper.model.as_deref(), Some("haiku"));
        assert_eq!((cheaper.step, cheaper.reason.as_str()), (Some(Act::Cheaper), "budget:cheaper".into()));
        // The agent's own choice goes cheaper too.
        assert_eq!(router.worker(&claude(None, 4.2)).model.as_deref(), Some("haiku"));
        let ask = router.worker(&claude(Some("sonnet"), 5.0));
        assert_eq!((ask.step, ask.ceiling_usd), (Some(Act::Ask), Some(0.0)));
        // An agent of a card: no cheaper model named for it, it stays.
        let droid = WorkerAsk { agent: "droid".into(), ..claude(Some("glm"), 4.2) };
        let deal = router.worker(&droid);
        assert_eq!((deal.model.as_deref(), deal.step), (Some("glm"), Some(Act::Cheaper)));
    }

    /// A work's budget over its folder's, the folder's over the general one.
    #[test]
    fn the_narrowest_budget_holds() {
        let (router, settings) = door();
        settings.set(&crate::router::settings::BUDGETS, &Budgets { works: Budget::usd(10.0) }, None).unwrap();
        let general = router.worker(&claude(None, 4.2));
        assert_eq!((general.scope, general.step), (Scope::Works, None));
        let folder = WorkerAsk { folder: Some(Budget::usd(5.0)), ..claude(None, 4.2) };
        let deal = router.worker(&folder);
        assert_eq!((deal.scope, deal.step), (Scope::Folder, Some(Act::Cheaper)));
        let work = WorkerAsk { work: Some(Budget::usd(4.0)), ..folder };
        let deal = router.worker(&work);
        assert_eq!((deal.scope, deal.step, deal.budget.limit), (Scope::Work, Some(Act::Ask), 4.0));
    }

    /// Claude Code is someone else's cloud: with «Только дома» or «Дом и моё облако» its run does not go. An agent
    /// whose model goes through DuoWave is held as the conversation is.
    #[test]
    fn the_regulator_keeps_an_agents_cloud_in() {
        let (router, settings) = door();
        assert!(!router.worker(&claude(None, 0.0)).closed);
        settings.set(&crate::settings::definitions::AWAY, &crate::core::helpers::Away::Mine, None).unwrap();
        router.forget_view();
        assert!(router.worker(&claude(None, 0.0)).closed);
        let through = WorkerAsk { agent: "opencode".into(), cloud: false, ..claude(None, 0.0) };
        assert!(!router.worker(&through).closed);
    }

    /// A helper's model is priced like any row of «Мозг», but where it is follows the regulator's classes: at home
    /// `home` and free, away from home `mine` — the household's own cloud, never someone else's.
    #[test]
    fn a_helpers_model_away_from_home_is_the_households_own_cloud() {
        let db = Arc::new(Db::memory());
        for (id, preset) in [("helper-own-a", HELPER_PRESET), ("openrouter", "openrouter")] {
            db.with(|c| {
                c.execute(
                    "INSERT INTO llm_providers (id, preset, kind, title, base_url, options, models, last_check,
                        created_at, updated_at)
                     VALUES (?1, ?2, 'openai', ?1, NULL, '{}', '[]', NULL, '2026-11-16', '2026-11-16')",
                    rusqlite::params![id, preset],
                )
            })
            .unwrap();
        }
        let settings = Arc::new(SettingsService::new(db.clone(), EventBus::new()));
        let router =
            Router::new(db, settings, EventBus::new(), pool::EnvModel { kind: DialogProvider::None, base_url: None });
        let spent = |provider: &str, cloud: bool| Spent {
            key: String::new(),
            by: Answerer { provider: provider.into(), cloud, ..answerer("m", Seat::Fast, Reason::Seat) },
            tokens: Tokens::default(),
            rounds: 1,
            ms: 0,
            outcome: Outcome::Ok,
            bill: None,
        };
        let deal = |provider: &str, cloud: bool| router.price(&spent(provider, cloud), Links::default(), 0);
        let away = deal("helper-own-a", true);
        assert_eq!((away.place.as_str(), away.billing.as_str()), ("mine", "key"));
        let home = deal("helper-own-a", false);
        assert_eq!((home.place.as_str(), home.billing.as_str(), home.usd_micro), ("home", "local", Some(0)));
        assert_eq!(deal("openrouter", true).place, "cloud");
    }
}
