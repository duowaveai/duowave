//! An honest «Готово» and the question at the end (docs/plans/audio-interface.md, «Честное «Готово» и вопрос в
//! конце», «На стол голосом», «Правка вещи на столе»).
//!
//! - The line is the gateway's, from the title, the manifest and the install's plan — never the agent's words: «Испанский:
//!   готово. На стол или примерить?», «…Нужен ключ Todoist…», «…Будет ходить на api.todoist.com…»; «не доделано.
//!   Продолжить?»; «не получилось. Подробности в пульте.». A new version of a thing on the desk that passes its check
//!   and asks for nothing new goes there at once: «Испанский: слова крупнее. На столе.»; one that asks — «…Применить?».
//! - It is said where the work was asked: a station says it when it is free (up to ten minutes); a line it did not say
//!   to its end (cut by «стоп», the station busy) is said once more after the device's next conversation. The
//!   computer's panel says its own (`work.changed`). The tile waits on the desk in any case.
//! - The answer, without the wake word, goes to the device's question (asked.rs): «да» puts it on the desk — the rights
//!   and hosts were named in the question, dangerous ones stay the panel's, a key nobody filled in leaves it on the
//!   desk «ждёт ключ» with the panel's window open; «примерить», «нет», «в черновики», «подробнее», «переделай…»,
//!   «удали» → «Удалить черновик?». What the pack cannot read is not the question's.

use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::asked::{Kind, Pending, ANSWER_FOR};
use super::executor::{ByVoice, Put};
use super::layer::TalkLayer;
use super::origin::{Origin, COMPUTER};
use super::pack::{self, fill, Pack};
use super::work_tools::{in_sentence, Refusal};
use super::works::{End, Face, Outcome, Question, Work, WorkResult, WorkState};
use crate::core::events::GatewayEvent;
use crate::core::requests::{RequestKind, RequestStatus};
use crate::utils::time::now_ms;

/// A busy station has «Готово» wait this long at most, asked again every so often.
const READY_WAITS: Duration = Duration::from_secs(10 * 60);
const BUSY_POLL: Duration = Duration::from_secs(2);
/// «Верни как было» undoes a new version put on the desk by voice this recently.
pub const UNDO_MS: u64 = 15 * 60_000;

/// An answer to the layer's question, as the pack reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "is", rename_all = "camelCase")]
pub enum Answer {
    Yes,
    Fit,
    No,
    Park,
    Details,
    Redo {
        text: String,
    },
    Drop,
    /// To «Поправить … или новую?»: a new thing.
    New,
    /// To «Поправить … или новую?»: a new version of that thing.
    Edit,
    /// To «Какую: … или …?»: the thing named so (0 — the first; `usize::MAX` — the last).
    Choice(usize),
}

/// What the pack reads in a phrase said to the question; None — not the question's.
pub fn answer_of(pack: &Pack, normalized: &str) -> Option<Answer> {
    let answers = &pack.work.rules.answers;
    let text = normalized.trim();
    let any = |patterns: &[&str]| {
        patterns.iter().any(|pattern| regex::Regex::new(pattern).is_ok_and(|rule| rule.is_match(text)))
    };
    if any(answers.drop) {
        return Some(Answer::Drop);
    }
    if any(answers.park) {
        return Some(Answer::Park);
    }
    if let Some(at) = answers.choices.iter().position(|pattern| any(&[pattern])) {
        return Some(Answer::Choice(if at == 3 { usize::MAX } else { at }));
    }
    if any(answers.new) {
        return Some(Answer::New);
    }
    if any(answers.edit) {
        return Some(Answer::Edit);
    }
    if any(answers.yes) {
        return Some(Answer::Yes);
    }
    if any(answers.fit) {
        return Some(Answer::Fit);
    }
    if any(answers.no) {
        return Some(Answer::No);
    }
    if any(answers.details) {
        return Some(Answer::Details);
    }
    answers.redo.iter().find_map(|pattern| {
        let found = regex::Regex::new(pattern).ok()?.captures(text)?;
        Some(Answer::Redo { text: found.name("text")?.as_str().trim().to_string() })
    })
}

/// The answer to «Что-то ещё добавить?» (owner 20.11): a «нет» in any words — «нет, не надо», «ничего», «так оставь»,
/// «продолжай» — is «nothing to add», the work goes on (`No`); only words of stopping stop it (`Drop`); a lone «да»
/// asks what (`Yes`); anything else is the addition (`Redo`), without its «да, …». None — nothing said.
pub fn extra_answer(pack: &Pack, normalized: &str) -> Option<Answer> {
    let answers = &pack.work.rules.answers;
    let text = normalized.trim();
    if text.is_empty() {
        return None;
    }
    let found = |patterns: &[&str]| {
        patterns.iter().any(|pattern| regex::Regex::new(pattern).is_ok_and(|rule| rule.is_match(text)))
    };
    if found(answers.halt) {
        return Some(Answer::Drop);
    }
    if found(answers.add_yes) {
        return Some(Answer::Yes);
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.iter().all(|word| answers.enough.contains(word)) {
        return Some(Answer::No);
    }
    let lead = words.iter().take_while(|word| answers.lead.contains(word)).count();
    Some(Answer::Redo { text: words[lead..].join(" ") })
}

/// What is said of a thing put on the desk, and whether it is there.
/// What became of the thing, in the pack's words. A theme on the desk is a theme applied: the panel applies it on the
/// work's «decided» (voice/flow.tsx), so «На столе» would be the wrong word (test 19.11, remark 7).
/// A failed run's cause in words the person can act on, when the model's error names one: money, a rate limit, a key.
/// Anything else stays «подробности в пульте».
fn failure_why<'a>(error: &str, say: &'a super::pack::SayWords) -> Option<&'a str> {
    if MONEY.is_match(error) {
        Some(say.why_money)
    } else if RATE.is_match(error) {
        Some(say.why_rate)
    } else if KEY.is_match(error) {
        Some(say.why_key)
    } else {
        None
    }
}

/// The model's error says its key is out of money: OpenAI's 429 «insufficient_quota», a 402, «no credits».
static MONEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b402\b|insufficient|no credits|credit balance|billing|quota").expect("a valid pattern")
});
/// A rate limit, money apart (checked after `MONEY`).
static RATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b429\b|rate.?limit|too many requests").expect("a valid pattern"));
/// The key refused or missing.
static KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b40[13]\b|no api key|invalid.{0,10}api.?key|unauthori[sz]ed").expect("a valid pattern")
});

fn desk_words(say: &super::pack::SayWords, put: Put, theme: bool) -> (String, bool) {
    match put {
        Put::OnDesk { needs, .. } if needs.is_empty() && theme => (say.theme_on.to_string(), true),
        Put::OnDesk { needs, .. } if needs.is_empty() => (say.on_desk.to_string(), true),
        Put::OnDesk { needs, open } => {
            let needs: Vec<String> = needs.iter().map(|need| in_sentence(need)).collect();
            let template = if open { say.on_desk_needs_open } else { say.on_desk_needs_tile };
            (fill(template, &[("needs", &needs.join(", "))]), true)
        }
        Put::Approve { open } => (if open { say.approve_open } else { say.approve_tile }.to_string(), false),
        Put::Dangerous { open } => (if open { say.dangerous_open } else { say.dangerous_tile }.to_string(), false),
        Put::Shown { open } => (if open { say.shown } else { say.tile_waits }.to_string(), false),
        Put::Parked => (say.parked.to_string(), false),
        Put::Deleted => (say.deleted.to_string(), false),
    }
}

impl TalkLayer {
    /// A run ended with something to say: its line and question, on their own task (the facts read the install's
    /// plan).
    pub(super) fn finished(self: &Arc<Self>, work: Work) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            self.told(&work, work.state.as_str());
            return;
        };
        let layer = self.clone();
        handle.spawn(async move { layer.finish(work).await });
    }

    async fn finish(self: Arc<Self>, work: Work) {
        let pack = pack::of(self.lang(work.device()));
        let say = &pack.work.say;
        let title = work.title.clone();
        let Some(executor) = self.executor(&work.executor) else { return };
        if matches!(work.kind.as_str(), super::works::TOOL | super::works::TASK) {
            return self.finish_background(work, executor);
        }
        if work.kind == super::works::FOLDER {
            return self.finish_folder(work, executor);
        }
        let planned = self.planned.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&work.id);
        let (line, kind, end) = match work.state {
            WorkState::Failed => {
                let why = executor.facts(&work).await.failure.as_deref().and_then(|error| failure_why(error, say));
                let line = match why {
                    Some(why) => fill(say.failed_why_line, &[("title", &title), ("why", why)]),
                    None => fill(say.failed_line, &[("title", &title)]),
                };
                (line, None, None)
            }
            // A run that changed nothing is no «не доделано» either: what to do is asked, the next phrase says it.
            WorkState::Unfinished if executor.facts(&work).await.unchanged => {
                (fill(say.unchanged_line, &[("title", &title)]), None, None)
            }
            WorkState::Unfinished => (fill(say.unfinished_line, &[("title", &title)]), Some(Kind::Unfinished), None),
            // «На стол» was said while it was being made: there now, nothing asked.
            _ if planned && !work.again => {
                let by = ByVoice { device: work.device().to_string(), who: work.origin.who.clone() };
                let ready = fill(say.ready_line, &[("title", &title)]);
                match executor.put(&work, End::Desk, by).await {
                    Ok(put) => {
                        let (words, on_desk) = desk_words(say, put, work.face == Some(Face::Theme));
                        (format!("{ready} {words}"), None, on_desk.then_some(End::Desk))
                    }
                    Err(why) => (format!("{ready} {}", fill(say.failed, &[("why", &why)])), None, None),
                }
            }
            _ => {
                let facts = executor.facts(&work).await;
                let hosts = match facts.hosts.as_slice() {
                    [] => String::new(),
                    hosts => fill(say.hosts, &[("hosts", &hosts.join(", "))]),
                };
                let changed = fill(say.changed, &[("title", &title), ("text", &in_sentence(&work.brief))]);
                if work.again && facts.installed {
                    // A new version of a thing on the desk: there at once when it asks for nothing new.
                    let clean = facts.ready && facts.rights.is_empty() && facts.hosts.is_empty() && !facts.dangerous;
                    let by = ByVoice { device: work.device().to_string(), who: work.origin.who.clone() };
                    let put = if clean { executor.put(&work, End::Desk, by).await.ok() } else { None };
                    match put {
                        Some(Put::OnDesk { .. }) => {
                            self.edited(work.device(), work.target.as_deref().unwrap_or_default());
                            (format!("{changed} {}", say.on_desk), None, Some(End::Desk))
                        }
                        _ => (format!("{changed}{hosts}{}", say.apply), Some(Kind::Apply), None),
                    }
                } else {
                    let line = format!(
                        "{}{}{hosts}{}",
                        fill(say.ready_line, &[("title", &title)]),
                        (say.needs)(&facts.needs),
                        // A theme is applied, not put on the desk: its own question (test 19.11, remark 7).
                        if work.face == Some(Face::Theme) { say.theme_or_fit } else { say.desk_or_fit }
                    );
                    (line, Some(Kind::Ready), None)
                }
            }
        };
        let until = now_ms() + ANSWER_FOR.as_millis() as u64;
        let question = kind.map(|kind| Question::new(line.clone(), until, kind));
        let state = work.state;
        let Some(work) = self.works.update(&work.id, |work| {
            work.result = Some(WorkResult { line: line.clone(), end, ..WorkResult::default() });
            work.question = question;
            work.said_at = None;
            if end == Some(End::Desk) {
                work.state = WorkState::Done;
            }
        }) else {
            return;
        };
        self.told(&work, if end.is_some() { "done" } else { state.as_str() });
        if work.device() != COMPUTER {
            self.say_on_station(work.id.clone(), true);
        }
    }

    /// A background work's «Готово» (docs/plans/long-tools.md, point 3): the frame is the pack's — the title, «готово»
    /// or «не получилось», «подробности в пульте» — and the result in it is the model's retelling. A retelling that was
    /// cut asks «Рассказать подробнее?». Nothing to put anywhere: the work is done once it is said.
    fn finish_background(self: &Arc<Self>, work: Work, executor: Arc<dyn super::executor::Executor>) {
        let words = &pack::of(self.lang(work.device())).work.background;
        let told = executor.told(&work).unwrap_or_default();
        let title = work.title.clone();
        let (line, kind) = match (work.state, told.summary.as_deref()) {
            (WorkState::Failed, _) => {
                let why =
                    told.error.clone().unwrap_or_else(|| words.waiting_answer.trim_end_matches('.').to_lowercase());
                (fill(words.failed, &[("title", &title), ("why", &why)]), None)
            }
            (_, Some(summary)) if told.more.is_some() => {
                let ready = fill(words.ready, &[("title", &title), ("summary", summary)]);
                (format!("{ready} {}", words.more_q), Some(Kind::More))
            }
            (_, Some(summary)) => (fill(words.ready, &[("title", &title), ("summary", summary)]), None),
            (_, None) => (fill(words.ready_bare, &[("title", &title)]), None),
        };
        let until = now_ms() + ANSWER_FOR.as_millis() as u64;
        let question = kind.map(|kind| Question::new(line.clone(), until, kind));
        let failed = work.state == WorkState::Failed;
        let Some(work) = self.works.update(&work.id, |work| {
            work.result = Some(WorkResult {
                line: line.clone(),
                end: None,
                summary: told.summary.clone(),
                data: told.data.clone(),
            });
            work.question = question;
            work.said_at = None;
            if !failed {
                work.state = WorkState::Done;
            }
            work.ended_at.get_or_insert(now_ms());
        }) else {
            return;
        };
        self.told(&work, if failed { "failed" } else { "ready" });
        if work.device() != COMPUTER {
            self.say_on_station(work.id.clone(), true);
        }
    }

    /// «Верни как было» may undo this one: the device's voice put a new version of `target` on the desk now.
    fn edited(&self, device: &str, target: &str) {
        let mut edits = self.edits.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        edits.insert(device.to_string(), (target.to_string(), now_ms()));
    }

    /// The thing whose new version a device's voice put on the desk in the last minutes, for «верни как было».
    pub fn recent_edit(&self, device: &str) -> Option<String> {
        let edits = self.edits.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        edits.get(device).filter(|(_, at)| now_ms().saturating_sub(*at) < UNDO_MS).map(|(target, _)| target.clone())
    }

    /// The stations, for «Готово», once the session registry stands (a test puts its own).
    pub fn set_turns(&self, turns: Arc<dyn crate::plugins::skill::TurnStation>) {
        *self.turns.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(turns);
    }

    /// The device's next conversation ends: a line its station did not say to its end is said once more.
    pub fn follow_requests(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        self.events.on("talk.unsaid", move |event| {
            let Some(layer) = me.upgrade() else { return };
            let GatewayEvent::RequestUpdated { request } = event.as_ref() else { return };
            if request.status == RequestStatus::Active || request.kind == RequestKind::Say {
                return;
            }
            let pending = layer.unsaid.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&request.device);
            if let Some(work) = pending {
                layer.say_on_station(work, false);
            }
        });
    }

    /// The work's line on its station, once the station is free; `again` — not said to its end, it waits for the
    /// device's next conversation (once).
    pub(super) fn say_on_station(self: &Arc<Self>, id: String, again: bool) {
        let turns = self.turns.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let (Some(turns), Ok(handle)) = (turns, tokio::runtime::Handle::try_current()) else { return };
        let Some(work) = self.works.get(&id) else { return };
        let layer = self.clone();
        handle.spawn(async move {
            let device = work.device().to_string();
            let line = work.result.as_ref().map(|result| result.line.clone()).unwrap_or_default();
            let began = Instant::now();
            while turns.busy(&device) {
                if began.elapsed() > READY_WAITS {
                    tracing::info!(target: "talk", "work {id}: {device} was busy for 10 minutes — the tile waits");
                    if again {
                        layer.unsaid.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(device, id);
                    }
                    return;
                }
                tokio::time::sleep(BUSY_POLL).await;
            }
            // Still the same line when the station calls back: not decided, not said meanwhile.
            let still = {
                let (layer, id, line) = (layer.clone(), id.clone(), line.clone());
                Arc::new(move || {
                    layer.works.get(&id).is_some_and(|work| {
                        work.said_at.is_none() && work.result.as_ref().is_some_and(|result| result.line == line)
                    })
                }) as Arc<dyn Fn() -> bool + Send + Sync>
            };
            let Some(intent) = super::rules::TalkRules::intent_of(&super::rules::Heard::Ready { work: id.clone() }) else {
                return;
            };
            let spoken = turns.ask(&device, intent.intent, format!("«готово» · работа {id}"), still).await;
            if spoken {
                if let Some(work) = layer.works.update(&id, |work| work.said_at = Some(now_ms())) {
                    layer.told(&work, "said");
                }
            } else if again {
                tracing::info!(target: "talk", "work {id}: «готово» was not said to its end on {device}: once more after its next talk");
                layer.unsaid.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(device, id);
            }
        });
    }

    /// The station calls back for a work's line: the line, and its question opens on the device.
    pub(super) fn ready_said(&self, id: &str, device: &str) -> Option<String> {
        let work = self.works.get(id)?;
        let line = work.result.as_ref().map(|result| result.line.clone()).filter(|line| !line.is_empty())?;
        self.ask_aloud(&work, device, false);
        Some(line)
    }

    /// The thing on the desk, by voice: what became of it, in the pack's words.
    pub(super) async fn desk(&self, origin: &Origin, work: &Work) -> Result<String, Refusal> {
        let say = &pack::of(self.lang(&origin.device)).work.say;
        // Still being made, or waiting for its turn: not the template now — the thing, once it is ready (finish).
        if work.going() || work.state.queued() {
            self.planned.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(work.id.clone());
            return Ok(say.desk_later.to_string());
        }
        let executor = self.executor(&work.executor).ok_or_else(|| Refusal::Failed("its executor is gone".into()))?;
        let by = ByVoice { device: origin.device.clone(), who: origin.who.clone() };
        let put = executor.put(work, End::Desk, by).await.map_err(Refusal::Failed)?;
        Ok(desk_words(say, put, work.face == Some(Face::Theme)).0)
    }

    /// The answer to the device's question.
    pub(super) async fn answer(&self, origin: Origin, id: &str, kind: Kind, again: bool, answer: Answer) -> String {
        let device = origin.device.clone();
        let pack = pack::of(self.lang(&device));
        let say = &pack.work.say;
        // The device's question is the one answered when it is about this work — by voice it always is (rules.rs
        // answers the question open); a button on the page «Работы» answers its own work and leaves another be.
        let pending = self.asked.take_of(&device, id).and_then(|question| question.pending);
        // «Нет» is the person's «not now»: what was named for it, nothing more; any other answer goes where it was meant.
        self.ended_asking(id, if answer == Answer::No { Outcome::Declined } else { Outcome::Answered });
        let refused = |refusal: Refusal| refusal.for_person(pack);
        // A wish or a change that waited for this answer: it is done now.
        match (kind, answer.clone()) {
            // «Запуск платный: … Запускать?» — `id` is the run's key.
            (Kind::Run, answer) => {
                let yes = matches!(answer, Answer::Yes | Answer::Fit | Answer::Edit | Answer::New);
                return match self.background() {
                    Some(executor) => executor.answer_paid(origin, id, yes),
                    None => say.what.to_string(),
                };
            }
            (Kind::More, Answer::Yes | Answer::Details | Answer::Fit) => {
                let words = &pack.work.background;
                let more =
                    self.works.get(id).and_then(|work| self.background().and_then(|executor| executor.more(&work)));
                return more.unwrap_or_else(|| words.no_more.to_string());
            }
            (Kind::More, _) => return pack.work.background.fine.to_string(),
            // «Что-то ещё добавить?»: only a stop stops; «нет, не надо» lets the work go on; the rest goes into it.
            (Kind::Extra, Answer::Drop) => {
                let label = format!("{}{id}", pack.work.label);
                return self.act(&origin, "stop", Some(&label)).await.unwrap_or_else(refused);
            }
            (Kind::Extra, Answer::Yes) => {
                let asked = Pending { question: Some(say.extra_what.to_string()), ..Pending::default() };
                self.asked.ask_with(&device, id, Kind::Extra, asked);
                return say.extra_what.to_string();
            }
            (Kind::Extra, Answer::Redo { text }) => {
                let label = format!("{}{id}", pack.work.label);
                return self.add(origin, &text, Some(&label), false).await.unwrap_or_else(refused);
            }
            (Kind::Extra, _) => return say.going_on.to_string(),
            // A plan's limit holds a work in a folder: «подождать» — it waits and goes on by itself; «остановить».
            (Kind::Wait, Answer::No | Answer::Drop) => {
                let label = format!("{}{id}", pack.work.label);
                return self.act(&origin, "stop", Some(&label)).await.unwrap_or_else(refused);
            }
            (Kind::Wait, _) => {
                if let Some(work) = self.works.update(id, |work| work.question = None) {
                    self.told(&work, "answered");
                }
                return pack.work.folder.waiting.to_string();
            }
            (Kind::Similar, Answer::New | Answer::No) => {
                let Some(wish) = pending else { return say.what.to_string() };
                return self.start(origin, &wish.text, wish.face, wish.title.as_deref(), true).unwrap_or_else(refused);
            }
            (Kind::Similar, Answer::Edit | Answer::Yes | Answer::Redo { .. }) => {
                let text = match (answer, pending) {
                    (Answer::Redo { text }, _) => text,
                    (_, Some(wish)) => wish.text,
                    _ => return say.what.to_string(),
                };
                let label = format!("{}{id}", pack.work.label);
                return self.add(origin, &text, Some(&label), false).await.unwrap_or_else(refused);
            }
            (Kind::Copy, Answer::Yes | Answer::Edit) => {
                let Some(change) = pending else { return say.what.to_string() };
                return self.add(origin, &change.text, change.thing.as_deref(), true).await.unwrap_or_else(refused);
            }
            (Kind::Copy, Answer::No | Answer::Drop | Answer::New) => return say.not_copied.to_string(),
            // «Какую?»: the action that waited, on the thing chosen («новую» — the newest, named first).
            (Kind::Which, Answer::Choice(_) | Answer::New) => {
                let Some(waited) = pending else { return say.what.to_string() };
                let at = match answer {
                    Answer::Choice(usize::MAX) => waited.choices.len().saturating_sub(1),
                    Answer::Choice(at) => at,
                    _ => 0,
                };
                let Some(chosen) = waited.choices.get(at) else { return say.what.to_string() };
                let label = format!("{}{chosen}", pack.work.label);
                return match waited.act.as_deref() {
                    Some("add") => self.add(origin, &waited.text, Some(&label), false).await,
                    Some(act) => self.act(&origin, act, Some(&label)).await,
                    None => Ok(say.what.to_string()),
                }
                .unwrap_or_else(refused);
            }
            // «Сделать заново?» about a thing that is gone: its wish, as a new thing.
            (Kind::Remake, Answer::Yes | Answer::New | Answer::Edit) => {
                let Some(wish) = pending else { return say.what.to_string() };
                return self.start(origin, &wish.text, wish.face, wish.title.as_deref(), true).unwrap_or_else(refused);
            }
            (Kind::Remake, Answer::No | Answer::Drop) => return say.not_remade.to_string(),
            (Kind::Similar | Kind::Copy | Kind::Which | Kind::Remake, _) => return say.what.to_string(),
            _ => {}
        }
        let Some(work) = self.works.get(id) else { return say.nothing.to_string() };
        let label = format!("{}{}", pack.work.label, work.id);
        let decided = |end: End| {
            self.works.update(id, |work| {
                let line = work.result.take().map(|result| result.line).unwrap_or_default();
                work.result = Some(WorkResult { line, end: Some(end), ..WorkResult::default() });
                work.question = None;
            })
        };
        match (kind, answer) {
            (Kind::Delete, Answer::Yes) => match self.put_by(&work, &device, End::Declined).await {
                Ok(_) => say.deleted.to_string(),
                Err(refusal) => refused(refusal),
            },
            (Kind::Delete, _) => say.not_deleted.to_string(),
            (Kind::Unfinished, Answer::Yes) => {
                self.add(origin, say.finish_brief, Some(&label), false).await.unwrap_or_else(refused)
            }
            (Kind::Unfinished, Answer::No) => say.not_deleted.to_string(),
            (_, Answer::Yes) => self.desk(&origin, &work).await.unwrap_or_else(refused),
            (_, Answer::Fit) => match self.put_by(&work, &device, End::Fitting).await {
                Ok(Put::Shown { open }) => {
                    decided(End::Fitting);
                    if open { say.shown } else { say.tile_waits }.to_string()
                }
                Ok(_) => say.tile_waits.to_string(),
                Err(refusal) => refused(refusal),
            },
            (_, Answer::No) => {
                decided(End::Fitting);
                say.kept.to_string()
            }
            (_, Answer::Park) => match self.put_by(&work, &device, End::Parked).await {
                Ok(_) => say.parked.to_string(),
                Err(refusal) => refused(refusal),
            },
            (_, Answer::Details) => {
                let details = self
                    .executor(&work.executor)
                    .and_then(|executor| executor.details(&work))
                    .unwrap_or_else(|| say.no_details.to_string());
                // The question once more, once.
                match work.question.as_ref().and_then(|question| question.kind) {
                    Some(_) if !again => {
                        self.ask_aloud(&work, &device, true);
                        let question = work.result.as_ref().map(|result| result.line.clone()).unwrap_or_default();
                        let ask = question.rsplit_once(". ").map(|(_, ask)| ask.to_string()).unwrap_or(question);
                        format!("{details} {ask}")
                    }
                    _ => details,
                }
            }
            (_, Answer::Redo { text }) => self.add(origin, &text, Some(&label), false).await.unwrap_or_else(refused),
            (_, Answer::Drop) => {
                self.asked.ask(&device, id, Kind::Delete, false);
                say.delete_q.to_string()
            }
            // «Новую», «поправь», «первая» without saying what, to a question about a ready thing.
            (_, Answer::New | Answer::Edit | Answer::Choice(_)) => say.what.to_string(),
        }
    }

    /// A work's question answered by a button on the page «Работы» (docs/plans/router-and-works.md, decision 17): the
    /// same answer as one said aloud — «да», «примерить», «в черновики», «подробнее», «нет» — to the question its
    /// record waits on. None — the work asks nothing a button answers: the questions that wait for words (which one,
    /// a new one or this one) are answered aloud.
    pub(super) async fn answer_work(&self, origin: Origin, id: &str, answer: Answer) -> Option<String> {
        let work = self.works.get(id)?;
        let question = work.question.as_ref().filter(|question| question.waits())?;
        let kind = match question.kind? {
            // A work in a folder going on asks it: wait for the limit, or stop.
            Kind::Wait if work.going() => Kind::Wait,
            kind @ (Kind::Ready | Kind::Unfinished | Kind::Apply | Kind::Delete | Kind::More) if work.awaiting() => {
                kind
            }
            _ => return None,
        };
        self.acted_on(&origin.device, id);
        Some(self.answer(origin, id, kind, false, answer).await)
    }

    /// Puts a work's thing where the person said, by the device's voice.
    async fn put_by(&self, work: &Work, device: &str, end: End) -> Result<Put, Refusal> {
        let executor = self.executor(&work.executor).ok_or_else(|| Refusal::Failed("its executor is gone".into()))?;
        executor.put(work, end, ByVoice { device: device.to_string(), who: None }).await.map_err(Refusal::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::text::Lang;

    /// 19.11: OpenAI's «no credits» made a game «не получилось. Подробности в пульте» — the cause is said now.
    #[test]
    fn names_a_failures_plain_cause_money_a_limit_a_key_and_nothing_else() {
        let say = &pack::of(Lang::Ru).work.say;
        let openai = r#"HTTP 429: { "error": { "message": "You have no credits remaining.", "type": "insufficient_quota", "code": "credit_balance_exhausted" } }"#;
        assert_eq!(failure_why(openai, say), Some(say.why_money));
        assert_eq!(
            failure_why("429 rate_limit_error: Number of request tokens has exceeded your limit", say),
            Some(say.why_rate)
        );
        assert_eq!(failure_why("401 invalid x-api-key", say), Some(say.why_key));
        assert_eq!(failure_why("no API key: add it to the provider in the panel", say), Some(say.why_key));
        assert_eq!(failure_why("Anthropic stream broke: 14029 tokens", say), None);
        assert_eq!(
            fill(say.failed_why_line, &[("title", "Новая игра"), ("why", say.why_money)]),
            "Новая игра: не получилось — на ключе «Мышления» кончились деньги. Подробности в пульте."
        );
    }

    #[test]
    fn reads_the_answers_to_the_question_and_lets_go_of_the_rest() {
        let ru = pack::of(Lang::Ru);
        let read = |text: &str| answer_of(ru, text);
        assert_eq!(read("да"), Some(Answer::Yes));
        assert_eq!(read("на стол"), Some(Answer::Yes));
        assert_eq!(read("поставь"), Some(Answer::Yes));
        assert_eq!(read("примерить"), Some(Answer::Fit));
        assert_eq!(read("нет"), Some(Answer::No));
        assert_eq!(read("потом"), Some(Answer::No));
        assert_eq!(read("в черновики"), Some(Answer::Park));
        assert_eq!(read("подробнее"), Some(Answer::Details));
        assert_eq!(read("переделай покрупнее"), Some(Answer::Redo { text: "переделай покрупнее".into() }));
        assert_eq!(read("а слова сделай покрупнее"), Some(Answer::Redo { text: "слова сделай покрупнее".into() }));
        assert_eq!(read("не надо"), Some(Answer::Drop));
        assert_eq!(read("какая погода"), None, "not the question's");
        let en = pack::of(Lang::En);
        assert_eq!(answer_of(en, "yes"), Some(Answer::Yes));
        assert_eq!(answer_of(en, "try it on"), Some(Answer::Fit));
        assert_eq!(answer_of(en, "what is the weather"), None);
    }

    /// 20.11: to «Что-то ещё добавить?» a «нет» in any words is «nothing to add» — the work goes on; it stops only on
    /// words of stopping; anything else is the addition.
    #[test]
    fn to_anything_else_a_no_in_any_words_lets_the_work_go_on_and_only_a_stop_stops_it() {
        let read = |lang: Lang, text: &str| extra_answer(pack::of(lang), &crate::skills::text::normalize(text));
        for no in [
            "Нет.",
            "Нет, не надо.",
            "Не надо",
            "Нет, не надо ничего больше добавлять",
            "Ничего",
            "Больше ничего не надо",
            "Ничего не добавляй",
            "Всё",
            "Всё, хватит",
            "Так оставь",
            "Пусть будет так",
            "Нет, спасибо",
            "Продолжай",
            "Нет, делай дальше",
            "Больше ничего не надо делать",
        ] {
            assert_eq!(read(Lang::Ru, no), Some(Answer::No), "{no}");
        }
        for stop in [
            "Останови",
            "Отмени",
            "Нет, отмени виджет",
            "Стоп",
            "Прекрати",
            "Не делай",
            "Нет, не делай виджет",
            "Не надо делать",
            "Хватит делать",
            "Я передумал",
        ] {
            assert_eq!(read(Lang::Ru, stop), Some(Answer::Drop), "{stop}");
        }
        assert_eq!(read(Lang::Ru, "Да"), Some(Answer::Yes));
        assert_eq!(read(Lang::Ru, "Добавь"), Some(Answer::Yes));
        assert_eq!(read(Lang::Ru, "Да, сделай фон синим"), Some(Answer::Redo { text: "сделай фон синим".into() }));
        assert_eq!(
            read(Lang::Ru, "Нет, но пусть будет секундная стрелка"),
            Some(Answer::Redo { text: "но пусть будет секундная стрелка".into() })
        );
        assert_eq!(read(Lang::Ru, "И ещё будильник"), Some(Answer::Redo { text: "и еще будильник".into() }));
        assert_eq!(read(Lang::Ru, ""), None);

        for no in [
            "No",
            "No, thanks",
            "Nothing",
            "Nothing else",
            "No, don't add anything",
            "That's all",
            "That's it",
            "No need",
            "Keep going",
            "It's fine",
        ] {
            assert_eq!(read(Lang::En, no), Some(Answer::No), "{no}");
        }
        for stop in ["Stop", "Cancel it", "No, stop it", "Don't make it", "Do not build it", "I changed my mind"] {
            assert_eq!(read(Lang::En, stop), Some(Answer::Drop), "{stop}");
        }
        assert_eq!(read(Lang::En, "Yes"), Some(Answer::Yes));
        assert_eq!(
            read(Lang::En, "Yes, make the background blue"),
            Some(Answer::Redo { text: "make the background blue".into() })
        );
    }
}

/// Step 5 of docs/plans/audio-interface.md on a whole gateway, with a station that answers every push: «Готово»
/// said where it was asked, «да» without the wake word puts the thing on the desk, a phrase the question cannot read
/// goes on as any other, a new version of the thing on the desk goes there at once and «верни как было» undoes it, a
/// line not said to its end is said once more after the next talk, a key nobody filled in leaves it on the desk
/// waiting for it.
#[cfg(test)]
mod gateway {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, OnceLock, Weak};

    use futures::future::BoxFuture;

    use crate::app::App;
    use crate::config::{Config, Env};
    use crate::core::events::GatewayEvent;
    use crate::core::requests::{RequestKind, RequestStart, RequestStatus};
    use crate::plugins::skill::TurnStation;
    use crate::plugins::workshop::meta::Stage;
    use crate::skills::capabilities::{CapabilityName, Intent};
    use crate::skills::router::{Decision, ModelState, Resolution, RouteInput};
    use crate::skills::skill::ExecutionContext;
    use crate::talk::asked::Kind;
    use crate::talk::works::{End, WorkState};
    use crate::utils::time::now_ms;

    /// A station that runs every push as the real one calls back, and keeps what it said.
    struct Station {
        app: OnceLock<Weak<App>>,
        said: Arc<Mutex<Vec<String>>>,
        /// Whether a push is said to its end; false — cut by «стоп».
        to_the_end: Arc<AtomicBool>,
    }

    impl TurnStation for Station {
        fn ask(
            &self,
            device: &str,
            intent: Intent,
            _label: String,
            still: Arc<dyn Fn() -> bool + Send + Sync>,
        ) -> BoxFuture<'static, bool> {
            let (app, said, to_the_end) =
                (self.app.get().and_then(Weak::upgrade), self.said.clone(), self.to_the_end.clone());
            let device = device.to_string();
            Box::pin(async move {
                let (Some(app), true) = (app, still()) else { return false };
                let Some(skill) = app.skills.router.implementation(&intent) else { return false };
                let context = ExecutionContext::new(&device, chrono::Utc::now());
                let Ok(outcome) = skill.execute(&intent, &context).await else { return false };
                said.lock().unwrap().push(outcome.speech.unwrap_or_default());
                to_the_end.load(Ordering::SeqCst)
            })
        }
        fn hearing(&self, _device: &str) -> bool {
            false
        }
        fn ringing(&self, _device: &str) -> bool {
            false
        }
    }

    fn env(dir: &std::path::Path) -> Env {
        let text = |sub: &str| dir.join(sub).to_string_lossy().into_owned();
        Env::from_pairs([
            ("STATE_DIR", String::new()),
            ("DIALOG_PROVIDER", "echo".into()),
            ("PACKAGES_DIR", text("packages")),
            ("DATA_DIR", text("data")),
            ("VOICE_LAB_DIR", text("voice-lab")),
            ("TTS_MODELS_DIR", text("voices")),
        ])
    }

    /// A phrase of the kitchen's station: the rule that took it and, when it is ours, what the station says.
    async fn phrase(app: &Arc<App>, text: &str, wake_word: bool) -> (Option<&'static str>, CapabilityName, String) {
        let input = RouteInput {
            text: text.into(),
            device_tag: "kitchen".into(),
            wake_word,
            model: ModelState { enabled: true, tools: true, ..Default::default() },
            now: now_ms(),
            ..RouteInput::default()
        };
        match app.skills.router.decide(&input) {
            Decision::Intent { intent, resolution, rule, .. } => {
                let said = match (&resolution, rule) {
                    (Resolution::Own(skill), Some(_)) if intent.capability == CapabilityName::Plugins => {
                        let context = ExecutionContext {
                            text: text.into(),
                            ..ExecutionContext::new("kitchen", chrono::Utc::now())
                        };
                        skill.execute(&intent, &context).await.unwrap().speech.unwrap_or_default()
                    }
                    _ => String::new(),
                };
                (rule, intent.capability, said)
            }
            Decision::Model { .. } => (None, CapabilityName::Chat, String::new()),
            other => panic!("{other:?}"),
        }
    }

    async fn heard(station: &Station, count: usize) -> String {
        for _ in 0..400 {
            if let Some(said) = station.said.lock().unwrap().get(count - 1).cloned() {
                return said;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!(
            "the station said {} line(s), not {count}: {:?}",
            station.said.lock().unwrap().len(),
            station.said.lock().unwrap()
        );
    }

    async fn until(what: &str, check: impl Fn() -> bool) {
        for _ in 0..400 {
            if check() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("never: {what}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ready_is_said_where_it_was_asked_and_yes_puts_it_on_the_desk() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::build(Config::for_tests(&env(dir.path()))).unwrap();
        let station = Arc::new(Station {
            app: OnceLock::new(),
            said: Arc::default(),
            to_the_end: Arc::new(AtomicBool::new(true)),
        });
        let _ = station.app.set(Arc::downgrade(&app));
        app.talk.set_turns(station.clone());
        let mut events = app.events.subscribe();

        // The old template's words: a work without the model; the agent finishes, the gateway checks it.
        let (rule, _, said) = phrase(&app, "Алиса, сделай виджет часы с секундами", true).await;
        assert_eq!(
            (rule, said.as_str()),
            (
                Some("talk:works"),
                "Делаю: часы с секундами. Около минуты. Можно добавлять детали по ходу. Что-то ещё добавить?"
            )
        );
        let ready = heard(&station, 1).await;
        assert_eq!(
            ready, "Часы с секундами: готово. На стол или примерить?",
            "the gateway's line, not the agent's words"
        );
        let work = app.talk.works().all(Some("kitchen"), None).remove(0);
        assert_eq!(work.state, WorkState::Ready);
        until("said to its end", || app.talk.works().get(&work.id).is_some_and(|work| work.said_at.is_some())).await;
        let draft = work.target.clone().unwrap();

        // «да» without the wake word: on the desk.
        let (rule, _, said) = phrase(&app, "да", false).await;
        assert_eq!((rule, said.as_str()), (Some("talk:works"), "На столе."));
        assert!(app.workshop.host().installed(&draft).is_some(), "installed by voice");
        until("decided", || {
            app.talk
                .works()
                .get(&work.id)
                .is_some_and(|work| work.result.as_ref().is_some_and(|result| result.end == Some(End::Desk)))
        })
        .await;
        assert_eq!(app.workshop.card(&draft).stage, Stage::Installed);

        // A phrase the question cannot read is not the question's: the weather answers it.
        app.talk.asked.ask("kitchen", &work.id, Kind::Ready, false);
        let (rule, capability, _) = phrase(&app, "какая погода", false).await;
        assert_ne!(rule, Some("talk:works"));
        assert_eq!(capability, CapabilityName::Weather);

        // A new version of the thing on the desk: there at once, «верни как было» undoes it.
        let (_, _, said) = phrase(&app, "Алиса, и еще сделай цифры крупнее", true).await;
        assert_eq!(said, "Поправляю «Часы с секундами»: версия 2. Скажу, когда будет готово.");
        let edited = heard(&station, 2).await;
        assert_eq!(edited, "Часы с секундами: сделай цифры крупнее. На столе.");
        assert_eq!(app.talk.recent_edit("kitchen").as_deref(), Some(draft.as_str()));
        assert_eq!(app.talk.recent_edit("hall"), None, "another device did not change it");
        if app.history.is_some() {
            let (rule, _, _) = phrase(&app, "Алиса, верни как было", true).await;
            assert_eq!(rule, Some("history:voice"));
        }

        // A line cut short is said once more after the device's next talk — once.
        station.to_the_end.store(false, Ordering::SeqCst);
        let id = work.id.clone();
        app.talk.works().update(&id, |work| work.said_at = None);
        app.talk.say_on_station(id.clone(), true);
        heard(&station, 3).await;
        let talk = || {
            let trip = app.requests.begin_in(
                RequestStart {
                    device: "kitchen".into(),
                    session: None,
                    kind: RequestKind::Voice,
                    text: Some("который час".into()),
                    parent: None,
                },
                "own",
            );
            trip.finish(RequestStatus::Answered, None, None);
        };
        talk();
        heard(&station, 4).await;
        talk();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(station.said.lock().unwrap().len(), 4, "once more, not again and again");

        let mut changes = Vec::new();
        while let Ok(event) = events.try_recv() {
            if let GatewayEvent::WorkChanged { work, change, .. } = event.as_ref() {
                changes.push(format!("{work}:{change}"));
            }
        }
        assert!(changes.contains(&format!("{}:ready", work.id)), "{changes:?}");
        assert!(changes.contains(&format!("{}:said", work.id)), "{changes:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_key_nobody_filled_in_leaves_it_on_the_desk_waiting_and_the_panel_opens_its_fields() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::build(Config::for_tests(&env(dir.path()))).unwrap();
        let station = Arc::new(Station {
            app: OnceLock::new(),
            said: Arc::default(),
            to_the_end: Arc::new(AtomicBool::new(true)),
        });
        let _ = station.app.set(Arc::downgrade(&app));
        app.talk.set_turns(station.clone());
        let mut events = app.events.subscribe();
        phrase(&app, "Алиса, сделай виджет дела на сегодня", true).await;
        heard(&station, 1).await;
        let work = app.talk.works().all(Some("kitchen"), None).remove(0);
        let draft = work.target.clone().unwrap();
        // The agent's manifest asks for a key.
        let mut manifest: serde_json::Value =
            serde_json::from_str(&app.workshop.read(&draft, "manifest.json").unwrap()).unwrap();
        manifest["settings"] =
            serde_json::json!([{ "key": "token", "type": "string", "title": "Ключ Todoist", "secret": true }]);
        app.workshop.write(&draft, "manifest.json", &manifest.to_string()).unwrap();

        app.talk.asked.ask("kitchen", &work.id, Kind::Ready, false);
        let (_, _, said) = phrase(&app, "на стол", false).await;
        assert_eq!(said, "На столе. Чтобы заработало, впишите ключ Todoist в пульте.");
        assert!(app.workshop.host().installed(&draft).is_some());
        let view = app.workshop.host().view_one(&draft).await.unwrap();
        assert_eq!(view.waits, ["Ключ Todoist"], "«ждёт ключ» on its tile");
        let mut opened = false;
        while let Ok(event) = events.try_recv() {
            if let GatewayEvent::WorkshopChanged { draft: changed, change } = event.as_ref() {
                opened |= changed == &draft && change == "open:needs";
            }
        }
        assert!(opened, "the panel opens the fields to fill in");
    }
}
