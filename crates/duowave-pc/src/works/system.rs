//! Our own runner as a work system (docs/plans/work-systems.md, «Our own system», stage 1): `duowave`, the first
//! implementation of the contract. The work folders are its places, the agents its setups; a work is one run of an
//! agent in a copy of the folder, its branch, the folder's checks, the journal, the deals — as before. The layer still
//! leads these works by the folder executor (talk/folder.rs), with their budgets and questions; this face is what the
//! contract asks of every system: what it can, where, on what, the kit.

use std::sync::Arc;

use async_trait::async_trait;

use super::{FolderWorks, State};
use crate::memory::text::Lang;
use crate::talk::folder::{FolderEnd, FolderStart, PortEvent, WorksPort};
use crate::talk::system::{
    About, Artifact, ArtifactKind, Can, Outcome, Place, Refusal, Seen, Setup, Spend, Start, Status, SystemEvent,
    SystemListener, SystemReady, SystemState, When, WorkSystem, DUOWAVE,
};

/// How many of its works `list` gives: the newest.
const LISTED: usize = 50;

pub struct DuowaveSystem {
    works: Arc<FolderWorks>,
}

impl DuowaveSystem {
    pub fn new(works: Arc<FolderWorks>) -> Arc<DuowaveSystem> {
        Arc::new(DuowaveSystem { works })
    }
}

fn state_of(state: State) -> SystemState {
    match state {
        State::Working => SystemState::Working,
        State::Waiting => SystemState::WaitsLimit,
        State::Ready => SystemState::Ready,
        State::Unfinished => SystemState::Unfinished,
        State::Failed => SystemState::Failed,
        State::Stopped => SystemState::Stopped,
        State::Interrupted => SystemState::Interrupted,
    }
}

fn ended(end: FolderEnd) -> SystemState {
    match end {
        FolderEnd::Ready => SystemState::Ready,
        FolderEnd::Unfinished => SystemState::Unfinished,
        FolderEnd::Failed => SystemState::Failed,
        FolderEnd::Stopped => SystemState::Stopped,
    }
}

#[async_trait]
impl WorkSystem for DuowaveSystem {
    fn id(&self) -> String {
        DUOWAVE.into()
    }

    fn about(&self) -> About {
        About {
            id: DUOWAVE.into(),
            title: "DuoWave".into(),
            names: vec!["DuoWave".into(), "дуовейв".into()],
            plug: "runner".into(),
            protocol: None,
            version: Some(crate::VERSION.into()),
        }
    }

    async fn ready(&self) -> SystemReady {
        if self.works.folders().is_empty() {
            return SystemReady::no(crate::tr!("Рабочих папок пока нет."));
        }
        SystemReady::yes()
    }

    fn can(&self) -> Can {
        Can {
            start: true,
            list: true,
            steps: false,
            tell_now: true,
            tell_after: true,
            stop: true,
            questions: false,
            result: true,
            artifacts: true,
            spend: Spend::Exact,
            limit: true,
            places: true,
            setups: true,
            ceiling: true,
            open: false,
            actions: false,
        }
    }

    async fn places(&self) -> Vec<Place> {
        super::folders::list(&self.works.settings)
            .into_iter()
            .map(|folder| Place { id: folder.id, title: folder.name, path: Some(folder.path) })
            .collect()
    }

    async fn setups(&self) -> Vec<Setup> {
        self.works.agents().into_iter().map(|(id, title)| Setup { id, title }).collect()
    }

    async fn start(&self, start: Start) -> Result<Seen, Refusal> {
        let folder = start.place.clone().ok_or_else(|| Refusal::because(crate::tr!("Назовите рабочую папку.")))?;
        let title = start.title.clone().unwrap_or_else(|| start.task.clone());
        let resume = crate::talk::pack::of(Lang::Ru).work.folder.resume_prompt.to_string();
        let id = self
            .works
            .start(FolderStart {
                folder: folder.clone(),
                task: start.task,
                title: title.clone(),
                agent: start.setup,
                model: None,
                resume,
                set: None,
                way: None,
            })
            .map_err(Refusal::because)?;
        Ok(Seen { id, title, place: Some(folder), state: SystemState::Working, line: None, at: None, outside: false })
    }

    async fn list(&self) -> Vec<Seen> {
        self.works
            .records(LISTED)
            .into_iter()
            .map(|record| Seen {
                id: record.id,
                title: record.title,
                place: Some(record.folder),
                state: state_of(record.state),
                line: record.last,
                at: Some(record.updated_at),
                outside: false,
            })
            .collect()
    }

    async fn status(&self, work: &str) -> Option<Status> {
        let record = self.works.record(work)?;
        Some(Status { state: state_of(record.state), line: record.last, at: Some(record.updated_at) })
    }

    async fn tell(&self, work: &str, text: &str, when: When) -> Result<(), Refusal> {
        match when {
            When::Now if self.works.add(work, text) => Ok(()),
            When::Now => Err(Refusal::because(crate::tr!("Эта работа сейчас не идёт."))),
            When::After => {
                let resume = self.works.can_again(work).map_err(Refusal::because)?;
                self.works.again(work, text, &resume).map_err(Refusal::because)
            }
        }
    }

    async fn stop(&self, work: &str) -> Result<(), Refusal> {
        if self.works.stop(work) {
            Ok(())
        } else {
            Err(Refusal::because(crate::tr!("Эта работа сейчас не идёт.")))
        }
    }

    async fn result(&self, work: &str) -> Option<Outcome> {
        let told = WorksPort::told(self.works.as_ref(), work)?;
        let artifacts = (!told.branch.is_empty())
            .then(|| Artifact { kind: ArtifactKind::Branch, title: told.folder.clone(), at: told.branch.clone() })
            .into_iter()
            .collect();
        Some(Outcome { summary: told.last.clone(), more: None, artifacts })
    }

    fn listen(&self, events: SystemListener) {
        let works = Arc::downgrade(&self.works);
        self.works.listen(Arc::new(move |event: PortEvent| {
            let event = match event {
                PortEvent::Waits { id, until, .. } => SystemEvent::Limit { work: id, until },
                PortEvent::Goes { id } => SystemEvent::State { work: id, state: SystemState::Working, line: None },
                PortEvent::Budget { id, .. } => {
                    SystemEvent::State { work: id, state: SystemState::WaitsPerson, line: None }
                }
                PortEvent::Plan { id, plan } => {
                    SystemEvent::State { work: id, state: SystemState::WaitsPerson, line: Some(plan) }
                }
                PortEvent::Ran { id, spent, .. } => match spent.bill.as_ref().and_then(|bill| bill.usd_micro) {
                    Some(usd_micro) => SystemEvent::Spent { work: id, usd_micro, mark: Spend::Exact },
                    None => return,
                },
                PortEvent::Ended { id, end } => {
                    let told = works.upgrade().and_then(|works| WorksPort::told(works.as_ref(), &id));
                    let outcome = Outcome { summary: told.and_then(|told| told.last), ..Outcome::default() };
                    SystemEvent::Ended { work: id, state: ended(end), outcome }
                }
                PortEvent::Taken { .. } | PortEvent::Model { .. } => return,
            };
            events(DUOWAVE, event);
        }));
    }
}
