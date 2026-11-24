//! MCP servers as programs on this computer — npx, uvx and the like (docs/plans/mcp.md), each in its own process group
//! so everything it starts goes with it: the computer's runner behind «Приложения»'s socket (`mcp_connection::Programs`).

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;

use rmcp::ClientLifecycleMode;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

use futures::future::BoxFuture;

use crate::mcp::mcp_connection::{negotiated, over, ConnectError, ConnectionSpec, LogSink, Opened, RunningProgram};

/// What a program is given of the gateway's environment: enough to run, nothing of the gateway's own.
#[cfg(unix)]
const INHERITED: &[&str] = &["HOME", "LOGNAME", "PATH", "SHELL", "TERM", "USER"];
#[cfg(windows)]
const INHERITED: &[&str] = &[
    "APPDATA",
    "HOMEDRIVE",
    "HOMEPATH",
    "LOCALAPPDATA",
    "PATH",
    "PROCESSOR_ARCHITECTURE",
    "SYSTEMDRIVE",
    "SYSTEMROOT",
    "TEMP",
    "USERNAME",
    "USERPROFILE",
    "PROGRAMFILES",
];
#[cfg(not(any(unix, windows)))]
const INHERITED: &[&str] = &["PATH"];

/// How long a program has to end on its own once its input closes, then after SIGTERM.
const END_WAIT: Duration = Duration::from_secs(2);
#[cfg(unix)]
const TERM_WAIT: Duration = Duration::from_millis(1500);

/// The program's environment: the safe set, PATH with where npx and uvx usually live (a gateway started without a
/// login shell's PATH), then its own variables.
pub(super) fn environment(own: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = INHERITED
        .iter()
        .filter_map(|name| std::env::var(name).ok().map(|value| (name.to_string(), value)))
        // A shell function exported to the environment is not a value.
        .filter(|(_, value)| !value.starts_with("()"))
        .collect();
    #[cfg(unix)]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        let mut path: Vec<String> = env
            .get("PATH")
            .map(|path| path.split(':').filter(|p| !p.is_empty()).map(String::from).collect())
            .unwrap_or_default();
        for extra in ["/opt/homebrew/bin".to_string(), "/usr/local/bin".to_string(), format!("{home}/.local/bin")] {
            if !path.contains(&extra) {
                path.push(extra);
            }
        }
        env.insert("PATH".into(), path.join(":"));
    }
    env.extend(own.iter().map(|(name, value)| (name.clone(), value.clone())));
    env
}

/// The longest line of stderr kept (its start): the card shows 300 characters of it.
const LOG_LINE: usize = 64 * 1024;

/// A program's stderr into its log, line by line, to the end of the stream. Bytes that are not UTF-8 read as
/// U+FFFD, as Node's `toString('utf8')` read them: reading never stops before the end, because a pipe nobody
/// reads closes, and the program's next write to it gets EPIPE or kills it (SIGPIPE).
async fn read_log(stderr: impl AsyncRead + Unpin, log: LogSink) {
    let say = |line: &[u8]| {
        let line = String::from_utf8_lossy(line);
        let line = line.trim();
        if !line.is_empty() {
            log(line.chars().take(300).collect());
        }
    };
    let mut reader = BufReader::new(stderr);
    let mut line: Vec<u8> = Vec::new();
    loop {
        let chunk = match reader.fill_buf().await {
            Ok(chunk) => chunk,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        if chunk.is_empty() {
            say(&line);
            break;
        }
        let (taken, ended) = match chunk.iter().position(|&byte| byte == b'\n') {
            Some(newline) => (newline + 1, true),
            None => (chunk.len(), false),
        };
        let room = LOG_LINE.saturating_sub(line.len());
        line.extend_from_slice(&chunk[..taken.min(room)]);
        reader.consume(taken);
        if ended {
            say(&line);
            line.clear();
        }
    }
}

struct Program {
    child: tokio::process::Child,
    #[cfg(unix)]
    group: Option<i32>,
}

impl RunningProgram for Program {
    fn stop(self: Box<Self>) -> BoxFuture<'static, ()> {
        Box::pin(self.end())
    }
}

impl Program {
    /// The program ends on its own when its input closes; whatever is left of its group gets SIGTERM, then SIGKILL.
    async fn end(mut self: Box<Self>) {
        let ended = tokio::time::timeout(END_WAIT, self.child.wait()).await.is_ok();
        #[cfg(unix)]
        if let Some(group) = self.group {
            signal_group(group, libc::SIGTERM);
            let deadline = tokio::time::Instant::now() + TERM_WAIT;
            while tokio::time::Instant::now() < deadline {
                let _ = self.child.try_wait();
                if !group_alive(group) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            signal_group(group, libc::SIGKILL);
        }
        if !ended {
            let _ = self.child.start_kill();
        }
        let _ = self.child.wait().await;
    }
}

#[cfg(unix)]
fn signal_group(group: i32, signal: i32) {
    // SAFETY: killpg has no memory effects; a group that is gone answers ESRCH, which is fine here.
    unsafe {
        libc::killpg(group, signal);
    }
}

#[cfg(unix)]
fn group_alive(group: i32) -> bool {
    // SAFETY: signal 0 only asks whether the group has members.
    unsafe { libc::killpg(group, 0) == 0 }
}

async fn open_program(spec: &ConnectionSpec, log: LogSink) -> Opened {
    let command =
        spec.command.as_deref().filter(|c| !c.is_empty()).ok_or_else(|| ConnectError::Other("no command".into()))?;
    if let Some(cwd) = &spec.cwd {
        std::fs::create_dir_all(cwd).map_err(|error| ConnectError::Other(format!("{}: {error}", cwd.display())))?;
    }
    match start(spec, command, &log, negotiated()).await {
        // A server that ends at the probe (it takes nothing before `initialize`): once more, the old handshake.
        Err(ConnectError::Closed(_)) => start(spec, command, &log, ClientLifecycleMode::Initialize).await,
        other => other,
    }
}

async fn start(spec: &ConnectionSpec, command: &str, log: &LogSink, lifecycle: ClientLifecycleMode) -> Opened {
    let mut process = tokio::process::Command::new(command);
    process
        .args(&spec.args)
        .env_clear()
        .envs(environment(&spec.env))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = &spec.cwd {
        process.current_dir(cwd);
    }
    #[cfg(unix)]
    process.process_group(0);
    let mut child = process.spawn().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => ConnectError::NotFound(command.to_string()),
        _ => ConnectError::Other(format!("{command}: {error}")),
    })?;
    #[cfg(unix)]
    let group = child.id().and_then(|pid| i32::try_from(pid).ok());
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.start_kill();
        return Err(ConnectError::Other("no pipes to the program".into()));
    };
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(read_log(stderr, log.clone()));
    }
    let program = Program {
        child,
        #[cfg(unix)]
        group,
    };
    over((stdout, stdin), lifecycle, Box::new(program)).await
}

/// The computer's runner of programs (`duowave_pc::install` gives it to the connections).
pub struct OnThisComputer;

impl crate::mcp::mcp_connection::Programs for OnThisComputer {
    fn open<'a>(&'a self, spec: &'a ConnectionSpec, log: LogSink) -> BoxFuture<'a, Opened> {
        Box::pin(open_program(spec, log))
    }
}

/// Whether the process `pid` is still there: the program tests (the program's too) see a program end with its card.
#[cfg(all(any(test, feature = "testing"), unix))]
pub fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only asks whether the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Whether the process `pid` is still there (Windows: `tasklist`).
#[cfg(all(any(test, feature = "testing"), not(unix)))]
pub fn alive(pid: i32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).split_whitespace().any(|word| word == pid.to_string()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn sink() -> (LogSink, Arc<Mutex<Vec<String>>>) {
        let said = Arc::new(Mutex::new(Vec::new()));
        let heard = said.clone();
        (Arc::new(move |line: String| heard.lock().unwrap().push(line)), said)
    }

    #[tokio::test]
    async fn reads_a_programs_stderr_to_its_end_bytes_that_are_not_utf8_as_replacement_characters() {
        let (log, said) = sink();
        let long = format!("{}\n", "я".repeat(100_000));
        let stream =
            [b"first\n bad \xff byte\n\n".as_slice(), long.as_bytes(), b"caf\xe9 au lait\nlast words"].concat();
        read_log(stream.as_slice(), log).await;
        let said = said.lock().unwrap();
        assert_eq!(said[..2], ["first", "bad \u{fffd} byte"]);
        assert_eq!(said[2], "я".repeat(300), "a long line is kept to its start");
        assert_eq!(said[3..], ["caf\u{fffd} au lait", "last words"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_program_that_writes_bytes_that_are_not_utf8_to_stderr_keeps_running() {
        let (log, said) = sink();
        let mut child = tokio::process::Command::new("sh")
            .args(["-c", r"printf 'caf\351\n' >&2; sleep 0.2; printf 'still here\n' >&2"])
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let reading = tokio::spawn(read_log(child.stderr.take().unwrap(), log));
        let status = child.wait().await.unwrap();
        reading.await.unwrap();
        // A closed pipe would have killed it at its second line (SIGPIPE).
        assert!(status.success(), "{status:?}");
        assert_eq!(*said.lock().unwrap(), ["caf\u{fffd}", "still here"]);
    }

    /// Windows: the same through PowerShell — raw bytes not UTF-8 (Windows-1252's «é»), then a line after it.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_program_that_writes_bytes_that_are_not_utf8_to_stderr_keeps_running_on_windows() {
        let (log, said) = sink();
        let script = "$e = [Console]::OpenStandardError(); $e.Write([byte[]](99,97,102,233,10), 0, 5); $e.Flush(); \
                      Start-Sleep -Milliseconds 200; [Console]::Error.WriteLine('still here')";
        let mut child = tokio::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let reading = tokio::spawn(read_log(child.stderr.take().unwrap(), log));
        let status = child.wait().await.unwrap();
        reading.await.unwrap();
        assert!(status.success(), "{status:?}");
        assert_eq!(*said.lock().unwrap(), ["caf\u{fffd}", "still here"]);
    }

    #[test]
    fn gives_a_program_the_safe_set_and_its_own_variables_only() {
        let own = BTreeMap::from([("TOKEN".to_string(), "secret".to_string())]);
        let env = environment(&own);
        assert_eq!(env.get("TOKEN").map(String::as_str), Some("secret"));
        assert!(env.keys().all(|key| INHERITED.contains(&key.as_str()) || key == "TOKEN"), "{env:?}");
        #[cfg(unix)]
        assert!(env["PATH"].split(':').any(|p| p == "/usr/local/bin"));
    }

    // ── The service with programs: a card of its own, its variables, its folder, its end ──

    use std::path::PathBuf;
    use std::time::Duration;

    use serde_json::{json, Value};
    use tokio_util::sync::CancellationToken;

    use crate::mcp::fixtures;
    use crate::mcp::mcp_import::Json;
    use crate::mcp::mcp_presets::TransportKind;
    use crate::mcp::mcp_service::tests::*;
    use crate::mcp::mcp_service::*;

    /// The fixture programs of this test binary: run again by their names here (mcp/fixtures.rs serves).
    const LEGACY_PROGRAM: &str = "mcp::programs::tests::stdio_legacy";
    const MODERN_PROGRAM: &str = "mcp::programs::tests::stdio_modern";
    const STRICT_PROGRAM: &str = "mcp::programs::tests::stdio_strict";

    #[test]
    #[ignore = "a fixture: the MCP tests run it as a program"]
    fn stdio_legacy() {
        fixtures::legacy_as(LEGACY_PROGRAM);
    }

    #[test]
    #[ignore = "a fixture: the MCP tests run it as a program"]
    fn stdio_modern() {
        fixtures::modern_as(MODERN_PROGRAM);
    }

    #[test]
    #[ignore = "a fixture: the MCP tests run it as a program"]
    fn stdio_strict() {
        fixtures::strict_as(STRICT_PROGRAM);
    }

    /// The service of the tests, with this computer's programs.
    fn setup() -> Setup {
        crate::install();
        crate::mcp::mcp_service::tests::setup()
    }

    fn program(env: Value) -> McpServerInput {
        let (command, args) = fixtures::program(LEGACY_PROGRAM);
        input(json!({ "transport": "stdio", "title": "Fixture", "command": command, "args": args, "env": env }))
    }

    #[tokio::test]
    async fn connects_a_program_of_the_old_protocol_lists_both_pages_of_its_tools_and_keeps_them_all_off_until_the_owner_turns_them_on(
    ) {
        let s = setup();
        let server = s.mcp.add(&program(json!({ "FIXTURE_VAR": "fixture-value-1234" })), None).unwrap();
        assert_eq!(server.id, "fixture");
        let secret = json!([{ "kind": "env", "name": "FIXTURE_VAR", "set": true, "hint": "…1234" }]);
        keeps_tools_off_until_turned_on(&s.mcp, &server.id, secret).await;
        s.mcp.shutdown().await;
    }

    #[tokio::test]
    async fn gives_the_program_only_its_own_variables_and_its_own_folder_and_stops_it_with_what_it_started() {
        let s = setup();
        let server = s.mcp.add(&program(json!({ "FIXTURE_VAR": "fixture-value-1234" })), None).unwrap();
        ready(&s.mcp, &server.id).await;
        let seen: Value =
            serde_json::from_str(&s.mcp.try_tool(&server.id, "env", None).await.unwrap().content).unwrap();
        assert_eq!(seen["FIXTURE_VAR"], json!("fixture-value-1234"));
        let keys: Vec<String> = serde_json::from_value(seen["keys"].clone()).unwrap();
        // The safe set is each system's own (mcp_connection.rs, INHERITED): Windows' has APPDATA, SYSTEMROOT and the
        // like, without which a program there does not start.
        let own: &[&str] = if cfg!(windows) {
            &[
                "APPDATA",
                "HOMEDRIVE",
                "HOMEPATH",
                "LOCALAPPDATA",
                "PATH",
                "PROCESSOR_ARCHITECTURE",
                "SYSTEMDRIVE",
                "SYSTEMROOT",
                "TEMP",
                "USERNAME",
                "USERPROFILE",
                "PROGRAMFILES",
                "FIXTURE_VAR",
            ]
        } else {
            &["HOME", "LOGNAME", "PATH", "SHELL", "TERM", "USER", "FIXTURE_VAR"]
        };
        // Nothing of the gateway's own environment: what the program sees beyond its own and the safe set, the
        // gateway does not have (the system gives every process a few).
        for key in &keys {
            assert!(
                own.iter().any(|own| own.eq_ignore_ascii_case(key))
                    || std::env::var_os(key).is_none()
                    || key.starts_with("__CF"),
                "{key} leaked"
            );
        }
        let place: Value =
            serde_json::from_str(&s.mcp.try_tool(&server.id, "where", None).await.unwrap().content).unwrap();
        let folder = s.dir.path().join("mcp").join(&server.id);
        // The same place, however it is spelt: Windows gives the program the temp folder's short name (RUNNER~1),
        // canonicalize its long one with `\\?\` (CI 0.18.1).
        let cwd = place["cwd"].as_str().unwrap_or_default();
        assert_eq!(std::fs::canonicalize(cwd).unwrap(), std::fs::canonicalize(&folder).unwrap(), "{place}");
        let pid: i32 = std::fs::read_to_string(folder.join("fixture.pid")).unwrap().trim().parse().unwrap();
        assert!(alive(pid));
        s.mcp.remove(&server.id, None).await.unwrap();
        until("the program stopped", || !alive(pid), Duration::from_secs(5)).await;
        assert!(s.secrets.ids(&format!("mcp:{}:", server.id)).is_empty());
        s.mcp.shutdown().await;
    }

    #[tokio::test]
    async fn turns_a_tool_off_when_the_server_changes_its_description_until_the_owner_approves_it_again() {
        let s = setup();
        let server = s.mcp.add(&program(json!({})), None).unwrap();
        turns_a_changed_tool_off(&s.mcp, &server.id).await;
        s.mcp.shutdown().await;
    }

    #[tokio::test]
    async fn says_a_failure_and_a_dead_server_as_errors_the_model_reads_and_reconnects_after_a_crash() {
        let s = setup();
        let server = s.mcp.add(&program(json!({})), None).unwrap();
        ready(&s.mcp, &server.id).await;
        for name in ["fail", "crash", "echo"] {
            s.mcp.set_tool(&server.id, name, &change(json!({ "on": true })), None).unwrap();
        }
        let offer = s.mcp.offer(&admin(), by_voice()).unwrap();
        let never = CancellationToken::new();
        assert_eq!(
            offer.run(&call("mcp_fixture__fail", json!({})), &never, None).await,
            Ok(ToolOutcome { content: "boom".into(), is_error: true, ..ToolOutcome::default() })
        );
        let crashed = offer.run(&call("mcp_fixture__crash", json!({})), &never, None).await.unwrap();
        assert!(crashed.is_error);
        assert!(crashed.content.starts_with("error: the server did not answer"), "{}", crashed.content);
        until("the server is down", || state(&s.mcp, &server.id).state == ServerState::Error, Duration::from_secs(10))
            .await;
        assert_eq!(state(&s.mcp, &server.id).detail.as_deref(), Some("соединение закрылось"));
        s.mcp.shutdown().await;
    }

    #[tokio::test]
    async fn finds_a_program_of_the_2026_07_28_protocol_through_server_discover() {
        let s = setup();
        let (command, args) = fixtures::program(MODERN_PROGRAM);
        let server = s
            .mcp
            .add(&input(json!({ "transport": "stdio", "title": "Modern", "command": command, "args": args })), None)
            .unwrap();
        finds_the_modern_server(&s.mcp, &server.id).await;
        s.mcp.shutdown().await;
    }

    #[tokio::test]
    async fn connects_a_program_that_ends_at_the_probe_with_the_old_handshake() {
        let s = setup();
        let (command, args) = fixtures::program(STRICT_PROGRAM);
        let given = json!({ "transport": "stdio", "title": "Fixture", "command": command, "args": args });
        let server = s.mcp.add(&input(given), None).unwrap();
        ready(&s.mcp, &server.id).await;
        assert_eq!(names(&state(&s.mcp, &server.id)), ALL_TOOLS);
        s.mcp.shutdown().await;
    }

    #[tokio::test]
    async fn reads_a_pasted_config_programs_and_addresses_their_variables_and_headers_sealed() {
        let s = setup();
        let (command, args) = fixtures::program(LEGACY_PROGRAM);
        let pasted = json!({
            "mcpServers": {
                "fixture": { "command": command, "args": args, "env": { "FIXTURE_VAR": "pasted-secret-9999" } },
                "remote": { "url": "http://127.0.0.1:9/mcp", "headers": { "Authorization": "Bearer pasted-token-5555" } },
                "broken": { "args": ["x"] },
            },
        });
        let result = s.mcp.import_json(Some(&Json::String(pasted.to_string())), None).unwrap();
        assert_eq!(
            result.added.iter().map(|server| (server.id.as_str(), server.transport)).collect::<Vec<_>>(),
            [("fixture", TransportKind::Stdio), ("remote", TransportKind::Http)]
        );
        assert_eq!(result.problems, ["«broken»: нет ни command, ни url."]);
        let shown = serde_json::to_string(&s.mcp.view()).unwrap();
        assert!(!shown.contains("pasted-secret") && !shown.contains("pasted-token"));
        let file = s.dir.path().join(crate::storage::db::DB_FILE);
        assert!(file.exists());
        for path in [file.clone(), PathBuf::from(format!("{}-wal", file.display()))] {
            if let Ok(bytes) = std::fs::read(&path) {
                assert!(
                    !String::from_utf8_lossy(&bytes).contains("pasted-secret"),
                    "{} holds the secret",
                    path.display()
                );
            }
        }
        s.mcp.shutdown().await;
    }
}
