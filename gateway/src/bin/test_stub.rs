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
            eprintln!("duowave-test-stub: no role «{other}» (claude, acp-stub, python, uv, node, job, sleep)");
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
            "permissionMode": if mode == "bypass" { "bypassPermissions" } else { "acceptEdits" },
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
/// - `protocol` — the protocol version it answers to `initialize` (1 when absent);
/// - `mode` — ok (default): thinks and writes in pieces, a plan, an edit with a diff (and contacts.txt in the copy),
///   asks to run `npm test`, then `git push`, then to read a file outside the copy; a usage with a cost in dollars;
///   nocost: the same, its usage without a cost; slow: works until `session/cancel`; auth: the prompt answers
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
        };
        while let Some(message) = agent.receive() {
            let id = message.get("id").cloned();
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            match message.get("method").and_then(Value::as_str) {
                Some("initialize") => {
                    stub.append("calls.log", &format!("initialize {}", spaced(&params["clientCapabilities"])));
                    let version: u64 = stub.read("protocol", "1").parse().unwrap_or(1);
                    send(json!({"id": id, "result": {
                        "protocolVersion": version,
                        "agentCapabilities": {"loadSession": mode != "noload", "promptCapabilities": {}},
                        "authMethods": [], "agentInfo": {"name": "stub", "version": "0.1.0"},
                    }}));
                }
                Some("session/new") => {
                    agent.sessions += 1;
                    let session = format!("acp-{}-{}", std::process::id(), agent.sessions);
                    let cwd = params["cwd"].as_str().unwrap_or_default();
                    stub.append("calls.log", &format!("new {session} in {cwd}"));
                    send(json!({"id": id, "result": {"sessionId": session, "configOptions": [
                        {"id": "model", "name": "Model", "category": "model", "type": "select",
                            "currentValue": "default", "options": []},
                    ]}}));
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
                    stub.append("calls.log", &format!("model {}", params["value"].as_str().unwrap_or_default()));
                    send(json!({"id": id, "result": {"configOptions": []}}));
                }
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
            self.next_id += 1;
            let asked = self.next_id;
            let mut call = json!({"toolCallId": format!("perm-{asked}"), "title": title, "kind": kind,
                "status": "pending", "rawInput": raw});
            if let Some(locations) = locations {
                call["locations"] = locations;
            }
            send(json!({"id": asked, "method": "session/request_permission", "params": {
                "sessionId": session, "toolCall": call, "options": [
                    {"optionId": "once", "name": "Allow once", "kind": "allow_once"},
                    {"optionId": "always", "name": "Always allow", "kind": "allow_always"},
                    {"optionId": "no", "name": "Reject", "kind": "reject_once"},
                ],
            }}));
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
