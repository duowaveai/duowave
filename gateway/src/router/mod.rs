//! The router (docs/plans/router-and-works.md): which model takes a call, and what it cost. Stage 1 is the journal of
//! deals and the prices. Every answer of a model leaves a deal: the seat that asked, the model that answered and why,
//! its tokens, its dollars at the price of the day and that price itself (`deals.rs`). A price comes, in this order,
//! from the panel, from the provider's own list, from the snapshot built into the program — or there is none
//! (`pool.rs`). In both builds, small: no network, no crates of its own. The models are still chosen as before; the
//! door with its seats, frames, table and budgets is stage 2.
//!
//! An answer's rounds are summed by its `Tally`: the dialog asks it before each round and tells it each round's
//! spending; the tally passes every round on to the sink (the journal's request, the work) and, once the answer is
//! over — ended, failed or dropped — closes it into one deal per model that answered: one, or two when «Запасная»
//! took over midway. The deal is written after the answer, never on its way.

pub mod deals;
pub mod pool;
pub mod routes;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::config::{DialogProvider, ModelProfile};
use crate::core::events::{EventBus, GatewayEvent};
use crate::dialog::llm::brain::ModelSlot;
use crate::dialog::llm::LlmUsage;
use crate::storage::Db;
use crate::utils::time::now_ms;

pub use deals::{Answered, Deal, Deals};
pub use pool::{Pool, Price};

/// How often the old deals and days are cleaned: once at the start, then daily.
const CLEAN_EVERY: Duration = Duration::from_secs(24 * 3600);

/// The seat a model answers on: the conversation's two (by the rule of the phrase), «Запасная» once it took over, the
/// workshop's agent, the memory's night, an app. The seats of the works in folders come with their stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Seat {
    Fast,
    Smart,
    Backup,
    Workshop,
    Memory,
    App,
}

impl Seat {
    pub const ALL: [Seat; 6] = [Seat::Fast, Seat::Smart, Seat::Backup, Seat::Workshop, Seat::Memory, Seat::App];

    pub fn as_str(self) -> &'static str {
        match self {
            Seat::Fast => "fast",
            Seat::Smart => "smart",
            Seat::Backup => "backup",
            Seat::Workshop => "workshop",
            Seat::Memory => "memory",
            Seat::App => "app",
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
/// hard question, a short one; what stands on the seat; «Запасная», which took over from a model that gave no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    RuleCommand,
    RuleHard,
    RuleShort,
    Seat,
    Backup,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::RuleCommand => "rule:command",
            Reason::RuleHard => "rule:hard",
            Reason::RuleShort => "rule:short",
            Reason::Seat => "seat",
            Reason::Backup => "backup",
        }
    }
}

/// How a deal is paid: by a key; by a plan's sign-in (ChatGPT's: the dollars are what a key would have cost); at home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Billing {
    Key,
    Plan,
    Local,
}

impl Billing {
    pub fn as_str(self) -> &'static str {
        match self {
            Billing::Key => "key",
            Billing::Plan => "plan",
            Billing::Local => "local",
        }
    }
}

/// Where the model is: at home, in one's own cloud (the regulator's class of «Дом и помощники», stage 3 — until then
/// every model away from home is `cloud`), in someone else's cloud.
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
/// asked for words); stopped (cancelled, or dropped in the middle of a round). `budget` comes with the budgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Failed,
    Limit,
    Cut,
    Stopped,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Failed => "failed",
            Outcome::Limit => "limit",
            Outcome::Cut => "cut",
            Outcome::Stopped => "stopped",
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
    /// The conversation it was in — a device's, `panel:<id>`, `workshop:make:<draft>`; empty — none.
    pub key: String,
    pub by: Answerer,
    pub tokens: Tokens,
    pub rounds: u32,
    pub ms: u64,
    pub outcome: Outcome,
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
    /// An answer of `asked` (its seat and reason) in the conversation `key`; without a sink it counts for nobody.
    pub fn new(sink: Option<UsageSink>, key: &str, asked: Answerer) -> Arc<Tally> {
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

/// The journal of deals and the prices behind it, as the gateway holds them.
pub struct Router {
    pub deals: Deals,
    pub pool: Pool,
    events: EventBus,
    cancel: CancellationToken,
    /// Who answered each work's last round, by the work's id: the page «Работы» names the model of a work going on
    /// before its deal is written — the workshop's agent closes one deal at the end of its run. Not kept.
    heard: Mutex<HashMap<String, (Answered, u64)>>,
}

/// A work's rounds heard are remembered for this many works: the oldest go first.
const HEARD_MAX: usize = 200;

impl Router {
    pub fn new(db: Arc<Db>, events: EventBus, env: pool::EnvModel) -> Arc<Router> {
        Arc::new(Router {
            deals: Deals::new(db.clone()),
            pool: Pool::new(db, env),
            events,
            cancel: CancellationToken::new(),
            heard: Mutex::new(HashMap::new()),
        })
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
        let (place, billing, price) = if by.cloud {
            let book = self.pool.book(&by.provider);
            let billing = if book.plan { Billing::Plan } else { Billing::Key };
            (Place::Cloud, billing, self.pool.price(&by.provider, book.preset.as_deref(), &by.model))
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
        assert_eq!((answers[0].by.seat, answers[0].by.reason), (Seat::Smart, Reason::RuleHard));
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
        assert_eq!(Seat::ALL.map(Seat::as_str), ["fast", "smart", "backup", "workshop", "memory", "app"]);
        assert_eq!(Seat::parse("memory"), Some(Seat::Memory));
        assert_eq!(Seat::parse("planner"), None);
        assert_eq!((Seat::of(ModelProfile::Fast), Seat::of(ModelProfile::Smart)), (Seat::Fast, Seat::Smart));
        let reasons = [Reason::RuleCommand, Reason::RuleHard, Reason::RuleShort, Reason::Seat, Reason::Backup];
        assert_eq!(reasons.map(Reason::as_str), ["rule:command", "rule:hard", "rule:short", "seat", "backup"]);
    }
}
