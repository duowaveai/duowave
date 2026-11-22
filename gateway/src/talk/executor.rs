//! Who does a work (docs/plans/audio-interface.md, «Исполнитель»): the handles every executor has, and the messages
//! it sends the layer. The workshop is the first (talk/workshop.rs); an errands' agent or an app's long task
//! (src/tasks) takes its place beside it without the layer changing. An executor speaks to the layer only by its
//! messages, and a work it starts by itself goes through the same layer a tool's does (the fourth rule of «Задел на
//! несколько дел»).

use std::sync::Arc;

use futures::future::BoxFuture;

use super::origin::Origin;
use super::works::{End, Face, Work, WorkState};

/// What an executor is at, in a few words of the language pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doing {
    Thinking,
    Writing,
    Reading,
    Checking,
    Trying,
    Phrase,
    Docs,
    Journal,
}

/// A thing of the executor's began to be worked on: everything a record needs, for a run the layer did not start
/// (the panel's workshop, its «+»).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Began {
    pub target: String,
    /// The work the run belongs to, when the layer started it.
    pub work: Option<String>,
    pub face: Option<Face>,
    pub title: String,
    pub brief: String,
    pub origin: Origin,
}

/// What an executor tells the layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// It works on a thing.
    Working(Began),
    /// A run on the thing ended: ready, not finished, failed, stopped, interrupted.
    /// `title`: the thing's name as its maker gave it (a draft's manifest), when it has one: the work takes it.
    Ended { target: String, state: WorkState, line: Option<String>, title: Option<String> },
    /// The person decided what becomes of the thing: on the desk, in the drafts.
    Decided { target: String, end: End },
    /// It took in what was added on the way: every note said so far was read.
    Read { target: String },
}

/// Where an executor reports: its name and the message.
pub type Reporter = Arc<dyn Fn(&'static str, Report) + Send + Sync>;

/// What a finished thing asks of the person before it goes on the desk: the gateway's, read from its manifest and its
/// install's plan, never the executor's words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    /// Settings it needs that nobody filled in yet: their titles («ключ Todoist»).
    pub needs: Vec<String>,
    /// Hosts it will reach that it did not before.
    pub hosts: Vec<String>,
    /// Rights it asks for that it did not before, hosts apart.
    pub rights: Vec<String>,
    /// A right among them only the panel approves: money, deletion, outside, locks, camera, access.
    pub dangerous: bool,
    /// It is on the desk already: this is its new version.
    pub installed: bool,
    /// It would install now (its check passes).
    pub ready: bool,
    /// Its last run changed nothing: «ничего не поменялось», not «не доделано».
    pub unchanged: bool,
    /// Its last run failed so: the model's error as the log has it (a key out of money, a key refused). The station
    /// names a plain cause in its «не получилось» (19.11: OpenAI's 429 «no credits» was only «подробности в пульте»).
    pub failure: Option<String>,
}

/// What became of a thing put where the person said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Put {
    /// Open to try on in the panel (`open`), or the tile waits on the desk.
    Shown {
        open: bool,
    },
    /// On the desk; it waits for these settings to be filled in (the panel's window opened when `open`).
    OnDesk {
        needs: Vec<String>,
        open: bool,
    },
    /// Only the panel puts it there: the owner keeps the desk by voice off.
    Approve {
        open: bool,
    },
    /// Rights only the panel approves.
    Dangerous {
        open: bool,
    },
    Parked,
    Deleted,
}

/// Who put a thing on the desk by voice, for the audit: the device it was said to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByVoice {
    pub device: String,
    pub who: Option<String>,
}

/// Why a thing cannot get a new version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Closed {
    /// A built-in app or a catalog's: not changed, one's own copy of it may be — its title.
    BuiltIn(String),
    /// Why not, in words.
    Failed(String),
}

/// What a work began on: the executor's own id of the thing, and its title as people hear it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub target: String,
    pub title: String,
    /// Said before «Делаю»: where the thing's device is when it is not at hand now (docs/plans/turbo-devices.md).
    pub note: Option<String>,
}

/// What a background work came to (docs/plans/long-tools.md, point 3): the model's retelling in a sentence or two,
/// the rest of it for «подробнее», the result as it came, or why it failed. Only the executors without a desk have it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Told {
    pub summary: Option<String>,
    pub more: Option<String>,
    pub data: Option<String>,
    pub error: Option<String>,
}

/// Whether an executor's thing is there, by its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// On the desk (installed).
    OnDesk,
    /// Only its draft is there: taken off the desk, or not put there.
    Draft,
    /// Neither: deleted.
    Gone,
    /// The executor cannot tell.
    Unknown,
}

pub trait Executor: Send + Sync {
    /// Its name in the records: `workshop`.
    fn name(&self) -> &'static str;
    /// The layer listens: once, when the executor joins it.
    fn listen(&self, reports: Reporter);
    /// Begins the work the record describes (its wish, face, title, origin, id) — a new thing, or with `target` a new
    /// version of that one; why not, in words, when it cannot.
    fn start(&self, work: &Work) -> Result<Started, String>;
    /// Adds words to the work going on («и ещё кнопку…»): it takes them in on its way. False when it is not at it.
    fn add(&self, work: &Work, text: &str) -> bool;
    /// The executor's own thing a person names that no work names (an app on the desk): its id and title.
    fn thing(&self, name: &str) -> Option<Started>;
    /// Gets an existing thing ready for a new version (a draft from the installed app). A built-in app is not
    /// changed: with `copy` one's own copy of it is made and opened instead.
    fn open<'a>(&'a self, target: &'a str, copy: bool) -> BoxFuture<'a, Result<Started, Closed>>;
    /// What it is at now on the work's thing; None when nothing goes on.
    fn status(&self, work: &Work) -> Option<Doing>;
    /// What was done, in the executor's last words.
    fn details(&self, work: &Work) -> Option<String>;
    /// What the finished thing asks of the person before it goes on the desk.
    fn facts<'a>(&'a self, work: &'a Work) -> BoxFuture<'a, Facts>;
    /// Puts the finished thing where the person said: on the desk (by voice — `by`), to try on, into the drafts,
    /// away; why not, in words.
    fn put<'a>(&'a self, work: &'a Work, end: End, by: ByVoice) -> BoxFuture<'a, Result<Put, String>>;
    /// Stops the work; false when nothing went on.
    fn stop(&self, work: &Work) -> bool;
    /// Whether it is at the work's thing now; None when it cannot tell (the layer then believes the record).
    fn running(&self, _work: &Work) -> Option<bool> {
        None
    }
    /// Whether its thing of this id is there — the layer checks by the id, never by a name (TalkLayer::recheck).
    fn presence(&self, _target: &str) -> Presence {
        Presence::Unknown
    }
    /// What a background work came to: its retelling and its result («Готово» with a summary, ready.rs).
    fn told(&self, _work: &Work) -> Option<Told> {
        None
    }
}

/// An executor for the layer's tests: things by their ids — on the desk, as drafts, or gone — and what it was asked.
#[cfg(test)]
pub mod fake {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    pub struct Shelf {
        /// The things by their ids: their titles and where they are.
        pub things: Mutex<HashMap<String, (String, Presence)>>,
        pub started: Mutex<Vec<String>>,
        /// Whether it is at a thing now, by its id; a thing not here — it cannot tell.
        pub at: Mutex<HashMap<String, bool>>,
    }

    impl Shelf {
        pub fn with(things: &[(&str, &str, Presence)]) -> Arc<Shelf> {
            let shelf = Shelf::default();
            for (id, title, presence) in things {
                shelf.put(id, title, *presence);
            }
            Arc::new(shelf)
        }

        pub fn put(&self, id: &str, title: &str, presence: Presence) {
            self.things.lock().unwrap().insert(id.to_string(), (title.to_string(), presence));
        }
    }

    impl Executor for Shelf {
        fn name(&self) -> &'static str {
            "workshop"
        }
        fn listen(&self, _reports: Reporter) {}
        fn start(&self, work: &Work) -> Result<Started, String> {
            self.started.lock().unwrap().push(work.brief.clone());
            let target = work.target.clone().unwrap_or_else(|| format!("thing-{}", work.id));
            self.put(&target, &work.title, Presence::Draft);
            Ok(Started { target, title: work.title.clone(), note: None })
        }
        fn add(&self, _work: &Work, _text: &str) -> bool {
            false
        }
        /// By the id, then by the name, as the workshop finds its own.
        fn thing(&self, name: &str) -> Option<Started> {
            let things = self.things.lock().unwrap();
            let id = name.trim().trim_start_matches('[').trim_end_matches(']');
            let alive = |presence: &Presence| *presence != Presence::Gone;
            if let Some((title, _)) = things.get(id).filter(|(_, presence)| alive(presence)) {
                return Some(Started { target: id.to_string(), title: title.clone(), note: None });
            }
            let wanted = crate::talk::work_tools::stems(name);
            things
                .iter()
                .find(|(_, (title, presence))| {
                    alive(presence) && crate::talk::work_tools::alike(&crate::talk::work_tools::stems(title), &wanted)
                })
                .map(|(id, (title, _))| Started { target: id.clone(), title: title.clone(), note: None })
        }
        fn open<'a>(&'a self, target: &'a str, _copy: bool) -> BoxFuture<'a, Result<Started, Closed>> {
            Box::pin(async move { Ok(Started { target: target.to_string(), title: target.to_string(), note: None }) })
        }
        fn status(&self, _work: &Work) -> Option<Doing> {
            None
        }
        fn details(&self, _work: &Work) -> Option<String> {
            None
        }
        fn facts<'a>(&'a self, _work: &'a Work) -> BoxFuture<'a, Facts> {
            Box::pin(async { Facts::default() })
        }
        fn put<'a>(&'a self, _work: &'a Work, _end: End, _by: ByVoice) -> BoxFuture<'a, Result<Put, String>> {
            Box::pin(async { Ok(Put::Parked) })
        }
        fn stop(&self, _work: &Work) -> bool {
            false
        }
        fn presence(&self, target: &str) -> Presence {
            self.things.lock().unwrap().get(target).map_or(Presence::Gone, |(_, presence)| *presence)
        }
        fn running(&self, work: &Work) -> Option<bool> {
            self.at.lock().unwrap().get(work.target.as_deref()?).copied()
        }
    }
}
