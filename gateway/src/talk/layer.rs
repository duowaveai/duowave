//! `TalkLayer` (docs/plans/audio-interface.md, «Слой разговора»): one service per gateway, its state per device —
//! the records of works, what the brain knows now. Executors report to it; the brain's request reads its block; the
//! panel reads its works. (`Talk` in this module is the answer on its way — the mic mode's name.)

use std::sync::{Arc, OnceLock, RwLock, Weak};

use super::executor::{Began, Executor, Presence, Report};
use super::now::{self, Sources};
use super::pack;
use super::works::{End, NewWork, Work, WorkResult, WorkState, Works, APP};
use crate::core::events::{EventBus, GatewayEvent};
use crate::memory::text::Lang;
use crate::storage::Db;
use crate::utils::time::now_ms;

/// The apps on the desk, for the block: their ids and titles — «Погода [weather]», so a thing is named by its id.
pub type Desk = Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>;

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
}

/// A device's conversation is remembered this long after its work last changed.
pub const REMEMBERED_MS: u64 = 30 * 60_000;

impl TalkLayer {
    /// The layer over the works the table keeps: what went on when the gateway stopped is interrupted.
    pub fn new(db: Option<Arc<Db>>, events: EventBus) -> Arc<TalkLayer> {
        Arc::new_cyclic(|me| TalkLayer {
            me: me.clone(),
            works: Works::open(db),
            door: super::door::Door::default(),
            asked: super::asked::Asked::default(),
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
        })
    }

    pub fn works(&self) -> &Works {
        &self.works
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
            match self.works.may_start(&work.origin, work.again, now_ms()) {
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
                work.result = Some(WorkResult { line, end: Some(end) });
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
        if let Some(kind) = work.question.as_ref().and_then(|question| question.kind) {
            self.asked.ask(device, id, kind, false);
        }
        self.told(&work, "said");
        Some(work)
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
                let Some(work) = self.works.of_target(executor, &target).filter(|work| work.going()) else { return };
                let changed = self.works.update(&work.id, |work| {
                    work.state = state;
                    if let Some(line) = line {
                        work.result = Some(WorkResult { line, end: None });
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
                let Some(work) = self.works.of_target(executor, &target).filter(|work| work.going()) else { return };
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
                let Some(work) = self.works.of_target(executor, &target) else { return };
                let changed = self.works.update(&work.id, |work| {
                    let line = work.result.take().map(|result| result.line).unwrap_or_default();
                    work.result = Some(WorkResult { line, end: Some(end) });
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

    /// A run began on a thing: the work the layer started for it, the one going on with it, or a new record — a
    /// run started elsewhere (the panel's workshop) is a work too.
    fn began(&self, executor: &'static str, began: Began) {
        let known = began
            .work
            .as_deref()
            .and_then(|id| self.works.get(id))
            .filter(|work| work.going())
            .or_else(|| self.works.of_target(executor, &began.target).filter(|work| work.going()));
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
        self.told(&work, "started");
    }

    /// A round of the model on an executor's thing (the workshop's agent on its draft): what it spent, summed into the
    /// work going on with it.
    pub fn spent(&self, executor: &str, target: &str, usage: &crate::dialog::llm::LlmUsage) {
        let Some(work) = self.works.of_target(executor, target).filter(|work| work.going()) else { return };
        self.works.update(&work.id, |work| {
            work.tokens.input += usage.input;
            work.tokens.output += usage.output;
            work.tokens.cache_read += usage.cache_read.unwrap_or(0);
            work.tokens.cache_write += usage.cache_write.unwrap_or(0);
            work.tokens.rounds += 1;
        });
    }

    /// The block «Сейчас» for a device, in the language given (the character's).
    pub fn now(&self, device: &str, lang: Lang) -> String {
        let works = self.works.all(Some(device), None);
        let doing = |work: &Work| self.executor(&work.executor).and_then(|executor| executor.status(work));
        let desk: Vec<String> = self
            .desk
            .get()
            .map(|desk| desk())
            .unwrap_or_default()
            .into_iter()
            .map(|(id, title)| format!("{title} [{id}]"))
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
}
