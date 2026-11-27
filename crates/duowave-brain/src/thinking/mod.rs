//! Thinking behind the door (docs/plans/thinking.md): `OurThinking`, our own implementation of the door's `Thinking`
//! (docs/plans/audio-thinking-contract.md). The audio side sends a phrase and news; Thinking answers with the door's
//! events and says its notices — whatever the device, which differs only in the phrase's data (`where`, `hands`,
//! `budget`). Nothing here is written for one device.
//!
//! A turn: the agent it is said to (§11), the fast route (fast.rs, §3), then the brain (turn.rs): the card (card.rs,
//! §4) and the rounds (rounds.rs, §8). Every part is a variant picked by the setting `thinking` (settings.rs): a change
//! later is a setting or a new variant, not a rebuild. Which Thinking stands behind the door is a setting too
//! (`implementation`): `switch.rs` picks ours or the talk layer's stand-in (`TalkThinking`) per phrase.
//!
//! - **`phrase`** opens a turn: `begun` comes first with the journal's request when the brain answers; then `say` for
//!   each phrase, `wait` while a tool works, `work` when the works layer touched one, `ask` for a question, and `end`.
//! - **`tell`**: `stop` cancels the turn; `delivered` marks a work's line said, or keeps it for the device's next turn;
//!   `no_answer` closes the question (the works layer reminds of it once, `on_silence`).
//! - **`notices`**: a work of the computer ready, or asking, while its last phrase was the computer's own ear's — a
//!   `notice`, with its question's `expects`; `withdraw` once it is decided, said or stopped elsewhere.

pub mod agent;
pub mod card;
pub mod catalog;
pub mod fast;
pub mod foreign;
pub mod hands;
pub mod line;
pub mod live;
pub mod more;
pub mod out;
pub mod panel;
pub mod rounds;
pub mod routes;
pub mod sense;
pub mod settings;
pub mod shadow;
pub mod stats;
pub mod switch;
pub mod turn;
pub mod usual;
pub mod words;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::channel::mpsc;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::core::events::GatewayEvent;
use crate::core::requests::RequestLines;
use crate::dialog::PersonaService;
use crate::door::{
    self, Delivery, Events, Expects, Hint, News, Notice, NoticeWhy, Notices, Outside, Phrase, Told, Unasked, Urgency,
};
use crate::settings::SettingsService;
use crate::talk::thinking::{turn_of, People};
use crate::talk::works::Work;
use crate::talk::{Parts, Why, COMPUTER};

pub use out::{Ended, Out};
pub use settings::{Settings, THINKING};
pub use switch::Switch;

/// The changes of a work after which its line is said.
const SAID_ON: &[&str] = &["ready", "unfinished", "failed", "done"];
/// The changes after which a waiting notice is no longer needed.
const WITHDRAWN_ON: &[&str] = &["said", "decided", "stopped", "answered", "declined", "removed"];

/// What Thinking works with: the talk's parts (the layer, the brain, the voice, the skills, MCP, the journal), the
/// characters, who speaks, and the setting `thinking`.
#[derive(Clone)]
pub struct Deps {
    pub parts: Parts,
    pub personas: Arc<PersonaService>,
    /// A person's id → who speaks.
    pub people: People,
    pub settings: Arc<SettingsService>,
    /// The home's devices and their hands, as the audio side tells them.
    pub devices: Option<Arc<dyn door::Devices>>,
    /// Where the turns' lines go: the journal (`requests.thinking`).
    pub lines: Option<Arc<dyn RequestLines>>,
    /// The apps' names for people, by their ids (the catalog's lines).
    pub titles: Option<catalog::AppTitles>,
    /// An installed character's title and persona text, by its id (the agent of a turn).
    pub characters: Option<agent::Characters>,
}

/// The last answered turn of a device, until what came next is known.
struct Last {
    request: String,
    line: line::Line,
    ended: Instant,
    text: String,
    asked: bool,
}

/// Our Thinking behind the door.
pub struct OurThinking {
    inner: Arc<Inner>,
}

struct Inner {
    deps: Deps,
    /// The turns going on: their stop, and their device.
    going: Mutex<HashMap<String, (CancellationToken, String)>>,
    /// A device → the work whose line it did not say: said again after its next turn, once.
    unsaid: Mutex<HashMap<String, String>>,
    listeners: Mutex<Vec<mpsc::UnboundedSender<Outside>>>,
    following: OnceLock<()>,
    /// The turns stopped: how much of their answer was heard.
    cuts: Mutex<HashMap<String, Option<u64>>>,
    /// A device → its last turn whose line waits for what came next.
    last: Mutex<HashMap<String, Last>>,
    /// The hands' calls waiting for `hands_done`, and the risky ones waiting for their «да».
    calls: Arc<hands::Calls>,
    pending: Arc<hands::Pending>,
    /// The skills' own dialogs going on, by device, and their hooks for the end of the speech, by turn.
    dialogs: Arc<hands::Dialogs>,
    spoken: hands::Spoken,
    /// The models' expected round times from the lines, and when they were counted.
    expected: Mutex<Option<(Instant, rounds::Expected)>>,
    /// «Чутьё» with Thinking's words steps (sense.rs).
    sense: crate::sense::Sense,
    /// The agents that spoke last on each device: a reply in the window is theirs.
    talks: agent::Talks,
}

/// How often the expected round times are counted again from the lines.
const EXPECTED_EVERY: Duration = Duration::from_secs(600);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl OurThinking {
    pub fn new(deps: Deps) -> OurThinking {
        let sense = sense::sense(&deps.parts.talk, &deps.parts.skills);
        OurThinking {
            inner: Arc::new(Inner {
                deps,
                sense,
                talks: agent::Talks::default(),
                going: Mutex::new(HashMap::new()),
                unsaid: Mutex::new(HashMap::new()),
                listeners: Mutex::new(Vec::new()),
                following: OnceLock::new(),
                cuts: Mutex::new(HashMap::new()),
                last: Mutex::new(HashMap::new()),
                calls: Arc::new(hands::Calls::default()),
                pending: Arc::new(hands::Pending::default()),
                dialogs: Arc::new(hands::Dialogs::default()),
                spoken: hands::Spoken::default(),
                expected: Mutex::new(None),
            }),
        }
    }

    /// The setting `thinking` as it stands now.
    pub fn settings(&self) -> Settings {
        Settings::of(&self.inner.deps.settings)
    }

    /// The turns going on now.
    pub fn going(&self) -> usize {
        lock(&self.inner.going).len()
    }

    /// The turns' lines the journal keeps, oldest first, as the query asks.
    pub fn lines(&self, query: &stats::Query) -> Vec<line::Line> {
        let Some(lines) = &self.inner.deps.lines else { return Vec::new() };
        lines
            .lines(query.from.unwrap_or(0), query.device.as_deref(), 20_000)
            .into_iter()
            .filter(|row| query.to.is_none_or(|to| row.at <= to))
            .filter_map(|row| serde_json::from_value::<line::Line>(row.line).ok())
            .filter(|line| stats::counts(line, query))
            .collect()
    }

    /// The numbers of the turns' lines, as the query asks.
    pub fn stats(&self, query: &stats::Query) -> stats::Stats {
        stats::of(&self.lines(query))
    }

    /// The card of a live session for a phrase (live.rs): the agent's prompt with the memory's block and the card's
    /// instructions, the card's tools, and how a call runs — the hands and works it sends gathered for the door.
    pub fn live_card(&self, phrase: &Phrase) -> live::LiveCard {
        let inner = &self.inner;
        let parts = &inner.deps.parts;
        let settings = Settings::of(&inner.deps.settings);
        let principal = (inner.deps.people)(phrase.who.person.as_deref());
        let agent = agent::pick(
            phrase,
            &settings,
            &inner.deps.settings,
            &inner.deps.personas,
            inner.deps.characters.as_ref(),
            &inner.talks,
        );
        let speaker = agent.id.clone().or_else(|| home_speaker(&inner.deps.personas));
        let turn = turn_of(phrase);
        let (sender, receiver) = mpsc::unbounded();
        let out = Out::new(sender, &phrase.turn, speaker.clone(), CancellationToken::new());
        let brain = crate::talk::Brain::of(parts);
        let trip = (parts.journal)(crate::core::requests::RequestStart {
            device: phrase.place.device.clone(),
            session: None,
            kind: crate::core::requests::RequestKind::Voice,
            text: Some(phrase.text.clone()),
            parent: None,
        });
        let (tool_events, _ignored) = tokio::sync::mpsc::unbounded_channel();
        let devices: Vec<door::Device> =
            inner.deps.devices.as_ref().map(|devices| devices.devices()).unwrap_or_default();
        let place = (!phrase.hands.is_empty()).then(|| {
            Arc::new(hands::Place {
                parts: parts.clone(),
                principal: principal.clone(),
                hands: hands::of_place(phrase, &devices, &principal),
                devices: devices.clone(),
                phrase: phrase.clone(),
                out: out.clone(),
                calls: inner.calls.clone(),
                pending: inner.pending.clone(),
                answer_ms: hands::ANSWER_MS,
                dialogs: inner.dialogs.clone(),
            })
        });
        let usual = usual::Usual::of(&inner.deps.settings);
        let device = phrase.place.device.clone();
        let lang = phrase.lang.clone().unwrap_or_else(|| parts.talk.lang(&device).as_str().to_string());
        let made = card::maker(settings::CardVariant::Card).make(card::Input {
            parts,
            principal: &principal,
            text: &turn.text,
            origin: turn.origin.clone().of(&principal),
            trip,
            events: tool_events,
            brain,
            hands: &phrase.hands,
            settings: &settings,
            device: &device,
            lang: &lang,
            usual: usual.top(&device, settings.card.caps.usual),
            used: usual.counts(&device),
            titles: inner.deps.titles.clone(),
            place,
            opened: None,
            delivery: settings::Delivery::Append,
            provider: None,
            picked: None,
        });
        let channel = match phrase.place.kind {
            door::Kind::Station | door::Kind::Box => crate::dialog::Channel::Station,
            _ => crate::dialog::Channel::Computer,
        };
        let context = crate::dialog::DialogContext {
            spoken: true,
            channel,
            device: matches!(phrase.place.kind, door::Kind::Station | door::Kind::Box).then(|| device.clone()),
            persona: agent.prompt.clone(),
            viewer: agent.own().then(|| agent.id.clone()).flatten(),
            // The device's time zone (a station's last request), else the home's.
            timezone: self
                .inner
                .deps
                .parts
                .skills
                .device_context(&device)
                .timezone
                .or_else(crate::dialog::llm::limits::home_timezone),
            ..crate::dialog::DialogContext::default()
        };
        let key = agent::history_key(&turn::history_key(phrase, &principal), &agent, &settings);
        let (tools, run, card_words) = match made.toolbox {
            Some(toolbox) => (toolbox.tools.clone(), toolbox.run.clone(), toolbox.instructions.clone()),
            None => (
                Vec::new(),
                Arc::new(|call: crate::dialog::llm::ToolCall, _| {
                    Box::pin(async move {
                        Ok(crate::dialog::ToolOutcome::error(format!("error: {} is not available", call.name)))
                    })
                        as futures::future::BoxFuture<'static, anyhow::Result<crate::dialog::ToolOutcome>>
                }) as crate::dialog::ToolRun,
                None,
            ),
        };
        let (system, tail) = parts.dialog.instructions(&key, &context, &tools, card_words.as_deref());
        live::LiveCard::new(format!("{system}\n\n{tail}"), tools, run, speaker, receiver)
    }

    /// A live exchange into the history of the place: the pipeline goes on with it.
    pub fn remember_live(&self, phrase: &Phrase, said: &str) {
        let inner = &self.inner;
        let settings = Settings::of(&inner.deps.settings);
        let principal = (inner.deps.people)(phrase.who.person.as_deref());
        let agent = agent::pick(
            phrase,
            &settings,
            &inner.deps.settings,
            &inner.deps.personas,
            inner.deps.characters.as_ref(),
            &inner.talks,
        );
        let key = agent::history_key(&turn::history_key(phrase, &principal), &agent, &settings);
        inner.deps.parts.dialog.remember(&key, &phrase.text, said, crate::dialog::AnsweredBy::Model, None);
    }

    /// The models' expected round times, counted from the lines at most every ten minutes.
    fn expected(&self, settings: &Settings) -> rounds::Expected {
        if let Some((at, expected)) =
            lock(&self.inner.expected).as_ref().filter(|(at, _)| at.elapsed() < EXPECTED_EVERY)
        {
            let _ = at;
            return expected.clone();
        }
        let days = settings.rounds.journal.days.max(1) as u64;
        let since = crate::utils::time::now_ms().saturating_sub(days * 86_400_000);
        let lines = self.lines(&stats::Query { from: Some(since), ..stats::Query::default() });
        let expected = rounds::Expected::of(&lines, settings.rounds.journal.rounds);
        *lock(&self.inner.expected) = Some((Instant::now(), expected.clone()));
        expected
    }
}

/// The agent speaking: the home's character (`<package>:<character>` or a card's id), as the panel chose it.
fn home_speaker(personas: &PersonaService) -> Option<String> {
    personas.chosen().and_then(|persona| persona.preset_id)
}

/// The last phrase of a work was the computer's own ear's: its line is the channel's to say.
fn by_the_ear(work: &Work) -> bool {
    let last = work
        .notes
        .iter()
        .map(|note| (&note.origin, note.at))
        .chain([(&work.origin, work.started_at)])
        .max_by_key(|(_, at)| *at);
    last.is_some_and(|(origin, _)| origin.device == COMPUTER && origin.name.is_some())
}

/// A work's «Готово» as a notice: its line, and the question it asks with what the question expects.
fn notice_of(work: &Work, speaker: Option<String>) -> Option<Notice> {
    let line = work.result.as_ref().map(|result| result.line.clone()).filter(|line| !line.is_empty())?;
    let asks = work.question.as_ref().is_some_and(|question| question.kind.is_some());
    Some(Notice {
        notice: format!("work:{}", work.id),
        text: line,
        speaker,
        why: if asks { NoticeWhy::WorkQuestion } else { NoticeWhy::WorkReady },
        work: Some(work.id.clone()),
        question: asks.then(|| Why::Ready.as_str().to_string()),
        urgency: Urgency::Now,
        to: Some(Hint { device: Some(work.device().to_string()), ..Hint::default() }),
        until: None,
        expects: expects_of_question(work.question.as_ref()),
    })
}

/// What a work's question expects (§14): the layer's questions know their shape.
pub fn expects_of(kind: Option<crate::talk::asked::Kind>) -> Expects {
    use crate::talk::asked::Kind;
    let choice =
        |options: &[&str]| Expects::Choice { options: options.iter().map(|option| option.to_string()).collect() };
    match kind {
        // «На стол или примерить?»
        Some(Kind::Ready) => choice(&["desk", "try"]),
        // «Поправить … — или сделать новую?»
        Some(Kind::Similar) => choice(&["edit", "new"]),
        // «…подождать или остановить?»
        Some(Kind::Wait) => choice(&["wait", "stop"]),
        // «Добавить ещё $5 или остановить?»
        Some(Kind::Budget) => choice(&["more", "stop"]),
        Some(
            Kind::Unfinished
            | Kind::Apply
            | Kind::Delete
            | Kind::Copy
            | Kind::Remake
            | Kind::Run
            | Kind::More
            | Kind::Wake
            // «Claude Code спрашивает: …» and «План для «…»: … Делаю?» (docs/plans/work-systems.md, stages 3 and 6).
            | Kind::Permission
            | Kind::Plan,
        ) => Expects::YesNo,
        Some(Kind::Which | Kind::Talk | Kind::Extra) | None => Expects::Free,
    }
}

/// What a work's question expects, with what a limit's question offers beyond its first answers (stage 5 of
/// docs/plans/work-systems.md): «…подождать, остановить, перейти на «Экономно» или передать bb?» expects those too.
pub fn expects_of_question(question: Option<&crate::talk::works::Question>) -> Expects {
    match (expects_of(question.and_then(|question| question.kind)), question) {
        (Expects::Choice { mut options }, Some(question)) => {
            options.extend(question.offers.iter().map(|offer| offer.kind.clone()));
            Expects::Choice { options }
        }
        (expects, _) => expects,
    }
}

impl Inner {
    fn send(&self, outside: Outside) {
        lock(&self.listeners).retain(|listener| listener.unbounded_send(outside.clone()).is_ok());
    }

    /// The works' changes become notices, once, for every listener.
    fn follow(self: &Arc<Self>) {
        if self.following.set(()).is_err() {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
        let me = Arc::downgrade(self);
        let mut changes = self.deps.parts.talk.events.subscribe();
        handle.spawn(async move {
            loop {
                let event = match changes.recv().await {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                };
                let Some(inner) = me.upgrade() else { return };
                let GatewayEvent::WorkChanged { work, device, change } = event.as_ref() else { continue };
                if device != COMPUTER {
                    continue;
                }
                if WITHDRAWN_ON.contains(&change.as_str()) {
                    inner.send(Outside::new(Unasked::Withdraw { notice: format!("work:{work}") }));
                    continue;
                }
                if !SAID_ON.contains(&change.as_str()) {
                    continue;
                }
                let Some(found) = inner.deps.parts.talk.works.get(work) else { continue };
                if found.said_at.is_some() || !by_the_ear(&found) {
                    continue;
                }
                if let Some(notice) = notice_of(&found, home_speaker(&inner.deps.personas)) {
                    inner.send(Outside::new(Unasked::Notice(notice)));
                }
            }
        });
    }

    /// A turn on a device ended: the line it did not say comes once more.
    fn turn_ended(&self, device: &str) {
        let pending = lock(&self.unsaid).remove(device);
        let Some(id) = pending else { return };
        let Some(work) = self.deps.parts.talk.works.get(&id).filter(|work| work.said_at.is_none()) else { return };
        if let Some(notice) = notice_of(&work, home_speaker(&self.deps.personas)) {
            self.send(Outside::new(Unasked::Notice(notice)));
        }
    }

    /// One turn, from the phrase to `end`.
    async fn run(
        self: Arc<Self>,
        phrase: Phrase,
        settings: Settings,
        out: Out,
        cancel: CancellationToken,
        record: line::Record,
        agent: agent::Agent,
    ) -> Ended {
        let parts = &self.deps.parts;
        let principal = (self.deps.people)(phrase.who.person.as_deref());
        let turn = turn_of(&phrase);
        let devices: Vec<door::Device> =
            self.deps.devices.as_ref().map(|devices| devices.devices()).unwrap_or_default();
        let ctx = fast::Ctx {
            parts,
            principal: &principal,
            turn: &turn,
            phrase: &phrase,
            normalized: crate::skills::text::normalize(&turn.text),
            devices: Some(&devices),
            pending: &self.pending,
            sense: &|kind, text, data| self.ask(&record, kind, text, phrase.lang.as_deref(), data),
            skill_dialog: self.dialogs.of(&phrase.place.device),
        };
        if !settings.fast.runs("answer", phrase.place.kind) {
            fast::let_go_of_the_brain_s_question(parts, &turn.origin.device);
        }
        for step in fast::steps(&settings, phrase.place.kind) {
            let Some(taken) = step.take(&ctx) else { continue };
            tracing::debug!(target: "thinking", "{} took «{}»", step.name(), phrase.turn);
            record.with(|line| line.fast = Some(step.name().to_string()));
            return match taken {
                fast::Taken::Talk(talk) => out::forward(parts, &phrase.place.device, talk, &out, &cancel).await,
                fast::Taken::Handoff(handed) => {
                    out.send(door::What::Handoff { capability: handed.capability, text: handed.text });
                    out.end(door::Outcome::HandedOff, None, None);
                    Ended { outcome: door::Outcome::HandedOff, said: String::new(), work: None, error: None }
                }
                fast::Taken::Skill(quick) => {
                    let leaves = hands::Leaves { dialogs: &self.dialogs, spoken: &self.spoken };
                    hands::skill_turn(parts, &principal, &phrase, &turn, &out, &cancel, quick, leaves).await
                }
                fast::Taken::Confirmed(pending) => {
                    self.confirmed(&phrase, &turn, &principal, &out, &cancel, pending).await
                }
                fast::Taken::Say(text) => {
                    out.say(&text);
                    out.end(door::Outcome::Answered, None, None);
                    Ended { outcome: door::Outcome::Answered, said: text, work: None, error: None }
                }
            };
        }
        turn::answer(turn::Brainwork {
            parts,
            principal: &principal,
            phrase: &phrase,
            turn: &turn,
            settings: &settings,
            out: &out,
            cancel: &cancel,
            record: &record,
            titles: self.deps.titles.clone(),
            usual: usual::Usual::of(&self.deps.settings),
            expected: OurThinking { inner: self.clone() }.expected(&settings),
            agent: &agent,
            sense: &|kind, text, data| self.ask(&record, kind, text, phrase.lang.as_deref(), data),
            place: (!phrase.hands.is_empty()).then(|| {
                Arc::new(hands::Place {
                    parts: parts.clone(),
                    principal: principal.clone(),
                    hands: hands::of_place(&phrase, &devices, &principal),
                    devices: devices.clone(),
                    phrase: phrase.clone(),
                    out: out.clone(),
                    calls: self.calls.clone(),
                    pending: self.pending.clone(),
                    answer_ms: hands::ANSWER_MS,
                    dialogs: self.dialogs.clone(),
                })
            }),
        })
        .await
    }

    /// A risky hand the person said «да» to: a group's through our skill, a named one through the door, and its words.
    async fn confirmed(
        &self,
        phrase: &Phrase,
        turn: &crate::talk::Turn,
        principal: &crate::core::identity::Principal,
        out: &Out,
        cancel: &CancellationToken,
        pending: hands::PendingHand,
    ) -> Ended {
        let parts = &self.deps.parts;
        let words = words::of(phrase.lang.as_deref().unwrap_or("ru"));
        if let Ok(intent) = serde_json::from_value::<crate::skills::capabilities::Intent>(pending.args.clone()) {
            if let crate::skills::router::Resolution::Own(skill) =
                parts.skills.router.resolve(&intent, false, Some(principal))
            {
                let quick = hands::Quick { skill, intent, device: pending.device };
                let leaves = hands::Leaves { dialogs: &self.dialogs, spoken: &self.spoken };
                return hands::skill_turn(parts, principal, phrase, turn, out, cancel, quick, leaves).await;
            }
        }
        let done = hands::send(out, &self.calls, &pending.device, &pending.hand, pending.args, hands::ANSWER_MS).await;
        let said = if done.ok { words.hands.done.clone() } else { words.hands.no_answer.clone() };
        out.say(&said);
        let outcome = if done.ok { door::Outcome::Answered } else { door::Outcome::Failed };
        out.end(outcome, (!done.ok).then_some(door::Failure::Error), done.error.clone());
        Ended { outcome, said, work: None, error: done.error }
    }

    /// A question of Thinking's kinds to «Чутьё», its answer kept in the turn's line.
    fn ask(
        &self,
        record: &line::Record,
        kind: crate::sense::Kind,
        text: &str,
        lang: Option<&str>,
        data: serde_json::Value,
    ) -> crate::sense::Answer {
        let answer = self.sense.ask(&sense::question(kind, text, lang, data));
        record.with(|line| line.sense.push(sense::sensed(kind, &answer)));
        answer
    }

    fn write(&self, request: &str, line: &line::Line) {
        if let (Some(lines), Ok(json)) = (&self.deps.lines, serde_json::to_value(line)) {
            lines.line(request, &json);
        }
    }

    /// A new phrase on a device: the line of the turn before it learns what came next.
    fn next_came(&self, phrase: &Phrase, settings: &Settings) {
        let earlier = lock(&self.last).remove(&phrase.place.device);
        let Some(mut earlier) = earlier else { return };
        let after_ms = earlier.ended.elapsed().as_millis() as u64;
        if after_ms > settings.line.next.watch_ms {
            earlier.line.next = Some(line::Next { after_ms, what: "quiet".into(), heard_chars: None });
        } else {
            let lang = phrase.lang.clone().unwrap_or_else(|| "ru".to_string());
            let words = words::of(&lang);
            earlier.line.next = Some(line::next_of(
                after_ms,
                phrase,
                &earlier.text,
                earlier.asked,
                settings.line.next.repeated_ms,
                words,
            ));
        }
        self.write(&earlier.request, &earlier.line);
    }

    /// A turn ended: its line is written, and kept until what comes next is known (or a quiet minute).
    fn line_ended(self: &Arc<Self>, phrase: &Phrase, out: &Out, ended: &Ended, record: &line::Record, watch_ms: u64) {
        let Some(request) = out.request() else { return };
        let cut = lock(&self.cuts).remove(&phrase.turn);
        record.with(|line| {
            line.first_word_ms = out.first_word_ms();
            line.waits = out
                .waits()
                .into_iter()
                .map(|(why, expect_ms, at_ms)| line::Wait { why: line::name_of(&why), expect_ms, at_ms })
                .collect();
            line.end = line::name_of(&ended.outcome);
            line.ms = out.elapsed_ms();
            if let Some(heard) = cut {
                line.next = Some(line::Next { after_ms: 0, what: "cut".into(), heard_chars: heard });
            }
        });
        let line = record.line();
        self.write(&request, &line);
        if line.next.is_some() {
            return;
        }
        let asked = ended.outcome == door::Outcome::Asked;
        let device = phrase.place.device.clone();
        lock(&self.last).insert(
            device.clone(),
            Last { request: request.clone(), line, ended: Instant::now(), text: phrase.text.clone(), asked },
        );
        // Nothing for a minute: quiet.
        let me = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(watch_ms)).await;
            let Some(inner) = me.upgrade() else { return };
            let waiting = {
                let mut last = lock(&inner.last);
                match last.get(&device) {
                    Some(earlier) if earlier.request == request => last.remove(&device),
                    _ => None,
                }
            };
            if let Some(mut earlier) = waiting {
                let after_ms = earlier.ended.elapsed().as_millis() as u64;
                earlier.line.next = Some(line::Next { after_ms, what: "quiet".into(), heard_chars: None });
                inner.write(&earlier.request, &earlier.line);
            }
        });
    }
}

/// The line a turn starts with: the settings and variants it runs with, the place, the agent, the budget.
fn line_of(phrase: &Phrase, settings: &Settings, speaker: Option<&str>) -> line::Line {
    let place = settings::Where::of(phrase.place.kind, phrase.budget.voice);
    line::Line {
        v: line::V,
        settings: settings.mark(),
        variants: line::Variants {
            thinking: "ours".into(),
            card: line::name_of(&settings.card_variant(place)),
            rounds: line::name_of(&settings.rounds_policy(place)),
            delivery: line::name_of(&settings.delivery(place)),
        },
        kind: settings::kind_name(phrase.place.kind).into(),
        voice: phrase.budget.voice,
        agent: line::Agent {
            to: phrase.to.agent.clone(),
            by: line::name_of(&phrase.to.by),
            speaker: speaker.map(str::to_string),
        },
        budget_ms: phrase.budget.first_word_ms,
        ..line::Line::default()
    }
}

impl door::Thinking for OurThinking {
    fn phrase(&self, phrase: Phrase) -> Events {
        let inner = self.inner.clone();
        let settings = Settings::of(&inner.deps.settings);
        // Who it is said to: its voice speaks every event of the turn (`as`).
        let agent = agent::pick(
            &phrase,
            &settings,
            &inner.deps.settings,
            &inner.deps.personas,
            inner.deps.characters.as_ref(),
            &inner.talks,
        );
        let speaker = agent.id.clone().or_else(|| home_speaker(&inner.deps.personas));
        let (sender, receiver) = mpsc::unbounded();
        let cancel = CancellationToken::new();
        let out = Out::new(sender, &phrase.turn, speaker, cancel.clone());
        let (turn_id, device) = (phrase.turn.clone(), phrase.place.device.clone());
        lock(&inner.going).insert(turn_id.clone(), (cancel.clone(), device.clone()));
        inner.next_came(&phrase, &settings);
        let record = line::Record::new(line_of(&phrase, &settings, out.speaker()));
        record.with(|line| line.agent.by = agent.by.to_string());
        let watch_ms = settings.line.next.watch_ms;
        tokio::spawn(async move {
            let named = matches!(agent.by, "to" | "window");
            let ended =
                inner.clone().run(phrase.clone(), settings, out.clone(), cancel, record.clone(), agent.clone()).await;
            // A word's agent keeps the device's talk for its window; anyone else lets it go.
            if named {
                inner.talks.spoke(&device, &agent);
            }
            if !out.ended() {
                // A turn stopped or dropped: its end, for whoever still listens.
                out.end(ended.outcome, None, ended.error.clone());
            }
            lock(&inner.going).remove(&turn_id);
            inner.line_ended(&phrase, &out, &ended, &record, watch_ms);
            inner.turn_ended(&device);
        });
        receiver.boxed()
    }

    fn tell(&self, news: News) {
        let inner = &self.inner;
        match news.what {
            Told::Stop { turn, why, heard } => {
                let going = lock(&inner.going).remove(&turn);
                if let Some((cancel, _)) = going {
                    tracing::info!(target: "thinking", "turn {turn} stopped: {why:?}");
                    lock(&inner.cuts).insert(turn, heard.and_then(|heard| heard.chars));
                    cancel.cancel();
                }
            }
            Told::Delivered { notice: Some(notice), outcome, device, .. } => {
                let Some(id) = notice.strip_prefix("work:") else { return };
                match outcome {
                    Delivery::Said => {
                        let _ = inner.deps.parts.talk.said_on(id, &device);
                    }
                    Delivery::Cut | Delivery::Later | Delivery::Shown => {
                        lock(&inner.unsaid).insert(device, id.to_string());
                    }
                    Delivery::Dropped | Delivery::Other => {}
                }
            }
            Told::NoAnswer { question } => {
                tracing::debug!(target: "thinking", "the window for «{question}» closed with no answer");
            }
            Told::HandsDone { call, ok, note, error } => {
                if !inner.calls.done(&call, hands::Done { ok, note, error }) {
                    tracing::debug!(target: "thinking", "a hand's answer nobody waits for: {call}");
                }
            }
            Told::Said { turn, at } => {
                tracing::debug!(target: "thinking", "turn {turn} was heard to its end at {at:?}");
                // A skill's «after the speech» starts now (a game's turn timer).
                inner.spoken.said(&turn, at);
            }
            Told::Delivered { .. } | Told::Unknown => {}
        }
    }

    fn notices(&self) -> Notices {
        let (sender, receiver) = mpsc::unbounded();
        lock(&self.inner.listeners).push(sender);
        self.inner.follow();
        receiver.boxed()
    }
}
