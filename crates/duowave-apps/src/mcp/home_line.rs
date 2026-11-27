//! `/mcp/home` (docs/plans/own-agent.md, stage 1; docs/plans/work-systems.md, «Plugs», 4: the second entry of events
//! in): DuoWave's own MCP server for an agent or a work system on this computer. JSON-RPC over HTTP, one answer to one
//! message, as `/mcp/plugins`. Tools: `say`, `ask_yes_no`, `notify`, `desk_card`, `task_step`, each by a grant of the
//! caller's `vgw_` key (`agent.say`, `agent.ask`, `agent.notify`, `agent.desk`), checked in the gateway on every call;
//! a refusal comes back in words.
//!
//! An agent's own tasks and its questions are works of a system of its own (`agent-<key>`), so «Работы», «как там» and
//! a question by voice are the works' own: `desk_card` begins a work begun outside (quiet at its end, D1),
//! `task_step` is its step or its end, and `ask_yes_no` is the work's question, answered by the home's admin. While
//! «Не беспокоить» is on, nothing is said aloud: a question goes to the phone and waits for its answer in «Работы».

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use futures::future::BoxFuture;
use serde_json::{json, Map, Value};
use tokio::sync::oneshot;

use crate::core::identity::permissions::{title_of, AGENT_ASK, AGENT_DESK, AGENT_NOTIFY, AGENT_SAY};
use crate::core::identity::Principal;
use crate::platform::http::{ApiError, Body, Caller, Provides, RouteState, Uses};
use crate::talk::system::{
    About, Can, Expects, Outcome, Question, Refusal, Reply, Seen, Status, SystemEvent, SystemListener, SystemReady,
    SystemState, WorkSystem,
};
use crate::talk::systems::Systems;

pub const PATH: &str = "/mcp/home";
/// How long a question waits for its answer when the agent does not say, and at most.
const ASK_WAIT: Duration = Duration::from_secs(60);
const ASK_MAX: Duration = Duration::from_secs(300);
/// The most of a line said or written.
const TEXT_MAX: usize = 500;

/// Says a line on a device (`*`: wherever a station is): whether it was said.
pub type Speak = Arc<dyn Fn(String, String) -> BoxFuture<'static, bool> + Send + Sync>;
/// A message to the owner's phone.
pub type Notify = Arc<dyn Fn(String) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;
/// «Не беспокоить» is on: nothing is said aloud.
pub type Quiet = Arc<dyn Fn() -> bool + Send + Sync>;

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The home line: the tools, and the system of each key that calls it.
pub struct HomeLine {
    systems: Arc<Systems>,
    speak: Speak,
    notify: Notify,
    quiet: Quiet,
    lines: Mutex<HashMap<String, Arc<AgentLine>>>,
}

/// A tool's grant.
fn grant_of(tool: &str) -> Option<&'static str> {
    match tool {
        "say" => Some(AGENT_SAY),
        "ask_yes_no" => Some(AGENT_ASK),
        "notify" => Some(AGENT_NOTIFY),
        "desk_card" | "task_step" => Some(AGENT_DESK),
        _ => None,
    }
}

impl HomeLine {
    pub fn new(systems: Arc<Systems>, speak: Speak, notify: Notify, quiet: Quiet) -> Arc<HomeLine> {
        Arc::new(HomeLine { systems, speak, notify, quiet, lines: Mutex::new(HashMap::new()) })
    }

    /// The system of this caller's key: made at its first call, in the registry from then on.
    fn line_of(&self, principal: &Principal) -> Arc<AgentLine> {
        let mut lines = locked(&self.lines);
        if let Some(line) = lines.get(&principal.id) {
            return line.clone();
        }
        let title = Some(principal.name.trim()).filter(|name| !name.is_empty()).unwrap_or("Агент");
        let line = Arc::new(AgentLine::new(&format!("agent-{}", principal.id), title));
        self.systems.add(line.clone());
        lines.insert(principal.id.clone(), line.clone());
        line
    }

    /// The tools this caller may use, as `tools/list` gives them.
    pub fn tools(&self, principal: &Principal) -> Vec<Value> {
        let text = |description: &str| json!({ "type": "string", "description": description });
        let all = [
            json!({ "name": "say", "description": "Say a line aloud in the home.",
                "inputSchema": { "type": "object", "required": ["text"], "properties": {
                    "text": text("What to say, one or two sentences."),
                    "room": text("A device or room; none: wherever a station is.") } } }),
            json!({ "name": "ask_yes_no", "description": "Ask the person a yes or no question by voice; the answer is yes, no or silence.",
                "inputSchema": { "type": "object", "required": ["question"], "properties": {
                    "question": text("The question, one sentence."),
                    "task": text("The task it is about, as in desk_card."),
                    "room": text("A device or room."),
                    "wait": { "type": "integer", "description": "Seconds to wait for the answer, 5 to 300." } } } }),
            json!({ "name": "notify", "description": "Send a message to the owner's phone.",
                "inputSchema": { "type": "object", "required": ["text"], "properties": { "text": text("The message.") } } }),
            json!({ "name": "desk_card", "description": "Show a task you began yourself in DuoWave's works, or change its line.",
                "inputSchema": { "type": "object", "required": ["task", "title"], "properties": {
                    "task": text("Your id of the task."),
                    "title": text("Its title, a few words."),
                    "line": text("What it is at now.") } } }),
            json!({ "name": "task_step", "description": "A step of a task shown by desk_card; a state other than working ends it.",
                "inputSchema": { "type": "object", "required": ["task", "line"], "properties": {
                    "task": text("Your id of the task."),
                    "line": text("The step, or the result at its end."),
                    "state": { "type": "string", "enum": ["working", "ready", "unfinished", "failed", "stopped"] } } } }),
        ];
        all.into_iter()
            .filter(|tool| {
                let name = tool["name"].as_str().unwrap_or_default();
                grant_of(name).is_some_and(|grant| crate::core::identity::can(Some(principal), grant))
            })
            .collect()
    }

    /// A tool called by this caller: its answer, or why not in words.
    pub async fn call(&self, principal: &Principal, tool: &str, args: &Map<String, Value>) -> Result<String, String> {
        let grant = grant_of(tool).ok_or_else(|| crate::tr!("Такого инструмента нет: {tool}.", tool = tool))?;
        if !crate::core::identity::can(Some(principal), grant) {
            return Err(crate::tr!("У этого ключа нет права «{right}».", right = title_of(grant)));
        }
        let text = |key: &str| {
            args.get(key)
                .and_then(Value::as_str)
                .map(|text| text.trim().chars().take(TEXT_MAX).collect::<String>())
                .filter(|text| !text.is_empty())
        };
        let needed = |key: &str| text(key).ok_or_else(|| crate::tr!("Нужно поле {field}.", field = key));
        let room = text("room").unwrap_or_else(|| "*".into());
        match tool {
            "say" => {
                let line = needed("text")?;
                if (self.quiet)() {
                    return Ok("not said: do not disturb is on".into());
                }
                let said = (self.speak)(room, line).await;
                Ok(if said { "said" } else { "not said: no station is on the line" }.into())
            }
            "notify" => {
                let message = needed("text")?;
                (self.notify)(format!("{}: {message}", principal.name)).await.map(|()| "sent".into())
            }
            "desk_card" => {
                self.line_of(principal).card(&needed("task")?, &needed("title")?, text("line"));
                Ok("shown".into())
            }
            "task_step" => {
                let state = match text("state").as_deref() {
                    None | Some("working") => SystemState::Working,
                    Some(word) => SystemState::parse(word),
                };
                self.line_of(principal).step(&needed("task")?, &needed("line")?, state);
                Ok(if state.going() { "noted" } else { "ended" }.into())
            }
            "ask_yes_no" => {
                let question = needed("question")?;
                let wait = args
                    .get("wait")
                    .and_then(Value::as_u64)
                    .map(|seconds| Duration::from_secs(seconds.clamp(5, ASK_MAX.as_secs())))
                    .unwrap_or(ASK_WAIT);
                // Quiet: the question goes to the phone too, and waits for its answer in «Работы».
                if (self.quiet)() {
                    let line = crate::tr!(
                        "{name} спрашивает: {question} Ответьте в «Работах».",
                        name = &principal.name,
                        question = &question
                    );
                    let _ = (self.notify)(line).await;
                }
                Ok(self.line_of(principal).ask(&question, text("task").as_deref(), wait).await.into())
            }
            _ => Err(crate::tr!("Такого инструмента нет: {tool}.", tool = tool)),
        }
    }
}

/// An agent's line as a work system: its tasks are works begun outside, its questions the works' questions.
pub struct AgentLine {
    id: String,
    title: String,
    works: Mutex<HashMap<String, Seen>>,
    waiting: Mutex<HashMap<String, oneshot::Sender<Reply>>>,
    listeners: Mutex<Vec<SystemListener>>,
    next: AtomicU64,
}

impl AgentLine {
    fn new(id: &str, title: &str) -> AgentLine {
        AgentLine {
            id: id.into(),
            title: title.into(),
            works: Mutex::new(HashMap::new()),
            waiting: Mutex::new(HashMap::new()),
            listeners: Mutex::new(Vec::new()),
            next: AtomicU64::new(1),
        }
    }

    fn emit(&self, event: SystemEvent) {
        let listeners: Vec<SystemListener> = locked(&self.listeners).clone();
        for listener in listeners {
            listener(&self.id, event.clone());
        }
    }

    /// A task shown: begun the first time, its line changed after.
    fn card(&self, task: &str, title: &str, line: Option<String>) {
        let begun = {
            let mut works = locked(&self.works);
            match works.get_mut(task) {
                Some(seen) => {
                    seen.title = title.to_string();
                    seen.line = line.clone();
                    None
                }
                None => {
                    let seen = Seen {
                        id: task.to_string(),
                        title: title.to_string(),
                        place: None,
                        state: SystemState::Working,
                        line: line.clone(),
                        at: None,
                        outside: true,
                    };
                    works.insert(task.to_string(), seen.clone());
                    Some(seen)
                }
            }
        };
        match begun {
            Some(seen) => self.emit(SystemEvent::Began { seen }),
            None => self.emit(SystemEvent::State { work: task.to_string(), state: SystemState::Working, line }),
        }
    }

    /// A step of a task; a state that does not go on ends it, with the line as its result. A task never shown is
    /// shown first, by its id.
    fn step(&self, task: &str, line: &str, state: SystemState) {
        if !locked(&self.works).contains_key(task) {
            self.card(task, task, None);
        }
        if state.going() {
            if let Some(seen) = locked(&self.works).get_mut(task) {
                seen.line = Some(line.to_string());
            }
            self.emit(SystemEvent::Step { work: task.to_string(), line: line.to_string() });
            return;
        }
        locked(&self.works).remove(task);
        let outcome = Outcome { summary: Some(line.to_string()), ..Outcome::default() };
        self.emit(SystemEvent::Ended { work: task.to_string(), state, outcome });
    }

    /// The person is asked; the answer, or silence when none comes in `wait`. A question about no task is a work of its
    /// own while it waits.
    async fn ask(&self, question: &str, task: Option<&str>, wait: Duration) -> &'static str {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let known = task.filter(|task| locked(&self.works).contains_key(*task)).map(str::to_string);
        let (work, own) = match known {
            Some(task) => (task, false),
            None => {
                let id = format!("ask-{n}");
                self.card(&id, question, None);
                (id, true)
            }
        };
        let id = format!("q-{n}");
        let (answer, answered) = oneshot::channel();
        locked(&self.waiting).insert(id.clone(), answer);
        let asked = Question { id: id.clone(), text: question.to_string(), expects: Expects::YesNo, dangerous: false };
        self.emit(SystemEvent::Asks { work: work.clone(), question: asked });
        let reply = tokio::time::timeout(wait, answered).await;
        locked(&self.waiting).remove(&id);
        let said = match reply {
            Ok(Ok(Reply::Yes)) => "yes",
            Ok(Ok(Reply::No)) => "no",
            _ => "silence",
        };
        if own {
            locked(&self.works).remove(&work);
            let outcome = Outcome { summary: Some(said.to_string()), ..Outcome::default() };
            self.emit(SystemEvent::Ended { work, state: SystemState::Ready, outcome });
        }
        said
    }
}

#[async_trait]
impl WorkSystem for AgentLine {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn about(&self) -> About {
        About {
            id: self.id.clone(),
            title: self.title.clone(),
            names: vec![self.title.clone()],
            plug: "home-line".into(),
            protocol: Some("MCP".into()),
            version: None,
        }
    }

    async fn ready(&self) -> SystemReady {
        SystemReady::yes()
    }

    fn can(&self) -> Can {
        Can { list: true, questions: true, result: true, ..Can::default() }
    }

    async fn list(&self) -> Vec<Seen> {
        locked(&self.works).values().cloned().collect()
    }

    async fn status(&self, work: &str) -> Option<Status> {
        let works = locked(&self.works);
        let seen = works.get(work)?;
        Some(Status { state: seen.state, line: seen.line.clone(), at: seen.at })
    }

    async fn answer(&self, _work: &str, question: &str, reply: Reply) -> Result<(), Refusal> {
        let waiting = locked(&self.waiting).remove(question);
        let waiting = waiting.ok_or_else(|| Refusal::because("the question no longer waits"))?;
        waiting.send(reply).map_err(|_| Refusal::because("the question no longer waits"))
    }

    fn listen(&self, events: SystemListener) {
        locked(&self.listeners).push(events);
    }
}

/// `POST /mcp/home`: one JSON-RPC message, one answer; a notification is taken with 202.
pub fn routes<S>() -> Router<S>
where
    S: RouteState + Provides<Arc<HomeLine>>,
{
    Router::new().route(PATH, post(home))
}

async fn home(
    Uses(line): Uses<Arc<HomeLine>>,
    caller: Caller,
    Body(request): Body<Value>,
) -> Result<Response, ApiError> {
    let principal =
        caller.principal().cloned().ok_or_else(|| ApiError::unauthorized(crate::tr!("Нужен ключ доступа.")))?;
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    if request.get("id").is_none() {
        return Ok(axum::http::StatusCode::ACCEPTED.into_response());
    }
    let method = request.get("method").and_then(Value::as_str).unwrap_or_default();
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18"),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "duowave-home", "version": crate::VERSION },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": line.tools(&principal) })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let arguments = params.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default();
            let (text, failed) = match line.call(&principal, name, &arguments).await {
                Ok(text) => (text, false),
                Err(why) => (why, true),
            };
            Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": failed }))
        }
        other => Err((-32601, format!("method {other} is not here"))),
    };
    let answer = match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    };
    Ok(axum::Json(answer).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::EventBus;
    use crate::core::identity::{PrincipalKind, Role};
    use crate::talk::asked::Kind;
    use crate::talk::TalkLayer;

    fn key(grants: &[&str]) -> Principal {
        Principal {
            kind: PrincipalKind::ApiKey,
            id: "a1b2c3d4e5f6".into(),
            name: "Hermes".into(),
            role: None,
            grants: grants.iter().map(|grant| grant.to_string()).collect(),
            denies: Vec::new(),
        }
    }

    fn admin() -> crate::talk::origin::Origin {
        let mut origin = crate::talk::origin::Origin::computer(crate::talk::origin::Activator::Button);
        origin.role = Some(Role::Admin);
        origin
    }

    struct Bench {
        layer: Arc<TalkLayer>,
        line: Arc<HomeLine>,
        said: Arc<Mutex<Vec<(String, String)>>>,
        sent: Arc<Mutex<Vec<String>>>,
        quiet: Arc<std::sync::atomic::AtomicBool>,
    }

    fn bench() -> Bench {
        let layer = TalkLayer::new(None, EventBus::new());
        let systems = Systems::new();
        crate::talk::systems::SystemExecutor::new(&layer, &systems, false);
        let (said, sent) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
        let quiet = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (into_said, into_sent, is_quiet) = (said.clone(), sent.clone(), quiet.clone());
        let line = HomeLine::new(
            systems,
            Arc::new(move |device, text| {
                into_said.lock().unwrap().push((device, text));
                Box::pin(async { true })
            }),
            Arc::new(move |text| {
                into_sent.lock().unwrap().push(text);
                Box::pin(async { Ok(()) })
            }),
            Arc::new(move || is_quiet.load(Ordering::Relaxed)),
        );
        Bench { layer, line, said, sent, quiet }
    }

    fn args(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    async fn until(what: &str, check: impl Fn() -> bool) {
        for _ in 0..300 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("never: {what}");
    }

    /// Each tool by its grant: a key sees and calls only what it was given, and a refusal says why in words.
    #[tokio::test]
    async fn a_key_sees_and_calls_only_the_tools_of_its_grants() {
        let bench = bench();
        let only_say = key(&[AGENT_SAY]);
        let names: Vec<String> =
            bench.line.tools(&only_say).iter().map(|tool| tool["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, ["say"]);
        assert_eq!(bench.line.tools(&key(&["agent.*"])).len(), 5);
        let said = bench.line.call(&only_say, "say", &args(json!({ "text": "Билеты подешевели" }))).await;
        assert_eq!(said.as_deref(), Ok("said"));
        assert_eq!(bench.said.lock().unwrap().as_slice(), [("*".to_string(), "Билеты подешевели".to_string())]);
        let refused = bench.line.call(&only_say, "notify", &args(json!({ "text": "x" }))).await.unwrap_err();
        assert!(refused.starts_with("У этого ключа нет права «"), "{refused}");
        assert!(bench.line.call(&only_say, "rm", &Map::new()).await.is_err());
        // «Не беспокоить»: nothing is said aloud.
        bench.quiet.store(true, Ordering::Relaxed);
        let quiet = bench.line.call(&only_say, "say", &args(json!({ "text": "ещё" }))).await.unwrap();
        assert!(quiet.starts_with("not said"), "{quiet}");
        assert_eq!(bench.said.lock().unwrap().len(), 1);
        let sent = bench.line.call(&key(&[AGENT_NOTIFY]), "notify", &args(json!({ "text": "Готово" }))).await;
        assert_eq!(sent.as_deref(), Ok("sent"));
        assert_eq!(bench.sent.lock().unwrap().as_slice(), ["Hermes: Готово".to_string()]);
    }

    /// An agent's own task is a work begun outside, its steps its line, a state that does not go on its end; its
    /// question is the work's question, and the admin's «да» comes back to the agent as yes.
    #[tokio::test]
    async fn an_agents_task_is_a_work_and_its_question_is_answered_by_voice() {
        let bench = bench();
        let agent = key(&["agent.*"]);
        let card = args(json!({ "task": "t-1", "title": "Билеты в Казань", "line": "смотрит цены" }));
        assert_eq!(bench.line.call(&agent, "desk_card", &card).await.as_deref(), Ok("shown"));
        let layer = bench.layer.clone();
        let of_agent =
            move || {
                layer.works.all(None, None).into_iter().find(|work| {
                    work.system.as_deref() == Some("agent-a1b2c3d4e5f6") && work.title == "Билеты в Казань"
                })
            };
        until("the work shows", || of_agent().is_some()).await;
        let work = of_agent().unwrap();
        assert!(work.outside && work.going());

        // The question waits for the admin's answer.
        let (line, asking) = (bench.line.clone(), agent.clone());
        let asked = tokio::spawn(async move {
            let question = args(json!({ "question": "Купить за 3000?", "task": "t-1", "wait": 5 }));
            line.call(&asking, "ask_yes_no", &question).await
        });
        let layer = bench.layer.clone();
        let id = work.id.clone();
        until("the question is the work's", || {
            layer.works.get(&id).and_then(|work| work.question).is_some_and(|question| question.text.contains("Купить"))
        })
        .await;
        let question = bench.layer.works.get(&work.id).unwrap().question.unwrap();
        assert_eq!(question.text, "Hermes спрашивает: Купить за 3000?");
        let said =
            bench.layer.answer(admin(), &work.id, Kind::Permission, false, crate::talk::ready::Answer::Yes).await;
        assert!(!said.is_empty());
        assert_eq!(asked.await.unwrap().as_deref(), Ok("yes"));

        // Its end, with the result.
        let end = args(json!({ "task": "t-1", "line": "Купил за 2900", "state": "ready" }));
        assert_eq!(bench.line.call(&agent, "task_step", &end).await.as_deref(), Ok("ended"));
        let layer = bench.layer.clone();
        let id = work.id.clone();
        until("the work ends", || layer.works.get(&id).is_some_and(|work| !work.going())).await;
    }

    /// A question with no task waits as a work of its own; no answer in its time is silence, and the work ends. In
    /// quiet hours it goes to the phone too.
    #[tokio::test]
    async fn a_question_without_an_answer_is_silence_and_quiet_hours_send_it_to_the_phone() {
        let bench = bench();
        bench.quiet.store(true, Ordering::Relaxed);
        let question = args(json!({ "question": "Напомнить про билеты?", "wait": 5 }));
        let line = bench.line.clone();
        let agent = key(&[AGENT_ASK]);
        let start = std::time::Instant::now();
        tokio::time::pause();
        let asked = tokio::spawn(async move { line.call(&agent, "ask_yes_no", &question).await });
        tokio::time::advance(Duration::from_secs(6)).await;
        assert_eq!(asked.await.unwrap().as_deref(), Ok("silence"));
        assert!(start.elapsed() < Duration::from_secs(5), "paused time");
        assert_eq!(
            bench.sent.lock().unwrap().as_slice(),
            ["Hermes спрашивает: Напомнить про билеты? Ответьте в «Работах».".to_string()]
        );
    }
}
