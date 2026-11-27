//! A Турбо's work systems on the hub (docs/plans/work-systems.md, stage 9; block В of docs/plans/router-and-works.md):
//! each system of a helper that gives `works` is a system here too, by the id `<system>@<entry>`, with its verbs and
//! its events over that computer's `/home/v1/systems/call` (home_server/systems.rs there). A slow look reads the
//! events after the last number, and once a minute the list of systems and the work folders with who does their works
//! by that computer's setup. So a station hears «готово» of a work in bb on the computer, and a phrase said to it
//! begins one there; the station runs no program of a system itself.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::future::BoxFuture;
use serde_json::{json, Value};

use crate::talk::system::{
    About, Can, OpenIn, Outcome, Place, Refusal, Reply, Seen, Start, Status, SystemEvent, SystemListener, SystemReady,
    When, WorkSystem,
};
use crate::talk::systems::{Journaled, RemoteFolder, Systems};

/// How often the events are read; how often the list.
const LOOK_EVERY: Duration = Duration::from_secs(5);
const LIST_EVERY: Duration = Duration::from_secs(60);
/// The Турбо's path of its systems' letters.
pub const PATH: &str = "/home/v1/systems/call";

/// A letter to a Турбо and its answer.
pub type Call = Arc<dyn Fn(Value) -> BoxFuture<'static, Result<Value, String>> + Send + Sync>;
/// The Турбо that give `works` now: each by its entry, with the way to write to it.
pub type Source = Arc<dyn Fn() -> Vec<(String, Call)> + Send + Sync>;

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The Турбо of this hub that give `works` now, each written to at its `/home/v1/systems/call` with its credential.
pub fn source_of(helpers: &Arc<super::Helpers>) -> Source {
    let helpers = Arc::downgrade(helpers);
    Arc::new(move || {
        let Some(helpers) = helpers.upgrade() else { return Vec::new() };
        helpers
            .pick_all(crate::helpers::contract::Ability::Works)
            .into_iter()
            .filter_map(|giver| match giver {
                super::Giver::Helper(remote) => Some((remote.id().to_string(), call_of(remote))),
                super::Giver::Itself => None,
            })
            .collect()
    })
}

/// A letter to one Турбо: its answer, or why not in words.
fn call_of(remote: Arc<super::remote::RemoteHelper>) -> Call {
    Arc::new(move |letter: Value| {
        let (url, key) = (format!("{}{PATH}", remote.base().trim_end_matches('/')), remote.credential());
        Box::pin(async move {
            let response = crate::utils::http::client_without_redirects()
                .post(url)
                .bearer_auth(key)
                .timeout(Duration::from_secs(15))
                .json(&letter)
                .send()
                .await
                .map_err(|_| crate::tr!("Турбо не ответило."))?;
            let status = response.status();
            let answer: Value = response.json().await.unwrap_or(Value::Null);
            if !status.is_success() {
                let words = answer.get("message").and_then(Value::as_str).map(str::to_string);
                return Err(
                    words.unwrap_or_else(|| crate::tr!("Турбо отказало (HTTP {status}).", status = status.as_u16()))
                );
            }
            Ok(answer.get("message").cloned().unwrap_or(Value::Null))
        })
    })
}

/// The hub's id of a Турбо's system.
pub fn id_of(system: &str, entry: &str) -> String {
    format!("{system}@{entry}")
}

/// The Турбо's systems as the hub keeps them.
pub struct RemoteSystems {
    systems: Weak<Systems>,
    source: Source,
    made: Mutex<HashMap<String, Arc<RemoteSystem>>>,
    since: Mutex<HashMap<String, u64>>,
    listed: Mutex<HashMap<String, Instant>>,
    folders: Mutex<HashMap<String, Vec<RemoteFolder>>>,
}

impl RemoteSystems {
    pub fn new(systems: &Arc<Systems>, source: Source) -> Arc<RemoteSystems> {
        Arc::new(RemoteSystems {
            systems: Arc::downgrade(systems),
            source,
            made: Mutex::new(HashMap::new()),
            since: Mutex::new(HashMap::new()),
            listed: Mutex::new(HashMap::new()),
            folders: Mutex::new(HashMap::new()),
        })
    }

    /// The slow look, while the hub lives (none outside a runtime).
    pub fn run(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
        handle.spawn(async move {
            loop {
                tokio::time::sleep(LOOK_EVERY).await;
                let Some(me) = me.upgrade() else { break };
                me.look().await;
            }
        });
    }

    /// The Турбо's work folders now, with who does their works there: for a phrase said to the hub.
    pub fn folders(&self) -> Vec<RemoteFolder> {
        locked(&self.folders).values().flatten().cloned().collect()
    }

    /// One look at every Турбо that gives works: its list when due, its events after the last number.
    pub async fn look(&self) {
        for (entry, call) in (self.source)() {
            let due = locked(&self.listed).get(&entry).is_none_or(|at| at.elapsed() >= LIST_EVERY);
            if due {
                self.list(&entry, &call).await;
            }
            self.events(&entry, &call).await;
        }
    }

    /// The Турбо's systems and folders: each system joins the hub's registry (made once, its look refreshed).
    async fn list(&self, entry: &str, call: &Call) {
        let Some(systems) = self.systems.upgrade() else { return };
        let Ok(answer) = call(json!({ "verb": "systems" })).await else { return };
        locked(&self.listed).insert(entry.to_string(), Instant::now());
        for listed in answer.get("systems").and_then(Value::as_array).cloned().unwrap_or_default() {
            let Ok(about) = serde_json::from_value::<About>(listed["about"].clone()) else { continue };
            let can: Can = serde_json::from_value(listed["can"].clone()).unwrap_or_default();
            let ready: SystemReady = serde_json::from_value(listed["ready"].clone()).unwrap_or_default();
            let present = listed["present"].as_bool().unwrap_or(false);
            let id = id_of(&about.id, entry);
            let made = locked(&self.made).get(&id).cloned();
            match made {
                Some(system) => system.refresh(about, can, ready, present),
                None => {
                    let system = Arc::new(RemoteSystem::new(&about.id, entry, call.clone()));
                    system.refresh(about, can, ready, present);
                    systems.add(system.clone());
                    locked(&self.made).insert(id, system);
                }
            }
        }
        let folders = answer.get("folders").and_then(Value::as_array).cloned().unwrap_or_default();
        let folders: Vec<RemoteFolder> = folders
            .iter()
            .filter_map(|folder| {
                Some(RemoteFolder {
                    entry: entry.to_string(),
                    folder: folder["folder"].as_str()?.to_string(),
                    name: folder["name"].as_str().unwrap_or_default().to_string(),
                    path: folder["path"].as_str().unwrap_or_default().to_string(),
                    system: id_of(folder["system"].as_str().unwrap_or(crate::talk::system::DUOWAVE), entry),
                })
            })
            .collect();
        locked(&self.folders).insert(entry.to_string(), folders);
    }

    /// The events after the last number, to the systems they are of.
    async fn events(&self, entry: &str, call: &Call) {
        let since = locked(&self.since).get(entry).copied();
        let Ok(answer) = call(json!({ "verb": "events", "since": since.unwrap_or(0) })).await else { return };
        let last = answer.get("last").and_then(Value::as_u64).unwrap_or(0);
        locked(&self.since).insert(entry.to_string(), last);
        // The first look takes no old events: what was before the hub looked is not news.
        if since.is_none() {
            return;
        }
        let events: Vec<Journaled> = serde_json::from_value(answer["events"].clone()).unwrap_or_default();
        for kept in events {
            let system = locked(&self.made).get(&id_of(&kept.system, entry)).cloned();
            if let Some(system) = system {
                system.emit(kept.event);
            }
        }
    }
}

/// A Турбо's system as the hub leads it: every verb a letter over the home link.
pub struct RemoteSystem {
    id: String,
    system: String,
    call: Call,
    about: Mutex<About>,
    can: Mutex<Can>,
    ready: Mutex<SystemReady>,
    present: AtomicBool,
    listeners: Mutex<Vec<SystemListener>>,
}

impl RemoteSystem {
    pub fn new(system: &str, entry: &str, call: Call) -> RemoteSystem {
        RemoteSystem {
            id: id_of(system, entry),
            system: system.to_string(),
            call,
            about: Mutex::new(About::default()),
            can: Mutex::new(Can::default()),
            ready: Mutex::new(SystemReady::no(crate::tr!("Турбо ещё не ответило."))),
            present: AtomicBool::new(false),
            listeners: Mutex::new(Vec::new()),
        }
    }

    pub fn refresh(&self, about: About, can: Can, ready: SystemReady, present: bool) {
        *locked(&self.about) = about;
        *locked(&self.can) = can;
        *locked(&self.ready) = ready;
        self.present.store(present, Ordering::Relaxed);
    }

    pub fn emit(&self, event: SystemEvent) {
        let listeners: Vec<SystemListener> = locked(&self.listeners).clone();
        for listener in listeners {
            listener(&self.id, event.clone());
        }
    }

    /// A verb of this system: the letter, the answer; the Турбо away — its words.
    async fn letter(&self, verb: &str, more: Value) -> Result<Value, Refusal> {
        let mut letter = json!({ "verb": verb, "system": self.system });
        if let (Some(letter), Some(more)) = (letter.as_object_mut(), more.as_object()) {
            letter.extend(more.clone());
        }
        let answer = (self.call)(letter).await.map_err(Refusal::because)?;
        match answer.get("refused") {
            Some(refused) => {
                Err(serde_json::from_value(refused.clone()).unwrap_or_else(|_| Refusal::because("refused")))
            }
            None => Ok(answer),
        }
    }
}

#[async_trait]
impl WorkSystem for RemoteSystem {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn about(&self) -> About {
        let mut about = locked(&self.about).clone();
        about.id = self.id.clone();
        about
    }

    async fn ready(&self) -> SystemReady {
        locked(&self.ready).clone()
    }

    fn present(&self) -> bool {
        self.present.load(Ordering::Relaxed)
    }

    fn can(&self) -> Can {
        locked(&self.can).clone()
    }

    async fn places(&self) -> Vec<Place> {
        let answer = self.letter("places", json!({})).await;
        answer.ok().and_then(|answer| serde_json::from_value(answer).ok()).unwrap_or_default()
    }

    async fn start(&self, start: Start) -> Result<Seen, Refusal> {
        let answer = self.letter("start", json!({ "start": start })).await?;
        serde_json::from_value(answer).map_err(|error| Refusal::because(error.to_string()))
    }

    async fn list(&self) -> Vec<Seen> {
        let answer = self.letter("list", json!({})).await;
        answer.ok().and_then(|answer| serde_json::from_value(answer).ok()).unwrap_or_default()
    }

    async fn status(&self, work: &str) -> Option<Status> {
        let answer = self.letter("status", json!({ "work": work })).await.ok()?;
        serde_json::from_value(answer).ok()
    }

    async fn tell(&self, work: &str, text: &str, when: When) -> Result<(), Refusal> {
        self.letter("tell", json!({ "work": work, "text": text, "when": when })).await.map(|_| ())
    }

    async fn stop(&self, work: &str) -> Result<(), Refusal> {
        self.letter("stop", json!({ "work": work })).await.map(|_| ())
    }

    async fn answer(&self, work: &str, question: &str, reply: Reply) -> Result<(), Refusal> {
        self.letter("answer", json!({ "work": work, "question": question, "reply": reply })).await.map(|_| ())
    }

    async fn result(&self, work: &str) -> Option<Outcome> {
        let answer = self.letter("result", json!({ "work": work })).await.ok()?;
        serde_json::from_value(answer).ok()
    }

    fn open(&self, _work: &str) -> Option<OpenIn> {
        // A command to open shows on the computer it runs on: the Турбо's own panel says it.
        None
    }

    fn listen(&self, events: SystemListener) {
        locked(&self.listeners).push(events);
    }
}
