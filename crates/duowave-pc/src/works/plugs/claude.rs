//! Claude Code run by hand as a work system (docs/plans/work-systems.md, stage 3): the sessions a person begins in a
//! terminal. DuoWave starts nothing here and changes nothing of Claude Code's own: it lists the sessions there are
//! (`claude agents --json`) and hears what the sessions report by its plugin's http hooks (events in): a prompt
//! (a work begins), a question of rights (asked aloud, the answer goes back as the hook's allow or deny), the end of a
//! turn (with words added «вдогонку» the hook says «go on» with them), a failure, the session's end. Without DuoWave
//! running, the hooks fail quietly and Claude Code works as it always did.
//!
//! Hook inputs are read defensively (docs: code.claude.com/docs/en/hooks, read 27.11.2026): `session_id`, `cwd`,
//! `hook_event_name`, `prompt`, `tool_name`, `tool_input`, `last_assistant_message`; anything else is passed over.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::talk::system::{
    About, Can, Expects, OpenIn, Outcome, Question, Refusal, Reply, Seen, Spend, Status, SystemEvent, SystemListener,
    SystemReady, SystemState, When, WorkSystem,
};

/// The system's id: the bundled card of Claude Code run by hand.
pub const CLAUDE_BY_HAND: &str = "claude-code";
/// How long `claude agents --json` may take.
const LIST_WAIT: Duration = Duration::from_secs(10);
/// How long a question of rights waits for the person before Claude Code asks in the terminal itself.
pub const ASK_WAIT: Duration = Duration::from_secs(120);
/// The summary of a turn's last message: its first sentences, at most this many characters; the rest is «подробнее».
const SUMMARY: usize = 300;
/// A title from a prompt: its start.
const TITLE: usize = 60;

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A session as its hooks told it.
#[derive(Debug, Clone)]
struct Heard {
    cwd: Option<String>,
    title: String,
    state: SystemState,
    line: Option<String>,
    last: Option<String>,
    at: u64,
}

/// A question of rights waiting for the person: its session, the question, the answer's way back to the hook.
type Waiting = (String, Question, oneshot::Sender<Reply>);

/// Where the program is now: its path the panel set, else `claude` on the search path.
pub type Program = Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>;

pub struct ClaudeByHand {
    program: Program,
    sessions: Mutex<HashMap<String, Heard>>,
    listeners: Mutex<Vec<SystemListener>>,
    /// Questions of rights waiting for the person, by the question's id.
    asked: Mutex<HashMap<String, Waiting>>,
    /// Words added after a turn, by session: given at its next `Stop`.
    queued: Mutex<HashMap<String, Vec<String>>>,
    next: AtomicU64,
    /// Folders whose sessions are DuoWave's own runs (its works' copies): not works begun outside (the hand check of
    /// 27.11 found «copy-bb» and «proba-b7» listed so).
    ours: Mutex<Vec<std::path::PathBuf>>,
}

/// A text's first `limit` characters, cut at a character.
fn short(text: &str, limit: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(limit) {
        Some((at, _)) => format!("{}…", text[..at].trim_end()),
        None => text.to_string(),
    }
}

/// A last message as a summary and the rest: its sentences up to `SUMMARY` characters.
fn summary_of(message: &str) -> (String, Option<String>) {
    let message = message.trim();
    if message.chars().count() <= SUMMARY {
        return (message.to_string(), None);
    }
    let mut cut = 0;
    for (at, character) in message.char_indices() {
        if message[..at].chars().count() > SUMMARY {
            break;
        }
        if matches!(character, '.' | '!' | '?' | '\n') {
            cut = at + character.len_utf8();
        }
    }
    if cut == 0 {
        return (short(message, SUMMARY), Some(message.to_string()));
    }
    (message[..cut].trim().to_string(), Some(message.to_string()))
}

/// What a tool asks to do, in a few words: the command, the file, the address.
fn asked_for(tool: &str, input: &Value) -> String {
    let field = |name: &str| input.get(name).and_then(Value::as_str).map(str::to_string);
    let what = field("command")
        .or_else(|| field("file_path"))
        .or_else(|| field("path"))
        .or_else(|| field("url"))
        .or_else(|| field("pattern"));
    match what {
        Some(what) => format!("{tool}: {}", short(&what, 120)),
        None => tool.to_string(),
    }
}

/// A command no voice approves (D3 of docs/plans/work-systems.md): sending code away, deleting, rights of the system.
fn dangerous(input: &Value) -> bool {
    let command = input.get("command").and_then(Value::as_str).unwrap_or_default().to_lowercase();
    ["git push", "rm -rf", "sudo ", "chmod ", "curl ", "wget ", "| sh", "|sh", "--force", "drop table"]
        .iter()
        .any(|word| command.contains(word))
}

fn now_ms() -> u64 {
    crate::utils::time::now_ms()
}

impl ClaudeByHand {
    pub fn new(program: Program) -> ClaudeByHand {
        ClaudeByHand {
            program,
            sessions: Mutex::new(HashMap::new()),
            listeners: Mutex::new(Vec::new()),
            asked: Mutex::new(HashMap::new()),
            queued: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            ours: Mutex::new(Vec::new()),
        }
    }

    fn emit(&self, event: SystemEvent) {
        let listeners: Vec<SystemListener> = locked(&self.listeners).clone();
        for listener in listeners {
            listener(CLAUDE_BY_HAND, event.clone());
        }
    }

    /// Sessions under this folder are DuoWave's own runs, not works begun outside.
    pub fn ours_under(&self, dir: std::path::PathBuf) {
        locked(&self.ours).push(dir);
    }

    fn is_ours(&self, cwd: Option<&str>) -> bool {
        let Some(cwd) = cwd else { return false };
        locked(&self.ours).iter().any(|dir| std::path::Path::new(cwd).starts_with(dir))
    }

    /// A hook's event as Claude Code posted it, and the answer to give back (JSON; `{}` — no decision). A question of
    /// rights waits up to `wait` for the person; then Claude Code asks in the terminal itself.
    pub async fn hook(&self, input: &Value, wait: Duration) -> Value {
        let text = |name: &str| input.get(name).and_then(Value::as_str).map(str::to_string);
        let Some(session) = text("session_id").filter(|session| !session.trim().is_empty()) else { return json!({}) };
        if self.is_ours(text("cwd").as_deref()) {
            return json!({});
        }
        let event = text("hook_event_name").unwrap_or_default();
        match event.as_str() {
            "SessionStart" => {
                let mut sessions = locked(&self.sessions);
                let heard = sessions.entry(session).or_insert_with(|| Heard {
                    cwd: None,
                    title: String::new(),
                    state: SystemState::Ready,
                    line: None,
                    last: None,
                    at: now_ms(),
                });
                heard.cwd = text("cwd").or(heard.cwd.take());
                json!({})
            }
            // A prompt begins a work: one for each turn the person starts.
            "UserPromptSubmit" => {
                let prompt = text("prompt").unwrap_or_default();
                // Claude Code's own turn — a background command ended (`<task-notification>`) — is not the person's: it
                // goes on in the work of the last prompt, and begins none (the hand check of 27.11 saw works so named).
                if prompt.trim_start().starts_with('<') {
                    if let Some(heard) = locked(&self.sessions).get_mut(&session) {
                        heard.state = SystemState::Working;
                        heard.at = now_ms();
                    }
                    return json!({});
                }
                let title = short(prompt.lines().next().unwrap_or_default(), TITLE);
                let cwd = text("cwd");
                {
                    let mut sessions = locked(&self.sessions);
                    let heard = sessions.entry(session.clone()).or_insert_with(|| Heard {
                        cwd: None,
                        title: String::new(),
                        state: SystemState::Working,
                        line: None,
                        last: None,
                        at: now_ms(),
                    });
                    heard.cwd = cwd.clone().or(heard.cwd.take());
                    heard.title = title.clone();
                    heard.state = SystemState::Working;
                    heard.line = None;
                    heard.at = now_ms();
                }
                let seen = Seen {
                    id: session,
                    title,
                    place: cwd,
                    state: SystemState::Working,
                    line: None,
                    at: Some(now_ms()),
                    outside: true,
                };
                self.emit(SystemEvent::Began { seen });
                json!({})
            }
            "PermissionRequest" => {
                let tool = text("tool_name").unwrap_or_else(|| "tool".into());
                let tool_input = input.get("tool_input").cloned().unwrap_or(Value::Null);
                let id = format!("q{}", self.next.fetch_add(1, Ordering::Relaxed));
                let question = Question {
                    id: id.clone(),
                    text: asked_for(&tool, &tool_input),
                    expects: Expects::YesNo,
                    dangerous: dangerous(&tool_input),
                };
                let (answer, answered) = oneshot::channel();
                locked(&self.asked).insert(id.clone(), (session.clone(), question.clone(), answer));
                self.emit(SystemEvent::Asks { work: session, question });
                let reply = tokio::time::timeout(wait, answered).await;
                locked(&self.asked).remove(&id);
                match reply {
                    Ok(Ok(Reply::Yes)) => json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest",
                        "decision": {"behavior": "allow"}}}),
                    Ok(Ok(_)) => json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest",
                        "decision": {"behavior": "deny", "message": "The person did not allow it (DuoWave)."}}}),
                    // No answer in time: Claude Code asks in the terminal, as without DuoWave.
                    _ => json!({}),
                }
            }
            // The turn ends: words added after it make it go on; else the work is ready, its last message the result.
            "Stop" => {
                let words = locked(&self.queued).remove(&session).filter(|words| !words.is_empty());
                if let Some(words) = words {
                    let reason = words.join("\n\n");
                    self.emit(SystemEvent::State {
                        work: session,
                        state: SystemState::Working,
                        line: Some(short(&reason, TITLE)),
                    });
                    return json!({"decision": "block", "reason": reason});
                }
                let last = text("last_assistant_message").unwrap_or_default();
                if let Some(heard) = locked(&self.sessions).get_mut(&session) {
                    heard.state = SystemState::Ready;
                    heard.last = Some(last.clone()).filter(|last| !last.trim().is_empty());
                    heard.at = now_ms();
                }
                let (summary, more) = summary_of(&last);
                let outcome = Outcome {
                    summary: Some(summary).filter(|summary| !summary.is_empty()),
                    more,
                    ..Outcome::default()
                };
                self.emit(SystemEvent::Ended { work: session, state: SystemState::Ready, outcome });
                json!({})
            }
            // A turn cut by an error: a limit or the money is «не доделано», the rest «не получилось».
            "StopFailure" => {
                let why = text("error").or_else(|| text("reason")).or_else(|| text("matcher")).unwrap_or_default();
                let limit = ["rate_limit", "billing_error", "quota"].iter().any(|word| why.contains(word));
                let state = if limit { SystemState::Unfinished } else { SystemState::Failed };
                if let Some(heard) = locked(&self.sessions).get_mut(&session) {
                    heard.state = state;
                }
                let outcome = Outcome { summary: Some(why).filter(|why| !why.is_empty()), ..Outcome::default() };
                self.emit(SystemEvent::Ended { work: session, state, outcome });
                json!({})
            }
            "SessionEnd" => {
                let going = locked(&self.sessions).remove(&session).is_some_and(|heard| heard.state.going());
                locked(&self.queued).remove(&session);
                if going {
                    self.emit(SystemEvent::Ended {
                        work: session,
                        state: SystemState::Stopped,
                        outcome: Outcome::default(),
                    });
                }
                json!({})
            }
            "Notification" => json!({}),
            other => {
                self.emit(SystemEvent::Unknown { work: Some(session), raw: other.to_string() });
                json!({})
            }
        }
    }

    /// The sessions Claude Code lists: interactive and background ones, begun by hand.
    async fn listed(&self) -> Vec<Value> {
        let Some(program) = (self.program)() else { return Vec::new() };
        let mut command = tokio::process::Command::new(program);
        command.args(["agents", "--json"]).kill_on_drop(true);
        let Ok(Ok(output)) = tokio::time::timeout(LIST_WAIT, command.output()).await else { return Vec::new() };
        if !output.status.success() {
            return Vec::new();
        }
        serde_json::from_slice::<Vec<Value>>(&output.stdout).unwrap_or_default()
    }
}

/// A listed session's state: `busy` works, `waiting` waits for the person, `idle` is ready for the next prompt.
fn listed_state(session: &Value) -> SystemState {
    match session.get("status").or_else(|| session.get("state")).and_then(Value::as_str) {
        Some("busy" | "working") => SystemState::Working,
        Some("waiting" | "blocked") => SystemState::WaitsPerson,
        Some("failed") => SystemState::Failed,
        Some("stopped") => SystemState::Stopped,
        _ => SystemState::Ready,
    }
}

#[async_trait]
impl WorkSystem for ClaudeByHand {
    fn id(&self) -> String {
        CLAUDE_BY_HAND.into()
    }

    fn about(&self) -> About {
        About {
            id: CLAUDE_BY_HAND.into(),
            title: "Claude Code".into(),
            names: vec!["Claude Code".into(), "клод код".into(), "клод".into()],
            plug: "hooks".into(),
            protocol: None,
            version: None,
        }
    }

    async fn ready(&self) -> SystemReady {
        if (self.program)().is_none() {
            return SystemReady::no(crate::tr!("Claude Code не найден на этом компьютере."));
        }
        SystemReady::yes()
    }

    fn can(&self) -> Can {
        Can {
            list: true,
            tell_after: true,
            questions: true,
            result: true,
            open: true,
            spend: Spend::None,
            ..Can::default()
        }
    }

    async fn list(&self) -> Vec<Seen> {
        let heard = locked(&self.sessions).clone();
        let mut seen: Vec<Seen> = self
            .listed()
            .await
            .into_iter()
            .filter(|session| !self.is_ours(session.get("cwd").and_then(Value::as_str)))
            .filter_map(|session| {
                let id = session.get("sessionId").and_then(Value::as_str)?.to_string();
                let known = heard.get(&id);
                Some(Seen {
                    title: known
                        .map(|known| known.title.clone())
                        .filter(|title| !title.is_empty())
                        .or_else(|| session.get("name").and_then(Value::as_str).map(str::to_string))
                        .unwrap_or_else(|| id.clone()),
                    place: session.get("cwd").and_then(Value::as_str).map(str::to_string),
                    state: listed_state(&session),
                    line: session.get("waitingFor").and_then(Value::as_str).map(str::to_string),
                    at: session.get("startedAt").and_then(Value::as_u64),
                    outside: true,
                    id,
                })
            })
            .collect();
        // Sessions heard by their hooks that the list does not show (it lists live ones only).
        for (id, known) in heard {
            if !seen.iter().any(|listed| listed.id == id) {
                seen.push(Seen {
                    id,
                    title: known.title,
                    place: known.cwd,
                    state: known.state,
                    line: known.line,
                    at: Some(known.at),
                    outside: true,
                });
            }
        }
        seen
    }

    async fn status(&self, work: &str) -> Option<Status> {
        if let Some(known) = locked(&self.sessions).get(work) {
            return Some(Status { state: known.state, line: known.line.clone(), at: Some(known.at) });
        }
        let listed = self.listed().await;
        let session = listed.iter().find(|session| session.get("sessionId").and_then(Value::as_str) == Some(work))?;
        Some(Status { state: listed_state(session), line: None, at: None })
    }

    async fn tell(&self, work: &str, text: &str, when: When) -> Result<(), Refusal> {
        if when == When::Now {
            return Err(Refusal::cannot("tell"));
        }
        if !locked(&self.sessions).contains_key(work) {
            return Err(Refusal::because(crate::tr!("Этой сессии Claude Code DuoWave не слышит.")));
        }
        locked(&self.queued).entry(work.to_string()).or_default().push(text.trim().to_string());
        Ok(())
    }

    async fn questions(&self, work: &str) -> Vec<Question> {
        locked(&self.asked)
            .values()
            .filter(|(session, _, _)| session == work)
            .map(|(_, question, _)| question.clone())
            .collect()
    }

    async fn answer(&self, _work: &str, question: &str, reply: Reply) -> Result<(), Refusal> {
        let waiting = locked(&self.asked).remove(question);
        // Who may answer, and how (a dangerous one only by a tap), the layer checks before (talk/systems.rs).
        let Some((_, _, answer)) = waiting else {
            return Err(Refusal::because(crate::tr!("Этот вопрос уже не ждёт ответа.")));
        };
        let _ = answer.send(reply);
        Ok(())
    }

    async fn result(&self, work: &str) -> Option<Outcome> {
        let last = locked(&self.sessions).get(work).and_then(|known| known.last.clone())?;
        let (summary, more) = summary_of(&last);
        Some(Outcome { summary: Some(summary), more, ..Outcome::default() })
    }

    fn open(&self, work: &str) -> Option<OpenIn> {
        let cwd = locked(&self.sessions).get(work).and_then(|known| known.cwd.clone());
        let resume = format!("claude --resume {work}");
        let command = match cwd {
            Some(cwd) => format!("cd {} && {resume}", crate::works::harness::claude_code::quoted(&cwd)),
            None => resume,
        };
        Some(OpenIn { command: Some(command), url: None })
    }

    fn listen(&self, events: SystemListener) {
        locked(&self.listeners).push(events);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn heard(system: &ClaudeByHand) -> Arc<Mutex<Vec<SystemEvent>>> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let kept = events.clone();
        system.listen(Arc::new(move |_: &str, event: SystemEvent| kept.lock().unwrap().push(event)));
        events
    }

    fn hook(event: &str, more: Value) -> Value {
        let mut input = json!({"session_id": "s1", "cwd": "/work/site", "hook_event_name": event});
        if let (Some(input), Some(more)) = (input.as_object_mut(), more.as_object()) {
            input.extend(more.clone());
        }
        input
    }

    /// A session begun in a terminal: its prompt begins a work begun outside; a question of rights waits for the
    /// person and goes back as the hook's allow; words added after the turn make the turn go on; then the end comes
    /// with the last message as the summary.
    #[tokio::test]
    async fn a_session_by_hand_reports_by_its_hooks_and_takes_answers_and_words() {
        let system = Arc::new(ClaudeByHand::new(Arc::new(|| None)));
        let events = heard(&system);
        system.hook(&hook("SessionStart", json!({"source": "startup"})), ASK_WAIT).await;
        system.hook(&hook("UserPromptSubmit", json!({"prompt": "Поправь опечатку в README"})), ASK_WAIT).await;
        let began = events.lock().unwrap().clone();
        assert!(
            matches!(&began[..], [SystemEvent::Began { seen }] if seen.id == "s1" && seen.outside && seen.title == "Поправь опечатку в README"),
            "{began:?}"
        );
        // A question of rights: answered «да» while the hook waits.
        let asking = {
            let system = system.clone();
            tokio::spawn(async move {
                let input =
                    hook("PermissionRequest", json!({"tool_name": "Bash", "tool_input": {"command": "npm test"}}));
                system.hook(&input, Duration::from_secs(5)).await
            })
        };
        let question = loop {
            let asked = system.questions("s1").await;
            if let Some(question) = asked.into_iter().next() {
                break question;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(question.text, "Bash: npm test");
        assert!(!question.dangerous);
        system.answer("s1", &question.id, Reply::Yes).await.unwrap();
        let answer = asking.await.unwrap();
        assert_eq!(answer["hookSpecificOutput"]["decision"]["behavior"], "allow");
        // No answer in time: no decision, Claude Code asks in its terminal.
        let input = hook("PermissionRequest", json!({"tool_name": "Bash", "tool_input": {"command": "git push"}}));
        assert_eq!(system.hook(&input, Duration::from_millis(50)).await, json!({}));
        // Words after the turn: the turn goes on with them.
        system.tell("s1", "и в CHANGELOG тоже", When::After).await.unwrap();
        assert!(system.tell("s1", "сейчас", When::Now).await.unwrap_err().cannot);
        let stop = system.hook(&hook("Stop", json!({"last_assistant_message": "Поправил."})), ASK_WAIT).await;
        assert_eq!(stop, json!({"decision": "block", "reason": "и в CHANGELOG тоже"}));
        // The next end is the work's end, with its last message.
        let last = "Поправил опечатку в README и в CHANGELOG. Проверил сборку: всё собирается.";
        let stop = system.hook(&hook("Stop", json!({"last_assistant_message": last})), ASK_WAIT).await;
        assert_eq!(stop, json!({}));
        let ended = events.lock().unwrap().last().cloned().unwrap();
        assert!(
            matches!(&ended, SystemEvent::Ended { work, state: SystemState::Ready, outcome } if work == "s1" && outcome.summary.as_deref() == Some(last)),
            "{ended:?}"
        );
        assert_eq!(system.result("s1").await.unwrap().summary.as_deref(), Some(last));
        assert_eq!(system.open("s1").unwrap().command.as_deref(), Some("cd /work/site && claude --resume s1"));
        // Claude Code's own turn — a background command ended — begins no work.
        let before = events.lock().unwrap().len();
        system.hook(&hook("UserPromptSubmit", json!({"prompt": "<task-notification> done"})), ASK_WAIT).await;
        assert_eq!(events.lock().unwrap().len(), before, "no work of its own");
        // A limit cuts a turn: not finished.
        system.hook(&hook("UserPromptSubmit", json!({"prompt": "Ещё одно"})), ASK_WAIT).await;
        system.hook(&hook("StopFailure", json!({"error": "rate_limit"})), ASK_WAIT).await;
        let cut = events.lock().unwrap().last().cloned().unwrap();
        assert!(matches!(cut, SystemEvent::Ended { state: SystemState::Unfinished, .. }), "{cut:?}");
    }

    /// A dangerous command is marked so (only a tap approves it); a long last message is cut to its first sentences.
    #[test]
    fn a_dangerous_command_is_marked_and_a_long_message_is_cut_at_a_sentence() {
        assert!(dangerous(&json!({"command": "git push origin HEAD"})));
        assert!(!dangerous(&json!({"command": "npm test"})));
        let long = format!("{} Второе предложение.", "Первое предложение. ".repeat(20));
        let (summary, more) = summary_of(&long);
        assert!(summary.ends_with('.') && summary.chars().count() <= SUMMARY, "{summary}");
        assert_eq!(more.as_deref(), Some(long.trim()));
    }

    /// Its sessions as Claude Code lists them, also those its hooks did not tell (begun before the plugin).
    #[tokio::test]
    async fn it_lists_the_sessions_claude_code_lists() {
        let dir = tempfile::tempdir().unwrap();
        let program = crate::utils::test_stub::put(dir.path(), "claude");
        std::fs::write(
            dir.path().join("agents.json"),
            r#"[{"pid": 7, "cwd": "/work/site", "kind": "interactive", "startedAt": 1764200000000,
                "sessionId": "s-hand", "name": "site-3f", "status": "busy"},
               {"pid": 8, "cwd": "/work/blog", "kind": "interactive", "startedAt": 1764200000001,
                "sessionId": "s-idle", "name": "blog-1a", "status": "idle"}]"#,
        )
        .unwrap();
        let system = ClaudeByHand::new(Arc::new(move || Some(program.clone())));
        assert!(system.ready().await.ready);
        let listed = system.list().await;
        let busy = listed.iter().find(|seen| seen.id == "s-hand").unwrap();
        assert_eq!((busy.title.as_str(), busy.state, busy.outside), ("site-3f", SystemState::Working, true));
        assert_eq!(busy.place.as_deref(), Some("/work/site"));
        let idle = listed.iter().find(|seen| seen.id == "s-idle").unwrap();
        assert_eq!(idle.state, SystemState::Ready);
        assert_eq!(crate::talk::system::check(&system, None).await, Vec::<String>::new());
        // DuoWave's own runs in its works' copies are not works begun outside (the hand check of 27.11).
        system.ours_under(std::path::PathBuf::from("/work/blog"));
        assert!(system.list().await.iter().all(|seen| seen.id != "s-idle"));
    }
}
