//! The work systems the home knows (docs/plans/work-systems.md, «The contract»), and the executor that leads their
//! works in the talk layer: `system`. Our own runner stays behind its folder executor (talk/folder.rs), with its
//! budgets, limits and questions; it is here as the system `duowave` for what the contract asks of every system (its
//! places, its agents, the kit). A work of any other system goes through this executor, by the contract's verbs and
//! events only.
//!
//! A work's target is `<system>/<its id there>`. A work the layer starts has a target of its own until the system
//! answers (`<system>/~<the work's id>`); then the system's id, which the layer takes by the work's id.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::Duration;

use futures::future::BoxFuture;
use futures::FutureExt;
use serde::Serialize;

use super::asked::Kind;
use super::executor::{
    Began, ByVoice, Closed, Doing, Executor, Facts, NotAnswered, Put, Report, Reporter, Started, Told,
};
use super::origin::{Activator, Origin};
use super::pack::{self, fill};
use super::system::{
    About, Can, Outcome, Question, Reply, Start, SystemEvent, SystemReady, SystemState, When, WorkSystem, DUOWAVE,
};
use super::works::{End, Work, WorkState};
use crate::utils::time::now_ms;

/// How often the systems that list their works are asked for those begun by hand.
const POLL_EVERY: std::time::Duration = std::time::Duration::from_secs(60);
/// How long a system's question waits for the person in the layer.
const ASK_FOR_MS: u64 = 120_000;

/// The executor's name in the records.
pub const SYSTEM_EXECUTOR: &str = "system";

/// The system of a target: what comes before its first `/`.
pub fn system_of(target: &str) -> Option<&str> {
    target.split_once('/').map(|(system, _)| system).filter(|system| !system.is_empty())
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The systems of this gateway: ours and those whose cards are installed. One of an id at a time.
#[derive(Default)]
pub struct Systems {
    list: RwLock<Vec<Arc<dyn WorkSystem>>>,
    /// Who hears every system's events: told to each system as it joins.
    listeners: RwLock<Vec<super::system::SystemListener>>,
    /// The systems of cards installed later (a card's block `system`) are looked for anew before a look at the list,
    /// at most once in `REFRESH_EVERY`.
    refresh: OnceLock<Arc<dyn Fn() + Send + Sync>>,
    refreshed: Mutex<Option<std::time::Instant>>,
    /// The last events of every system, numbered (stage 9): what a hub asks this computer for over the home link.
    journal: Arc<Mutex<std::collections::VecDeque<Journaled>>>,
    next_event: Arc<AtomicU64>,
}

/// An event as the journal keeps it: its number, its system, the event.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct Journaled {
    pub n: u64,
    pub system: String,
    pub event: super::system::SystemEvent,
}

/// How many events the journal keeps.
const JOURNAL: usize = 300;

/// A work folder of a Турбо as the hub sees it (stage 9): its helper's entry, the folder, and who does its works by
/// that computer's own setup.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, serde::Deserialize)]
pub struct RemoteFolder {
    pub entry: String,
    pub folder: String,
    pub name: String,
    pub path: String,
    /// The system that gets its works there, as this hub calls it: `<system>@<entry>`.
    pub system: String,
}

/// The Турбо's folders now, as the home floor keeps them.
pub type RemoteFolders = Arc<dyn Fn() -> Vec<RemoteFolder> + Send + Sync>;

/// How often the cards' systems are looked for anew.
const REFRESH_EVERY: Duration = Duration::from_secs(3);

/// A system as the panel shows it: who it is, what it can, whether it is ready.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemView {
    #[serde(flatten)]
    pub about: About,
    pub can: Can,
    pub ready: SystemReady,
}

impl Systems {
    pub fn new() -> Arc<Systems> {
        let systems = Arc::new(Systems::default());
        // Every system's events into the journal, numbered.
        let (journal, next) = (systems.journal.clone(), systems.next_event.clone());
        systems.listen(Arc::new(move |system: &str, event: super::system::SystemEvent| {
            let n = next.fetch_add(1, Ordering::Relaxed) + 1;
            let mut journal = locked(&journal);
            journal.push_back(Journaled { n, system: system.to_string(), event });
            while journal.len() > JOURNAL {
                journal.pop_front();
            }
        }));
        systems
    }

    /// The events after number `since`, and the last number (stage 9).
    pub fn journal_since(&self, since: u64) -> (Vec<Journaled>, u64) {
        let journal = locked(&self.journal);
        let events = journal.iter().filter(|kept| kept.n > since).cloned().collect();
        (events, self.next_event.load(Ordering::Relaxed))
    }

    /// A system joins, in place of one of its id.
    pub fn add(&self, system: Arc<dyn WorkSystem>) {
        for listener in read(&self.listeners).iter() {
            system.listen(listener.clone());
        }
        let mut list = write(&self.list);
        list.retain(|known| known.id() != system.id());
        list.push(system);
    }

    /// A system leaves (its card was removed).
    pub fn remove(&self, id: &str) {
        write(&self.list).retain(|known| known.id() != id);
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn WorkSystem>> {
        self.fresh();
        read(&self.list).iter().find(|system| system.id() == id).cloned()
    }

    pub fn all(&self) -> Vec<Arc<dyn WorkSystem>> {
        self.fresh();
        read(&self.list).clone()
    }

    /// Every system's names as people say them: its title and its names (for «Чутьё»'s options, stage 7).
    pub fn names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for system in self.all() {
            let about = system.about();
            for name in std::iter::once(about.title).chain(about.names) {
                if !name.trim().is_empty() && !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }

    /// What looks for the cards' systems anew (it adds and removes them here).
    pub fn set_refresh(&self, refresh: Arc<dyn Fn() + Send + Sync>) {
        let _ = self.refresh.set(refresh);
    }

    fn fresh(&self) {
        let Some(refresh) = self.refresh.get() else { return };
        {
            let mut at = locked(&self.refreshed);
            if at.is_some_and(|at| at.elapsed() < REFRESH_EVERY) {
                return;
            }
            *at = Some(std::time::Instant::now());
        }
        refresh();
    }

    /// Every system's events go here from now on, those of systems that join later too.
    pub fn listen(&self, listener: super::system::SystemListener) {
        for system in read(&self.list).iter() {
            system.listen(listener.clone());
        }
        write(&self.listeners).push(listener);
    }

    /// What the panel shows of each.
    pub async fn views(&self) -> Vec<SystemView> {
        let mut views = Vec::new();
        for system in self.all() {
            views.push(SystemView { about: system.about(), can: system.can(), ready: system.ready().await });
        }
        views
    }
}

/// What is known of a work of a system: told by its events, or by its start.
#[derive(Debug, Clone)]
struct Known {
    state: SystemState,
    line: Option<String>,
    outcome: Option<Outcome>,
    usd_micro: Option<u64>,
}

/// The executor of the works of every system but ours.
pub struct SystemExecutor {
    systems: Arc<Systems>,
    me: Weak<SystemExecutor>,
    layer: Weak<super::TalkLayer>,
    /// The question each work's system waits on, by the layer's target.
    questions: Mutex<HashMap<String, Question>>,
    /// Where «who gets a work» is kept (router/who.rs); none — ours for every work.
    settings: OnceLock<Arc<crate::settings::SettingsService>>,
    /// The work folder of a work given to a system, by the work's id, until its start finds the system's place.
    staged: Mutex<HashMap<String, String>>,
    /// The work folder of each work given to a system from here, by the work's id: «добавить туда».
    in_folder: Mutex<HashMap<String, String>>,
    /// A station as the hub (stage 9): the Турбо's folders, from the home floor.
    remote_folders: OnceLock<RemoteFolders>,
    reports: OnceLock<Reporter>,
    known: Mutex<HashMap<String, Known>>,
    /// The targets of works the layer started → their id in the system, once it answered.
    ids: Mutex<HashMap<String, String>>,
    /// Events of kinds the contract has no use for yet.
    unknown: AtomicU64,
}

/// A system's state as the layer's record keeps it.
fn work_state(state: SystemState) -> WorkState {
    match state {
        SystemState::Queued => WorkState::Queued,
        SystemState::Working | SystemState::WaitsPerson | SystemState::WaitsLimit => WorkState::Working,
        SystemState::Ready => WorkState::Ready,
        SystemState::Unfinished => WorkState::Unfinished,
        SystemState::Failed => WorkState::Failed,
        SystemState::Stopped => WorkState::Stopped,
        SystemState::Interrupted => WorkState::Interrupted,
    }
}

/// A future run beside the caller: the executor's verbs are not async, the systems' are.
fn beside(future: impl std::future::Future<Output = ()> + Send + 'static) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(future);
    }
}

impl SystemExecutor {
    /// The executor joins the layer and hears every system of the registry; with `poll`, the systems that list their
    /// works are asked every minute for those begun by hand (none in tests: they would list this computer's own).
    pub fn new(layer: &Arc<super::TalkLayer>, systems: &Arc<Systems>, poll: bool) -> Arc<SystemExecutor> {
        let executor = Arc::new_cyclic(|me| SystemExecutor {
            systems: systems.clone(),
            me: me.clone(),
            layer: Arc::downgrade(layer),
            questions: Mutex::new(HashMap::new()),
            settings: OnceLock::new(),
            staged: Mutex::new(HashMap::new()),
            in_folder: Mutex::new(HashMap::new()),
            remote_folders: OnceLock::new(),
            reports: OnceLock::new(),
            known: Mutex::new(HashMap::new()),
            ids: Mutex::new(HashMap::new()),
            unknown: AtomicU64::new(0),
        });
        layer.add_executor(executor.clone());
        layer.set_systems(&executor);
        let me = Arc::downgrade(&executor);
        systems.listen(Arc::new(move |system: &str, event: SystemEvent| {
            if let Some(executor) = me.upgrade() {
                executor.heard(system, event);
            }
        }));
        // Works begun by hand in the systems that list them: found on a slow poll (and when the system reports them
        // by itself, at once).
        let me = Arc::downgrade(&executor);
        beside(async move {
            if !poll {
                return;
            }
            // The first look a minute after the start, not at it.
            let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + POLL_EVERY, POLL_EVERY);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let Some(executor) = me.upgrade() else { break };
                executor.poll().await;
            }
        });
        executor
    }

    /// The works the systems list now: one begun outside and going is new to the layer; a known one that the system
    /// shows ended has ended.
    pub async fn poll(&self) {
        for system in self.systems.all() {
            let id = system.id();
            if id == DUOWAVE || !system.can().list || !system.ready().await.ready {
                continue;
            }
            for seen in system.list().await {
                let target = format!("{id}/{}", seen.id);
                let known = locked(&self.known).get(&target).map(|known| known.state);
                match known {
                    None if seen.outside && seen.state.going() => self.heard(&id, SystemEvent::Began { seen }),
                    Some(state) if state.going() && !seen.state.going() => {
                        self.heard(&id, SystemEvent::State { work: seen.id, state: seen.state, line: seen.line })
                    }
                    _ => {}
                }
            }
        }
    }

    /// How many events of kinds not known came so far.
    pub fn unknown(&self) -> u64 {
        self.unknown.load(Ordering::Relaxed)
    }

    /// Where «who gets a work» is kept.
    pub fn set_settings(&self, settings: Arc<crate::settings::SettingsService>) {
        let _ = self.settings.set(settings);
    }

    /// Who gets the works now: the home's default and the folders' own.
    pub fn setup(&self) -> crate::router::who::WorkSystemsSetup {
        self.settings
            .get()
            .and_then(|settings| settings.get(&crate::router::who::WORK_SYSTEMS, None))
            .unwrap_or_default()
    }

    /// A system said by its name, title or id that can begin works: «попроси bb», «а это дай T3».
    pub fn named(&self, said: &str) -> Option<String> {
        let said = said.trim().to_lowercase();
        self.systems.all().into_iter().find_map(|system| {
            let about = system.about();
            let fits = about.id.to_lowercase() == said
                || about.title.to_lowercase() == said
                || about.names.iter().any(|name| name.to_lowercase() == said);
            (fits && system.can().start && about.id != DUOWAVE).then_some(about.id)
        })
    }

    /// A system named anywhere in a phrase, as a whole word or words: «в папке сайт дай т3 код …».
    pub fn named_in(&self, phrase: &str) -> Option<String> {
        let phrase = format!(" {} ", phrase.to_lowercase());
        self.systems.all().into_iter().find_map(|system| {
            let about = system.about();
            let names = std::iter::once(about.title.clone()).chain(about.names.iter().cloned());
            let said = names
                .map(|name| name.to_lowercase())
                .filter(|name| name.chars().count() > 1)
                .any(|name| phrase.contains(&format!(" {name} ")));
            (said && system.can().start && about.id != DUOWAVE).then_some(about.id)
        })
    }

    /// The system for a work in this folder: the one named, the folder's own, the home's default, ours.
    pub fn who_gets(&self, named: Option<&str>, folder: &str) -> String {
        let setup = self.setup();
        let who = crate::router::who::who_gets(&setup, named, Some(folder)).to_string();
        // A system no longer there gives the work back to ours.
        if who != DUOWAVE && self.systems.get(&who).is_none() {
            return DUOWAVE.to_string();
        }
        who
    }

    /// The work folder a work given to a system begins in.
    pub fn stage(&self, work: &str, folder: &str) {
        locked(&self.staged).insert(work.to_string(), folder.to_string());
        locked(&self.in_folder).insert(work.to_string(), folder.to_string());
    }

    /// The registry this executor leads the works of.
    pub fn registry(&self) -> &Arc<Systems> {
        &self.systems
    }

    /// The Турбо's folders, for a hub with no folders of its own (stage 9).
    pub fn set_remote_folders(&self, folders: RemoteFolders) {
        let _ = self.remote_folders.set(folders);
    }

    pub fn remote_folders(&self) -> Vec<RemoteFolder> {
        self.remote_folders.get().map(|folders| folders()).unwrap_or_default()
    }

    /// The work folder a work given to a system from here began in.
    pub fn folder_of(&self, work: &str) -> Option<String> {
        locked(&self.in_folder).get(work).cloned()
    }

    /// The systems a work can be handed to now, but ours: they begin works and are there, by id and title.
    pub fn takers(&self) -> Vec<(String, String)> {
        self.systems
            .all()
            .into_iter()
            .filter(|system| system.id() != DUOWAVE && system.can().start && system.present())
            .map(|system| (system.id(), system.about().title))
            .collect()
    }

    /// A system's title by its id; the id when it is gone.
    pub fn title_of(&self, id: &str) -> String {
        self.systems.get(id).map(|system| system.about().title).unwrap_or_else(|| id.to_string())
    }

    /// The system takes words into a work: now or after its turn.
    pub fn takes_words(&self, system: &str) -> bool {
        self.systems.get(system).is_some_and(|system| {
            let can = system.can();
            can.tell_now || can.tell_after
        })
    }

    /// The system and the work's id there, by its target in the layer; a target of the layer's own before the system
    /// answered has none yet.
    fn resolve(&self, target: &str) -> Option<(Arc<dyn WorkSystem>, String)> {
        let (system, id) = target.split_once('/')?;
        let id = match id.strip_prefix('~') {
            Some(_) => locked(&self.ids).get(target).cloned()?,
            None => id.to_string(),
        };
        Some((self.systems.get(system)?, id))
    }

    /// What is known of a work by the layer's target of it (its own or the system's).
    fn known(&self, work: &Work) -> Option<Known> {
        let target = work.target.as_deref()?;
        let known = locked(&self.known);
        if let Some(known) = known.get(target) {
            return Some(known.clone());
        }
        let real = locked(&self.ids).get(target).map(|id| format!("{}/{id}", system_of(target).unwrap_or_default()))?;
        known.get(&real).cloned()
    }

    fn keep(&self, target: &str, change: impl FnOnce(&mut Known)) {
        let mut known = locked(&self.known);
        let entry = known.entry(target.to_string()).or_insert(Known {
            state: SystemState::Working,
            line: None,
            outcome: None,
            usd_micro: None,
        });
        change(entry);
    }

    fn report(&self, report: Report) {
        if let Some(reports) = self.reports.get() {
            reports(SYSTEM_EXECUTOR, report);
        }
    }

    fn heard(&self, system: &str, event: SystemEvent) {
        let target = |work: &str| format!("{system}/{work}");
        match event {
            SystemEvent::Began { seen } => {
                let at = target(&seen.id);
                self.keep(&at, |known| {
                    known.state = seen.state;
                    known.line = seen.line.clone();
                });
                // A work begun here is reported by its start; one begun outside is new to the layer.
                if seen.outside {
                    self.report(Report::Working(Began {
                        target: at,
                        work: None,
                        face: None,
                        title: seen.title,
                        brief: String::new(),
                        origin: Origin::computer(Activator::Text),
                    }));
                }
            }
            SystemEvent::State { work, state, line } => {
                let at = target(&work);
                self.keep(&at, |known| {
                    known.state = state;
                    if line.is_some() {
                        known.line = line.clone();
                    }
                });
                if !state.going() {
                    self.report(Report::Ended { target: at, state: work_state(state), line, title: None });
                }
            }
            SystemEvent::Step { work, line } => self.keep(&target(&work), |known| known.line = Some(line)),
            SystemEvent::Spent { work, usd_micro, .. } => {
                self.keep(&target(&work), |known| known.usd_micro = Some(usd_micro))
            }
            SystemEvent::Ended { work, state, outcome } => {
                let at = target(&work);
                // A system's summary is a sentence of the line «…: готово. {summary} Подробности в пульте.»: it ends with
                // its stop (live check 27.11: «…за 2900 Подробности в пульте.»).
                let line = outcome.summary.as_deref().map(sentence).filter(|line| !line.is_empty());
                self.keep(&at, |known| {
                    known.state = state;
                    known.outcome = Some(outcome);
                });
                self.report(Report::Ended { target: at, state: work_state(state), line, title: None });
            }
            SystemEvent::Asks { work, question } => {
                let at = target(&work);
                self.keep(&at, |known| known.state = SystemState::WaitsPerson);
                locked(&self.questions).insert(at.clone(), question.clone());
                let title = self.systems.get(system).map(|system| system.about().title).unwrap_or_default();
                if let Some(layer) = self.layer.upgrade() {
                    layer.system_asks(&at, &title, &question);
                }
            }
            SystemEvent::Limit { work, .. } => self.keep(&target(&work), |known| known.state = SystemState::WaitsLimit),
            SystemEvent::Left { .. } => {}
            SystemEvent::Unknown { .. } => {
                self.unknown.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

impl SystemExecutor {
    /// The model a system begins a work on: the home's set, else «Обычно», at the run's tier, in the system's words;
    /// none when the set does not reach the system (stage 5 of docs/plans/work-systems.md).
    fn model_for(&self, system: &str) -> Option<String> {
        use crate::router::sets;
        let kept = self.settings.get().and_then(|settings| settings.get(&sets::WORK_SETS, None)).unwrap_or_default();
        let id = sets::chosen(None, None, &kept).unwrap_or(sets::USUAL).to_string();
        let set = sets::find(&kept, &id).or_else(|| sets::find(&kept, sets::USUAL))?;
        sets::model_of(&set, system.split('@').next().unwrap_or(system), sets::Tier::Mid)
    }
}

impl Executor for SystemExecutor {
    fn name(&self) -> &'static str {
        SYSTEM_EXECUTOR
    }

    fn listen(&self, reports: Reporter) {
        let _ = self.reports.set(reports);
    }

    /// Asks the work's system to begin it: the record's system, its brief and title.
    fn start(&self, work: &Work) -> Result<Started, String> {
        let id = work.system.clone().ok_or_else(|| "no system for the work".to_string())?;
        let system = self.systems.get(&id).ok_or_else(|| format!("no system «{id}»"))?;
        let provisional = format!("{id}/~{}", work.id);
        let mut start = Start {
            task: work.brief.clone(),
            title: Some(work.title.clone()).filter(|title| !title.trim().is_empty()),
            who: work.origin.who.clone(),
            model: self.model_for(&id),
            ..Start::default()
        };
        let folder = locked(&self.staged).remove(&work.id);
        let (me, at, work_id, origin) = (self.me.clone(), provisional.clone(), work.id.clone(), work.origin.clone());
        let systems = self.systems.clone();
        let remote = self.remote_folders();
        beside(async move {
            // The system's place of the work folder: its project at the folder's path, or the path itself for a
            // system that names its projects by a path.
            if let Some(folder) = folder {
                match place_of(&systems, &system, &folder, &remote).await {
                    Ok(place) => start.place = Some(place),
                    Err(why) => {
                        if let Some(me) = me.upgrade() {
                            me.report(Report::Ended {
                                target: at,
                                state: WorkState::Failed,
                                line: Some(why),
                                title: None,
                            });
                        }
                        return;
                    }
                }
            }
            let started = system.start(start).await;
            let Some(me) = me.upgrade() else { return };
            match started {
                Ok(seen) => {
                    let real = format!("{id}/{}", seen.id);
                    locked(&me.ids).insert(at, seen.id.clone());
                    me.keep(&real, |known| {
                        known.state = seen.state;
                        known.line = seen.line.clone();
                    });
                    me.report(Report::Working(Began {
                        target: real,
                        work: Some(work_id),
                        face: None,
                        title: seen.title,
                        brief: String::new(),
                        origin,
                    }));
                }
                Err(refusal) => me.report(Report::Ended {
                    target: at,
                    state: WorkState::Failed,
                    line: Some(refusal.why),
                    title: None,
                }),
            }
        });
        Ok(Started { target: provisional, title: work.title.clone(), note: None })
    }

    /// Words into the work: into the going turn when the system takes them so, else after it.
    fn add(&self, work: &Work, text: &str) -> bool {
        let Some((system, id)) = work.target.as_deref().and_then(|target| self.resolve(target)) else { return false };
        let can = system.can();
        let when = match (can.tell_now, can.tell_after) {
            (true, _) => When::Now,
            (false, true) => When::After,
            (false, false) => return false,
        };
        let text = text.to_string();
        beside(async move {
            if let Err(refusal) = system.tell(&id, &text, when).await {
                tracing::info!(target: "talk", "work systems: words not taken by {}: {}", system.id(), refusal.why);
            }
        });
        true
    }

    fn thing(&self, _name: &str) -> Option<Started> {
        None
    }

    fn open<'a>(&'a self, _target: &'a str, _copy: bool) -> BoxFuture<'a, Result<Started, Closed>> {
        async { Err(Closed::Failed("a work system's work has no thing to change".into())) }.boxed()
    }

    fn status(&self, work: &Work) -> Option<Doing> {
        self.known(work).filter(|known| known.state.going()).map(|_| Doing::Writing)
    }

    fn details(&self, work: &Work) -> Option<String> {
        self.known(work).and_then(|known| known.line)
    }

    fn facts<'a>(&'a self, _work: &'a Work) -> BoxFuture<'a, Facts> {
        async { Facts::default() }.boxed()
    }

    /// «Открой» (End::Fitting): the work in the system itself, by its `open` command where the system lives; a
    /// system that cannot is said so. Nothing else is put anywhere: a system's work has no draft.
    fn put<'a>(&'a self, work: &'a Work, end: End, _by: ByVoice) -> BoxFuture<'a, Result<Put, String>> {
        async move {
            let Some((system, id)) = work.target.as_deref().and_then(|target| self.resolve(target)) else {
                return Ok(Put::Shown { open: false });
            };
            let title = system.about().title;
            if end != End::Fitting {
                return Ok(Put::Shown { open: false });
            }
            if !system.can().open {
                return Ok(Put::NoOpen { system: title });
            }
            system.show(&id).await.map_err(|refusal| refusal.why)?;
            Ok(Put::InSystem { system: title })
        }
        .boxed()
    }

    fn stop(&self, work: &Work) -> bool {
        let Some((system, id)) = work.target.as_deref().and_then(|target| self.resolve(target)) else { return false };
        if !system.can().stop {
            return false;
        }
        beside(async move {
            if let Err(refusal) = system.stop(&id).await {
                tracing::info!(target: "talk", "work systems: {} did not stop {id}: {}", system.id(), refusal.why);
            }
        });
        true
    }

    fn stop_waiting<'a>(&'a self, work: &'a Work) -> BoxFuture<'a, Result<bool, String>> {
        let resolved = work.target.as_deref().and_then(|target| self.resolve(target));
        Box::pin(async move {
            let Some((system, id)) = resolved else { return Ok(false) };
            if !system.can().stop {
                return Ok(false);
            }
            system.stop(&id).await.map(|()| true).map_err(|refusal| {
                tracing::info!(target: "talk", "work systems: {} did not stop {id}: {}", system.id(), refusal.why);
                refusal.why
            })
        })
    }

    fn running(&self, work: &Work) -> Option<bool> {
        self.known(work).map(|known| known.state.going())
    }

    /// The answer to the question the work's system waits on: a dangerous one only by a tap (D3).
    fn answer_question(&self, work: &Work, yes: bool, by_voice: bool) -> Result<(), NotAnswered> {
        let target = work.target.as_deref().ok_or(NotAnswered::Gone)?;
        let question = locked(&self.questions).get(target).cloned().ok_or(NotAnswered::Gone)?;
        if question.dangerous && yes && by_voice {
            return Err(NotAnswered::Dangerous);
        }
        let (system, id) = self.resolve(target).ok_or(NotAnswered::Gone)?;
        locked(&self.questions).remove(target);
        let reply = if yes { Reply::Yes } else { Reply::No };
        beside(async move {
            if let Err(refusal) = system.answer(&id, &question.id, reply).await {
                tracing::info!(target: "talk", "work systems: {} took no answer for {id}: {}", system.id(), refusal.why);
            }
        });
        Ok(())
    }

    fn who(&self, work: &Work) -> Option<String> {
        let system = work.target.as_deref().and_then(system_of).or(work.system.as_deref())?;
        self.systems.get(system).map(|system| system.about().title)
    }

    /// What came of it, as the system said.
    fn told(&self, work: &Work) -> Option<Told> {
        let known = self.known(work)?;
        let outcome = known.outcome.unwrap_or_default();
        let failed = known.state == SystemState::Failed;
        // The summary is a sentence of «…: готово. {summary} Подробности в пульте.».
        let said =
            outcome.summary.clone().or(known.line.clone()).map(|said| sentence(&said)).filter(|said| !said.is_empty());
        Some(Told {
            summary: said.clone().filter(|_| !failed),
            more: outcome.more,
            data: None,
            error: if failed { said } else { None },
        })
    }
}

/// Words as a sentence: trimmed, and ended with a stop when they have none.
fn sentence(words: &str) -> String {
    let words = words.trim();
    match words.chars().last() {
        None => String::new(),
        Some('.' | '!' | '?' | '…' | '»' | '"' | ')') => words.to_string(),
        Some(_) => format!("{words}."),
    }
}

/// A path as compared: no trailing separator.
fn same_path(a: &str, b: &str) -> bool {
    a.trim_end_matches(['/', '\\']) == b.trim_end_matches(['/', '\\'])
}

/// The place in `system` of our work folder `folder`: the system's project at its path, or the path itself when the
/// system lists no places; why not, in words.
async fn place_of(
    systems: &Systems,
    system: &Arc<dyn WorkSystem>,
    folder: &str,
    remote: &[RemoteFolder],
) -> Result<String, String> {
    let mut ours = match systems.get(DUOWAVE) {
        Some(ours) => ours.places().await,
        None => Vec::new(),
    };
    // A Турбо's folder (stage 9): its path as that computer has it.
    ours.extend(remote.iter().filter(|kept| kept.folder == folder).map(|kept| super::system::Place {
        id: kept.folder.clone(),
        title: kept.name.clone(),
        path: Some(kept.path.clone()),
    }));
    let found = ours.iter().find(|place| place.id == folder);
    let path = found.and_then(|place| place.path.clone()).ok_or_else(|| format!("no work folder «{folder}»"))?;
    if !system.can().places {
        return Ok(path);
    }
    let theirs = system.places().await;
    match theirs.iter().find(|place| place.path.as_deref().is_some_and(|their| same_path(their, &path))) {
        Some(place) => Ok(place.id.clone()),
        None => {
            let title = found.map(|place| place.title.clone()).unwrap_or_default();
            let words = &pack::of(crate::memory::text::Lang::Ru).work.system;
            Err(fill(words.no_place, &[("system", &system.about().title), ("folder", &title)]))
        }
    }
}

impl super::TalkLayer {
    /// A work in a folder given to a work system (owner decision 8): of the kind `system`, its system leads it by its
    /// own rules. The words said. `new` — a new one even while another of the system goes on in the folder.
    pub(super) fn start_in_system(
        &self,
        origin: Origin,
        system: &str,
        folder: &str,
        folder_name: &str,
        task: &str,
        new: bool,
    ) -> Result<String, super::work_tools::Refusal> {
        let executor = self.systems().ok_or_else(|| super::work_tools::Refusal::Failed("no work systems".into()))?;
        // A work of this system goes on in the folder: «Добавить туда или начать новую?» — the answer «новую» comes back
        // here through the question (ready.rs) with the wish as it was.
        let going = self
            .works
            .all(None, None)
            .into_iter()
            .filter(|work| work.going() && work.system.as_deref() == Some(system))
            .filter(|work| executor.folder_of(&work.id).as_deref() == Some(folder))
            .max_by_key(|work| work.updated_at);
        if let Some(work) = going.filter(|_| !new && executor.takes_words(system)) {
            let words = &pack::of(self.lang(&origin.device)).work.system;
            let question = fill(words.add_or_new, &[("title", &work.title)]);
            let wish = super::asked::Pending {
                text: task.to_string(),
                question: Some(question.clone()),
                in_system: Some(super::asked::InSystem {
                    system: system.to_string(),
                    folder: folder.to_string(),
                    folder_name: folder_name.to_string(),
                }),
                ..super::asked::Pending::default()
            };
            self.asked.ask_with(&origin.device, &work.id, Kind::Similar, wish);
            return Ok(question);
        }
        let title = super::folder::title_of(task);
        let work = self.works.create(super::works::NewWork {
            kind: super::works::SYSTEM,
            face: None,
            executor: SYSTEM_EXECUTOR.into(),
            target: None,
            title,
            brief: task.to_string(),
            origin,
            again: false,
        });
        let work = self.works.update(&work.id, |work| work.system = Some(system.to_string())).unwrap_or(work);
        executor.stage(&work.id, folder);
        let started = Executor::start(executor.as_ref(), &work).map_err(super::work_tools::Refusal::Failed)?;
        let work = self.works.update(&work.id, |work| work.target = Some(started.target.clone())).unwrap_or(work);
        self.told(&work, "started");
        let who = executor.who(&work).unwrap_or_else(|| system.to_string());
        let words = &pack::of(self.lang(work.device())).work.system;
        Ok(fill(words.started, &[("folder", folder_name), ("system", &who), ("title", &work.title)]))
    }

    /// A work system's question holds one of its works: its words in the record, told to the panel (which says it
    /// aloud on the computer); «как там» asks it too.
    pub(super) fn system_asks(&self, target: &str, system: &str, question: &Question) {
        self.ask_yes_no_on(SYSTEM_EXECUTOR, target, system, &question.text);
    }

    /// A yes or no question of whoever leads a work (a work system, an MCP server during a background call): the
    /// work's question, said aloud and in «как там»; the home's admin answers it, and the answer goes to the
    /// executor's `answer_question`.
    pub fn ask_yes_no_on(&self, executor: &str, target: &str, who: &str, what: &str) {
        let Some(work) = self.works.going_on(executor, target) else { return };
        let words = &pack::of(self.lang(work.device())).work.system;
        // A question that is one already (an agent's «Купить за 3000?») is said as it is; a command to allow asks
        // «Разрешить?».
        let pattern = if what.trim_end().ends_with('?') { words.asks_q } else { words.permission_q };
        let text = fill(pattern, &[("system", who), ("what", what.trim_end())]);
        let changed = self.works.update(&work.id, |work| {
            work.question = Some(super::works::Question::new(text.clone(), now_ms() + ASK_FOR_MS, Kind::Permission));
        });
        if let Some(work) = changed {
            self.told(&work, "asks");
        }
    }
}

/// The systems' API (docs/plans/work-systems.md, stage 1): for whoever may change the gateway's settings.
///
/// - `GET /api/v1/work-systems` → `{systems: [{id, title, names, plug, protocol?, version?, can, ready}]}`.
/// - `GET /api/v1/work-systems/setup` → `{default?, folders: {folder: system}}` (stage 4, who gets a work); `PUT` the
///   same, whole: the home's default and each folder's own system (a folder's page sets its own).
/// - `PUT /api/v1/work-systems/{id}/assign` `{mode: all | places | named}` → the setup.
pub fn routes<S>() -> axum::Router<S>
where
    S: crate::platform::http::RouteState
        + crate::platform::http::Provides<Arc<Systems>>
        + crate::platform::http::Provides<Arc<crate::settings::SettingsService>>,
{
    axum::Router::new()
        .route("/api/v1/work-systems", axum::routing::get(list))
        .route("/api/v1/work-systems/setup", axum::routing::get(setup).put(set_setup))
        .route("/api/v1/work-systems/{id}/assign", axum::routing::put(assign))
}

/// Who gets the works now.
async fn setup(
    crate::platform::http::Uses(settings): crate::platform::http::Uses<Arc<crate::settings::SettingsService>>,
    caller: crate::platform::http::Caller,
) -> crate::platform::http::ApiResult<crate::router::who::WorkSystemsSetup> {
    caller.permitted(crate::core::identity::permissions::SETTINGS_EDIT)?;
    Ok(crate::platform::http::ok(settings.get(&crate::router::who::WORK_SYSTEMS, None).unwrap_or_default()))
}

/// Who gets the works, set whole: each system named must be one that begins works; ours is said by no entry.
async fn set_setup(
    crate::platform::http::Uses(systems): crate::platform::http::Uses<Arc<Systems>>,
    crate::platform::http::Uses(settings): crate::platform::http::Uses<Arc<crate::settings::SettingsService>>,
    caller: crate::platform::http::Caller,
    crate::platform::http::Body(asked): crate::platform::http::Body<crate::router::who::WorkSystemsSetup>,
) -> crate::platform::http::ApiResult<crate::router::who::WorkSystemsSetup> {
    use crate::platform::http::ApiError;
    caller.permitted(crate::core::identity::permissions::SETTINGS_EDIT)?;
    let begins = |id: &str| id == DUOWAVE || systems.get(id).is_some_and(|system| system.can().start);
    let ours: Vec<String> = match systems.get(DUOWAVE) {
        Some(ours) => ours.places().await.into_iter().map(|place| place.id).collect(),
        None => Vec::new(),
    };
    let mut setup = crate::router::who::WorkSystemsSetup::default();
    if let Some(default) = asked.default.as_deref().map(str::trim).filter(|id| !id.is_empty()) {
        if !begins(default) {
            return Err(ApiError::field("default", crate::tr!("Такой системы нет.")));
        }
        setup.default = Some(default.to_string()).filter(|id| id != DUOWAVE);
    }
    for (folder, system) in asked.folders {
        if !ours.contains(&folder) {
            return Err(ApiError::field("folders", crate::tr!("Такой рабочей папки нет.")));
        }
        if !begins(&system) {
            return Err(ApiError::field("folders", crate::tr!("Такой системы нет.")));
        }
        // Ours is the folder's own only when the home's default is another system.
        if system != DUOWAVE || setup.default.is_some() {
            setup.folders.insert(folder, system);
        }
    }
    settings.set(&crate::router::who::WORK_SYSTEMS, &setup, caller.actor())?;
    Ok(crate::platform::http::ok(setup))
}

/// The answer to «Что поручать …?» (D8): `{mode: all | places | named}`. «Работы в его проектах» makes the system the
/// own one of each work folder at the path of one of its places.
async fn assign(
    crate::platform::http::Uses(systems): crate::platform::http::Uses<Arc<Systems>>,
    crate::platform::http::Uses(settings): crate::platform::http::Uses<Arc<crate::settings::SettingsService>>,
    caller: crate::platform::http::Caller,
    axum::extract::Path(id): axum::extract::Path<String>,
    crate::platform::http::Body(body): crate::platform::http::Body<serde_json::Value>,
) -> crate::platform::http::ApiResult<crate::router::who::WorkSystemsSetup> {
    use crate::platform::http::ApiError;
    caller.permitted(crate::core::identity::permissions::SETTINGS_EDIT)?;
    let system = systems.get(&id).ok_or_else(|| ApiError::not_found(crate::tr!("Такой системы нет.")))?;
    let mode = body.get("mode").and_then(serde_json::Value::as_str).unwrap_or_default();
    let mut folders = Vec::new();
    if mode == "places" {
        let ours = match systems.get(DUOWAVE) {
            Some(ours) => ours.places().await,
            None => Vec::new(),
        };
        let theirs = system.places().await;
        for folder in ours {
            let at = folder.path.as_deref().unwrap_or_default();
            if theirs.iter().any(|place| place.path.as_deref().is_some_and(|path| same_path(path, at))) {
                folders.push(folder.id);
            }
        }
    }
    let asked = crate::router::who::Assign::parse(mode, folders)
        .ok_or_else(|| ApiError::field("mode", crate::tr!("Что поручать: all, places или named.")))?;
    let mut setup = settings.get(&crate::router::who::WORK_SYSTEMS, None).unwrap_or_default();
    crate::router::who::assign(&mut setup, &id, &asked);
    settings.set(&crate::router::who::WORK_SYSTEMS, &setup, caller.actor())?;
    Ok(crate::platform::http::ok(setup))
}

#[derive(Debug, Serialize)]
struct SystemList {
    systems: Vec<SystemView>,
}

async fn list(
    crate::platform::http::Uses(systems): crate::platform::http::Uses<Arc<Systems>>,
    caller: crate::platform::http::Caller,
) -> crate::platform::http::ApiResult<SystemList> {
    caller.permitted(crate::core::identity::permissions::SETTINGS_EDIT)?;
    Ok(crate::platform::http::ok(SystemList { systems: systems.views().await }))
}

#[cfg(test)]
mod sentence_tests {
    #[test]
    fn a_summary_ends_as_a_sentence() {
        assert_eq!(super::sentence(" Билет куплен за 2900 "), "Билет куплен за 2900.");
        assert_eq!(super::sentence("Готово!"), "Готово!");
        assert_eq!(super::sentence("  "), "");
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::core::events::EventBus;
    use crate::talk::system::stand_in::StandIn;
    use crate::talk::works::{NewWork, SYSTEM};
    use crate::talk::TalkLayer;

    async fn until(what: &str, check: impl Fn() -> bool) {
        let end = std::time::Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(std::time::Instant::now() < end, "timed out waiting: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn env(system: StandIn) -> (Arc<TalkLayer>, Arc<StandIn>) {
        let layer = TalkLayer::new(None, EventBus::new());
        let systems = Systems::new();
        let system = Arc::new(system);
        systems.add(system.clone());
        SystemExecutor::new(&layer, &systems, false);
        (layer, system)
    }

    /// The layer starts a work in a system and leads it by the contract only: its own target until the system
    /// answers, then the system's id; words go in, the stop is the system's, and its end comes back as the work's.
    #[tokio::test]
    async fn a_work_of_a_system_goes_through_the_layer_by_the_contract() {
        let (layer, system) = env(StandIn::full("bb"));
        let work = layer.works().create(NewWork {
            kind: SYSTEM,
            face: None,
            executor: SYSTEM_EXECUTOR.into(),
            target: None,
            title: "Тёмная тема".into(),
            brief: "сделай тёмную тему".into(),
            origin: Origin::computer(Activator::Text),
            again: false,
        });
        let work = layer.works().update(&work.id, |work| work.system = Some("bb".into())).unwrap();
        let executor = layer.executor(SYSTEM_EXECUTOR).unwrap();
        let started = executor.start(&work).unwrap();
        assert_eq!(started.target, format!("bb/~{}", work.id));
        layer.works().update(&work.id, |work| work.target = Some(started.target.clone()));
        let id = work.id.clone();
        let target_of = |layer: &TalkLayer| layer.works().get(&id).and_then(|work| work.target);
        until("the system's id", || target_of(&layer).as_deref() == Some("bb/w1")).await;
        let work = layer.works().get(&id).unwrap();
        assert!(!work.outside, "begun here");
        assert!(executor.add(&work, "и ещё кнопку"));
        until("the words taken", || system.told("w1") == vec!["и ещё кнопку".to_string()]).await;
        assert!(executor.stop(&work));
        until("stopped", || layer.works().get(&id).is_some_and(|work| work.state == WorkState::Stopped)).await;
    }

    /// A work begun in the system by hand comes to the layer as begun outside, with its system; its end is said with
    /// the system's summary.
    #[tokio::test]
    async fn a_work_begun_in_the_system_by_hand_is_a_work_begun_outside() {
        let (layer, system) = env(StandIn::full("claude-code"));
        let id = system.begin_outside("Опечатка в README");
        let target = format!("claude-code/{id}");
        until("the outside work", || layer.works().of_target(SYSTEM_EXECUTOR, &target).is_some()).await;
        let work = layer.works().of_target(SYSTEM_EXECUTOR, &target).unwrap();
        assert_eq!((work.kind.as_str(), work.system.as_deref(), work.outside), (SYSTEM, Some("claude-code"), true));
        system.end(&id, SystemState::Ready, "Опечатка поправлена.");
        let work_id = work.id.clone();
        until("done", || layer.works().get(&work_id).is_some_and(|work| work.state == WorkState::Done)).await;
        let line = layer.works().get(&work_id).unwrap().result.unwrap().line;
        assert!(line.contains("Опечатка поправлена."), "{line}");
    }

    /// A system that cannot take words or stop is not asked to: the flags hold.
    #[tokio::test]
    async fn a_system_without_tell_or_stop_takes_neither() {
        let (layer, system) = env(StandIn::new("t3", Can { list: true, ..Can::default() }));
        let id = system.begin_outside("Новая страница");
        let target = format!("t3/{id}");
        until("the outside work", || layer.works().of_target(SYSTEM_EXECUTOR, &target).is_some()).await;
        let work = layer.works().of_target(SYSTEM_EXECUTOR, &target).unwrap();
        let executor = layer.executor(SYSTEM_EXECUTOR).unwrap();
        assert!(!executor.add(&work, "и ещё"));
        assert!(!executor.stop(&work));
        assert_eq!(executor.running(&work), Some(true));
    }
}

#[cfg(test)]
mod stage_3 {
    use std::time::Duration;

    use super::*;
    use crate::core::events::EventBus;
    use crate::core::identity::Role;
    use crate::talk::ready::Answer;
    use crate::talk::system::stand_in::StandIn;
    use crate::talk::TalkLayer;

    async fn until(what: &str, check: impl Fn() -> bool) {
        let end = std::time::Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(std::time::Instant::now() < end, "timed out waiting: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn env(system: StandIn) -> (Arc<TalkLayer>, Arc<StandIn>) {
        let layer = TalkLayer::new(None, EventBus::new());
        let systems = Systems::new();
        let system = Arc::new(system);
        systems.add(system.clone());
        SystemExecutor::new(&layer, &systems, false);
        (layer, system)
    }

    fn admin() -> Origin {
        let mut origin = Origin::computer(Activator::Text);
        origin.role = Some(Role::Admin);
        origin
    }

    /// A work begun outside: its system's question of rights is the work's question, and the admin's «да» goes back
    /// as the system's answer; a dangerous one is not approved by voice (D3).
    #[tokio::test]
    async fn a_systems_question_is_the_works_and_the_answer_goes_back() {
        let (layer, system) = env(StandIn::full("claude-code"));
        let id = system.begin_outside("Тесты сайта");
        let target = format!("claude-code/{id}");
        until("the work", || layer.works().of_target(SYSTEM_EXECUTOR, &target).is_some()).await;
        let work = layer.works().of_target(SYSTEM_EXECUTOR, &target).unwrap();
        let asked = system.ask(&id, "Bash: npm test", false);
        let asks = || layer.works().get(&work.id).and_then(|work| work.question).map(|question| question.text);
        until("the question", || asks().is_some()).await;
        assert_eq!(asks().unwrap(), "Stand-in claude-code спрашивает: Bash: npm test. Разрешить?");
        let mut guest = Origin::computer(Activator::Text);
        guest.role = Some(Role::Guest);
        let said = layer.answer(guest, &work.id, Kind::Permission, false, Answer::Yes).await;
        assert_eq!(said, "Разрешать может только администратор дома.");
        // The panel: the work waits for the person, and its button answers it (the hand check of 27.11 found both
        // missing).
        assert!(layer.waiting().iter().any(|waiting| waiting.id == work.id), "«Ждут вас»");
        let said = layer.answer_work(admin(), &work.id, Answer::Yes).await;
        assert_eq!(said.as_deref(), Some("Разрешаю."));
        until("the reply", || system.replies() == vec![(asked.clone(), Reply::Yes)]).await;
        assert!(layer.works().get(&work.id).unwrap().question.is_none(), "answered");

        let dangerous = system.ask(&id, "Bash: git push origin HEAD", true);
        until("the second question", || asks().is_some()).await;
        let mut by_voice = admin();
        by_voice.via = crate::talk::Via::Voice;
        let said = layer.answer(by_voice, &work.id, Kind::Permission, false, Answer::Yes).await;
        assert_eq!(said, "Это разрешают только кнопкой в пульте.");
        let said = layer.answer(admin(), &work.id, Kind::Permission, false, Answer::No).await;
        assert_eq!(said, "Не разрешаю.");
        until("the refusal", || system.replies().contains(&(dangerous.clone(), Reply::No))).await;
    }

    /// A work begun outside is quiet: its end is kept, not said; «следи за этой» makes the next one said aloud.
    #[tokio::test]
    async fn a_work_begun_outside_is_quiet_unless_watched() {
        let (layer, system) = env(StandIn::full("claude-code"));
        let quiet = system.begin_outside("Опечатка");
        let target = format!("claude-code/{quiet}");
        until("the work", || layer.works().of_target(SYSTEM_EXECUTOR, &target).is_some()).await;
        let work = layer.works().of_target(SYSTEM_EXECUTOR, &target).unwrap();
        system.end(&quiet, SystemState::Ready, "Поправил.");
        until("done", || layer.works().get(&work.id).is_some_and(|work| work.state == WorkState::Done)).await;
        let done = layer.works().get(&work.id).unwrap();
        assert!(done.question.is_none(), "nothing asked of a quiet work");

        let loud = system.begin_outside("Новая страница");
        let target = format!("claude-code/{loud}");
        until("the second work", || layer.works().of_target(SYSTEM_EXECUTOR, &target).is_some()).await;
        let work = layer.works().of_target(SYSTEM_EXECUTOR, &target).unwrap();
        let label = format!("{}{}", pack::of(crate::memory::text::Lang::Ru).work.label, work.id);
        let said = layer.act(&admin(), "watch", Some(&label)).await.unwrap();
        assert_eq!(said, "Слежу за «Новая страница». Скажу, когда закончится.");
        assert!(layer.works().get(&work.id).unwrap().watched);
        system.end(&loud, SystemState::Ready, "Страница готова.");
        until("done", || layer.works().get(&work.id).is_some_and(|work| work.state == WorkState::Done)).await;
    }

    /// A system that cannot stop or take words says so in words, and the work goes on.
    #[tokio::test]
    async fn a_system_that_cannot_stop_or_take_words_says_so() {
        let (layer, system) = env(StandIn::new("t3", Can { list: true, ..Can::default() }));
        let id = system.begin_outside("Новая страница");
        let target = format!("t3/{id}");
        until("the work", || layer.works().of_target(SYSTEM_EXECUTOR, &target).is_some()).await;
        let work = layer.works().of_target(SYSTEM_EXECUTOR, &target).unwrap();
        let label = format!("{}{}", pack::of(crate::memory::text::Lang::Ru).work.label, work.id);
        let said = layer.act(&admin(), "stop", Some(&label)).await.unwrap();
        assert_eq!(said, "Остановить отсюда не могу. Откройте Stand-in t3.");
        assert!(layer.works().get(&work.id).unwrap().going(), "it goes on");
        let said = layer.add(admin(), "и ещё кнопку", Some(&label), false).await.unwrap();
        assert!(said.starts_with("Stand-in t3 вдогонку не принимает."), "{said}");
        let said = layer.act(&admin(), "show", Some(&label)).await.unwrap();
        assert_eq!(said, "Stand-in t3 не умеет показывать свои работы отсюда. Откройте его сами.");
    }

    /// «Открой» on a system's work opens it in the system itself (the hand check of 27.11: «Плитка ждёт на столе»).
    #[tokio::test]
    async fn open_shows_a_systems_work_in_the_system() {
        let (layer, system) = env(StandIn::full("bb"));
        let id = system.begin_outside("Новая страница");
        let target = format!("bb/{id}");
        until("the work", || layer.works().of_target(SYSTEM_EXECUTOR, &target).is_some()).await;
        let work = layer.works().of_target(SYSTEM_EXECUTOR, &target).unwrap();
        let label = format!("{}{}", pack::of(crate::memory::text::Lang::Ru).work.label, work.id);
        let said = layer.act(&admin(), "show", Some(&label)).await.unwrap();
        assert_eq!(said, "Открыла в Stand-in bb.");
    }
}
