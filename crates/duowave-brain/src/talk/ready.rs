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
    /// To a work's «подождать или остановить?»: it waits.
    Wait,
    /// To the same question, or to a work's budget question: it stops.
    Stop,
    /// To a work's «бюджет кончился: добавить ещё или остановить?»: as much again.
    Raise,
    /// To a limit's question: the cheaper set offered (stage 5 of docs/plans/work-systems.md).
    Switch,
    /// To a limit's question: the work goes to that system, with what is done.
    HandTo {
        to: String,
    },
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
    if any(answers.switch) {
        return Some(Answer::Switch);
    }
    let hand = answers.hand.iter().find_map(|pattern| {
        let found = regex::Regex::new(pattern).ok()?.captures(text)?;
        Some(found.name("to")?.as_str().trim().to_string())
    });
    if let Some(to) = hand.filter(|to| !to.is_empty()) {
        return Some(Answer::HandTo { to });
    }
    if any(answers.new) {
        return Some(Answer::New);
    }
    if any(answers.edit) {
        return Some(Answer::Edit);
    }
    if any(answers.wait) {
        return Some(Answer::Wait);
    }
    if any(answers.stop) {
        return Some(Answer::Stop);
    }
    if any(answers.raise) {
        return Some(Answer::Raise);
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
pub fn failure_why<'a>(error: &str, say: &'a super::pack::SayWords) -> Option<&'a str> {
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
        Put::InSystem { system } => (fill(say.opened_in, &[("system", &system)]), false),
        Put::NoOpen { system } => (fill(say.no_open, &[("system", &system)]), false),
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
        // A work system's work ends as a background one: its summary from the system, «подробнее» for the rest.
        if matches!(work.kind.as_str(), super::works::TOOL | super::works::TASK | super::works::SYSTEM) {
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
            WorkState::Unfinished => {
                let facts = executor.facts(&work).await;
                if facts.overthought {
                    // The model's thinking took its answer whole: «не доделано», and why.
                    (fill(say.overthought_line, &[("title", &title)]), Some(Kind::Unfinished), None)
                } else if facts.unchanged {
                    // A run that changed nothing is no «не доделано» either: what to do is asked, the next phrase
                    // says it.
                    (fill(say.unchanged_line, &[("title", &title)]), None, None)
                } else {
                    (fill(say.unfinished_line, &[("title", &title)]), Some(Kind::Unfinished), None)
                }
            }
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
        if touched_on(&work) != COMPUTER {
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
        // A work begun outside is quiet unless watched (D1 of docs/plans/work-systems.md): its end is kept, not said.
        let quiet = work.outside && !work.watched;
        let (line, kind) = match (work.state, told.summary.as_deref()) {
            (WorkState::Failed, _) => {
                // A system that refused to begin says why in the end it reported (T3 Code «the project has no default
                // model», the hand check of 27.11); «жду ответа сервиса» only when nobody said anything.
                let reported =
                    work.result.as_ref().map(|result| result.line.trim().to_string()).filter(|line| !line.is_empty());
                let why = told
                    .error
                    .clone()
                    .or(reported)
                    .unwrap_or_else(|| words.waiting_answer.trim_end_matches('.').to_lowercase());
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
        let question = kind.filter(|_| !quiet).map(|kind| Question::new(line.clone(), until, kind));
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
        if quiet {
            self.told(&work, "quiet");
            return;
        }
        self.told(&work, if failed { "failed" } else { "ready" });
        if touched_on(&work) != COMPUTER {
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
    pub fn set_turns(&self, turns: Arc<dyn crate::ear::TurnStation>) {
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
    pub fn say_on_station(self: &Arc<Self>, id: String, again: bool) {
        let turns = self.turns.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let (Some(turns), Ok(handle)) = (turns, tokio::runtime::Handle::try_current()) else { return };
        let Some(work) = self.works.get(&id) else { return };
        let layer = self.clone();
        handle.spawn(async move {
            // Where it was touched last (Н1): the device its last phrase came from, else where it began.
            let device = touched_on(&work).to_string();
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
    pub async fn answer(&self, origin: Origin, id: &str, kind: Kind, again: bool, answer: Answer) -> String {
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
            // «Разбудить?» — any agreement wakes the Турбо; anything else leaves it to the person (owner 20.11, 5).
            (Kind::Wake, answer) => {
                let yes = matches!(answer, Answer::Yes | Answer::Fit);
                let helper = pending.as_ref().and_then(|pending| pending.thing.clone()).unwrap_or_default();
                return self.wake_answer(&helper, yes).await;
            }
            // «Запуск платный: … Запускать?» — `id` is the run's key.
            (Kind::Run, answer) => {
                let yes = matches!(answer, Answer::Yes | Answer::Fit | Answer::Edit | Answer::New);
                return match self.background() {
                    Some(executor) => executor.answer_paid(origin, id, yes),
                    None => say.what.to_string(),
                };
            }
            // «… спрашивает: …. Разрешить?» — a work system's question of rights: «да» allows, «нет» refuses; only the
            // home's admin answers, and a dangerous one only by a tap (D3 of docs/plans/work-systems.md).
            (Kind::Permission, answer) => {
                let words = &pack.work.system;
                let Some(work) = self.works.get(id) else { return words.gone.to_string() };
                if !origin.admin() {
                    return words.admin_only.to_string();
                }
                let Some(executor) = self.executor(&work.executor) else { return words.gone.to_string() };
                let yes = matches!(answer, Answer::Yes | Answer::Fit);
                let by_voice = origin.via == super::origin::Via::Voice;
                return match executor.answer_question(&work, yes, by_voice) {
                    Ok(()) => {
                        if let Some(work) = self.works.update(id, |work| work.question = None) {
                            self.told(&work, "answered");
                        }
                        (if yes { words.allowed } else { words.denied }).to_string()
                    }
                    Err(super::executor::NotAnswered::Dangerous) => words.tap_only.to_string(),
                    Err(super::executor::NotAnswered::Gone) => {
                        if let Some(work) = self.works.update(id, |work| work.question = None) {
                            self.told(&work, "answered");
                        }
                        words.gone.to_string()
                    }
                };
            }
            (Kind::More, Answer::Yes | Answer::Details | Answer::Fit) => {
                let words = &pack.work.background;
                let more = self.works.get(id).and_then(|work| self.more_of(&work));
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
            // A limit's question with more than waiting (stage 5): a cheaper set, another system.
            (Kind::Wait, Answer::Switch) => {
                return self.folder_switch(id).unwrap_or_else(|| pack.work.folder.no_switch.to_string());
            }
            (Kind::Wait | Kind::Budget, Answer::HandTo { to }) => {
                return self.hand_over(origin, id, &to).await.unwrap_or_else(refused);
            }
            // «С планом»: «да» — by the plan; «нет» — stop.
            (Kind::Plan, Answer::Yes | Answer::Fit | Answer::Edit) => {
                return self.folder_plan_go(id).unwrap_or_else(|| say.what.to_string());
            }
            (Kind::Plan, Answer::No | Answer::Drop | Answer::Stop) => {
                let label = format!("{}{id}", pack.work.label);
                return self.act(&origin, "stop", Some(&label)).await.unwrap_or_else(refused);
            }
            (Kind::Plan, _) => return say.what.to_string(),
            (Kind::Wait, Answer::No | Answer::Drop | Answer::Stop) => {
                let label = format!("{}{id}", pack.work.label);
                return self.act(&origin, "stop", Some(&label)).await.unwrap_or_else(refused);
            }
            (Kind::Wait, _) => {
                if let Some(work) = self.works.update(id, |work| work.question = None) {
                    self.told(&work, "answered");
                }
                return pack.work.folder.waiting.to_string();
            }
            // A work's budget is used up: «добавить» — as much again, and it goes on; «остановить».
            (Kind::Budget, Answer::No | Answer::Drop | Answer::Stop) => {
                let label = format!("{}{id}", pack.work.label);
                return self.act(&origin, "stop", Some(&label)).await.unwrap_or_else(refused);
            }
            (Kind::Budget, Answer::Yes | Answer::Raise) => {
                return self.folder_raise(id).unwrap_or_else(|| say.what.to_string());
            }
            (Kind::Budget, _) => return say.what.to_string(),
            (Kind::Similar, Answer::New | Answer::No) => {
                let Some(wish) = pending else { return say.what.to_string() };
                // «Добавить туда или начать новую?» of a work system: the new work begins in that system and folder.
                if let Some(begin) = &wish.in_system {
                    return self
                        .start_in_system(origin, &begin.system, &begin.folder, &begin.folder_name, &wish.text, true)
                        .unwrap_or_else(refused);
                }
                return self.start(origin, &wish.text, wish.face, wish.title.as_deref(), true).unwrap_or_else(refused);
            }
            (Kind::Similar, Answer::Edit | Answer::Yes | Answer::Raise | Answer::Redo { .. }) => {
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
            (
                _,
                Answer::New
                | Answer::Edit
                | Answer::Choice(_)
                | Answer::Wait
                | Answer::Stop
                | Answer::Raise
                | Answer::Switch
                | Answer::HandTo { .. },
            ) => say.what.to_string(),
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
            Kind::Plan if work.going() => Kind::Plan,
            // A system's question of rights holds its going work (docs/plans/work-systems.md, stage 3): the panel's buttons.
            Kind::Permission if work.going() => Kind::Permission,
            Kind::Budget if work.going() => Kind::Budget,
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

/// The device a work was touched on last (docs/plans/audio-interface-3.md, «Н1»): where its last phrase came from —
/// its start, a note, «и ещё…» —, so that its «Готово» is said there; where it began, when nothing came after.
pub fn touched_on(work: &Work) -> &str {
    work.notes
        .iter()
        .map(|note| (note.origin.device.as_str(), note.at))
        .chain([(work.device(), work.started_at)])
        .max_by_key(|(_, at)| *at)
        .map_or(work.device(), |(device, _)| device)
}
