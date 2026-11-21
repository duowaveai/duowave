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
use super::workspace::{Opened, Place};
use super::{folders, git, locked, FolderWorks, Going, Record, State};
use crate::config::DialogProvider;
use crate::router::pool::PriceSource;
use crate::router::{Answerer, Bill, Billing, Outcome, Reason, Seat, Spent, Tokens};
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
        let (mut run, mut log) = match self.begin_run(id, what, &record, going) {
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
        let open = Open {
            copy: copy.clone(),
            model: record.model.clone(),
            max_usd: folder.max_usd,
            allow: folder.allow.clone(),
            session: record.session.clone(),
            recap: record.last.clone(),
        };
        let mut session = match harness.open(open).await {
            Ok(session) => session,
            Err(not) => {
                log.step(
                    None,
                    Step::End {
                        outcome: "failed".into(),
                        text: None,
                        error: Some(format!("{not:?}")),
                        usd_micro: None,
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
        let mut busy = 0usize;
        let mut limits = 0u32;
        let came = loop {
            *locked(&going.doing) = Some(FolderDoing::Begins);
            self.told(id, run.n, "started");
            let end = session.prompt(&text, log.clone(), going.stop.clone()).await;
            let session_id = end.session.clone().or_else(|| session.id());
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
                spent: spent_of(id, harness.as_ref(), &record, &end),
            });
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
                Ended::Budget => break Came::why(State::Unfinished, FolderWhy::Budget, end.error.clone()),
                Ended::Turns => break Came::why(State::Unfinished, FolderWhy::Turns, end.error.clone()),
                Ended::Refused => break Came::why(State::Unfinished, FolderWhy::Refused, end.error.clone()),
                Ended::Failed => break Came::why(State::Unfinished, FolderWhy::Failed, end.error.clone()),
            };
            let Some((next_text, next_what)) = next else { break Came::of(State::Ready) };
            text = next_text;
            what = next_what;
            match self.begin_run(id, what, &record, going) {
                Ok((next_run, next_log)) => {
                    run = next_run;
                    log = next_log;
                }
                Err(error) => break Came::why(State::Unfinished, FolderWhy::Failed, Some(error.to_string())),
            }
        };
        session.close().await;
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
        going: &Arc<Going>,
    ) -> std::io::Result<(runs::Run, Arc<RunLog>)> {
        let run = runs::begin(&self.db, id, what, record.model.as_deref(), record.session.as_deref())
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
            _ = going.stop.cancelled() => false,
        };
        if waited {
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

/// A run as a deal: the agent's seat «Исполнитель», its way, its model, its tokens, its own dollars.
fn spent_of(id: &str, harness: &dyn Harness, record: &Record, end: &RunEnd) -> Spent {
    let via = harness.via();
    let builtin = via.starts_with("harness:");
    let billing =
        end.billing.as_deref().and_then(Billing::parse).unwrap_or(if builtin { Billing::Plan } else { Billing::Agent });
    let model = end.model.clone().or(record.model.clone()).unwrap_or_else(|| "default".into());
    let tokens: Tokens = end.tokens();
    Spent {
        key: deal_key(id),
        by: Answerer {
            provider: harness.id(),
            model,
            kind: if builtin { DialogProvider::Anthropic } else { DialogProvider::OpenAi },
            cloud: true,
            seat: Seat::Worker,
            reason: Reason::Seat,
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
