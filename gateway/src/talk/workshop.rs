//! The workshop as an executor of works (docs/plans/audio-interface.md, «Исполнитель»): over `Workshop::make`, the
//! agent's activity and the drafts' cards. Every run of the agent on a draft with a card is a work — whoever started
//! it: a conversation, the panel's workshop, its «+» — so the brain knows what goes on. The card says how the draft
//! stands, the activity what the agent does this minute.

use std::sync::{Arc, OnceLock};

use futures::future::BoxFuture;

use super::executor::{Began, ByVoice, Closed, Doing, Executor, Facts, Presence, Put, Report, Reporter, Started};
use super::origin::{Activator, Origin, COMPUTER};
use super::works::{End, Face, Work, WorkState};
use crate::plugins::workshop::activity::Moment;
use crate::plugins::workshop::activity::{By, StepKind};
use crate::plugins::workshop::meta::{DraftMeta, Face as DraftFace, Stage};
use crate::plugins::workshop::Workshop;

/// The executor's name in the records.
pub const WORKSHOP: &str = "workshop";

/// Whether any of these rights reaches a handle only the panel approves.
pub type Dangers = Arc<dyn Fn(&[String]) -> bool + Send + Sync>;

pub struct WorkshopExecutor {
    workshop: Arc<Workshop>,
    reports: OnceLock<Reporter>,
    /// Whether a panel is open now: «Открыто в пульте» or «Плитка ждёт на столе».
    panels: OnceLock<Arc<dyn Fn() -> bool + Send + Sync>>,
    /// Whether any of these rights reaches a handle only the panel approves (src/bus/danger.rs).
    dangers: OnceLock<Dangers>,
}

/// A draft's face as a work's.
pub fn face_of(face: DraftFace) -> Face {
    match face {
        DraftFace::Widget => Face::Widget,
        DraftFace::Skill => Face::Skill,
        DraftFace::Game => Face::Game,
        DraftFace::Theme => Face::Theme,
    }
}

/// Where the card says the work was asked: a station by its wake word, or the panel by a person signed in.
fn origin_of(card: &DraftMeta) -> Origin {
    match card.by {
        By::Voice => Origin::station(card.who.as_deref().unwrap_or_default(), true),
        By::Panel => Origin::computer(Activator::Text).by(card.who.clone()),
    }
}

/// A work's face as a draft's.
fn draft_face(face: Face) -> DraftFace {
    match face {
        Face::Widget => DraftFace::Widget,
        Face::Skill => DraftFace::Skill,
        Face::Game => DraftFace::Game,
        Face::Theme => DraftFace::Theme,
    }
}

/// What a step of the agent is about.
fn doing_of(tool: Option<&str>) -> Doing {
    match tool.unwrap_or_default() {
        "write_file" | "append_file" | "edit_file" | "delete_file" => Doing::Writing,
        "read_file" | "draft_files" => Doing::Reading,
        "check" => Doing::Checking,
        "run_function" => Doing::Trying,
        "try_phrase" => Doing::Phrase,
        "docs" => Doing::Docs,
        "plugin_journal" => Doing::Journal,
        _ => Doing::Thinking,
    }
}

impl WorkshopExecutor {
    pub fn new(workshop: Arc<Workshop>) -> Arc<WorkshopExecutor> {
        let executor = Arc::new(WorkshopExecutor {
            workshop: workshop.clone(),
            reports: OnceLock::new(),
            panels: OnceLock::new(),
            dangers: OnceLock::new(),
        });
        let me = Arc::downgrade(&executor);
        workshop.follow_cards(Arc::new(move |draft: &str, card: &DraftMeta| {
            if let Some(executor) = me.upgrade() {
                executor.card_changed(draft, card);
            }
        }));
        // The agent took in what was added on the way: the work's notes are read.
        let me = Arc::downgrade(&executor);
        workshop.follow_work(Arc::new(move |draft: &str, moment: &Moment| {
            if let (Some(executor), Moment::NotesRead { .. }) = (me.upgrade(), moment) {
                executor.report(Report::Read { target: draft.to_string() });
            }
        }));
        executor
    }

    /// Whether a panel is open now (the live channel's listeners).
    pub fn set_panels(&self, panels: Arc<dyn Fn() -> bool + Send + Sync>) {
        let _ = self.panels.set(panels);
    }

    /// Which rights are dangerous: the bus knows its handles, once it stands.
    pub fn set_dangers(&self, dangers: Dangers) {
        let _ = self.dangers.set(dangers);
    }

    /// Whether a «да» said to a station puts a thing on the desk (`talk.desk_by_voice`, on by default).
    fn desk_by_voice(&self) -> bool {
        self.workshop.host().settings().get(&crate::settings::definitions::TALK_DESK_BY_VOICE, None).unwrap_or(true)
    }

    fn panel_open(&self) -> bool {
        self.panels.get().is_some_and(|open| open())
    }

    fn report(&self, report: Report) {
        if let Some(reports) = self.reports.get() {
            reports(WORKSHOP, report);
        }
    }

    /// The draft's title as people see it: its manifest's, or the card's words.
    fn title_of(&self, draft: &str, card: &DraftMeta) -> String {
        self.workshop
            .draft(draft)
            .ok()
            .and_then(|view| view.title)
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| card.asked.chars().take(40).collect())
    }

    /// A card changed: a run began or ended, the person put the thing on the desk or into the drafts.
    fn card_changed(&self, draft: &str, card: &DraftMeta) {
        let target = draft.to_string();
        // The name the agent gave it in its manifest: «Сделай часы секундами» once stayed a clock's name (live 14.11).
        let named =
            self.workshop.draft(draft).ok().and_then(|view| view.title).filter(|title| !title.trim().is_empty());
        let ended =
            |state: WorkState| Report::Ended { target: target.clone(), state, line: None, title: named.clone() };
        let report = match card.stage {
            Stage::Working => Report::Working(Began {
                target: target.clone(),
                work: card.work.clone(),
                face: Some(face_of(card.kind)),
                title: self.title_of(draft, card),
                brief: card.asked.clone(),
                origin: origin_of(card),
            }),
            Stage::Ready => ended(WorkState::Ready),
            Stage::Failed => ended(WorkState::Failed),
            Stage::Stopped => ended(WorkState::Stopped),
            Stage::Interrupted => ended(WorkState::Interrupted),
            Stage::Unfinished => ended(WorkState::Unfinished),
            Stage::Installed => Report::Decided { target, end: End::Desk },
            Stage::Parked => Report::Decided { target, end: End::Parked },
        };
        self.report(report);
    }
}

impl Executor for WorkshopExecutor {
    fn name(&self) -> &'static str {
        WORKSHOP
    }

    fn listen(&self, reports: Reporter) {
        let _ = self.reports.set(reports);
    }

    /// A new draft of the work's face from its wish — or, with a target, new work on that draft: the station that
    /// asked hears «Готово» (its card is the voice's); the computer's talk is the panel's person's.
    fn start(&self, work: &Work) -> Result<Started, String> {
        let (by, who) = if work.device() == COMPUTER {
            (By::Panel, work.origin.who.clone())
        } else {
            (By::Voice, Some(work.device().to_string()))
        };
        if let Some(draft) = work.target.as_deref() {
            let made = self
                .workshop
                .make_for(&work.brief, Some(draft), by, who, Some(&work.id))
                .map_err(|error| error.message)?;
            return Ok(Started { target: made.draft, title: made.title, note: made.note });
        }
        let face = draft_face(work.face.unwrap_or(Face::Widget));
        let title = Some(work.title.as_str()).filter(|title| !title.trim().is_empty());
        let made =
            self.workshop.begin(face, title, &work.brief, by, who, Some(&work.id)).map_err(|error| error.message)?;
        Ok(Started { target: made.draft, title: made.title, note: made.note })
    }

    fn add(&self, work: &Work, text: &str) -> bool {
        work.target.as_deref().is_some_and(|draft| self.workshop.note(draft, text))
    }

    /// An app on the desk or a draft — by its id first («[weather]», as «Сейчас» lists the desk), else by its name;
    /// only when one is sure.
    fn thing(&self, name: &str) -> Option<Started> {
        let host = self.workshop.host();
        let id = name.trim().trim_start_matches('[').trim_end_matches(']');
        if let Some(package) = host.visible_all().into_iter().find(|package| package.manifest.id == id) {
            return Some(Started { target: id.to_string(), title: package.manifest.title.clone(), note: None });
        }
        if let Ok(view) = self.workshop.draft(id) {
            return Some(Started {
                target: id.to_string(),
                title: view.title.unwrap_or_else(|| id.to_string()),
                note: None,
            });
        }
        let wanted = super::work_tools::stems(name);
        let drafts = self.workshop.drafts().into_iter().filter_map(|view| Some((view.id, view.title?)));
        let installed =
            host.visible_all().into_iter().map(|package| (package.manifest.id.clone(), package.manifest.title.clone()));
        let mut found: Vec<(String, String)> = Vec::new();
        for (id, title) in drafts.chain(installed) {
            let fits = super::work_tools::alike(&super::work_tools::stems(&title), &wanted);
            if fits && !found.iter().any(|(known, _)| *known == id) {
                found.push((id, title));
            }
        }
        (found.len() == 1).then(|| {
            let (target, title) = found.remove(0);
            Started { target, title, note: None }
        })
    }

    fn open<'a>(&'a self, target: &'a str, copy: bool) -> BoxFuture<'a, Result<Started, Closed>> {
        Box::pin(async move {
            let workshop = &self.workshop;
            if let Ok(view) = workshop.draft(target) {
                let title = view.title.unwrap_or_else(|| target.to_string());
                return Ok(Started { target: target.to_string(), title, note: None });
            }
            let host = workshop.host();
            let package = host.installed(target).ok_or_else(|| Closed::Failed(crate::tr!("Такого приложения нет.")))?;
            let title = package.manifest.title.clone();
            if host.origin_of(target).editable() {
                let view = workshop.open(target).map_err(|error| Closed::Failed(error.message))?;
                return Ok(Started { target: target.to_string(), title: view.title.unwrap_or(title), note: None });
            }
            if !copy {
                return Err(Closed::BuiltIn(title));
            }
            let copied = host.fork(target, None, None).await.map_err(|error| Closed::Failed(error.message))?;
            let view = workshop.open(&copied.id).map_err(|error| Closed::Failed(error.message))?;
            Ok(Started { target: copied.id.clone(), title: view.title.unwrap_or(title), note: None })
        })
    }

    fn details(&self, work: &Work) -> Option<String> {
        let draft = work.target.as_deref()?;
        self.workshop.card(draft).note.filter(|note| !note.trim().is_empty())
    }

    fn facts<'a>(&'a self, work: &'a Work) -> BoxFuture<'a, Facts> {
        Box::pin(async move {
            let Some(draft) = work.target.as_deref() else { return Facts::default() };
            let unchanged = self.workshop.card(draft).unchanged;
            let failure = self.workshop.activity().failure(draft);
            let host = self.workshop.host();
            let installed = host.installed(draft).is_some();
            // What is filled in already: the installed app's settings (a key shows as «••••••»).
            let filled: Vec<String> = match installed {
                true => host
                    .view_one(draft)
                    .await
                    .map(|view| {
                        view.settings
                            .into_iter()
                            .filter(|setting| setting.value.is_some())
                            .map(|setting| setting.spec.key)
                            .collect()
                    })
                    .unwrap_or_default(),
                false => Vec::new(),
            };
            let needs = self
                .workshop
                .draft(draft)
                .map(|view| {
                    view.needs
                        .into_iter()
                        .filter(|need| need.required && !filled.contains(&need.key))
                        .map(|need| need.title)
                        .collect()
                })
                .unwrap_or_default();
            match self.workshop.plan(draft).await {
                Ok(plan) => {
                    let added: Vec<String> = plan
                        .added
                        .iter()
                        .cloned()
                        .chain(plan.hosts_added.iter().map(|host| format!("net:{host}")))
                        .collect();
                    let dangerous = self.dangers.get().is_some_and(|dangers| dangers(&added));
                    let rights = plan.added;
                    Facts {
                        needs,
                        hosts: plan.hosts_added,
                        rights,
                        dangerous,
                        installed,
                        ready: true,
                        unchanged,
                        failure,
                    }
                }
                Err(_) => Facts { needs, installed, unchanged, failure, ..Facts::default() },
            }
        })
    }

    fn put<'a>(&'a self, work: &'a Work, end: End, by: ByVoice) -> BoxFuture<'a, Result<Put, String>> {
        Box::pin(async move {
            let draft = work.target.as_deref().ok_or_else(|| crate::tr!("Черновика ещё нет."))?;
            let open = self.panel_open();
            match end {
                End::Fitting => {
                    self.workshop.tell(draft, "open:fitting");
                    Ok(Put::Shown { open })
                }
                End::Parked => {
                    self.workshop.park(draft).map_err(|error| error.message)?;
                    Ok(Put::Parked)
                }
                End::Declined => {
                    self.workshop.delete(draft).map_err(|error| error.message)?;
                    Ok(Put::Deleted)
                }
                End::Removed => Err(crate::tr!("Этой вещи больше нет.")),
                End::Desk => {
                    // Only the panel puts it there: the owner keeps the desk by voice off, or a right is dangerous.
                    if !self.desk_by_voice() {
                        self.workshop.tell(draft, "open:consent");
                        return Ok(Put::Approve { open });
                    }
                    let facts = self.facts(work).await;
                    if facts.dangerous {
                        self.workshop.tell(draft, "open:consent");
                        return Ok(Put::Dangerous { open });
                    }
                    // The «да» after the question that named the new rights and hosts is the consent; the audit
                    // says it was by voice, on which device.
                    let actor = crate::core::events::Actor {
                        kind: crate::core::identity::principal::PrincipalKind::Household,
                        id: format!("voice:{}", by.device),
                        name: crate::tr!("голосом, {device}", device = by.device),
                    };
                    let report = self
                        .workshop
                        .install(draft, true, &serde_json::Map::new(), Some(actor))
                        .await
                        .map_err(|error| error.message)?;
                    if !report.installed {
                        return Err(report.message);
                    }
                    // A key nobody filled in: it is on the desk all the same, the panel opens its fields.
                    if !facts.needs.is_empty() {
                        self.workshop.tell(draft, "open:needs");
                    }
                    Ok(Put::OnDesk { needs: facts.needs, open })
                }
            }
        })
    }

    fn status(&self, work: &Work) -> Option<Doing> {
        let draft = work.target.as_deref()?;
        let (job, _) = self.workshop.activity().log(draft, u64::MAX)?;
        if !job.busy {
            return None;
        }
        Some(match &job.last {
            Some(step) if step.kind == StepKind::Tool => doing_of(step.tool.as_deref()),
            Some(step) if step.kind == StepKind::Result => Doing::Checking,
            _ => Doing::Thinking,
        })
    }

    /// By the id: on the desk (installed), only a draft, or gone.
    fn presence(&self, target: &str) -> Presence {
        if self.workshop.host().installed(target).is_some() {
            Presence::OnDesk
        } else if self.workshop.draft(target).is_ok() {
            Presence::Draft
        } else {
            Presence::Gone
        }
    }

    fn stop(&self, work: &Work) -> bool {
        work.target.as_deref().is_some_and(|draft| self.workshop.stop(draft))
    }

    fn running(&self, work: &Work) -> Option<bool> {
        work.target.as_deref().map(|draft| self.workshop.working(draft))
    }
}
