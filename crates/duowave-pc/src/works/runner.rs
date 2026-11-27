//! The run of a work (docs/plans/router-and-works.md, block A, 9): the copy → the folder's preparation → the agent's
//! run → the notes said on the way, each a new run in the same session → a commit if the agent left changes → the
//! branch into the folder → the folder's checks → the end. A short limit of the service is waited out and tried again
//! (1, 5, 15 minutes); a plan's limit is waited for until it resets — the person is asked «подождать или
//! остановить?», and silence waits. A key without money and an agent not signed in end it «не доделано» with honest
//! words. Each run is one deal of the router.

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use super::harness::{
    cut_to, environment, Chore, Ended, Harness, NotOpened, NotReady, Open, Out, RunEnd, Step, ToolKind, ToolState,
};
use super::runs::{self, RunLog, What};
use super::ways;
use super::workspace::{Opened, Place};
use super::{folders, git, locked, FolderWorks, Going, Record, State};
use crate::config::DialogProvider;
use crate::router::budgets::Act;
use crate::router::pool::PriceSource;
use crate::router::{Answerer, Bill, Billing, Outcome, Reason, Seat, Spent, Tokens, WorkerAsk, WorkerDeal};
use crate::settings::definitions::WorkFolder;
use crate::talk::folder::{deal_key, FolderCheck, FolderDoing, FolderEnd, FolderWhy, PortEvent};
use crate::utils::time::now_ms;

/// A short limit of a service: waits before trying again.
const BUSY_WAITS: [Duration; 3] = [Duration::from_secs(60), Duration::from_secs(300), Duration::from_secs(900)];
/// A plan's limit without its time: tried again every half hour.
const LIMIT_RETRY: Duration = Duration::from_secs(30 * 60);
/// A plan's limit waited out this many times at most; then the work is «не доделано».
const LIMIT_TIMES: u32 = 100;
/// A command of the folder (preparation, a check) at most this long.
const COMMAND_WAIT: Duration = Duration::from_secs(20 * 60);
/// What a check keeps of its output: its end.
const TAIL: usize = 4000;

/// How a job came out.
struct Came {
    state: State,
    why: Option<FolderWhy>,
    error: Option<String>,
}

impl Came {
    fn of(state: State) -> Came {
        Came { state, why: None, error: None }
    }

    fn why(state: State, why: FolderWhy, error: Option<String>) -> Came {
        Came { state, why: Some(why), error }
    }
}

/// Words of a not-ready agent for the work's record, in the gateway's language.
fn not_ready_words(not: &NotReady) -> (FolderWhy, String) {
    match not {
        NotReady::Missing { .. } => (FolderWhy::NoAgent, crate::tr!("программа агента не найдена")),
        NotReady::Old { version, need } => {
            (FolderWhy::NoAgent, crate::tr!("версия {version} старше нужной {need}", version = version, need = need))
        }
        NotReady::Unknown { .. } => (FolderWhy::NoAgent, crate::tr!("версия агента неизвестна")),
        NotReady::SignedOut { .. } => (FolderWhy::Auth, String::new()),
        NotReady::NoKey => (FolderWhy::Auth, crate::tr!("нет ключа API")),
        NotReady::Gone => (FolderWhy::NoAgent, crate::tr!("агента больше нет")),
    }
}

impl FolderWorks {
    /// A job: the work led to its end, its record kept, the port told.
    pub(super) async fn job(self: Arc<Self>, id: &str, first: String, what: What, resume: String, going: Arc<Going>) {
        let mut came = self.lead(id, first, what, &resume, &going).await;
        // Cut by the gateway's own stop: interrupted, as a restart leaves a work — nothing is told to a closing talk.
        let closing = self.closing() && came.state == State::Stopped;
        if closing {
            came.state = State::Interrupted;
        }
        let Some(mut record) = self.record(id) else { return };
        record.state = came.state;
        record.why = came.why.map(|why| why.as_str().to_string());
        if came.error.is_some() {
            record.error = came.error;
        }
        record.wait_until = None;
        record.ended_at = Some(now_ms());
        self.keep(&mut record);
        let end = match came.state {
            State::Ready => FolderEnd::Ready,
            State::Stopped => FolderEnd::Stopped,
            State::Failed => FolderEnd::Failed,
            _ => FolderEnd::Unfinished,
        };
        tracing::info!(target: "works", "work {id} ended: {}", came.state.as_str());
        if !closing {
            self.tell(PortEvent::Ended { id: id.to_string(), end });
        }
    }

    async fn lead(&self, id: &str, first: String, what: What, resume: &str, going: &Arc<Going>) -> Came {
        let Some(mut record) = self.record(id) else { return Came::of(State::Failed) };
        let Some(folder) = folders::get(&self.settings, &record.folder) else {
            return Came::why(State::Failed, FolderWhy::Folder, None);
        };
        let Some(harness) = self.harness(&record.agent) else {
            return Came::why(State::Failed, FolderWhy::NoAgent, Some(crate::tr!("агента больше нет")));
        };
        let ready = harness.ready().await;
        if let Some(not) = &ready.not {
            let (why, words) = not_ready_words(not);
            return Came::why(State::Failed, why, Some(words).filter(|words| !words.is_empty()));
        }
        // The router's door before the first run: the regulator, the budget's step, the model and the ceiling.
        let untouched = record.runs == 0;
        let mut deal =
            match self.through_door(id, &mut record, &folder, harness.as_ref(), going, false, untouched).await {
                Ok(deal) => deal,
                Err(came) => return came,
            };
        let folder_path = std::path::PathBuf::from(&folder.path);
        // The first run of the work: its base and a branch nobody has.
        if record.base_commit.is_none() {
            record.base = git::current_branch(&folder_path).await;
            record.base_commit = git::head_commit(&folder_path).await;
            let taken = self.taken_branches(&record.folder, id).await;
            let wanted = record.branch.clone();
            let mut n = 2;
            while git::branch_exists(&folder_path, &record.branch).await || taken.contains(&record.branch) {
                record.branch = format!("{wanted}-{n}");
                n += 1;
            }
            self.keep(&mut record);
        }
        let copy = self.copy_of(id);
        let place = Place {
            folder: folder_path.clone(),
            copy: copy.clone(),
            branch: record.branch.clone(),
            base_commit: record.base_commit.clone(),
        };
        let mut text = first;
        let mut what = what;
        // The model of the session: the door's, else the work's own.
        let mut model = deal.as_ref().map_or(record.model.clone(), |deal| deal.model.clone());
        // «С планом»: its first run is the plan's (the copy is made in it), the work's own comes after the «да».
        let planning = what == What::Work && record.way.as_deref() == Some(ways::PLAN) && record.plan.is_none();
        let first = if planning { What::Plan } else { what };
        let (mut run, mut log) = match self.begin_run(id, first, &record, model.as_deref(), going) {
            Ok(begun) => begun,
            Err(error) => return Came::why(State::Failed, FolderWhy::Failed, Some(error.to_string())),
        };
        // The copy: kept, or made — and prepared when new.
        let opened = match self.workspace.open(&place).await {
            Ok(opened) => {
                log.step(
                    None,
                    Step::Chore {
                        chore: Chore::Copy,
                        command: None,
                        ok: true,
                        output: Some(copy.display().to_string()),
                    },
                );
                opened
            }
            Err(why) => {
                log.step(
                    None,
                    Step::Chore { chore: Chore::Copy, command: None, ok: false, output: Some(cut_to(&why, 2000)) },
                );
                self.end_run(id, &run, "failed", None, None, &log);
                return Came::why(State::Failed, FolderWhy::Git, Some(cut_to(&why, 300)));
            }
        };
        record.copy = Some(copy.display().to_string());
        self.keep(&mut record);
        if opened == Opened::New {
            *locked(&going.doing) = Some(FolderDoing::Prepares);
            for command in &folder.prepare {
                let done = self.command(&copy, command, &going.stop).await;
                let ok = done.ok;
                log.step(
                    None,
                    Step::Chore { chore: Chore::Prepare, command: Some(command.clone()), ok, output: Some(done.tail) },
                );
                if going.stop.is_cancelled() {
                    self.end_run(id, &run, "stopped", None, None, &log);
                    return Came::of(State::Stopped);
                }
                if !ok {
                    self.end_run(id, &run, "failed", None, None, &log);
                    return Came::why(
                        State::Failed,
                        FolderWhy::Failed,
                        Some(crate::tr!("подготовка «{command}» не прошла", command = command)),
                    );
                }
            }
        }
        // «С планом» (works/ways.rs, stage 6): the plan first, in a session that changes nothing; the work waits for the
        // person's «да», then goes by it.
        if planning {
            let prompt = ways::plan_prompt(&record.task);
            let plan = self
                .read_only_session(id, (&run, &log), &folder, harness.as_ref(), &copy, &prompt, model.clone(), going)
                .await;
            let Some(plan) = plan.filter(|plan| !plan.trim().is_empty()) else {
                if going.stop.is_cancelled() {
                    return Came::of(State::Stopped);
                }
                return Came::why(State::Failed, FolderWhy::Failed, Some(crate::tr!("план не пришёл")));
            };
            if let Some(mut fresh) = self.record(id) {
                fresh.plan = Some(plan.clone());
                self.keep(&mut fresh);
                record = fresh;
            }
            if !self.wait_plan(id, &mut record, &plan, going).await {
                return Came::of(State::Stopped);
            }
            text = ways::with_plan(&text, &plan);
            match self.begin_run(id, What::Work, &record, model.as_deref(), going) {
                Ok((next_run, next_log)) => {
                    run = next_run;
                    log = next_log;
                }
                Err(error) => return Came::why(State::Failed, FolderWhy::Failed, Some(error.to_string())),
            }
        }
        // Its session goes on only on the model it was opened with: on another one (a budget's step) it is a new one,
        // told the task and the last result.
        let fresh = record.session.is_some() && model != record.session_model();
        if fresh {
            text = switched(&record, &text);
        }
        let open = Open {
            copy: copy.clone(),
            model: model.clone(),
            max_usd: match &deal {
                Some(deal) => ceiling(deal),
                None => folder.max_usd,
            },
            allow: folder.allow.clone(),
            session: if fresh { None } else { record.session.clone() },
            recap: record.last.clone(),
            read_only: false,
        };
        let session = match harness.open(open).await {
            Ok(session) => session,
            Err(not) => {
                log.step(
                    None,
                    Step::End {
                        outcome: "failed".into(),
                        text: None,
                        error: Some(format!("{not:?}")),
                        usd_micro: None,
                        unknown: None,
                    },
                );
                self.end_run(id, &run, "failed", None, None, &log);
                return match not {
                    NotOpened::Auth(_) => Came::why(State::Failed, FolderWhy::Auth, None),
                    NotOpened::Protocol(words) | NotOpened::Failed(words) => {
                        Came::why(State::Failed, FolderWhy::NoAgent, Some(cut_to(&words, 300)))
                    }
                };
            }
        };
        self.opened_on(&mut record, model.as_deref());
        let mut session = Some(session);
        let mut busy = 0usize;
        let mut limits = 0u32;
        // «С проверкой» and «с планом»: the review's rounds (works/ways.rs).
        let reviewer = {
            let all = self.settings.get(&crate::router::sets::WORK_SETS, None).unwrap_or_default();
            ways::reviewer_model(&all, record.model_set.as_deref(), &record.agent)
        };
        let mut reviews = 0u32;
        let came = 'work: loop {
            let came = loop {
                let Some(current) = session.as_mut() else {
                    break Came::why(State::Unfinished, FolderWhy::Failed, None);
                };
                *locked(&going.doing) = Some(FolderDoing::Begins);
                self.told(id, run.n, "started");
                let end = current.prompt(&text, log.clone(), going.stop.clone()).await;
                let session_id = end.session.clone().or_else(|| current.id());
                self.end_run(id, &run, end.ended.as_str(), end.model.as_deref(), session_id.as_deref(), &log);
                if let Some(mut fresh) = self.record(id) {
                    fresh.session = session_id.clone().or(fresh.session);
                    fresh.billing = end.billing.clone().or(fresh.billing);
                    if let Some(usd) = end.usd_micro {
                        fresh.usd_micro = Some(fresh.usd_micro.unwrap_or(0) + usd);
                    }
                    fresh.last = end.text.clone().or(fresh.last);
                    fresh.runs = run.n;
                    fresh.state = State::Working;
                    fresh.wait_until = None;
                    self.keep(&mut fresh);
                    record = fresh;
                }
                self.tell(PortEvent::Ran {
                    id: id.to_string(),
                    n: run.n,
                    spent: spent_of(id, harness.as_ref(), model.as_deref(), deal.as_ref(), &end),
                });
                // Its ceiling reached: the door is asked whether the budget is used up; if it says not, the work ends.
                let hit = end.ended == Ended::Budget && self.door.get().is_some();
                let next = match &end.ended {
                    Ended::Ok => {
                        let notes = std::mem::take(&mut *locked(&going.notes));
                        if notes.is_empty() {
                            break Came::of(State::Ready);
                        }
                        self.tell(PortEvent::Taken { id: id.to_string() });
                        Some((notes.join("\n\n"), What::Note))
                    }
                    Ended::Stopped => break Came::of(State::Stopped),
                    Ended::Limit { until } => {
                        limits += 1;
                        if limits > LIMIT_TIMES {
                            break Came::why(State::Unfinished, FolderWhy::Limit, end.error.clone());
                        }
                        let until = until.filter(|until| *until > now_ms());
                        if !self.wait_limit(id, &mut record, until, harness.as_ref(), going).await {
                            break Came::of(State::Stopped);
                        }
                        Some((resume.to_string(), What::Resume))
                    }
                    Ended::Busy => {
                        let Some(wait) = BUSY_WAITS.get(busy).copied() else {
                            break Came::why(State::Unfinished, FolderWhy::Limit, end.error.clone());
                        };
                        busy += 1;
                        *locked(&going.doing) = Some(FolderDoing::Waits(Some(now_ms() + wait.as_millis() as u64)));
                        tokio::select! {
                            _ = tokio::time::sleep(wait) => {}
                            _ = going.stop.cancelled() => break Came::of(State::Stopped),
                        }
                        Some((resume.to_string(), What::Resume))
                    }
                    Ended::Billing => break Came::why(State::Unfinished, FolderWhy::Billing, end.error.clone()),
                    Ended::Auth => break Came::why(State::Unfinished, FolderWhy::Auth, end.error.clone()),
                    Ended::Budget if hit => Some((resume.to_string(), What::Resume)),
                    Ended::Budget => break Came::why(State::Unfinished, FolderWhy::Budget, end.error.clone()),
                    Ended::Turns => break Came::why(State::Unfinished, FolderWhy::Turns, end.error.clone()),
                    Ended::Refused => break Came::why(State::Unfinished, FolderWhy::Refused, end.error.clone()),
                    Ended::Failed => break Came::why(State::Unfinished, FolderWhy::Failed, end.error.clone()),
                };
                let Some((next_text, next_what)) = next else { break Came::of(State::Ready) };
                text = next_text;
                what = next_what;
                // The door before the next run: the budget's step may want a cheaper model, a question, a stop. Without a
                // door, the work's own model: a set switched while it waited changes it.
                let door = self.door.get().is_some();
                if door || record.model != model {
                    let next =
                        match self.through_door(id, &mut record, &folder, harness.as_ref(), going, hit, false).await {
                            Ok(next) => next,
                            Err(came) => break came,
                        };
                    let next_model =
                        if door { next.as_ref().and_then(|deal| deal.model.clone()) } else { record.model.clone() };
                    if next_model != model {
                        // Another model is another session (the model never changes in the middle of one): told the task
                        // and the last result, in the same copy.
                        if let Some(old) = session.take() {
                            old.close().await;
                        }
                        let open = Open {
                            copy: copy.clone(),
                            model: next_model.clone(),
                            max_usd: next.as_ref().and_then(ceiling),
                            allow: folder.allow.clone(),
                            session: None,
                            recap: None,
                            read_only: false,
                        };
                        match harness.open(open).await {
                            Ok(opened) => session = Some(opened),
                            Err(not) => {
                                break Came::why(State::Unfinished, FolderWhy::NoAgent, Some(format!("{not:?}")))
                            }
                        }
                        self.opened_on(&mut record, next_model.as_deref());
                        text = switched(&record, &text);
                        tracing::info!(
                            target: "works",
                            "work {id}: a new session on {} ({})",
                            next_model.as_deref().unwrap_or("the agent's own model"),
                            next.as_ref().map_or("seat".into(), |deal| deal.reason.as_str())
                        );
                        model = next_model;
                    } else if let Some(current) = session.as_mut() {
                        current.ceiling(next.as_ref().and_then(ceiling));
                    }
                    deal = next;
                }
                match self.begin_run(id, what, &record, model.as_deref(), going) {
                    Ok((next_run, next_log)) => {
                        run = next_run;
                        log = next_log;
                    }
                    Err(error) => break Came::why(State::Unfinished, FolderWhy::Failed, Some(error.to_string())),
                }
            };
            // The work done is reviewed in a fresh session on another model, read only; its blocking remarks go back to
            // the writer's session, two rounds at most, then the person decides.
            if came.state != State::Ready || !ways::reviews(record.way.as_deref()) || going.stop.is_cancelled() {
                break 'work came;
            }
            let prompt = ways::review_prompt(&record.task, record.base_commit.as_deref(), record.plan.as_deref());
            let said = self
                .read_only_run(
                    id,
                    What::Review,
                    &record,
                    &folder,
                    harness.as_ref(),
                    &copy,
                    &prompt,
                    reviewer.clone(),
                    going,
                )
                .await;
            let Some(verdict) = said.as_deref().and_then(ways::verdict_of) else {
                // A review that says nothing readable leaves the work as it is.
                break 'work came;
            };
            if verdict.clean() {
                self.reviewed(id, if reviews == 0 { "clean".to_string() } else { format!("fixed:{reviews}") });
                break 'work came;
            }
            if reviews >= ways::REVIEW_ROUNDS {
                let left = verdict.blocking.join("; ");
                self.reviewed(id, left.clone());
                break 'work Came::why(State::Unfinished, FolderWhy::Review, Some(cut_to(&left, 300)));
            }
            reviews += 1;
            text = ways::fix_prompt(&verdict.blocking);
            match self.begin_run(id, What::Note, &record, model.as_deref(), going) {
                Ok((next_run, next_log)) => {
                    run = next_run;
                    log = next_log;
                }
                Err(error) => break 'work Came::why(State::Unfinished, FolderWhy::Failed, Some(error.to_string())),
            }
        };
        if let Some(session) = session {
            session.close().await;
        }
        // What was done goes to the person, even when it was stopped: committed, the branch in the folder.
        *locked(&going.doing) = Some(FolderDoing::Hands);
        let handed = self.workspace.hand_over(&place, &record.title).await;
        if handed.committed {
            log.step(None, Step::Chore { chore: Chore::Commit, command: None, ok: true, output: None });
        }
        log.step(
            None,
            Step::Chore {
                chore: Chore::Fetch,
                command: Some(record.branch.clone()),
                ok: handed.fetched,
                output: handed.error.clone(),
            },
        );
        let mut checks = None;
        let mut skipped = false;
        let checked = matches!(came.state, State::Ready | State::Unfinished) && came.why != Some(FolderWhy::Auth);
        if checked && !folder.checks.is_empty() {
            if folder.prepare.is_empty() {
                // A fresh copy has no dependencies: without preparation the checks would say nothing true.
                skipped = true;
                log.step(
                    None,
                    Step::Chore { chore: Chore::Check, command: None, ok: true, output: Some("skipped".into()) },
                );
            } else {
                let mut results = Vec::new();
                for command in &folder.checks {
                    *locked(&going.doing) = Some(FolderDoing::Checks(command.clone()));
                    let done = self.command(&copy, command, &going.stop).await;
                    log.step(
                        None,
                        Step::Chore {
                            chore: Chore::Check,
                            command: Some(command.clone()),
                            ok: done.ok,
                            output: Some(done.tail.clone()),
                        },
                    );
                    results.push(FolderCheck {
                        command: command.clone(),
                        ok: done.ok,
                        code: done.code,
                        ms: done.ms,
                        tail: done.tail,
                    });
                    if going.stop.is_cancelled() {
                        break;
                    }
                }
                checks = Some(results);
            }
        }
        if let Some(mut fresh) = self.record(id) {
            fresh.committed |= handed.committed;
            fresh.fetched = handed.fetched;
            fresh.changed = handed.changed;
            fresh.error = handed.error.clone().map(|error| cut_to(&error, 300));
            if checks.is_some() || skipped {
                fresh.checks = checks;
                fresh.checks_skipped = skipped;
            }
            self.keep(&mut fresh);
        }
        self.told(id, run.n, "ended");
        came
    }

    /// A new run: its row, its journal; what it hears goes to the work's line and the panel.
    fn begin_run(
        &self,
        id: &str,
        what: What,
        record: &Record,
        model: Option<&str>,
        going: &Arc<Going>,
    ) -> std::io::Result<(runs::Run, Arc<RunLog>)> {
        let run = runs::begin(&self.db, id, what, model, record.session.as_deref())
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let me: Weak<FolderWorks> = self.me.clone();
        let (work, n, going) = (id.to_string(), run.n, going.clone());
        let heard = Box::new(move |step: &Step| {
            if let Some(doing) = doing_of(step) {
                *locked(&going.doing) = Some(doing);
            }
            let Some(works) = me.upgrade() else { return };
            if let Step::Begin { model: Some(model), .. } = step {
                let provider = works.record(&work).map(|record| record.agent).unwrap_or_default();
                works.tell(PortEvent::Model { id: work.clone(), provider, model: model.clone() });
            }
            works.told(&work, n, "step");
        });
        let log = RunLog::create(&self.dir, id, run.n, heard)?;
        Ok((run, log))
    }

    fn end_run(
        &self,
        id: &str,
        run: &runs::Run,
        outcome: &str,
        model: Option<&str>,
        session: Option<&str>,
        log: &RunLog,
    ) {
        let outcome = if self.closing() && outcome == "stopped" { "interrupted" } else { outcome };
        runs::end(&self.db, id, run.n, outcome, model, session, log.was_cut());
    }

    /// The router's door before a run (docs/plans/router-and-works.md, 5): the regulator may keep the agent's model in;
    /// the narrowest budget's step may stop the work, or ask the person — then the work waits for «добавить» (its
    /// budget grew: the door is asked again) or «остановить». `hit` — the last run reached its ceiling: a door that
    /// sees no step to stop at ends the work rather than run it again. `untouched` — no run of the work ever went: a
    /// work cut before it failed rather than stayed unfinished. None — no door: as before it.
    #[allow(clippy::too_many_arguments)]
    async fn through_door(
        &self,
        id: &str,
        record: &mut Record,
        folder: &WorkFolder,
        harness: &dyn Harness,
        going: &Going,
        mut hit: bool,
        untouched: bool,
    ) -> Result<Option<WorkerDeal>, Came> {
        let Some(door) = self.door.get() else { return Ok(None) };
        let cut = if untouched { State::Failed } else { State::Unfinished };
        loop {
            let ask = WorkerAsk {
                agent: record.agent.clone(),
                model: record.model.clone(),
                cloud: harness.cloud(),
                folder: folders::budget_of(folder),
                ..WorkerAsk::default()
            };
            let deal = door(&deal_key(id), ask);
            if deal.closed {
                return Err(Came::why(cut, FolderWhy::Closed, None));
            }
            match deal.step {
                Some(Act::Stop) => return Err(Came::why(cut, FolderWhy::Budget, None)),
                Some(Act::Ask) => {
                    if !self.wait_budget(id, record, &deal, going).await {
                        return Err(Came::of(State::Stopped));
                    }
                    hit = false;
                }
                _ if hit => return Err(Came::why(State::Unfinished, FolderWhy::Budget, None)),
                _ => return Ok(Some(deal)),
            }
        }
    }

    /// A run in a session that changes nothing (the plan, the review of stage 6): its own row and journal, a fresh
    /// session on `model`, the copy's history readable; the agent's last words, or None.
    #[allow(clippy::too_many_arguments)]
    async fn read_only_run(
        &self,
        id: &str,
        what: What,
        record: &Record,
        folder: &WorkFolder,
        harness: &dyn Harness,
        copy: &Path,
        prompt: &str,
        model: Option<String>,
        going: &Arc<Going>,
    ) -> Option<String> {
        let (run, log) = self.begin_run(id, what, record, model.as_deref(), going).ok()?;
        self.read_only_session(id, (&run, &log), folder, harness, copy, prompt, model, going).await
    }

    /// The same in a run already begun.
    #[allow(clippy::too_many_arguments)]
    async fn read_only_session(
        &self,
        id: &str,
        (run, log): (&runs::Run, &Arc<RunLog>),
        folder: &WorkFolder,
        harness: &dyn Harness,
        copy: &Path,
        prompt: &str,
        model: Option<String>,
        going: &Arc<Going>,
    ) -> Option<String> {
        let mut allow: Vec<String> = ways::READ_ONLY.iter().map(|rule| rule.to_string()).collect();
        allow.extend(folder.allow.iter().filter(|rule| rule.starts_with("WebFetch")).cloned());
        let open = Open {
            copy: copy.to_path_buf(),
            model: model.clone(),
            max_usd: folder.max_usd,
            allow,
            session: None,
            recap: None,
            read_only: true,
        };
        let mut session = match harness.open(open).await {
            Ok(session) => session,
            Err(_) => {
                self.end_run(id, run, "failed", None, None, log);
                return None;
            }
        };
        *locked(&going.doing) = Some(FolderDoing::Thinks);
        let end = session.prompt(prompt, log.clone(), going.stop.clone()).await;
        let session_id = end.session.clone().or_else(|| session.id());
        self.end_run(id, run, end.ended.as_str(), end.model.as_deref(), session_id.as_deref(), log);
        self.tell(PortEvent::Ran {
            id: id.to_string(),
            n: run.n,
            spent: spent_of(id, harness, model.as_deref(), None, &end),
        });
        session.close().await;
        (end.ended == Ended::Ok).then_some(end.text).flatten()
    }

    /// «С планом»: the plan to the person, then the wait for «да» (a nudge) or a stop. False — stopped.
    async fn wait_plan(&self, id: &str, record: &mut Record, plan: &str, going: &Going) -> bool {
        record.state = State::Waiting;
        self.keep(record);
        *locked(&going.doing) = Some(FolderDoing::WaitsPlan);
        self.tell(PortEvent::Plan { id: id.to_string(), plan: plan.to_string() });
        let yes = tokio::select! {
            _ = going.nudge.notified() => true,
            _ = going.stop.cancelled() => false,
        };
        if yes {
            record.state = State::Working;
            self.keep(record);
            *locked(&going.doing) = Some(FolderDoing::Begins);
        }
        yes
    }

    /// What the review came to, kept with the work for its «Готово».
    fn reviewed(&self, id: &str, review: String) {
        if let Some(mut fresh) = self.record(id) {
            fresh.review = Some(review);
            self.keep(&mut fresh);
        }
    }

    /// The work's model and set as kept now: a set switched while it waited (stage 5 of docs/plans/work-systems.md).
    fn refresh_model(&self, record: &mut Record) {
        if let Some(fresh) = self.record(&record.id) {
            record.model = fresh.model;
            record.model_set = fresh.model_set;
        }
    }

    /// A session was opened on this model (none — the agent's own): kept, so «продолжай» knows whether it may go on in
    /// it.
    fn opened_on(&self, record: &mut Record, model: Option<&str>) {
        if let Some(mut fresh) = self.record(&record.id) {
            fresh.session_model = Some(model.unwrap_or_default().to_string());
            self.keep(&mut fresh);
            *record = fresh;
        }
    }

    /// The budget's question to the person, then the wait: for its budget to grow, or for a stop. False — stopped.
    async fn wait_budget(&self, id: &str, record: &mut Record, deal: &WorkerDeal, going: &Going) -> bool {
        record.state = State::Waiting;
        record.wait_until = None;
        self.keep(record);
        *locked(&going.doing) = Some(FolderDoing::WaitsBudget);
        self.tell(PortEvent::Budget { id: id.to_string(), deal: Box::new(deal.clone()) });
        let grew = tokio::select! {
            _ = going.nudge.notified() => true,
            _ = going.stop.cancelled() => false,
        };
        if grew {
            self.refresh_model(record);
            record.state = State::Working;
            self.keep(record);
            *locked(&going.doing) = Some(FolderDoing::Begins);
        }
        grew
    }

    /// A plan's limit: the question to the person, then the wait — until its time, or half an hour without one. False
    /// — stopped meanwhile.
    async fn wait_limit(
        &self,
        id: &str,
        record: &mut Record,
        until: Option<u64>,
        harness: &dyn Harness,
        going: &Going,
    ) -> bool {
        record.state = State::Waiting;
        record.wait_until = until;
        self.keep(record);
        *locked(&going.doing) = Some(FolderDoing::Waits(until));
        self.tell(PortEvent::Waits { id: id.to_string(), until, agent: harness.title() });
        let wait =
            until.map(|until| Duration::from_millis(until.saturating_sub(now_ms()) + 5_000)).unwrap_or(LIMIT_RETRY);
        let waited = tokio::select! {
            _ = tokio::time::sleep(wait) => true,
            // «Перейти на «Экономно»» or «Продолжить»: it goes on now.
            _ = going.nudge.notified() => true,
            _ = going.stop.cancelled() => false,
        };
        if waited {
            self.refresh_model(record);
            record.state = State::Working;
            record.wait_until = None;
            self.keep(record);
            self.tell(PortEvent::Goes { id: id.to_string() });
        }
        waited
    }

    /// A command of the folder in the copy, with the agent's environment: whether it passed, its code, how long,
    /// the end of what it printed.
    async fn command(&self, copy: &Path, command: &str, stop: &CancellationToken) -> Done {
        let began = Instant::now();
        #[cfg(unix)]
        let mut process = {
            let mut process = tokio::process::Command::new("sh");
            process.arg("-c").arg(command);
            process.process_group(0);
            process
        };
        #[cfg(not(unix))]
        let mut process = {
            let mut process = tokio::process::Command::new("cmd");
            process.arg("/C").arg(command);
            process
        };
        process
            .current_dir(copy)
            .env_clear()
            .envs(environment(&[]))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = match process.spawn() {
            Ok(child) => child,
            Err(error) => return Done { ok: false, code: None, ms: 0, tail: error.to_string() },
        };
        let pid = child.id();
        let (mut out, mut err) = (child.stdout.take(), child.stderr.take());
        let read = async {
            let mut text = Vec::new();
            if let Some(out) = out.as_mut() {
                let _ = out.read_to_end(&mut text).await;
            }
            let mut more = Vec::new();
            if let Some(err) = err.as_mut() {
                let _ = err.read_to_end(&mut more).await;
            }
            text.extend(more);
            text
        };
        let outcome = tokio::select! {
            (text, status) = async { let text = read.await; (text, child.wait().await) } => Some((text, status)),
            _ = tokio::time::sleep(COMMAND_WAIT) => None,
            _ = stop.cancelled() => None,
        };
        let ms = began.elapsed().as_millis() as u64;
        match outcome {
            Some((text, Ok(status))) => {
                let text = String::from_utf8_lossy(&text);
                Done { ok: status.success(), code: status.code(), ms, tail: tail(&text) }
            }
            Some((_, Err(error))) => Done { ok: false, code: None, ms, tail: error.to_string() },
            None => {
                #[cfg(unix)]
                if let Some(pid) = pid.and_then(|pid| i32::try_from(pid).ok()) {
                    // SAFETY: killpg has no memory effects; a group that is gone answers ESRCH.
                    unsafe {
                        libc::killpg(pid, libc::SIGKILL);
                    }
                }
                // Windows: cmd and the command under it, at once (cmd alone would leave the command running).
                #[cfg(windows)]
                if let Some(pid) = pid {
                    crate::utils::process::kill_tree(pid);
                }
                #[cfg(not(any(unix, windows)))]
                let _ = pid;
                let _ = child.start_kill();
                let words = if stop.is_cancelled() { "stopped" } else { "took too long" };
                Done { ok: false, code: None, ms, tail: words.into() }
            }
        }
    }
}

/// A command's result.
struct Done {
    ok: bool,
    code: Option<i32>,
    ms: u64,
    tail: String,
}

fn tail(text: &str) -> String {
    let text = text.trim_end();
    if text.len() <= TAIL {
        return text.to_string();
    }
    let mut start = text.len() - TAIL;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

/// What a step says the work is at.
fn doing_of(step: &Step) -> Option<FolderDoing> {
    match step {
        Step::Begin { .. } => Some(FolderDoing::Begins),
        Step::Says { .. } => Some(FolderDoing::Writes),
        Step::Thinks { .. } | Step::Plan { .. } => Some(FolderDoing::Thinks),
        Step::Tool { kind, title, path, command, state: ToolState::InProgress | ToolState::Pending, .. } => {
            let what = || path.clone().or_else(|| command.clone()).unwrap_or_else(|| title.clone());
            Some(match kind {
                ToolKind::Read => FolderDoing::Reads(what()),
                ToolKind::Edit | ToolKind::Delete | ToolKind::Move => FolderDoing::Edits(what()),
                ToolKind::Execute => FolderDoing::Runs(command.clone().unwrap_or_else(|| title.clone())),
                ToolKind::Search => FolderDoing::Searches(title.clone()),
                ToolKind::Fetch => FolderDoing::Fetches(title.clone()),
                _ => FolderDoing::Thinks,
            })
        }
        Step::Limit { until } => Some(FolderDoing::Waits(*until)),
        Step::Chore { chore: Chore::Prepare, .. } => Some(FolderDoing::Prepares),
        Step::Chore { chore: Chore::Check, command: Some(command), .. } => Some(FolderDoing::Checks(command.clone())),
        Step::Chore { chore: Chore::Commit | Chore::Fetch, .. } => Some(FolderDoing::Hands),
        _ => None,
    }
}

/// What a run may spend in dollars, as the door said: at least a cent — a ceiling of nothing would be none at all.
fn ceiling(deal: &WorkerDeal) -> Option<f64> {
    deal.ceiling_usd.map(|usd| usd.max(0.01))
}

/// The prompt of a new session on another model: the task and the last result, then what goes on.
fn switched(record: &Record, text: &str) -> String {
    let last = record.last.as_deref().unwrap_or("—");
    format!(
        "{}\n\n{}\n\n{}\n\n{last}\n\n---\n\n{text}",
        crate::tr!("Это продолжение работы, начатой на другой модели, в той же рабочей копии. Задание было:"),
        record.task,
        crate::tr!("Последний итог:"),
    )
}

/// A run as a deal: the agent's seat «Исполнитель», its way, its model, why it, its tokens, its own dollars.
fn spent_of(id: &str, harness: &dyn Harness, model: Option<&str>, deal: Option<&WorkerDeal>, end: &RunEnd) -> Spent {
    let via = harness.via();
    let builtin = via.starts_with("harness:");
    let billing =
        end.billing.as_deref().and_then(Billing::parse).unwrap_or(if builtin { Billing::Plan } else { Billing::Agent });
    let model = end.model.clone().or(model.map(str::to_string)).unwrap_or_else(|| "default".into());
    let tokens: Tokens = end.tokens();
    Spent {
        key: deal_key(id),
        by: Answerer {
            provider: harness.id(),
            model,
            kind: if builtin { DialogProvider::Anthropic } else { DialogProvider::OpenAi },
            cloud: true,
            seat: Seat::Worker,
            reason: deal.map_or(Reason::Seat, |deal| deal.reason.clone()),
        },
        tokens,
        rounds: 1,
        ms: end.ms,
        outcome: Outcome::parse(end.ended.as_str()),
        bill: Some(Bill {
            via,
            billing,
            usd_micro: end.usd_micro,
            from: if builtin { PriceSource::Harness } else { PriceSource::Agent },
        }),
    }
}
