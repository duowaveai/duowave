//! Thinking's first implementation of the door (docs/plans/audio-thinking-contract.md; audio-interface-3.md, «The door
//! to Thinking», point 1): the talk layer behind it as it is — `begin`, its rules, the reader, the brain — and its
//! events turned into the door's. The computer's voice channel (crates/duowave-ears/src/channel) talks to it; the
//! panel's own talk (routes.rs) still calls `begin` straight.
//!
//! | Talk | Door |
//! |---|---|
//! | `start` | `begun`: the journal's request, where the brain thinks |
//! | `phrase` | `say` |
//! | `tool` | `wait` (`tool`, its name) |
//! | `done` | `work` when it touched one; the reminder as one more `say`; `ask` when it asks; `end` |
//! | `error` | the pack's line as a `say`, then `end` `failed` |
//!
//! A question's id says why the window opens: `question` (the answer asks), `ready` (a work's question reminded); the
//! audio side gives it back as the reply's `answer_to`, and the phrase becomes the window's with that reason
//! (`Origin.why`). A turn that touched a work and asks nothing keeps the window open by the audio side's own rule: the
//! work's line invites more («Добавлю: …»).
//!
//! Who it is said to goes through from the start, with today's behaviour behind it: the phrase's `to` becomes the
//! origin's word and character (the journal keeps them), and every event's `as` is the home's character.
//!
//! Outside a turn: a work of the computer is ready and its last phrase was the computer's own ear's (`Origin.name` set
//! on the computer) — a `notice` with its line; the line said (`delivered`, `said`) is the panel's
//! `POST /api/v1/works/{id}/said`; not said (`cut`, `later`, `shown`) — the notice comes once more after the device's
//! next turn, as the station's `unsaid`. A work decided or said elsewhere — `withdraw`.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use futures::channel::mpsc;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use super::origin::{Activator, Origin, Via, Why, COMPUTER};
use super::works::Work;
use super::{begin, Heard, Parts, TalkEvent, Turn};
use crate::core::events::GatewayEvent;
use crate::core::identity::Principal;
use crate::core::requests::Place;
use crate::dialog::PersonaService;
use crate::door::{
    self, Delivery, Event, Events, Expects, Failure, Notice, NoticeWhy, Notices, Opened, Outcome, Outside, Phrase,
    Told, Unasked, Urgency, WaitWhy, What,
};
use crate::skills::tools::ToolsKind;

/// A person's id (None — nobody signed in on the device) → who speaks: the gateway gives it (app.rs).
pub type People = Arc<dyn Fn(Option<&str>) -> Principal + Send + Sync>;

/// The changes of a work after which its line is said (the panel's `SAID_ON`).
const SAID_ON: &[&str] = &["ready", "unfinished", "failed", "done"];
/// The changes after which a waiting notice is no longer needed.
const WITHDRAWN_ON: &[&str] = &["said", "decided", "stopped", "answered", "declined", "removed"];

/// The talk layer behind the door.
pub struct TalkThinking {
    inner: Arc<Inner>,
}

struct Inner {
    parts: Parts,
    personas: Arc<PersonaService>,
    people: People,
    /// The turns going on: their stop.
    going: Mutex<HashMap<String, CancellationToken>>,
    /// A device → the work whose line it did not say: said again after its next turn, once.
    unsaid: Mutex<HashMap<String, String>>,
    listeners: Mutex<Vec<mpsc::UnboundedSender<Outside>>>,
    following: std::sync::OnceLock<()>,
}

impl TalkThinking {
    pub fn new(parts: Parts, personas: Arc<PersonaService>, people: People) -> TalkThinking {
        TalkThinking {
            inner: Arc::new(Inner {
                parts,
                personas,
                people,
                going: Mutex::new(HashMap::new()),
                unsaid: Mutex::new(HashMap::new()),
                listeners: Mutex::new(Vec::new()),
                following: std::sync::OnceLock::new(),
            }),
        }
    }
}

/// The agent speaking: the home's character (`<package>:<character>` or a card's id), as the panel chose it.
fn speaker(personas: &PersonaService) -> Option<String> {
    personas.chosen().and_then(|persona| persona.preset_id)
}

/// The phrase as the talk layer takes it.
pub fn turn_of(phrase: &Phrase) -> Turn {
    let activator = match phrase.opened {
        Opened::Word => Activator::Word,
        Opened::Window | Opened::Tap => Activator::Answer,
        Opened::Text => Activator::Text,
        Opened::Button | Opened::Other => Activator::Button,
    };
    let why = (activator == Activator::Answer).then(|| {
        if phrase.context.over.is_some() {
            return Why::Barge;
        }
        match (phrase.context.answer_to.as_deref().and_then(Why::parse), phrase.context.after) {
            (Some(why), _) => why,
            (None, Some(door::After::Work)) => Why::Work,
            (None, Some(door::After::Notice)) => Why::Ready,
            (None, _) => Why::Question,
        }
    });
    let origin = Origin {
        activator,
        name: phrase.to.name.clone(),
        character: phrase.to.agent.clone(),
        who: None,
        device: phrase.place.device.clone(),
        via: if activator == Activator::Text { Via::Text } else { Via::Voice },
        why,
        role: None,
    };
    let heard = phrase.heard.as_ref().map(|heard| Heard {
        by: if heard.by == "cloud" { Place::Cloud } else { Place::Own },
        ms: heard.ms,
        what: heard.model.clone(),
    });
    Turn {
        text: phrase.text.clone(),
        origin,
        heard,
        voice: phrase.budget.voice,
        fresh: phrase.context.fresh,
        tools: tools_kind(&phrase.hands),
    }
}

/// The hands of the place as today's choice of tools (audio-interface-3.md, «The door to Thinking», point 3): a
/// station's hands give its commands, no hands the conversation's tools. Thinking's own plan offers the hands of the
/// place instead.
pub fn tools_kind(hands: &[String]) -> ToolsKind {
    if hands.is_empty() {
        ToolsKind::Conversation
    } else {
        ToolsKind::Command
    }
}

/// A work's title and state, for the door's `work`.
fn work_of(parts: &Parts, id: &str) -> What {
    let work = parts.talk.works.get(id);
    What::Work {
        work: id.to_string(),
        title: work.as_ref().map(|work| work.title.clone()),
        state: work.as_ref().map(|work| work.state.as_str().to_string()),
    }
}

/// One event of the talk as the door's: what to send, in order.
fn door_of(parts: &Parts, device: &str, event: TalkEvent) -> Vec<What> {
    match event {
        TalkEvent::Start { request, by } => vec![What::Begun { request, by: by.to_string() }],
        TalkEvent::Phrase { text } => vec![What::Say { text }],
        TalkEvent::Tool { name } => vec![What::Wait { why: WaitWhy::Tool, expect_ms: None, name: Some(name) }],
        TalkEvent::Done { said, ms, work, asks, remind, dropped, .. } => {
            let mut whats = Vec::new();
            if let Some(id) = &work {
                whats.push(work_of(parts, id));
            }
            if dropped {
                whats.push(What::End { outcome: Outcome::Dropped, reason: None, said, ms, error: None });
                return whats;
            }
            let why = if remind.is_some() {
                Some(Why::Ready)
            } else if asks {
                Some(Why::Question)
            } else {
                None
            };
            if let Some(remind) = remind {
                whats.push(What::Say { text: remind });
            }
            match why {
                Some(why) => {
                    whats.push(What::Ask { question: why.as_str().to_string(), text: None, expects: Expects::Free });
                    whats.push(What::End { outcome: Outcome::Asked, reason: None, said, ms, error: None });
                }
                None => whats.push(What::End { outcome: Outcome::Answered, reason: None, said, ms, error: None }),
            }
            whats
        }
        TalkEvent::Error { message } => {
            tracing::info!(target: "talk", "an answer on {device} broke off: {message}");
            let pack = super::pack::of(parts.talk.lang(device));
            vec![
                What::Say { text: pack.failed.to_string() },
                What::End {
                    outcome: Outcome::Failed,
                    reason: Some(Failure::Error),
                    said: String::new(),
                    ms: 0,
                    error: Some(message),
                },
            ]
        }
    }
}

/// The last phrase of a work was the computer's own ear's: its line is the channel's to say (stage 3, К5–К6).
fn by_the_ear(work: &Work) -> bool {
    let last = work
        .notes
        .iter()
        .map(|note| (&note.origin, note.at))
        .chain([(&work.origin, work.started_at)])
        .max_by_key(|(_, at)| *at);
    last.is_some_and(|(origin, _)| origin.device == COMPUTER && origin.name.is_some())
}

/// A work's «Готово» as a notice.
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
        to: Some(door::Hint { device: Some(work.device().to_string()), ..door::Hint::default() }),
        until: None,
        expects: Expects::Free,
    })
}

impl Inner {
    fn send(&self, outside: Outside) {
        let mut listeners = self.listeners.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        listeners.retain(|listener| listener.unbounded_send(outside.clone()).is_ok());
    }

    /// The works' changes become notices, once, for every listener.
    fn follow(self: &Arc<Self>) {
        if self.following.set(()).is_err() {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
        let me = Arc::downgrade(self);
        let mut changes = self.parts.talk.events.subscribe();
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
                let Some(found) = inner.parts.talk.works.get(work) else { continue };
                if found.said_at.is_some() || !by_the_ear(&found) {
                    continue;
                }
                if let Some(notice) = notice_of(&found, speaker(&inner.personas)) {
                    inner.send(Outside::new(Unasked::Notice(notice)));
                }
            }
        });
    }

    /// A turn on a device ended: the line it did not say comes once more.
    fn turn_ended(&self, device: &str) {
        let pending = self.unsaid.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(device);
        let Some(id) = pending else { return };
        let Some(work) = self.parts.talk.works.get(&id).filter(|work| work.said_at.is_none()) else { return };
        if let Some(notice) = notice_of(&work, speaker(&self.personas)) {
            self.send(Outside::new(Unasked::Notice(notice)));
        }
    }
}

impl door::Thinking for TalkThinking {
    fn phrase(&self, phrase: Phrase) -> Events {
        let inner = self.inner.clone();
        let turn_id = phrase.turn.clone();
        let speaker = speaker(&inner.personas);
        let principal = (inner.people)(phrase.who.person.as_deref());
        let turn = turn_of(&phrase);
        let device = turn.origin.device.clone();
        let (sender, receiver) = mpsc::unbounded();
        let event = {
            let (turn_id, speaker) = (turn_id.clone(), speaker.clone());
            move |what: What| Event::new(&turn_id, speaker.as_deref(), what)
        };
        let mut talk = match begin(&inner.parts, &principal, turn) {
            Ok(talk) => talk,
            Err(error) => {
                // No model, or a phrase that cannot be taken: its words, then the end.
                let reason =
                    if error.status == axum::http::StatusCode::CONFLICT { Failure::NoModel } else { Failure::Error };
                let _ = sender.unbounded_send(event(What::Say { text: error.message.clone() }));
                let _ = sender.unbounded_send(event(What::End {
                    outcome: Outcome::Failed,
                    reason: Some(reason),
                    said: String::new(),
                    ms: 0,
                    error: Some(error.message.clone()),
                }));
                return receiver.boxed();
            }
        };
        inner
            .going
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(turn_id.clone(), talk.cancel.clone());
        tokio::spawn(async move {
            'talk: while let Some(next) = talk.events.recv().await {
                for what in door_of(&inner.parts, &device, next) {
                    // The audio side dropped the answer: it is not needed any more.
                    if sender.unbounded_send(event(what)).is_err() {
                        talk.cancel.cancel();
                        break 'talk;
                    }
                }
            }
            inner.going.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&turn_id);
            inner.turn_ended(&device);
        });
        receiver.boxed()
    }

    fn tell(&self, news: door::News) {
        let inner = &self.inner;
        match news.what {
            Told::Stop { turn, why, .. } => {
                let going = inner.going.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&turn);
                if let Some(cancel) = going {
                    tracing::info!(target: "talk", "turn {turn} stopped: {why:?}");
                    cancel.cancel();
                }
            }
            Told::Delivered { notice: Some(notice), outcome, device, .. } => {
                let Some(id) = notice.strip_prefix("work:") else { return };
                match outcome {
                    Delivery::Said => {
                        let _ = inner.parts.talk.said_on(id, &device);
                    }
                    Delivery::Cut | Delivery::Later | Delivery::Shown => {
                        inner
                            .unsaid
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .insert(device, id.to_string());
                    }
                    Delivery::Dropped | Delivery::Other => {}
                }
            }
            Told::NoAnswer { question } => {
                tracing::debug!(target: "talk", "the window for «{question}» closed with no answer");
            }
            Told::HandsDone { .. } | Told::Delivered { .. } | Told::Said { .. } | Told::Unknown => {}
        }
    }

    fn notices(&self) -> Notices {
        let (sender, receiver) = mpsc::unbounded();
        self.inner.listeners.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(sender);
        self.inner.follow();
        receiver.boxed()
    }
}

/// The station's chain behind the door (docs/plans/audio-interface-3.md, part Н; the contract's «Where it lives in the
/// code», the station: «the chain moves behind the door — by wrapping, not rewriting»). The station's session routes a
/// phrase as before and prepares the turn's work — a conversation with its tools, one of our skills on the intent the
/// rules chose (the quick rules, the layer's rules, the others), the talk's reader first, or a command the model
/// picks a tool for — with what only the station knows (its tools' runners, how its router resolves an intent, its
/// Yandex); then it sends the phrase. What Thinking finds comes back as the door's events: `say`, `hands` (the
/// station's commands, `ear::Command` as JSON), `ask`, `follow` (the skill goes on with the next phrase), `wait`,
/// `handoff` (to Alice), `end`. A skill's «after the speech» waits for `said`.
pub struct DialogThinking {
    dialog: Arc<crate::dialog::DialogService>,
    personas: Arc<PersonaService>,
    prepared: Mutex<HashMap<String, Prepared>>,
    /// A skill's hook for the end of the speech, by turn: run on `said`. An answer not spoken to its end tells none,
    /// so only the last few are kept.
    spoken: Arc<Mutex<VecDeque<(String, crate::skills::skill::SpokenHook)>>>,
}

/// How many hooks for the end of the speech wait for their `said`.
const SPOKEN_KEPT: usize = 16;

/// A turn as the station's session prepares it.
pub struct Prepared {
    /// The talk's history: the station's own.
    pub key: String,
    pub context: crate::dialog::DialogContext,
    pub cancel: CancellationToken,
    pub work: StationWork,
    /// Who hears that the model now puts a skill's data into words (the journal's step).
    pub wording: Option<Box<dyn FnOnce() + Send + Sync>>,
}

/// What a station's turn asks of Thinking.
pub enum StationWork {
    /// The model's words, with the station's tools; `start` hears that the conversation begins (the stats).
    Converse { toolbox: Option<crate::dialog::ToolBox>, start: Option<Box<dyn FnOnce() + Send + Sync>> },
    /// One of our skills on the intent the rules chose.
    Skill {
        skill: Arc<dyn crate::skills::skill::OwnSkill>,
        intent: crate::core::capabilities::Intent,
        context: Box<crate::skills::skill::ExecutionContext>,
    },
    /// The talk's reader first; a phrase it does not take goes on as `then`.
    Read { reader: Arc<super::reader::Reader>, origin: Origin, then: Box<StationWork> },
    /// A command the model picks a tool for.
    Command(Box<Command>),
}

/// A command by the model: its tools, and the station's ways to go on.
pub struct Command {
    pub tools: Vec<crate::dialog::llm::LlmTool>,
    pub instructions: Option<String>,
    /// A call a connected server or an app answers: the talk goes on as a conversation.
    pub aside: Box<dyn Fn(&str) -> bool + Send + Sync>,
    /// The tools of that conversation.
    pub lookups: Box<dyn FnOnce() -> Option<crate::dialog::ToolBox> + Send>,
    /// How the station's router resolves the intent the model's call names.
    pub resolve: Resolve,
    /// Who hears of the calls not run: one command per request.
    pub ignored: Ignored,
}

/// How the station's router resolves an intent a model's call names (the call's name with it).
pub type Resolve = Box<dyn FnOnce(&crate::core::capabilities::Intent, &str) -> Resolved + Send>;
/// Who hears of a model's calls not run.
pub type Ignored = Box<dyn FnOnce(&[String]) + Send>;

/// An intent as the station's router resolves it.
pub enum Resolved {
    Own {
        skill: Arc<dyn crate::skills::skill::OwnSkill>,
        intent: crate::core::capabilities::Intent,
        context: Box<crate::skills::skill::ExecutionContext>,
    },
    /// Yandex answers it: the station handed it there already.
    HandedOff,
    /// Not served here: these words say why.
    Unsupported { phrase: String },
}

/// The question an `ask` of the station names: «listen after this answer».
pub const STATION_LISTENS: &str = "station";
/// The question the model asked about a command: its answer comes back with the command's tools.
pub const COMMAND_QUESTION: &str = "command";
/// The hand that records the answer's sound into a file (`record_audio`).
pub const RECORD_AUDIO: &str = "record_audio";

impl DialogThinking {
    pub fn new(dialog: Arc<crate::dialog::DialogService>, personas: Arc<PersonaService>) -> DialogThinking {
        DialogThinking { dialog, personas, prepared: Mutex::new(HashMap::new()), spoken: Arc::default() }
    }

    /// The turn's work, before its phrase.
    pub fn prepare(&self, turn: &str, prepared: Prepared) {
        self.prepared.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(turn.to_string(), prepared);
    }
}

/// What a turn sends: the door's events, in order.
type Send_ = Arc<dyn Fn(What) -> bool + Send + Sync>;

struct Turn_ {
    dialog: Arc<crate::dialog::DialogService>,
    spoken: Arc<Mutex<VecDeque<(String, crate::skills::skill::SpokenHook)>>>,
    turn: String,
    device: String,
    key: String,
    text: String,
    context: crate::dialog::DialogContext,
    cancel: CancellationToken,
    wording: Option<Box<dyn FnOnce() + Send + Sync>>,
    send: Send_,
    calls: u32,
}

impl Turn_ {
    /// A whole text as phrases, so long ones are synthesized in parallel.
    fn say_text(&self, text: &str) {
        for phrase in crate::dialog::split_phrases(text) {
            (self.send)(What::Say { text: phrase });
        }
    }

    async fn say_all(&self, mut phrases: crate::dialog::PhraseStream) -> anyhow::Result<()> {
        while let Some(phrase) = phrases.next().await {
            (self.send)(What::Say { text: phrase? });
        }
        Ok(())
    }

    /// The work done; an error is what broke.
    fn run(mut self, work: StationWork) -> futures::future::BoxFuture<'static, anyhow::Result<()>> {
        Box::pin(async move {
            match work {
                StationWork::Converse { toolbox, start } => {
                    if let Some(start) = start {
                        start();
                    }
                    let phrases =
                        self.dialog.answer(&self.key, &self.text, self.context.clone(), self.cancel.clone(), toolbox);
                    self.say_all(phrases).await
                }
                StationWork::Skill { skill, intent, context } => {
                    let outcome = skill.execute(&intent, &context).await?;
                    self.outcome(outcome, None).await
                }
                StationWork::Read { reader, origin, then } => {
                    let read =
                        reader.read(&self.key, origin, &self.text, self.context.clone(), self.cancel.clone()).await;
                    match read {
                        Some(read) => {
                            if read.listen {
                                (self.send)(What::Ask {
                                    question: STATION_LISTENS.to_string(),
                                    text: None,
                                    expects: Expects::Free,
                                });
                            }
                            self.say_text(&read.said);
                            Ok(())
                        }
                        None => self.run(*then).await,
                    }
                }
                StationWork::Command(command) => self.command(*command).await,
            }
        })
    }

    async fn command(mut self, command: Command) -> anyhow::Result<()> {
        let Command { tools, instructions, aside, lookups, resolve, ignored } = command;
        let choice = self
            .dialog
            .decide(&self.key, &self.text, self.context.clone(), self.cancel.clone(), tools, instructions)
            .await?;
        if choice.calls.iter().any(|made| aside(&made.name)) {
            // A connected server or an app answers: «Сейчас посмотрю» while it works, then the model's words.
            (self.send)(What::Wait { why: WaitWhy::Tool, expect_ms: None, name: None });
            let Some(toolbox) = lookups() else { return Ok(()) };
            return self.say_all(choice.proceed(toolbox, self.cancel.clone())).await;
        }
        let mut calls = choice.calls.clone().into_iter();
        let Some(call) = calls.next() else {
            if !choice.text.is_empty() {
                self.dialog.remember(&self.key, &self.text, &choice.text, crate::dialog::AnsweredBy::Model, None);
            }
            // A question about the command: its answer comes back with the command's tools.
            if super::ends_with_question(&choice.text) {
                (self.send)(What::Ask { question: COMMAND_QUESTION.to_string(), text: None, expects: Expects::Free });
            }
            self.say_text(&choice.text);
            return Ok(());
        };
        let not_run: Vec<String> = calls.map(|call| call.name).collect();
        if !not_run.is_empty() {
            ignored(&not_run);
        }
        let found = match crate::skills::tools::intent_of(&call) {
            Ok(found) => found,
            Err(error) => {
                let result = crate::dialog::llm::ToolResult {
                    id: call.id.clone(),
                    content: format!("error: {error}"),
                    is_error: true,
                };
                return self.say_all(choice.continue_with(vec![result], self.cancel.clone())).await;
            }
        };
        let (skill, intent, context) = match resolve(&found, &call.name) {
            Resolved::Own { skill, intent, context } => (skill, intent, context),
            Resolved::HandedOff => return Ok(()),
            Resolved::Unsupported { phrase } => {
                (self.send)(What::Say { text: phrase });
                return Ok(());
            }
        };
        let outcome = skill.execute(&intent, &context).await?;
        let choice = Arc::new(choice);
        let (continued, token, id) = (choice.clone(), self.cancel.clone(), call.id.clone());
        let on_data: Box<dyn FnOnce(String) -> crate::dialog::PhraseStream + Send> = Box::new(move |data| {
            continued.continue_with(vec![crate::dialog::llm::ToolResult { id, content: data, is_error: false }], token)
        });
        self.outcome(outcome, Some(on_data)).await
    }

    /// A skill's outcome as the door's events: Alice first when it is hers, the hook for the end of the speech kept,
    /// the commands as hands, «listen after», the skill going on, a recording, then the words.
    async fn outcome(
        &mut self,
        outcome: crate::skills::skill::Outcome,
        on_data: Option<Box<dyn FnOnce(String) -> crate::dialog::PhraseStream + Send>>,
    ) -> anyhow::Result<()> {
        let crate::skills::skill::Outcome {
            speech,
            phrases,
            commands,
            listen,
            data,
            continuation,
            record_audio,
            done,
            kept,
            on_spoken,
            to_yandex,
        } = outcome;
        if to_yandex {
            (self.send)(What::Handoff { capability: None, text: None });
        }
        if let Some(hook) = on_spoken {
            let mut spoken = self.spoken.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            spoken.retain(|(turn, _)| *turn != self.turn);
            spoken.push_back((self.turn.clone(), hook));
            while spoken.len() > SPOKEN_KEPT {
                spoken.pop_front();
            }
        }
        for command in &commands {
            self.calls += 1;
            let (hand, args) = crate::ear::hands::hand_of(command);
            (self.send)(What::Hands {
                call: format!("{}:{}", self.turn, self.calls),
                device: self.device.clone(),
                hand,
                args,
            });
        }
        if listen {
            (self.send)(What::Ask { question: STATION_LISTENS.to_string(), text: None, expects: Expects::Free });
        }
        if let Some(file) = record_audio {
            self.calls += 1;
            (self.send)(What::Hands {
                call: format!("{}:{}", self.turn, self.calls),
                device: self.device.clone(),
                hand: RECORD_AUDIO.to_string(),
                args: serde_json::json!({ "file": file }),
            });
        }
        // The skill going on (a game's next move); none: an earlier one is over.
        let (capability, ttl_ms) = continuation
            .map(|continuation| (continuation.capability.as_str().to_string(), continuation.ttl_ms))
            .unwrap_or_default();
        (self.send)(What::Follow { capability, ttl_ms });
        if let Some(phrases) = phrases {
            return self.say_all(phrases).await;
        }
        if let (Some(data), true) = (&data, self.dialog.enabled()) {
            // The model puts the skill's data into words.
            if let Some(wording) = self.wording.take() {
                wording();
            }
            if let Some(on_data) = on_data {
                return self.say_all(on_data(data.clone())).await;
            }
            let context = crate::dialog::DialogContext {
                facts: Some(data.clone()),
                profile: Some(crate::config::ModelProfile::Fast),
                reason: None,
                ..self.context.clone()
            };
            let phrases = self.dialog.answer(&self.key, &self.text, context, self.cancel.clone(), None);
            return self.say_all(phrases).await;
        }
        if let Some(speech) = speech {
            let remembered = kept.as_deref().unwrap_or(&speech);
            self.dialog.remember(&self.key, &self.text, remembered, crate::dialog::AnsweredBy::Skill, done.as_deref());
            self.say_text(&speech);
        }
        Ok(())
    }
}

impl door::Thinking for DialogThinking {
    fn phrase(&self, phrase: Phrase) -> Events {
        let prepared = self.prepared.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&phrase.turn);
        let speaker = speaker(&self.personas);
        let (sender, receiver) = mpsc::unbounded();
        let event = {
            let (turn, speaker) = (phrase.turn.clone(), speaker.clone());
            move |what: What| Event::new(&turn, speaker.as_deref(), what)
        };
        let Some(prepared) = prepared else {
            let _ = sender.unbounded_send(event(What::End {
                outcome: Outcome::Failed,
                reason: Some(Failure::Error),
                said: String::new(),
                ms: 0,
                error: Some("the turn was not prepared".to_string()),
            }));
            return receiver.boxed();
        };
        let said = Arc::new(Mutex::new(Vec::<String>::new()));
        let send: Send_ = {
            let (sender, said, event) = (sender.clone(), said.clone(), event.clone());
            Arc::new(move |what: What| {
                if let What::Say { text } = &what {
                    said.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(text.clone());
                }
                sender.unbounded_send(event(what)).is_ok()
            })
        };
        let Prepared { key, context, cancel, work, wording } = prepared;
        let turn = Turn_ {
            dialog: self.dialog.clone(),
            spoken: self.spoken.clone(),
            turn: phrase.turn.clone(),
            device: phrase.place.device.clone(),
            key,
            text: phrase.text.clone(),
            context,
            cancel,
            wording,
            send,
            calls: 0,
        };
        tokio::spawn(async move {
            let ended = turn.run(work).await;
            let said = said.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).join(" ");
            let end = match ended {
                Ok(()) => What::End { outcome: Outcome::Answered, reason: None, said, ms: 0, error: None },
                Err(error) => What::End {
                    outcome: Outcome::Failed,
                    reason: Some(Failure::Error),
                    said,
                    ms: 0,
                    error: Some(format!("{error:#}")),
                },
            };
            let _ = sender.unbounded_send(event(end));
        });
        receiver.boxed()
    }

    /// `said` runs a skill's hook for the end of the speech; the station stops its answer itself (its token).
    fn tell(&self, news: door::News) {
        if let Told::Said { turn, at } = news.what {
            let hook = {
                let mut spoken = self.spoken.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                spoken.iter().position(|(kept, _)| *kept == turn).and_then(|at| spoken.remove(at)).map(|(_, hook)| hook)
            };
            if let Some(hook) = hook {
                hook(at.unwrap_or_else(crate::utils::time::now_ms));
            }
        }
    }

    /// The station's «Готово» goes by the talk layer's own way (ready.rs) today.
    fn notices(&self) -> Notices {
        futures::stream::empty().boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::door::{After, Budget, By, Context, Kind, News, Place as Where, To, Who, V};

    fn phrase(opened: Opened, context: Context) -> Phrase {
        Phrase {
            v: V,
            turn: "t-1".into(),
            text: "и ещё юань".into(),
            lang: None,
            alts: Vec::new(),
            who: Who::default(),
            to: To { agent: Some("butler:otto".into()), by: By::Word, name: Some("Отто".into()) },
            place: Where { device: COMPUTER.into(), room: None, kind: Kind::Computer, screen: true, speaker: true },
            hands: Vec::new(),
            opened,
            context,
            budget: Budget { voice: true, first_word_ms: Some(1500), limit_ms: None },
            heard: Some(door::Heard { by: "cloud".into(), ms: Some(420), model: Some("gigaam".into()) }),
            at: None,
        }
    }

    /// A skill that answers with the outcomes it is given, one per turn.
    struct Gives(Mutex<VecDeque<crate::skills::skill::Outcome>>);

    impl crate::skills::skill::OwnSkill for Gives {
        fn capability(&self) -> crate::skills::capabilities::CapabilityName {
            crate::skills::capabilities::CapabilityName::Games
        }

        fn verified(&self, _intent: &crate::core::capabilities::Intent) -> bool {
            true
        }

        fn execute<'a>(
            &'a self,
            _intent: &'a crate::core::capabilities::Intent,
            _context: &'a crate::skills::skill::ExecutionContext,
        ) -> futures::future::BoxFuture<'a, anyhow::Result<crate::skills::skill::Outcome>> {
            let outcome = self.0.lock().unwrap().pop_front().unwrap_or_default();
            Box::pin(async move { Ok(outcome) })
        }
    }

    fn station_thinking() -> DialogThinking {
        use crate::config::{Config, Env};
        let config = Config::for_tests(&Env::default()).dialog;
        let db = Arc::new(crate::storage::Db::memory());
        let events = crate::core::events::EventBus::new();
        let settings = Arc::new(crate::settings::SettingsService::new(db.clone(), events.clone()));
        let personas = Arc::new(PersonaService::new(&config, db, settings, events).unwrap());
        DialogThinking::new(Arc::new(crate::dialog::DialogService::new(config, personas.clone(), None)), personas)
    }

    /// The station's turn on the skill: what the door brings back.
    async fn station_turn(thinking: &DialogThinking, turn: &str, skill: Arc<Gives>) -> Vec<What> {
        let intent = crate::core::capabilities::Intent::new(crate::skills::capabilities::CapabilityName::Games, "play");
        thinking.prepare(
            turn,
            Prepared {
                key: "kitchen".into(),
                context: crate::dialog::DialogContext::default(),
                cancel: CancellationToken::new(),
                work: StationWork::Skill {
                    skill,
                    intent,
                    context: Box::new(crate::skills::skill::ExecutionContext::new("kitchen", chrono::Utc::now())),
                },
                wording: None,
            },
        );
        let mut asked = phrase(Opened::Word, Context::default());
        asked.turn = turn.into();
        asked.place = Where { device: "kitchen".into(), room: None, kind: Kind::Station, screen: false, speaker: true };
        door::Thinking::phrase(thinking, asked).map(|event| event.what).collect().await
    }

    #[tokio::test]
    async fn a_station_skill_s_outcome_comes_through_the_door_as_hands_ask_follow_and_words() {
        let thinking = station_thinking();
        let heard = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let hook = {
            let heard = heard.clone();
            Box::new(move |at: u64| heard.store(at, std::sync::atomic::Ordering::SeqCst))
        };
        let game = crate::skills::skill::Outcome {
            speech: Some("Загадала. Ваш ход.".into()),
            commands: vec![crate::ear::Command::Louder],
            listen: true,
            continuation: Some(crate::skills::skill::ContinuationRequest {
                capability: crate::skills::capabilities::CapabilityName::Games,
                ttl_ms: 30_000,
            }),
            on_spoken: Some(hook),
            ..Default::default()
        };
        let over = crate::skills::skill::Outcome::speech("Игра окончена.");
        let skill = Arc::new(Gives(Mutex::new(VecDeque::from([game, over]))));

        let events = station_turn(&thinking, "kitchen:1", skill.clone()).await;
        let Some(What::Hands { call, device, hand, args }) = events.first() else { panic!("hands first: {events:?}") };
        assert_eq!((call.as_str(), device.as_str(), hand.as_str()), ("kitchen:1:1", "kitchen", "louder"));
        assert!(matches!(crate::ear::hands::command_of(args), Some(crate::ear::Command::Louder)));
        assert!(matches!(&events[1], What::Ask { question, .. } if question == STATION_LISTENS), "{events:?}");
        assert!(
            matches!(&events[2], What::Follow { capability, ttl_ms: 30_000 } if capability == "games"),
            "{events:?}"
        );
        assert!(matches!(&events[3], What::Say { text } if text == "Загадала. Ваш ход."), "{events:?}");
        assert!(matches!(events.last(), Some(What::End { outcome: Outcome::Answered, .. })), "{events:?}");

        // The speech ended on the station: the skill's hook hears when; a turn not spoken tells nothing.
        door::Thinking::tell(&thinking, News::new(Told::Said { turn: "kitchen:0".into(), at: Some(7) }));
        assert_eq!(heard.load(std::sync::atomic::Ordering::SeqCst), 0);
        door::Thinking::tell(&thinking, News::new(Told::Said { turn: "kitchen:1".into(), at: Some(42) }));
        assert_eq!(heard.load(std::sync::atomic::Ordering::SeqCst), 42);

        // A skill that does not go on: the station forgets the earlier one.
        let events = station_turn(&thinking, "kitchen:2", skill).await;
        assert!(matches!(&events[0], What::Follow { capability, ttl_ms: 0 } if capability.is_empty()), "{events:?}");
        assert!(matches!(&events[1], What::Say { text } if text == "Игра окончена."), "{events:?}");
    }

    #[test]
    fn a_phrase_becomes_today_s_turn_with_its_word_and_the_window_s_reason() {
        let turn = turn_of(&phrase(Opened::Button, Context::default()));
        assert_eq!((turn.origin.activator, turn.origin.why), (Activator::Button, None));
        assert_eq!(
            (turn.origin.name.as_deref(), turn.origin.character.as_deref()),
            (Some("Отто"), Some("butler:otto"))
        );
        assert_eq!(turn.heard.as_ref().map(|heard| (heard.by, heard.ms)), Some((Place::Cloud, Some(420))));
        assert!(turn.voice);
        let after_work = Context { after: Some(After::Work), ..Context::default() };
        assert_eq!(turn_of(&phrase(Opened::Window, after_work)).origin.why, Some(Why::Work));
        let to_ready = Context { answer_to: Some("ready".into()), after: Some(After::Ask), ..Context::default() };
        assert_eq!(turn_of(&phrase(Opened::Window, to_ready)).origin.why, Some(Why::Ready));
        let over = Context { over: Some("t-0".into()), ..Context::default() };
        assert_eq!(turn_of(&phrase(Opened::Window, over)).origin.why, Some(Why::Barge));
        assert_eq!(turn_of(&phrase(Opened::Text, Context::default())).origin.via, Via::Text);
    }
}
