//! The own voices' heavy work — installing an engine, taking an imprint — one job at a time, watched. A job is a list
//! of steps; a step that runs a program speaks JSON lines ({"stage", "percent", "memoryMb", "error"}) or `@@stage`
//! lines, and the rest of its output is the log the panel shows. Under a memory guard the watchdog samples the system
//! every 2 seconds while the job runs (and only then) and stops it before the household's recognition is pushed into
//! swap.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::future::BoxFuture;
use futures::FutureExt;
use serde::Serialize;
use serde_json::{Map, Value};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::js;
use super::memory_policy::{stop_message, MemoryWatch, StopReason, WatchSample};
use super::platform::{OsName, PlatformInfo, Taskpolicy};
use super::system_memory::MemorySnapshot;
use crate::utils::ids::uuid;
use crate::utils::time::now_iso;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobKind {
    Install,
    Imprint,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::Install => "install",
            JobKind::Imprint => "imprint",
        }
    }

    /// «установка», «отпечаток».
    pub fn title(self) -> &'static str {
        match self {
            JobKind::Install => "установка",
            JobKind::Imprint => "отпечаток",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobRunState {
    Running,
    Done,
    Error,
    Stopped,
}

impl JobRunState {
    pub fn as_str(self) -> &'static str {
        match self {
            JobRunState::Running => "running",
            JobRunState::Done => "done",
            JobRunState::Error => "error",
            JobRunState::Stopped => "stopped",
        }
    }
}

/// Why a job ended other than done: the memory guard's reasons, the owner's stop, or an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobReason {
    Memory,
    Pressure,
    Swap,
    Stopped,
    Error,
}

impl JobReason {
    pub fn as_str(self) -> &'static str {
        match self {
            JobReason::Memory => "memory",
            JobReason::Pressure => "pressure",
            JobReason::Swap => "swap",
            JobReason::Stopped => "stopped",
            JobReason::Error => "error",
        }
    }
}

impl From<StopReason> for JobReason {
    fn from(reason: StopReason) -> JobReason {
        match reason {
            StopReason::Memory => JobReason::Memory,
            StopReason::Pressure => JobReason::Pressure,
            StopReason::Swap => JobReason::Swap,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobState {
    pub id: String,
    pub kind: JobKind,
    pub engine: String,
    pub state: JobRunState,
    pub stage: String,
    pub label: String,
    pub percent: u32,
    pub log: Vec<String>,
    /// The job's memory as it reports it, MB.
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "js::opt_number")]
    pub memory_mb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "js::opt_number")]
    pub peak_mb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_mb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<JobReason>,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Map<String, Value>>,
}

/// How a step's program runs.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub cwd: Option<PathBuf>,
    /// The whole environment of the program; none: the gateway's own.
    pub env: Option<HashMap<String, String>>,
    /// The watchdog's limit, MB; without it the program runs unguarded (light work: pip, git).
    pub limit_mb: Option<u64>,
    /// Started on the owner's «всё равно»: the watchdog lets a lasting memory warning be (MemoryWatch).
    pub anyway: bool,
    /// The program's 0..100 mapped into this part of the job's progress.
    pub percent: Option<(f64, f64)>,
    /// Human labels of the stage ids the program prints.
    pub labels: &'static [(&'static str, &'static str)],
}

/// A step that failed: what to tell, and why when a program or the guard said why.
#[derive(Debug, Clone, PartialEq)]
pub struct JobError {
    pub message: String,
    pub reason: Option<JobReason>,
}

impl JobError {
    pub fn new(message: impl Into<String>) -> JobError {
        JobError { message: message.into(), reason: None }
    }

    fn because(message: impl Into<String>, reason: JobReason) -> JobError {
        JobError { message: message.into(), reason: Some(reason) }
    }
}

impl From<String> for JobError {
    fn from(message: String) -> JobError {
        JobError::new(message)
    }
}

impl From<&str> for JobError {
    fn from(message: &str) -> JobError {
        JobError::new(message)
    }
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// What the panel is told of work that panicked: the panic itself (its text may carry a value of the environment) is
/// in the gateway's log, where the panic hook printed it.
pub(crate) const PANICKED: &str = "Сбой в шлюзе — подробности в его журнале.";

/// What a job's work returns: its result for the panel, or nothing.
pub type JobOutcome = Result<Option<Map<String, Value>>, JobError>;

pub type SampleMemory = Arc<dyn Fn() -> BoxFuture<'static, MemorySnapshot> + Send + Sync>;

pub struct JobDeps {
    pub sample_memory: SampleMemory,
    /// The watchdog's period.
    pub watch_ms: u64,
    /// How long a program has between the polite signal and the kill.
    pub grace_ms: u64,
}

impl JobDeps {
    pub fn new(sample_memory: SampleMemory) -> JobDeps {
        JobDeps { sample_memory, watch_ms: 2_000, grace_ms: 5_000 }
    }
}

/// Told how a job ended.
pub type Finished = Box<dyn FnOnce(&JobState) + Send>;

/// A job is running already.
#[derive(Debug, Clone)]
pub struct JobBusy {
    pub kind: JobKind,
    pub engine: String,
}

impl std::fmt::Display for JobBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "уже идёт: {} ({})", self.kind.title(), self.engine)
    }
}

const LOG_LINES: usize = 40;

struct Job {
    state: Mutex<JobState>,
    abort: CancellationToken,
}

impl Job {
    fn state(&self) -> MutexGuard<'_, JobState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn stage(&self, stage: &str, label: &str, percent: Option<f64>) {
        let mut state = self.state();
        state.stage = stage.to_string();
        state.label = label.to_string();
        if let Some(percent) = percent {
            let value = js::round(percent).min(99.0);
            if value.is_finite() && value > state.percent as f64 {
                state.percent = value as u32;
            }
        }
    }

    fn log(&self, line: &str) {
        let clean = strip_colours(line);
        let clean = clean.trim_end();
        if js::trim(clean).is_empty() {
            return;
        }
        let entry = if js::length(clean) > 300 { format!("{}…", js::slice(clean, 300)) } else { clean.to_string() };
        let mut state = self.state();
        state.log.push(entry);
        if state.log.len() > LOG_LINES {
            let extra = state.log.len() - LOG_LINES;
            state.log.drain(..extra);
        }
    }

    /// The failure of a job already stopped: what the stop said.
    fn stopped(&self) -> JobError {
        let state = self.state();
        JobError::because(
            state.error.clone().unwrap_or_else(|| "Остановлено.".into()),
            state.reason.unwrap_or(JobReason::Stopped),
        )
    }
}

/// `\x1b[…m` colour codes out of a program's line.
fn strip_colours(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find("\u{1b}[") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let codes = after.bytes().take_while(|b| b.is_ascii_digit() || *b == b';').count();
        if after.as_bytes().get(codes) == Some(&b'm') {
            rest = &after[codes + 1..];
        } else {
            out.push_str("\u{1b}[");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

#[derive(Default)]
struct Slots {
    current: Option<Arc<Job>>,
    last: Option<Arc<Job>>,
}

pub struct JobRunner {
    deps: Arc<JobDeps>,
    slots: Arc<Mutex<Slots>>,
}

impl JobRunner {
    pub fn new(deps: JobDeps) -> JobRunner {
        JobRunner { deps: Arc::new(deps), slots: Arc::new(Mutex::new(Slots::default())) }
    }

    fn slots(&self) -> MutexGuard<'_, Slots> {
        self.slots.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The running job, else the last one finished (until dismissed or replaced).
    pub fn state(&self) -> Option<JobState> {
        let slots = self.slots();
        slots.current.as_ref().or(slots.last.as_ref()).map(|job| job.state().clone())
    }

    pub fn busy(&self) -> bool {
        self.slots().current.is_some()
    }

    /// Starts `work` in its own task; `finished` hears how it ended. Answers at once with the job as it starts.
    pub fn start<W, F>(
        &self,
        kind: JobKind,
        engine: &str,
        work: W,
        finished: Option<Finished>,
    ) -> Result<JobState, JobBusy>
    where
        W: FnOnce(JobContext) -> F + Send + 'static,
        F: std::future::Future<Output = JobOutcome> + Send + 'static,
    {
        let job = {
            let mut slots = self.slots();
            if let Some(running) = &slots.current {
                let running = running.state();
                return Err(JobBusy { kind: running.kind, engine: running.engine.clone() });
            }
            let job = Arc::new(Job {
                state: Mutex::new(JobState {
                    id: uuid(),
                    kind,
                    engine: engine.to_string(),
                    state: JobRunState::Running,
                    stage: "start".into(),
                    label: "Начинаю".into(),
                    percent: 0,
                    log: Vec::new(),
                    memory_mb: None,
                    peak_mb: None,
                    limit_mb: None,
                    error: None,
                    reason: None,
                    started_at: now_iso(),
                    finished_at: None,
                    result: None,
                }),
                abort: CancellationToken::new(),
            });
            slots.current = Some(job.clone());
            slots.last = None;
            job
        };
        let snapshot = job.state().clone();
        let context = JobContext { job: job.clone(), deps: self.deps.clone() };
        let slots = self.slots.clone();
        tokio::spawn(async move {
            // A work that panics ends as a failed job (TS: its finally), never as one running forever that keeps
            // every other job and the takes refused.
            let worked = std::panic::AssertUnwindSafe(async move { work(context).await }).catch_unwind().await;
            let outcome = match worked.unwrap_or_else(|_| Err(JobError::new(PANICKED))) {
                Ok(_) if job.abort.is_cancelled() => Err(job.stopped()),
                other => other,
            };
            {
                let mut state = job.state();
                match outcome {
                    Ok(result) => {
                        state.state = JobRunState::Done;
                        state.percent = 100;
                        if let Some(result) = result {
                            state.result = Some(result);
                        }
                    }
                    Err(error) => {
                        let reason = state.reason.or(error.reason).unwrap_or(if job.abort.is_cancelled() {
                            JobReason::Stopped
                        } else {
                            JobReason::Error
                        });
                        state.reason = Some(reason);
                        state.state =
                            if reason == JobReason::Error { JobRunState::Error } else { JobRunState::Stopped };
                        if state.error.is_none() {
                            state.error = Some(error.message);
                        }
                    }
                }
                state.finished_at = Some(now_iso());
            }
            {
                let mut slots = slots.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                slots.last = Some(job.clone());
                slots.current = None;
            }
            if let Some(finished) = finished {
                // The job's own outcome stands whatever the listener does.
                let state = job.state().clone();
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| finished(&state)));
            }
        });
        Ok(snapshot)
    }

    /// Stops the running job (the panel's «Остановить»).
    pub fn stop(&self) -> bool {
        let slots = self.slots();
        let Some(job) = &slots.current else { return false };
        {
            let mut state = job.state();
            state.reason.get_or_insert(JobReason::Stopped);
            state.error.get_or_insert_with(|| "Остановлено.".into());
        }
        job.abort.cancel();
        true
    }

    pub fn dismiss(&self) {
        self.slots().last = None;
    }
}

/// What a job's work is given: its state, its stop, its progress and log, and a way to run programs.
#[derive(Clone)]
pub struct JobContext {
    job: Arc<Job>,
    deps: Arc<JobDeps>,
}

impl JobContext {
    pub fn state(&self) -> JobState {
        self.job.state().clone()
    }

    /// Cancelled when the job is stopped.
    pub fn signal(&self) -> CancellationToken {
        self.job.abort.clone()
    }

    pub fn stage(&self, stage: &str, label: &str, percent: Option<f64>) {
        self.job.stage(stage, label, percent);
    }

    pub fn log(&self, line: &str) {
        self.job.log(line);
    }

    /// Runs a program to its end; its last JSON object, or its error.
    pub async fn run(
        &self,
        command: &str,
        args: &[String],
        options: RunOptions,
    ) -> Result<Map<String, Value>, JobError> {
        let job = &self.job;
        if job.abort.is_cancelled() {
            return Err(job.stopped());
        }
        let mut program = tokio::process::Command::new(command);
        program.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(cwd) = &options.cwd {
            program.current_dir(cwd);
        }
        if let Some(env) = &options.env {
            program.env_clear().envs(env);
        }
        // Its own group on POSIX, so a stop takes the whole tree; stdin stays open — the job dies with the gateway.
        #[cfg(unix)]
        program.process_group(0);
        #[cfg(windows)]
        program.creation_flags(super::platform::CREATE_NO_WINDOW);
        let (from, to) = options.percent.unwrap_or_else(|| {
            let percent = job.state().percent as f64;
            (percent, percent)
        });
        let mut child = match program.spawn() {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(JobError::because(format!("не найдена программа {command}"), JobReason::Error));
            }
            Err(error) => return Err(JobError::because(error.to_string(), JobReason::Error)),
        };
        let pid = child.id();
        // Kept until the program ends: a closed stdin tells it the gateway is gone.
        let stdin = child.stdin.take();
        let (lines, mut incoming) = mpsc::unbounded_channel::<String>();
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(read_lines(stdout, lines.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(read_lines(stderr, lines.clone()));
        }
        drop(lines);
        let exited = Arc::new(AtomicBool::new(false));
        let (exit_tx, mut exit_rx) = oneshot::channel();
        {
            let exited = exited.clone();
            tokio::spawn(async move {
                let status = child.wait().await;
                exited.store(true, Ordering::SeqCst);
                let _ = exit_tx.send(status);
            });
        }

        let mut last_json: Map<String, Value> = Map::new();
        let mut child_error: Option<(String, JobReason)> = None;
        let mut stop_reason: Option<JobReason> = None;
        let mut kill_at: Option<tokio::time::Instant> = None;
        let mut watch = None;
        if let Some(limit) = options.limit_mb.filter(|limit| *limit > 0) {
            job.state().limit_mb = Some(limit);
            let period = Duration::from_millis(self.deps.watch_ms.max(1));
            let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            watch = Some((MemoryWatch::new(limit, options.anyway), ticker, limit));
        }
        let mut sampling: Option<BoxFuture<'static, MemorySnapshot>> = None;
        let mut status: Option<Option<i32>> = None;
        let mut spawn_error: Option<String> = None;
        let mut lines_done = false;
        let mut abort_seen = false;

        let kill =
            |reason: JobReason, stop_reason: &mut Option<JobReason>, kill_at: &mut Option<tokio::time::Instant>| {
                stop_reason.get_or_insert(reason);
                let Some(pid) = pid else { return };
                if exited.load(Ordering::SeqCst) {
                    return;
                }
                if cfg!(windows) {
                    taskkill(pid);
                    return;
                }
                signal_group(pid, Signal::Term);
                kill_at.get_or_insert(tokio::time::Instant::now() + Duration::from_millis(self.deps.grace_ms));
            };

        while status.is_none() || !lines_done {
            tokio::select! {
                line = incoming.recv(), if !lines_done => match line {
                    Some(line) => self.on_line(&line, from, to, options.labels, &mut last_json, &mut child_error),
                    None => lines_done = true,
                },
                exit = &mut exit_rx, if status.is_none() => match exit {
                    Ok(Ok(exit)) => status = Some(exit.code()),
                    Ok(Err(error)) => {
                        spawn_error = Some(error.to_string());
                        status = Some(None);
                    }
                    Err(_) => status = Some(None),
                },
                _ = job.abort.cancelled(), if !abort_seen => {
                    abort_seen = true;
                    let reason = job.state().reason.unwrap_or(JobReason::Stopped);
                    kill(reason, &mut stop_reason, &mut kill_at);
                }
                _ = async { watch.as_mut().map(|(_, ticker, _)| ticker).expect("guarded").tick().await },
                    if watch.is_some() && status.is_none() => {
                    if sampling.is_none() {
                        sampling = Some((self.deps.sample_memory)());
                    }
                }
                sample = async { sampling.as_mut().expect("guarded").await }, if sampling.is_some() => {
                    sampling = None;
                    if let Some((judge, _, limit)) = watch.as_mut() {
                        let job_mb = job.state().memory_mb;
                        let verdict = judge.observe(WatchSample {
                            job_mb,
                            pressure: sample.pressure,
                            swap_in_mbps: sample.swap_in_mbps,
                            at: sample.at,
                        });
                        if let Some(reason) = verdict.filter(|_| stop_reason.is_none()) {
                            {
                                let mut state = job.state();
                                state.reason = Some(reason.into());
                                state.error = Some(stop_message(reason, *limit));
                            }
                            kill(reason.into(), &mut stop_reason, &mut kill_at);
                        }
                    }
                }
                _ = async { tokio::time::sleep_until(kill_at.expect("guarded")).await }, if kill_at.is_some() => {
                    kill_at = None;
                    if let Some(pid) = pid.filter(|_| !exited.load(Ordering::SeqCst)) {
                        signal_group(pid, Signal::Kill);
                    }
                }
            }
        }
        drop(stdin);

        let code = status.flatten();
        if spawn_error.is_none() && code == Some(0) && stop_reason.is_none() {
            return Ok(last_json);
        }
        // The program said its work is done and only then fell over (an interpreter's shutdown, 06.11: exit 134
        // after the voice was written): the work stands, the fall goes to the log.
        if spawn_error.is_none()
            && stop_reason.is_none()
            && child_error.is_none()
            && last_json.get("stage").and_then(Value::as_str) == Some("done")
        {
            let code = code.map(|code| code.to_string()).unwrap_or_else(|| "null".into());
            job.log(&format!("программа сделала дело и упала на выходе (код {code})"));
            return Ok(last_json);
        }
        let reason = stop_reason.or(child_error.as_ref().map(|(_, reason)| *reason)).unwrap_or(JobReason::Error);
        let state = job.state().clone();
        let message = stop_reason
            .and(state.error.clone())
            .or(child_error.map(|(message, _)| message))
            .or(spawn_error)
            .or(state.log.last().cloned())
            .unwrap_or_else(|| {
                format!(
                    "программа завершилась с кодом {}",
                    code.map(|c| c.to_string()).unwrap_or_else(|| "null".into())
                )
            });
        Err(JobError::because(message, reason))
    }

    fn on_line(
        &self,
        line: &str,
        from: f64,
        to: f64,
        labels: &[(&str, &str)],
        last_json: &mut Map<String, Value>,
        child_error: &mut Option<(String, JobReason)>,
    ) {
        let job = &self.job;
        let text = js::trim(line);
        if text.is_empty() {
            return;
        }
        if text.starts_with('{') {
            if let Ok(Value::Object(message)) = serde_json::from_str::<Value>(text) {
                {
                    let mut state = job.state();
                    if let Some(memory) = message.get("memoryMb").and_then(Value::as_f64) {
                        state.memory_mb = Some(memory);
                        state.peak_mb = Some(state.peak_mb.unwrap_or(0.0).max(memory));
                    }
                    if let Some(peak) = message.get("peakMb").and_then(Value::as_f64) {
                        state.peak_mb = Some(state.peak_mb.unwrap_or(0.0).max(peak));
                    }
                }
                if let Some(error) = message.get("error").and_then(Value::as_str) {
                    let reason = if message.get("reason").and_then(Value::as_str) == Some("memory") {
                        JobReason::Memory
                    } else {
                        JobReason::Error
                    };
                    *child_error = Some((error.to_string(), reason));
                }
                if let Some(log) = message.get("log").and_then(Value::as_str) {
                    job.log(log);
                }
                if let Some(stage) = message.get("stage").and_then(Value::as_str) {
                    let percent =
                        message.get("percent").and_then(Value::as_f64).map(|p| from + (to - from) * p / 100.0);
                    let label = labels
                        .iter()
                        .find(|(id, _)| *id == stage)
                        .map(|(_, label)| label.to_string())
                        .unwrap_or_else(|| job.state().label.clone());
                    job.stage(stage, &label, percent);
                }
                if !message.contains_key("memoryMb") || message.len() > 1 {
                    *last_json = message;
                }
                return;
            }
            // Not JSON after all: a log line.
        }
        if let Some((stage, label)) = marked_stage(text) {
            let label = if label.is_empty() { job.state().label.clone() } else { label.to_string() };
            job.stage(stage, &label, None);
            return;
        }
        job.log(text);
    }
}

/// `@@stage <id> <label>`.
fn marked_stage(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("@@stage")?;
    let after = rest.trim_start();
    if after.len() == rest.len() {
        return None;
    }
    let end = after.find(char::is_whitespace).unwrap_or(after.len());
    let (stage, label) = after.split_at(end);
    (!stage.is_empty()).then(|| (stage, label.trim_start()))
}

/// A program's output, line by line (`\n`, `\r\n` or a lone `\r` — progress bars).
async fn read_lines(mut stream: impl AsyncRead + Unpin, lines: mpsc::UnboundedSender<String>) {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        buffer.extend_from_slice(&chunk[..read]);
        let mut start = 0;
        let mut index = 0;
        while index < buffer.len() {
            match buffer[index] {
                b'\n' => {
                    let _ = lines.send(String::from_utf8_lossy(&buffer[start..index]).into_owned());
                    start = index + 1;
                }
                b'\r' => {
                    if index + 1 == buffer.len() {
                        break; // a `\n` may follow in the next chunk
                    }
                    let _ = lines.send(String::from_utf8_lossy(&buffer[start..index]).into_owned());
                    start = if buffer[index + 1] == b'\n' { index + 2 } else { index + 1 };
                    index = start;
                    continue;
                }
                _ => {}
            }
            index += 1;
        }
        buffer.drain(..start);
    }
    let rest = String::from_utf8_lossy(&buffer).into_owned();
    let rest = rest.trim_end_matches('\r');
    if !rest.is_empty() {
        let _ = lines.send(rest.to_string());
    }
}

enum Signal {
    Term,
    Kill,
}

/// A signal to the program's whole group, or to the program alone when the group is gone.
fn signal_group(pid: u32, signal: Signal) {
    #[cfg(unix)]
    {
        let number = match signal {
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        };
        let Ok(pid) = i32::try_from(pid) else { return };
        // SAFETY: kill only sends a signal; the group is the one this job started.
        unsafe {
            if libc::kill(-pid, number) != 0 {
                libc::kill(pid, number);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, signal);
    }
}

/// Windows: the program and its children, at once.
pub(crate) fn taskkill(pid: u32) {
    let mut kill = std::process::Command::new("taskkill");
    kill.args(["/PID", &pid.to_string(), "/T", "/F"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        kill.creation_flags(super::platform::CREATE_NO_WINDOW);
    }
    if let Ok(mut child) = kill.spawn() {
        std::thread::spawn(move || child.wait());
    }
}

/// A limiter that refuses to run the program here (taskpolicy or systemd-run without the flags or the session it
/// needs) ends it at once: a program started under one that ends sooner than this is started once more as it is.
/// Python with torch takes longer than this to load, let alone to reach its memory limit.
pub const LIMITER_REFUSED_WITHIN: std::time::Duration = std::time::Duration::from_secs(3);

/// A heavy program under the kernel's own memory limit where the system has one: macOS `taskpolicy -m … -P kill` (it
/// killed a 2 GB process at exactly 1 GiB, 06.11; MPS memory a single allocation late) where taskpolicy takes it —
/// macOS 15 does not, and gets the lower priority alone —, Linux a systemd user scope with MemoryMax, both at a lower
/// CPU and disk priority. Without the kernel's limit the program's own guard (voicelab/limits.py) still holds. Windows: the Python process joins a Job Object itself
/// (clone-server/voicelab/limits.py). The kernel's limit is a little above the job's own, so the job's clear message
/// comes first.
pub fn with_limits(
    platform: &PlatformInfo,
    command: &str,
    args: &[String],
    limit_mb: Option<u64>,
) -> (String, Vec<String>) {
    let kernel_mb = limit_mb.filter(|mb| *mb > 0).map(|mb| js::round_i64(mb as f64 * 1.1 + 256.0));
    let strings = |items: &[&str]| items.iter().map(|item| item.to_string()).collect::<Vec<_>>();
    if platform.os == OsName::Darwin && platform.tools.taskpolicy != Taskpolicy::None {
        let mut wrapped = Vec::new();
        if let (Taskpolicy::Limits, Some(kernel_mb)) = (platform.tools.taskpolicy, kernel_mb) {
            wrapped.extend(["-m".to_string(), kernel_mb.to_string(), "-P".into(), "kill".into()]);
        }
        wrapped.extend(strings(&["-c", "utility", "-d", "throttle", command]));
        wrapped.extend(args.iter().cloned());
        return ("/usr/sbin/taskpolicy".into(), wrapped);
    }
    if platform.os == OsName::Linux {
        if let (true, Some(kernel_mb)) = (platform.tools.systemd_run, kernel_mb) {
            let mut wrapped = strings(&["--user", "--scope", "--quiet", "-p"]);
            wrapped.push(format!("MemoryMax={kernel_mb}M"));
            wrapped.extend(strings(&["-p", "MemorySwapMax=0", "--", "nice", "-n", "10", command]));
            wrapped.extend(args.iter().cloned());
            return ("systemd-run".into(), wrapped);
        }
        let mut wrapped = strings(&["-n", "10", command]);
        wrapped.extend(args.iter().cloned());
        return ("nice".into(), wrapped);
    }
    (command.to_string(), args.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice_lab::platform::{GpuInfo, GpuKind, Tools};
    use crate::voice_lab::system_memory::Pressure;

    fn calm() -> MemorySnapshot {
        MemorySnapshot {
            total_mb: 36_864,
            pool_mb: 20_000,
            pressure: Pressure::Normal,
            swap_in_mbps: None,
            vram_free_mb: None,
            at: 0,
        }
    }

    fn runner_with(sample: impl Fn() -> MemorySnapshot + Send + Sync + 'static) -> JobRunner {
        let sample = Arc::new(sample);
        JobRunner::new(JobDeps {
            sample_memory: Arc::new(move || {
                let sample = sample.clone();
                Box::pin(async move { sample() })
            }),
            watch_ms: 50,
            grace_ms: 300,
        })
    }

    fn runner() -> JobRunner {
        runner_with(calm)
    }

    /// A fake job: sh -c with a script, speaking as imprint.py does.
    #[cfg(unix)]
    fn sh(script: &str) -> (String, Vec<String>) {
        ("sh".into(), vec!["-c".into(), script.into()])
    }

    async fn finished(jobs: &JobRunner) -> JobState {
        for _ in 0..400 {
            if !jobs.busy() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        jobs.state().expect("a job ran")
    }

    #[cfg(unix)]
    fn object(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn follows_a_program_stages_into_the_job_progress_labels_the_log_its_memory_its_last_word() {
        let jobs = runner();
        let (command, args) = sh(r#"
            echo '{"stage":"select","percent":10}'
            echo 'Loading weights: 100%'
            echo '{"memoryMb":1234}'
            echo '{"stage":"samples","percent":60}'
            echo '{"stage":"done","percent":100,"peakMb":2955,"rtf":1.2}'
        "#);
        let (said, heard) = oneshot::channel();
        jobs.start(
            JobKind::Imprint,
            "omnivoice",
            move |context| async move {
                let options = RunOptions {
                    percent: Some((0.0, 50.0)),
                    labels: &[("select", "Выбираю"), ("samples", "Пробы")],
                    ..RunOptions::default()
                };
                let _ = said.send(context.run(&command, &args, options).await);
                Ok(Some(object(serde_json::json!({"voiceId": "v-1"}))))
            },
            None,
        )
        .unwrap();
        let state = finished(&jobs).await;
        assert_eq!(state.state, JobRunState::Done);
        assert_eq!(state.percent, 100);
        assert_eq!(state.peak_mb, Some(2955.0));
        assert_eq!(state.memory_mb, Some(1234.0));
        assert_eq!(state.result, Some(object(serde_json::json!({"voiceId": "v-1"}))));
        assert_eq!(state.log, vec!["Loading weights: 100%"]);
        let result = heard.await.unwrap().unwrap();
        assert_eq!(result.get("stage"), Some(&Value::from("done")));
        assert_eq!(result.get("rtf"), Some(&Value::from(1.2)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fails_with_what_the_program_said_and_refuses_a_second_job_while_one_runs() {
        let jobs = runner();
        let (command, args) = sh(r#"
            echo '{"stage":"words","percent":20}'
            echo '{"error":"распознавание не ответило","reason":"error"}'
            sleep 0.1
            exit 1
        "#);
        jobs.start(
            JobKind::Imprint,
            "omnivoice",
            move |context| async move { context.run(&command, &args, RunOptions::default()).await.map(Some) },
            None,
        )
        .unwrap();
        let second = jobs.start(JobKind::Install, "cosyvoice3", |_| async { Ok(None) }, None);
        assert!(second.is_err(), "one job at a time");
        let state = finished(&jobs).await;
        assert_eq!(state.state, JobRunState::Error);
        assert_eq!(state.reason, Some(JobReason::Error));
        assert_eq!(state.error.as_deref(), Some("распознавание не ответило"));
    }

    /// std::env::vars on a variable that is not Unicode.
    fn unreadable_env() -> HashMap<String, String> {
        panic!("the environment could not be read")
    }

    #[tokio::test]
    async fn ends_a_job_whose_work_panics_as_failed_and_takes_the_next_one() {
        let jobs = runner();
        let (said, heard) = oneshot::channel();
        jobs.start(
            JobKind::Imprint,
            "omnivoice",
            |context| async move {
                context.stage("prepare", "Готовлю записи", Some(1.0));
                let _env = unreadable_env();
                Ok(None)
            },
            Some(Box::new(move |state: &JobState| {
                let _ = said.send(state.clone());
            })),
        )
        .unwrap();
        let state = tokio::time::timeout(Duration::from_secs(5), heard).await.expect("the job ended").unwrap();
        assert_eq!((state.state, state.reason), (JobRunState::Error, Some(JobReason::Error)));
        assert_eq!(state.error.as_deref(), Some(PANICKED));
        assert!(state.finished_at.is_some());
        assert!(!jobs.busy());
        assert!(jobs.start(JobKind::Install, "omnivoice", |_| async { Ok(None) }, None).is_ok(), "not busy any more");
        assert_eq!(finished(&jobs).await.state, JobRunState::Done);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stops_on_the_owners_word_and_kills_a_program_that_ignores_the_polite_signal() {
        let jobs = runner();
        let (command, args) = sh(r#"
            trap '' TERM
            echo '{"stage":"load","percent":30}'
            while true; do sleep 1; done
        "#);
        jobs.start(
            JobKind::Imprint,
            "omnivoice",
            move |context| async move { context.run(&command, &args, RunOptions::default()).await.map(Some) },
            None,
        )
        .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(jobs.stop());
        let state = finished(&jobs).await;
        assert_eq!(state.state, JobRunState::Stopped);
        assert_eq!(state.reason, Some(JobReason::Stopped));
        assert_eq!(state.error.as_deref(), Some("Остановлено."));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stops_a_job_that_outgrows_its_ceiling_by_its_own_report() {
        let jobs = runner();
        let (command, args) = sh(r#"
            mb=1000
            while true; do mb=$((mb + 1000)); echo "{\"memoryMb\": $mb}"; sleep 0.02; done
        "#);
        jobs.start(
            JobKind::Imprint,
            "omnivoice",
            move |context| async move {
                let options = RunOptions { limit_mb: Some(4_000), ..RunOptions::default() };
                context.run(&command, &args, options).await.map(Some)
            },
            None,
        )
        .unwrap();
        let state = finished(&jobs).await;
        assert_eq!(state.state, JobRunState::Stopped);
        assert_eq!(state.reason, Some(JobReason::Memory));
        assert_eq!(state.limit_mb, Some(4_000));
        assert_eq!(
            state.error.as_deref(),
            Some("Остановлено: модель заняла больше 3,9 ГБ — намного больше обычного, похоже на сбой.")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stops_a_job_when_the_system_starts_to_run_out_whoever_is_to_blame() {
        let jobs =
            runner_with(|| MemorySnapshot { pressure: Pressure::Critical, at: crate::utils::time::now_ms(), ..calm() });
        let (command, args) = sh(r#"while true; do echo '{"memoryMb": 500}'; sleep 0.02; done"#);
        jobs.start(
            JobKind::Imprint,
            "omnivoice",
            move |context| async move {
                let options = RunOptions { limit_mb: Some(4_000), ..RunOptions::default() };
                context.run(&command, &args, options).await.map(Some)
            },
            None,
        )
        .unwrap();
        let state = finished(&jobs).await;
        assert_eq!(state.state, JobRunState::Stopped);
        assert_eq!(state.reason, Some(JobReason::Pressure));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn keeps_the_jobs_stdin_open_the_program_can_tie_its_life_to_the_gateways() {
        let jobs = runner();
        // A reader in the background holds stdin: alive after 200 ms only while nobody closed it.
        let (command, args) = sh(r#"
            exec 3<&0
            cat <&3 >/dev/null &
            reader=$!
            sleep 0.2
            if kill -0 $reader 2>/dev/null; then open=true; else open=false; fi
            kill $reader 2>/dev/null
            echo "{\"stage\":\"done\",\"open\":$open}"
            exit 0
        "#);
        let (said, heard) = oneshot::channel();
        jobs.start(
            JobKind::Imprint,
            "omnivoice",
            move |context| async move {
                let _ = said.send(context.run(&command, &args, RunOptions::default()).await);
                Ok(None)
            },
            None,
        )
        .unwrap();
        finished(&jobs).await;
        let result = heard.await.unwrap().unwrap();
        assert_eq!(result.get("open"), Some(&Value::Bool(true)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn keeps_the_work_a_program_reported_done_even_if_it_fell_over_on_its_way_out_06_11_exit_134_after_the_voice()
    {
        let jobs = runner();
        let (command, args) = sh(r#"echo '{"stage":"done","percent":100,"peakMb":3066}'; exit 134"#);
        jobs.start(
            JobKind::Imprint,
            "omnivoice",
            move |context| async move {
                context.run(&command, &args, RunOptions::default()).await?;
                Ok(Some(object(serde_json::json!({"voiceId": "v-1"}))))
            },
            None,
        )
        .unwrap();
        let state = finished(&jobs).await;
        assert_eq!(state.state, JobRunState::Done);
        assert_eq!(state.result, Some(object(serde_json::json!({"voiceId": "v-1"}))));
        assert_eq!(state.peak_mb, Some(3066.0));
        assert_eq!(state.log.last().map(String::as_str), Some("программа сделала дело и упала на выходе (код 134)"));
    }

    #[tokio::test]
    async fn names_a_missing_program() {
        let jobs = runner();
        jobs.start(
            JobKind::Install,
            "omnivoice",
            |context| async move { context.run("/no/such/uv", &["venv".into()], RunOptions::default()).await.map(Some) },
            None,
        )
        .unwrap();
        assert_eq!(finished(&jobs).await.error.as_deref(), Some("не найдена программа /no/such/uv"));
    }

    #[test]
    fn reads_stage_marks_and_strips_colours() {
        assert_eq!(marked_stage("@@stage load Загружаю модель"), Some(("load", "Загружаю модель")));
        assert_eq!(marked_stage("@@stage load"), Some(("load", "")));
        assert_eq!(marked_stage("@@stageload"), None);
        assert_eq!(strip_colours("\u{1b}[32mok\u{1b}[0m done"), "ok done");
    }

    fn base() -> PlatformInfo {
        PlatformInfo {
            os: OsName::Darwin,
            arch: "arm64".into(),
            ram_mb: 36_864,
            cpus: 14,
            cpu_name: None,
            docker: false,
            gpu: GpuInfo { kind: GpuKind::Apple, name: None, vram_mb: None, driver: None },
            disk_free_mb: 80_000,
            tools: Tools { uv: None, git: true, taskpolicy: Taskpolicy::Limits, systemd_run: false },
            checked_at: String::new(),
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn macos_the_kernel_kills_past_the_limit_a_little_above_the_jobs_own_at_utility_priority_disk_throttled() {
        assert_eq!(
            with_limits(&base(), "python", &strings(&["imprint.py"]), Some(4000)),
            (
                "/usr/sbin/taskpolicy".into(),
                strings(&["-m", "4656", "-P", "kill", "-c", "utility", "-d", "throttle", "python", "imprint.py"])
            )
        );
        // macOS 15: no `-m`/`-P` in its taskpolicy — the priority alone; none of ours at all — the program as it is.
        let sequoia = PlatformInfo { tools: Tools { taskpolicy: Taskpolicy::Priority, ..base().tools }, ..base() };
        assert_eq!(
            with_limits(&sequoia, "python", &strings(&["imprint.py"]), Some(4000)),
            ("/usr/sbin/taskpolicy".into(), strings(&["-c", "utility", "-d", "throttle", "python", "imprint.py"]))
        );
        let bare = PlatformInfo { tools: Tools { taskpolicy: Taskpolicy::None, ..base().tools }, ..base() };
        assert_eq!(
            with_limits(&bare, "python", &strings(&["imprint.py"]), Some(4000)),
            ("python".into(), strings(&["imprint.py"]))
        );
    }

    #[test]
    fn linux_a_systemd_user_scope_with_memorymax_and_no_swap_or_just_nice_without_systemd() {
        let linux = PlatformInfo {
            os: OsName::Linux,
            tools: Tools { taskpolicy: Taskpolicy::None, systemd_run: true, ..base().tools },
            ..base()
        };
        assert_eq!(
            with_limits(&linux, "python", &strings(&["x"]), Some(4000)),
            (
                "systemd-run".into(),
                strings(&[
                    "--user",
                    "--scope",
                    "--quiet",
                    "-p",
                    "MemoryMax=4656M",
                    "-p",
                    "MemorySwapMax=0",
                    "--",
                    "nice",
                    "-n",
                    "10",
                    "python",
                    "x"
                ])
            )
        );
        let plain = PlatformInfo { tools: Tools { systemd_run: false, ..linux.tools.clone() }, ..linux };
        assert_eq!(
            with_limits(&plain, "python", &strings(&["x"]), Some(4000)),
            ("nice".into(), strings(&["-n", "10", "python", "x"]))
        );
    }

    #[test]
    fn windows_as_it_is_the_process_joins_its_job_object_itself() {
        let windows = PlatformInfo { os: OsName::Win32, ..base() };
        assert_eq!(
            with_limits(&windows, "python.exe", &strings(&["x"]), Some(4000)),
            ("python.exe".into(), strings(&["x"]))
        );
    }
}
