//! Agents of the Agent Client Protocol (docs/plans/router-and-works.md, block A, 7б; decision 19): one implementation
//! of the socket for every agent with ACP, its program and its command taken from its card (a package of the kind
//! `agent` from MagicPlace). The program is the person's own; the card only says how to run it.
//!
//! JSON-RPC 2.0 over the program's stdio, one message per line (agentclientprotocol.com: schema v1, read 16.11.2026;
//! version 2, in preview since 20.11.2026, read 27.11.2026). Two dialects (docs/plans/work-systems.md, stage 2): the
//! card says which version to ask, `initialize` agrees on one, 1 or 2, for the whole connection; another one is «не
//! готов», never a run on a guess. `session/new` in the work copy. For a later run version 1 has `session/load` (its
//! history is not journaled again) or `session/resume` when the agent has them, and version 2 `session/resume`
//! without a replay; else a new session told the last result. The model by `session/set_config_option` when the card
//! says it can be chosen; `session/prompt` — the task, a note, a continuation; `session/cancel` — the stop. Version 1
//! ends a turn with the prompt's answer and its `stopReason`; version 2 answers the prompt at once and ends the turn
//! with an idle `state_update`.
//!
//! `session/update` becomes the common steps: `agent_message_chunk` (and version 2's whole `agent_message`) → Says,
//! `agent_thought_chunk` → Thinks, `tool_call`/`tool_call_update` (and version 2's `tool_call_content_chunk`) → Tool
//! with its diff, `plan` or version 2's `plan_update` → Plan, `usage_update` → Usage. Kinds it knows and does not need
//! are passed over; kinds it does not know are counted in the run's end.
//!
//! The socket answers the agent's `session/request_permission` itself, by the folder's allowed commands: a command —
//! when the folder's rules allow it (`git push` too only when listed, decision 21), and in version 2 only when it runs
//! in the copy; reading, searching and changing files — only inside the work copy; a fetch — when the folder allows
//! `WebFetch`. Allowed — `allow_once`, otherwise `reject_once`; `allow_always` never. The client offers no files and no
//! terminal of its own: the agent works with its own tools in the copy.
//!
//! Money: a `usage_update` with a cost in dollars is the agent's own estimate — the run's deal gets what it grew by in
//! the run; without one the deal has no price («цена неизвестна»). How the agent pays is its own business.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::claude_code::quoted;
use super::{
    ask, command_allowed, cut, environment, find_program, version_in, version_text, Diff, Ended, Harness, NotOpened,
    NotReady, Open, Out, PlanEntry, Ready, RunEnd, Session, Step, ToolKind, ToolState, STOP_WAIT,
};
use crate::plugins::manifest::AgentCard;

/// The newest version of the protocol this client speaks; it speaks 1 too.
pub const PROTOCOL: u64 = 2;
/// How long the agent has to answer `initialize` and to open a session.
const OPEN_WAIT: Duration = Duration::from_secs(60);
/// JSON-RPC's «method not found»; ACP's «authentication required» and «no such version».
const METHOD_NOT_FOUND: i64 = -32601;
const AUTH_REQUIRED: i64 = -32000;
const NO_VERSION: i64 = -32022;
/// What the journal keeps of the agent's stderr: its end.
const STDERR_TAIL: usize = 4000;

/// What is known of the agents' programs since the gateway started: a protocol of another version — by card id.
pub type Known = Arc<Mutex<HashMap<String, String>>>;

/// The protocol's version agreed at `initialize`, for the whole connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dialect {
    V1,
    V2,
}

impl Dialect {
    fn of(version: u64) -> Option<Dialect> {
        match version {
            1 => Some(Dialect::V1),
            2 => Some(Dialect::V2),
            _ => None,
        }
    }

    /// As the run's journal says it.
    fn name(self) -> &'static str {
        match self {
            Dialect::V1 => "ACP 1",
            Dialect::V2 => "ACP 2",
        }
    }
}

pub struct AcpAgent {
    id: String,
    title: String,
    card: AgentCard,
    /// The program's path the panel set (`harness_paths`), else the card's name on the search path.
    path: Option<String>,
    known: Known,
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl AcpAgent {
    pub fn new(id: String, title: String, card: AgentCard, path: Option<String>, known: Known) -> AcpAgent {
        AcpAgent { id, title, card, path, known }
    }

    pub fn card(&self) -> &AgentCard {
        &self.card
    }

    pub fn program(&self) -> Option<PathBuf> {
        let set = self.path.as_deref().filter(|path| !path.trim().is_empty());
        find_program(set.unwrap_or(&self.card.program.name))
    }
}

#[async_trait]
impl Harness for AcpAgent {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn via(&self) -> String {
        format!("agent:{}", self.id)
    }

    fn cloud(&self) -> bool {
        self.card.model.via != "duowave"
    }

    async fn ready(&self) -> Ready {
        let Some(program) = self.program() else {
            return Ready::not(NotReady::Missing { install: self.card.program.install.clone() });
        };
        let path = Some(program.display().to_string());
        let env = environment(&[]);
        let mut version = None;
        if !self.card.program.check.is_empty() {
            let args: Vec<&str> = self.card.program.check.iter().map(String::as_str).collect();
            let read = match ask(&program, &args, &env).await {
                Some((text, Some(0))) => version_in(&text),
                _ => None,
            };
            let Some(read) = read else {
                return Ready { path, not: Some(NotReady::Unknown { detail: None }), ..Ready::not(NotReady::Gone) };
            };
            let shown = version_text(&read);
            if let Some(need) = self.card.program.min_version.as_deref().and_then(version_in) {
                if read < need {
                    let not = NotReady::Old { version: shown.clone(), need: version_text(&need) };
                    return Ready { path, version: Some(shown), signed_in: None, pays: None, not: Some(not) };
                }
            }
            version = Some(shown);
        }
        if let Some(why) = locked(&self.known).get(&self.id).cloned() {
            return Ready {
                path,
                version,
                signed_in: None,
                pays: None,
                not: Some(NotReady::Unknown { detail: Some(why) }),
            };
        }
        let signed_in = if self.card.signin.check.is_empty() {
            None
        } else {
            let args: Vec<&str> = self.card.signin.check.iter().map(String::as_str).collect();
            match ask(&program, &args, &env).await {
                Some((_, Some(0))) => Some(true),
                Some((_, Some(_))) => Some(false),
                _ => None,
            }
        };
        let not = (signed_in == Some(false)).then(|| NotReady::SignedOut { hint: self.card.signin.hint.clone() });
        Ready { path, version, signed_in, pays: Some("agent".into()), not }
    }

    async fn open(&self, open: Open) -> Result<Box<dyn Session>, NotOpened> {
        let program = self.program().ok_or_else(|| NotOpened::Failed(crate::tr!("программа агента не найдена")))?;
        let session = Connection::start(&program, &self.card, &open).await?;
        match session.begin(&self.card, &open).await {
            Ok(begun) => Ok(Box::new(AcpSession { connection: session, id: begun, open })),
            Err(why) => {
                if let NotOpened::Protocol(words) = &why {
                    locked(&self.known).insert(self.id.clone(), words.clone());
                }
                session.close().await;
                Err(why)
            }
        }
    }

    fn resume_command(&self, session: &str, copy: &Path) -> Option<String> {
        let resume = self.card.resume.as_deref()?;
        Some(format!("cd {} && {}", quoted(&copy.display().to_string()), resume.replace("{session}", session)))
    }
}

/// A tool call as it stands now: what the updates said of it so far.
#[derive(Debug, Clone, Default)]
struct ToolNow {
    kind: Option<String>,
    title: String,
    path: Option<String>,
    command: Option<String>,
    output: Option<String>,
    diff: Vec<Diff>,
    state: Option<String>,
}

/// The prompt going on: where its steps go, what the agent wrote and thought in pieces, its tool calls; in version 2
/// its messages by id (a whole message may follow its pieces), and the idle state that ends its turn; the updates of
/// kinds not known.
struct Prompting {
    out: Arc<dyn Out>,
    says: String,
    thinks: String,
    said: String,
    tools: HashMap<String, ToolNow>,
    messages: HashMap<String, String>,
    thoughts: HashMap<String, String>,
    idle: Option<Value>,
    unknown: u32,
}

impl Prompting {
    fn new(out: Arc<dyn Out>) -> Prompting {
        Prompting {
            out,
            says: String::new(),
            thinks: String::new(),
            said: String::new(),
            tools: HashMap::new(),
            messages: HashMap::new(),
            thoughts: HashMap::new(),
            idle: None,
            unknown: 0,
        }
    }

    /// The new words of an update: a piece's, or what a whole message (version 2) adds to what its pieces said; a
    /// message said anew is all of it.
    fn piece(&mut self, update: &Value, thought: bool) -> String {
        let id = update.get("messageId").and_then(Value::as_str).unwrap_or_default().to_string();
        let texts = if thought { &mut self.thoughts } else { &mut self.messages };
        match update.get("content") {
            Some(Value::Array(blocks)) => {
                let whole: String =
                    blocks.iter().filter_map(|block| block.get("text").and_then(Value::as_str)).collect();
                let before = texts.insert(id, whole.clone()).unwrap_or_default();
                whole.strip_prefix(before.as_str()).map(str::to_string).unwrap_or(whole)
            }
            Some(block) if block.is_object() => {
                let piece = block.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
                texts.entry(id).or_default().push_str(&piece);
                piece
            }
            _ => String::new(),
        }
    }

    fn flush(&mut self) {
        if !self.says.trim().is_empty() {
            self.out.step(None, Step::Says { text: cut(self.says.trim()) });
        }
        if !self.thinks.trim().is_empty() {
            self.out.step(None, Step::Thinks { text: cut(self.thinks.trim()) });
        }
        self.says.clear();
        self.thinks.clear();
    }
}

/// An answer of the agent: its result, or its error's code and words.
type Answer = Result<Value, (i64, String)>;

/// The program and the line between us: the answers awaited, the prompt going on, the copy its rights hold to.
struct Shared {
    pending: Mutex<HashMap<u64, oneshot::Sender<Answer>>>,
    prompting: Mutex<Option<Prompting>>,
    /// What was said before any prompt (the opening): written into the first run's raw journal.
    early: Mutex<Vec<String>>,
    /// `session/load` replays the history: not journaled again.
    replaying: std::sync::atomic::AtomicBool,
    /// The session's cost so far as the agent said it, in dollars.
    cost: Mutex<Option<f64>>,
    /// Version 2: a turn went idle.
    turn: tokio::sync::Notify,
    stderr: Mutex<String>,
    allow: Vec<String>,
    copy: PathBuf,
    /// Nothing may change (a plan, a review): every edit is refused.
    read_only: bool,
}

impl Shared {
    fn raw(&self, line: &str) {
        let mut prompting = locked(&self.prompting);
        match prompting.as_mut() {
            Some(prompting) => prompting.out.raw(line),
            None => {
                let mut early = locked(&self.early);
                if early.len() < 200 {
                    early.push(line.to_string());
                }
            }
        }
    }
}

struct Connection {
    shared: Arc<Shared>,
    stdin: tokio::sync::Mutex<tokio::process::ChildStdin>,
    child: tokio::sync::Mutex<tokio::process::Child>,
    pid: Option<u32>,
    next: AtomicU64,
    capabilities: Mutex<Value>,
    dialect: Mutex<Dialect>,
    /// The agent's version as it named itself at `initialize`.
    agent: Mutex<Option<String>>,
}

impl Connection {
    async fn start(program: &Path, card: &AgentCard, open: &Open) -> Result<Arc<Connection>, NotOpened> {
        let mut command = tokio::process::Command::new(program);
        command
            .args(&card.acp.command)
            .current_dir(&open.copy)
            .env_clear()
            .envs(environment(&[]))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child =
            command.spawn().map_err(|error| NotOpened::Failed(format!("{}: {error}", program.display())))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.start_kill();
            return Err(NotOpened::Failed("no pipes to the agent".into()));
        };
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
            prompting: Mutex::new(None),
            early: Mutex::new(Vec::new()),
            replaying: std::sync::atomic::AtomicBool::new(false),
            cost: Mutex::new(None),
            turn: tokio::sync::Notify::new(),
            stderr: Mutex::new(String::new()),
            allow: open.allow.clone(),
            copy: crate::utils::files::canonical(&open.copy).unwrap_or_else(|_| open.copy.clone()),
            read_only: open.read_only,
        });
        if let Some(mut stderr) = child.stderr.take() {
            let shared = shared.clone();
            tokio::spawn(async move {
                let mut chunk = [0u8; 4096];
                while let Ok(n) = stderr.read(&mut chunk).await {
                    if n == 0 {
                        break;
                    }
                    let mut kept = locked(&shared.stderr);
                    kept.push_str(&String::from_utf8_lossy(&chunk[..n]));
                    if kept.len() > STDERR_TAIL * 2 {
                        let mut start = kept.len() - STDERR_TAIL;
                        while !kept.is_char_boundary(start) {
                            start += 1;
                        }
                        *kept = kept[start..].to_string();
                    }
                }
            });
        }
        let pid = child.id();
        let connection = Arc::new(Connection {
            shared: shared.clone(),
            stdin: tokio::sync::Mutex::new(stdin),
            child: tokio::sync::Mutex::new(child),
            pid,
            next: AtomicU64::new(1),
            capabilities: Mutex::new(Value::Null),
            dialect: Mutex::new(Dialect::V1),
            agent: Mutex::new(None),
        });
        let reader = Arc::downgrade(&connection);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Some(connection) = reader.upgrade() else { break };
                connection.heard(&line).await;
            }
            if let Some(connection) = reader.upgrade() {
                for (_, waiting) in locked(&connection.shared.pending).drain() {
                    let _ = waiting.send(Err((0, "the agent ended".into())));
                }
            }
        });
        Ok(connection)
    }

    async fn send(&self, message: &Value) {
        let line = message.to_string();
        self.shared.raw(&line);
        let mut stdin = self.stdin.lock().await;
        let _ = stdin.write_all(line.as_bytes()).await;
        let _ = stdin.write_all(b"\n").await;
        let _ = stdin.flush().await;
    }

    /// A request and its answer: the result, or the agent's error (its code and words).
    async fn call(&self, method: &str, params: Value, wait: Option<Duration>) -> Result<Value, (i64, String)> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (answer, answered) = oneshot::channel();
        locked(&self.shared.pending).insert(id, answer);
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).await;
        let answered = async { answered.await.unwrap_or_else(|_| Err((0, "the agent ended".into()))) };
        match wait {
            Some(wait) => tokio::time::timeout(wait, answered).await.unwrap_or_else(|_| {
                locked(&self.shared.pending).remove(&id);
                Err((0, format!("{method}: no answer")))
            }),
            None => answered.await,
        }
    }

    /// The version agreed for the whole connection.
    fn dialect(&self) -> Dialect {
        *locked(&self.dialect)
    }

    /// The protocol agreed, a session opened in the copy (or the earlier one gone on with), the model set.
    async fn begin(&self, card: &AgentCard, open: &Open) -> Result<String, NotOpened> {
        // The version the card was written for, at most this client's newest; the agent answers the one it speaks.
        let asked = card.acp.protocol.clamp(1, PROTOCOL);
        let info = json!({ "name": "duowave", "title": "DuoWave", "version": crate::VERSION });
        let mut params = json!({
            "protocolVersion": asked,
            "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
            "clientInfo": info,
        });
        // Version 2's names beside version 1's: an agent that answers 1 reads its own.
        if asked >= 2 {
            params["info"] = info.clone();
            params["capabilities"] = json!({});
        }
        let initialized = match self.call("initialize", params, Some(OPEN_WAIT)).await {
            Ok(initialized) => initialized,
            Err((NO_VERSION, _)) => {
                return Err(NotOpened::Protocol(crate::tr!(
                    "агент не говорит на версии ACP {asked}, а DuoWave знает версии 1 и 2",
                    asked = asked
                )))
            }
            Err((_, why)) => return Err(NotOpened::Failed(self.words(&why))),
        };
        let version = initialized.get("protocolVersion").and_then(Value::as_u64);
        let Some(dialect) = version.and_then(Dialect::of) else {
            let said = version.map(|version| version.to_string()).unwrap_or_else(|| "?".into());
            return Err(NotOpened::Protocol(crate::tr!(
                "агент говорит на протоколе ACP версии {said}, а DuoWave знает версии 1 и 2",
                said = said
            )));
        };
        *locked(&self.dialect) = dialect;
        let (capabilities, named) = match dialect {
            Dialect::V1 => ("agentCapabilities", "/agentInfo/version"),
            Dialect::V2 => ("capabilities", "/info/version"),
        };
        let capabilities = initialized.get(capabilities).cloned().unwrap_or(Value::Null);
        *locked(&self.agent) = initialized.pointer(named).and_then(Value::as_str).map(str::to_string);
        *locked(&self.capabilities) = capabilities.clone();
        let cwd = self.shared.copy.display().to_string();
        let opened = |result: Result<Value, (i64, String)>| -> Result<Value, NotOpened> {
            result.map_err(
                |(code, why)| if code == AUTH_REQUIRED { NotOpened::Auth(why) } else { NotOpened::Failed(why) },
            )
        };
        let mut session = None;
        if let Some(earlier) = open.session.as_deref() {
            let params = json!({ "sessionId": earlier, "cwd": cwd, "mcpServers": [] });
            let resumes = match dialect {
                Dialect::V1 => capabilities.pointer("/sessionCapabilities/resume").is_some(),
                // Version 2: every agent with sessions resumes them; without `replayFrom` nothing is replayed.
                Dialect::V2 => capabilities.get("session").is_some_and(|session| !session.is_null()),
            };
            if dialect == Dialect::V1 && capabilities.get("loadSession").and_then(Value::as_bool) == Some(true) {
                self.shared.replaying.store(true, Ordering::Relaxed);
                let loaded = self.call("session/load", params, Some(OPEN_WAIT)).await;
                self.shared.replaying.store(false, Ordering::Relaxed);
                if loaded.is_ok() {
                    session = Some((earlier.to_string(), Value::Null));
                }
            } else if resumes && self.call("session/resume", params, Some(OPEN_WAIT)).await.is_ok() {
                session = Some((earlier.to_string(), Value::Null));
            }
        }
        let (id, opened_session) = match session {
            Some(session) => session,
            None => {
                let created =
                    opened(self.call("session/new", json!({ "cwd": cwd, "mcpServers": [] }), Some(OPEN_WAIT)).await)?;
                let id = created.get("sessionId").and_then(Value::as_str).unwrap_or_default().to_string();
                if id.is_empty() {
                    return Err(NotOpened::Failed("no session id".into()));
                }
                (id, created)
            }
        };
        // The model asked for, when the card says one can be chosen and the agent has the option: version 2 names an
        // option `configId` and its kind of value.
        if let (true, Some(model)) = (card.model.choose, open.model.as_deref().filter(|model| !model.trim().is_empty()))
        {
            let key = if dialect == Dialect::V2 { "configId" } else { "id" };
            let option = opened_session
                .get("configOptions")
                .and_then(Value::as_array)
                .and_then(|options| {
                    options.iter().find(|option| option.get("category").and_then(Value::as_str) == Some("model"))
                })
                .and_then(|option| option.get(key).and_then(Value::as_str));
            if let Some(option) = option {
                let mut set = json!({ "sessionId": id, "configId": option, "value": model.trim() });
                if dialect == Dialect::V2 {
                    set["type"] = json!("id");
                }
                let _ = self.call("session/set_config_option", set, Some(OPEN_WAIT)).await;
            }
        }
        Ok(id)
    }

    /// The agent's words of a failure, with the end of what it printed to stderr.
    fn words(&self, why: &str) -> String {
        let stderr = locked(&self.shared.stderr).trim().to_string();
        if stderr.is_empty() {
            return why.to_string();
        }
        let mut start = stderr.len().saturating_sub(600);
        while !stderr.is_char_boundary(start) {
            start += 1;
        }
        format!("{why}: {}", &stderr[start..])
    }

    async fn heard(&self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        if !self.shared.replaying.load(Ordering::Relaxed) {
            self.shared.raw(line);
        }
        let Ok(message) = serde_json::from_str::<Value>(line) else { return };
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        match (method, id) {
            // An answer to a request of ours.
            (None, Some(id)) => {
                let Some(id) = id.as_u64() else { return };
                let Some(waiting) = locked(&self.shared.pending).remove(&id) else { return };
                let answer = match message.get("error") {
                    Some(error) => Err((
                        error.get("code").and_then(Value::as_i64).unwrap_or(0),
                        error.get("message").and_then(Value::as_str).unwrap_or("error").to_string(),
                    )),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = waiting.send(answer);
            }
            // A request of the agent: rights by the folder's rules; files and a terminal of ours it was not offered.
            (Some("session/request_permission"), Some(id)) => {
                let outcome = self.permission(message.get("params").unwrap_or(&Value::Null));
                self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": { "outcome": outcome } })).await;
            }
            (Some(_), Some(id)) => {
                let error = json!({ "code": METHOD_NOT_FOUND, "message": "not offered by this client" });
                self.send(&json!({ "jsonrpc": "2.0", "id": id, "error": error })).await;
            }
            (Some("session/update"), None) if !self.shared.replaying.load(Ordering::Relaxed) => {
                if let Some(update) = message.pointer("/params/update") {
                    self.update(update);
                }
            }
            _ => {}
        }
    }

    /// `session/update` as common steps. The cost is kept outside a prompt too: a resumed session may say what it spent
    /// before.
    fn update(&self, update: &Value) {
        let kind = update.get("sessionUpdate").and_then(Value::as_str).unwrap_or_default();
        if kind == "usage_update" {
            if let Some(cost) = cost_of(update) {
                *locked(&self.shared.cost) = cost;
            }
        }
        let mut guard = locked(&self.shared.prompting);
        let Some(prompting) = guard.as_mut() else { return };
        match kind {
            "agent_message_chunk" | "agent_message" => {
                if !prompting.thinks.is_empty() {
                    prompting.flush();
                }
                let piece = prompting.piece(update, false);
                prompting.says.push_str(&piece);
                prompting.said.push_str(&piece);
            }
            "agent_thought_chunk" | "agent_thought" => {
                if !prompting.says.is_empty() {
                    prompting.flush();
                }
                let piece = prompting.piece(update, true);
                prompting.thinks.push_str(&piece);
            }
            "tool_call" | "tool_call_update" | "tool_call_content_chunk" => {
                prompting.flush();
                let id = update.get("toolCallId").and_then(Value::as_str).unwrap_or_default().to_string();
                let tool = prompting.tools.entry(id.clone()).or_default();
                if kind == "tool_call_content_chunk" {
                    add_content(tool, update.get("content"));
                } else {
                    merge_tool(tool, update);
                }
                let step = tool_step(&id, tool);
                prompting.out.step(None, step);
            }
            "plan" | "plan_update" => {
                prompting.flush();
                prompting.out.step(None, Step::Plan { entries: plan_entries(update) });
            }
            "usage_update" => {
                prompting.out.step(
                    None,
                    Step::Usage {
                        tokens: None,
                        usd_micro: cost_of(update).flatten().map(|usd| (usd * 1_000_000.0).round() as u64),
                        used: update.get("used").and_then(Value::as_u64),
                        size: update.get("size").and_then(Value::as_u64),
                    },
                );
            }
            // Version 2: the first idle state since the prompt went is the end of its turn.
            "state_update" => {
                if update.get("state").and_then(Value::as_str) == Some("idle") && prompting.idle.is_none() {
                    prompting.idle = Some(update.clone());
                    self.shared.turn.notify_one();
                }
            }
            // Known, and not a step of the journal.
            "user_message_chunk"
            | "user_message"
            | "available_commands_update"
            | "config_option_update"
            | "current_mode_update"
            | "session_info_update"
            | "replay_complete"
            | "plan_removed"
            | "notice"
            | "compaction_update"
            | "compaction_summary_chunk"
            | "terminal_update"
            | "terminal_output_chunk"
            | "subagent_update"
            | "session_message"
            | "session_message_chunk" => {}
            _ => prompting.unknown += 1,
        }
    }

    /// The answer to a question of rights: `allow_once` when the folder's rules allow what it asks, else
    /// `reject_once`; `allow_always` never. The question and the answer go to the journal.
    fn permission(&self, params: &Value) -> Value {
        // Version 1 names the call; version 2 a subject: the call, or a command and the folder it runs in.
        let subject = params.get("subject").filter(|subject| !subject.is_null());
        let of = |kind: &str| subject.filter(|subject| subject.get("type").and_then(Value::as_str) == Some(kind));
        let call = params
            .get("toolCall")
            .or_else(|| of("tool_call").and_then(|subject| subject.get("toolCall")))
            .cloned()
            .unwrap_or(Value::Null);
        let mut tool = ToolNow::default();
        let known = call.get("toolCallId").and_then(Value::as_str).and_then(|id| {
            locked(&self.shared.prompting).as_ref().and_then(|prompting| prompting.tools.get(id).cloned())
        });
        if let Some(known) = known {
            tool = known;
        }
        merge_tool(&mut tool, &call);
        let mut where_ok = true;
        if let Some(command) = of("command") {
            tool.kind = Some("execute".into());
            tool.command = command.get("command").and_then(Value::as_str).map(str::to_string);
            // A command runs in the folder it names: only the copy.
            where_ok = command.get("cwd").and_then(Value::as_str).is_some_and(|cwd| self.inside(cwd));
        }
        if tool.title.is_empty() {
            tool.title = params.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
        }
        let allowed = where_ok && self.allows(&tool);
        let options = params.get("options").and_then(Value::as_array).cloned().unwrap_or_default();
        let pick = |kind: &str| {
            options
                .iter()
                .find(|option| option.get("kind").and_then(Value::as_str) == Some(kind))
                .and_then(|option| option.get("optionId").cloned())
        };
        let chosen = if allowed { pick("allow_once") } else { None }
            .or_else(|| pick("reject_once"))
            .or_else(|| pick("reject_always"));
        let granted = allowed && pick("allow_once").is_some();
        let what = tool.command.clone().or_else(|| tool.path.clone()).unwrap_or_else(|| tool.title.clone());
        if let Some(prompting) = locked(&self.shared.prompting).as_ref() {
            let kind = tool.kind.clone().unwrap_or_else(|| "other".into());
            prompting.out.step(None, Step::Asks { tool: kind, what: cut(&what), allowed: granted });
        }
        match chosen {
            Some(option) => json!({ "outcome": "selected", "optionId": option }),
            None => json!({ "outcome": "cancelled" }),
        }
    }

    /// Whether the folder's rules allow a call: a command by the allowed commands, files only inside the copy, a fetch
    /// when `WebFetch` is allowed; anything else not.
    fn allows(&self, tool: &ToolNow) -> bool {
        let inside = |path: &Option<String>| path.as_deref().is_none_or(|path| self.inside(path));
        match tool.kind.as_deref() {
            Some("execute") => tool
                .command
                .as_deref()
                .or(Some(tool.title.as_str()))
                .is_some_and(|command| command_allowed(&self.shared.allow, command)),
            Some("edit" | "delete" | "move") if self.shared.read_only => false,
            Some("read" | "edit" | "delete" | "move" | "search") => inside(&tool.path),
            Some("think" | "switch_mode") => true,
            Some("fetch") => {
                self.shared.allow.iter().any(|rule| rule.trim() == "WebFetch" || rule.trim().starts_with("WebFetch("))
            }
            _ => false,
        }
    }

    /// A path inside the work copy, relative to it or not.
    fn inside(&self, path: &str) -> bool {
        let path = Path::new(path);
        // Rooted is not relative: on Windows «/etc/hosts» (no drive) is that drive's root, not the copy's — joined to
        // the copy it was taken for a file inside it, and allowed.
        let path = if path.has_root() { path.to_path_buf() } else { self.shared.copy.join(path) };
        let real = crate::utils::files::canonical(&path)
            .or_else(|_| path.parent().map(crate::utils::files::canonical).unwrap_or(Ok(path.clone())))
            .unwrap_or(path);
        real.starts_with(&self.shared.copy)
    }

    async fn close(&self) {
        let mut child = self.child.lock().await;
        #[cfg(unix)]
        if let Some(pid) = self.pid.and_then(|pid| i32::try_from(pid).ok()) {
            // SAFETY: killpg has no memory effects; a group that is gone answers ESRCH.
            unsafe {
                libc::killpg(pid, libc::SIGTERM);
            }
        }
        // Windows: no signals; the agent and what it started go at once (it was the agent alone, its commands left).
        #[cfg(windows)]
        if let Some(pid) = self.pid {
            crate::utils::process::kill_tree(pid);
        }
        if tokio::time::timeout(Duration::from_secs(2), child.wait()).await.is_err() {
            #[cfg(unix)]
            if let Some(pid) = self.pid.and_then(|pid| i32::try_from(pid).ok()) {
                // SAFETY: as above.
                unsafe {
                    libc::killpg(pid, libc::SIGKILL);
                }
            }
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }
}

/// What an update or a question says of a call, over what was known.
fn merge_tool(tool: &mut ToolNow, update: &Value) {
    let text = |key: &str| update.get(key).and_then(Value::as_str).map(str::to_string);
    if let Some(kind) = text("kind") {
        tool.kind = Some(kind);
    }
    if let Some(title) = text("title") {
        tool.title = title;
    }
    if let Some(status) = text("status") {
        // Version 2's `paused` goes on; its `cancelled` did not come to pass.
        tool.state = Some(match status.as_str() {
            "paused" => "in_progress".into(),
            "cancelled" => "failed".into(),
            _ => status,
        });
    }
    if let Some(path) = update.pointer("/locations/0/path").and_then(Value::as_str) {
        tool.path = Some(path.to_string());
    }
    let input = update.get("rawInput");
    let command = input.and_then(|input| match input.get("command") {
        Some(Value::String(command)) => Some(command.clone()),
        Some(Value::Array(parts)) => Some(parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")),
        _ => None,
    });
    if command.is_some() {
        tool.command = command;
    }
    if tool.path.is_none() {
        tool.path = input
            .and_then(|input| input.get("path").or_else(|| input.get("file_path")).or_else(|| input.get("filePath")))
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    if let Some(content) = update.get("content").and_then(Value::as_array) {
        let mut said = Vec::new();
        let mut diff = Vec::new();
        for item in content {
            match item.get("type").and_then(Value::as_str) {
                Some("content") => {
                    if let Some(text) = item.pointer("/content/text").and_then(Value::as_str) {
                        said.push(text.to_string());
                    }
                }
                Some("diff") => match item.get("changes").and_then(Value::as_array) {
                    // Version 2: what changed, file by file, and the change as a patch when it is given (with the
                    // first file); no old text, so a file that was there has an empty one.
                    Some(changes) => {
                        let patch = item.pointer("/patch/text").and_then(Value::as_str).unwrap_or_default();
                        for (n, change) in changes.iter().enumerate() {
                            let added = change.get("operation").and_then(Value::as_str) == Some("add");
                            diff.push(Diff {
                                path: change.get("path").and_then(Value::as_str).unwrap_or_default().to_string(),
                                old: (!added).then(String::new),
                                new: if n == 0 { cut(patch) } else { String::new() },
                            });
                        }
                    }
                    None => diff.push(Diff {
                        path: item.get("path").and_then(Value::as_str).unwrap_or_default().to_string(),
                        old: item.get("oldText").and_then(Value::as_str).map(cut),
                        new: cut(item.get("newText").and_then(Value::as_str).unwrap_or_default()),
                    }),
                },
                _ => {}
            }
        }
        if !said.is_empty() {
            tool.output = Some(cut(&said.join("\n")));
        }
        if !diff.is_empty() {
            tool.diff = diff;
        }
    }
    if tool.output.is_none() {
        if let Some(Value::String(output)) = update.get("rawOutput") {
            tool.output = Some(cut(output));
        }
    }
}

/// A piece of a call's content (version 2's `tool_call_content_chunk`): its words go after the earlier ones, a diff
/// beside the earlier.
fn add_content(tool: &mut ToolNow, item: Option<&Value>) {
    let Some(item) = item else { return };
    let mut piece = ToolNow::default();
    merge_tool(&mut piece, &json!({ "content": [item] }));
    if let Some(output) = piece.output {
        let joined = match tool.output.take() {
            Some(before) => format!("{before}{output}"),
            None => output,
        };
        tool.output = Some(cut(&joined));
    }
    tool.diff.extend(piece.diff);
}

/// A plan's entries: version 1's `entries`; version 2's `plan` — its items, or its markdown as one entry.
fn plan_entries(update: &Value) -> Vec<PlanEntry> {
    let plan = update.get("plan").unwrap_or(update);
    if plan.get("type").and_then(Value::as_str) == Some("markdown") {
        let text = plan.get("content").and_then(Value::as_str).unwrap_or_default();
        return vec![PlanEntry { text: cut(text), state: "pending".into() }];
    }
    plan.get("entries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| PlanEntry {
                    text: entry.get("content").and_then(Value::as_str).unwrap_or_default().to_string(),
                    state: entry.get("status").and_then(Value::as_str).unwrap_or("pending").to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What a `usage_update` says of the session's cost in dollars: nothing (no `cost`, or another currency), taken away
/// (`null`, version 2), or the amount.
fn cost_of(update: &Value) -> Option<Option<f64>> {
    match update.get("cost")? {
        Value::Null => Some(None),
        cost => {
            let dollars = cost
                .get("currency")
                .and_then(Value::as_str)
                .is_none_or(|currency| currency.eq_ignore_ascii_case("USD"));
            let amount = cost.get("amount").and_then(Value::as_f64);
            dollars.then_some(amount).flatten().map(Some)
        }
    }
}

fn tool_step(id: &str, tool: &ToolNow) -> Step {
    Step::Tool {
        id: id.to_string(),
        kind: ToolKind::parse(tool.kind.as_deref().unwrap_or("other")),
        title: cut(&tool.title),
        path: tool.path.clone(),
        command: tool.command.as_deref().map(cut),
        output: tool.output.clone(),
        diff: tool.diff.clone(),
        state: ToolState::parse(tool.state.as_deref().unwrap_or("pending")),
    }
}

struct AcpSession {
    connection: Arc<Connection>,
    id: String,
    open: Open,
}

#[async_trait]
impl Session for AcpSession {
    fn id(&self) -> Option<String> {
        Some(self.id.clone())
    }

    async fn prompt(&mut self, text: &str, out: Arc<dyn Out>, stop: CancellationToken) -> RunEnd {
        let began = Instant::now();
        let shared = self.connection.shared.clone();
        for line in std::mem::take(&mut *locked(&shared.early)) {
            out.raw(&line);
        }
        // A new session that could not go on in the earlier one is told what came of it.
        let text = match (self.open.session.as_deref(), self.open.recap.as_deref()) {
            (Some(earlier), Some(recap)) if earlier != self.id => {
                format!("{}\n\n{}\n\n---\n\n{text}", crate::tr!("Это продолжение прошлой работы. Её итог:"), recap)
            }
            _ => text.to_string(),
        };
        self.open.recap = None;
        let dialect = self.connection.dialect();
        out.step(
            None,
            Step::Begin {
                session: Some(self.id.clone()),
                model: self.open.model.clone(),
                billing: Some("agent".into()),
                version: locked(&self.connection.agent).clone(),
                protocol: Some(dialect.name().into()),
                prompt: Some(cut(&text)),
            },
        );
        let cost_before = *locked(&shared.cost);
        *locked(&shared.prompting) = Some(Prompting::new(out.clone()));
        let params = json!({ "sessionId": self.id, "prompt": [{ "type": "text", "text": text }] });
        let connection = self.connection.clone();
        let mut answer = Box::pin(async move { connection.call("session/prompt", params, None).await });
        // Version 2: the prompt's answer says it was taken; its turn ends with the idle state.
        let mut taken: Option<Answer> = None;
        let mut stopped = false;
        let result = loop {
            if matches!(taken, Some(Err(_))) {
                // Not taken at all.
                break taken.take();
            }
            if matches!(taken, Some(Ok(_))) {
                let idle = locked(&shared.prompting).as_ref().and_then(|prompting| prompting.idle.clone());
                if let Some(idle) = idle {
                    break Some(Ok(idle));
                }
            }
            tokio::select! {
                result = &mut answer, if taken.is_none() => {
                    // Version 1 ends the turn with the answer, and so does an answer with a `stopReason` from an agent
                    // between the two versions.
                    if dialect == Dialect::V1 || result.as_ref().is_ok_and(|answer| answer.get("stopReason").is_some()) {
                        break Some(result);
                    }
                    taken = Some(result);
                }
                _ = shared.turn.notified(), if dialect == Dialect::V2 => {}
                _ = stop.cancelled(), if !stopped => {
                    stopped = true;
                    let cancel = json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": self.id } });
                    self.connection.send(&cancel).await;
                }
                _ = tokio::time::sleep(STOP_WAIT), if stopped => break None,
            }
        };
        let prompting = locked(&shared.prompting).take();
        let (said, unknown) = match prompting {
            Some(mut prompting) => {
                prompting.flush();
                (Some(prompting.said), prompting.unknown)
            }
            None => (None, 0),
        };
        let cost_after = *locked(&shared.cost);
        let usd_micro = match (cost_before, cost_after) {
            (Some(before), Some(after)) => Some((((after - before).max(0.0)) * 1_000_000.0).round() as u64),
            (None, Some(after)) => Some((after.max(0.0) * 1_000_000.0).round() as u64),
            _ => None,
        };
        let (ended, error) = match result {
            None => (Ended::Stopped, None),
            Some(Ok(answer)) => match answer.get("stopReason").and_then(Value::as_str) {
                Some("end_turn") => (Ended::Ok, None),
                Some("cancelled") => (Ended::Stopped, None),
                Some("refusal") => (Ended::Refused, None),
                Some("max_tokens" | "max_turn_requests") => (Ended::Turns, None),
                Some("error") => {
                    let why = answer.pointer("/error/message").and_then(Value::as_str).unwrap_or("error");
                    (Ended::Failed, Some(self.connection.words(why)))
                }
                // Version 2: idle is the end of the turn, whatever reason it gives or does not.
                _ if dialect == Dialect::V2 => (Ended::Ok, None),
                other => (Ended::Failed, Some(format!("stopReason {}", other.unwrap_or("?")))),
            },
            Some(Err((code, why))) if code == AUTH_REQUIRED => (Ended::Auth, Some(why)),
            Some(Err((_, why))) if stopped => (Ended::Stopped, Some(why)),
            Some(Err((_, why))) => (Ended::Failed, Some(self.connection.words(&why))),
        };
        let end = RunEnd {
            ended,
            text: said.filter(|said| !said.trim().is_empty()).map(|said| said.trim().to_string()),
            error,
            session: Some(self.id.clone()),
            model: self.open.model.clone(),
            billing: Some("agent".into()),
            usd_micro,
            models: Vec::new(),
            ms: began.elapsed().as_millis() as u64,
        };
        out.step(
            None,
            Step::End {
                outcome: end.ended.as_str().into(),
                text: end.text.as_deref().map(cut),
                error: end.error.clone(),
                usd_micro: end.usd_micro,
                unknown: (unknown > 0).then_some(unknown),
            },
        );
        end
    }

    async fn close(self: Box<Self>) {
        self.connection.close().await;
    }
}
