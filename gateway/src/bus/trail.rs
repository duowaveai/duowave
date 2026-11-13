//! The trail: who started a chain of letters and how far it has gone (docs/plans/bus.md; crew's `chainOf` as the
//! model). Every letter carries one. The gateway sets it — the code of an app never sees it, so it can neither forge
//! nor drop it: a guest's request stays the guest's through every app it passes, and the rights of the whole chain are
//! never more than those of whoever started it.
//!
//! It travels two ways. In the gateway's own tasks it is a task-local (`Trail::scope`, `current`). An engine that
//! runs a plugin's code on a thread of its own (QuickJS: one call at a time per plugin) holds the trail of the call
//! it runs in a slot of that thread (`Trail::hold`), where the plugin's host calls read it.

use std::cell::RefCell;
use std::future::Future;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::core::identity::principal::{Principal, PrincipalKind};

/// How many apps a chain may pass after the first: crew's `max_hops`.
pub const MAX_HOPS: u8 = 4;
/// How many letters one chain may send in all: two apps answering each other stop here.
pub const MAX_LETTERS: u32 = 32;

/// Who started a chain.
#[derive(Debug, Clone, PartialEq)]
pub enum Root {
    /// A person signed in, the household's voice, an API key, or the gateway itself — as access knows them.
    Principal(Principal),
    /// An app acting on its own: its timer, an event, its own start.
    App(String),
}

/// What the root stands for, as crew counts authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Authority {
    /// A person: signed in, or the voice of the house.
    Person,
    /// A script or an agent with a key.
    Agent,
    /// An app on its own.
    App,
    /// The gateway itself.
    System,
}

impl Root {
    pub fn authority(&self) -> Authority {
        match self {
            Root::Principal(principal) => match principal.kind {
                PrincipalKind::User | PrincipalKind::Household => Authority::Person,
                PrincipalKind::ApiKey => Authority::Agent,
                PrincipalKind::System => Authority::System,
            },
            Root::App(_) => Authority::App,
        }
    }

    /// The person, key or gateway behind it; none for an app on its own.
    pub fn principal(&self) -> Option<&Principal> {
        match self {
            Root::Principal(principal) => Some(principal),
            Root::App(_) => None,
        }
    }

    /// For the journal: `user:<id>`, `household`, `apiKey:<id>`, `system`, `app:<id>`.
    pub fn label(&self) -> String {
        match self {
            Root::Principal(principal) => match principal.kind {
                PrincipalKind::Household => "household".into(),
                PrincipalKind::System => "system".into(),
                kind => format!("{}:{}", kind.as_str(), principal.id),
            },
            Root::App(id) => format!("app:{id}"),
        }
    }

    /// For people: «Мама», «Дом», «приложение radio».
    pub fn title(&self) -> String {
        match self {
            Root::Principal(principal) => principal.name.clone(),
            Root::App(id) => format!("приложение {id}"),
        }
    }
}

/// Why a chain may not go on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TrailError {
    #[error("цепочка слишком длинная: больше {MAX_HOPS} шагов от приложения к приложению")]
    Hops,
    #[error("круг: приложение {0} уже в этой цепочке и ждёт ответа")]
    Loop(String),
    #[error("цепочка исчерпала свои {MAX_LETTERS} писем")]
    Letters,
    #[error("цепочку отменили")]
    Cancelled,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn next_id() -> u64 {
    let seed = NEXT_ID.load(Ordering::Relaxed);
    if seed == 0 {
        // Ids only tie the journal's lines together: a seed from the clock keeps them apart across restarts.
        let start = (crate::utils::time::now_ms() & 0xFFFF_FFFF) << 16;
        let _ = NEXT_ID.compare_exchange(0, start.max(1), Ordering::Relaxed, Ordering::Relaxed);
    }
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone)]
pub struct Trail {
    pub id: u64,
    pub root: Arc<Root>,
    /// The apps the chain entered, the first first.
    pub chain: Vec<String>,
    /// Steps from app to app after the first.
    pub hops: u8,
    letters: Arc<AtomicU32>,
    /// «Алиса, отмени»: cancels the whole chain below.
    pub cancel: CancellationToken,
}

tokio::task_local! {
    static CURRENT: Trail;
}

thread_local! {
    static ENGINE: RefCell<Option<Trail>> = const { RefCell::new(None) };
}

/// The trail of the work going on here: the task's, or the engine thread's call's.
pub fn current() -> Option<Trail> {
    CURRENT.try_with(Clone::clone).ok().or_else(|| ENGINE.with(|slot| slot.borrow().clone()))
}

/// Holds a trail in the engine thread's slot while a call runs; puts back what was there when dropped.
pub struct Held(Option<Trail>);

impl Drop for Held {
    fn drop(&mut self) {
        let previous = self.0.take();
        ENGINE.with(|slot| *slot.borrow_mut() = previous);
    }
}

impl Trail {
    pub fn of(root: Root) -> Trail {
        Trail {
            id: next_id(),
            root: Arc::new(root),
            chain: Vec::new(),
            hops: 0,
            letters: Arc::new(AtomicU32::new(0)),
            cancel: CancellationToken::new(),
        }
    }

    /// A chain a person, the household, a key or the gateway starts.
    pub fn principal(principal: &Principal) -> Trail {
        Trail::of(Root::Principal(principal.clone()))
    }

    /// A chain an app starts on its own.
    pub fn app(id: &str) -> Trail {
        Trail::of(Root::App(id.to_string()))
    }

    /// The gateway's own.
    pub fn system() -> Trail {
        Trail::of(Root::Principal(crate::core::identity::principal::system()))
    }

    pub fn authority(&self) -> Authority {
        self.root.authority()
    }

    /// The trail as `app` takes it: a step further once the chain has an app in it; refused past the hops, into a
    /// loop, or once cancelled.
    pub fn entering(&self, app: &str) -> Result<Trail, TrailError> {
        if self.cancel.is_cancelled() {
            return Err(TrailError::Cancelled);
        }
        if self.chain.iter().any(|id| id == app) {
            // Its own call into itself is not a step (a tool of its own); another app's way back into it is a loop.
            if self.chain.last().is_some_and(|last| last == app) {
                return Ok(self.clone());
            }
            return Err(TrailError::Loop(app.to_string()));
        }
        let hops = if self.chain.is_empty() { 0 } else { self.hops + 1 };
        if hops > MAX_HOPS {
            return Err(TrailError::Hops);
        }
        let mut next = self.clone();
        next.chain.push(app.to_string());
        next.hops = hops;
        Ok(next)
    }

    /// Counts a letter of the chain.
    pub fn spend(&self) -> Result<(), TrailError> {
        if self.cancel.is_cancelled() {
            return Err(TrailError::Cancelled);
        }
        if self.letters.fetch_add(1, Ordering::Relaxed) >= MAX_LETTERS {
            return Err(TrailError::Letters);
        }
        Ok(())
    }

    pub fn letters(&self) -> u32 {
        self.letters.load(Ordering::Relaxed)
    }

    /// The app that sends from here: the last one the chain entered.
    pub fn app_now(&self) -> Option<&str> {
        self.chain.last().map(String::as_str)
    }

    /// Runs `future` with this trail as the task's.
    pub async fn scope<F: Future>(self, future: F) -> F::Output {
        CURRENT.scope(self, future).await
    }

    /// Puts the trail in this thread's slot until the guard goes (an engine's thread, around one call).
    pub fn hold(trail: Option<Trail>) -> Held {
        Held(ENGINE.with(|slot| std::mem::replace(&mut *slot.borrow_mut(), trail)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::identity::principal::household_principal;
    use crate::core::identity::roles::Role;

    #[test]
    fn a_chain_counts_steps_after_its_first_app_and_stops_past_the_limit() {
        let trail = Trail::principal(&household_principal(Role::Guest, None));
        assert_eq!(trail.authority(), Authority::Person);
        let mut at = trail.entering("a").unwrap();
        assert_eq!(at.hops, 0);
        for (step, app) in ["b", "c", "d", "e"].iter().enumerate() {
            at = at.entering(app).unwrap();
            assert_eq!(at.hops as usize, step + 1);
        }
        assert_eq!(at.entering("f").unwrap_err(), TrailError::Hops);
        assert_eq!(at.root.label(), "household");
    }

    #[test]
    fn a_way_back_into_an_app_of_the_chain_is_a_loop_but_its_own_call_is_not() {
        let at = Trail::app("a").entering("a").unwrap().entering("b").unwrap();
        assert_eq!(at.entering("a").unwrap_err(), TrailError::Loop("a".into()));
        assert_eq!(at.entering("b").unwrap().hops, at.hops);
        assert_eq!(Trail::app("a").authority(), Authority::App);
    }

    #[test]
    fn letters_are_shared_by_the_whole_chain_and_cancel_stops_it() {
        let trail = Trail::system();
        let below = trail.entering("a").unwrap();
        for _ in 0..MAX_LETTERS {
            below.spend().unwrap();
        }
        assert_eq!(trail.spend().unwrap_err(), TrailError::Letters);
        let other = Trail::system();
        other.cancel.cancel();
        assert_eq!(other.entering("a").unwrap_err(), TrailError::Cancelled);
    }

    #[tokio::test]
    async fn the_trail_is_the_tasks_and_a_held_one_is_the_threads() {
        assert!(current().is_none());
        let trail = Trail::app("radio");
        let id = trail.id;
        assert_eq!(trail.clone().scope(async { current().map(|t| t.id) }).await, Some(id));
        assert!(current().is_none());
        {
            let _held = Trail::hold(Some(trail));
            assert_eq!(current().map(|t| t.id), Some(id));
        }
        assert!(current().is_none());
    }
}
