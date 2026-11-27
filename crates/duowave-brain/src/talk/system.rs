//! The contract «Система работ» (docs/plans/work-systems.md, «The contract»): what DuoWave asks of a system that leads
//! works by its own rules — our own runner (`duowave`), Claude Code run by hand, bb, T3 Code, an agent behind ACP.
//! Voice, the panel, the router and «Расход» know this contract only; each system stands behind it by a plug (ACP, a
//! command, MCP, events in) and its card.
//!
//! The rules (docs/plans/work-systems.md, «Rules for fitting in and for updates»): a system says what it can after
//! the handshake (`Can`), and every verb it cannot answers «cannot» by default, so a plug writes only what its system
//! does; the contract grows only by adding (a new verb is optional and has a flag); an unknown event arrives as
//! `Unknown` with its raw text; the messages are plain data, so they can travel as JSON over the home link.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The contract's number: a card names the one it was written for.
pub const CONTRACT: u32 = 1;

/// Our own system's id: the works runner of the work folders.
pub const DUOWAVE: &str = "duowave";

/// The states of a work, one list for every system: each plug or card turns its system's words into these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemState {
    Queued,
    Working,
    /// A question waits for the person.
    WaitsPerson,
    /// A limit holds it (a plan's, a budget's, a provider's).
    WaitsLimit,
    Ready,
    Unfinished,
    Failed,
    Stopped,
    Interrupted,
}

impl SystemState {
    /// It goes on: queued, working, or waiting.
    pub fn going(self) -> bool {
        matches!(self, SystemState::Queued | SystemState::Working | SystemState::WaitsPerson | SystemState::WaitsLimit)
    }

    /// A system's own word for a state, as its card maps it; an unknown word is the nearest of the nine: working.
    pub fn parse(word: &str) -> SystemState {
        match word {
            "queued" => SystemState::Queued,
            "working" => SystemState::Working,
            "waits_person" => SystemState::WaitsPerson,
            "waits_limit" => SystemState::WaitsLimit,
            "ready" => SystemState::Ready,
            "unfinished" => SystemState::Unfinished,
            "failed" => SystemState::Failed,
            "stopped" => SystemState::Stopped,
            "interrupted" => SystemState::Interrupted,
            _ => SystemState::Working,
        }
    }
}

/// How exact a system's spend is: through our router or a key we keep (a deal), told by the system, or not at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Spend {
    Exact,
    Told,
    #[default]
    None,
}

/// What a system can, after the handshake. Voice and the panel follow these: no `tell_now` — «Эта система вдогонку не
/// принимает», no `steps` — «как там» answers with the state and the time, no `stop` — «откройте её саму».
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Can {
    pub start: bool,
    pub list: bool,
    pub steps: bool,
    pub tell_now: bool,
    pub tell_after: bool,
    pub stop: bool,
    pub questions: bool,
    pub result: bool,
    pub artifacts: bool,
    pub spend: Spend,
    pub limit: bool,
    pub places: bool,
    pub setups: bool,
    pub ceiling: bool,
    pub open: bool,
    pub actions: bool,
}

/// A system as people and the journal know it: its title, the names it is said by, its plug and the protocol agreed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct About {
    pub id: String,
    pub title: String,
    /// The names a person says it by, in the household's languages («бэбэ», «bb»).
    #[serde(default)]
    pub names: Vec<String>,
    /// `runner` (ours), `acp`, `command`, `mcp`, `hooks`.
    pub plug: String,
    /// The protocol and its version agreed, when it has one: `ACP 2`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// The system's own version, as it said it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Whether a system can take works now, and why not in words.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemReady {
    pub ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

impl SystemReady {
    pub fn yes() -> SystemReady {
        SystemReady { ready: true, why: None }
    }

    pub fn no(why: impl Into<String>) -> SystemReady {
        SystemReady { ready: false, why: Some(why.into()) }
    }
}

/// Where a system can work: a bb project, a work folder, a session's folder. `path` matches it to a work folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// What a system can think with: an agent, a model, a preset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setup {
    pub id: String,
    pub title: String,
}

/// A work asked of a system: the task in the person's words, where, on what, its title, a ceiling in dollars.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Start {
    pub task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    /// The model of the work's set for this system, in its own words (stage 5): T3 Code's begins no work without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ceiling_usd: Option<f64>,
    /// Who asked: the home's admin may, a guest may not (the layer checks it before).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
}

/// A work as a system shows it: its id there, title, place, state and one line, when it last changed, and whether it
/// was begun outside DuoWave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Seen {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    pub state: SystemState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// When it last changed, ms since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    #[serde(default)]
    pub outside: bool,
}

/// A work's state now: the state, one line, and how old the knowledge is (`at`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: SystemState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
}

/// What a work did, in order, from a cursor: plain lines, and the cursor to go on from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Steps {
    pub lines: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// When added words go in: into the going turn, or after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum When {
    Now,
    After,
}

/// Why a system did not do what it was asked, in words people hear; `cannot` — it does not do this at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Refusal {
    pub why: String,
    #[serde(default)]
    pub cannot: bool,
}

impl Refusal {
    /// The system does not do this verb.
    pub fn cannot(verb: &str) -> Refusal {
        Refusal { why: format!("the system has no «{verb}»"), cannot: true }
    }

    pub fn because(why: impl Into<String>) -> Refusal {
        Refusal { why: why.into(), cannot: false }
    }
}

/// What a question of a system waits for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expects {
    YesNo,
    Choice { options: Vec<String> },
    Free,
}

/// A question of a system to the person: a permission, a choice, free words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub id: String,
    pub text: String,
    pub expects: Expects,
    /// Marked dangerous: never approved by voice (D3), only by a tap.
    #[serde(default)]
    pub dangerous: bool,
}

/// The person's answer to a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Reply {
    Yes,
    No,
    Choice(usize),
    Free(String),
}

/// What a work left: a branch, a pull request, a file, a link, a text. Links, not copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Branch,
    PullRequest,
    File,
    Link,
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub kind: ArtifactKind,
    pub title: String,
    /// Where it is: a branch's name, a URL, a path.
    pub at: String,
}

/// What came of a work: a summary, more of it for «подробнее», what it left.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub more: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
}

/// How to show a work in the system itself: a command for a terminal, or an address.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenIn {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// What a system tells by itself about one of its works.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum SystemEvent {
    /// A work began, here or outside.
    Began { seen: Seen },
    /// Its state changed.
    State { work: String, state: SystemState, line: Option<String> },
    /// A step of it, in a line.
    Step { work: String, line: String },
    /// It asks the person.
    Asks { work: String, question: Question },
    /// What it spent so far, in millionths of a dollar, and how exact that is.
    Spent { work: String, usd_micro: u64, mark: Spend },
    /// A limit holds it: until when, if said.
    Limit { work: String, until: Option<u64> },
    /// It left something.
    Left { work: String, artifact: Artifact },
    /// It ended: how, and what came of it.
    Ended { work: String, state: SystemState, outcome: Outcome },
    /// Anything else, as it came: counted, never acted on.
    Unknown { work: Option<String>, raw: String },
}

impl SystemEvent {
    /// The work it is about.
    pub fn work(&self) -> Option<&str> {
        match self {
            SystemEvent::Began { seen } => Some(&seen.id),
            SystemEvent::State { work, .. }
            | SystemEvent::Step { work, .. }
            | SystemEvent::Asks { work, .. }
            | SystemEvent::Spent { work, .. }
            | SystemEvent::Limit { work, .. }
            | SystemEvent::Left { work, .. }
            | SystemEvent::Ended { work, .. } => Some(work),
            SystemEvent::Unknown { work, .. } => work.as_deref(),
        }
    }
}

/// Where a system's events go: its id and the event.
pub type SystemListener = Arc<dyn Fn(&str, SystemEvent) + Send + Sync>;

/// A system that leads works by its own rules. Every verb but `ready` answers «cannot» unless the system has it;
/// `can` says which it has.
#[async_trait]
pub trait WorkSystem: Send + Sync {
    /// The card's id; `duowave` is ours.
    fn id(&self) -> String;
    fn about(&self) -> About;
    async fn ready(&self) -> SystemReady;
    /// A quick look, with nothing run, whether it can be asked now (its program is there): what a question offers
    /// only possible answers by. `ready` is the full answer.
    fn present(&self) -> bool {
        true
    }
    fn can(&self) -> Can;
    async fn places(&self) -> Vec<Place> {
        Vec::new()
    }
    async fn setups(&self) -> Vec<Setup> {
        Vec::new()
    }
    async fn start(&self, _start: Start) -> Result<Seen, Refusal> {
        Err(Refusal::cannot("start"))
    }
    async fn list(&self) -> Vec<Seen> {
        Vec::new()
    }
    async fn status(&self, _work: &str) -> Option<Status> {
        None
    }
    async fn steps(&self, _work: &str, _after: Option<&str>) -> Steps {
        Steps::default()
    }
    async fn tell(&self, _work: &str, _text: &str, _when: When) -> Result<(), Refusal> {
        Err(Refusal::cannot("tell"))
    }
    async fn stop(&self, _work: &str) -> Result<(), Refusal> {
        Err(Refusal::cannot("stop"))
    }
    async fn questions(&self, _work: &str) -> Vec<Question> {
        Vec::new()
    }
    async fn answer(&self, _work: &str, _question: &str, _reply: Reply) -> Result<(), Refusal> {
        Err(Refusal::cannot("answer"))
    }
    async fn result(&self, _work: &str) -> Option<Outcome> {
        None
    }
    fn open(&self, _work: &str) -> Option<OpenIn> {
        None
    }
    /// «Открой»: show the work in the system itself, by its `open` command run where the system lives.
    async fn show(&self, _work: &str) -> Result<(), Refusal> {
        Err(Refusal::cannot("open"))
    }
    async fn act(&self, _work: &str, _action: &str, _args: Value) -> Result<Value, Refusal> {
        Err(Refusal::cannot("act"))
    }
    /// Its events from now on; once, when it joins the registry.
    fn listen(&self, _events: SystemListener) {}
}

/// The tools of MCP servers, for a plug that leads a system through MCP (docs/plans/work-systems.md, «Plugs», 3): a
/// server as a card names it, by its id or its title.
#[async_trait]
pub trait McpCaller: Send + Sync {
    /// The server is on and connected now.
    fn on_line(&self, server: &str) -> bool;
    /// A tool's answer as JSON: its structured content, else its text read as JSON, else the text; an error in words.
    async fn call(&self, server: &str, tool: &str, args: serde_json::Map<String, Value>) -> Result<Value, String>;
}

/// The conformance kit (rule 7): what a system must do to stand behind the contract. A task is started only when the
/// system can start and `task` is given; a work started is stopped again when it can stop. The problems found, in
/// words; none — it passes.
pub async fn check(system: &dyn WorkSystem, task: Option<&str>) -> Vec<String> {
    let mut problems = Vec::new();
    let id = system.id();
    if id.trim().is_empty() {
        problems.push("no id".to_string());
    }
    let about = system.about();
    if about.id != id {
        problems.push(format!("about.id «{}» is not the id «{id}»", about.id));
    }
    if about.title.trim().is_empty() {
        problems.push("no title".into());
    }
    if about.plug.trim().is_empty() {
        problems.push("no plug".into());
    }
    let can = system.can();
    if !can.start && !can.list {
        problems.push("neither start nor list".into());
    }
    let ready = system.ready().await;
    if !ready.ready && ready.why.as_deref().is_none_or(|why| why.trim().is_empty()) {
        problems.push("not ready, and no words why".into());
    }
    // A verb it says it has not answers «cannot».
    let unknown = "no-such-work";
    if !can.tell_now && !can.tell_after {
        if let Err(refusal) = system.tell(unknown, "x", When::Now).await {
            if !refusal.cannot {
                problems.push("tell: no flag, yet not «cannot»".into());
            }
        } else {
            problems.push("tell: no flag, yet it took words".into());
        }
    }
    if !can.stop && system.stop(unknown).await.is_ok() {
        problems.push("stop: no flag, yet it stopped".into());
    }
    if can.stop && system.stop(unknown).await.is_ok() {
        problems.push("stop: an unknown work «stopped»".into());
    }
    if !can.places && !system.places().await.is_empty() {
        problems.push("places: no flag, yet places".into());
    }
    if can.list {
        for seen in system.list().await {
            if seen.id.trim().is_empty() {
                problems.push("list: a work without an id".into());
            }
        }
    }
    if let (true, Some(task), true) = (can.start, task, ready.ready) {
        let place = system.places().await.into_iter().next().map(|place| place.id);
        match system.start(Start { task: task.into(), place, ..Start::default() }).await {
            Ok(seen) => {
                if seen.id.trim().is_empty() {
                    problems.push("start: no id".into());
                }
                if seen.outside {
                    problems.push("start: a work begun here is marked outside".into());
                }
                if system.status(&seen.id).await.is_none() {
                    problems.push("status: no state of the work just started".into());
                }
                if can.stop {
                    if let Err(refusal) = system.stop(&seen.id).await {
                        problems.push(format!("stop: {}", refusal.why));
                    }
                }
            }
            Err(refusal) => problems.push(format!("start: {}", refusal.why)),
        }
    }
    problems
}

/// A system for tests (feature `testing`): in memory, its flags as given; a work started goes on until stopped or
/// ended by `end`.
#[cfg(any(test, feature = "testing"))]
pub mod stand_in {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    pub struct StandIn {
        pub id: String,
        pub can: Can,
        pub ready: bool,
        works: Mutex<HashMap<String, (Seen, Vec<String>)>>,
        listeners: Mutex<Vec<SystemListener>>,
        next: Mutex<u32>,
        replies: Mutex<Vec<(String, Reply)>>,
    }

    impl StandIn {
        pub fn new(id: &str, can: Can) -> StandIn {
            StandIn {
                id: id.into(),
                can,
                ready: true,
                works: Mutex::new(HashMap::new()),
                listeners: Mutex::new(Vec::new()),
                next: Mutex::new(0),
                replies: Mutex::new(Vec::new()),
            }
        }

        /// Everything it can.
        pub fn full(id: &str) -> StandIn {
            let can = Can {
                start: true,
                list: true,
                steps: true,
                tell_now: true,
                tell_after: true,
                stop: true,
                questions: true,
                result: true,
                places: true,
                open: true,
                ..Can::default()
            };
            StandIn::new(id, can)
        }

        fn tell_all(&self, event: SystemEvent) {
            let listeners: Vec<SystemListener> = self.listeners.lock().unwrap().clone();
            for listener in listeners {
                listener(&self.id, event.clone());
            }
        }

        /// A work begun in the system by hand: told as begun outside.
        pub fn begin_outside(&self, title: &str) -> String {
            let id = self.next_id();
            let seen = Seen {
                id: id.clone(),
                title: title.into(),
                place: Some("p1".into()),
                state: SystemState::Working,
                line: None,
                at: None,
                outside: true,
            };
            self.works.lock().unwrap().insert(id.clone(), (seen.clone(), Vec::new()));
            self.tell_all(SystemEvent::Began { seen });
            id
        }

        /// The work ends: told, with a summary.
        pub fn end(&self, work: &str, state: SystemState, summary: &str) {
            if let Some((seen, _)) = self.works.lock().unwrap().get_mut(work) {
                seen.state = state;
                seen.line = Some(summary.into());
            }
            let outcome = Outcome { summary: Some(summary.into()), ..Outcome::default() };
            self.tell_all(SystemEvent::Ended { work: work.into(), state, outcome });
        }

        /// The system asks the person about a work: a question of rights, dangerous or not. Its id.
        pub fn ask(&self, work: &str, text: &str, dangerous: bool) -> String {
            let id = format!("q-{}", self.next_id());
            let question = Question { id: id.clone(), text: text.into(), expects: Expects::YesNo, dangerous };
            self.tell_all(SystemEvent::Asks { work: work.into(), question });
            id
        }

        /// The answers it got, by question.
        pub fn replies(&self) -> Vec<(String, Reply)> {
            self.replies.lock().unwrap().clone()
        }

        /// What was told to a work.
        pub fn told(&self, work: &str) -> Vec<String> {
            self.works.lock().unwrap().get(work).map(|(_, told)| told.clone()).unwrap_or_default()
        }

        fn next_id(&self) -> String {
            let mut next = self.next.lock().unwrap();
            *next += 1;
            format!("w{}", *next)
        }
    }

    #[async_trait]
    impl WorkSystem for StandIn {
        fn id(&self) -> String {
            self.id.clone()
        }

        fn about(&self) -> About {
            About {
                id: self.id.clone(),
                title: format!("Stand-in {}", self.id),
                names: vec![self.id.clone()],
                plug: "stand-in".into(),
                protocol: None,
                version: Some("1.0.0".into()),
            }
        }

        async fn ready(&self) -> SystemReady {
            if self.ready {
                SystemReady::yes()
            } else {
                SystemReady::no("the stand-in is off")
            }
        }

        fn present(&self) -> bool {
            self.ready
        }

        fn can(&self) -> Can {
            self.can.clone()
        }

        async fn places(&self) -> Vec<Place> {
            if !self.can.places {
                return Vec::new();
            }
            vec![Place { id: "p1".into(), title: "Сайт".into(), path: Some("/work/site".into()) }]
        }

        async fn start(&self, start: Start) -> Result<Seen, Refusal> {
            if !self.can.start {
                return Err(Refusal::cannot("start"));
            }
            let id = self.next_id();
            let seen = Seen {
                id: id.clone(),
                title: start.title.unwrap_or_else(|| start.task.clone()),
                place: start.place,
                state: SystemState::Working,
                line: None,
                at: None,
                outside: false,
            };
            self.works.lock().unwrap().insert(id, (seen.clone(), Vec::new()));
            self.tell_all(SystemEvent::Began { seen: seen.clone() });
            Ok(seen)
        }

        async fn list(&self) -> Vec<Seen> {
            if !self.can.list {
                return Vec::new();
            }
            self.works.lock().unwrap().values().map(|(seen, _)| seen.clone()).collect()
        }

        async fn status(&self, work: &str) -> Option<Status> {
            let works = self.works.lock().unwrap();
            let (seen, _) = works.get(work)?;
            Some(Status { state: seen.state, line: seen.line.clone(), at: seen.at })
        }

        async fn tell(&self, work: &str, text: &str, when: When) -> Result<(), Refusal> {
            let able = match when {
                When::Now => self.can.tell_now,
                When::After => self.can.tell_after,
            };
            if !able {
                return Err(Refusal::cannot("tell"));
            }
            let mut works = self.works.lock().unwrap();
            let Some((_, told)) = works.get_mut(work) else { return Err(Refusal::because("no such work")) };
            told.push(text.into());
            Ok(())
        }

        async fn stop(&self, work: &str) -> Result<(), Refusal> {
            if !self.can.stop {
                return Err(Refusal::cannot("stop"));
            }
            let known = self.works.lock().unwrap().contains_key(work);
            if !known {
                return Err(Refusal::because("no such work"));
            }
            self.end(work, SystemState::Stopped, "stopped");
            Ok(())
        }

        async fn answer(&self, _work: &str, question: &str, reply: Reply) -> Result<(), Refusal> {
            if !self.can.questions {
                return Err(Refusal::cannot("answer"));
            }
            self.replies.lock().unwrap().push((question.into(), reply));
            Ok(())
        }

        async fn result(&self, work: &str) -> Option<Outcome> {
            let works = self.works.lock().unwrap();
            let (seen, _) = works.get(work)?;
            Some(Outcome { summary: seen.line.clone(), ..Outcome::default() })
        }

        fn open(&self, work: &str) -> Option<OpenIn> {
            self.can.open.then(|| OpenIn { command: Some(format!("stand-in open {work}")), url: None })
        }

        async fn show(&self, work: &str) -> Result<(), Refusal> {
            if !self.can.open || !self.works.lock().unwrap().contains_key(work) {
                return Err(Refusal::cannot("open"));
            }
            Ok(())
        }

        fn listen(&self, events: SystemListener) {
            self.listeners.lock().unwrap().push(events);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::StandIn;
    use super::*;

    #[tokio::test]
    async fn the_stand_in_passes_the_contract_full_or_bare() {
        assert_eq!(check(&StandIn::full("bb"), Some("поправь опечатку")).await, Vec::<String>::new());
        // A system that only lists its works (Claude Code by hand): nothing it cannot is asked of it.
        let lists = StandIn::new("claude-code", Can { list: true, ..Can::default() });
        assert_eq!(check(&lists, Some("поправь опечатку")).await, Vec::<String>::new());
    }

    #[tokio::test]
    async fn the_kit_finds_what_a_system_gets_wrong() {
        let mut off = StandIn::new("off", Can::default());
        off.ready = false;
        let problems = check(&off, None).await;
        assert!(problems.iter().any(|problem| problem.contains("neither start nor list")), "{problems:?}");
    }

    #[test]
    fn states_are_plain_data_and_unknown_words_are_working() {
        assert_eq!(serde_json::to_value(SystemState::WaitsPerson).unwrap(), "waits_person");
        assert_eq!(SystemState::parse("active"), SystemState::Working);
        assert!(SystemState::WaitsLimit.going() && !SystemState::Ready.going());
        let event = SystemEvent::Ended {
            work: "w1".into(),
            state: SystemState::Ready,
            outcome: Outcome { summary: Some("готово".into()), ..Outcome::default() },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event"], "ended");
        assert_eq!(serde_json::from_value::<SystemEvent>(json).unwrap(), event);
        assert_eq!(serde_json::to_value(Reply::Choice(2)).unwrap(), serde_json::json!({"kind": "choice", "value": 2}));
    }
}
