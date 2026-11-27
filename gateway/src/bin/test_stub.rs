//! `duowave-test-stub`: the programs the gateway's tests stand in for, in one program that runs the same on every
//! system — until 21.11 they were sh and Python scripts, and none of their tests could run on Windows. Its role is its
//! own file name, as a test copies it into place:
//!
//! - `claude` — Claude Code for the works in folders (docs/plans/router-and-works.md, stage 3): `--version`,
//!   `auth status`, and a run `-p … --output-format stream-json` answered with a stream like the real one;
//! - `acp-stub` — an agent of the Agent Client Protocol (stage 3б): JSON-RPC 2.0 over stdio, one message per line.
//! - `python` — an engine's Python for the voice lab: clone-server/imprint.py without a model (the same JSON lines and
//!   the same voice folder, made of the first take), or `-c …` — the install's check and its weights' download
//!   (`python` and `python.exe` on Windows, as the lab looks for the one and Windows runs the other);
//! - `uv` — the voice lab's Python manager: `venv` makes an environment whose Python is this stub again;
//! - `node` — Node running npm (`ci …`): it puts a dependency, as npm ci would;
//! - `job` — a program of the voice lab's jobs, its steps in its arguments (`job::run` says which);
//! - `sleep` — runs for the seconds it is given, and nothing else;
//! - whatever its name, with `DUOWAVE-STUB-VERSION:<version>` in its last bytes — the gateway's program of that
//!   version: `version` answers `duowave <version>` (updates/testing.rs, on Windows, where a script does not run).
//!
//! What it does is told by files beside it, since the gateway gives the programs it runs no environment of the test
//! (the roles below say which). Each run appends what it was asked to `calls.log` beside it.
//!
//! Only the standard library and serde_json; `libc` for SIGINT on Unix (Claude Code ends its turn on it).

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{json, Value};

fn main() -> ExitCode {
    let exe = std::env::current_exe().expect("its own path");
    let here = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let role = exe.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(version) = version_in_tail(&exe) {
        if args.first().is_some_and(|arg| arg == "version") {
            println!("duowave {version}");
        }
        return ExitCode::SUCCESS;
    }
    let stub = Stub { here };
    match role.as_str() {
        "claude" => claude::run(&stub, &args),
        "acp-stub" => acp::run(&stub),
        "bb" => bb::run(&stub, &args),
        "python" if args.first().is_some_and(|arg| arg == "-c") => engine::python(&stub, &args),
        "python" => imprint::run(&args),
        "uv" => engine::uv(&stub, &args, &exe),
        "node" => {
            stub.append("calls", &args.join(" "));
            let _ = std::fs::create_dir_all("node_modules/left-pad");
            ExitCode::SUCCESS
        }
        "job" => job::run(&args),
        "sleep" => {
            let seconds = args.first().and_then(|arg| arg.parse().ok()).unwrap_or(30);
            std::thread::sleep(Duration::from_secs(seconds));
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("duowave-test-stub: no role «{other}» (claude, acp-stub, bb, python, uv, node, job, sleep)");
            ExitCode::from(2)
        }
    }
}

/// The version written after the program's own bytes, if any: `…\nDUOWAVE-STUB-VERSION:9.9.9\n`.
fn version_in_tail(exe: &Path) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(exe).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(64))).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    let tail = String::from_utf8_lossy(&tail);
    let (_, version) = tail.rsplit_once("DUOWAVE-STUB-VERSION:")?;
    Some(version.trim().to_string()).filter(|version| !version.is_empty())
}

/// The folder it lies in, where the test leaves its words and reads what it was asked.
struct Stub {
    here: PathBuf,
}

impl Stub {
    fn path(&self, name: &str) -> PathBuf {
        self.here.join(name)
    }

    fn has(&self, name: &str) -> bool {
        self.path(name).exists()
    }

    /// A file beside it, without its line end; `default` when it is not there or empty.
    fn read(&self, name: &str, default: &str) -> String {
        std::fs::read_to_string(self.path(name))
            .ok()
            .map(|text| text.trim_end_matches(['\n', '\r']).to_string())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| default.to_string())
    }

    fn append(&self, name: &str, line: &str) {
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(self.path(name)).expect("its log");
        writeln!(file, "{line}").expect("its log");
    }
}

/// bb, as its card in works/plugs/cards.rs leads it: answers in JSON, its threads kept in `threads.json` beside it.
/// Beside it:
/// - `project-path` — the folder of its one project `pr1` (default `/work/site`);
/// - `server-down` — its server does not run: what needs it fails, as bb's own `status` does not;
/// - `done-<thread>` — that thread is done, with this file's words as its output.
///
/// Each run appends its arguments to calls.log; the words it got by stdin go to prompts.log (a new thread) or told.log
/// (`<thread> <mode> <words>`).
mod bb {
    use super::*;

    fn threads(stub: &Stub) -> Vec<Value> {
        serde_json::from_str(&stub.read("threads.json", "[]")).unwrap_or_default()
    }

    fn keep(stub: &Stub, threads: &[Value]) {
        std::fs::write(stub.path("threads.json"), Value::Array(threads.to_vec()).to_string()).expect("its threads");
    }

    fn stdin() -> String {
        let mut text = String::new();
        let _ = std::io::stdin().read_to_string(&mut text);
        text.trim_end_matches(['\n', '\r']).to_string()
    }

    /// A thread as `thread show` gives it: done when `done-<id>` is beside it.
    fn shown(stub: &Stub, thread: &Value) -> Value {
        let id = thread["id"].as_str().unwrap_or_default();
        let mut thread = thread.clone();
        if stub.has(&format!("done-{id}")) && thread["status"] != "stopped" {
            thread["status"] = json!("done");
            thread["summary"] = json!(stub.read(&format!("done-{id}"), "done"));
        }
        thread
    }

    pub fn run(stub: &Stub, args: &[String]) -> ExitCode {
        stub.append("calls.log", &args.join(" "));
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut threads = threads(stub);
        let found = |id: &str, threads: &[Value]| threads.iter().position(|thread| thread["id"] == id);
        match words.as_slice() {
            ["status", "--json"] => say(&json!({ "ok": true, "version": "0.9.0" })),
            // As bb 0.x answers (27.11.2026): a bare array, each project's folder in its sources; with its server
            // down, an error on stderr, `ok: false` on stdout and exit 1.
            ["project", "list", "--json"] if stub.has("server-down") => {
                eprintln!("Error: Cannot connect to BB server. Ensure it is running and BB_SERVER_URL is correct.");
                say(&json!({ "ok": false, "error": { "code": "error", "message": "Cannot connect to BB server." } }));
                std::process::exit(1);
            }
            ["project", "list", "--json"] => say(&json!([
                { "id": "pr1", "kind": "standard", "name": "Сайт",
                  "sources": [{ "type": "local_path", "path": stub.read("project-path", "/work/site"), "isDefault": true }] }
            ])),
            ["thread", "spawn", "--json", "--project", project, "--prompt-file", "-"] => {
                let prompt = stdin();
                stub.append("prompts.log", &prompt);
                let id = format!("t{}", threads.len() + 1);
                let title: String = prompt.split_whitespace().take(4).collect::<Vec<_>>().join(" ");
                threads.push(json!({ "id": id, "title": title, "status": "active", "project": project }));
                keep(stub, &threads);
                say(&json!({ "thread": { "id": id, "title": title } }));
            }
            ["thread", "list", "--json"] => {
                let shown: Vec<Value> = threads.iter().map(|thread| shown(stub, thread)).collect();
                say(&json!({ "threads": shown }));
            }
            ["thread", "show", id, "--json"] => match found(id, &threads) {
                Some(at) => say(&json!({ "thread": shown(stub, &threads[at]) })),
                None => {
                    eprintln!("no thread {id}");
                    return ExitCode::from(1);
                }
            },
            ["thread", "tell", id, "--mode", mode, "--message-file", "-"] if found(id, &threads).is_some() => {
                let text = stdin();
                stub.append("told.log", &format!("{id} {mode} {text}"));
            }
            // bb opens its window at the thread and says so in words.
            ["thread", "open", id] if found(id, &threads).is_some() => println!("Opened thread {id}"),
            ["thread", "stop", id] => match found(id, &threads) {
                Some(at) => {
                    threads[at]["status"] = json!("stopped");
                    keep(stub, &threads);
                    // bb's own words, not JSON.
                    println!("Thread {id} stopped");
                }
                None => return ExitCode::from(1),
            },
            ["thread", "output", id, "--json"] => {
                say(&json!({ "output": stub.read(&format!("done-{id}"), "") }));
            }
            _ => {
                eprintln!("bb: unknown command");
                return ExitCode::from(2);
            }
        }
        ExitCode::SUCCESS
    }
}

/// One line of JSON on stdout, at once (the gateway reads it as it comes).
fn say(value: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

/// Claude Code. Beside it:
/// - `signed-out` — `auth status` says nobody signed in (exit 1);
/// - `mode` — ok (default): writes contacts.txt, a subagent's step, a plan, a result with dollars; push: tries
///   `git push` of the branch — only when --allowedTools lists it, else a refusal; slow: works until SIGINT ends its
///   turn (on Windows, where there is no SIGINT, until the gateway ends it) or `go-on` appears; limit: a plan's limit
///   the first time (it leaves `limited`), then ok; fail: an error; bypass: reports the mode bypassPermissions;
///   billing: a key with no money.
///
/// Each run appends to calls.log its arguments and whether a key came, and to prompts.log what it was asked.
mod claude {
    use super::*;

    pub fn run(stub: &Stub, args: &[String]) -> ExitCode {
        let key = std::env::var("ANTHROPIC_API_KEY").is_ok_and(|key| !key.is_empty());
        match args.first().map(String::as_str) {
            Some("--version") => {
                println!("2.1.302 (Claude Code)");
                return ExitCode::SUCCESS;
            }
            // Its sessions, as `claude agents --json` lists them: what `agents.json` beside it says, else none.
            Some("agents") => {
                println!("{}", stub.read("agents.json", "[]"));
                return ExitCode::SUCCESS;
            }
            Some("auth") => {
                if stub.has("signed-out") {
                    println!(r#"{{"loggedIn":false,"authMethod":"none"}}"#);
                    return ExitCode::from(1);
                }
                let method = if key { "api_key" } else { "claude.ai" };
                println!(r#"{{"loggedIn":true,"authMethod":"{method}"}}"#);
                return ExitCode::SUCCESS;
            }
            _ => {}
        }
        let (session, allowed) = read_args(args);
        let mut prompt = String::new();
        let _ = std::io::stdin().read_to_string(&mut prompt);
        stub.append("calls.log", &format!("{} key={}", args.join(" "), if key { "yes" } else { "no" }));
        stub.append("prompts.log", prompt.trim_end_matches('\n'));
        let mode = stub.read("mode", "ok");
        let cwd = std::env::current_dir().map(|dir| dir.display().to_string()).unwrap_or_default();
        say(&json!({
            "type": "system", "subtype": "init", "session_id": session, "model": "claude-sonnet-5-5",
            "apiKeySource": if key { "ANTHROPIC_API_KEY" } else { "none" },
            "permissionMode": if mode == "bypass" { "bypassPermissions" } else { asked_mode(args) },
            "claude_code_version": "2.1.302", "cwd": cwd,
        }));
        let result = |subtype: &str, is_error: bool, text: &str, usd: f64| {
            say(&json!({
                "type": "result", "subtype": subtype, "is_error": is_error, "result": text, "session_id": session,
                "total_cost_usd": usd,
                "modelUsage": {"claude-sonnet-5-5": {
                    "inputTokens": 100, "outputTokens": 50, "cacheReadInputTokens": 1000,
                    "cacheCreationInputTokens": 10, "costUSD": usd,
                }},
                "permission_denials": [],
            }))
        };
        let assistant = |parent: Value, content: Value| {
            say(&json!({"type": "assistant", "parent_tool_use_id": parent, "message": {"content": content}}))
        };
        let user = |parent: Value, content: Value| {
            say(&json!({"type": "user", "parent_tool_use_id": parent, "message": {"content": content}}))
        };
        // Our system's ways (stage 6 of docs/plans/work-systems.md): a plan or a review, said and nothing changed. The
        // plan is `plan` beside it; the first review `review`, the next ones `review-again` (both «ok» by default).
        if prompt.contains("<duowave-plan>") || prompt.contains("<duowave-review>") {
            const OK: &str = r#"{"verdict": "ok", "blocking": [], "notes": []}"#;
            let text = if prompt.contains("<duowave-plan>") {
                stub.read(
                    "plan",
                    "1. Прочитать README. 2. Поправить опечатку. Не сломать: сборку. Проверка: test -f README.md",
                )
            } else {
                let before =
                    std::fs::read_to_string(stub.path("reviewed.log")).map(|text| text.lines().count()).unwrap_or(0);
                stub.append("reviewed.log", "review");
                if before == 0 {
                    stub.read("review", OK)
                } else {
                    stub.read("review-again", OK)
                }
            };
            assistant(Value::Null, json!([{"type": "text", "text": text}]));
            result("success", false, &text, 0.05);
            return ExitCode::SUCCESS;
        }
        match mode.as_str() {
            "bypass" => {
                std::thread::sleep(Duration::from_secs(5));
                result("success", false, "done", 0.0);
                return ExitCode::SUCCESS;
            }
            "fail" => {
                assistant(Value::Null, json!([{"type": "text", "text": "Something broke."}]));
                result("error_during_execution", true, "API Error: 500", 0.01);
                return ExitCode::from(1);
            }
            "billing" => {
                say(&json!({
                    "type": "assistant", "error": "billing_error", "parent_tool_use_id": null,
                    "message": {"content": [{"type": "text", "text": "Credit balance is too low"}]},
                }));
                result("success", true, "Credit balance is too low", 0.0);
                return ExitCode::from(1);
            }
            "limit" if !stub.has("limited") => {
                let _ = std::fs::write(stub.path("limited"), "");
                let resets = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|now| now.as_secs() + 2)
                    .unwrap_or(0);
                say(&json!({
                    "type": "rate_limit_event", "session_id": session,
                    "rate_limit_info": {"status": "rejected", "resetsAt": resets, "rateLimitType": "five_hour"},
                }));
                result("success", true, "You've hit your session limit", 0.02);
                return ExitCode::from(1);
            }
            "slow" => {
                interrupt::listen();
                assistant(
                    Value::Null,
                    json!([{"type": "tool_use", "id": "t-slow", "name": "Bash",
                        "input": {"command": "npm test", "description": "Run tests"}}]),
                );
                for _ in 0..300 {
                    if interrupt::heard() {
                        result("success", false, "Stopped by the person.", 0.05);
                        return ExitCode::SUCCESS;
                    }
                    if stub.has("go-on") {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                user(Value::Null, json!([{"type": "tool_result", "tool_use_id": "t-slow", "content": "ok"}]));
            }
            "push" => push(&allowed, assistant, user),
            _ => {}
        }
        // The work itself: a file, a subagent's step, a plan, the last words.
        let runs = std::fs::read_to_string(stub.path("calls.log")).map(|text| text.lines().count()).unwrap_or(0);
        let mut contacts =
            std::fs::OpenOptions::new().create(true).append(true).open("contacts.txt").expect("contacts.txt");
        let _ = writeln!(contacts, "form v{runs}");
        assistant(
            Value::Null,
            json!([{"type": "thinking", "thinking": "A contacts page with a form."},
                {"type": "tool_use", "id": "t1", "name": "Write",
                    "input": {"file_path": "contacts.txt", "content": "form"}}]),
        );
        user(Value::Null, json!([{"type": "tool_result", "tool_use_id": "t1", "content": "written"}]));
        assistant(
            Value::Null,
            json!([{"type": "tool_use", "id": "task1", "name": "Task", "input": {"description": "Check the form"}}]),
        );
        assistant(
            json!("task1"),
            json!([{"type": "tool_use", "id": "t2", "name": "Read", "input": {"file_path": "contacts.txt"}}]),
        );
        user(json!("task1"), json!([{"type": "tool_result", "tool_use_id": "t2", "content": "form"}]));
        user(Value::Null, json!([{"type": "tool_result", "tool_use_id": "task1", "content": "fine"}]));
        assistant(
            Value::Null,
            json!([{"type": "tool_use", "id": "todo", "name": "TodoWrite",
                "input": {"todos": [{"content": "Form", "status": "completed"}]}}]),
        );
        assistant(Value::Null, json!([{"type": "text", "text": "Done: the contacts page."}]));
        result("success", false, "Done: the contacts page.", 1.2);
        ExitCode::SUCCESS
    }

    /// The permission mode asked (`--permission-mode`), as Claude Code says it back.
    fn asked_mode(args: &[String]) -> &str {
        args.iter()
            .position(|arg| arg == "--permission-mode")
            .and_then(|at| args.get(at + 1))
            .map(String::as_str)
            .unwrap_or("acceptEdits")
    }

    /// The session (`--session-id` or `--resume`) and the rules after `--allowedTools`, up to the next option.
    fn read_args(args: &[String]) -> (String, Vec<String>) {
        let mut session = "none".to_string();
        let mut allowed = Vec::new();
        let mut words = args.iter().peekable();
        while let Some(word) = words.next() {
            match word.as_str() {
                "--session-id" | "--resume" => {
                    if let Some(id) = words.next() {
                        session = id.clone();
                    }
                }
                "--allowedTools" => {
                    while let Some(rule) = words.next_if(|next| !next.starts_with("--")) {
                        allowed.push(rule.clone());
                    }
                }
                _ => {}
            }
        }
        (session, allowed)
    }

    /// `git push` of the branch — when the folder's rules let it, else Claude Code's refusal.
    fn push(allowed: &[String], assistant: impl Fn(Value, Value), user: impl Fn(Value, Value)) {
        let git = |args: &[&str]| std::process::Command::new("git").args(args).output();
        let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"])
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_default();
        assistant(
            Value::Null,
            json!([{"type": "tool_use", "id": "t-push", "name": "Bash",
                "input": {"command": format!("git push origin {branch}")}}]),
        );
        if !allowed.iter().any(|rule| rule.contains("Bash(git push")) {
            say(&json!({
                "type": "system", "subtype": "permission_denied", "tool_name": "Bash", "tool_use_id": "t-push",
                "message": "This command requires approval",
            }));
            return;
        }
        let _ = git(&["add", "-A"]);
        let _ = git(&["-c", "user.name=Stub", "-c", "user.email=stub@example.com", "commit", "-qm", "stub"]);
        let pushed = git(&["push", "-q", "origin", &branch]).is_ok_and(|output| output.status.success());
        let mut answer = json!({"type": "tool_result", "tool_use_id": "t-push",
            "content": if pushed { "pushed" } else { "push failed" }});
        if !pushed {
            answer["is_error"] = json!(true);
        }
        user(Value::Null, json!([answer]));
    }

    /// SIGINT, as Claude Code takes it: the end of its turn, with its result. Windows has no such signal: the gateway
    /// ends the program there, and the stub with it.
    mod interrupt {
        use std::sync::atomic::{AtomicBool, Ordering};

        static HEARD: AtomicBool = AtomicBool::new(false);

        #[cfg(unix)]
        extern "C" fn on_interrupt(_: libc::c_int) {
            HEARD.store(true, Ordering::SeqCst);
        }

        pub fn listen() {
            #[cfg(unix)]
            // SAFETY: the handler only stores into an atomic, which is async-signal-safe.
            unsafe {
                libc::signal(libc::SIGINT, on_interrupt as *const () as libc::sighandler_t);
            }
        }

        pub fn heard() -> bool {
            HEARD.load(Ordering::SeqCst)
        }
    }
}

/// An agent of the Agent Client Protocol. Beside it:
/// - `protocol` — the protocol version it speaks: 1 when absent; 2 — up to version 2 (agentclientprotocol.com/protocol/
///   v2), so it answers the version asked, and when it is 2: its names at `initialize`, `session/resume`, the prompt
///   answered at once and its turn ended by an idle `state_update`, updates and questions of rights in version 2's
///   shapes, and one update of a kind no client knows; 3 — it answers 3 whatever is asked; reject — it refuses
///   `initialize` with «unsupported protocol version»;
/// - `mode` — ok (default): thinks and writes in pieces, a plan, an edit with a diff (and contacts.txt in the copy),
///   asks to run `npm test`, then `git push`, then to read a file outside the copy; a usage with a cost in dollars;
///   nocost: the same, its usage without a cost; junie: as Junie of JetBrains said it (only «Yes» and «No», the
///   tokens of the turn in the prompt's answer, no price); slow: works until `session/cancel`; auth: the prompt answers
///   «authentication required»; noload: it cannot load a session.
///
/// It appends to calls.log what it was asked and how its questions were answered.
mod acp {
    use super::*;

    struct Agent<'a> {
        stub: &'a Stub,
        lines: std::io::Lines<std::io::StdinLock<'static>>,
        /// Messages read while it waited for an answer.
        pending: Vec<Value>,
        cancelled: bool,
        next_id: u64,
        sessions: u32,
        /// Version 2 agreed at `initialize`.
        v2: bool,
        /// What its session spent before this run, as told at `session/resume`.
        cost: f64,
    }

    pub fn run(stub: &Stub) -> ExitCode {
        let mode = stub.read("mode", "ok");
        let mut agent = Agent {
            stub,
            lines: std::io::stdin().lock().lines(),
            pending: Vec::new(),
            cancelled: false,
            next_id: 100,
            sessions: 0,
            v2: false,
            cost: 0.0,
        };
        let protocol = stub.read("protocol", "1");
        while let Some(message) = agent.receive() {
            let id = message.get("id").cloned();
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            match message.get("method").and_then(Value::as_str) {
                Some("initialize") => {
                    stub.append("calls.log", &format!("initialize {}", spaced(&params["clientCapabilities"])));
                    let asked = params["protocolVersion"].as_u64().unwrap_or(1);
                    stub.append("calls.log", &format!("asked {asked}"));
                    let version = match protocol.trim() {
                        "reject" => {
                            send(json!({"id": id, "error": {"code": -32022, "message": "Unsupported protocol version",
                                "data": {"requested": asked, "supported": [1]}}}));
                            continue;
                        }
                        "2" => asked.min(2),
                        other => other.parse().unwrap_or(1),
                    };
                    agent.v2 = version == 2;
                    if agent.v2 {
                        send(json!({"id": id, "result": {
                            "protocolVersion": 2,
                            "info": {"name": "stub", "title": "Stub", "version": "0.2.0"},
                            "capabilities": {"session": {"prompt": {}, "mcp": {"stdio": {}}}},
                            "authMethods": [],
                        }}));
                    } else {
                        send(json!({"id": id, "result": {
                            "protocolVersion": version,
                            "agentCapabilities": {"loadSession": mode != "noload", "promptCapabilities": {}},
                            "authMethods": if mode == "junie" {
                                json!([{"type": "agent", "id": "jetbrains-account", "name": "Junie Account"}])
                            } else {
                                json!([])
                            },
                            "agentInfo": {"name": "stub", "version": "0.1.0"},
                        }}));
                    }
                }
                Some("session/new") => {
                    agent.sessions += 1;
                    let session = format!("acp-{}-{}", std::process::id(), agent.sessions);
                    let cwd = params["cwd"].as_str().unwrap_or_default();
                    stub.append("calls.log", &format!("new {session} in {cwd}"));
                    if agent.v2 {
                        send(json!({"id": id, "result": {"sessionId": session, "configOptions": [
                            {"configId": "model", "name": "Model", "category": "model", "type": "select",
                                "currentValue": "default", "options": []},
                        ], "availableCommands": []}}));
                    } else {
                        send(json!({"id": id, "result": {"sessionId": session, "configOptions": [
                            {"id": "model", "name": "Model", "category": "model", "type": "select",
                                "currentValue": "default", "options": []},
                        ]}}));
                    }
                }
                // Version 2: the session goes on, its state and what it spent told before the answer, no replay.
                Some("session/resume") if agent.v2 => {
                    let session = params["sessionId"].as_str().unwrap_or_default().to_string();
                    stub.append("calls.log", &format!("resume {session}"));
                    update(&session, json!({"sessionUpdate": "state_update", "state": "idle"}));
                    if mode != "nocost" {
                        agent.cost = 0.42;
                        update(
                            &session,
                            json!({"sessionUpdate": "usage_update", "cost": {"amount": 0.42, "currency": "USD"}}),
                        );
                    }
                    send(json!({"id": id, "result": {}}));
                }
                Some("session/load") => {
                    let session = params["sessionId"].as_str().unwrap_or_default().to_string();
                    stub.append("calls.log", &format!("load {session}"));
                    update(
                        &session,
                        json!({"sessionUpdate": "agent_message_chunk",
                            "content": {"type": "text", "text": "replayed history"}}),
                    );
                    send(json!({"id": id, "result": {}}));
                }
                Some("session/set_config_option") => {
                    let kind = params["type"].as_str().map(|kind| format!(" {kind}")).unwrap_or_default();
                    stub.append("calls.log", &format!("model {}{kind}", params["value"].as_str().unwrap_or_default()));
                    send(json!({"id": id, "result": {"configOptions": []}}));
                }
                Some("session/prompt") if agent.v2 => agent.prompt2(id, &params, &mode),
                Some("session/prompt") => agent.prompt(id, &params, &mode),
                Some("session/cancel") => {}
                Some(_) if id.is_some() => {
                    send(json!({"id": id, "error": {"code": -32601, "message": "unknown"}}));
                }
                _ => {}
            }
        }
        ExitCode::SUCCESS
    }

    /// JSON as Python's json.dumps writes it, `{"a": 1, "b": 2}`: the tests read the log of the Python stub this was.
    fn spaced(value: &Value) -> String {
        match value {
            Value::Object(map) => {
                let fields: Vec<String> =
                    map.iter().map(|(key, value)| format!("{}: {}", json!(key), spaced(value))).collect();
                format!("{{{}}}", fields.join(", "))
            }
            Value::Array(items) => format!("[{}]", items.iter().map(spaced).collect::<Vec<_>>().join(", ")),
            other => other.to_string(),
        }
    }

    fn send(mut message: Value) {
        message["jsonrpc"] = json!("2.0");
        say(&message);
    }

    fn update(session: &str, body: Value) {
        send(json!({"method": "session/update", "params": {"sessionId": session, "update": body}}));
    }

    /// What a question of rights offers.
    fn options() -> Value {
        json!([
            {"optionId": "once", "name": "Allow once", "kind": "allow_once"},
            {"optionId": "always", "name": "Always allow", "kind": "allow_always"},
            {"optionId": "no", "name": "Reject", "kind": "reject_once"},
        ])
    }

    impl Agent<'_> {
        /// The next message: one set aside first; None once the input closes.
        fn receive(&mut self) -> Option<Value> {
            if !self.pending.is_empty() {
                return Some(self.pending.remove(0));
            }
            loop {
                let line = self.lines.next()?.ok()?;
                if !line.trim().is_empty() {
                    return serde_json::from_str(&line).ok();
                }
            }
        }

        /// A question of rights: the option the client chose.
        fn ask(&mut self, session: &str, title: &str, kind: &str, raw: Value, locations: Option<Value>) -> String {
            self.ask_with(session, title, kind, raw, locations, options())
        }

        /// The same with the options the agent offers: Junie offers only «Yes» and «No».
        fn ask_with(
            &mut self,
            session: &str,
            title: &str,
            kind: &str,
            raw: Value,
            locations: Option<Value>,
            offered: Value,
        ) -> String {
            self.next_id += 1;
            let asked = self.next_id;
            let mut call = json!({"toolCallId": format!("perm-{asked}"), "title": title, "kind": kind,
                "status": "pending", "rawInput": raw});
            if let Some(locations) = locations {
                call["locations"] = locations;
            }
            send(json!({"id": asked, "method": "session/request_permission", "params": {
                "sessionId": session, "toolCall": call, "options": offered,
            }}));
            self.answer(asked, title)
        }

        /// Version 2's question of rights: its title and its subject, a call or a command and where it runs.
        fn ask2(&mut self, session: &str, title: &str, subject: Value) -> String {
            self.next_id += 1;
            let asked = self.next_id;
            send(json!({"id": asked, "method": "session/request_permission", "params": {
                "sessionId": session, "title": title, "subject": subject, "options": options(),
            }}));
            self.answer(asked, title)
        }

        /// The client's answer to a question of rights: the option it chose, logged.
        fn answer(&mut self, asked: u64, title: &str) -> String {
            loop {
                let Some(message) = self.receive() else { std::process::exit(0) };
                if message.get("id").and_then(Value::as_u64) == Some(asked) && message.get("result").is_some() {
                    let outcome = &message["result"]["outcome"];
                    let chosen = outcome
                        .get("optionId")
                        .or_else(|| outcome.get("outcome"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    self.stub.append("calls.log", &format!("permission {title} -> {chosen}"));
                    return chosen;
                }
                if message.get("method").and_then(Value::as_str) == Some("session/cancel") {
                    self.cancelled = true;
                    continue;
                }
                self.pending.push(message);
            }
        }

        fn prompt(&mut self, id: Option<Value>, params: &Value, mode: &str) {
            let session = params["sessionId"].as_str().unwrap_or_default().to_string();
            let text = params["prompt"][0]["text"].as_str().unwrap_or_default();
            self.stub.append("calls.log", &format!("prompt {session}: {}", text.replace('\n', " ")));
            match mode {
                "auth" => {
                    send(json!({"id": id, "error": {"code": -32000, "message": "Authentication required"}}));
                    return;
                }
                "slow" => {
                    update(
                        &session,
                        json!({"sessionUpdate": "tool_call", "toolCallId": "t-slow", "title": "Run tests",
                            "kind": "execute", "status": "in_progress", "rawInput": {"command": "npm test"}}),
                    );
                    while !self.cancelled {
                        let Some(message) = self.receive() else { std::process::exit(0) };
                        if message.get("method").and_then(Value::as_str) == Some("session/cancel") {
                            self.cancelled = true;
                        }
                    }
                    self.cancelled = false;
                    send(json!({"id": id, "result": {"stopReason": "cancelled"}}));
                    return;
                }
                "junie" => {
                    self.prompt_junie(id, &session);
                    return;
                }
                _ => {}
            }
            let thought =
                |text: &str| json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": text}});
            update(&session, thought("A contacts page "));
            update(&session, thought("with a form."));
            update(
                &session,
                json!({"sessionUpdate": "plan",
                    "entries": [{"content": "Form", "priority": "high", "status": "in_progress"}]}),
            );
            let mut contacts =
                std::fs::OpenOptions::new().create(true).append(true).open("contacts.txt").expect("contacts.txt");
            let _ = writeln!(contacts, "form");
            let written = std::env::current_dir().unwrap_or_default().join("contacts.txt");
            update(
                &session,
                json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Write contacts.txt",
                    "kind": "edit", "status": "in_progress",
                    "locations": [{"path": written.display().to_string()}]}),
            );
            update(
                &session,
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
                    "content": [{"type": "diff", "path": "contacts.txt", "oldText": null, "newText": "form"}]}),
            );
            let tests = self.ask(&session, "npm test", "execute", json!({"command": "npm test"}), None);
            let passed = tests == "once";
            update(
                &session,
                json!({"sessionUpdate": "tool_call", "toolCallId": "t2", "title": "npm test", "kind": "execute",
                    "status": if passed { "completed" } else { "failed" }, "rawInput": {"command": "npm test"},
                    "content": [{"type": "content", "content": {"type": "text",
                        "text": if passed { "Tests: 3 passed" } else { "not allowed" }}}]}),
            );
            self.ask(&session, "git push origin HEAD", "execute", json!({"command": "git push origin HEAD"}), None);
            self.ask(
                &session,
                "Read a secret",
                "read",
                json!({"path": "/etc/hosts"}),
                Some(json!([{"path": "/etc/hosts"}])),
            );
            let mut usage = json!({"sessionUpdate": "usage_update", "used": 12000, "size": 200000});
            if mode != "nocost" {
                usage["cost"] = json!({"amount": 0.42, "currency": "USD"});
            }
            update(&session, usage);
            let message =
                |text: &str| json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}});
            update(&session, message("Done: "));
            update(&session, message("the contacts page."));
            send(json!({"id": id, "result": {"stopReason": "end_turn"}}));
        }

        /// What Junie (JetBrains, `junie --acp true`) said over ACP version 1 in a real run (27.11.2026): its commands
        /// and the session's title as updates, a question of rights with only «Yes» (allow_once) and «No» (reject_once),
        /// the command's own input with its folder and its output with an exit code, a read that starts as «other», the
        /// final answer marked in `_meta`, and the turn's tokens in the prompt's answer: no price.
        fn prompt_junie(&mut self, id: Option<Value>, session: &str) {
            let offered = json!([
                {"optionId": "Yes", "name": "Yes", "kind": "allow_once"},
                {"optionId": "No", "name": "No", "kind": "reject_once"},
            ]);
            update(
                session,
                json!({"sessionUpdate": "available_commands_update",
                    "availableCommands": [{"name": "plan", "description": "Switch to Plan mode"}]}),
            );
            update(session, json!({"sessionUpdate": "session_info_update", "title": "Write probe.txt via echo"}));
            let cwd = std::env::current_dir().unwrap_or_default().display().to_string();
            let write = json!({"command": "echo hello > probe.txt", "cwd": cwd});
            update(
                session,
                json!({"sessionUpdate": "tool_call", "toolCallId": "j1", "title": "echo hello > probe.txt",
                    "kind": "execute", "status": "in_progress", "content": [], "locations": [], "rawInput": write}),
            );
            let allowed =
                self.ask_with(session, "echo hello > probe.txt", "execute", write.clone(), None, offered.clone());
            if allowed == "Yes" {
                let _ = std::fs::write("probe.txt", "hello\n");
            }
            update(
                session,
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "j1", "title": "echo hello > probe.txt",
                    "kind": "execute", "status": if allowed == "Yes" { "completed" } else { "failed" },
                    "content": [], "locations": [], "rawInput": write, "rawOutput": {"output": "", "exitCode": 0}}),
            );
            let clean = json!({"command": "rm -rf node_modules", "cwd": cwd});
            self.ask_with(session, "rm -rf node_modules", "execute", clean, None, offered);
            let probe = std::env::current_dir().unwrap_or_default().join("probe.txt").display().to_string();
            update(
                session,
                json!({"sessionUpdate": "tool_call", "toolCallId": "j2", "title": "Open probe.txt", "kind": "other",
                    "status": "in_progress", "content": [], "locations": []}),
            );
            update(
                session,
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "j2", "title": "Open probe.txt",
                    "kind": "read", "status": "completed",
                    "content": [{"type": "content", "content": {"type": "text", "text": "probe.txt"}}],
                    "locations": [{"path": probe}]}),
            );
            update(
                session,
                json!({"sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": "**Summary**\n- probe.txt holds `hello`.\n\n"},
                    "messageId": "m1", "_meta": {"codex": {"phase": "final_answer"}, "junie": {"version": 1, "phase": "final_answer"}}}),
            );
            send(json!({"id": id, "result": {"stopReason": "end_turn", "usage": {"inputTokens": 20472,
                "outputTokens": 962, "totalTokens": 57594, "cachedReadTokens": 36160, "cachedWriteTokens": 0}}}));
        }

        /// Version 2: the prompt answered at once with its message's id; the same work in version 2's shapes; its turn
        /// ended by an idle state.
        fn prompt2(&mut self, id: Option<Value>, params: &Value, mode: &str) {
            let session = params["sessionId"].as_str().unwrap_or_default().to_string();
            let text = params["prompt"][0]["text"].as_str().unwrap_or_default();
            self.stub.append("calls.log", &format!("prompt {session}: {}", text.replace('\n', " ")));
            if mode == "auth" {
                send(json!({"id": id, "error": {"code": -32000, "message": "Authentication required"}}));
                return;
            }
            self.next_id += 1;
            send(json!({"id": id, "result": {"messageId": format!("u-{}", self.next_id)}}));
            let state = |state: &str, reason: Option<&str>| {
                let mut body = json!({"sessionUpdate": "state_update", "state": state});
                if let Some(reason) = reason {
                    body["stopReason"] = json!(reason);
                }
                update(&session, body);
            };
            state("running", None);
            if mode == "slow" {
                update(
                    &session,
                    json!({"sessionUpdate": "tool_call_update", "toolCallId": "t-slow", "title": "Run tests",
                        "kind": "execute", "status": "in_progress", "rawInput": {"command": "npm test"}}),
                );
                while !self.cancelled {
                    let Some(message) = self.receive() else { std::process::exit(0) };
                    if message.get("method").and_then(Value::as_str) == Some("session/cancel") {
                        self.cancelled = true;
                    }
                }
                self.cancelled = false;
                state("idle", Some("cancelled"));
                return;
            }
            let thought = |text: &str| {
                json!({"sessionUpdate": "agent_thought_chunk", "messageId": "th-1",
                    "content": {"type": "text", "text": text}})
            };
            update(&session, thought("A contacts page "));
            update(&session, thought("with a form."));
            update(
                &session,
                json!({"sessionUpdate": "plan_update", "plan": {"type": "items", "planId": "p-1",
                    "entries": [{"content": "Form", "priority": "high", "status": "in_progress"}]}}),
            );
            let mut contacts =
                std::fs::OpenOptions::new().create(true).append(true).open("contacts.txt").expect("contacts.txt");
            let _ = writeln!(contacts, "form");
            let here = std::env::current_dir().unwrap_or_default();
            let written = here.join("contacts.txt");
            update(
                &session,
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "title": "Write contacts.txt",
                    "kind": "edit", "status": "in_progress",
                    "locations": [{"path": written.display().to_string()}]}),
            );
            update(
                &session,
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
                    "content": [{"type": "diff", "changes": [{"operation": "add", "path": "contacts.txt"}],
                        "patch": {"format": "git_patch", "text": "+form"}}]}),
            );
            let here = here.display().to_string();
            let tests = self.ask2(&session, "npm test", json!({"type": "command", "command": "npm test", "cwd": here}));
            let passed = tests == "once";
            update(
                &session,
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "t2", "title": "npm test", "kind": "execute",
                    "status": if passed { "completed" } else { "failed" }, "rawInput": {"command": "npm test"}}),
            );
            update(
                &session,
                json!({"sessionUpdate": "tool_call_content_chunk", "toolCallId": "t2",
                    "content": {"type": "content", "content": {"type": "text",
                        "text": if passed { "Tests: 3 passed" } else { "not allowed" }}}}),
            );
            self.ask2(&session, "npm test in the root", json!({"type": "command", "command": "npm test", "cwd": "/"}));
            self.ask2(
                &session,
                "git push origin HEAD",
                json!({"type": "command", "command": "git push origin HEAD", "cwd": here}),
            );
            self.ask2(
                &session,
                "Read a secret",
                json!({"type": "tool_call", "toolCall": {"toolCallId": "t3", "title": "Read a secret", "kind": "read",
                    "status": "pending", "rawInput": {"path": "/etc/hosts"}, "locations": [{"path": "/etc/hosts"}]}}),
            );
            update(&session, json!({"sessionUpdate": "a_kind_of_a_later_version", "what": "no client knows it"}));
            let mut usage = json!({"sessionUpdate": "usage_update", "used": 12000, "size": 200000});
            if mode != "nocost" {
                usage["cost"] = json!({"amount": self.cost + 0.42, "currency": "USD"});
            }
            update(&session, usage);
            let message = |text: &str| {
                json!({"sessionUpdate": "agent_message_chunk", "messageId": "m-1",
                    "content": {"type": "text", "text": text}})
            };
            update(&session, message("Done: "));
            update(&session, message("the contacts page."));
            // The whole message after its pieces: nothing new in it.
            update(
                &session,
                json!({"sessionUpdate": "agent_message", "messageId": "m-1",
                    "content": [{"type": "text", "text": "Done: the contacts page."}]}),
            );
            state("idle", Some("end_turn"));
        }
    }
}

/// The imprint as clone-server/imprint.py does it (`python imprint.py --takes … --out … --name … --engine … --device …
/// --samples '["…"]'`), without a model: its stages and memory as JSON lines, and the voice folder — the first take
/// as the reference and the one sample, a voice.pt, meta.json.
mod imprint {
    use super::*;

    pub fn run(args: &[String]) -> ExitCode {
        let mut fields: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
        let mut field: Option<&str> = None;
        for arg in args {
            if let Some(name) = arg.strip_prefix("--") {
                field = Some(name);
                continue;
            }
            // --takes lists them all: the first is the one.
            match field {
                Some("takes") if fields.contains_key("takes") => {}
                Some(name) => {
                    fields.insert(name, arg.clone());
                }
                None => {}
            }
        }
        let get = |name: &str| fields.get(name).cloned().unwrap_or_default();
        let out = PathBuf::from(get("out"));
        let first = serde_json::from_str::<Vec<String>>(&get("samples"))
            .ok()
            .and_then(|samples| samples.into_iter().next())
            .unwrap_or_default();
        say(&json!({"stage": "select", "percent": 12}));
        say(&json!({"memoryMb": 900}));
        say(&json!({"stage": "imprint", "percent": 55}));
        let made = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(out.join("samples"))?;
            std::fs::copy(get("takes"), out.join("ref.wav"))?;
            std::fs::copy(get("takes"), out.join("samples/1.wav"))?;
            std::fs::write(out.join("voice.pt"), "imprint")?;
            let id = out.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            let meta = json!({
                "id": id, "name": get("name"), "engine": get("engine"), "device": get("device"),
                "createdAt": "2026-11-06T01:00:00+00:00",
                "reference": {"take": 1, "start": 1, "end": 7, "seconds": 6, "snrDb": 30, "words": "слова образца"},
                "samples": [{"n": 1, "text": first, "seconds": 3.2}], "rtf": 1.2, "peakMb": 2100,
            });
            std::fs::write(out.join("meta.json"), meta.to_string())
        })();
        if let Err(error) = made {
            eprintln!("imprint: {error}");
            return ExitCode::from(1);
        }
        say(
            &json!({"stage": "done", "percent": 100, "peakMb": 2100, "rtf": 1.2, "refSeconds": 6, "device": get("device")}),
        );
        ExitCode::SUCCESS
    }
}

/// The voice lab's install (voice_lab/engine_installer.rs) with nothing fetched.
mod engine {
    use super::*;

    /// `uv …`, written down in `calls`; `uv venv --python 3.11 <env>` makes `<env>/bin/python` — this stub again, with
    /// the `python-mode` beside uv, if any, beside it.
    pub fn uv(stub: &Stub, args: &[String], exe: &Path) -> ExitCode {
        stub.append("calls", &args.join(" "));
        if args.first().is_some_and(|arg| arg == "venv") {
            let Some(env) = args.get(3) else { return ExitCode::from(2) };
            let bin = Path::new(env).join("bin");
            let made = (|| -> std::io::Result<()> {
                std::fs::create_dir_all(&bin)?;
                std::fs::copy(exe, bin.join("python"))?;
                if cfg!(windows) {
                    std::fs::copy(exe, bin.join("python.exe"))?;
                }
                if stub.has("python-mode") {
                    std::fs::copy(stub.path("python-mode"), bin.join("python-mode"))?;
                }
                Ok(())
            })();
            if let Err(error) = made {
                eprintln!("uv venv: {error}");
                return ExitCode::from(1);
            }
        }
        ExitCode::SUCCESS
    }

    /// `python -c <code>`, written down in `calls`: the install's check (`import json, torch`) answers as on a Mac
    /// with MPS; the weights' download (`snapshot_download`) does what `python-mode` beside it says —
    /// - nothing: done at once, nothing fetched;
    /// - `hang <good>`: a hanging CDN — each start brings a little into its own `.<8 hex>.incomplete` (as
    ///   huggingface_hub 2.x does) and stands still; from the `good`-th start on it finishes. `$HF_HOME/../starts`
    ///   counts them;
    /// - `hung`: its pid into `$HF_HOME/../pid`, SIGTERM ignored, and a minute's wait.
    pub fn python(stub: &Stub, args: &[String]) -> ExitCode {
        stub.append("calls", &args.join(" "));
        let code = args.get(1).map(String::as_str).unwrap_or_default();
        if code.contains("snapshot_download") {
            return download(&stub.read("python-mode", ""));
        }
        if code.contains("import json, torch") {
            say(&json!({"stage": "done", "device": "mps", "torch": "2.8.0"}));
        }
        ExitCode::SUCCESS
    }

    fn download(mode: &str) -> ExitCode {
        let home = PathBuf::from(std::env::var("HF_HOME").unwrap_or_default());
        let _ = std::fs::create_dir_all(&home);
        let above = home.parent().map(Path::to_path_buf).unwrap_or_default();
        if mode == "hung" {
            let _ = std::fs::write(above.join("pid"), std::process::id().to_string());
            #[cfg(unix)]
            // SAFETY: SIG_IGN for SIGTERM changes only this process's disposition.
            unsafe {
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
            }
            std::thread::sleep(Duration::from_secs(60));
            return ExitCode::SUCCESS;
        }
        let Some(good) = mode.strip_prefix("hang ").and_then(|good| good.trim().parse::<u32>().ok()) else {
            return ExitCode::SUCCESS;
        };
        let starts = above.join("starts");
        let n = std::fs::read_to_string(&starts).ok().and_then(|text| text.trim().parse::<u32>().ok()).unwrap_or(0) + 1;
        let _ = std::fs::write(&starts, format!("{n}\n"));
        let blobs = home.join("hub").join("blobs");
        let _ = std::fs::create_dir_all(&blobs);
        if n >= good {
            let _ = std::fs::write(blobs.join("w"), vec![0u8; 4096]);
            return ExitCode::SUCCESS;
        }
        let _ = std::fs::write(blobs.join(format!("w.0a1b2c3{n}.incomplete")), vec![0u8; 2048]);
        std::thread::sleep(Duration::from_secs(30));
        ExitCode::SUCCESS
    }
}

/// A program of the voice lab's jobs (voice_lab/jobs.rs), its steps in its arguments, in order:
/// - `say:<line>` — the line on stdout;
/// - `sleep:<ms>` — a wait;
/// - `exit:<code>` — the end, with this code;
/// - `ignore-term` — SIGTERM ignored from now on (Unix; Windows has none);
/// - `forever:<line>:<ms>` — the line every `ms`, never ending; `grow:<step>:<ms>` — `{"memoryMb": n}` every `ms`, n
///   growing by `step`, never ending;
/// - `stdin-open:<ms>` — after `ms`, `{"stage":"done","open":…}`: whether its stdin is still open (nobody closed it).
mod job {
    use super::*;

    pub fn run(steps: &[String]) -> ExitCode {
        for step in steps {
            let (word, rest) = step.split_once(':').unwrap_or((step.as_str(), ""));
            let number = |text: &str| text.parse::<u64>().unwrap_or(0);
            match word {
                "say" => println!("{rest}"),
                "sleep" => std::thread::sleep(Duration::from_millis(number(rest))),
                "exit" => {
                    let _ = std::io::stdout().flush();
                    std::process::exit(rest.parse().unwrap_or(1));
                }
                "ignore-term" => {
                    #[cfg(unix)]
                    // SAFETY: SIG_IGN for SIGTERM changes only this process's disposition.
                    unsafe {
                        libc::signal(libc::SIGTERM, libc::SIG_IGN);
                    }
                }
                "forever" => {
                    let (line, every) = rest.rsplit_once(':').unwrap_or((rest, "1000"));
                    loop {
                        say_now(line);
                        std::thread::sleep(Duration::from_millis(number(every)));
                    }
                }
                "grow" => {
                    let (step, every) = rest.split_once(':').unwrap_or((rest, "20"));
                    let mut mb = 0;
                    loop {
                        mb += number(step);
                        say_now(&format!("{{\"memoryMb\": {mb}}}"));
                        std::thread::sleep(Duration::from_millis(number(every)));
                    }
                }
                "stdin-open" => {
                    let closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let seen = closed.clone();
                    std::thread::spawn(move || {
                        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
                        seen.store(true, std::sync::atomic::Ordering::SeqCst);
                    });
                    std::thread::sleep(Duration::from_millis(number(rest)));
                    let open = !closed.load(std::sync::atomic::Ordering::SeqCst);
                    say_now(&format!("{{\"stage\":\"done\",\"open\":{open}}}"));
                }
                other => {
                    eprintln!("job: no step «{other}»");
                    return ExitCode::from(2);
                }
            }
        }
        ExitCode::SUCCESS
    }

    fn say_now(line: &str) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}
