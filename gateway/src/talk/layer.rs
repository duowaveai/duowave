//! `TalkLayer` (docs/plans/audio-interface.md, «Слой разговора»): one service per gateway, its state per device —
//! the records of works, what the brain knows now. Executors report to it; the brain's request reads its block; the
//! panel reads its works. (`Talk` in this module is the answer on its way — the mic mode's name.)

use std::sync::{Arc, OnceLock, RwLock, Weak};

use super::executor::{Began, Executor, Presence, Report};
use super::now::{self, Sources};
use super::pack;
use super::works::{End, NewWork, Outcome, Work, WorkResult, WorkState, Works, APP};
use crate::core::events::{EventBus, GatewayEvent};
use crate::memory::text::Lang;
use crate::storage::Db;
use crate::utils::time::now_ms;

/// The apps that are on, for the block: their ids and titles — «Погода [weather]», so a thing is named by its id —,
/// and whether each runs on the home's Турбо.
pub type Desk = Arc<dyn Fn() -> Vec<DeskApp> + Send + Sync>;

/// An app that is on, as the block names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskApp {
    pub id: String,
    pub title: String,
    /// Its code runs on the home's Турбо (a face here, docs/plans/home-and-devices.md, stage 5б).
    pub turbo: bool,
}

/// The language a device's character speaks: its pack's words for the phrases and the block.
pub type Langs = Arc<dyn Fn(&str) -> Lang + Send + Sync>;

/// Who keeps a device's conversation past its pause, until when (ms).
pub type Hold = Arc<dyn Fn(&str, u64) + Send + Sync>;

pub struct TalkLayer {
    /// Itself, for what it does later: the queue moves on its own timer.
    me: Weak<TalkLayer>,
    pub(super) works: Works,
    /// The workshop's door of each device (door.rs).
    pub(super) door: super::door::Door,
    /// The question each device was just asked (asked.rs).
    pub(super) asked: super::asked::Asked,
    pub(super) events: EventBus,
    executors: RwLock<Vec<Arc<dyn Executor>>>,
    desk: OnceLock<Desk>,
    langs: OnceLock<Langs>,
    /// Where a station says «Готово» (ready.rs), once the session registry stands.
    pub(super) turns: RwLock<Option<Arc<dyn crate::plugins::skill::TurnStation>>>,
    /// A line a station did not say to its end: said once more after the device's next conversation.
    pub(super) unsaid: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// The last new version a device's voice put on the desk at once, for «верни как было»: the thing and when.
    pub(super) edits: std::sync::Mutex<std::collections::HashMap<String, (String, u64)>>,
    /// Phrases to keep ahead in our own voice (a work's «…: готово.»), once the voice stands.
    warm: OnceLock<Arc<dyn Fn(Vec<String>) + Send + Sync>>,
    /// Who keeps a device's conversation past its pause (the dialog), until when.
    hold: OnceLock<Hold>,
    /// The work a device's phrase began or added to last, and when: the panel's talk answers with it (`work`).
    touched: std::sync::Mutex<std::collections::HashMap<String, (String, u64)>>,
    /// The work a device's phrase acted on last (began, added to, asked about, put on the desk), and when: the history
    /// keeps its line in place of the layer's phrase (`fact_since`).
    acted: std::sync::Mutex<std::collections::HashMap<String, (String, u64)>>,
    /// Reads a phrase about the works before the talk (reader.rs), once the dialog stands.
    reader: OnceLock<Arc<super::reader::Reader>>,
    /// The works whose thing goes on the desk once it is ready: «на стол» was said while it was being made.
    pub(super) planned: std::sync::Mutex<std::collections::HashSet<String>>,
    /// The day (since the epoch) each device last heard how to add on the way.
    hinted: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    /// MagicPlace by voice (store.rs): its rule, its questions.
    pub(super) store: super::store::Store,
    /// The background tools' executor (tool_work.rs): a paid run's «да», «подробнее».
    background: OnceLock<Weak<super::tool_work::ToolExecutor>>,
    /// The works in folders' executor (folder.rs): «Новая работа».
    folders: OnceLock<Weak<super::folder::FolderExecutor>>,
}

/// A device's conversation is remembered this long after its work last changed.
pub const REMEMBERED_MS: u64 = 30 * 60_000;

/// A work going in its record and untouched this long, its executor at nothing on it, is settled as interrupted: the
/// time a run takes to show itself after its work began, with room to spare.
pub const SETTLE_AFTER_MS: u64 = 60_000;

impl TalkLayer {
    /// The layer over the works the table keeps: what went on when the gateway stopped is interrupted.
    pub fn new(db: Option<Arc<Db>>, events: EventBus) -> Arc<TalkLayer> {
        let layer = Arc::new_cyclic(|me| TalkLayer {
            me: me.clone(),
            works: Works::open(db),
            door: super::door::Door::default(),
            asked: super::asked::Asked::default(),
            store: super::store::Store::new(events.clone()),
            events,
            executors: RwLock::new(Vec::new()),
            desk: OnceLock::new(),
            langs: OnceLock::new(),
            turns: RwLock::new(None),
            unsaid: std::sync::Mutex::new(std::collections::HashMap::new()),
            edits: std::sync::Mutex::new(std::collections::HashMap::new()),
            warm: OnceLock::new(),
            hold: OnceLock::new(),
            touched: std::sync::Mutex::new(std::collections::HashMap::new()),
            acted: std::sync::Mutex::new(std::collections::HashMap::new()),
            reader: OnceLock::new(),
            planned: std::sync::Mutex::new(std::collections::HashSet::new()),
            hinted: std::sync::Mutex::new(std::collections::HashMap::new()),
            background: OnceLock::new(),
            folders: OnceLock::new(),
        });
        // A question nobody answered in its time ends silent: the work's question says so, and waits.
        let me = Arc::downgrade(&layer);
        layer.asked.on_silence(Arc::new(move |device: &str, question: &super::asked::Question| {
            if let Some(layer) = me.upgrade() {
                layer.silent(device, question);
            }
        }));
        layer
    }

    pub fn works(&self) -> &Works {
        &self.works
    }

    /// The background tools' executor joined (it added itself as an executor too).
    pub fn set_background(&self, executor: &Arc<super::tool_work::ToolExecutor>) {
        let _ = self.background.set(Arc::downgrade(executor));
    }

    pub(super) fn background(&self) -> Option<Arc<super::tool_work::ToolExecutor>> {
        self.background.get().and_then(Weak::upgrade)
    }

    /// The works in folders' executor joined (it added itself as an executor too).
    pub fn set_folders(&self, executor: &Arc<super::folder::FolderExecutor>) {
        let _ = self.folders.set(Arc::downgrade(executor));
    }

    pub fn folders(&self) -> Option<Arc<super::folder::FolderExecutor>> {
        self.folders.get().and_then(Weak::upgrade)
    }

    /// An executor joins: from now on it reports here.
    pub fn add_executor(self: &Arc<Self>, executor: Arc<dyn Executor>) {
        let me = Arc::downgrade(self);
        executor.listen(Arc::new(move |name: &'static str, report: Report| {
            if let Some(layer) = me.upgrade() {
                layer.heard(name, report);
            }
        }));
        self.executors.write().unwrap_or_else(|poisoned| poisoned.into_inner()).push(executor);
    }

    pub(super) fn executor(&self, name: &str) -> Option<Arc<dyn Executor>> {
        let executors = self.executors.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        executors.iter().find(|executor| executor.name() == name).cloned()
    }

    /// The apps on the desk, once the plugins stand.
    pub fn set_desk(&self, desk: Desk) {
        let _ = self.desk.set(desk);
    }

    /// MagicPlace by voice, once the catalog stands: what it installs from, the switch, whether a panel listens.
    pub fn set_store(
        &self,
        catalog: std::sync::Weak<crate::plugins::catalog::PackageCatalog>,
        settings: Arc<crate::settings::SettingsService>,
        panels: super::store::Panels,
    ) {
        self.store.attach(catalog, settings, panels);
    }

    /// Who keeps phrases ahead in our own voice (src/speech/tts/phrase_cache.rs).
    pub fn set_warm(&self, warm: Arc<dyn Fn(Vec<String>) + Send + Sync>) {
        let _ = self.warm.set(warm);
    }

    /// A work began: its «…: готово.» is synthesized ahead, so the line starts at once when it is ready.
    pub(super) fn warm_ready(&self, work: &Work) {
        if let Some(warm) = self.warm.get() {
            let pack = pack::of(self.lang(work.device()));
            warm(vec![pack::fill(pack.work.say.ready_line, &[("title", &work.title)])]);
        }
    }

    /// The language of a device's character, once the personas stand.
    pub fn set_langs(&self, langs: Langs) {
        let _ = self.langs.set(langs);
    }

    /// The language a device's character speaks; Russian until the personas are known.
    pub fn lang(&self, device: &str) -> Lang {
        self.langs.get().map(|langs| langs(device)).unwrap_or(Lang::Ru)
    }

    /// The work a device's phrase began or added to since `since` (ms since the epoch), if any.
    pub fn touched_since(&self, device: &str, since: u64) -> Option<String> {
        let touched = self.touched.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        touched.get(device).filter(|(_, at)| *at >= since).map(|(work, _)| work.clone())
    }

    /// A work begins on its executor now — at once, or out of the queue: the thing it makes, the panel told.
    pub(super) fn launch(&self, work: Work) -> Result<Work, super::work_tools::Refusal> {
        use super::work_tools::Refusal;
        let executor = self.executor(&work.executor).ok_or_else(|| Refusal::Failed("its executor is gone".into()))?;
        // Going before the executor begins: its first report («a run began») must find this work, not make another
        // one for the same thing — a work out of the queue once got its «готово» taken by such a twin.
        let now = now_ms();
        let work = self
            .works
            .update(&work.id, |work| {
                work.state = WorkState::Working;
                work.started_at = now;
            })
            .unwrap_or(work);
        let started = match executor.start(&work) {
            Ok(started) => started,
            Err(why) => {
                self.works.update(&work.id, |work| work.state = WorkState::Failed);
                return Err(Refusal::Failed(why));
            }
        };
        let work = self
            .works
            .update(&work.id, |work| {
                work.target = Some(started.target.clone());
                work.title = started.title.clone();
            })
            .unwrap_or(work);
        self.told(&work, "started");
        self.warm_ready(&work);
        tracing::info!(target: "talk", "work {} on {}: {} {}", work.id, work.device(), work.executor, started.target);
        Ok(work)
    }

    /// «Делаю: …» on a device: the first time a day with how to add on the way, then short — every time it is long.
    pub(super) fn started_words(&self, device: &str) -> &'static str {
        let say = &pack::of(self.lang(device)).work.say;
        let today = now_ms() / (24 * 60 * 60_000);
        let mut hinted = self.hinted.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if hinted.insert(device.to_string(), today) == Some(today) {
            say.started
        } else {
            say.started_first
        }
    }

    /// Which version of its thing a new work makes: the works of that thing so far, this one too — and one more when
    /// the thing was there before any of them (an app made before the works were kept, a built-in one's copy).
    pub(super) fn version_of(&self, work: &Work) -> usize {
        let mut made: Vec<Work> = self
            .works
            .all(None, None)
            .into_iter()
            .filter(|known| known.executor == work.executor && known.target.is_some() && known.target == work.target)
            .collect();
        made.sort_by_key(|known| (known.started_at, known.id.parse::<u64>().unwrap_or(0)));
        let before = made.first().is_some_and(|first| first.again);
        made.len().max(1) + usize::from(before)
    }

    /// A work waits for its turn (the limits held it): it starts by itself, in `wait` ms or when a place is free.
    pub(super) fn enqueue(&self, work: &Work, wait: Option<u64>) {
        if let Some(work) = self.works.update(&work.id, |work| work.state = WorkState::Queued) {
            self.told(&work, "queued");
        }
        if let Some(wait) = wait {
            self.later(wait);
        }
        // A place may have freed between the limits' «no» and now: the end that moved the queue did not see this one.
        self.run_queue();
    }

    /// The queue moves: each work waiting starts when the limits let it — when a work ends (two at once), or once
    /// its device's two minutes passed (a new one by voice).
    pub(super) fn run_queue(&self) {
        let mut waiting = self.works.all(None, Some(WorkState::Queued));
        waiting.sort_by_key(|work| (work.started_at, work.id.parse::<u64>().unwrap_or(0)));
        for work in waiting {
            match self.works.may_start_kind(&work.kind, &work.origin, work.again, now_ms()) {
                Ok(()) => {
                    let id = work.id.clone();
                    if let Err(refusal) = self.launch(work) {
                        tracing::warn!(target: "talk", "work {id} could not leave the queue: {refusal:?}");
                    }
                }
                Err(super::works::Refused::TooSoon(wait)) => self.later(wait),
                // Two at once: a work's end moves the queue.
                Err(_) => {}
            }
        }
    }

    /// The queue moves again in `wait` ms.
    fn later(&self, wait: u64) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
        let me = self.me.clone();
        handle.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(wait + 50)).await;
            if let Some(layer) = me.upgrade() {
                layer.run_queue();
            }
        });
    }

    /// A thing changed where it lives — told by its id, never its name (an app removed, a draft deleted): its works
    /// learn what is left. On the desk — as they were; only its draft — taken off the desk; nothing — gone, and a work
    /// going on it stops. On 15.11 a deleted widget stayed «на столе» for the brain and was still found by its name.
    pub fn recheck(&self, target: &str) {
        let works: Vec<Work> =
            self.works.all(None, None).into_iter().filter(|work| work.target.as_deref() == Some(target)).collect();
        let Some(executor) = works.first().and_then(|work| self.executor(&work.executor)) else { return };
        let presence = executor.presence(target);
        for work in works {
            let end = match presence {
                Presence::Gone if !work.removed() => End::Removed,
                Presence::Draft if work.result.as_ref().is_some_and(|result| result.end == Some(End::Desk)) => {
                    End::Parked
                }
                _ => continue,
            };
            let now = now_ms();
            let changed = self.works.update(&work.id, |work| {
                let line = work.result.take().map(|result| result.line).unwrap_or_default();
                work.result = Some(WorkResult { line, end: Some(end), ..WorkResult::default() });
                work.question = None;
                if work.going() || work.state.queued() {
                    work.state = WorkState::Stopped;
                } else if matches!(work.state, WorkState::Ready | WorkState::Unfinished) {
                    work.state = WorkState::Done;
                }
                work.ended_at.get_or_insert(now);
            });
            if let Some(work) = changed {
                tracing::info!(target: "talk", "work {}: its thing {target} is {presence:?}", work.id);
                self.told(&work, if end == End::Removed { "removed" } else { "decided" });
            }
        }
    }

    /// The gateway tells what was removed, by the id: the works of that thing learn it (`recheck`).
    pub fn follow_removals(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        self.events.on("talk.removed", move |event| {
            let Some(layer) = me.upgrade() else { return };
            match event.as_ref() {
                GatewayEvent::WorkshopChanged { draft, change } if change == "deleted" => layer.recheck(draft),
                GatewayEvent::PackagesChanged { package_id, change, .. } if change == "removed" => {
                    layer.recheck(package_id)
                }
                _ => {}
            }
        });
    }

    /// At the gateway's start: what was removed while it was off, by the ids of the things the works remember.
    pub fn reconcile(&self) {
        let mut targets: Vec<String> = self
            .works
            .all(None, None)
            .into_iter()
            .filter(|work| !work.removed())
            .filter_map(|work| work.target)
            .collect();
        targets.sort();
        targets.dedup();
        for target in targets {
            self.recheck(&target);
        }
    }

    /// The works going on in their records whose executor has been at nothing on their thing for a minute: their run
    /// ended and the end was lost on the way. They are interrupted now — a work that ended never stays «идёт» (19.11:
    /// «Bitcoin rate · начинает · идёт 21 мин» twenty minutes after its run). Read with the works, so what the page and
    /// «Ждёт вас» show is what is. An executor that cannot tell (`running` is None) is believed.
    pub fn settle(&self) {
        self.settle_at(now_ms());
    }

    fn settle_at(&self, now: u64) {
        for work in self.works.all(None, None) {
            if !work.going() || now.saturating_sub(work.updated_at) < SETTLE_AFTER_MS {
                continue;
            }
            let Some(executor) = self.executor(&work.executor) else { continue };
            if executor.running(&work) != Some(false) {
                continue;
            }
            if let Some(work) = self.works.update(&work.id, |work| work.state = WorkState::Interrupted) {
                tracing::warn!(target: "talk", "work {}: going in its record, its executor at nothing on it: interrupted", work.id);
                self.run_queue();
                self.told(&work, "interrupted");
            }
        }
    }

    /// The reader of the phrases about the works, once the dialog stands.
    pub fn set_reader(&self, reader: Arc<super::reader::Reader>) {
        let _ = self.reader.set(reader);
    }

    /// The reader, when there is one.
    pub fn reader(&self) -> Option<Arc<super::reader::Reader>> {
        self.reader.get().cloned()
    }

    /// A device's phrase acted on a work now: the history keeps that work's line for the phrase (`fact_since`).
    pub(super) fn acted_on(&self, device: &str, work: &str) {
        let mut acted = self.acted.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        acted.insert(device.to_string(), (work.to_string(), now_ms()));
    }

    /// What the history keeps of the layer's phrase said on a device since `since` (ms since the epoch): the line of the
    /// work it acted on, in brackets (now.rs, `fact`). None — the phrase acted on no work (a refusal, the door): the
    /// history keeps the phrase.
    pub fn fact_since(&self, device: &str, since: u64) -> Option<String> {
        let id = {
            let acted = self.acted.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            acted.get(device).filter(|(_, at)| *at >= since).map(|(work, _)| work.clone())
        }?;
        let work = self.works.get(&id)?;
        let doing = self.executor(&work.executor).and_then(|executor| executor.status(&work));
        Some(now::fact(pack::of(self.lang(device)), &work, now_ms(), doing))
    }

    /// The computer said a work's line to its end (the panel's voice, «Готово»): said, and its question is open for
    /// the answer the panel listens for.
    pub fn said_on(&self, id: &str, device: &str) -> Option<Work> {
        let work = self.works.update(id, |work| work.said_at = Some(now_ms()))?;
        self.ask_aloud(&work, device, false);
        self.told(&work, "said");
        self.works.get(id)
    }

    /// A work's question was said aloud on a device: it waits for its answer there, and its record says when.
    pub(super) fn ask_aloud(&self, work: &Work, device: &str, again: bool) {
        let Some(kind) = work.question.as_ref().and_then(|question| question.kind) else { return };
        self.asked.ask(device, &work.id, kind, again);
        self.works.update(&work.id, |work| {
            if let Some(question) = work.question.as_mut() {
                question.asked_at = Some(now_ms());
                question.outcome = None;
            }
        });
    }

    /// How the asking of a work's question ended: the record keeps it, the panel hears it.
    pub(super) fn ended_asking(&self, id: &str, outcome: Outcome) {
        if id.is_empty() {
            return;
        }
        let Some(work) = self.works.update(id, |work| {
            if let Some(question) = work.question.as_mut() {
                question.outcome = Some(outcome);
            }
        }) else {
            return;
        };
        if work.question.is_some() {
            self.told(&work, outcome.as_str());
        }
    }

    /// A device's question ended without an answer: nothing is done — a work's question waits, to be asked once more.
    fn silent(&self, device: &str, question: &super::asked::Question) {
        tracing::info!(target: "talk", "the question on {device} ended silent: {:?} {}", question.kind, question.work);
        if question.kind != super::asked::Kind::Talk {
            self.ended_asking(&question.work, Outcome::Silent);
        }
    }

    /// The works whose question waits for the person: ready or not finished, asked, neither answered nor declined —
    /// «Ждёт вас»; and a work in a folder going on that a plan's limit holds («подождать или остановить?»).
    pub fn waiting(&self) -> Vec<Work> {
        let asks = |work: &Work| {
            let question = work.question.as_ref().filter(|question| question.waits());
            match question.and_then(|question| question.kind) {
                Some(super::asked::Kind::Wait) => work.going(),
                Some(_) => work.awaiting(),
                None => false,
            }
        };
        self.works.all(None, None).into_iter().filter(asks).collect()
    }

    /// After a device's talk: the question of its work that ended silent, asked once more — once ever. Its words, and
    /// it waits for the answer again.
    pub fn remind(&self, device: &str) -> Option<String> {
        let work = self.works.all(Some(device), None).into_iter().find(|work| {
            work.awaiting()
                && work
                    .question
                    .as_ref()
                    .is_some_and(|question| question.outcome == Some(Outcome::Silent) && !question.reminded)
        })?;
        let line = work.question.as_ref().map(|question| question.text.clone()).unwrap_or_default();
        let ask = line.rsplit_once(". ").map(|(_, ask)| ask.trim().to_string()).unwrap_or_default();
        if ask.is_empty() {
            return None;
        }
        let mut chars = ask.chars();
        let ask: String = chars.next().map(|first| first.to_lowercase().chain(chars).collect()).unwrap_or_default();
        let work = self.works.update(&work.id, |work| {
            if let Some(question) = work.question.as_mut() {
                question.reminded = true;
            }
        })?;
        self.ask_aloud(&work, device, false);
        self.told(&work, "reminded");
        let say = &pack::of(self.lang(device)).work.say;
        Some(pack::fill(say.remind, &[("title", &work.title), ("ask", &ask)]))
    }

    /// Who keeps a device's conversation past its pause: the dialog's history.
    pub fn set_hold(&self, hold: Hold) {
        let _ = self.hold.set(hold);
    }

    /// The panel hears a work change: `started`, `working`, `ready`, `failed`, `decided`… The device's conversation is
    /// remembered while the work goes on and half an hour after its last change.
    pub(super) fn told(&self, work: &Work, change: &str) {
        if let Some(hold) = self.hold.get() {
            hold(work.device(), if work.going() { u64::MAX } else { now_ms() + REMEMBERED_MS });
        }
        if matches!(change, "started" | "note") {
            let mut touched = self.touched.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            touched.insert(work.device().to_string(), (work.id.clone(), now_ms()));
            drop(touched);
            self.acted_on(work.device(), &work.id);
        }
        self.events.emit(GatewayEvent::WorkChanged {
            work: work.id.clone(),
            device: work.device().to_string(),
            change: change.to_string(),
        });
    }

    /// An executor's message.
    fn heard(self: &Arc<Self>, executor: &'static str, report: Report) {
        match report {
            Report::Working(began) => self.began(executor, began),
            Report::Ended { target, state, line, title } => {
                // A thing has one run at a time: when it ends, no work on it goes on. The newest one going takes the
                // end; an older one still going on the same thing — its words went into this run, or its own end once
                // reached a newer record — ends with it, quietly. On 19.11 such a twin hung «идёт» for twenty minutes
                // after its run had ended, and «Остановить» could not end it.
                let going: Vec<Work> = self.works.of_thing(executor, &target).into_iter().filter(Work::going).collect();
                let Some((work, twins)) = going.split_first() else { return };
                for twin in twins {
                    let closed = self.works.update(&twin.id, |twin| {
                        twin.state = state;
                        if let Some(title) = &title {
                            twin.title = title.clone();
                        }
                    });
                    if let Some(twin) = closed {
                        tracing::info!(target: "talk", "work {}: its thing {target} ended its run with work {}", twin.id, work.id);
                        self.told(&twin, "closed");
                    }
                }
                let changed = self.works.update(&work.id, |work| {
                    work.state = state;
                    if let Some(line) = line {
                        work.result = Some(WorkResult { line, end: None, ..WorkResult::default() });
                    }
                    // The thing is called what its maker named it.
                    if let Some(title) = title {
                        work.title = title;
                    }
                });
                let Some(work) = changed else { return };
                // A place is free: the queue moves.
                self.run_queue();
                // A run that ended with something to say: its line, its question, said where it was asked.
                if matches!(state, WorkState::Ready | WorkState::Unfinished | WorkState::Failed) {
                    self.finished(work);
                } else {
                    self.told(&work, state.as_str());
                }
            }
            Report::Read { target } => {
                let Some(work) = self.works.going_on(executor, &target) else { return };
                let now = now_ms();
                let changed = self.works.update(&work.id, |work| {
                    for note in work.notes.iter_mut().filter(|note| note.read_at.is_none()) {
                        note.read_at = Some(now);
                    }
                });
                if let Some(work) = changed {
                    self.told(&work, "note-read");
                }
            }
            Report::Decided { target, end } => {
                // What became of the thing answers every work of it that waits: the newest one, and an older version
                // still asking «Доделать?» or «На стол?» — on 19.11 one such stayed «Ждёт вас · 1» after the thing
                // was on the desk, with nothing to show behind it.
                let works = self.works.of_thing(executor, &target);
                for (at, work) in works.iter().enumerate() {
                    if at > 0 && !work.awaiting() {
                        continue;
                    }
                    let changed = self.works.update(&work.id, |work| {
                        let line = work.result.take().map(|result| result.line).unwrap_or_default();
                        work.result = Some(WorkResult { line, end: Some(end), ..WorkResult::default() });
                        work.question = None;
                        if !work.going() {
                            work.state = WorkState::Done;
                        }
                    });
                    if let Some(work) = changed {
                        self.told(&work, "decided");
                    }
                }
            }
        }
    }

    /// A run began on a thing: the work the layer started for it, the one going on with it, or a new record — a
    /// run started elsewhere (the panel's workshop) is a work too.
    fn began(&self, executor: &'static str, began: Began) {
        let known = began
            .work
            .as_deref()
            .and_then(|id| self.works.get(id))
            .filter(|work| work.going())
            .or_else(|| self.works.going_on(executor, &began.target));
        if let Some(work) = known {
            let target = began.target.clone();
            if let Some(work) = self.works.update(&work.id, |work| {
                work.target = Some(target);
                if work.title.trim().is_empty() {
                    work.title = began.title.clone();
                }
            }) {
                self.told(&work, "working");
            }
            return;
        }
        let again = self.works.of_target(executor, &began.target).is_some();
        let work = self.works.create(NewWork {
            kind: APP,
            face: began.face,
            executor: executor.to_string(),
            target: Some(began.target),
            title: began.title,
            brief: began.brief,
            origin: began.origin,
            again,
        });
        self.supersede(&work);
        self.told(&work, "started");
    }

    /// A new version of a thing began: the questions its older works still ask («Доделать?», «На стол или
    /// примерить?») are answered by it — they leave «Ждёт вас». On 19.11 «Доделать» began the new version, and the old
    /// «не доделано» kept asking with nothing behind it.
    pub(super) fn supersede(&self, newer: &Work) {
        let Some(target) = newer.target.as_deref() else { return };
        for older in self.works.of_thing(&newer.executor, target) {
            if older.id == newer.id || older.question.is_none() || !older.awaiting() {
                continue;
            }
            if let Some(older) = self.works.update(&older.id, |older| older.question = None) {
                self.told(&older, "answered");
            }
        }
    }

    /// A round of the model on an executor's thing (the workshop's agent on its draft): what it spent, summed into the
    /// work going on with it.
    pub fn spent(&self, executor: &str, target: &str, usage: &crate::dialog::llm::LlmUsage) {
        let Some(work) = self.works.going_on(executor, target) else { return };
        self.works.update(&work.id, |work| {
            work.tokens.input += usage.input;
            work.tokens.output += usage.output;
            work.tokens.cache_read += usage.cache_read.unwrap_or(0);
            work.tokens.cache_write += usage.cache_write.unwrap_or(0);
            work.tokens.rounds += 1;
        });
    }

    /// The work of an executor's thing (the workshop's agent on its draft), going or just ended: what a deal belongs
    /// to — the run's answer may end after its work did.
    pub fn work_of(&self, executor: &str, target: &str) -> Option<String> {
        self.works.of_target(executor, target).map(|work| work.id)
    }

    /// The block «Сейчас» for a device, in the language given (the character's).
    pub fn now(&self, device: &str, lang: Lang) -> String {
        let works = self.works.all(Some(device), None);
        let doing = |work: &Work| self.executor(&work.executor).and_then(|executor| executor.status(work));
        let words = pack::of(lang).work;
        let desk: Vec<String> = self
            .desk
            .get()
            .map(|desk| desk())
            .unwrap_or_default()
            .into_iter()
            .map(|app| {
                let turbo = if app.turbo { words.turbo } else { "" };
                format!("{} [{}]{turbo}", app.title, app.id)
            })
            .collect();
        let door = self.door.is_open(device);
        let sources = Sources { works: &works, doing: &doing, desk: &desk, door, now: now_ms() };
        now::block(pack::of(lang), &sources)
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{Config, Env};
    use crate::plugins::workshop::activity::By;
    use crate::plugins::workshop::meta::{DraftMeta, Face, Stage};
    use crate::talk::works::WorkState;

    fn env(dir: &std::path::Path) -> Env {
        let text = |sub: &str| dir.join(sub).to_string_lossy().into_owned();
        Env::from_pairs([
            ("STATE_DIR", text("state")),
            ("DIALOG_PROVIDER", "echo".into()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ])
    }

    /// The clean-path check of 19.11: the block said «На столе: …» of every package that was on, and the brain told
    /// the person a widget was on their desk while nobody saw it there. It names the apps that are on — no claim of
    /// where their widgets stand — and the ones whose code runs on the Турбо, so a call that waits for it is no surprise.
    #[test]
    fn the_block_names_the_apps_that_are_on_and_which_run_on_the_turbo() {
        let layer = super::TalkLayer::new(None, crate::core::events::EventBus::new());
        layer.set_desk(std::sync::Arc::new(|| {
            vec![
                super::DeskApp { id: "weather".into(), title: "Погода".into(), turbo: false },
                super::DeskApp { id: "youtube".into(), title: "YouTube".into(), turbo: true },
            ]
        }));
        layer.door.open("kitchen");
        let ru = layer.now("kitchen", crate::memory::text::Lang::Ru);
        assert!(ru.contains("- Включены приложения: Погода [weather], YouTube [youtube] — на Турбо."), "{ru}");
        assert!(!ru.contains("На столе"), "{ru}");
        let en = layer.now("kitchen", crate::memory::text::Lang::En);
        assert!(en.contains("- Apps switched on: Погода [weather], YouTube [youtube] — on the Turbo."), "{en}");
    }

    /// A run of the agent is a work whoever started it; a restart while it goes on interrupts the work and the card —
    /// nothing restarts by itself.
    #[tokio::test]
    async fn follows_the_cards_and_a_restart_interrupts_the_work_going_on() {
        let dir = tempfile::tempdir().unwrap();
        let app = crate::app::App::build(Config::for_tests(&env(dir.path()))).unwrap();
        let mut heard = app.events.subscribe();
        app.workshop.create("golos-vidzhet-1", Some("widget"), Some("Испанский"), None).unwrap();
        let card = DraftMeta {
            by: By::Voice,
            who: Some("kitchen".into()),
            asked: "сделай виджет: испанские слова дня".into(),
            kind: Face::Widget,
            stage: Stage::Working,
            updated: 0,
            note: None,
            kept: true,
            work: None,
            unchanged: false,
        };
        app.workshop.keep_card("golos-vidzhet-1", card, Some("made")).unwrap();
        let work = app.talk.works().of_target("workshop", "golos-vidzhet-1").expect("a work for the run");
        assert_eq!((work.state, work.title.as_str(), work.device()), (WorkState::Working, "Испанский", "kitchen"));
        assert_eq!(work.origin.activator, crate::talk::Activator::Word);
        let mut changes = Vec::new();
        while let Ok(event) = heard.try_recv() {
            if let crate::core::events::GatewayEvent::WorkChanged { work, device, change } = event.as_ref() {
                changes.push(format!("{work}:{device}:{change}"));
            }
        }
        assert_eq!(changes, ["1:kitchen:started"]);
        let now = app.talk.now("kitchen", crate::memory::text::Lang::Ru);
        assert!(now.contains("- В работе [р1]: виджет «Испанский»"), "{now}");
        assert_eq!(app.talk.now("hall", crate::memory::text::Lang::Ru), "", "another device knows nothing of it");
        drop(app);

        let app = crate::app::App::build(Config::for_tests(&env(dir.path()))).unwrap();
        let work = app.talk.works().get(&work.id).unwrap();
        assert_eq!(work.state, WorkState::Interrupted);
        assert_eq!(app.workshop.card("golos-vidzhet-1").stage, Stage::Interrupted);
        let now = app.talk.now("kitchen", crate::memory::text::Lang::Ru);
        assert!(now.contains("- Прервано [р1]: виджет «Испанский»"), "{now}");
    }

    /// «На стол или примерить?» said and nobody answered in its time: nothing is done, the work waits («Ждёт вас»), and
    /// the question is asked once more after the device's next talk — once, never twice. «Нет» is a «not now».
    #[tokio::test(start_paused = true)]
    async fn a_question_nobody_answered_waits_and_is_asked_once_more_once() {
        use crate::talk::asked::{Kind, ANSWER_FOR};
        use crate::talk::works::{NewWork, Outcome, Question, WorkResult};
        let layer = super::TalkLayer::new(None, crate::core::events::EventBus::new());
        let ready = |title: &str| {
            let work = layer.works.create(NewWork {
                kind: super::APP,
                face: None,
                executor: "workshop".into(),
                target: Some(title.to_lowercase()),
                title: title.into(),
                brief: "часы".into(),
                origin: crate::talk::Origin::station("kitchen", true),
                again: false,
            });
            let line = format!("{title}: готово. На стол или примерить?");
            layer
                .works
                .update(&work.id, |work| {
                    work.state = WorkState::Ready;
                    work.result = Some(WorkResult { line: line.clone(), end: None, ..WorkResult::default() });
                    work.question = Some(Question::new(line, 0, Kind::Ready));
                })
                .unwrap()
        };
        let clock = ready("Часы");
        assert!(layer.waiting().iter().any(|work| work.id == clock.id), "ready with a question: it waits");
        assert_eq!(layer.remind("kitchen"), None, "not asked yet — nothing to remind of");
        layer.said_on(&clock.id, "kitchen");
        assert!(layer.asked.open("kitchen").is_some());
        tokio::time::sleep(ANSWER_FOR + std::time::Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        let question = layer.works.get(&clock.id).unwrap().question.unwrap();
        assert_eq!(question.outcome, Some(Outcome::Silent));
        assert!(layer.waiting().iter().any(|work| work.id == clock.id), "silent: it still waits");
        assert_eq!(layer.remind("hall").as_deref(), None, "another device was not asked");
        assert_eq!(layer.remind("kitchen").as_deref(), Some("Часы ещё ждёт: на стол или примерить?"));
        assert!(layer.asked.open("kitchen").is_some(), "the reminder asks again: the answer is taken");
        tokio::time::sleep(ANSWER_FOR + std::time::Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        assert_eq!(layer.remind("kitchen"), None, "never a second time");

        let weather = ready("Погода");
        layer.said_on(&weather.id, "kitchen");
        layer.ended_asking(&weather.id, Outcome::Declined);
        assert!(!layer.waiting().iter().any(|work| work.id == weather.id), "declined: it does not wait for the person");
    }

    /// The hand check of 19.11 on the layer: a run's end reaches every work going on its thing, «Остановить» ends a
    /// work its executor is no longer at, a work whose run ended unheard is settled, and what became of a thing — or
    /// its new version — answers the questions of its older works, so «Ждёт вас» counts only what really waits.
    mod hand_check_1911 {
        use std::sync::Arc;

        use crate::core::events::EventBus;
        use crate::talk::asked::Kind;
        use crate::talk::executor::{fake::Shelf, Presence, Report};
        use crate::talk::layer::{TalkLayer, SETTLE_AFTER_MS};
        use crate::talk::origin::{Activator, Origin};
        use crate::talk::works::{End, NewWork, Question, Work, WorkResult, WorkState, APP};
        use crate::utils::time::now_ms;

        fn layer() -> (Arc<TalkLayer>, Arc<Shelf>) {
            let layer = TalkLayer::new(None, EventBus::new());
            let shelf = Shelf::with(&[("btc", "Bitcoin rate", Presence::Draft)]);
            layer.add_executor(shelf.clone());
            (layer, shelf)
        }

        /// A work of the thing `btc` as the panel's «Доделать» makes it: going until its run ends.
        fn going(layer: &TalkLayer) -> Work {
            layer.works.create(NewWork {
                kind: APP,
                face: None,
                executor: "workshop".into(),
                target: Some("btc".into()),
                title: "Bitcoin rate".into(),
                brief: "Доделай начатое и проверь, что всё работает.".into(),
                origin: Origin::computer(Activator::Text),
                again: true,
            })
        }

        /// A work of `btc` that ended «не доделано» and asks «Доделать?».
        fn unfinished(layer: &TalkLayer) -> Work {
            let work = going(layer);
            layer
                .works
                .update(&work.id, |work| {
                    work.state = WorkState::Unfinished;
                    let line = "Bitcoin rate: не доделано. Доделать?".to_string();
                    work.result = Some(WorkResult { line: line.clone(), end: None, ..WorkResult::default() });
                    work.question = Some(Question::new(line, 0, Kind::Unfinished));
                })
                .unwrap()
        }

        fn state(layer: &TalkLayer, work: &Work) -> WorkState {
            layer.works.get(&work.id).unwrap().state
        }

        #[tokio::test]
        async fn a_run_that_ends_ends_every_work_going_on_its_thing() {
            let (layer, _) = layer();
            let older = going(&layer);
            std::thread::sleep(std::time::Duration::from_millis(3));
            let newer = going(&layer);
            let ended = Report::Ended { target: "btc".into(), state: WorkState::Ready, line: None, title: None };
            layer.heard("workshop", ended);
            assert_eq!(state(&layer, &newer), WorkState::Ready, "the newest going takes the end");
            assert_eq!(state(&layer, &older), WorkState::Ready, "its twin ends with it, not «идёт» forever");
            assert_eq!(layer.works.going(), 0);

            // A newer record that ended no longer hides an older one still going: the next run's end finds it.
            let left = going(&layer);
            std::thread::sleep(std::time::Duration::from_millis(3));
            let done = going(&layer);
            layer.works.update(&done.id, |work| work.state = WorkState::Failed);
            let ended = Report::Ended { target: "btc".into(), state: WorkState::Unfinished, line: None, title: None };
            layer.heard("workshop", ended);
            assert_eq!(state(&layer, &left), WorkState::Unfinished, "the end reached the work going");
            assert_eq!(state(&layer, &done), WorkState::Failed, "the ended one is left as it was");
        }

        #[tokio::test]
        async fn stop_ends_a_work_its_executor_is_no_longer_at() {
            let (layer, _) = layer();
            let work = going(&layer);
            let label = format!("р{}", work.id);
            let said = layer.act(&Origin::computer(Activator::Text), "stop", Some(&label)).await.unwrap();
            assert!(!said.contains("уже не в работе"), "{said}");
            assert_eq!(state(&layer, &work), WorkState::Stopped);
            assert!(layer.works.get(&work.id).unwrap().ended_at.is_some());
            let again = layer.act(&Origin::computer(Activator::Text), "stop", Some(&label)).await.unwrap();
            assert!(again.contains("уже не в работе"), "stopped already: {again}");
        }

        #[tokio::test]
        async fn a_work_whose_run_ended_unheard_is_settled_and_one_at_work_is_not() {
            let (layer, shelf) = layer();
            let work = going(&layer);
            let later = now_ms() + SETTLE_AFTER_MS + 1000;
            layer.settle_at(later);
            assert_eq!(state(&layer, &work), WorkState::Working, "the executor cannot tell: the record is believed");
            shelf.at.lock().unwrap().insert("btc".into(), true);
            layer.settle_at(later);
            assert_eq!(state(&layer, &work), WorkState::Working, "at work on it: going");
            shelf.at.lock().unwrap().insert("btc".into(), false);
            layer.settle_at(now_ms());
            assert_eq!(state(&layer, &work), WorkState::Working, "just begun: its run may not show yet");
            layer.settle_at(later);
            assert_eq!(state(&layer, &work), WorkState::Interrupted, "at nothing on it for a minute: not «идёт»");
        }

        #[tokio::test]
        async fn a_new_version_answers_the_old_question_and_the_desk_answers_them_all() {
            let (layer, _) = layer();
            let old = unfinished(&layer);
            assert_eq!(layer.waiting().len(), 1, "«Доделать?» waits");
            std::thread::sleep(std::time::Duration::from_millis(3));
            let label = format!("р{}", old.id);
            layer.add(Origin::computer(Activator::Text), "Доделай начатое.", Some(&label), false).await.unwrap();
            assert!(layer.waiting().is_empty(), "«Доделать» began the new version: the old one no longer waits");
            assert_eq!(layer.works.going(), 1);

            // An older version asking, a newer one put on the desk: the desk answers both.
            let (layer, _) = self::layer();
            let older = unfinished(&layer);
            std::thread::sleep(std::time::Duration::from_millis(3));
            let newer = going(&layer);
            layer.works.update(&newer.id, |work| work.state = WorkState::Ready);
            layer.heard("workshop", Report::Decided { target: "btc".into(), end: End::Desk });
            assert!(layer.waiting().is_empty(), "nothing behind «Ждёт вас»");
            let older = layer.works.get(&older.id).unwrap();
            assert_eq!((older.state, older.result.and_then(|result| result.end)), (WorkState::Done, Some(End::Desk)));
        }
    }
}
