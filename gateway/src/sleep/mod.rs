//! The sleep (docs/plans/memory.md, «Сон и рефлексия»): the gateway's night work, in parts. At the sleep's hour on
//! the station's clock (04:00 unless set) a night starts — only when a part that is on has work. The parts go one by
//! one, each with its own time; one failing does not stop the others. A part that could not finish (no model to think
//! with, the model did not answer) is tried again every half hour till morning; the next night takes what is left.
//! The night's report («За ночь» in the panel) is its parts' reports, and every part can undo its night («Откатить
//! ночь»). «Запустить сон сейчас» starts a night at once, outside the hour too — only when a part can do something
//! now; otherwise it says why and records nothing. A night that left nothing behind is tidied away once a later night
//! has done the work: the morning still sees a night that waited for its model, until one does better.
//!
//! The reflection (src/memory/reflect.rs) is the first and so far the only part. Later ones — vectors recounted after
//! a model change, old things cleaned up, the station's memory copied to the computer — come each with its own switch.

pub mod controller;

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use futures::future::BoxFuture;
use futures::FutureExt;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub use controller::routes;

use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::core::identity::principal::Principal;
use crate::platform::http::ApiError;
use crate::settings::definitions::SLEEP_TIME;
use crate::settings::SettingsService;
use crate::skills::own::alarms_skill::DEFAULT_TIMEZONE;
use crate::storage::Db;
use crate::utils::ids::uuid;
use crate::utils::time::{iso, now_iso};

/// The sleep's hour when none is set.
pub const DEFAULT_TIME: &str = "04:00";
/// How long a night lasts after its hour: what is not done by then waits for the next night.
const NIGHT_HOURS: i64 = 3;
/// How often a part that could not finish is tried again in the night.
const RETRY_MINUTES: i64 = 30;
/// How often the clock is looked at.
const TICK: StdDuration = StdDuration::from_secs(60);
/// What one part may spend on one go.
const PART_BUDGET: StdDuration = StdDuration::from_secs(20 * 60);
/// How long the nights' reports are kept.
const KEEP_DAYS: i64 = 90;

// ───────── Parts ─────────

/// How a part's go ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PartState {
    /// It did its work.
    Done,
    /// It had nothing to do: off, or no work tonight.
    Nothing,
    /// It could not start (no model to think with): tried again till morning.
    Waiting,
    /// It broke off (the model did not answer, its answer made no sense): tried again till morning.
    Failed,
}

impl PartState {
    /// Whether the night tries it again.
    pub fn again(self) -> bool {
        matches!(self, PartState::Waiting | PartState::Failed)
    }
}

/// A part's say in the night's report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartReport {
    pub part: String,
    pub title: String,
    pub state: PartState,
    /// For people, one line: what it did, or why not.
    pub summary: String,
    /// What it did, line by line.
    #[serde(default)]
    pub lines: Vec<String>,
    pub at: String,
}

impl PartReport {
    pub fn new(part: &dyn SleepPart, state: PartState, summary: impl Into<String>) -> PartReport {
        PartReport {
            part: part.id().to_string(),
            title: part.title().to_string(),
            state,
            summary: summary.into(),
            lines: Vec::new(),
            at: now_iso(),
        }
    }
}

/// What a part gets for its go.
pub struct Night {
    /// What it changes carries it: the report and the rollback find the night's changes by it.
    pub run_id: String,
    /// The station's zone: what «a day» is.
    pub zone: Tz,
    /// Cancelled when the part's time is up or the gateway stops.
    pub cancel: CancellationToken,
}

/// One part of the sleep.
pub trait SleepPart: Send + Sync {
    /// Its id in the reports: `reflection`.
    fn id(&self) -> &'static str;
    /// For people: «Рефлексия».
    fn title(&self) -> String;
    /// A part that is off never runs.
    fn enabled(&self) -> bool;
    /// Whether it has work tonight: Ok, or why not («за день не было разговоров»).
    fn work(&self) -> Result<(), String>;
    /// Whether it can start now, its model there: Ok, or why not. The night's own hour starts it anyway, so the
    /// morning sees what it waited for; «Запустить сон сейчас» does not.
    fn ready(&self) -> Result<(), String> {
        Ok(())
    }
    /// One go of the night.
    fn run<'a>(&'a self, night: &'a Night) -> BoxFuture<'a, PartReport>;
    /// What it left from a night: changes made (undone too) and things thought over. None from every part — an
    /// empty night.
    fn traces(&self, run_id: &str) -> usize;
    /// What it did in a night, for «За ночь»: its own JSON.
    fn details(&self, run_id: &str) -> serde_json::Value;
    /// Undoes what it did in a night; how many changes went back.
    fn roll_back(&self, run_id: &str, actor: &Principal) -> Result<usize, String>;
}

// ───────── Nights ─────────

/// A night as the panel shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    /// The day the night started on the station's clock.
    pub night: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// schedule | manual
    pub trigger: String,
    /// running | done | partial | nothing
    pub state: String,
    pub parts: Vec<PartReport>,
    pub attempts: i64,
    pub rolled_back_at: Option<String>,
}

/// A night with each part's own account of it, by the part's id.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    #[serde(flatten)]
    pub run: Run,
    pub details: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartView {
    pub id: String,
    pub title: String,
    pub enabled: bool,
    /// Why it has no work tonight; None — it has.
    pub idle: Option<String>,
}

/// The sleep as the panel's settings show it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepView {
    /// "HH:MM" on the station's clock.
    pub time: String,
    pub zone: String,
    /// When the next night starts, in the zone (RFC 3339).
    pub next: Option<String>,
    /// The night going on now.
    pub running: Option<String>,
    /// Why «Запустить сон сейчас» has nothing to do now; None — it has.
    pub why_not: Option<String>,
    pub parts: Vec<PartView>,
    pub last: Option<Run>,
}

pub struct SleepService {
    db: Arc<Db>,
    settings: Arc<SettingsService>,
    events: EventBus,
    parts: RwLock<Vec<Arc<dyn SleepPart>>>,
    /// The live station's zone (src/gateway/uniproxy), set once the stations' sessions are built.
    zone: OnceLock<Arc<dyn Fn() -> Option<String> + Send + Sync>>,
    /// The zone a station said last: the nights keep to it while none is on line.
    said: Mutex<Option<String>>,
    /// The night going on now: one at a time.
    running: Mutex<Option<String>>,
    stop: CancellationToken,
}

impl SleepService {
    pub fn new(db: Arc<Db>, settings: Arc<SettingsService>, events: EventBus) -> SleepService {
        SleepService {
            db,
            settings,
            events,
            parts: RwLock::new(Vec::new()),
            zone: OnceLock::new(),
            said: Mutex::new(None),
            running: Mutex::new(None),
            stop: CancellationToken::new(),
        }
    }

    /// A part joins the night, after those before it.
    pub fn register(&self, part: Arc<dyn SleepPart>) {
        self.parts.write().unwrap_or_else(|poisoned| poisoned.into_inner()).push(part);
    }

    /// Where the station's clock comes from: its live session's zone.
    pub fn set_zone(&self, zone: Arc<dyn Fn() -> Option<String> + Send + Sync>) {
        let _ = self.zone.set(zone);
    }

    fn parts(&self) -> Vec<Arc<dyn SleepPart>> {
        self.parts.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// The station's zone: as it says it now, else as it said it last, else the gateway's default.
    pub fn zone(&self) -> Tz {
        let now = self.zone.get().and_then(|zone| zone()).filter(|zone| zone.parse::<Tz>().is_ok());
        let mut said = locked(&self.said);
        if now.is_some() {
            *said = now;
        }
        said.as_deref()
            .and_then(|zone| zone.parse().ok())
            .unwrap_or_else(|| DEFAULT_TIMEZONE.parse().unwrap_or(Tz::UTC))
    }

    /// The sleep's hour on the station's clock.
    pub fn time(&self) -> NaiveTime {
        self.settings
            .get(&SLEEP_TIME, None)
            .and_then(|text| NaiveTime::parse_from_str(&text, "%H:%M").ok())
            .unwrap_or_else(|| NaiveTime::from_hms_opt(4, 0, 0).expect("4:00 is a time"))
    }

    pub fn set_time(&self, text: &str, actor: Option<Actor>) -> Result<(), ApiError> {
        let text = text.trim();
        if (SLEEP_TIME.parse)(&serde_json::Value::String(text.to_string())).is_none() {
            return Err(ApiError::bad_request(crate::tr!("Время — ЧЧ:ММ, например 04:00.")));
        }
        self.settings.set(&SLEEP_TIME, &text.to_string(), actor)
    }

    pub fn running(&self) -> Option<String> {
        locked(&self.running).clone()
    }

    pub fn view(&self) -> SleepView {
        let (zone, at, now) = (self.zone(), self.time(), Utc::now());
        let parts = self
            .parts()
            .iter()
            .map(|part| PartView {
                id: part.id().to_string(),
                title: part.title().to_string(),
                enabled: part.enabled(),
                idle: part.work().err(),
            })
            .collect();
        SleepView {
            time: at.format("%H:%M").to_string(),
            zone: zone.name().to_string(),
            next: next_start(now, zone, at).map(|start| start.with_timezone(&zone).to_rfc3339()),
            running: self.running(),
            why_not: self.startable().err(),
            parts,
            last: self.db.with(|c| list_runs(c, 1)).ok().and_then(|runs| runs.into_iter().next()),
        }
    }

    /// The nights, newest first.
    pub fn runs(&self, limit: i64) -> Result<Vec<Run>, ApiError> {
        Ok(self.db.with(|c| list_runs(c, limit))?)
    }

    pub fn run(&self, id: &str) -> Result<RunDetail, ApiError> {
        let run =
            self.db.with(|c| find_run(c, id))?.ok_or_else(|| ApiError::not_found(crate::tr!("Такой ночи нет.")))?;
        let details = self.parts().iter().map(|part| (part.id().to_string(), part.details(id))).collect();
        Ok(RunDetail { run, details })
    }

    // ───────── The clock ─────────

    /// Looks at the clock every minute until `stop`; a night left «running» by a gateway that went down is
    /// counted as not finished, so the night tries it again.
    pub fn start(self: &Arc<Self>) {
        let _ = self.db.with(|c| {
            c.execute(
                "UPDATE sleep_runs SET state = 'partial', finished_at = COALESCE(finished_at, ?1) WHERE state = 'running'",
                [now_iso()],
            )
        });
        self.tidy();
        let me = Arc::downgrade(self);
        let stop = self.stop.clone();
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(TICK);
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = ticks.tick() => {}
                }
                let Some(sleep) = me.upgrade() else { break };
                sleep.tick(Utc::now()).await;
            }
        });
    }

    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// In the night's hours: starts the night when a part has work, or tries again what did not finish.
    pub async fn tick(self: &Arc<Self>, now: DateTime<Utc>) {
        if self.running().is_some() {
            return;
        }
        let zone = self.zone();
        let Some(day) = night_at(now, zone, self.time()) else { return };
        let night = day.format("%Y-%m-%d").to_string();
        match self.db.with(|c| scheduled_run(c, &night)).ok().flatten() {
            None => {
                if !self.parts().iter().any(|part| part.enabled() && part.work().is_ok()) {
                    return;
                }
                let Ok(id) = self.begin("schedule", &night, None) else { return };
                self.go(&id, zone, Vec::new(), None).await;
            }
            Some(run) if run.state == "partial" && run.rolled_back_at.is_none() => {
                let last = run.finished_at.as_deref().and_then(parse_time).unwrap_or(now);
                if now < last + Duration::minutes(RETRY_MINUTES) {
                    return;
                }
                // What did not finish, and a part the broken night never got to.
                let again: Vec<String> = self
                    .parts()
                    .iter()
                    .map(|part| part.id().to_string())
                    .filter(|id| {
                        run.parts.iter().find(|report| &report.part == id).is_none_or(|report| report.state.again())
                    })
                    .collect();
                if again.is_empty() || !self.claim(&run.id) {
                    return;
                }
                self.go(&run.id, zone, run.parts.clone(), Some(again)).await;
            }
            Some(_) => {}
        }
    }

    /// «Запустить сон сейчас»: a night at once, in the background; its id. Nothing to do now — it says why and records
    /// no night.
    pub fn run_now(self: &Arc<Self>, actor: &Principal) -> Result<String, ApiError> {
        self.startable().map_err(ApiError::conflict)?;
        let zone = self.zone();
        let night = Utc::now().with_timezone(&zone).date_naive().format("%Y-%m-%d").to_string();
        let id = self.begin("manual", &night, Some(Actor::from(actor)))?;
        let (me, run) = (self.clone(), id.clone());
        tokio::spawn(async move { me.go(&run, zone, Vec::new(), None).await });
        Ok(id)
    }

    /// «Откатить ночь»: every part undoes what it did that night.
    pub fn roll_back(&self, id: &str, actor: &Principal) -> Result<Run, ApiError> {
        let run =
            self.db.with(|c| find_run(c, id))?.ok_or_else(|| ApiError::not_found(crate::tr!("Такой ночи нет.")))?;
        if self.running().as_deref() == Some(id) {
            return Err(ApiError::conflict(crate::tr!("Ночь ещё идёт: откатить её можно, когда она закончится.")));
        }
        if run.rolled_back_at.is_some() {
            return Err(ApiError::conflict(crate::tr!("Эта ночь уже откачена.")));
        }
        for part in self.parts() {
            part.roll_back(id, actor).map_err(ApiError::internal)?;
        }
        self.db
            .with(|c| c.execute("UPDATE sleep_runs SET rolled_back_at = ?2 WHERE id = ?1", params![id, now_iso()]))?;
        self.events.emit(GatewayEvent::SleepChanged {
            run_id: id.to_string(),
            state: "rolledBack".into(),
            actor: Some(Actor::from(actor)),
        });
        Ok(self.db.with(|c| find_run(c, id))?.unwrap_or(run))
    }

    // ───────── A night ─────────

    /// Whether a night started now would do something: a part that is on, has work and can start. Why not — the
    /// first part's reason.
    fn startable(&self) -> Result<(), String> {
        let mut why = None;
        for part in self.parts() {
            let can = if part.enabled() {
                part.work().and_then(|()| part.ready())
            } else {
                Err(crate::tr!("{part} выключена.", part = part.title()))
            };
            match can {
                Ok(()) => return Ok(()),
                Err(reason) => {
                    why.get_or_insert(reason);
                }
            }
        }
        Err(why.unwrap_or_else(|| crate::tr!("У сна нет частей.")))
    }

    /// Takes away the nights that left nothing behind and that a later night has done the work of: a press of the
    /// button with no model, a night that waited till morning before the model came.
    fn tidy(&self) {
        let Ok(runs) = self.db.with(|c| list_runs(c, 200)) else { return };
        let parts = self.parts();
        let mut done_later = false;
        let mut gone = Vec::new();
        for run in runs {
            if run.state == "running" {
                continue;
            }
            let traces: usize = parts.iter().map(|part| part.traces(&run.id)).sum();
            if traces > 0 {
                done_later = true;
            } else if done_later {
                gone.push(run.id);
            }
        }
        if gone.is_empty() {
            return;
        }
        let removed = self.db.with(|c| {
            let mut statement = c.prepare("DELETE FROM sleep_runs WHERE id = ?1")?;
            for id in &gone {
                statement.execute([id])?;
            }
            Ok(gone.len())
        });
        match removed {
            Ok(count) => tracing::info!(target: "sleep", "{count} empty night(s) tidied away"),
            Err(error) => tracing::warn!(target: "sleep", "empty nights not tidied: {error}"),
        }
    }

    /// Takes the night's turn; false when another night is going on.
    fn claim(&self, id: &str) -> bool {
        let mut running = locked(&self.running);
        if running.is_some() {
            return false;
        }
        *running = Some(id.to_string());
        true
    }

    fn begin(&self, trigger: &str, night: &str, actor: Option<Actor>) -> Result<String, ApiError> {
        let id = uuid();
        if !self.claim(&id) {
            return Err(ApiError::conflict(crate::tr!("Сон уже идёт.")));
        }
        let written = self.db.with(|c| {
            c.execute(
                "INSERT INTO sleep_runs (id, night, started_at, trigger, state) VALUES (?1, ?2, ?3, ?4, 'running')",
                params![id, night, now_iso(), trigger],
            )
        });
        if let Err(error) = written {
            *locked(&self.running) = None;
            return Err(error.into());
        }
        self.events.emit(GatewayEvent::SleepChanged { run_id: id.clone(), state: "running".into(), actor });
        Ok(id)
    }

    /// The parts one by one (`only`: those to try again), then the night's report.
    async fn go(&self, id: &str, zone: Tz, mut reports: Vec<PartReport>, only: Option<Vec<String>>) {
        let again = only.is_some();
        for part in self.parts() {
            if only.as_ref().is_some_and(|only| !only.iter().any(|wanted| wanted == part.id())) {
                continue;
            }
            let report = self.one(&*part, id, zone).await;
            match reports.iter_mut().find(|known| known.part == report.part) {
                Some(slot) => *slot = report,
                None => reports.push(report),
            }
        }
        let state = state_of(&reports);
        let report = serde_json::to_string(&reports).unwrap_or_else(|_| "[]".into());
        let written = self.db.with(|c| {
            c.execute(
                "UPDATE sleep_runs SET state = ?2, report = ?3, finished_at = ?4, attempts = attempts + ?5 WHERE id = ?1",
                params![id, state, report, now_iso(), i64::from(again)],
            )?;
            c.execute("DELETE FROM sleep_runs WHERE started_at < ?1", [iso(Utc::now() - Duration::days(KEEP_DAYS))])
        });
        if let Err(error) = written {
            tracing::warn!(target: "sleep", "the night's report is not kept: {error}");
        }
        tracing::info!(target: "sleep", "night {id}: {state}");
        *locked(&self.running) = None;
        self.tidy();
        self.events.emit(GatewayEvent::SleepChanged { run_id: id.to_string(), state: state.to_string(), actor: None });
    }

    /// One part's go, within its time; a part that panics is a failed one.
    async fn one(&self, part: &dyn SleepPart, id: &str, zone: Tz) -> PartReport {
        if !part.enabled() {
            return PartReport::new(part, PartState::Nothing, crate::tr!("Выключена."));
        }
        if let Err(why) = part.work() {
            return PartReport::new(part, PartState::Nothing, why);
        }
        let night = Night { run_id: id.to_string(), zone, cancel: self.stop.child_token() };
        let went = tokio::time::timeout(PART_BUDGET, AssertUnwindSafe(part.run(&night)).catch_unwind()).await;
        match went {
            Ok(Ok(report)) => report,
            Ok(Err(_)) => PartReport::new(part, PartState::Failed, crate::tr!("Сломалась на ходу; попробую ещё раз.")),
            Err(_) => {
                night.cancel.cancel();
                PartReport::new(part, PartState::Failed, crate::tr!("Не успела за 20 минут; попробую ещё раз."))
            }
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// running | done | partial | nothing: partial while a part is to be tried again.
fn state_of(reports: &[PartReport]) -> &'static str {
    if reports.iter().any(|report| report.state.again()) {
        "partial"
    } else if reports.iter().any(|report| report.state == PartState::Done) {
        "done"
    } else {
        "nothing"
    }
}

fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text).ok().map(|at| at.with_timezone(&Utc))
}

/// A wall-clock time where the station is, as a moment: a time the clock skips (spring) moves on to the first that
/// exists, a time it repeats (autumn) is the first of the two.
fn moment(zone: Tz, wall: NaiveDateTime) -> Option<DateTime<Utc>> {
    (0..=180).find_map(|minutes| {
        zone.from_local_datetime(&(wall + Duration::minutes(minutes))).earliest().map(|at| at.with_timezone(&Utc))
    })
}

/// The night `now` falls in, by the day it started on the station's clock; None outside the night's hours.
fn night_at(now: DateTime<Utc>, zone: Tz, at: NaiveTime) -> Option<NaiveDate> {
    let today = now.with_timezone(&zone).date_naive();
    [today, today - Duration::days(1)].into_iter().find(|day| {
        moment(zone, day.and_time(at)).is_some_and(|start| start <= now && now < start + Duration::hours(NIGHT_HOURS))
    })
}

/// When the next night starts after `now`.
fn next_start(now: DateTime<Utc>, zone: Tz, at: NaiveTime) -> Option<DateTime<Utc>> {
    let today = now.with_timezone(&zone).date_naive();
    (0..=2).find_map(|shift| moment(zone, (today + Duration::days(shift)).and_time(at)).filter(|start| *start > now))
}

// ───────── The table ─────────

const RUN_COLUMNS: &str = "id, night, started_at, finished_at, trigger, state, report, attempts, rolled_back_at";

fn run_from(row: &Row) -> rusqlite::Result<Run> {
    let report: String = row.get(6)?;
    Ok(Run {
        id: row.get(0)?,
        night: row.get(1)?,
        started_at: row.get(2)?,
        finished_at: row.get(3)?,
        trigger: row.get(4)?,
        state: row.get(5)?,
        parts: serde_json::from_str(&report).unwrap_or_default(),
        attempts: row.get(7)?,
        rolled_back_at: row.get(8)?,
    })
}

fn find_run(conn: &Connection, id: &str) -> rusqlite::Result<Option<Run>> {
    conn.query_row(&format!("SELECT {RUN_COLUMNS} FROM sleep_runs WHERE id = ?1"), [id], run_from).optional()
}

/// The night's own run, the one its hour started.
fn scheduled_run(conn: &Connection, night: &str) -> rusqlite::Result<Option<Run>> {
    conn.query_row(
        &format!(
            "SELECT {RUN_COLUMNS} FROM sleep_runs WHERE night = ?1 AND trigger = 'schedule' ORDER BY started_at DESC LIMIT 1"
        ),
        [night],
        run_from,
    )
    .optional()
}

fn list_runs(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<Run>> {
    let mut statement =
        conn.prepare(&format!("SELECT {RUN_COLUMNS} FROM sleep_runs ORDER BY started_at DESC, rowid DESC LIMIT ?1"))?;
    let rows = statement.query_map([limit.clamp(1, 200)], run_from)?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::core::identity::principal::user_principal;
    use crate::core::identity::roles::Role;

    fn moscow() -> Tz {
        "Europe/Moscow".parse().unwrap()
    }

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    fn four() -> NaiveTime {
        NaiveTime::from_hms_opt(4, 0, 0).unwrap()
    }

    #[test]
    fn a_night_lasts_from_its_hour_till_morning_on_the_stations_clock() {
        // Moscow is UTC+3: 04:00 there is 01:00 UTC.
        assert_eq!(night_at(utc("2026-11-13T00:59:00Z"), moscow(), four()), None);
        let day = NaiveDate::from_ymd_opt(2026, 11, 13).unwrap();
        assert_eq!(night_at(utc("2026-11-13T01:00:00Z"), moscow(), four()), Some(day));
        assert_eq!(night_at(utc("2026-11-13T03:59:00Z"), moscow(), four()), Some(day));
        assert_eq!(night_at(utc("2026-11-13T04:00:00Z"), moscow(), four()), None);
        // An hour before midnight: the night goes on into the next day and keeps its first day.
        let late = NaiveTime::from_hms_opt(23, 30, 0).unwrap();
        assert_eq!(
            night_at(utc("2026-11-13T21:10:00Z"), moscow(), late),
            Some(NaiveDate::from_ymd_opt(2026, 11, 13).unwrap())
        );
        assert_eq!(next_start(utc("2026-11-13T02:00:00Z"), moscow(), four()), Some(utc("2026-11-14T01:00:00Z")));
        assert_eq!(next_start(utc("2026-11-12T20:00:00Z"), moscow(), four()), Some(utc("2026-11-13T01:00:00Z")));
    }

    /// A part whose goes end as told, one by one (the last repeats); a go that ends done leaves a trace.
    struct Fake {
        on: bool,
        work: bool,
        ready: Mutex<Result<(), String>>,
        ends: Vec<PartState>,
        runs: AtomicUsize,
        rolled: AtomicUsize,
        left: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(on: bool, work: bool, ends: Vec<PartState>) -> Arc<Fake> {
            Arc::new(Fake {
                on,
                work,
                ready: Mutex::new(Ok(())),
                ends,
                runs: AtomicUsize::new(0),
                rolled: AtomicUsize::new(0),
                left: Mutex::new(Vec::new()),
            })
        }
    }

    impl SleepPart for Fake {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn title(&self) -> String {
            "Проба".into()
        }
        fn enabled(&self) -> bool {
            self.on
        }
        fn work(&self) -> Result<(), String> {
            if self.work {
                Ok(())
            } else {
                Err("Нечего делать.".into())
            }
        }
        fn ready(&self) -> Result<(), String> {
            locked(&self.ready).clone()
        }
        fn run<'a>(&'a self, night: &'a Night) -> BoxFuture<'a, PartReport> {
            let go = self.runs.fetch_add(1, Ordering::SeqCst);
            let state = self.ends[go.min(self.ends.len() - 1)];
            if state == PartState::Done {
                locked(&self.left).push(night.run_id.clone());
            }
            Box::pin(async move { PartReport::new(self, state, format!("go {go}")) })
        }
        fn traces(&self, run_id: &str) -> usize {
            locked(&self.left).iter().filter(|id| *id == run_id).count()
        }
        fn details(&self, run_id: &str) -> serde_json::Value {
            serde_json::json!({ "run": run_id })
        }
        fn roll_back(&self, _run_id: &str, _actor: &Principal) -> Result<usize, String> {
            self.rolled.fetch_add(1, Ordering::SeqCst);
            Ok(2)
        }
    }

    fn sleep(part: Arc<Fake>) -> Arc<SleepService> {
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        let sleep = Arc::new(SleepService::new(db, settings, events));
        sleep.set_zone(Arc::new(|| Some("Europe/Moscow".to_string())));
        sleep.register(part);
        sleep
    }

    fn admin() -> Principal {
        user_principal("u1", "Админ", Role::Admin, None)
    }

    async fn settled(sleep: &SleepService) {
        for _ in 0..50 {
            if sleep.running().is_none() {
                return;
            }
            tokio::time::sleep(StdDuration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn the_night_runs_once_at_its_hour_and_only_with_work() {
        let idle = Fake::new(true, false, vec![PartState::Done]);
        let quiet = sleep(idle.clone());
        quiet.tick(utc("2026-11-13T01:05:00Z")).await;
        assert_eq!(idle.runs.load(Ordering::SeqCst), 0);
        assert!(quiet.runs(10).unwrap().is_empty(), "no night without work");

        let part = Fake::new(true, true, vec![PartState::Done]);
        let sleep = sleep(part.clone());
        sleep.tick(utc("2026-11-12T22:00:00Z")).await;
        assert_eq!(part.runs.load(Ordering::SeqCst), 0, "not before the hour");
        sleep.tick(utc("2026-11-13T01:05:00Z")).await;
        sleep.tick(utc("2026-11-13T01:06:00Z")).await;
        assert_eq!(part.runs.load(Ordering::SeqCst), 1);
        let runs = sleep.runs(10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(
            (runs[0].night.as_str(), runs[0].trigger.as_str(), runs[0].state.as_str()),
            ("2026-11-13", "schedule", "done")
        );
        assert_eq!(runs[0].parts[0].summary, "go 0");
        assert_eq!(sleep.running(), None);
    }

    #[tokio::test]
    async fn what_did_not_finish_is_tried_again_every_half_hour_till_morning() {
        let part = Fake::new(true, true, vec![PartState::Waiting, PartState::Failed, PartState::Done]);
        // No model: the night's own hour still starts it, so the morning sees what it waited for.
        *locked(&part.ready) = Err("Ждёт модель.".into());
        let sleep = sleep(part.clone());
        sleep.tick(utc("2026-11-13T01:00:00Z")).await;
        assert_eq!(sleep.runs(1).unwrap()[0].state, "partial");
        // The finish time is now, not 01:00: a retry waits half an hour from now; set it back to 01:00.
        let id = sleep.runs(1).unwrap()[0].id.clone();
        sleep
            .db
            .with(|c| c.execute("UPDATE sleep_runs SET finished_at = '2026-11-13T01:00:00.000Z' WHERE id = ?1", [&id]))
            .unwrap();
        sleep.tick(utc("2026-11-13T01:20:00Z")).await;
        assert_eq!(part.runs.load(Ordering::SeqCst), 1, "not yet");
        sleep.tick(utc("2026-11-13T01:31:00Z")).await;
        assert_eq!(part.runs.load(Ordering::SeqCst), 2);
        let run = &sleep.runs(1).unwrap()[0];
        assert_eq!((run.state.as_str(), run.attempts, run.parts.len()), ("partial", 2, 1));
        sleep
            .db
            .with(|c| c.execute("UPDATE sleep_runs SET finished_at = '2026-11-13T01:31:00.000Z' WHERE id = ?1", [&id]))
            .unwrap();
        // Morning: the next night takes it.
        sleep.tick(utc("2026-11-13T04:05:00Z")).await;
        assert_eq!(part.runs.load(Ordering::SeqCst), 2);
        sleep.tick(utc("2026-11-13T02:05:00Z")).await;
        assert_eq!(part.runs.load(Ordering::SeqCst), 3);
        assert_eq!(sleep.runs(1).unwrap()[0].state, "done");
    }

    #[tokio::test]
    async fn the_button_says_why_it_cannot_and_records_no_night() {
        let off = sleep(Fake::new(false, true, vec![PartState::Done]));
        assert!(off.run_now(&admin()).is_err());
        assert_eq!(off.view().why_not.as_deref(), Some("Проба выключена."));
        let idle = sleep(Fake::new(true, false, vec![PartState::Done]));
        assert_eq!(idle.view().why_not.as_deref(), Some("Нечего делать."));
        let part = Fake::new(true, true, vec![PartState::Done]);
        *locked(&part.ready) = Err("Ждёт модель.".into());
        let waiting = sleep(part.clone());
        let refused = waiting.run_now(&admin()).unwrap_err();
        assert_eq!(refused.message, "Ждёт модель.");
        assert!(waiting.runs(10).unwrap().is_empty(), "nothing recorded");
        assert_eq!(part.runs.load(Ordering::SeqCst), 0);

        *locked(&part.ready) = Ok(());
        assert_eq!(waiting.view().why_not, None);
        let id = waiting.run_now(&admin()).unwrap();
        settled(&waiting).await;
        let detail = waiting.run(&id).unwrap();
        assert_eq!((detail.run.trigger.as_str(), detail.run.state.as_str()), ("manual", "done"));
        assert_eq!(detail.details["fake"]["run"], id.as_str());

        let rolled = waiting.roll_back(&id, &admin()).unwrap();
        assert!(rolled.rolled_back_at.is_some());
        assert_eq!(part.rolled.load(Ordering::SeqCst), 1);
        assert!(waiting.roll_back(&id, &admin()).is_err(), "once is enough");
        assert!(waiting.set_time("4:00", None).is_err());
        waiting.set_time("23:30", None).unwrap();
        assert_eq!(waiting.view().time, "23:30");
        assert_eq!(waiting.view().zone, "Europe/Moscow");
    }

    #[tokio::test]
    async fn an_empty_night_goes_once_a_later_one_has_done_the_work() {
        let part = Fake::new(true, true, vec![PartState::Waiting, PartState::Done]);
        *locked(&part.ready) = Err("Ждёт модель.".into());
        let sleep = sleep(part.clone());
        // The 4:00 night waits for its model: it stays while nothing did better.
        sleep.tick(utc("2026-11-13T01:00:00Z")).await;
        assert_eq!(sleep.runs(10).unwrap().len(), 1);
        sleep.tidy();
        assert_eq!(sleep.runs(10).unwrap().len(), 1, "the morning sees what it waited for");
        // The model comes; a press of the button does the work, and the empty night goes.
        *locked(&part.ready) = Ok(());
        let id = sleep.run_now(&admin()).unwrap();
        settled(&sleep).await;
        let runs = sleep.runs(10).unwrap();
        assert_eq!(runs.iter().map(|run| run.id.as_str()).collect::<Vec<_>>(), vec![id.as_str()]);
    }
}
