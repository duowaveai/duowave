//! The command plug (docs/plans/work-systems.md, «Plugs», 2): a system that answers commands with JSON, led by its
//! card's block `system` — which command starts, lists, shows, tells, stops, and where in the answer the id, the
//! state and the text are. bb stands on it whole, T3 Code without `stop`.
//!
//! The card's limits («The system card»): it runs only its own program, found as an agent's program is (the path set
//! in the panel, else the search path), never a shell; arguments are a list, and a placeholder stands for a whole
//! argument and takes only an id the system itself returned (a place, a work) or the path of a work folder; the
//! person's words go by stdin, never into an argument; an answer is read as JSON with a size and a time limit, by
//! plain dotted paths. A work it started is watched by `status` on a slow poll, so its end comes back as an event.
//!
//! A verb may go through MCP instead (`"via": "mcp"`, stage 8): a tool of a server the person added, named by its id
//! or title, with the card's arguments; a placeholder is a whole value, and there the person's words may go (`{task}`,
//! `{text}`), as they are plain JSON and never a command line. Such a verb is there only while its server is on; a
//! card whose plug is `mcp` needs no program at all. T3 Code stops this way.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use crate::talk::system::{
    About, Can, McpCaller, OpenIn, Outcome, Place, Refusal, Seen, Spend, Status, SystemEvent, SystemListener,
    SystemReady, SystemState, When, WorkSystem,
};

/// How long a command may take.
const VERB_WAIT: Duration = Duration::from_secs(20);
/// The most of an answer read.
const ANSWER_MAX: usize = 1024 * 1024;
/// How often the works it started are asked how they go.
const WATCH_EVERY: Duration = Duration::from_secs(15);

/// Where the program is now.
pub type Program = Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>;
/// The work folders' paths now: a system that names its projects by a path takes one of these as a place.
pub type Folders = Arc<dyn Fn() -> Vec<String> + Send + Sync>;

/// One dotted path into an answer, or several to try in turn (a card written before the first live run).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Paths {
    #[default]
    None,
    One(String),
    Many(Vec<String>),
}

impl Paths {
    fn list(&self) -> Vec<&str> {
        match self {
            Paths::None => Vec::new(),
            Paths::One(path) => vec![path.as_str()],
            Paths::Many(paths) => paths.iter().map(String::as_str).collect(),
        }
    }

    /// The first of the paths that leads somewhere.
    fn find<'a>(&self, value: &'a Value) -> Option<&'a Value> {
        self.list().into_iter().find_map(|path| at(value, path)).filter(|found| !found.is_null())
    }

    fn text(&self, value: &Value) -> Option<String> {
        match self.find(value)? {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        }
    }
}

/// A plain dotted path: `thread.id`, `threads`, `messages.0.text`, `messages.-1.text` (the last); empty — the value
/// itself.
fn at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return Some(value);
    }
    path.split('.').try_fold(value, |value, part| match value {
        Value::Array(items) => match part.strip_prefix('-').and_then(|back| back.parse::<usize>().ok()) {
            Some(back) => items.len().checked_sub(back).and_then(|index| items.get(index)),
            None => part.parse::<usize>().ok().and_then(|index| items.get(index)),
        },
        _ => value.get(part),
    })
}

/// A log line of a program, not its words: a bracketed head with a level, as Chromium and Electron write them.
fn is_log_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('[')
        && line
            .split(']')
            .next()
            .is_some_and(|head| head.contains(":ERROR:") || head.contains(":WARNING:") || head.contains(":INFO:"))
}

/// A verb of the card: the command's arguments, what goes by stdin (`task`, `text`), and where its answer says what.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Verb {
    #[serde(default)]
    pub run: Vec<String>,
    /// `ready`: words of a refusal that still say the system answers — T3 Code's «no project matches '.'» comes from a
    /// server that runs (the hand check of 27.11).
    #[serde(default)]
    pub fine: Vec<String>,
    /// `mcp`: the verb is a tool of an MCP server, not a command.
    #[serde(default)]
    pub via: Option<String>,
    /// The MCP server by its id or title; none — the card's `server`.
    #[serde(default)]
    pub server: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    /// The tool's arguments; a string that is a whole placeholder is filled.
    #[serde(default)]
    pub args: Option<serde_json::Map<String, Value>>,
    #[serde(default)]
    pub stdin: Option<String>,
    /// The array of a list's answer.
    #[serde(default)]
    pub each: Paths,
    #[serde(default)]
    pub id: Paths,
    #[serde(default)]
    pub title: Paths,
    #[serde(default)]
    pub state: Paths,
    #[serde(default)]
    pub line: Paths,
    /// A place's folder.
    #[serde(default)]
    pub path: Paths,
    /// `now` and `after` in the system's own words (`tell`).
    #[serde(default)]
    pub when: BTreeMap<String, String>,
}

/// The block `system` of an agent card.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemCard {
    #[serde(default = "one")]
    pub contract: u32,
    pub plug: String,
    /// The MCP server of its verbs `via: "mcp"`, by its id or title.
    #[serde(default)]
    pub server: Option<String>,
    /// The names it is said by, by language.
    #[serde(default)]
    pub names: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub ready: Option<Verb>,
    #[serde(default)]
    pub places: Option<Verb>,
    #[serde(default)]
    pub start: Option<Verb>,
    #[serde(default)]
    pub list: Option<Verb>,
    #[serde(default)]
    pub status: Option<Verb>,
    #[serde(default)]
    pub tell: Option<Verb>,
    #[serde(default)]
    pub stop: Option<Verb>,
    #[serde(default)]
    pub result: Option<Verb>,
    /// A command for a terminal that shows the work in the system itself: shown, never run here.
    #[serde(default)]
    pub open: Option<Verb>,
    /// The system's words for states → the contract's (`working`, `ready`, …); a word not here is the nearest:
    /// working.
    #[serde(default)]
    pub states: BTreeMap<String, String>,
}

fn one() -> u32 {
    1
}

impl SystemCard {
    fn verbs(&self) -> impl Iterator<Item = &Verb> {
        [
            &self.ready,
            &self.places,
            &self.start,
            &self.list,
            &self.status,
            &self.tell,
            &self.stop,
            &self.result,
            &self.open,
        ]
        .into_iter()
        .flatten()
    }
}

fn by_mcp(verb: &Verb) -> bool {
    verb.via.as_deref() == Some("mcp")
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct CommandSystem {
    id: String,
    title: String,
    program_name: String,
    card: SystemCard,
    program: Program,
    folders: Folders,
    me: Weak<CommandSystem>,
    /// Ids the system returned (places, works) and work folders' paths: what a placeholder may take.
    known: Mutex<HashSet<String>>,
    /// The works it started, and their state as last seen: watched until they end.
    watched: Mutex<HashMap<String, SystemState>>,
    /// Whether the slow poll of the watched works runs.
    watching: AtomicBool,
    /// The MCP servers, for its verbs `via: "mcp"`.
    mcp: OnceLock<Arc<dyn McpCaller>>,
    listeners: Mutex<Vec<SystemListener>>,
}

impl CommandSystem {
    pub fn new(
        id: &str,
        title: &str,
        program_name: &str,
        card: SystemCard,
        program: Program,
        folders: Folders,
    ) -> Arc<CommandSystem> {
        Arc::new_cyclic(|me| CommandSystem {
            id: id.into(),
            title: title.into(),
            program_name: program_name.into(),
            card,
            program,
            folders,
            me: me.clone(),
            known: Mutex::new(HashSet::new()),
            watched: Mutex::new(HashMap::new()),
            watching: AtomicBool::new(false),
            mcp: OnceLock::new(),
            listeners: Mutex::new(Vec::new()),
        })
    }

    /// A card's block `system` as JSON; None when it is not one this plug leads (`command`, or `mcp` whole).
    pub fn card_of(value: &Value) -> Option<SystemCard> {
        let card: SystemCard = serde_json::from_value(value.clone()).ok()?;
        (matches!(card.plug.as_str(), "command" | "mcp") && card.contract == 1).then_some(card)
    }

    /// The MCP servers its verbs `via: "mcp"` call.
    pub fn set_mcp(&self, mcp: Arc<dyn McpCaller>) {
        let _ = self.mcp.set(mcp);
    }

    fn server_of<'a>(&'a self, verb: &'a Verb) -> Option<&'a str> {
        verb.server.as_deref().or(self.card.server.as_deref())
    }

    /// The verb can be run now: a command, or a tool of a server that is on.
    fn usable(&self, verb: &Verb) -> bool {
        if !by_mcp(verb) {
            return true;
        }
        let server = self.server_of(verb);
        self.mcp.get().zip(server).is_some_and(|(mcp, server)| mcp.on_line(server))
    }

    fn has(&self, verb: &Option<Verb>) -> bool {
        verb.as_ref().is_some_and(|verb| self.usable(verb))
    }

    /// It runs a program: some verb is a command.
    fn needs_program(&self) -> bool {
        self.card.verbs().any(|verb| !by_mcp(verb))
    }

    fn state(&self, word: Option<String>) -> SystemState {
        let Some(word) = word else { return SystemState::Working };
        let ours = self.card.states.get(&word).map(String::as_str).unwrap_or(word.as_str());
        SystemState::parse(ours)
    }

    fn emit(&self, event: SystemEvent) {
        let listeners: Vec<SystemListener> = locked(&self.listeners).clone();
        for listener in listeners {
            listener(&self.id, event.clone());
        }
    }

    /// The command's arguments with its placeholders filled: each stands for a whole argument and takes only a known
    /// id (never one that reads as a flag); a placeholder inside a longer argument is a card's mistake.
    fn args(&self, verb: &Verb, values: &[(&str, &str)]) -> Result<Vec<String>, Refusal> {
        let mut known = locked(&self.known).clone();
        known.extend((self.folders)());
        verb.run
            .iter()
            .map(|arg| {
                let Some(name) = arg.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')) else {
                    if arg.contains('{') {
                        return Err(Refusal::because(format!(
                            "the card's argument «{arg}» is not a whole placeholder"
                        )));
                    }
                    return Ok(arg.clone());
                };
                let value = values.iter().find(|(key, _)| *key == name).map(|(_, value)| *value);
                let value = value.ok_or_else(|| Refusal::because(format!("no value for {{{name}}}")))?;
                // `when` is the card's own word, `model` the model of the work's set (docs/plans/work-systems.md, stage 5).
                let free = name == "when" || name == "model";
                if value.starts_with('-') || (!free && !known.contains(value)) {
                    return Err(Refusal::because(format!("«{value}» is not an id the system gave")));
                }
                Ok(value.to_string())
            })
            .collect()
    }

    /// A verb through MCP: its tool, the card's arguments with their placeholders filled; an id only one the system
    /// gave, the person's words for `{task}` and `{text}`.
    async fn run_mcp(&self, verb: &Verb, values: &[(&str, &str)], words: Option<&str>) -> Result<Value, Refusal> {
        let mcp = self.mcp.get().ok_or_else(|| Refusal::because("no MCP servers here"))?;
        let server = self.server_of(verb).ok_or_else(|| Refusal::because("the card names no MCP server"))?;
        let tool = verb.tool.as_deref().ok_or_else(|| Refusal::because("the card names no tool"))?;
        let mut known = locked(&self.known).clone();
        known.extend((self.folders)());
        let mut args = serde_json::Map::new();
        for (key, value) in verb.args.clone().unwrap_or_default() {
            let name = value.as_str().and_then(|text| text.strip_prefix('{')?.strip_suffix('}')).map(str::to_string);
            let filled = match name.as_deref() {
                Some("task" | "text") => Value::String(words.unwrap_or_default().to_string()),
                Some(name) => {
                    let found = values.iter().find(|(key, _)| *key == name).map(|(_, value)| *value);
                    let found = found.ok_or_else(|| Refusal::because(format!("no value for {{{name}}}")))?;
                    if name != "when" && !known.contains(found) {
                        return Err(Refusal::because(format!("«{found}» is not an id the system gave")));
                    }
                    Value::String(found.to_string())
                }
                None => value,
            };
            args.insert(key, filled);
        }
        mcp.call(server, tool, args).await.map_err(Refusal::because)
    }

    /// Runs a verb: its program with the arguments, the words by stdin when the verb says so; its answer as JSON (empty
    /// — null). A verb `via: "mcp"` calls its tool.
    async fn run(&self, verb: &Verb, values: &[(&str, &str)], words: Option<&str>) -> Result<Value, Refusal> {
        // The ids it gave are kept in memory: after a restart of the gateway a work begun before is not known, and
        // nothing could be done with it (the hand check of 27.11: «открой» on bb's work, «is not an id the system
        // gave»). The system's own list tells its ids again.
        let unknown = values
            .iter()
            .any(|(key, value)| *key == "work" && !value.starts_with('-') && !locked(&self.known).contains(*value));
        if unknown && self.card.list.is_some() {
            self.list().await;
        }
        if by_mcp(verb) {
            return self.run_mcp(verb, values, words).await;
        }
        let stdin = words.filter(|_| verb.stdin.is_some());
        let program = (self.program)().ok_or_else(|| {
            Refusal::because(crate::tr!("{title} не найден на этом компьютере.", title = &self.title))
        })?;
        let args = self.args(verb, values)?;
        let mut command = tokio::process::Command::new(program);
        command
            .args(&args)
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|error| Refusal::because(error.to_string()))?;
        if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
            let _ = pipe.write_all(text.as_bytes()).await;
            drop(pipe);
        }
        let output = tokio::time::timeout(VERB_WAIT, child.wait_with_output())
            .await
            .map_err(|_| Refusal::because(format!("{}: no answer in {} s", self.program_name, VERB_WAIT.as_secs())))?
            .map_err(|error| Refusal::because(error.to_string()))?;
        if !output.status.success() {
            let said = String::from_utf8_lossy(&output.stderr);
            // A program's own log lines (T3 Code's Electron: «[11/27/…:ERROR:electron/…] …») are not its words.
            let said = said.lines().filter(|line| !is_log_line(line)).collect::<Vec<_>>().join("\n");
            let said = said.trim();
            let said = if said.is_empty() {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            } else {
                said.to_string()
            };
            return Err(Refusal::because(crate::works::harness::cut_to(&said, 300)));
        }
        let stdout = &output.stdout[..output.stdout.len().min(ANSWER_MAX)];
        if stdout.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        // A verb whose answer is not read (bb's `thread stop` says «Thread … stopped», the hand check of 27.11) does
        // not fail by its plain words: they come as a string, and a path read from them finds nothing.
        Ok(serde_json::from_slice(stdout)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(stdout).trim().to_string())))
    }

    /// The items of a list's answer.
    fn items(verb: &Verb, answer: &Value) -> Vec<Value> {
        match verb.each.find(answer).unwrap_or(answer) {
            Value::Array(items) => items.clone(),
            _ => Vec::new(),
        }
    }

    /// The works it started are asked how they go on a slow poll, one at a time, while there are any.
    fn keep_watching(&self) {
        if self.watching.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            self.watching.store(false, Ordering::Release);
            return;
        };
        let me = self.me.clone();
        handle.spawn(async move {
            loop {
                tokio::time::sleep(WATCH_EVERY).await;
                let Some(system) = me.upgrade() else { break };
                if locked(&system.watched).is_empty() {
                    system.watching.store(false, Ordering::Release);
                    // A work begun just now keeps the poll going.
                    let again = !locked(&system.watched).is_empty()
                        && system.watching.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok();
                    if !again {
                        break;
                    }
                    continue;
                }
                system.watch().await;
            }
        });
    }

    /// The works it started, asked how they go: a changed state is told, an end with its result.
    async fn watch(&self) {
        let works: Vec<(String, SystemState)> =
            locked(&self.watched).iter().map(|(id, state)| (id.clone(), *state)).collect();
        for (work, before) in works {
            let Some(status) = self.status(&work).await else { continue };
            if status.state == before {
                continue;
            }
            if status.state.going() {
                locked(&self.watched).insert(work.clone(), status.state);
                self.emit(SystemEvent::State { work, state: status.state, line: status.line });
            } else {
                locked(&self.watched).remove(&work);
                let mut outcome = self.result(&work).await.unwrap_or_default();
                if outcome.summary.is_none() {
                    outcome.summary = status.line;
                }
                self.emit(SystemEvent::Ended { work, state: status.state, outcome });
            }
        }
    }
}

#[async_trait]
impl WorkSystem for CommandSystem {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn about(&self) -> About {
        let names = self.card.names.values().flatten().cloned().collect();
        About {
            id: self.id.clone(),
            title: self.title.clone(),
            names,
            plug: "command".into(),
            protocol: None,
            version: None,
        }
    }

    async fn ready(&self) -> SystemReady {
        if self.needs_program() && (self.program)().is_none() {
            return SystemReady::no(crate::tr!("{title} не найден на этом компьютере.", title = &self.title));
        }
        if self.card.plug == "mcp" && !self.card.start.as_ref().is_some_and(|start| self.usable(start)) {
            let server = self.card.server.clone().unwrap_or_default();
            return SystemReady::no(crate::tr!("MCP-сервер «{server}» не на связи.", server = server));
        }
        let Some(verb) = &self.card.ready else { return SystemReady::yes() };
        match self.run(verb, &[], None).await {
            Ok(_) => SystemReady::yes(),
            Err(refusal) if verb.fine.iter().any(|words| refusal.why.contains(words.as_str())) => SystemReady::yes(),
            Err(refusal) => SystemReady::no(refusal.why),
        }
    }

    fn present(&self) -> bool {
        let program = !self.needs_program() || (self.program)().is_some();
        program && self.card.start.as_ref().is_some_and(|start| self.usable(start))
    }

    fn can(&self) -> Can {
        let card = &self.card;
        let tell = |key: &str| {
            card.tell
                .as_ref()
                .is_some_and(|tell| self.usable(tell) && (tell.when.contains_key(key) || tell.when.is_empty()))
        };
        Can {
            start: self.has(&card.start),
            list: self.has(&card.list),
            steps: false,
            tell_now: tell("now"),
            tell_after: tell("after"),
            stop: self.has(&card.stop),
            questions: false,
            result: self.has(&card.result) || self.has(&card.status),
            artifacts: false,
            spend: Spend::None,
            limit: false,
            places: self.has(&card.places),
            setups: false,
            ceiling: false,
            open: card.open.as_ref().is_some_and(|open| !by_mcp(open)),
            actions: false,
        }
    }

    async fn places(&self) -> Vec<Place> {
        let Some(verb) = &self.card.places else { return Vec::new() };
        let Ok(answer) = self.run(verb, &[], None).await else { return Vec::new() };
        let places: Vec<Place> = Self::items(verb, &answer)
            .iter()
            .filter_map(|item| {
                let id = verb.id.text(item)?;
                let title = verb.title.text(item).unwrap_or_else(|| id.clone());
                Some(Place { id, title, path: verb.path.text(item) })
            })
            .collect();
        locked(&self.known).extend(places.iter().map(|place| place.id.clone()));
        places
    }

    async fn start(&self, start: crate::talk::system::Start) -> Result<Seen, Refusal> {
        let verb = self.card.start.as_ref().filter(|verb| self.usable(verb)).ok_or_else(|| Refusal::cannot("start"))?;
        let place = start.place.clone().unwrap_or_default();
        let model = start.model.clone().unwrap_or_default();
        let values = [("place", place.as_str()), ("model", model.as_str())];
        let answer = self.run(verb, &values, Some(start.task.as_str())).await?;
        let id = verb
            .id
            .text(&answer)
            .ok_or_else(|| Refusal::because(format!("{}: no id in the answer", self.program_name)))?;
        locked(&self.known).insert(id.clone());
        locked(&self.watched).insert(id.clone(), SystemState::Working);
        self.keep_watching();
        let title = verb.title.text(&answer).or(start.title).unwrap_or_else(|| start.task.clone());
        Ok(Seen {
            id,
            title,
            place: Some(place).filter(|place| !place.is_empty()),
            state: SystemState::Working,
            line: None,
            at: None,
            outside: false,
        })
    }

    async fn list(&self) -> Vec<Seen> {
        let Some(verb) = &self.card.list else { return Vec::new() };
        let Ok(answer) = self.run(verb, &[], None).await else { return Vec::new() };
        let watched = locked(&self.watched).clone();
        let seen: Vec<Seen> = Self::items(verb, &answer)
            .iter()
            .filter_map(|item| {
                let id = verb.id.text(item)?;
                Some(Seen {
                    title: verb.title.text(item).unwrap_or_else(|| id.clone()),
                    place: verb.path.text(item),
                    state: self.state(verb.state.text(item)),
                    line: verb.line.text(item),
                    at: None,
                    outside: !watched.contains_key(&id),
                    id,
                })
            })
            .collect();
        locked(&self.known).extend(seen.iter().map(|seen| seen.id.clone()));
        seen
    }

    async fn status(&self, work: &str) -> Option<Status> {
        let verb = self.card.status.as_ref()?;
        let answer = self.run(verb, &[("work", work)], None).await.ok()?;
        Some(Status { state: self.state(verb.state.text(&answer)), line: verb.line.text(&answer), at: None })
    }

    async fn tell(&self, work: &str, text: &str, when: When) -> Result<(), Refusal> {
        let verb = self.card.tell.as_ref().filter(|verb| self.usable(verb)).ok_or_else(|| Refusal::cannot("tell"))?;
        let key = match when {
            When::Now => "now",
            When::After => "after",
        };
        if !verb.when.is_empty() && !verb.when.contains_key(key) {
            return Err(Refusal::cannot("tell"));
        }
        let word = verb.when.get(key).cloned().unwrap_or_default();
        self.run(verb, &[("work", work), ("when", word.as_str())], Some(text)).await.map(|_| ())
    }

    async fn stop(&self, work: &str) -> Result<(), Refusal> {
        let verb = self.card.stop.as_ref().filter(|verb| self.usable(verb)).ok_or_else(|| Refusal::cannot("stop"))?;
        self.run(verb, &[("work", work)], None).await?;
        locked(&self.watched).remove(work);
        self.emit(SystemEvent::Ended { work: work.into(), state: SystemState::Stopped, outcome: Outcome::default() });
        Ok(())
    }

    async fn result(&self, work: &str) -> Option<Outcome> {
        let verb = self.card.result.as_ref().or(self.card.status.as_ref())?;
        let answer = self.run(verb, &[("work", work)], None).await.ok()?;
        let summary = verb.line.text(&answer).or_else(|| answer.as_str().map(str::to_string));
        Some(Outcome { summary, ..Outcome::default() })
    }

    async fn show(&self, work: &str) -> Result<(), Refusal> {
        let verb = self.card.open.as_ref().filter(|verb| !by_mcp(verb)).ok_or_else(|| Refusal::cannot("open"))?;
        self.run(verb, &[("work", work)], None).await.map(|_| ())
    }

    fn open(&self, work: &str) -> Option<OpenIn> {
        let verb = self.card.open.as_ref().filter(|verb| !by_mcp(verb))?;
        let args = self.args(verb, &[("work", work)]).ok()?;
        let line = std::iter::once(self.program_name.clone()).chain(args).collect::<Vec<_>>().join(" ");
        Some(OpenIn { command: Some(line), url: None })
    }

    fn listen(&self, events: SystemListener) {
        locked(&self.listeners).push(events);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::talk::system::Start;

    /// bb by its bundled card, with the stub `bb` (src/bin/test_stub.rs).
    fn bb(dir: &std::path::Path) -> (Arc<CommandSystem>, Arc<Mutex<Vec<SystemEvent>>>) {
        let program = crate::utils::test_stub::put(dir, "bb");
        let card = super::super::cards::bundled().into_iter().find(|card| card.id == "bb").unwrap().card;
        let system = CommandSystem::new(
            "bb",
            "bb",
            "bb",
            card,
            Arc::new(move || Some(program.clone())),
            Arc::new(|| vec!["/work/site".to_string()]),
        );
        let heard = Arc::new(Mutex::new(Vec::new()));
        let into = heard.clone();
        system.listen(Arc::new(move |_, event| into.lock().unwrap().push(event)));
        (system, heard)
    }

    fn log(dir: &std::path::Path, name: &str) -> String {
        std::fs::read_to_string(dir.join(name)).unwrap_or_default()
    }

    /// The card leads bb whole: its projects are places, a work begins with the words by stdin, the list and the
    /// status read its answers by the card's paths, words go now or after the turn, its end comes back by the slow
    /// poll with the output, «стоп» stops, «открой» is a command shown.
    #[tokio::test]
    async fn bb_by_its_card_begins_tells_stops_and_its_end_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let (system, heard) = bb(dir.path());
        assert!(system.ready().await.ready);
        let can = system.can();
        assert!(can.start && can.list && can.stop && can.places && can.tell_now && can.tell_after && can.open);
        let places = system.places().await;
        assert_eq!((places[0].id.as_str(), places[0].path.as_deref()), ("pr1", Some("/work/site")));

        let task = "сделай тёмную тему --and-do-not-ask";
        let seen =
            system.start(Start { task: task.into(), place: Some("pr1".into()), ..Start::default() }).await.unwrap();
        assert_eq!((seen.id.as_str(), seen.state), ("t1", SystemState::Working));
        assert_eq!(log(dir.path(), "prompts.log").trim(), task, "the words go by stdin");
        assert!(log(dir.path(), "calls.log").contains("thread spawn --json --project pr1 --prompt-file -"));
        assert!(!log(dir.path(), "calls.log").contains("тёмную"), "never in an argument");

        let listed = system.list().await;
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].outside, "begun by us");
        system.tell("t1", "и футер", When::Now).await.unwrap();
        system.tell("t1", "и шапку", When::After).await.unwrap();
        assert_eq!(log(dir.path(), "told.log"), "t1 steer и футер\nt1 queue и шапку\n");
        assert_eq!(system.status("t1").await.map(|status| status.state), Some(SystemState::Working));

        // A placeholder takes only an id the system gave, never one that reads as a flag.
        assert!(system.status("--help").await.is_none());
        assert!(system.status("t9").await.is_none());
        let calls = log(dir.path(), "calls.log");
        assert!(!calls.contains("--help") && !calls.contains("show t9"), "nothing ran with them: {calls}");
        assert!(system
            .start(Start { task: "x".into(), place: Some("/etc".into()), ..Start::default() })
            .await
            .is_err());

        // Its end comes back by the poll: the state and the output.
        std::fs::write(dir.path().join("done-t1"), "Сделала тёмную тему.").unwrap();
        system.watch().await;
        let ended = heard.lock().unwrap().iter().find_map(|event| match event {
            SystemEvent::Ended { work, state, outcome } => Some((work.clone(), *state, outcome.summary.clone())),
            _ => None,
        });
        assert_eq!(ended, Some(("t1".into(), SystemState::Ready, Some("Сделала тёмную тему.".into()))));

        let second = system.start(Start { task: "ещё".into(), place: Some("pr1".into()), ..Start::default() }).await;
        let second = second.unwrap().id;
        system.stop(&second).await.unwrap();
        assert!(log(dir.path(), "calls.log").contains(&format!("thread stop {second}")));
        assert_eq!(system.open("t1").and_then(|open| open.command).as_deref(), Some("bb thread open t1"));
        // «Открой» runs it where bb lives; bb's words are not JSON, and that is no failure.
        system.show("t1").await.unwrap();
        assert!(log(dir.path(), "calls.log").contains("thread open t1"));
        // After a restart of the gateway its works are known again from bb's own list.
        let (again, _) = bb(dir.path());
        again.show("t1").await.unwrap();
        assert!(again.show("t-never").await.is_err(), "an id bb never gave");
    }

    /// The contract's kit passes on bb's card.
    #[tokio::test]
    async fn bb_by_its_card_passes_the_contract_kit() {
        let dir = tempfile::tempdir().unwrap();
        let (system, _) = bb(dir.path());
        system.places().await;
        let problems = crate::talk::system::check(system.as_ref(), Some("поправь опечатку")).await;
        assert!(problems.is_empty(), "{problems:?}");
    }

    /// Without its program the system is «не готов» in words, and nothing runs.
    /// T3 Code's Electron writes its log to stderr before its own words (the hand check of 27.11).
    #[test]
    fn a_programs_log_lines_are_not_its_words() {
        assert!(super::is_log_line(
            "[1127/144524.041258:ERROR:electron/shell/common/node_util.cc:78] CompileAndCall failed"
        ));
        assert!(!super::is_log_line("No running T3 Code server uses /x. Start T3 Code, then retry."));
        assert!(!super::is_log_line("[note] a word in brackets"));
    }

    #[tokio::test]
    async fn without_its_program_it_is_not_ready_in_words() {
        let card = super::super::cards::bundled().into_iter().find(|card| card.id == "t3code").unwrap().card;
        let system = CommandSystem::new("t3code", "T3 Code", "t3", card, Arc::new(|| None), Arc::new(Vec::new));
        let ready = system.ready().await;
        assert!(!ready.ready);
        assert_eq!(ready.why.as_deref(), Some("T3 Code не найден на этом компьютере."));
        assert!(!system.can().stop, "T3 Code stops only by its MCP, and no MCP is here");
    }

    /// bb's own `status` answers with its server down; its projects do not: «Готова» only while they answer (the hand
    /// check of 27.11 found bb «Готова» with its server not running).
    #[tokio::test]
    async fn bb_with_its_server_down_is_not_ready_in_its_words() {
        let dir = tempfile::tempdir().unwrap();
        let (system, _) = bb(dir.path());
        std::fs::write(dir.path().join("server-down"), "").unwrap();
        let ready = system.ready().await;
        assert!(!ready.ready);
        assert!(ready.why.as_deref().unwrap_or_default().contains("BB server"), "{:?}", ready.why);
        std::fs::remove_file(dir.path().join("server-down")).unwrap();
        assert!(system.ready().await.ready);
    }

    /// MCP as a test sees it: one server «T3 Code», on or off; answers by tool; the calls kept.
    #[derive(Default)]
    struct FakeMcp {
        on: AtomicBool,
        calls: Mutex<Vec<(String, serde_json::Map<String, Value>)>>,
    }

    #[async_trait]
    impl McpCaller for FakeMcp {
        fn on_line(&self, server: &str) -> bool {
            server == "T3 Code" && self.on.load(Ordering::Relaxed)
        }

        async fn call(&self, _server: &str, tool: &str, args: serde_json::Map<String, Value>) -> Result<Value, String> {
            self.calls.lock().unwrap().push((tool.to_string(), args));
            Ok(match tool {
                "launch" => serde_json::json!({ "threadId": "th1", "title": "Тёмная тема" }),
                "read" => serde_json::json!({ "state": "running", "latest": "правит стили" }),
                _ => Value::Null,
            })
        }
    }

    /// T3 Code's stop goes through its MCP: it is there only while the server is on, the other verbs stay commands.
    #[tokio::test]
    async fn t3_codes_stop_is_there_only_while_its_mcp_server_is_on() {
        let card = super::super::cards::bundled().into_iter().find(|card| card.id == "t3code").unwrap().card;
        let program = Arc::new(|| Some(PathBuf::from("/usr/bin/true")));
        let system = CommandSystem::new("t3code", "T3 Code", "t3", card, program, Arc::new(Vec::new));
        assert!(!system.can().stop, "no MCP here");
        let mcp = Arc::new(FakeMcp::default());
        system.set_mcp(mcp.clone());
        assert!(!system.can().stop, "the server is off");
        mcp.on.store(true, Ordering::Relaxed);
        let can = system.can();
        assert!(can.stop && can.start && can.list && can.tell_after && !can.tell_now);
        assert!(system.stop("not-given").await.is_err(), "an id the system never gave");
        assert!(mcp.calls.lock().unwrap().is_empty());
    }

    /// A system wholly through MCP: no program; its start takes the task as a tool's argument, the id its answer gave
    /// is then known, its status reads the answer by the card's paths, «стоп» calls the tool with that id.
    #[tokio::test]
    async fn a_system_wholly_through_mcp_needs_no_program() {
        let block = serde_json::json!({
            "contract": 1, "plug": "mcp", "server": "T3 Code",
            "start": { "via": "mcp", "tool": "launch", "args": { "prompt": "{task}", "project": "{place}" },
                "id": "threadId", "title": "title" },
            "status": { "via": "mcp", "tool": "read", "args": { "threadId": "{work}" }, "state": "state", "line": "latest" },
            "stop": { "via": "mcp", "tool": "interrupt", "args": { "threadId": "{work}" } },
            "states": { "running": "working" }
        });
        let card = CommandSystem::card_of(&block).expect("a card of the MCP plug");
        let folders: Folders = Arc::new(|| vec!["/work/site".to_string()]);
        let system = CommandSystem::new("t3mcp", "T3 Code", "t3", card, Arc::new(|| None), folders);
        let mcp = Arc::new(FakeMcp::default());
        system.set_mcp(mcp.clone());
        let ready = system.ready().await;
        assert_eq!(ready.why.as_deref(), Some("MCP-сервер «T3 Code» не на связи."));
        assert!(!system.present());
        mcp.on.store(true, Ordering::Relaxed);
        assert!(system.ready().await.ready && system.present(), "no program needed");

        let start = Start {
            task: "сделай тёмную тему".into(), place: Some("/work/site".into()), ..Start::default()
        };
        let seen = system.start(start).await.unwrap();
        assert_eq!((seen.id.as_str(), seen.title.as_str()), ("th1", "Тёмная тема"));
        let status = system.status("th1").await.unwrap();
        assert_eq!((status.state, status.line.as_deref()), (SystemState::Working, Some("правит стили")));
        system.stop("th1").await.unwrap();
        let calls = mcp.calls.lock().unwrap().clone();
        assert_eq!(calls[0].0, "launch");
        assert_eq!(calls[0].1["prompt"], "сделай тёмную тему", "the words go as an argument of the tool");
        assert_eq!(calls[0].1["project"], "/work/site");
        assert_eq!((calls[2].0.as_str(), calls[2].1["threadId"].as_str()), ("interrupt", Some("th1")));
        assert!(system.status("--help").await.is_none(), "an id the system never gave");
        assert_eq!(mcp.calls.lock().unwrap().len(), 3);
    }

    #[test]
    fn a_dotted_path_reads_arrays_and_objects() {
        let answer = serde_json::json!({ "thread": { "id": "t1" }, "messages": [{ "text": "a" }, { "text": "b" }] });
        assert_eq!(Paths::Many(vec!["id".into(), "thread.id".into()]).text(&answer).as_deref(), Some("t1"));
        assert_eq!(Paths::One("messages.0.text".into()).text(&answer).as_deref(), Some("a"));
        // The last one: T3 Code's thread ends with its agent's message (the hand check of 27.11).
        assert_eq!(Paths::One("messages.-1.text".into()).text(&answer).as_deref(), Some("b"));
        assert_eq!(Paths::One("messages.-3.text".into()).text(&answer), None);
        assert_eq!(Paths::One("nothing".into()).text(&answer), None);
    }
}
