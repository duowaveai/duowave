//! Claude Code, built in (docs/plans/router-and-works.md, block A, 7; decisions 7 and 19): its own program `claude`,
//! installed by the person and run as it is, never changed, its sign-in never offered nor read by the gateway.
//!
//! A run: `claude -p --output-format stream-json --verbose --forward-subagent-text --permission-mode acceptEdits
//! --permission-prompts none --allowedTools <the folder's rules> [--session-id <uuid> | --resume <uuid>] [--model]
//! [--max-budget-usd]`, the prompt on its input, in the work copy. Never `bypassPermissions` (a run that reports it is
//! ended at once); `--bare` is not set, so the folder's `CLAUDE.md`, skills and hooks are read. What needs a right not in
//! the folder's list is refused — `git push` too unless the folder lists it (decision 21).
//!
//! How it pays (`harness_claude_auth`): `terminal` — the sign-in the person made in Claude Code itself, a
//! subscription too; the gateway passes no key, and `ANTHROPIC_API_KEY` of its own environment never reaches the run.
//! `key` — the secret `harness:claude-code:key` goes to the run as `ANTHROPIC_API_KEY`. What a run paid by, the stream
//! says (`apiKeySource` of `system/init`). Whether it is signed in, the program says itself: `claude auth status`
//! (exit 0 — signed in, its `authMethod`).
//!
//! The stream (code.claude.com/docs, read 16.11.2026): `system/init`; `assistant` with `text`, `thinking`, `tool_use`;
//! `user` with `tool_result`; a subagent's messages with `parent_tool_use_id`; `system/api_retry`;
//! `system/permission_denied`; `rate_limit_event` (a plan's limit, `status: rejected`, `resetsAt`); at the end
//! `result` with `total_cost_usd`, `modelUsage` and `permission_denials`. SIGINT ends the turn with its result; SIGTERM
//! leaves it unfinished (exit 143).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

use super::{
    ask, cut, environment, find_program, version_in, version_text, Diff, Ended, Harness, ModelUse, NotOpened, NotReady,
    Open, Out, PlanEntry, Ready, RunEnd, Session, Step, ToolKind, ToolState, STOP_WAIT,
};
use crate::router::Tokens;
use crate::secrets::SecretsService;
use crate::settings::definitions::{HARNESS_CLAUDE_AUTH, HARNESS_CLAUDE_PATH};
use crate::settings::SettingsService;

/// Its id in the folders, the deals and the API.
pub const CLAUDE_CODE: &str = "claude-code";
/// The secret of «По ключу API».
pub const KEY_SECRET: &str = "harness:claude-code:key";
/// The oldest version with every flag a run gives: `--permission-prompts` came in 2.1.259, `--forward-subagent-text`
/// in 2.1.211, `--max-budget-usd` in 2.0.28.
pub const MIN_VERSION: [u32; 3] = [2, 1, 259];
/// How the person installs it, said when it is not found.
pub const INSTALL: &str = "curl -fsSL https://claude.ai/install.sh | bash";

/// How Claude Code pays for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    /// «Как в терминале»: its own sign-in.
    Terminal,
    /// «По ключу API»: the gateway's key.
    Key,
}

impl Auth {
    pub fn parse(text: &str) -> Option<Auth> {
        match text {
            "terminal" => Some(Auth::Terminal),
            "key" => Some(Auth::Key),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Auth::Terminal => "terminal",
            Auth::Key => "key",
        }
    }
}

pub struct ClaudeCode {
    settings: Arc<SettingsService>,
    secrets: Arc<SecretsService>,
}

impl ClaudeCode {
    pub fn new(settings: Arc<SettingsService>, secrets: Arc<SecretsService>) -> ClaudeCode {
        ClaudeCode { settings, secrets }
    }

    /// The program as set in the panel, else `claude` on the search path.
    pub fn program(&self) -> Option<PathBuf> {
        let set = self.settings.get(&HARNESS_CLAUDE_PATH, None).filter(|path| !path.trim().is_empty());
        find_program(set.as_deref().unwrap_or("claude"))
    }

    /// The way of paying chosen in the panel; None — not chosen yet («как в терминале» is used).
    pub fn auth_chosen(&self) -> Option<Auth> {
        self.settings.get(&HARNESS_CLAUDE_AUTH, None).as_deref().and_then(Auth::parse)
    }

    pub fn auth(&self) -> Auth {
        self.auth_chosen().unwrap_or(Auth::Terminal)
    }

    /// The program's environment for this way of paying: the key only with «по ключу».
    fn env(&self) -> Result<Vec<(String, String)>, NotReady> {
        match self.auth() {
            Auth::Terminal => Ok(environment(&[])),
            Auth::Key => {
                let key = self.secrets.get(KEY_SECRET).filter(|key| !key.trim().is_empty()).ok_or(NotReady::NoKey)?;
                Ok(environment(&[("ANTHROPIC_API_KEY".into(), key)]))
            }
        }
    }
}

/// How a sign-in pays, by `authMethod` of `claude auth status`.
fn pays_of(method: &str) -> Option<&'static str> {
    match method {
        "claude.ai" | "oauth_token" => Some("plan"),
        "api_key" | "api_key_helper" | "third_party" => Some("key"),
        _ => None,
    }
}

/// How a run pays, by `apiKeySource` of its `system/init`: a key, else the sign-in of claude.ai.
fn billing_of(source: &str) -> &'static str {
    match source {
        "ANTHROPIC_API_KEY" | "apiKeyHelper" | "/login managed key" => "key",
        _ => "plan",
    }
}

#[async_trait]
impl Harness for ClaudeCode {
    fn id(&self) -> String {
        CLAUDE_CODE.into()
    }

    fn title(&self) -> String {
        "Claude Code".into()
    }

    fn via(&self) -> String {
        format!("harness:{CLAUDE_CODE}")
    }

    async fn ready(&self) -> Ready {
        let Some(program) = self.program() else {
            return Ready::not(NotReady::Missing { install: Some(INSTALL.into()) });
        };
        let path = Some(program.display().to_string());
        let env = match self.env() {
            Ok(env) => env,
            Err(not) => return Ready { path, not: Some(not), ..Ready::not(NotReady::NoKey) },
        };
        let version = match ask(&program, &["--version"], &env).await {
            Some((text, Some(0))) => version_in(&text),
            _ => None,
        };
        let Some(version) = version else {
            return Ready { path, not: Some(NotReady::Unknown { detail: None }), ..Ready::not(NotReady::NoKey) };
        };
        let shown = version_text(&version);
        if version.as_slice() < MIN_VERSION.as_slice() {
            let not = NotReady::Old { version: shown.clone(), need: version_text(&MIN_VERSION) };
            return Ready { path, version: Some(shown), signed_in: None, pays: None, not: Some(not) };
        }
        // Signed in: the program says it itself; its tokens are never read here.
        let (signed_in, pays) = match ask(&program, &["auth", "status"], &env).await {
            Some((text, Some(0))) => {
                let method = serde_json::from_str::<Value>(text.trim())
                    .ok()
                    .and_then(|json| json.get("authMethod").and_then(Value::as_str).map(str::to_string));
                (Some(true), method.as_deref().and_then(pays_of).map(str::to_string))
            }
            Some((_, Some(1))) => (Some(false), None),
            _ => (None, None),
        };
        let pays = pays.or_else(|| (self.auth() == Auth::Key).then(|| "key".to_string()));
        let not = (signed_in == Some(false)).then_some(NotReady::SignedOut { hint: None });
        Ready { path, version: Some(shown), signed_in, pays, not }
    }

    async fn open(&self, open: Open) -> Result<Box<dyn Session>, NotOpened> {
        let program = self.program().ok_or_else(|| NotOpened::Failed(crate::tr!("программа агента не найдена")))?;
        let env = self.env().map_err(|_| NotOpened::Auth(crate::tr!("нет ключа API")))?;
        let (id, begun) = match open.session.clone() {
            Some(session) => (session, true),
            None => (uuid::Uuid::new_v4().to_string(), false),
        };
        Ok(Box::new(ClaudeSession { program, env, open, id, begun }))
    }

    fn resume_command(&self, session: &str, copy: &Path) -> Option<String> {
        let program = self.program().map(|path| path.display().to_string()).unwrap_or_else(|| "claude".into());
        Some(format!("cd {} && {} --resume {session}", quoted(&copy.display().to_string()), quoted(&program)))
    }
}

/// A path for a shell: as it is when it is plain, else in single quotes.
pub fn quoted(text: &str) -> String {
    if !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+=:@".contains(c)) {
        return text.to_string();
    }
    format!("'{}'", text.replace('\'', r"'\''"))
}

struct ClaudeSession {
    program: PathBuf,
    env: Vec<(String, String)>,
    open: Open,
    id: String,
    /// The session exists: the next run goes on in it (`--resume`).
    begun: bool,
}

impl ClaudeSession {
    fn args(&self) -> Vec<String> {
        let mut args: Vec<String> = [
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--forward-subagent-text",
            "--permission-mode",
            "acceptEdits",
            "--permission-prompts",
            "none",
        ]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
        if self.begun {
            args.extend(["--resume".into(), self.id.clone()]);
        } else {
            args.extend(["--session-id".into(), self.id.clone()]);
        }
        if let Some(model) = self.open.model.as_deref().filter(|model| !model.trim().is_empty()) {
            args.extend(["--model".into(), model.trim().to_string()]);
        }
        if let Some(usd) = self.open.max_usd.filter(|usd| *usd > 0.0) {
            args.extend(["--max-budget-usd".into(), format!("{usd:.2}")]);
        }
        let allow: Vec<&String> = self.open.allow.iter().filter(|rule| !rule.trim().is_empty()).collect();
        if !allow.is_empty() {
            // One value per rule, and an option after them: the list never takes in what follows.
            args.push("--allowedTools".into());
            args.extend(allow.into_iter().cloned());
        }
        args
    }
}

#[async_trait]
impl Session for ClaudeSession {
    fn id(&self) -> Option<String> {
        Some(self.id.clone())
    }

    async fn prompt(&mut self, text: &str, out: Arc<dyn Out>, stop: CancellationToken) -> RunEnd {
        let began = Instant::now();
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(self.args())
            .current_dir(&self.open.copy)
            .env_clear()
            .envs(self.env.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => return RunEnd::failed(format!("{}: {error}", self.program.display())),
        };
        // The prompt goes on its input: a text beginning with «-» is never taken for an option.
        if let Some(mut input) = child.stdin.take() {
            let prompt = text.to_string();
            tokio::spawn(async move {
                let _ = input.write_all(prompt.as_bytes()).await;
                let _ = input.shutdown().await;
            });
        }
        let errors = child.stderr.take().map(|mut stderr| {
            tokio::spawn(async move {
                let mut text = String::new();
                let _ = stderr.read_to_string(&mut text).await;
                text
            })
        });
        let Some(stdout) = child.stdout.take() else {
            let _ = child.start_kill();
            return RunEnd::failed("no pipe from the program");
        };
        let mut reader = Reader::new(out.clone(), text);
        let mut lines = BufReader::new(stdout).lines();
        let pid = child.id();
        let mut stopping: Option<tokio::time::Instant> = None;
        loop {
            let deadline = stopping.map(|at| at + STOP_WAIT);
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => {
                        if reader.line(&line) == Verdict::Forbidden {
                            // A mode that asks nothing at all: never run in it.
                            signal(pid, Signal::Kill);
                            break;
                        }
                    }
                    _ => break,
                },
                _ = stop.cancelled(), if stopping.is_none() => {
                    stopping = Some(tokio::time::Instant::now());
                    signal(pid, Signal::Interrupt);
                }
                _ = async { tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)).await }, if deadline.is_some() => {
                    signal(pid, Signal::Terminate);
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    signal(pid, Signal::Kill);
                    break;
                }
            }
        }
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        if status.is_err() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        let stderr = match errors {
            Some(task) => tokio::time::timeout(Duration::from_secs(2), task).await.ok().and_then(Result::ok),
            None => None,
        };
        self.begun = true;
        let mut end = reader.end(stop.is_cancelled(), stderr.as_deref());
        if let Some(session) = end.session.clone() {
            self.id = session;
        }
        end.session.get_or_insert_with(|| self.id.clone());
        end.ms = began.elapsed().as_millis() as u64;
        end
    }

    async fn close(self: Box<Self>) {}
}

#[derive(Clone, Copy)]
enum Signal {
    Interrupt,
    Terminate,
    Kill,
}

/// SIGINT to the program (it ends its turn); SIGTERM and SIGKILL to its whole group (its commands too).
fn signal(pid: Option<u32>, signal: Signal) {
    let Some(pid) = pid.and_then(|pid| i32::try_from(pid).ok()) else { return };
    #[cfg(unix)]
    // SAFETY: kill/killpg have no memory effects; a process or group that is gone answers ESRCH, which is fine here.
    unsafe {
        match signal {
            Signal::Interrupt => {
                libc::kill(pid, libc::SIGINT);
            }
            Signal::Terminate => {
                libc::killpg(pid, libc::SIGTERM);
            }
            Signal::Kill => {
                libc::killpg(pid, libc::SIGKILL);
            }
        }
    }
    // Windows has no SIGINT for a program without our console: a stop ends it and what it started at once (its turn
    // without its result — the work is stopped all the same). Until 21.11 nothing was sent: a stop waited STOP_WAIT
    // and killed claude alone, its commands left running.
    #[cfg(windows)]
    {
        let _ = signal;
        crate::utils::process::kill_tree(pid as u32);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pid, signal);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Fine,
    /// The run reports a mode where nothing is asked (`bypassPermissions`): it must not go on.
    Forbidden,
}

/// A tool call as it was asked: its name, kind, title, file and command — its result fills in the rest.
#[derive(Debug, Clone)]
struct Called {
    kind: ToolKind,
    title: String,
    path: Option<String>,
    command: Option<String>,
    diff: Vec<Diff>,
}

/// The stream read line by line into the common steps, and what its end needs.
struct Reader {
    out: Arc<dyn Out>,
    prompt: String,
    called: HashMap<String, Called>,
    denied: std::collections::HashSet<String>,
    session: Option<String>,
    model: Option<String>,
    billing: Option<String>,
    /// The last error an answer of the model carried (`rate_limit`, `billing_error`…).
    error: Option<String>,
    /// A plan's limit refused the run: until when.
    limit: Option<Option<u64>>,
    result: Option<Value>,
    forbidden: bool,
    last_text: Option<String>,
}

impl Reader {
    fn new(out: Arc<dyn Out>, prompt: &str) -> Reader {
        Reader {
            out,
            prompt: prompt.to_string(),
            called: HashMap::new(),
            denied: std::collections::HashSet::new(),
            session: None,
            model: None,
            billing: None,
            error: None,
            limit: None,
            result: None,
            forbidden: false,
            last_text: None,
        }
    }

    fn line(&mut self, line: &str) -> Verdict {
        let line = line.trim();
        if line.is_empty() {
            return Verdict::Fine;
        }
        self.out.raw(line);
        let Ok(message) = serde_json::from_str::<Value>(line) else { return Verdict::Fine };
        let parent = message.get("parent_tool_use_id").and_then(Value::as_str).map(str::to_string);
        let parent = parent.as_deref();
        let text = |key: &str| message.get(key).and_then(Value::as_str).map(str::to_string);
        match (message.get("type").and_then(Value::as_str), message.get("subtype").and_then(Value::as_str)) {
            (Some("system"), Some("init")) => {
                self.session = text("session_id");
                self.model = text("model");
                self.billing = text("apiKeySource").map(|source| billing_of(&source).to_string());
                if text("permissionMode").as_deref() == Some("bypassPermissions") {
                    self.forbidden = true;
                }
                self.out.step(
                    None,
                    Step::Begin {
                        session: self.session.clone(),
                        model: self.model.clone(),
                        billing: self.billing.clone(),
                        version: text("claude_code_version"),
                        prompt: Some(cut(&self.prompt)),
                    },
                );
                if self.forbidden {
                    return Verdict::Forbidden;
                }
            }
            (Some("system"), Some("api_retry")) => {
                let reason = text("error").unwrap_or_else(|| "unknown".into());
                self.out.step(
                    parent,
                    Step::Retry {
                        reason,
                        attempt: message.get("attempt").and_then(Value::as_u64).map(|n| n as u32),
                        wait_ms: message.get("retry_delay_ms").and_then(Value::as_u64),
                    },
                );
            }
            (Some("system"), Some("permission_denied")) => {
                let tool = text("tool_name").unwrap_or_default();
                if let Some(id) = text("tool_use_id") {
                    self.denied.insert(id);
                }
                let what = text("message").unwrap_or_default();
                self.out.step(parent, Step::Denied { tool, what: cut(&what) });
            }
            (Some("rate_limit_event"), _) => {
                let info = message.get("rate_limit_info");
                let status = info.and_then(|info| info.get("status")).and_then(Value::as_str);
                if status == Some("rejected") {
                    let until = info.and_then(|info| info.get("resetsAt")).and_then(Value::as_f64).map(epoch_ms);
                    self.limit = Some(until);
                    self.out.step(parent, Step::Limit { until });
                }
            }
            (Some("assistant"), _) => self.assistant(&message, parent),
            (Some("user"), _) => self.results(&message, parent),
            (Some("result"), _) => self.result = Some(message.clone()),
            _ => {}
        }
        Verdict::Fine
    }

    fn assistant(&mut self, message: &Value, parent: Option<&str>) {
        if let Some(error) = message.get("error").and_then(Value::as_str) {
            self.error = Some(error.to_string());
        }
        let Some(blocks) = message.pointer("/message/content").and_then(Value::as_array) else { return };
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let text = block.get("text").and_then(Value::as_str).unwrap_or_default();
                    if !text.trim().is_empty() {
                        if parent.is_none() {
                            self.last_text = Some(text.to_string());
                        }
                        self.out.step(parent, Step::Says { text: cut(text) });
                    }
                }
                Some("thinking") => {
                    let text = block.get("thinking").and_then(Value::as_str).unwrap_or_default();
                    if !text.trim().is_empty() {
                        self.out.step(parent, Step::Thinks { text: cut(text) });
                    }
                }
                Some("tool_use") => self.tool_use(block, parent),
                _ => {}
            }
        }
    }

    fn tool_use(&mut self, block: &Value, parent: Option<&str>) {
        let id = block.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let name = block.get("name").and_then(Value::as_str).unwrap_or_default();
        let input = block.get("input").cloned().unwrap_or(Value::Null);
        let field = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_string);
        // The list of things to do is the agent's plan.
        if name == "TodoWrite" {
            let entries = input
                .get("todos")
                .and_then(Value::as_array)
                .map(|todos| {
                    todos
                        .iter()
                        .map(|todo| PlanEntry {
                            text: todo.get("content").and_then(Value::as_str).unwrap_or_default().to_string(),
                            state: todo.get("status").and_then(Value::as_str).unwrap_or("pending").to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            self.out.step(parent, Step::Plan { entries });
            self.called.insert(
                id,
                Called { kind: ToolKind::Think, title: name.into(), path: None, command: None, diff: Vec::new() },
            );
            return;
        }
        let path = field("file_path").or_else(|| field("notebook_path")).or_else(|| field("path"));
        let (kind, title, command, diff) = match name {
            "Read" => (ToolKind::Read, path.clone().unwrap_or_default(), None, Vec::new()),
            "Write" => {
                let diff =
                    path.clone().map(|path| Diff { path, old: None, new: cut(&field("content").unwrap_or_default()) });
                (ToolKind::Edit, path.clone().unwrap_or_default(), None, diff.into_iter().collect())
            }
            "Edit" => {
                let diff = path.clone().map(|path| Diff {
                    path,
                    old: field("old_string").map(|old| cut(&old)),
                    new: cut(&field("new_string").unwrap_or_default()),
                });
                (ToolKind::Edit, path.clone().unwrap_or_default(), None, diff.into_iter().collect())
            }
            "MultiEdit" => {
                let edits = input.get("edits").and_then(Value::as_array).cloned().unwrap_or_default();
                let diff = edits
                    .iter()
                    .filter_map(|edit| {
                        Some(Diff {
                            path: path.clone()?,
                            old: edit.get("old_string").and_then(Value::as_str).map(cut),
                            new: cut(edit.get("new_string").and_then(Value::as_str).unwrap_or_default()),
                        })
                    })
                    .collect();
                (ToolKind::Edit, path.clone().unwrap_or_default(), None, diff)
            }
            "NotebookEdit" => (ToolKind::Edit, path.clone().unwrap_or_default(), None, Vec::new()),
            "Bash" | "BashOutput" | "KillShell" | "KillBash" => {
                let command = field("command");
                let title = field("description").or_else(|| command.clone()).unwrap_or_else(|| name.to_string());
                (ToolKind::Execute, title, command, Vec::new())
            }
            "Glob" | "Grep" | "LS" => {
                let pattern = field("pattern").or_else(|| path.clone()).unwrap_or_default();
                (ToolKind::Search, pattern, None, Vec::new())
            }
            "WebFetch" | "WebSearch" => {
                (ToolKind::Fetch, field("url").or_else(|| field("query")).unwrap_or_default(), None, Vec::new())
            }
            "Task" | "Agent" => {
                let title = field("description").unwrap_or_else(|| name.to_string());
                (ToolKind::Other, title, None, Vec::new())
            }
            "ExitPlanMode" | "EnterPlanMode" => (ToolKind::SwitchMode, name.to_string(), None, Vec::new()),
            other => (ToolKind::Other, other.to_string(), None, Vec::new()),
        };
        let called = Called { kind, title: cut(&title), path: path.clone(), command: command.map(|c| cut(&c)), diff };
        self.out.step(
            parent,
            Step::Tool {
                id: id.clone(),
                kind: called.kind,
                title: called.title.clone(),
                path: called.path.clone(),
                command: called.command.clone(),
                output: None,
                diff: called.diff.clone(),
                state: ToolState::InProgress,
            },
        );
        self.called.insert(id, called);
    }

    fn results(&mut self, message: &Value, parent: Option<&str>) {
        let Some(blocks) = message.pointer("/message/content").and_then(Value::as_array) else { return };
        for block in blocks.iter().filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result")) {
            let id = block.get("tool_use_id").and_then(Value::as_str).unwrap_or_default().to_string();
            let Some(called) = self.called.get(&id).cloned() else { continue };
            if called.kind == ToolKind::Think && called.title == "TodoWrite" {
                continue;
            }
            let failed = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            let output = match block.get("content") {
                Some(Value::String(text)) => text.clone(),
                Some(Value::Array(parts)) => parts
                    .iter()
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            self.out.step(
                parent,
                Step::Tool {
                    id,
                    kind: called.kind,
                    title: called.title,
                    path: called.path,
                    command: called.command,
                    output: Some(cut(&output)).filter(|output| !output.is_empty()),
                    diff: Vec::new(),
                    state: if failed { ToolState::Failed } else { ToolState::Completed },
                },
            );
        }
    }

    /// The run's end, from its `result` — or, without one, from what is known: stopped, or failed with the program's
    /// words.
    fn end(&mut self, stopped: bool, stderr: Option<&str>) -> RunEnd {
        let base = RunEnd {
            session: self.session.clone(),
            model: self.model.clone(),
            billing: self.billing.clone(),
            ..RunEnd::of(Ended::Ok)
        };
        if self.forbidden {
            let end = RunEnd { error: Some("permission mode bypassPermissions is never used".into()), ..base };
            return self.finish(RunEnd { ended: Ended::Failed, ..end });
        }
        let Some(result) = self.result.clone() else {
            let error = stderr.map(str::trim).filter(|text| !text.is_empty()).map(|text| cut_tail(text, 600));
            let ended = if stopped {
                Ended::Stopped
            } else if let Some(until) = self.limit {
                Ended::Limit { until }
            } else {
                self.error_kind().unwrap_or(Ended::Failed)
            };
            return self.finish(RunEnd { ended, error, ..base });
        };
        let subtype = result.get("subtype").and_then(Value::as_str).unwrap_or_default();
        let is_error = result.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        let text = result.get("result").and_then(Value::as_str).map(str::to_string);
        let errors: Vec<String> = result
            .get("errors")
            .and_then(Value::as_array)
            .map(|errors| errors.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let usd_micro =
            result.get("total_cost_usd").and_then(Value::as_f64).map(|usd| (usd * 1_000_000.0).round() as u64);
        let models: Vec<ModelUse> = result
            .get("modelUsage")
            .and_then(Value::as_object)
            .map(|models| {
                models
                    .iter()
                    .map(|(model, used)| {
                        let number = |key: &str| used.get(key).and_then(Value::as_u64).unwrap_or(0);
                        ModelUse {
                            model: model.clone(),
                            tokens: Tokens {
                                input: number("inputTokens"),
                                output: number("outputTokens"),
                                cache_read: number("cacheReadInputTokens"),
                                cache_write: number("cacheCreationInputTokens"),
                            },
                            usd_micro: used
                                .get("costUSD")
                                .and_then(Value::as_f64)
                                .map(|usd| (usd * 1_000_000.0).round() as u64),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        // What was refused and never told by its own event.
        if let Some(denials) = result.get("permission_denials").and_then(Value::as_array) {
            for denial in denials {
                let id = denial.get("tool_use_id").and_then(Value::as_str).unwrap_or_default();
                if self.denied.contains(id) {
                    continue;
                }
                let tool = denial.get("tool_name").and_then(Value::as_str).unwrap_or_default().to_string();
                let input = denial.get("tool_input");
                let what = input
                    .and_then(|input| input.get("command").or_else(|| input.get("file_path")))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.out.step(None, Step::Denied { tool, what: cut(what) });
            }
        }
        let ended = match subtype {
            _ if stopped => Ended::Stopped,
            "error_max_budget_usd" => Ended::Budget,
            "error_max_turns" => Ended::Turns,
            "success" if !is_error => Ended::Ok,
            _ => match self.limit {
                Some(until) => Ended::Limit { until },
                None => self.error_kind().unwrap_or(Ended::Failed),
            },
        };
        let error = (ended != Ended::Ok).then(|| {
            let words = if errors.is_empty() { text.clone().unwrap_or_default() } else { errors.join("; ") };
            cut_tail(&words, 600)
        });
        let session = result.get("session_id").and_then(Value::as_str).map(str::to_string).or(base.session.clone());
        let model = base.model.clone().or_else(|| models.first().map(|used| used.model.clone()));
        let text = text.or_else(|| self.last_text.clone());
        self.finish(RunEnd {
            ended,
            text,
            error: error.filter(|e| !e.is_empty()),
            session,
            model,
            usd_micro,
            models,
            ..base
        })
    }

    /// The model's error said in the answers, as an end.
    fn error_kind(&self) -> Option<Ended> {
        match self.error.as_deref()? {
            "rate_limit" | "overloaded" => Some(Ended::Busy),
            "billing_error" => Some(Ended::Billing),
            "authentication_failed" | "oauth_org_not_allowed" | "account_on_hold" | "verification_required" => {
                Some(Ended::Auth)
            }
            _ => None,
        }
    }

    fn finish(&self, end: RunEnd) -> RunEnd {
        self.out.step(
            None,
            Step::End {
                outcome: end.ended.as_str().into(),
                text: end.text.as_deref().map(cut),
                error: end.error.clone(),
                usd_micro: end.usd_micro,
            },
        );
        end
    }
}

/// The end of a text, at most `limit` bytes: an error's last words say most.
fn cut_tail(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.len() <= limit {
        return text.to_string();
    }
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

/// `resetsAt` — seconds or milliseconds since the epoch — in ms.
fn epoch_ms(value: f64) -> u64 {
    if value < 1e12 {
        (value * 1000.0) as u64
    } else {
        value as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Kept {
        raw: Mutex<Vec<String>>,
        steps: Mutex<Vec<(Option<String>, Step)>>,
    }

    impl Out for Kept {
        fn raw(&self, line: &str) {
            self.raw.lock().unwrap().push(line.to_string());
        }
        fn step(&self, parent: Option<&str>, step: Step) {
            self.steps.lock().unwrap().push((parent.map(str::to_string), step));
        }
    }

    fn read(lines: &[Value]) -> (Arc<Kept>, RunEnd) {
        let kept = Arc::new(Kept::default());
        let mut reader = Reader::new(kept.clone(), "сделай страницу");
        for line in lines {
            reader.line(&line.to_string());
        }
        let end = reader.end(false, None);
        (kept, end)
    }

    #[test]
    fn reads_the_stream_into_common_steps_with_the_subagents_nested() {
        let (kept, end) = read(&[
            serde_json::json!({"type":"system","subtype":"init","session_id":"s-1","model":"claude-sonnet-5-5","apiKeySource":"none","permissionMode":"acceptEdits","claude_code_version":"2.1.302"}),
            serde_json::json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"thinking","thinking":"Нужна форма."},{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"src/form.ts","old_string":"a","new_string":"b"}}]}}),
            serde_json::json!({"type":"user","parent_tool_use_id":null,"message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}),
            serde_json::json!({"type":"assistant","parent_tool_use_id":"t9","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"npm test","description":"Run tests"}}]}}),
            serde_json::json!({"type":"user","parent_tool_use_id":"t9","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":[{"type":"text","text":"1 failed"}],"is_error":true}]}}),
            serde_json::json!({"type":"system","subtype":"permission_denied","tool_name":"Bash","tool_use_id":"t3","message":"git push needs approval"}),
            serde_json::json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"text","text":"Готово: страница контактов."}]}}),
            serde_json::json!({"type":"result","subtype":"success","is_error":false,"result":"Готово: страница контактов.","session_id":"s-1","total_cost_usd":1.2,"modelUsage":{"claude-sonnet-5-5":{"inputTokens":10,"outputTokens":20,"cacheReadInputTokens":30,"cacheCreationInputTokens":40,"costUSD":1.2}},"permission_denials":[{"tool_name":"Bash","tool_use_id":"t3","tool_input":{"command":"git push"}}]}),
        ]);
        assert_eq!(end.ended, Ended::Ok);
        assert_eq!(end.usd_micro, Some(1_200_000));
        assert_eq!(end.billing.as_deref(), Some("plan"));
        assert_eq!(end.session.as_deref(), Some("s-1"));
        assert_eq!(end.tokens(), Tokens { input: 10, output: 20, cache_read: 30, cache_write: 40 });
        assert_eq!(end.text.as_deref(), Some("Готово: страница контактов."));
        let steps = kept.steps.lock().unwrap();
        let denied = steps.iter().filter(|(_, step)| matches!(step, Step::Denied { .. })).count();
        assert_eq!(denied, 1, "a refusal told by its event is not told twice");
        let nested: Vec<_> = steps.iter().filter(|(parent, _)| parent.as_deref() == Some("t9")).collect();
        assert_eq!(nested.len(), 2, "the subagent's call and its result carry the call that started it");
        assert!(
            matches!(&nested[1].1, Step::Tool { state: ToolState::Failed, output: Some(output), .. } if output == "1 failed")
        );
        assert!(
            matches!(&steps[2].1, Step::Tool { kind: ToolKind::Edit, diff, .. } if diff[0].old.as_deref() == Some("a"))
        );
        assert_eq!(kept.raw.lock().unwrap().len(), 8);
    }

    #[test]
    fn a_plans_limit_a_key_and_a_forbidden_mode_are_told_apart() {
        let (_, end) = read(&[
            serde_json::json!({"type":"system","subtype":"init","session_id":"s","apiKeySource":"none","permissionMode":"acceptEdits"}),
            serde_json::json!({"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1_800_000_000u64,"rateLimitType":"five_hour"}}),
            serde_json::json!({"type":"result","subtype":"success","is_error":true,"result":"You've hit your session limit · resets 6pm","session_id":"s"}),
        ]);
        assert_eq!(end.ended, Ended::Limit { until: Some(1_800_000_000_000) });
        let (_, end) = read(&[
            serde_json::json!({"type":"system","subtype":"init","session_id":"s","apiKeySource":"ANTHROPIC_API_KEY"}),
            serde_json::json!({"type":"assistant","error":"billing_error","message":{"content":[]}}),
            serde_json::json!({"type":"result","subtype":"success","is_error":true,"result":"Credit balance is too low"}),
        ]);
        assert_eq!((end.ended, end.billing.as_deref()), (Ended::Billing, Some("key")));
        let kept = Arc::new(Kept::default());
        let mut reader = Reader::new(kept, "x");
        let verdict = reader.line(
            &serde_json::json!({"type":"system","subtype":"init","permissionMode":"bypassPermissions"}).to_string(),
        );
        assert_eq!(verdict, Verdict::Forbidden);
        assert_eq!(reader.end(false, None).ended, Ended::Failed);
    }

    #[test]
    fn a_path_is_quoted_for_the_shell_only_when_it_must_be() {
        assert_eq!(quoted("/data/works/7/copy"), "/data/works/7/copy");
        assert_eq!(quoted("/Users/user/My Site"), "'/Users/user/My Site'");
        assert_eq!(quoted("it's"), r"'it'\''s'");
    }
}
