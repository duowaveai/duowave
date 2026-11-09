//! Every request as it goes (core/requests.rs): sessions start them and say each step; the service keeps the live
//! ones, publishes every change on the bus once per tick (`request.updated`, whole), and writes a finished one to the
//! journal for as many days as the household keeps it. A request nobody ended in three minutes is closed as failed.
//! The live channel (src/live) and scenarios to come listen on the bus, not here.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::routing::{get, put};
use axum::Router;
use rusqlite::{params, Row};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::app::App;
use crate::config::UniproxyMode;
use crate::core::events::{Actor, EventBus, GatewayEvent};
use crate::core::identity::permissions::{JOURNAL_VIEW, SETTINGS_EDIT};
use crate::core::requests::{
    monotonic_clock, note_step, Place, RequestHandle, RequestHooks, RequestKind, RequestPhase, RequestStart,
    RequestStarter, RequestStatus, RequestStep, RequestView,
};
use crate::platform::http::{ok, ApiError, ApiResult, Body, Caller};
use crate::settings::definitions::{PanelMode, KEEP_REQUESTS, MODE};
use crate::settings::SettingsService;
use crate::storage::Db;
use crate::utils::ids::uuid;
use crate::utils::time::{iso, now_iso, now_ms};

/// Finished requests kept in memory for the panel's first look (the live channel's hello).
pub const RECENT: usize = 30;
/// A request with no end in this long ended unseen (a session that never closed it): it is closed as failed.
const STALE_MS: u64 = 180_000;
/// One that is speaking may take longer: a tale is read for minutes.
const STALE_SPEAKING_MS: u64 = 15 * 60_000;
/// The journal's rows at most, whatever the days say.
const MAX_ROWS: i64 = 20_000;
pub const DEFAULT_KEEP_DAYS: u32 = 30;
const DAY_MS: u64 = 86_400_000;
const SWEEP_EVERY: Duration = Duration::from_secs(30);
const PRUNE_EVERY: Duration = Duration::from_secs(6 * 3600);

/// The station's mode as a request starts in it, and as /health and the connectors read it: the ModeService's
/// (src/gateway/mode) once it is ported; until then `StoredMode`.
pub trait ModeSource: Send + Sync {
    /// The panel's mode now: relay, gateway, own.
    fn current(&self) -> PanelMode;
}

/// The mode as the gateway keeps it: the panel's choice (the `gateway_mode` setting), else UNIPROXY_MODE — what the
/// ModeService reads at its start.
pub struct StoredMode {
    settings: Arc<SettingsService>,
    configured: PanelMode,
}

impl StoredMode {
    pub fn new(settings: Arc<SettingsService>, configured: UniproxyMode) -> StoredMode {
        let configured = match configured {
            UniproxyMode::Gateway => PanelMode::Gateway,
            UniproxyMode::Relay => PanelMode::Relay,
        };
        StoredMode { settings, configured }
    }
}

impl ModeSource for StoredMode {
    fn current(&self) -> PanelMode {
        self.settings.get(&MODE, None).map(|stored| stored.mode).unwrap_or(self.configured)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HistoryQuery {
    /// As the API got it (a JavaScript Number); 50 when absent, from 1 to 200.
    pub limit: Option<f64>,
    /// Older than this start (ISO): the next page.
    pub before: Option<String>,
    pub device: Option<String>,
}

#[derive(Default)]
struct Pending {
    handles: Vec<Arc<RequestHandle>>,
    scheduled: bool,
}

/// The handles' way back to the service; weak, so a request in flight does not keep a stopped service alive.
struct Hooks(Weak<RequestsService>);

impl RequestHooks for Hooks {
    fn changed(&self, handle: &RequestHandle) {
        if let Some(service) = self.0.upgrade() {
            service.changed(handle);
        }
    }

    fn ended(&self, handle: &RequestHandle) {
        if let Some(service) = self.0.upgrade() {
            service.end(handle);
        }
    }
}

pub struct RequestsService {
    me: Weak<RequestsService>,
    db: Arc<Db>,
    events: EventBus,
    settings: Arc<SettingsService>,
    modes: Arc<dyn ModeSource>,
    hooks: Arc<dyn RequestHooks>,
    /// The requests going now, oldest first.
    active: Mutex<Vec<Arc<RequestHandle>>>,
    /// The last finished ones, newest first.
    recent: Mutex<VecDeque<RequestView>>,
    pending: Mutex<Pending>,
    /// One publisher at a time, so an older snapshot never goes out after a newer one.
    publishing: Mutex<()>,
    keep_days: AtomicU32,
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl RequestsService {
    /// The service with the days to keep, the journal pruned and the last ones read back (after a restart the
    /// dashboard still knows what was asked last).
    pub fn new(
        db: Arc<Db>,
        events: EventBus,
        settings: Arc<SettingsService>,
        modes: Arc<dyn ModeSource>,
    ) -> Arc<RequestsService> {
        let keep_days = settings.get(&KEEP_REQUESTS, None).unwrap_or(DEFAULT_KEEP_DAYS);
        let service = Arc::new_cyclic(|me: &Weak<RequestsService>| RequestsService {
            me: me.clone(),
            db,
            events,
            settings,
            modes,
            hooks: Arc::new(Hooks(me.clone())),
            active: Mutex::new(Vec::new()),
            recent: Mutex::new(VecDeque::new()),
            pending: Mutex::new(Pending::default()),
            publishing: Mutex::new(()),
            keep_days: AtomicU32::new(keep_days),
        });
        service.prune(now_ms());
        if keep_days > 0 {
            match service.history(&HistoryQuery { limit: Some(RECENT as f64), ..HistoryQuery::default() }) {
                Ok(last) => guard(&service.recent).extend(last),
                Err(error) => tracing::warn!("journal not read: {error}"),
            }
        }
        service
    }

    /// The sweep every 30 s, the prune every 6 hours, and the days to keep followed when someone else sets them
    /// (inside a tokio runtime).
    pub fn start(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        self.events.on("requests", move |event| {
            if let GatewayEvent::SettingsChanged { key, user_id: None, .. } = &*event {
                if key == KEEP_REQUESTS.key {
                    if let Some(service) = me.upgrade() {
                        service.reload_keep();
                    }
                }
            }
        });
        let me = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut sweep = tokio::time::interval(SWEEP_EVERY);
            let mut prune = tokio::time::interval(PRUNE_EVERY);
            sweep.tick().await;
            prune.tick().await;
            loop {
                tokio::select! {
                    _ = sweep.tick() => match me.upgrade() {
                        Some(service) => service.sweep(now_ms()),
                        None => break,
                    },
                    _ = prune.tick() => match me.upgrade() {
                        Some(service) => {
                            service.prune(now_ms());
                        }
                        None => break,
                    },
                }
            }
        });
    }

    /// Before the database closes: what is still going is journaled as cancelled, not lost.
    pub fn shutdown(&self) {
        let going: Vec<Arc<RequestHandle>> = guard(&self.active).clone();
        for handle in going {
            handle.finish(RequestStatus::Cancelled, Some("the gateway stopped"), None);
        }
    }

    /// A request starts in the given mode: the RequestStarter (`begin`) fills in the mode now.
    pub fn begin_in(&self, start: RequestStart, mode: &str) -> Arc<RequestHandle> {
        let some = |value: Option<String>| value.filter(|value| !value.is_empty());
        let view = RequestView {
            id: uuid(),
            device: start.device,
            session: some(start.session),
            kind: start.kind,
            mode: mode.to_string(),
            status: RequestStatus::Active,
            phase: None,
            started_at: now_iso(),
            ms: None,
            text: some(start.text),
            partial: None,
            answer: None,
            capability: None,
            executor: None,
            steps: Vec::new(),
            error: None,
            parent: some(start.parent),
        };
        let handle = Arc::new(RequestHandle::new(view, self.hooks.clone(), monotonic_clock()));
        guard(&self.active).push(handle.clone());
        self.touch(handle.clone());
        handle
    }

    /// A note on a request, going or finished: what the station said of what our player started for it
    /// (docs/plans/plugins.md, stage 5а). A finished one is written again and told anew; one no longer kept here is
    /// left alone.
    pub fn annotate(&self, id: &str, phase: RequestPhase, by: Option<Place>, what: &str) {
        let going = guard(&self.active).iter().find(|handle| handle.id() == id).cloned();
        if let Some(handle) = going {
            handle.note(phase, by, what);
            return;
        }
        let updated = {
            let mut recent = guard(&self.recent);
            recent.iter_mut().find(|view| view.id == id).map(|view| {
                let elapsed = chrono::DateTime::parse_from_rfc3339(&view.started_at)
                    .ok()
                    .map(|at| (chrono::Utc::now() - at.with_timezone(&chrono::Utc)).num_milliseconds().max(0) as u64)
                    .unwrap_or_else(|| view.ms.unwrap_or(0));
                note_step(view, phase, by, what, elapsed);
                view.clone()
            })
        };
        let Some(view) = updated else { return };
        if self.keep() > 0 {
            let steps = serde_json::to_string(&view.steps).unwrap_or_else(|_| "[]".into());
            let written = self.db.with(|c| {
                c.execute("UPDATE requests SET steps = ?2 WHERE id = ?1", params![view.id, steps]).map(|_| ())
            });
            if let Err(error) = written {
                tracing::warn!("journal: a note on {} not written: {error}", view.id);
            }
        }
        self.events.emit(GatewayEvent::RequestUpdated { request: view });
    }

    /// The requests going now, oldest first.
    pub fn active_views(&self) -> Vec<RequestView> {
        guard(&self.active).iter().map(|handle| handle.snapshot()).collect()
    }

    /// The last finished ones, newest first.
    pub fn recent_views(&self, limit: usize) -> Vec<RequestView> {
        guard(&self.recent).iter().take(limit).cloned().collect()
    }

    /// How many days the journal is kept: 0 — none.
    pub fn keep(&self) -> u32 {
        self.keep_days.load(Ordering::Relaxed)
    }

    /// The journal, newest first: from the database, or from memory while the household keeps none.
    pub fn history(&self, query: &HistoryQuery) -> Result<Vec<RequestView>, ApiError> {
        let limit = history_limit(query.limit);
        if self.keep() == 0 {
            return Ok(self
                .recent_views(RECENT)
                .into_iter()
                .filter(|view| query.device.as_ref().is_none_or(|device| &view.device == device))
                .filter(|view| query.before.as_ref().is_none_or(|before| &view.started_at < before))
                .take(limit)
                .collect());
        }
        let rows = self.db.with(|c| {
            let mut statement = c.prepare(
                "SELECT id, device, session, kind, mode, status, started_at, ms, text, answer, capability, executor,
                        steps, error, parent
                 FROM requests WHERE (?1 IS NULL OR started_at < ?1) AND (?2 IS NULL OR device = ?2)
                 ORDER BY started_at DESC LIMIT ?3",
            )?;
            let rows = statement.query_map(params![query.before, query.device, limit as i64], view_of)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })?;
        Ok(rows.into_iter().flatten().collect())
    }

    pub fn set_keep(&self, days: Option<&Value>, actor: Option<Actor>) -> Result<u32, ApiError> {
        let days = days
            .and_then(Value::as_f64)
            .filter(|days| days.fract() == 0.0 && (0.0..=3650.0).contains(days))
            .ok_or_else(|| ApiError::bad_request("days: целое число от 0 до 3650."))? as u32;
        self.settings.set(&KEEP_REQUESTS, &days, actor)?;
        self.keep_days.store(days, Ordering::Relaxed);
        self.prune(now_ms());
        Ok(days)
    }

    /// Requests nobody ended: closed as failed, so the panel does not show them going forever.
    pub fn sweep(&self, now: u64) {
        let going: Vec<Arc<RequestHandle>> = guard(&self.active).clone();
        for handle in going {
            let limit = if handle.phase() == Some(RequestPhase::Speak) { STALE_SPEAKING_MS } else { STALE_MS };
            let started = chrono::DateTime::parse_from_rfc3339(&handle.started_at())
                .map(|at| at.timestamp_millis().max(0) as u64)
                .unwrap_or(now);
            if now.saturating_sub(started) > limit {
                let minutes = (limit as f64 / 60_000.0).round();
                handle.finish(RequestStatus::Failed, Some(&format!("no end in {minutes} minutes")), None);
            }
        }
    }

    /// Old rows out: older than the days kept, past the cap; all of them when the household keeps none.
    pub fn prune(&self, now: u64) -> usize {
        let keep_days = self.keep();
        let pruned = self.db.with(|c| {
            if keep_days == 0 {
                return c.execute("DELETE FROM requests", []);
            }
            let cutoff = chrono::DateTime::from_timestamp_millis(now.saturating_sub(keep_days as u64 * DAY_MS) as i64)
                .map(iso)
                .unwrap_or_default();
            let old = c.execute("DELETE FROM requests WHERE started_at < ?1", params![cutoff])?;
            let past_cap = c.execute(
                "DELETE FROM requests WHERE id NOT IN (SELECT id FROM requests ORDER BY started_at DESC LIMIT ?1)",
                params![MAX_ROWS],
            )?;
            Ok(old + past_cap)
        });
        match pruned {
            Ok(removed) => {
                if removed > 0 && keep_days > 0 {
                    tracing::info!("journal: {removed} request(s) past {keep_days} days or {MAX_ROWS} rows removed");
                }
                removed
            }
            Err(error) => {
                tracing::warn!("journal not pruned: {error}");
                0
            }
        }
    }

    fn reload_keep(&self) {
        let days = self.settings.get(&KEEP_REQUESTS, None).unwrap_or(DEFAULT_KEEP_DAYS);
        self.keep_days.store(days, Ordering::Relaxed);
    }

    fn changed(&self, handle: &RequestHandle) {
        let found = guard(&self.active).iter().find(|known| std::ptr::eq(known.as_ref(), handle)).cloned();
        if let Some(handle) = found {
            self.touch(handle);
        }
    }

    /// Changes are published once per tick: a step and the words with it go as one update.
    fn touch(&self, handle: Arc<RequestHandle>) {
        let schedule = {
            let mut pending = guard(&self.pending);
            if !pending.handles.iter().any(|known| Arc::ptr_eq(known, &handle)) {
                pending.handles.push(handle);
            }
            !std::mem::replace(&mut pending.scheduled, true)
        };
        if !schedule {
            return;
        }
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                let me = self.me.clone();
                runtime.spawn(async move {
                    if let Some(service) = me.upgrade() {
                        service.flush();
                    }
                });
            }
            Err(_) => self.flush(),
        }
    }

    fn flush(&self) {
        let _publishing = guard(&self.publishing);
        let handles = {
            let mut pending = guard(&self.pending);
            pending.scheduled = false;
            std::mem::take(&mut pending.handles)
        };
        for handle in handles {
            self.events.emit(GatewayEvent::RequestUpdated { request: handle.snapshot() });
        }
    }

    fn end(&self, handle: &RequestHandle) {
        guard(&self.active).retain(|known| !std::ptr::eq(known.as_ref(), handle));
        let view = handle.snapshot();
        {
            let mut recent = guard(&self.recent);
            recent.push_front(view.clone());
            recent.truncate(RECENT);
        }
        tracing::debug!(
            "request {} {} {} in {} ms: {}",
            view.id.chars().take(8).collect::<String>(),
            view.kind.as_str(),
            view.status.as_str(),
            view.ms.map(|ms| ms.to_string()).unwrap_or_else(|| "?".into()),
            trail(&view)
        );
        if self.keep() > 0 {
            self.save(&view);
        }
    }

    fn save(&self, view: &RequestView) {
        let steps = serde_json::to_string(&view.steps).unwrap_or_else(|_| "[]".into());
        let saved = self.db.with(|c| {
            c.execute(
                "INSERT INTO requests (id, device, session, kind, mode, status, started_at, ms, text, answer, capability,
                                       executor, steps, error, parent)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15) ON CONFLICT (id) DO NOTHING",
                params![
                    view.id,
                    view.device,
                    view.session,
                    view.kind.as_str(),
                    view.mode,
                    view.status.as_str(),
                    view.started_at,
                    view.ms.map(|ms| ms as i64),
                    view.text,
                    view.answer,
                    view.capability,
                    view.executor,
                    steps,
                    view.error,
                    view.parent,
                ],
            )
        });
        if let Err(error) = saved {
            tracing::warn!("request {} not journaled: {error}", view.id.chars().take(8).collect::<String>());
        }
    }
}

impl RequestStarter for RequestsService {
    fn begin(&self, start: RequestStart) -> Arc<RequestHandle> {
        let mode = self.modes.current();
        self.begin_in(start, mode.as_str())
    }
}

/// «listen 900 · recognize 210 · think 430 cloud · speak 1200 own»: the trip in the log, without the words.
fn trail(view: &RequestView) -> String {
    if view.steps.is_empty() {
        return "no steps".into();
    }
    view.steps
        .iter()
        .map(|step| {
            let ms = step.ms.map(|ms| ms.to_string()).unwrap_or_else(|| "…".into());
            let by = step.by.map(|by| format!(" {}", place_name(by))).unwrap_or_default();
            format!("{} {ms}{by}", phase_name(step.phase))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn phase_name(phase: RequestPhase) -> &'static str {
    match phase {
        RequestPhase::Listen => "listen",
        RequestPhase::Recognize => "recognize",
        RequestPhase::Think => "think",
        RequestPhase::Tool => "tool",
        RequestPhase::Speak => "speak",
        RequestPhase::Play => "play",
    }
}

fn place_name(place: Place) -> &'static str {
    match place {
        Place::Own => "own",
        Place::Cloud => "cloud",
        Place::Yandex => "yandex",
        Place::Agent => "agent",
    }
}

/// A journal row as the API gives it; a row whose kind or status this version does not know is left out.
fn view_of(row: &Row<'_>) -> rusqlite::Result<Option<RequestView>> {
    let text = |index: usize| -> rusqlite::Result<Option<String>> {
        Ok(row.get::<_, Option<String>>(index)?.filter(|value| !value.is_empty()))
    };
    let kind: String = row.get(3)?;
    let status: String = row.get(5)?;
    let (Ok(kind), Ok(status)) = (
        serde_json::from_value::<RequestKind>(Value::String(kind)),
        serde_json::from_value::<RequestStatus>(Value::String(status)),
    ) else {
        return Ok(None);
    };
    let steps: String = row.get(12)?;
    Ok(Some(RequestView {
        id: row.get(0)?,
        device: row.get(1)?,
        session: text(2)?,
        kind,
        mode: row.get(4)?,
        status,
        phase: None,
        started_at: row.get(6)?,
        ms: row.get::<_, Option<i64>>(7)?.map(|ms| ms.max(0) as u64),
        text: text(8)?,
        partial: None,
        answer: text(9)?,
        capability: text(10)?,
        executor: text(11)?,
        steps: serde_json::from_str::<Vec<RequestStep>>(&steps).unwrap_or_default(),
        error: text(13)?,
        parent: text(14)?,
    }))
}

/// 50 when absent, not a number or 0; whole, from 1 to 200.
fn history_limit(limit: Option<f64>) -> usize {
    let limit = limit.unwrap_or(50.0).trunc();
    let limit = if limit.is_nan() || limit == 0.0 { 50.0 } else { limit };
    limit.clamp(1.0, 200.0) as usize
}

/// JavaScript's Number() of a query value, for what the panel sends: a decimal number, NaN otherwise.
fn js_number(text: &str) -> f64 {
    let text = text.trim();
    if text.is_empty() {
        return 0.0;
    }
    let decimal = text.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'));
    if decimal {
        text.parse().unwrap_or(f64::NAN)
    } else {
        f64::NAN
    }
}

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/v1/requests", get(list)).route("/api/v1/requests/keep", put(keep))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Journal {
    requests: Vec<RequestView>,
    keep_days: u32,
}

/// The journal of requests, newest first, and how many days it is kept.
async fn list(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Journal> {
    caller.permitted(JOURNAL_VIEW)?;
    let query = HistoryQuery {
        limit: query.get("limit").filter(|limit| !limit.is_empty()).map(|limit| js_number(limit)),
        before: query.get("before").filter(|before| !before.is_empty()).cloned(),
        device: query.get("device").filter(|device| !device.is_empty()).cloned(),
    };
    Ok(ok(Journal { requests: app.requests.history(&query)?, keep_days: app.requests.keep() }))
}

#[derive(Debug, Serialize)]
struct Keep {
    days: u32,
}

/// How many days the journal is kept: 0 — none, the older rows go at once.
async fn keep(State(app): State<Arc<App>>, caller: Caller, Body(body): Body<Map<String, Value>>) -> ApiResult<Keep> {
    caller.permitted(SETTINGS_EDIT)?;
    Ok(ok(Keep { days: app.requests.set_keep(body.get("days"), caller.actor())? }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::broadcast;

    /// The tasks a change woke run: the publisher, then whoever listens.
    async fn tick() {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    struct Built {
        service: Arc<RequestsService>,
        settings: Arc<SettingsService>,
        db: Arc<Db>,
        updates: broadcast::Receiver<Arc<GatewayEvent>>,
    }

    impl Built {
        fn updates(&mut self) -> Vec<RequestView> {
            let mut views = Vec::new();
            while let Ok(event) = self.updates.try_recv() {
                if let GatewayEvent::RequestUpdated { request } = &*event {
                    views.push(request.clone());
                }
            }
            views
        }
    }

    fn build_with(keep: Option<u32>) -> Built {
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let settings = Arc::new(SettingsService::new(db.clone(), events.clone()));
        if let Some(days) = keep {
            settings.set(&KEEP_REQUESTS, &days, None).unwrap();
        }
        let updates = events.subscribe();
        let modes = Arc::new(StoredMode::new(settings.clone(), UniproxyMode::Gateway));
        let service = RequestsService::new(db.clone(), events, settings.clone(), modes);
        Built { service, settings, db, updates }
    }

    fn start(device: &str, kind: RequestKind, text: Option<&str>) -> RequestStart {
        RequestStart { device: device.into(), session: None, kind, text: text.map(str::to_string), parent: None }
    }

    fn insert(db: &Db, id: &str, device: &str, started_at: &str) {
        db.with(|c| {
            c.execute(
                "INSERT INTO requests (id, device, kind, mode, status, started_at, steps)
                 VALUES (?1, ?2, 'voice', 'gateway', 'answered', ?3, '[]')",
                params![id, device, started_at],
            )
        })
        .unwrap();
    }

    fn ids(views: &[RequestView]) -> Vec<&str> {
        views.iter().map(|view| view.id.as_str()).collect()
    }

    #[tokio::test]
    async fn publishes_a_request_whole_once_per_tick_and_journals_it_when_it_ends() {
        let mut built = build_with(None);
        let trip = built.service.begin_in(
            RequestStart { session: Some("s1".into()), ..start("kitchen", RequestKind::Voice, None) },
            "gateway",
        );
        trip.step(RequestPhase::Listen, Some(Place::Own), Some("gigaam-v3"));
        tick().await;
        let updates = built.updates();
        assert_eq!(updates.len(), 1);
        assert_eq!(
            (updates[0].device.as_str(), updates[0].kind, updates[0].mode.as_str()),
            ("kitchen", RequestKind::Voice, "gateway")
        );
        assert_eq!((updates[0].status, updates[0].phase), (RequestStatus::Active, Some(RequestPhase::Listen)));
        assert_eq!(ids(&built.service.active_views()), [trip.id()]);

        trip.heard("Который час?");
        trip.step(RequestPhase::Think, Some(Place::Own), Some("Разговор, время, дата"));
        trip.answered("Без пяти три.");
        trip.finish(RequestStatus::Answered, None, None);
        tick().await;
        let updates = built.updates();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].status, RequestStatus::Answered);
        assert_eq!(updates[0].text.as_deref(), Some("Который час?"));
        assert_eq!(updates[0].answer.as_deref(), Some("Без пяти три."));
        assert!(built.service.active_views().is_empty());
        assert_eq!(built.service.recent_views(RECENT)[0].id, trip.id());

        let journaled = built.service.history(&HistoryQuery::default()).unwrap();
        assert_eq!(journaled[0].id, trip.id());
        assert_eq!(journaled[0].status, RequestStatus::Answered);
        assert_eq!(journaled[0].text.as_deref(), Some("Который час?"));
        assert_eq!(journaled[0].answer.as_deref(), Some("Без пяти три."));
        assert_eq!(journaled[0].session.as_deref(), Some("s1"));
        let phases: Vec<RequestPhase> = journaled[0].steps.iter().map(|step| step.phase).collect();
        assert_eq!(phases, [RequestPhase::Listen, RequestPhase::Think]);
    }

    #[tokio::test]
    async fn pages_the_journal_by_start_newest_first_and_by_station() {
        let built = build_with(None);
        insert(&built.db, "r0", "kitchen", "2026-11-01T10:00:00.000Z");
        insert(&built.db, "r1", "kids", "2026-11-02T10:00:00.000Z");
        insert(&built.db, "r2", "kitchen", "2026-11-03T10:00:00.000Z");
        let history = |query: HistoryQuery| built.service.history(&query).unwrap();
        assert_eq!(ids(&history(HistoryQuery { limit: Some(2.0), ..HistoryQuery::default() })), ["r2", "r1"]);
        assert_eq!(
            ids(&history(HistoryQuery { before: Some("2026-11-02T10:00:00.000Z".into()), ..HistoryQuery::default() })),
            ["r0"]
        );
        assert_eq!(ids(&history(HistoryQuery { device: Some("kids".into()), ..HistoryQuery::default() })), ["r1"]);
    }

    #[tokio::test]
    async fn keeps_as_many_days_as_the_household_says_older_rows_go_and_none_at_all_is_kept_with_0() {
        let mut built = build_with(Some(7));
        assert_eq!(built.service.keep(), 7);
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-05T12:00:00.000Z").unwrap().timestamp_millis() as u64;
        insert(&built.db, "old", "kitchen", "2026-10-20T12:00:00.000Z");
        insert(&built.db, "new", "kitchen", "2026-11-04T12:00:00.000Z");
        assert_eq!(built.service.prune(now), 1);
        assert_eq!(ids(&built.service.history(&HistoryQuery::default()).unwrap()), ["new"]);

        built.service.set_keep(Some(&json!(0)), None).unwrap();
        assert!(built.service.history(&HistoryQuery::default()).unwrap().is_empty());
        let trip = built.service.begin_in(start("kitchen", RequestKind::Text, Some("Привет")), "gateway");
        trip.finish(RequestStatus::Answered, None, None);
        tick().await;
        built.updates();
        // Nothing is written, but the dashboard still has what just happened.
        let rows: i64 = built.db.with(|c| c.query_row("SELECT count(*) FROM requests", [], |row| row.get(0))).unwrap();
        assert_eq!(rows, 0);
        assert_eq!(ids(&built.service.history(&HistoryQuery::default()).unwrap()), [trip.id()]);
        for wrong in [json!(-1), json!(3651), json!(1.5), json!("7")] {
            let error = built.service.set_keep(Some(&wrong), None).unwrap_err();
            assert_eq!(error.message, "days: целое число от 0 до 3650.");
        }
        assert_eq!(built.settings.get(&KEEP_REQUESTS, None), Some(0));
    }

    #[tokio::test]
    async fn closes_a_request_nobody_ended_as_failed_so_it_does_not_go_forever() {
        let built = build_with(None);
        let started = |handle: &RequestHandle| {
            chrono::DateTime::parse_from_rfc3339(&handle.started_at()).unwrap().timestamp_millis() as u64
        };
        let trip = built.service.begin_in(start("kitchen", RequestKind::Voice, None), "relay");
        built.service.sweep(started(&trip) + 60_000);
        assert!(trip.active());
        built.service.sweep(started(&trip) + 181_000);
        let view = trip.snapshot();
        assert_eq!((view.status, view.error.as_deref()), (RequestStatus::Failed, Some("no end in 3 minutes")));

        // A tale read aloud takes its time.
        let tale = built.service.begin_in(start("kitchen", RequestKind::Voice, None), "gateway");
        tale.step(RequestPhase::Speak, Some(Place::Own), Some("baya"));
        built.service.sweep(started(&tale) + 10 * 60_000);
        assert!(tale.active());
        built.service.sweep(started(&tale) + 16 * 60_000);
        let view = tale.snapshot();
        assert_eq!((view.status, view.error.as_deref()), (RequestStatus::Failed, Some("no end in 15 minutes")));
    }

    #[tokio::test]
    async fn starts_a_request_in_the_mode_the_panel_chose_and_journals_what_goes_at_shutdown() {
        let built = build_with(None);
        let first = built.service.begin(start("kitchen", RequestKind::Say, Some("Напоминаю")));
        assert_eq!(first.snapshot().mode, "gateway", "UNIPROXY_MODE while the panel chose none");
        built.settings.set(&MODE, &crate::settings::definitions::ModeSetting { mode: PanelMode::Own }, None).unwrap();
        let second = built.service.begin(start("kitchen", RequestKind::Text, None));
        assert_eq!(second.snapshot().mode, "own");
        built.service.shutdown();
        let journal = built.service.history(&HistoryQuery::default()).unwrap();
        assert_eq!(journal.len(), 2);
        assert!(journal.iter().all(|view| view.status == RequestStatus::Cancelled));
        assert_eq!(
            journal.iter().find(|view| view.id == first.id()).unwrap().error.as_deref(),
            Some("the gateway stopped")
        );
    }

    #[test]
    fn reads_the_limit_as_the_node_gateway_did() {
        assert_eq!(history_limit(None), 50);
        assert_eq!(history_limit(Some(js_number("abc"))), 50);
        assert_eq!(history_limit(Some(js_number("0"))), 50);
        assert_eq!(history_limit(Some(js_number("-3"))), 1);
        assert_eq!(history_limit(Some(js_number("2.7"))), 2);
        assert_eq!(history_limit(Some(js_number("1e3"))), 200);
        assert_eq!(history_limit(Some(js_number(" 20 "))), 20);
    }
}
