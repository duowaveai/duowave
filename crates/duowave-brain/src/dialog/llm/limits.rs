//! A provider's limits (docs/plans/provider-limits.md): one logic for every service. A refusal is read as one of three
//! kinds — wait a minute, run out until a time, or the owner must act — and the two last put the provider on a pause:
//! no request goes to it until the limit's time, or until one probe in 15 minutes goes through. While it is paused
//! «Мозг» gives its questions to the backup at once (brain_service.rs, `Judged::Limited`), said once in words; without a
//! backup the station says when the limit renews, or — in «Вместе», as the owner chose — Alice answers.
//!
//! DuoWave's gateway also tells what is spent (`x-quota-*`, `GET /v1/quota`): kept here for the card, and said once a
//! day at 90 %.
//!
//! All in memory: a restart starts clean, the next request finds out again.

use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use chrono::{DateTime, Datelike, Duration, NaiveDateTime, Timelike, Utc};
use reqwest::header::HeaderMap;
use serde::Serialize;
use serde_json::Value;

use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use super::llm_client::{AccessProblem, ErrorWords, LlmClient, LlmEvent, LlmRequest, LlmStream, SpokenFailure};
use crate::dialog::generator::generate;

/// How often a paused provider is asked once more: its limit may be raised, its plan renewed.
pub const PROBE_EVERY_MINUTES: i64 = 15;
/// A probe that never told how it ended (its stream dropped) stops counting after this.
const PROBE_STALE_SECONDS: i64 = 120;
/// A `Retry-After` longer than this is no «through a minute»: the limit is out until a time.
const LONG_WAIT_SECONDS: u64 = 10 * 60;
/// The share of the day's limit spent at which the station says so, once a day.
pub const NEAR_PERCENT: u8 = 90;

/// What a refusal asks: wait a minute, wait until a time, or act.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    Wait,
    Until,
    Action,
}

/// Why a service refused, as the station and the panel word it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LimitReason {
    /// Too many requests a minute, or at once.
    Busy,
    /// Too many requests, and the service asks to wait long: until a time.
    Later,
    /// The day's limit (DuoWave's `day_limit`).
    Day,
    /// The week's — the plan period's — limit (DuoWave's `quota_exhausted`).
    Week,
    /// The subscription's limit (the ChatGPT plan).
    Plan,
    /// The service as a whole is busy (DuoWave's `service_busy`).
    Service,
    /// The money on the account ran out (OpenAI's `insufficient_quota`, OpenRouter's 402, Claude's credit balance).
    Credits,
    /// The access ended (DuoWave's 402).
    Ended,
    /// The key was revoked (DuoWave's 401).
    Revoked,
}

impl LimitReason {
    pub fn kind(self) -> LimitKind {
        match self {
            LimitReason::Busy => LimitKind::Wait,
            LimitReason::Later | LimitReason::Day | LimitReason::Week | LimitReason::Plan | LimitReason::Service => {
                LimitKind::Until
            }
            LimitReason::Credits | LimitReason::Ended | LimitReason::Revoked => LimitKind::Action,
        }
    }
}

/// One refusal read: why, and until when when the service said so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Limit {
    pub reason: LimitReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<DateTime<Utc>>,
    /// How long `Retry-After` asked to wait, for «через N мин».
    #[serde(skip)]
    pub wait_seconds: Option<u64>,
}

/// One of DuoWave's counters: spent, the limit, and when it renews (the day) or the period ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaPart {
    pub used: u64,
    pub limit: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,
}

impl QuotaPart {
    /// The share spent, 0–100: what is spent may pass the limit by the last request let through.
    pub fn percent(&self) -> u8 {
        if self.limit == 0 {
            return 100;
        }
        (self.used.saturating_mul(100) / self.limit).min(100) as u8
    }
}

/// What DuoWave's gateway said is spent, and when it said so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day: Option<QuotaPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<QuotaPart>,
    pub at: DateTime<Utc>,
}

/// A limit as the panel shows it on the provider's card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitView {
    pub reason: LimitReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<DateTime<Utc>>,
    /// The station's words for it now.
    pub words: String,
}

/// A held limit: the pause and what was said about it.
#[derive(Debug, Clone)]
struct Held {
    limit: Limit,
    next_probe: DateTime<Utc>,
    probing_since: Option<DateTime<Utc>>,
    /// The backup's one sentence was said.
    warned: bool,
    /// «Пока передавать вопросы Алисе?» was asked.
    asked: bool,
}

impl Held {
    fn probing(&self, now: DateTime<Utc>) -> bool {
        self.probing_since.is_some_and(|since| now - since < Duration::seconds(PROBE_STALE_SECONDS))
    }
}

#[derive(Debug, Default)]
struct CellState {
    /// DuoWave's last word on the access that a new connection fixes: the model is not in the plan.
    problem: Option<AccessProblem>,
    held: Option<Held>,
    quota: Option<Quota>,
    /// The day (by its reset) the 90 % line was said for.
    near_said: Option<DateTime<Utc>>,
    /// When `GET /v1/quota` was last asked.
    quota_asked: Option<DateTime<Utc>>,
}

#[derive(Debug)]
struct CellInner {
    title: String,
    words: ErrorWords,
    state: Mutex<CellState>,
}

/// One provider's limits, shared by its clients, «Мозг» and its card. One per provider row.
#[derive(Debug, Clone)]
pub struct ProviderCell(Arc<CellInner>);

/// Whether the claim of a request went through: it may go out (`Go`, `probe` when it is the paused provider's one
/// probe), or not — the words say why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    Go { probe: bool },
    Paused(Limit),
}

impl ProviderCell {
    pub fn new(title: &str, words: ErrorWords) -> ProviderCell {
        ProviderCell(Arc::new(CellInner { title: title.to_string(), words, state: Mutex::default() }))
    }

    fn state(&self) -> MutexGuard<'_, CellState> {
        self.0.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn title(&self) -> &str {
        &self.0.title
    }

    pub fn words(&self) -> ErrorWords {
        self.0.words
    }

    pub fn same(&self, other: &ProviderCell) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    // DuoWave's word on the access.

    pub fn problem(&self) -> Option<AccessProblem> {
        self.state().problem
    }

    pub fn set_problem(&self, problem: Option<AccessProblem>) {
        self.state().problem = problem;
    }

    /// A new connection (a key, a sign-in): what the old one was refused for goes.
    pub fn reset(&self) {
        let mut state = self.state();
        state.problem = None;
        state.held = None;
    }

    // The pause.

    /// The limit held now, paused or due for its probe.
    pub fn limit(&self) -> Option<Limit> {
        self.state().held.as_ref().map(|held| held.limit.clone())
    }

    /// Whether «Мозг» passes the provider by now: a limit is held and its probe is not due (or is under way).
    pub fn paused(&self) -> bool {
        self.paused_at(Utc::now())
    }

    pub fn paused_at(&self, now: DateTime<Utc>) -> bool {
        self.state().held.as_ref().is_some_and(|held| held.probing(now) || now < held.next_probe)
    }

    /// Right before a request: it goes out, or — paused — not, and no request goes to the service. While the probe is
    /// due exactly one request is let through as the probe; the others wait for its end.
    pub fn claim(&self) -> Claim {
        self.claim_at(Utc::now())
    }

    pub fn claim_at(&self, now: DateTime<Utc>) -> Claim {
        let mut state = self.state();
        let Some(held) = state.held.as_mut() else { return Claim::Go { probe: false } };
        if held.probing(now) || now < held.next_probe {
            return Claim::Paused(held.limit.clone());
        }
        held.probing_since = Some(now);
        Claim::Go { probe: true }
    }

    /// A refusal read as a limit. Wait is held by no pause (a probe under way ends, its next one in 15 minutes);
    /// until-a-time and act are: until the next probe. The same reason again keeps what was said about it.
    pub fn refused(&self, limit: &Limit) {
        self.refused_at(limit, Utc::now());
    }

    pub fn refused_at(&self, limit: &Limit, now: DateTime<Utc>) {
        let mut state = self.state();
        let next_probe = next_probe(limit.until, now);
        if limit.reason.kind() == LimitKind::Wait {
            if let Some(held) = state.held.as_mut().filter(|held| held.probing_since.is_some()) {
                held.probing_since = None;
                held.next_probe = next_probe;
            }
            return;
        }
        let (warned, asked) = match &state.held {
            Some(held) if held.limit.reason == limit.reason => (held.warned, held.asked),
            _ => (false, false),
        };
        state.held = Some(Held { limit: limit.clone(), next_probe, probing_since: None, warned, asked });
    }

    /// A request failed otherwise (no connection, the service's own error): a probe under way ends, the pause goes on.
    pub fn failed(&self) {
        self.failed_at(Utc::now());
    }

    pub fn failed_at(&self, now: DateTime<Utc>) {
        if let Some(held) = self.state().held.as_mut().filter(|held| held.probing_since.is_some()) {
            held.probing_since = None;
            held.next_probe = next_probe(held.limit.until, now);
        }
    }

    /// A request went through: the limit and the access's word go.
    pub fn answered(&self) {
        let mut state = self.state();
        state.held = None;
        state.problem = None;
    }

    /// The backup's one sentence for this pause: once, then None until another limit.
    pub fn take_warning(&self) -> Option<Limit> {
        let mut state = self.state();
        let held = state.held.as_mut().filter(|held| !held.warned)?;
        held.warned = true;
        Some(held.limit.clone())
    }

    /// «Пока передавать вопросы Алисе?» for this pause: once, then None until another limit.
    pub fn take_ask(&self) -> Option<Limit> {
        let mut state = self.state();
        let held = state.held.as_mut().filter(|held| !held.asked)?;
        held.asked = true;
        Some(held.limit.clone())
    }

    /// The station's words for the held limit now.
    pub fn phrase(&self) -> Option<String> {
        let limit = self.limit()?;
        Some(phrase(&limit, self.title(), Utc::now(), timezone().as_deref()))
    }

    /// The card's view of the held limit.
    pub fn view(&self) -> Option<LimitView> {
        let limit = self.limit()?;
        let words = phrase(&limit, self.title(), Utc::now(), timezone().as_deref());
        Some(LimitView { reason: limit.reason, until: limit.until, words })
    }

    // What DuoWave's gateway says is spent.

    /// The `x-quota-*` of a response, refusals too: none — the last snapshot stays.
    pub fn quota_seen(&self, headers: &HeaderMap) {
        self.quota_seen_at(headers, Utc::now());
    }

    pub fn quota_seen_at(&self, headers: &HeaderMap, now: DateTime<Utc>) {
        if let Some(quota) = quota_of_headers(headers, now) {
            self.state().quota = Some(quota);
        }
    }

    /// The answer of `GET /v1/quota`.
    pub fn quota_read(&self, quota: Quota) {
        self.state().quota = Some(quota);
    }

    pub fn quota(&self) -> Option<Quota> {
        self.state().quota.clone()
    }

    /// Whether `GET /v1/quota` may be asked now: not more than once a minute, and not while the answers' headers are
    /// fresh. Marks it asked.
    pub fn take_quota_turn(&self, now: DateTime<Utc>) -> bool {
        let mut state = self.state();
        let minute = Duration::seconds(60);
        let fresh = state.quota.as_ref().is_some_and(|quota| now - quota.at < minute);
        if fresh || state.quota_asked.is_some_and(|asked| now - asked < minute) {
            return false;
        }
        state.quota_asked = Some(now);
        true
    }

    /// The 90 % line, once a day (by the day's reset): Some(words) the first time the day's share reaches it.
    pub fn take_near(&self) -> Option<String> {
        let mut state = self.state();
        let day = state.quota.as_ref()?.day.clone()?;
        if day.percent() < NEAR_PERCENT || day.percent() >= 100 {
            return None;
        }
        let key = day.resets_at.unwrap_or(DateTime::<Utc>::MIN_UTC);
        if state.near_said == Some(key) {
            return None;
        }
        state.near_said = Some(key);
        Some(crate::tr!("Дневной лимит {title} почти закончился.", title = self.title()))
    }
}

/// The next probe: in 15 minutes, or sooner at the limit's own time.
fn next_probe(until: Option<DateTime<Utc>>, now: DateTime<Utc>) -> DateTime<Utc> {
    let later = now + Duration::minutes(PROBE_EVERY_MINUTES);
    match until {
        Some(until) if until > now && until < later => until,
        _ => later,
    }
}

// Reading a refusal.

/// The `x-quota-*` pair of a counter, with its time: none without both numbers.
fn quota_part(headers: &HeaderMap, name: &str, time: &str) -> Option<QuotaPart> {
    let number = |header: String| headers.get(header)?.to_str().ok()?.trim().parse::<u64>().ok();
    let used = number(format!("x-quota-{name}-used"))?;
    let limit = number(format!("x-quota-{name}-limit"))?;
    let resets_at =
        headers.get(format!("x-quota-{name}-{time}")).and_then(|value| value.to_str().ok()).and_then(parse_time);
    Some(QuotaPart { used, limit, resets_at })
}

/// DuoWave's `x-quota-*` headers (docs: quota-headers.md): none of them — None.
pub fn quota_of_headers(headers: &HeaderMap, now: DateTime<Utc>) -> Option<Quota> {
    let day = quota_part(headers, "day", "reset");
    let period = quota_part(headers, "period", "ends");
    (day.is_some() || period.is_some()).then_some(Quota { day, period, at: now })
}

/// The answer of `GET /v1/quota`: `day` and `period` null when the plan has no such limit.
pub fn quota_of_body(body: &Value, now: DateTime<Utc>) -> Option<Quota> {
    let part = |name: &str, time: &str| {
        let part = body.get(name)?;
        Some(QuotaPart {
            used: part.get("used")?.as_u64()?,
            limit: part.get("limit")?.as_u64()?,
            resets_at: part.get(time).and_then(Value::as_str).and_then(parse_time),
        })
    };
    body.as_object()?;
    Some(Quota { day: part("day", "resets_at"), period: part("period", "ends_at"), at: now })
}

fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text.trim()).ok().map(|at| at.with_timezone(&Utc))
}

/// A service's refusal as a limit; None — not a limit (a wrong request, the service's own fault, a key refused by a
/// service that does not say why in words: they stay plain errors). `words` says whose codes are read.
pub fn limit_of(words: ErrorWords, status: u16, headers: &HeaderMap, body: &str, now: DateTime<Utc>) -> Option<Limit> {
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let error = parsed.get("error").unwrap_or(&parsed);
    let code = error.get("code").and_then(Value::as_str).unwrap_or_default().to_lowercase();
    let message = error.get("message").and_then(Value::as_str).unwrap_or_default().to_lowercase();
    let wait = super::llm_client::retry_after_secs(headers).map(|after| after.as_secs());
    let after = wait.map(|seconds| now + Duration::seconds(seconds as i64));
    let header_time = |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).and_then(parse_time);
    let limit = |reason: LimitReason, until: Option<DateTime<Utc>>| Some(Limit { reason, until, wait_seconds: wait });
    // Too many requests: a minute's wait, or — asked to wait long — until a time.
    let busy = || match wait {
        Some(seconds) if seconds > LONG_WAIT_SECONDS => limit(LimitReason::Later, after),
        _ => limit(LimitReason::Busy, after),
    };
    if words == ErrorWords::DuoWave {
        return match (status, code.as_str()) {
            (401, _) => limit(LimitReason::Revoked, None),
            (402, "quota_exhausted") => limit(LimitReason::Week, after.or_else(|| header_time("x-quota-period-ends"))),
            (402, _) => limit(LimitReason::Ended, None),
            (429, "day_limit") => limit(LimitReason::Day, after.or_else(|| header_time("x-quota-day-reset"))),
            (429, "rate_limited" | "too_many_parallel") => limit(LimitReason::Busy, after),
            (429, _) => busy(),
            (503, "service_busy") => limit(LimitReason::Service, after),
            _ => None,
        };
    }
    // The ChatGPT plan: its own code; the time when it is told.
    if code.contains("usage_limit") {
        let told = error
            .get("resets_in_seconds")
            .and_then(Value::as_i64)
            .map(|seconds| now + Duration::seconds(seconds))
            .or_else(|| error.get("resets_at").and_then(Value::as_i64).and_then(|at| DateTime::from_timestamp(at, 0)));
        return limit(LimitReason::Plan, told.or(after));
    }
    match status {
        // OpenAI's money ran out: a 429 with its own code.
        _ if code == "insufficient_quota" => limit(LimitReason::Credits, None),
        402 => limit(LimitReason::Credits, None),
        // Claude's money: a 400 that says so.
        400 if message.contains("credit balance") => limit(LimitReason::Credits, None),
        429 => busy(),
        _ => None,
    }
}

/// A limit as the error a client returns: the station's words, the detail for the log. Held limits are kept on the
/// cell first.
pub fn failure(cell: &ProviderCell, limit: &Limit, status: Option<u16>, code: &str) -> anyhow::Error {
    let phrase = phrase(limit, cell.title(), Utc::now(), timezone().as_deref());
    let status = status.map(|status| format!("HTTP {status}")).unwrap_or_else(|| "paused".into());
    let code = if code.is_empty() { String::new() } else { format!(": {code}") };
    let until = limit.until.map(|until| format!(" until {}", until.to_rfc3339())).unwrap_or_default();
    anyhow::Error::new(SpokenFailure {
        phrase,
        detail: format!("{}: {status}{code} ({:?}{until})", cell.title(), limit.reason),
    })
}

/// The refusal's code, for the log: `error.code`, else `error.type`.
pub fn code_of(body: &str) -> String {
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let error = parsed.get("error").unwrap_or(&parsed);
    ["code", "type"]
        .iter()
        .find_map(|name| error.get(*name).and_then(Value::as_str).filter(|code| !code.is_empty()))
        .unwrap_or_default()
        .to_string()
}

// The station's time zone.

static TIMEZONE: RwLock<Option<String>> = RwLock::new(None);

/// The time zone the station last named (an answer's context): the limits' times are said in it.
pub fn set_timezone(timezone: Option<&str>) {
    let Some(timezone) = timezone.filter(|timezone| !timezone.is_empty()) else { return };
    let mut kept = TIMEZONE.write().unwrap_or_else(|poisoned| poisoned.into_inner());
    if kept.as_deref() != Some(timezone) {
        *kept = Some(timezone.to_string());
    }
}

fn timezone() -> Option<String> {
    TIMEZONE.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

// The words.

/// The genitive month, translated.
fn month_name(month0: u32) -> String {
    match month0 {
        0 => crate::tr!("января"),
        1 => crate::tr!("февраля"),
        2 => crate::tr!("марта"),
        3 => crate::tr!("апреля"),
        4 => crate::tr!("мая"),
        5 => crate::tr!("июня"),
        6 => crate::tr!("июля"),
        7 => crate::tr!("августа"),
        8 => crate::tr!("сентября"),
        9 => crate::tr!("октября"),
        10 => crate::tr!("ноября"),
        _ => crate::tr!("декабря"),
    }
}

fn local(at: DateTime<Utc>, timezone: Option<&str>) -> Option<NaiveDateTime> {
    crate::skills::own::clock_skill::wall_clock(at, timezone)
}

/// When, where the station is: «в 7:00» today, «завтра в 7:00», «28 ноября в 7:00» later.
fn when_words(until: DateTime<Utc>, now: DateTime<Utc>, timezone: Option<&str>) -> Option<String> {
    let (at, today) = (local(until, timezone)?, local(now, timezone)?);
    let time = format!("{}:{:02}", at.hour(), at.minute());
    let days = (at.date() - today.date()).num_days();
    Some(match days {
        ..=0 => crate::tr!("в {time}", time = time),
        1 => crate::tr!("завтра в {time}", time = time),
        _ => crate::tr!("{day} {month} в {time}", day = at.day(), month = month_name(at.month0()), time = time),
    })
}

/// «Попробуйте после …»: «после 7:00» today, «завтра после 7:00», «28 ноября после 7:00» later.
fn after_words(until: DateTime<Utc>, now: DateTime<Utc>, timezone: Option<&str>) -> Option<String> {
    let (at, today) = (local(until, timezone)?, local(now, timezone)?);
    let time = format!("{}:{:02}", at.hour(), at.minute());
    Some(match (at.date() - today.date()).num_days() {
        ..=0 => crate::tr!("после {time}", time = time),
        1 => crate::tr!("завтра после {time}", time = time),
        _ => crate::tr!("{day} {month} после {time}", day = at.day(), month = month_name(at.month0()), time = time),
    })
}

/// The date only: «28 ноября», «завтра».
fn date_words(until: DateTime<Utc>, now: DateTime<Utc>, timezone: Option<&str>) -> Option<String> {
    let (at, today) = (local(until, timezone)?, local(now, timezone)?);
    Some(match (at.date() - today.date()).num_days() {
        ..=0 => crate::tr!("сегодня"),
        1 => crate::tr!("завтра"),
        _ => crate::tr!("{day} {month}", day = at.day(), month = month_name(at.month0())),
    })
}

/// «Через минуту», «через N мин», «позже».
fn wait_words(seconds: Option<u64>) -> String {
    match seconds.map(|seconds| seconds.div_ceil(60)) {
        None | Some(0) => crate::tr!("позже"),
        Some(1) => crate::tr!("через минуту"),
        Some(minutes) => crate::tr!("через {minutes} мин", minutes = minutes),
    }
}

/// What happened, in one sentence: «Дневной лимит DuoWave закончился.»
pub fn headline(limit: &Limit, title: &str) -> String {
    match limit.reason {
        LimitReason::Busy | LimitReason::Later => crate::tr!("Слишком много запросов к {title}.", title = title),
        LimitReason::Day => crate::tr!("Дневной лимит {title} закончился.", title = title),
        LimitReason::Week => crate::tr!("Лимит {title} на эту неделю закончился.", title = title),
        LimitReason::Plan => crate::tr!("Лимит подписки {title} закончился.", title = title),
        LimitReason::Service => crate::tr!("{title} сейчас перегружен.", title = title),
        LimitReason::Credits => crate::tr!("Деньги на счёте {title} закончились.", title = title),
        LimitReason::Ended => crate::tr!("Доступ к {title} закончился.", title = title),
        LimitReason::Revoked => crate::tr!("Подключение {title} отключено.", title = title),
    }
}

/// The station's words without a backup: what happened and when it renews, or what to do.
pub fn phrase(limit: &Limit, title: &str, now: DateTime<Utc>, timezone: Option<&str>) -> String {
    let head = headline(limit, title);
    let when = limit.until.and_then(|until| when_words(until, now, timezone));
    let tail = match limit.reason {
        LimitReason::Busy => crate::tr!("Попробуйте {when}.", when = wait_words(limit.wait_seconds)),
        LimitReason::Later | LimitReason::Service => {
            match limit.until.and_then(|until| after_words(until, now, timezone)) {
                Some(after) => crate::tr!("Попробуйте {after}.", after = after),
                None => crate::tr!("Попробуйте позже."),
            }
        }
        LimitReason::Day => match when {
            Some(when) => crate::tr!("Он обновится {when}.", when = when),
            None => crate::tr!("Он обновится завтра."),
        },
        LimitReason::Week => match limit.until.and_then(|until| date_words(until, now, timezone)) {
            Some(date) => crate::tr!("Он обновится {date}.", date = date),
            None => crate::tr!("Загляните в кабинет {title}.", title = title),
        },
        LimitReason::Plan => match when {
            Some(when) => crate::tr!("Он обновится {when}.", when = when),
            None => crate::tr!("Выберите в пульте другую модель или подождите."),
        },
        LimitReason::Credits => crate::tr!("Пополните счёт или выберите в пульте другую модель."),
        LimitReason::Ended => crate::tr!("Загляните в кабинет {title}.", title = title),
        LimitReason::Revoked => crate::tr!("Подключите {title} заново.", title = title),
    };
    format!("{head} {tail}")
}

/// The backup's one sentence: «Дневной лимит DuoWave закончился. До 7:00 отвечает OpenRouter.»
pub fn notice(limit: &Limit, title: &str, backup: &str, now: DateTime<Utc>, timezone: Option<&str>) -> String {
    let head = headline(limit, title);
    let until = match limit.reason.kind() {
        LimitKind::Until => limit.until.and_then(|until| local(until, timezone).zip(local(now, timezone))),
        _ => None,
    };
    let tail = match until {
        Some((at, today)) if at.date() == today.date() => {
            crate::tr!(
                "До {time} отвечает {backup}.",
                time = format!("{}:{:02}", at.hour(), at.minute()),
                backup = backup
            )
        }
        _ => crate::tr!("Пока отвечает {backup}.", backup = backup),
    };
    format!("{head} {tail}")
}

/// The question in «Вместе», asked once a pause: the words, then «Пока передавать вопросы Алисе?».
pub fn ask_words(limit: &Limit, title: &str, now: DateTime<Utc>, timezone: Option<&str>) -> String {
    format!("{} {}", phrase(limit, title, now, timezone), crate::tr!("Пока передавать вопросы Алисе?"))
}

/// The same with the station's last time zone and the clock now.
pub fn ask_words_now(limit: &Limit, title: &str) -> String {
    ask_words(limit, title, Utc::now(), timezone().as_deref())
}

/// The backup's sentence with the station's last time zone and the clock now.
pub fn notice_now(limit: &Limit, title: &str, backup: &str) -> String {
    notice(limit, title, backup, Utc::now(), timezone().as_deref())
}

static YES: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new("^(да|давай|давайте|конечно|ага|угу|передавай|передавайте|да передавай|да давай|можно|хорошо)$")
        .expect("a valid pattern")
});
static NO: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new("^(нет|не надо|не нужно|не передавай|не передавайте|нет спасибо|нет не надо|не стоит)$")
        .expect("a valid pattern")
});

/// The answer to «Пока передавать вопросы Алисе?»: yes, no, or None — something else (a new question).
pub fn yes_or_no(text: &str) -> Option<bool> {
    let said = crate::skills::text::normalize(text);
    if YES.is_match(&said) {
        Some(true)
    } else if NO.is_match(&said) {
        Some(false)
    } else {
        None
    }
}

/// A model that speaks of its provider's limit (brain_service.rs `speaking`): as the backup taking over from a limit it
/// says so first, once a pause — «Дневной лимит DuoWave закончился. До 7:00 отвечает OpenRouter.»; as the chosen model
/// it adds the day's 90 % to its last words, once a day. Only the answers people hear are wrapped.
pub struct Spoken {
    inner: Arc<dyn LlmClient>,
    cell: ProviderCell,
    /// The backup's title: it says the limit's sentence; None — the chosen model itself.
    backup: Option<String>,
}

impl Spoken {
    pub fn new(inner: Arc<dyn LlmClient>, cell: ProviderCell, backup: Option<String>) -> Spoken {
        Spoken { inner, cell, backup }
    }
}

impl LlmClient for Spoken {
    fn description(&self) -> String {
        self.inner.description()
    }

    fn tools(&self) -> bool {
        self.inner.tools()
    }

    fn stream(&self, request: LlmRequest, cancel: CancellationToken) -> LlmStream {
        // Asked when the round starts: the backup only once the chosen model failed, so the limit is the one it failed
        // with; taken once, never again this pause.
        let first = self
            .backup
            .as_ref()
            .and_then(|backup| self.cell.take_warning().map(|limit| notice_now(&limit, self.cell.title(), backup)));
        let near = self.backup.is_none().then(|| self.cell.clone());
        let mut events = self.inner.stream(request, cancel);
        generate(move |y| async move {
            if let Some(first) = first {
                y.send(Ok(LlmEvent::Text(format!("{first} ")))).await;
            }
            while let Some(item) = events.next().await {
                // The last words of an answer (a turn with no calls): the 90 % after them.
                if let (Some(cell), Ok(LlmEvent::Turn(turn))) = (&near, &item) {
                    if turn.tool_calls.is_empty() && !turn.content.trim().is_empty() {
                        if let Some(line) = cell.take_near() {
                            y.send(Ok(LlmEvent::Text(format!(" {line}")))).await;
                        }
                    }
                }
                y.send(item).await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use reqwest::header::{HeaderName, HeaderValue};

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(HeaderName::from_bytes(name.as_bytes()).unwrap(), HeaderValue::from_str(value).unwrap());
        }
        map
    }

    /// 23.11, 17:52 UTC — 00:52 in Jakarta the next day; the limit renews at 00:00 UTC, 07:00 there.
    fn evening() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 22, 17, 52, 0).unwrap()
    }

    fn body(code: &str) -> String {
        format!(r#"{{"error":{{"code":"{code}","message":"x","type":"rate_limit_error"}}}}"#)
    }

    #[test]
    fn reads_every_services_refusals_as_three_kinds() {
        let now = evening();
        let duowave = |status: u16, code: &str, after: Option<&str>| {
            let headers = headers(&after.map(|after| vec![("retry-after", after)]).unwrap_or_default());
            limit_of(ErrorWords::DuoWave, status, &headers, &body(code), now).map(|limit| limit.reason)
        };
        assert_eq!(duowave(429, "day_limit", Some("22080")), Some(LimitReason::Day));
        assert_eq!(duowave(429, "rate_limited", Some("30")), Some(LimitReason::Busy));
        assert_eq!(duowave(429, "too_many_parallel", Some("2")), Some(LimitReason::Busy));
        assert_eq!(duowave(429, "", Some("3600")), Some(LimitReason::Later));
        assert_eq!(duowave(429, "", Some("60")), Some(LimitReason::Busy));
        assert_eq!(duowave(402, "quota_exhausted", None), Some(LimitReason::Week));
        assert_eq!(duowave(402, "no_access", None), Some(LimitReason::Ended));
        assert_eq!(duowave(401, "unauthorized", None), Some(LimitReason::Revoked));
        assert_eq!(duowave(503, "service_busy", Some("600")), Some(LimitReason::Service));
        assert_eq!(duowave(403, "model_not_in_plan", None), None, "a model, not a limit");
        assert_eq!(duowave(500, "internal", None), None);

        let plain = |status: u16, body: &str, after: Option<&str>| {
            let headers = headers(&after.map(|after| vec![("retry-after", after)]).unwrap_or_default());
            limit_of(ErrorWords::Plain, status, &headers, body, now).map(|limit| limit.reason)
        };
        assert_eq!(plain(429, &body("insufficient_quota"), None), Some(LimitReason::Credits), "OpenAI's money");
        assert_eq!(plain(429, &body("rate_limit_exceeded"), Some("20")), Some(LimitReason::Busy));
        assert_eq!(
            plain(429, r#"{"error":{"message":"free models per day"}}"#, Some("40000")),
            Some(LimitReason::Later)
        );
        assert_eq!(plain(402, r#"{"error":{"message":"Insufficient credits"}}"#, None), Some(LimitReason::Credits));
        let claude =
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low"}}"#;
        assert_eq!(plain(400, claude, None), Some(LimitReason::Credits));
        assert_eq!(plain(400, r#"{"error":{"message":"max_tokens is too large"}}"#, None), None);
        assert_eq!(plain(401, r#"{"error":"bad key"}"#, None), None, "a key refused stays a plain error");
        assert_eq!(plain(429, &body("subscription_sharing_usage_limit_exceeded"), None), Some(LimitReason::Plan));
    }

    #[test]
    fn the_day_limits_time_comes_from_retry_after_or_the_quota_header() {
        let now = evening();
        let by_after =
            limit_of(ErrorWords::DuoWave, 429, &headers(&[("retry-after", "22080")]), &body("day_limit"), now);
        assert_eq!(by_after.unwrap().until, Some(Utc.with_ymd_and_hms(2026, 11, 23, 0, 0, 0).unwrap()));
        let by_header = limit_of(
            ErrorWords::DuoWave,
            429,
            &headers(&[("x-quota-day-reset", "2026-11-23T00:00:00.000Z")]),
            &body("day_limit"),
            now,
        );
        assert_eq!(by_header.unwrap().until, Some(Utc.with_ymd_and_hms(2026, 11, 23, 0, 0, 0).unwrap()));
    }

    #[test]
    fn says_when_the_limit_renews_where_the_station_is() {
        let now = evening();
        let day = Limit { reason: LimitReason::Day, until: Some(now + Duration::seconds(22080)), wait_seconds: None };
        // 00:52 in Jakarta: the renewal at 07:00 is the same local day.
        assert_eq!(
            phrase(&day, "DuoWave", now, Some("Asia/Jakarta")),
            "Дневной лимит DuoWave закончился. Он обновится в 7:00."
        );
        // In Moscow it is 20:52: 03:00 tomorrow.
        assert_eq!(
            phrase(&day, "DuoWave", now, Some("Europe/Moscow")),
            "Дневной лимит DuoWave закончился. Он обновится завтра в 3:00."
        );
        let unknown = Limit { until: None, ..day.clone() };
        assert_eq!(
            phrase(&unknown, "DuoWave", now, Some("Asia/Jakarta")),
            "Дневной лимит DuoWave закончился. Он обновится завтра."
        );
        let week = Limit {
            reason: LimitReason::Week,
            until: Some(Utc.with_ymd_and_hms(2026, 11, 28, 9, 14, 2).unwrap()),
            wait_seconds: None,
        };
        assert_eq!(
            phrase(&week, "DuoWave", now, Some("Asia/Jakarta")),
            "Лимит DuoWave на эту неделю закончился. Он обновится 28 ноября."
        );
        let busy = Limit { reason: LimitReason::Busy, until: None, wait_seconds: Some(30) };
        assert_eq!(phrase(&busy, "DuoWave", now, None), "Слишком много запросов к DuoWave. Попробуйте через минуту.");
        let later = Limit {
            reason: LimitReason::Later,
            until: Some(now + Duration::seconds(22080)),
            wait_seconds: Some(22080),
        };
        assert_eq!(
            phrase(&later, "OpenRouter", now, Some("Asia/Jakarta")),
            "Слишком много запросов к OpenRouter. Попробуйте после 7:00."
        );
        let credits = Limit { reason: LimitReason::Credits, until: None, wait_seconds: None };
        assert_eq!(
            phrase(&credits, "OpenAI", now, None),
            "Деньги на счёте OpenAI закончились. Пополните счёт или выберите в пульте другую модель."
        );
        assert_eq!(
            notice(&day, "DuoWave", "OpenRouter", now, Some("Asia/Jakarta")),
            "Дневной лимит DuoWave закончился. До 7:00 отвечает OpenRouter."
        );
        assert_eq!(
            notice(&day, "DuoWave", "OpenRouter", now, Some("Europe/Moscow")),
            "Дневной лимит DuoWave закончился. Пока отвечает OpenRouter.",
            "tomorrow's time is no «до»"
        );
        assert_eq!(
            ask_words(&day, "DuoWave", now, Some("Asia/Jakarta")),
            "Дневной лимит DuoWave закончился. Он обновится в 7:00. Пока передавать вопросы Алисе?"
        );
    }

    #[test]
    fn a_pause_lets_one_probe_through_in_fifteen_minutes_and_an_answer_ends_it() {
        let now = evening();
        let cell = ProviderCell::new("DuoWave", ErrorWords::DuoWave);
        assert_eq!(cell.claim_at(now), Claim::Go { probe: false });
        let day = Limit { reason: LimitReason::Day, until: Some(now + Duration::hours(6)), wait_seconds: None };
        cell.refused_at(&day, now);
        assert!(cell.paused_at(now));
        assert_eq!(cell.claim_at(now + Duration::minutes(5)), Claim::Paused(day.clone()), "no request while paused");
        let due = now + Duration::minutes(15);
        assert!(!cell.paused_at(due), "the probe is due: «Мозг» asks it");
        assert_eq!(cell.claim_at(due), Claim::Go { probe: true });
        assert_eq!(cell.claim_at(due), Claim::Paused(day.clone()), "one probe at a time");
        assert!(cell.paused_at(due), "while the probe is out, the others go to the backup");
        // Refused again: 15 more minutes, the words not said twice.
        assert_eq!(cell.take_warning(), Some(day.clone()));
        cell.refused_at(&day, due);
        assert_eq!(cell.take_warning(), None, "said once a pause");
        assert_eq!(cell.claim_at(due + Duration::minutes(14)), Claim::Paused(day.clone()));
        // A probe that never reported stops counting after two minutes.
        let second = due + Duration::minutes(15);
        assert_eq!(cell.claim_at(second), Claim::Go { probe: true });
        assert_eq!(cell.claim_at(second + Duration::minutes(3)), Claim::Go { probe: true });
        // A failure of another kind: the pause goes on.
        cell.failed_at(second + Duration::minutes(3));
        assert!(cell.paused_at(second + Duration::minutes(4)));
        // An answer ends it.
        cell.answered();
        assert!(!cell.paused_at(second + Duration::minutes(4)));
        assert_eq!(cell.limit(), None);
    }

    #[test]
    fn the_probe_comes_at_the_limits_own_time_when_it_is_sooner() {
        let now = evening();
        let cell = ProviderCell::new("DuoWave", ErrorWords::DuoWave);
        let soon = Limit { reason: LimitReason::Day, until: Some(now + Duration::minutes(4)), wait_seconds: None };
        cell.refused_at(&soon, now);
        assert!(cell.paused_at(now + Duration::minutes(3)));
        assert!(!cell.paused_at(now + Duration::minutes(4)));
    }

    #[test]
    fn hears_yes_and_no_and_nothing_else() {
        assert_eq!(yes_or_no("Да."), Some(true));
        assert_eq!(yes_or_no("Алиса, давай"), Some(true));
        assert_eq!(yes_or_no("Нет, не надо"), Some(false));
        assert_eq!(yes_or_no("какая погода"), None);
        assert_eq!(yes_or_no("да нет наверное"), None);
    }

    #[test]
    fn a_minutes_wait_holds_no_pause() {
        let now = evening();
        let cell = ProviderCell::new("DuoWave", ErrorWords::DuoWave);
        cell.refused_at(&Limit { reason: LimitReason::Busy, until: None, wait_seconds: Some(30) }, now);
        assert!(!cell.paused_at(now));
        assert_eq!(cell.limit(), None);
    }

    #[test]
    fn reads_the_quota_headers_and_says_ninety_percent_once_a_day() {
        let now = evening();
        let cell = ProviderCell::new("DuoWave", ErrorWords::DuoWave);
        cell.quota_seen_at(&headers(&[]), now);
        assert_eq!(cell.quota(), None, "no headers: no data, not zero");
        cell.quota_seen_at(
            &headers(&[
                ("x-quota-day-used", "243118"),
                ("x-quota-day-limit", "300000"),
                ("x-quota-day-reset", "2026-11-23T00:00:00.000Z"),
            ]),
            now,
        );
        let quota = cell.quota().unwrap();
        assert_eq!(quota.day.as_ref().unwrap().percent(), 81);
        assert_eq!(quota.period, None, "only the day's triple");
        assert_eq!(cell.take_near(), None, "81 %");
        let full = [
            ("x-quota-day-used", "271000"),
            ("x-quota-day-limit", "300000"),
            ("x-quota-day-reset", "2026-11-23T00:00:00.000Z"),
            ("x-quota-period-used", "612400"),
            ("x-quota-period-limit", "1000000"),
            ("x-quota-period-ends", "2026-11-28T09:14:02.000Z"),
        ];
        cell.quota_seen_at(&headers(&full), now);
        assert_eq!(cell.quota().unwrap().period.unwrap().percent(), 61);
        assert_eq!(cell.take_near().as_deref(), Some("Дневной лимит DuoWave почти закончился."));
        assert_eq!(cell.take_near(), None, "once a day");
        let next_day = [
            ("x-quota-day-used", "280000"),
            ("x-quota-day-limit", "300000"),
            ("x-quota-day-reset", "2026-11-24T00:00:00.000Z"),
        ];
        cell.quota_seen_at(&headers(&next_day), now + Duration::days(1));
        assert!(cell.take_near().is_some(), "a new day");
        let over = QuotaPart { used: 304000, limit: 300000, resets_at: None };
        assert_eq!(over.percent(), 100, "what passes the limit is cut to 100");
        let body = serde_json::json!({
            "plan": {"tier": "trial", "title": "Пробная неделя"},
            "day": {"used": 243118, "limit": 300000, "resets_at": "2026-11-24T00:00:00.000Z"},
            "period": null,
            "now": "2026-11-23T17:40:11.512Z"
        });
        let read = quota_of_body(&body, now).unwrap();
        assert_eq!((read.day.unwrap().percent(), read.period), (81, None));
    }

    #[test]
    fn the_quota_is_asked_at_most_once_a_minute_and_not_while_the_headers_are_fresh() {
        let now = evening();
        let cell = ProviderCell::new("DuoWave", ErrorWords::DuoWave);
        assert!(cell.take_quota_turn(now));
        assert!(!cell.take_quota_turn(now + Duration::seconds(30)));
        assert!(cell.take_quota_turn(now + Duration::seconds(61)));
        cell.quota_seen_at(
            &headers(&[("x-quota-day-used", "1"), ("x-quota-day-limit", "2")]),
            now + Duration::seconds(200),
        );
        assert!(!cell.take_quota_turn(now + Duration::seconds(230)), "the answers told it");
    }
}
