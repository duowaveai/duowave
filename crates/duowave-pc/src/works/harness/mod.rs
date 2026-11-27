//! The socket (docs/plans/router-and-works.md, block A, 7; decision 19): one contract for every agent that reads and
//! changes files by itself. The rest of DuoWave — the run of a work, the page «Работы», «Расход» — knows only this
//! contract: what the agent is, whether it is ready, a session in a work copy, a prompt into it with its steps and its
//! end, a stop. Claude Code is built in (`claude_code.rs`, its own program `claude -p`); the agents of the Agent
//! Client Protocol come as cards (`acp.rs`, stage 3б) through the same contract.
//!
//! The steps are common to all agents (`Step`): what it says, what it thinks, each tool call — what it reads, changes,
//! runs, and what came of it — its plan, its spending, its retries, what was not allowed, the questions about rights.
//! A step of an agent's helper (a subagent) carries the id of the call that started it. Everything the agent printed
//! goes to the raw journal as it came (`Out::raw`).

pub mod acp;
pub mod claude_code;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::router::Tokens;

/// What a tool call does: the Agent Client Protocol's kinds, which Claude Code's tools map onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

impl ToolKind {
    pub fn parse(text: &str) -> ToolKind {
        match text {
            "read" => ToolKind::Read,
            "edit" => ToolKind::Edit,
            "delete" => ToolKind::Delete,
            "move" => ToolKind::Move,
            "search" => ToolKind::Search,
            "execute" => ToolKind::Execute,
            "think" => ToolKind::Think,
            "fetch" => ToolKind::Fetch,
            "switch_mode" => ToolKind::SwitchMode,
            _ => ToolKind::Other,
        }
    }
}

/// Where a tool call is: asked, going, done, failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolState {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl ToolState {
    pub fn parse(text: &str) -> ToolState {
        match text {
            "in_progress" => ToolState::InProgress,
            "completed" => ToolState::Completed,
            "failed" => ToolState::Failed,
            _ => ToolState::Pending,
        }
    }
}

/// A change of a file: «было / стало». No `old` — a new file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diff {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<String>,
    pub new: String,
}

/// An item of the agent's plan: its words and where it is (`pending`, `in_progress`, `completed`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntry {
    pub text: String,
    pub state: String,
}

/// The gateway's own steps around the agent's run: preparing the copy, the commit it made, the branch fetched into the
/// folder, a check of the folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Chore {
    Copy,
    Prepare,
    Commit,
    Fetch,
    Check,
}

/// One step of a run, common to every agent (docs/plans/router-and-works.md, 7). Long texts are cut in the steps; the
/// raw journal keeps them whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Step {
    /// The run began: the agent's session, its model, how it pays (`plan`, `key`, `agent`), its version, the protocol
    /// and its version agreed with it (`ACP 2`).
    Begin {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        billing: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        protocol: Option<String>,
        /// What it was asked, as given to the agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
    },
    /// What the agent writes.
    Says { text: String },
    /// What it thinks, when it gives it (for some models a retelling, not word for word).
    Thinks { text: String },
    /// A tool call: what it does and on what, what came of it. The same `id` again — the call went on.
    Tool {
        id: String,
        kind: ToolKind,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        diff: Vec<Diff>,
        state: ToolState,
    },
    /// The agent's plan, whole each time.
    Plan { entries: Vec<PlanEntry> },
    /// What it spent so far, as it said: tokens, dollars (its own estimate), the context used of its size.
    Usage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tokens: Option<Tokens>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usd_micro: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        used: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    },
    /// It tries again: why (`rate_limit`, `overloaded`…), which time, how long it waits.
    Retry {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait_ms: Option<u64>,
    },
    /// A plan's limit holds it: until when, if said.
    Limit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<u64>,
    },
    /// What it was not allowed to do.
    Denied { tool: String, what: String },
    /// It asked whether it may, and the socket answered by the folder's allowed commands.
    Asks { tool: String, what: String, allowed: bool },
    /// The gateway's own step: what, the command, whether it went well, what it printed.
    Chore {
        chore: Chore,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// The journal reached its limit: from here only the end is written.
    Cut,
    /// The run ended: how, the agent's last words, the dollars it said, how many of its messages were not understood
    /// (an update of a kind this gateway does not know): many of them — its plug is due an update.
    End {
        outcome: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usd_micro: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unknown: Option<u32>,
    },
}

impl Step {
    /// A failure worth «только ошибки»: a call that failed, a refusal, a retry, a limit, an end that went wrong.
    pub fn is_trouble(&self) -> bool {
        match self {
            Step::Tool { state, .. } => *state == ToolState::Failed,
            Step::Denied { .. } | Step::Retry { .. } | Step::Limit { .. } => true,
            Step::Asks { allowed, .. } => !allowed,
            Step::Chore { ok, .. } => !ok,
            Step::End { outcome, .. } => outcome != "ok",
            _ => false,
        }
    }
}

/// The longest text a step keeps of one field; the raw journal has it whole.
pub const STEP_TEXT: usize = 16 * 1024;

/// A text at most `STEP_TEXT` bytes, cut at a char with «…».
pub fn cut(text: &str) -> String {
    cut_to(text, STEP_TEXT)
}

pub fn cut_to(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Where a run's journal goes: what the agent printed, as it came, and the common steps. `parent` — the call of the
/// agent's helper the step belongs to.
pub trait Out: Send + Sync {
    fn raw(&self, line: &str);
    fn step(&self, parent: Option<&str>, step: Step);
}

/// Why an agent is not ready, for the panel's words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "why", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum NotReady {
    /// The program is not found: where it was looked for, and how to install it.
    Missing {
        #[serde(skip_serializing_if = "Option::is_none")]
        install: Option<String>,
    },
    /// Its version is older than the flags need.
    Old { version: String, need: String },
    /// Its version could not be read, or the protocol it speaks is not one we know.
    Unknown {
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// Not signed in: open it in the terminal and sign in.
    SignedOut {
        #[serde(skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
    /// «По ключу API» is chosen and there is no key.
    NoKey,
    /// The card was removed, or there is no such agent.
    Gone,
}

/// Whether an agent is ready to work: found where, which version, signed in, and why not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ready {
    pub path: Option<String>,
    pub version: Option<String>,
    /// None — not checked (no way to check it without running the agent).
    pub signed_in: Option<bool>,
    /// How it would pay, as the agent says of its sign-in: `plan`, `key`, `agent`.
    pub pays: Option<String>,
    pub not: Option<NotReady>,
}

impl Ready {
    pub fn ok(&self) -> bool {
        self.not.is_none()
    }

    pub fn not(not: NotReady) -> Ready {
        Ready { path: None, version: None, signed_in: None, pays: None, not: Some(not) }
    }
}

/// Why a session did not open: the agent is not signed in; it speaks another version of the protocol; anything else
/// — in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotOpened {
    Auth(String),
    Protocol(String),
    Failed(String),
}

/// A program asked a question (its version, its sign-in): what it printed and its exit code; None — it did not start
/// or took too long.
pub async fn ask(program: &Path, args: &[&str], env: &[(String, String)]) -> Option<(String, Option<i32>)> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .env_clear()
        .envs(env.iter().cloned())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(ASK_WAIT, command.output()).await.ok()?.ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if text.trim().is_empty() {
        text = String::from_utf8_lossy(&output.stderr).into_owned();
    }
    Some((text, output.status.code()))
}

/// How long a question to a program itself (its version, its sign-in) may take.
const ASK_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// A session to open in a work copy: the model asked (none — the agent's own), the ceiling of one run, the folder's
/// allowed commands, the session to go on with (a later run), the agent's environment.
#[derive(Debug, Clone)]
pub struct Open {
    pub copy: PathBuf,
    pub model: Option<String>,
    pub max_usd: Option<f64>,
    pub allow: Vec<String>,
    /// The session of an earlier run: go on in it.
    pub session: Option<String>,
    /// When the agent cannot go on in a session (no `session/load`), this is said to its new one: the task and the
    /// last result.
    pub recap: Option<String>,
    /// A session that changes nothing (the plan, the review of stage 6): Claude Code in its mode `plan`, an ACP agent
    /// refused every change.
    pub read_only: bool,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    Ok,
    /// It failed: the agent's error.
    Failed,
    /// A plan's limit (until when, if said) — the run waits for it.
    Limit {
        until: Option<u64>,
    },
    /// A service's limit a short wait helps: try again in a minute.
    Busy,
    /// The key has no money left.
    Billing,
    /// Not signed in, or the key was refused.
    Auth,
    /// The ceiling of one run was reached.
    Budget,
    /// Stopped by the person.
    Stopped,
    /// The model refused.
    Refused,
    /// Its turns or its tokens ran out.
    Turns,
}

impl Ended {
    /// The word of the run's record and its deal.
    pub fn as_str(&self) -> &'static str {
        match self {
            Ended::Ok => "ok",
            Ended::Failed | Ended::Refused | Ended::Auth | Ended::Billing => "failed",
            Ended::Limit { .. } | Ended::Busy => "limit",
            Ended::Budget => "budget",
            Ended::Stopped => "stopped",
            Ended::Turns => "cut",
        }
    }
}

/// What one model of a run spent, as the agent reported it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelUse {
    pub model: String,
    pub tokens: Tokens,
    pub usd_micro: Option<u64>,
}

/// The end of a run: how, the agent's last words, its session, how it paid, the dollars it said (its own estimate),
/// what each model spent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEnd {
    pub ended: Ended,
    pub text: Option<String>,
    pub error: Option<String>,
    pub session: Option<String>,
    /// The model of the run (the main one).
    pub model: Option<String>,
    /// `plan`, `key` or `agent`.
    pub billing: Option<String>,
    pub usd_micro: Option<u64>,
    pub models: Vec<ModelUse>,
    pub ms: u64,
}

impl RunEnd {
    pub fn of(ended: Ended) -> RunEnd {
        RunEnd {
            ended,
            text: None,
            error: None,
            session: None,
            model: None,
            billing: None,
            usd_micro: None,
            models: Vec::new(),
            ms: 0,
        }
    }

    pub fn failed(error: impl Into<String>) -> RunEnd {
        RunEnd { error: Some(error.into()), ..RunEnd::of(Ended::Failed) }
    }

    /// All the tokens the run's models spent.
    pub fn tokens(&self) -> Tokens {
        let mut sum = Tokens::default();
        for used in &self.models {
            sum.add(used.tokens);
        }
        sum
    }
}

/// An agent: Claude Code built in, or one of a card. Its program is the person's own, installed by them.
#[async_trait]
pub trait Harness: Send + Sync {
    /// `claude-code`, or the card's id.
    fn id(&self) -> String;
    /// Its name as people see it: «Claude Code», «Factory Droid».
    fn title(&self) -> String;
    /// How its runs are written in the deals: `harness:claude-code`, `agent:<card>`.
    fn via(&self) -> String;
    /// Whether its model is someone else's cloud (the regulator «Что может уходить из дома» holds it): Claude Code
    /// always; a card's agent unless its model goes through DuoWave.
    fn cloud(&self) -> bool {
        true
    }
    /// Found, which version, signed in — and why not.
    async fn ready(&self) -> Ready;
    /// A session in a work copy: a new one, or the given one gone on with.
    async fn open(&self, open: Open) -> Result<Box<dyn Session>, NotOpened>;
    /// The command that opens this session in a terminal, to talk to the agent oneself — if the agent has one.
    fn resume_command(&self, session: &str, copy: &Path) -> Option<String>;
}

/// A session of an agent in a work copy: one prompt after another, each a run with its own journal.
#[async_trait]
pub trait Session: Send {
    /// Its id, once the agent gave it.
    fn id(&self) -> Option<String>;
    /// A prompt: the run goes to its end, its steps to `out`. `stop` ends it: the agent finishes the turn, and is
    /// made to end after ten seconds.
    async fn prompt(&mut self, text: &str, out: Arc<dyn Out>, stop: CancellationToken) -> RunEnd;
    /// What the next run may spend in dollars — what the work's budget left it; None — no ceiling. An agent that
    /// cannot be held to a ceiling (ACP has none) leaves it: the budget is counted between its runs.
    fn ceiling(&mut self, _usd: Option<f64>) {}
    /// The session is left: its program ends.
    async fn close(self: Box<Self>);
}

/// How long an agent has to end its turn after a stop before it is made to end.
pub const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// The environment an agent's program gets: enough to run and to reach git's remotes, nothing of the gateway's own.
/// PATH with where programs usually live — a gateway started by the app has no login shell's PATH.
pub fn environment(extra: &[(String, String)]) -> Vec<(String, String)> {
    #[cfg(unix)]
    const INHERITED: &[&str] = &[
        "HOME",
        "LOGNAME",
        "USER",
        "SHELL",
        "TERM",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TMPDIR",
        "PATH",
        "SSH_AUTH_SOCK",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "no_proxy",
        "CLAUDE_CONFIG_DIR",
        "NVM_DIR",
        "VOLTA_HOME",
        "BUN_INSTALL",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "GOPATH",
    ];
    #[cfg(windows)]
    const INHERITED: &[&str] = &[
        "APPDATA",
        "HOMEDRIVE",
        "HOMEPATH",
        "LOCALAPPDATA",
        "PATH",
        "PATHEXT",
        "PROCESSOR_ARCHITECTURE",
        "SYSTEMDRIVE",
        "SYSTEMROOT",
        "TEMP",
        "TMP",
        "USERNAME",
        "USERPROFILE",
        "PROGRAMFILES",
        "COMSPEC",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "CLAUDE_CONFIG_DIR",
    ];
    #[cfg(not(any(unix, windows)))]
    const INHERITED: &[&str] = &["PATH"];
    let mut env: Vec<(String, String)> = INHERITED
        .iter()
        .filter_map(|name| std::env::var(name).ok().map(|value| (name.to_string(), value)))
        .filter(|(_, value)| !value.starts_with("()"))
        .collect();
    let path = search_path();
    let joined = std::env::join_paths(&path).map(|joined| joined.to_string_lossy().into_owned()).unwrap_or_default();
    env.retain(|(name, _)| name != "PATH");
    env.push(("PATH".into(), joined));
    for (name, value) in extra {
        env.retain(|(known, _)| known != name);
        env.push((name.clone(), value.clone()));
    }
    env
}

/// Where programs are looked for: PATH, then where installers usually put them.
pub fn search_path() -> Vec<PathBuf> {
    let mut path: Vec<PathBuf> =
        std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default();
    #[cfg(unix)]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        let extra = [
            "/opt/homebrew/bin".to_string(),
            "/usr/local/bin".to_string(),
            format!("{home}/.local/bin"),
            format!("{home}/.claude/local"),
            format!("{home}/.npm-global/bin"),
            format!("{home}/.bun/bin"),
            format!("{home}/.cargo/bin"),
            format!("{home}/.opencode/bin"),
            format!("{home}/.factory/bin"),
        ];
        for dir in extra {
            let dir = PathBuf::from(dir);
            if !path.contains(&dir) {
                path.push(dir);
            }
        }
    }
    path
}

/// A program by its name or path: a path as given when it is a file, else the first one of the search path.
pub fn find_program(name_or_path: &str) -> Option<PathBuf> {
    let given = Path::new(name_or_path);
    if given.components().count() > 1 || given.is_absolute() {
        return given.is_file().then(|| given.to_path_buf());
    }
    for dir in search_path() {
        let candidate = dir.join(name_or_path);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        for ext in ["exe", "cmd", "bat"] {
            let candidate = dir.join(format!("{name_or_path}.{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// The first `a.b.c` in a program's words: «2.1.302 (Claude Code)» → 2.1.302.
pub fn version_in(text: &str) -> Option<Vec<u32>> {
    for word in text.split(|c: char| c.is_whitespace() || c == 'v' || c == '(' || c == ')') {
        let parts: Vec<&str> = word.split('.').collect();
        if parts.len() >= 2 && parts.iter().all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit())) {
            return Some(parts.iter().filter_map(|part| part.parse().ok()).collect());
        }
    }
    None
}

/// `a.b.c` as text.
pub fn version_text(version: &[u32]) -> String {
    version.iter().map(u32::to_string).collect::<Vec<_>>().join(".")
}

/// Whether a command a tool runs is among the folder's allowed ones — the rules as Claude Code writes them:
/// `Bash(npm test *)` (a prefix, the space before `*` a word's end), `Bash(npm test:*)` (the same), `Bash(git push)`
/// (only that), `Bash` (any command). A command of several joined with `&&`, `;`, `|` is allowed only when each is.
pub fn command_allowed(allow: &[String], command: &str) -> bool {
    let parts = split_command(command);
    !parts.is_empty() && parts.iter().all(|part| allow.iter().any(|rule| rule_allows(rule, "Bash", part)))
}

/// Whether a tool of this name is allowed whatever it is given (`Read`, `Edit`, `WebFetch`).
pub fn tool_allowed(allow: &[String], tool: &str) -> bool {
    allow.iter().any(|rule| rule.trim() == tool)
}

fn rule_allows(rule: &str, tool: &str, command: &str) -> bool {
    let rule = rule.trim();
    if rule == tool {
        return true;
    }
    let Some(inner) =
        rule.strip_prefix(tool).and_then(|rest| rest.strip_prefix('(')).and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    let command = command.trim();
    if let Some(prefix) = inner.strip_suffix(":*") {
        return command == prefix.trim() || command.starts_with(&format!("{} ", prefix.trim()));
    }
    if let Some(prefix) = inner.strip_suffix(" *") {
        return command == prefix.trim() || command.starts_with(&format!("{} ", prefix.trim()));
    }
    if let Some(prefix) = inner.strip_suffix('*') {
        return command.starts_with(prefix);
    }
    command == inner.trim()
}

/// A command line split where several commands are joined: `&&`, `||`, `;`, `|`, a new line.
fn split_command(command: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let chars: Vec<char> = command.chars().collect();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        match quote {
            Some(open) if c == open => {
                quote = None;
                current.push(c);
            }
            Some(_) => current.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                current.push(c);
            }
            None if c == ';' || c == '\n' || c == '|' || (c == '&' && chars.get(at + 1) == Some(&'&')) => {
                if c == '&' || (c == '|' && chars.get(at + 1) == Some(&'|')) {
                    at += 1;
                }
                parts.push(std::mem::take(&mut current));
            }
            None => current.push(c),
        }
        at += 1;
    }
    parts.push(current);
    parts.into_iter().map(|part| part.trim().to_string()).filter(|part| !part.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_allowed_only_by_the_folders_rules() {
        let allow = vec!["Bash(npm test *)".to_string(), "Bash(git push:*)".to_string(), "Bash(ls)".to_string()];
        assert!(command_allowed(&allow, "npm test"));
        assert!(command_allowed(&allow, "npm test -- --watch=false"));
        assert!(!command_allowed(&allow, "npm testx"));
        assert!(command_allowed(&allow, "git push origin duowave/kontakty"));
        assert!(!command_allowed(&allow, "git push-x"));
        assert!(command_allowed(&allow, "ls"));
        assert!(!command_allowed(&allow, "ls -la"));
        assert!(!command_allowed(&allow, "npm test && rm -rf /"), "each joined command must be allowed");
        assert!(command_allowed(&allow, "npm test && ls"));
        assert!(!command_allowed(&allow, "echo 'a;b'"));
        assert!(!command_allowed(&[], "git push"), "git push is allowed only when the folder lists it");
        assert!(command_allowed(&["Bash".to_string()], "anything at all"));
        assert!(tool_allowed(&["WebFetch".to_string()], "WebFetch"));
    }

    #[test]
    fn reads_versions_and_cuts_texts_at_a_char() {
        assert_eq!(version_in("2.1.302 (Claude Code)"), Some(vec![2, 1, 302]));
        assert_eq!(version_in("droid v0.22.4"), Some(vec![0, 22, 4]));
        assert_eq!(version_in("no version"), None);
        let long = "я".repeat(STEP_TEXT);
        assert!(cut(&long).len() <= STEP_TEXT + 3);
        assert!(cut(&long).ends_with('…'));
    }

    #[test]
    fn steps_are_tagged_and_only_trouble_is_kept_by_the_filter() {
        let step = Step::Tool {
            id: "t1".into(),
            kind: ToolKind::Execute,
            title: "npm test".into(),
            path: None,
            command: Some("npm test".into()),
            output: Some("1 failed".into()),
            diff: Vec::new(),
            state: ToolState::Failed,
        };
        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(json["type"], "tool");
        assert_eq!(json["kind"], "execute");
        assert_eq!(json["state"], "failed");
        assert!(step.is_trouble());
        assert!(!Step::Says { text: "ok".into() }.is_trouble());
        let back: Step = serde_json::from_value(json).unwrap();
        assert_eq!(back, step);
        let end = serde_json::to_value(Step::End {
            outcome: "ok".into(),
            text: None,
            error: None,
            usd_micro: Some(5),
            unknown: None,
        });
        assert_eq!(end.unwrap()["usdMicro"], 5);
    }
}
