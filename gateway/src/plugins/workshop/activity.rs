//! What the workshop's agent is doing (docs/plans/desk.md, «Делаю виджет»): for each draft, who asked and what, then
//! the agent's steps — a call and what it is about, its result, what it said — and how the work ended. The panel's
//! top line shows a draft the agent works on, whoever started it (by voice, in the workshop, from «+»), and the
//! workshop shows the log: the person sees that the work goes on and what it is at. The work goes on when the panel
//! that started it is closed; «Остановить» ends it. In memory only: a restart forgets it.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use futures::StreamExt;
use serde::Serialize;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::core::events::{EventBus, GatewayEvent};
use crate::dialog::generator::generate;
use crate::dialog::PhraseStream;
use crate::utils::time::now_ms;

/// Steps kept per draft, over its runs: the last ones are what the panel shows.
const KEPT: usize = 200;
/// A run that ended stays on the plaque this long, so «готово» or what went wrong is seen.
const SHOWN_AFTER_MS: u64 = 90_000;
/// What a step keeps of its text.
const TEXT: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StepKind {
    /// What the person asked: a run starts with it.
    Asked,
    /// What the agent said.
    Said,
    /// A call of the agent's: its tool, and what it is about (a file, a phrase).
    Tool,
    /// The call's answer, shortened.
    Result,
    Done,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    /// Grows over every draft's steps: `after` in a read gives only the newer ones.
    pub seq: u64,
    /// Milliseconds since the epoch.
    pub at: u64,
    pub kind: StepKind,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// A result's call: the `seq` of the tool step it answers. The calls of one turn run at once (the dialog's
    /// `join_all`), their answers come in any order, two of the same tool too — the panel pairs them by it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call: Option<u64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
}

/// Who started the work: the station's voice, or the panel (the workshop's chat, «+» → «Новый виджет»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum By {
    Voice,
    Panel,
}

/// A draft's work as the plaque shows it: what was asked, since when, whether it goes on, its last step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub draft: String,
    pub by: By,
    pub asked: String,
    pub since: u64,
    pub busy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<Step>,
}

struct Entry {
    run: u64,
    by: By,
    asked: String,
    since: u64,
    ended: Option<u64>,
    cancel: CancellationToken,
    log: VecDeque<Step>,
}

impl Entry {
    fn job(&self, draft: &str) -> Job {
        Job {
            draft: draft.to_string(),
            by: self.by,
            asked: self.asked.clone(),
            since: self.since,
            busy: self.ended.is_none(),
            ended: self.ended,
            last: self.log.back().cloned(),
        }
    }
}

/// One run of the agent on a draft: its steps are told under it. A newer run of the same draft replaces it, and the
/// steps of the replaced one are no longer kept.
#[derive(Debug, Clone)]
pub struct Run {
    pub draft: String,
    id: u64,
    /// Cancelled by «Остановить», by a newer run, and by whatever cancels the token it was made from.
    pub cancel: CancellationToken,
}

#[derive(Default)]
pub struct Activity {
    entries: Mutex<HashMap<String, Entry>>,
    seq: AtomicU64,
    events: OnceLock<EventBus>,
}

impl Activity {
    /// The panel hears of every step through `workshop.changed` (change `agent`) and reads the activity again.
    pub fn attach_events(&self, events: EventBus) {
        let _ = self.events.set(events);
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn next(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn tell(&self, draft: &str) {
        if let Some(events) = self.events.get() {
            events.emit(GatewayEvent::WorkshopChanged { draft: draft.to_string(), change: "agent".into() });
        }
    }

    fn step(&self, kind: StepKind, text: &str, tool: Option<&str>, failed: bool) -> Step {
        let tool = tool.map(str::to_string);
        Step { seq: self.next(), at: now_ms(), kind, text: short(text), tool, call: None, failed }
    }

    /// A run starts on `draft` with what was asked; one still going on the draft stops, the newer request replaces it.
    pub fn begin(&self, draft: &str, by: By, asked: &str, parent: &CancellationToken) -> Run {
        let id = self.next();
        let cancel = parent.child_token();
        {
            let mut entries = self.entries();
            let mut log = match entries.remove(draft) {
                Some(mut old) => {
                    if old.ended.is_none() {
                        old.cancel.cancel();
                        old.log.push_back(self.step(
                            StepKind::Stopped,
                            "Новая просьба — эта работа прервана.",
                            None,
                            false,
                        ));
                    }
                    old.log
                }
                None => VecDeque::new(),
            };
            log.push_back(self.step(StepKind::Asked, asked, None, false));
            trim(&mut log);
            let entry =
                Entry { run: id, by, asked: short(asked), since: now_ms(), ended: None, cancel: cancel.clone(), log };
            entries.insert(draft.to_string(), entry);
        }
        self.tell(draft);
        Run { draft: draft.to_string(), id, cancel }
    }

    /// A step of a run that still goes on; a replaced or ended run's steps are dropped.
    fn push(&self, run: &Run, step: Step, end: bool) -> bool {
        {
            let mut entries = self.entries();
            let Some(entry) = entries.get_mut(&run.draft) else { return false };
            if entry.run != run.id || entry.ended.is_some() {
                return false;
            }
            entry.log.push_back(step);
            trim(&mut entry.log);
            if end {
                entry.ended = Some(now_ms());
            }
        }
        self.tell(&run.draft);
        true
    }

    pub fn said(&self, run: &Run, text: &str) {
        if !text.trim().is_empty() {
            self.push(run, self.step(StepKind::Said, text, None, false), false);
        }
    }

    /// A call starts: its tool and what it is about. Its step's `seq`, for its answer (`answer`): a replaced or ended
    /// run's step is dropped, and so is the answer then.
    pub fn call(&self, run: &Run, tool: &str, input: &Map<String, Value>) -> u64 {
        let step = self.step(StepKind::Tool, &about(input), Some(tool), false);
        let seq = step.seq;
        self.push(run, step, false);
        seq
    }

    /// The answer to the call `call` (the seq `call` gave): the step carries it, whatever came between.
    pub fn answer(&self, run: &Run, call: u64, tool: &str, content: &str, failed: bool) {
        let step = Step { call: Some(call), ..self.step(StepKind::Result, content, Some(tool), failed) };
        self.push(run, step, false);
    }

    /// The run ends: done, failed (with why) or stopped.
    pub fn end(&self, run: &Run, kind: StepKind, text: &str) {
        self.push(run, self.step(kind, text, None, kind == StepKind::Failed), true);
    }

    /// «Остановить»: the draft's run ends now. False when nothing goes on there.
    pub fn stop(&self, draft: &str) -> bool {
        let stopped = {
            let mut entries = self.entries();
            match entries.get_mut(draft) {
                Some(entry) if entry.ended.is_none() => {
                    entry.cancel.cancel();
                    entry.log.push_back(self.step(StepKind::Stopped, "Остановлено в пульте.", None, false));
                    trim(&mut entry.log);
                    entry.ended = Some(now_ms());
                    true
                }
                _ => false,
            }
        };
        if stopped {
            self.tell(draft);
        }
        stopped
    }

    /// The work the plaque shows: what goes on, and what ended in the last minute and a half; the newest first.
    pub fn jobs(&self) -> Vec<Job> {
        let now = now_ms();
        let mut jobs: Vec<Job> = self
            .entries()
            .iter()
            .filter(|(_, entry)| entry.ended.is_none_or(|ended| now.saturating_sub(ended) < SHOWN_AFTER_MS))
            .map(|(draft, entry)| entry.job(draft))
            .collect();
        jobs.sort_by_key(|job| std::cmp::Reverse(job.since));
        jobs
    }

    /// A draft's work and its steps after `after` (0 — all that is kept); None when the agent never worked on it.
    pub fn log(&self, draft: &str, after: u64) -> Option<(Job, Vec<Step>)> {
        let entries = self.entries();
        let entry = entries.get(draft)?;
        Some((entry.job(draft), entry.log.iter().filter(|step| step.seq > after).cloned().collect()))
    }

    /// The answer's phrases, each told as said; the run ends with them: done, failed on an error, stopped when it was
    /// stopped or the stream is dropped before its end. A stopped run's error is not passed on: the station says
    /// nothing more, rather than «не получилось».
    pub fn watch(self: &Arc<Self>, run: Run, phrases: PhraseStream) -> PhraseStream {
        let activity = self.clone();
        // Made before the generator: dropped unread, the stream still ends the run.
        let ending = Ending { activity: activity.clone(), run: run.clone(), ended: false };
        generate(move |y| async move {
            let mut ending = ending;
            let mut phrases = phrases;
            while let Some(phrase) = phrases.next().await {
                match phrase {
                    Ok(text) => {
                        activity.said(&run, &text);
                        y.send(Ok(text)).await;
                    }
                    Err(error) => {
                        ending.ended = true;
                        if run.cancel.is_cancelled() {
                            activity.end(&run, StepKind::Stopped, "");
                        } else {
                            activity.end(&run, StepKind::Failed, &format!("{error:#}"));
                            y.send(Err(error)).await;
                        }
                        return;
                    }
                }
            }
            ending.ended = true;
            let kind = if run.cancel.is_cancelled() { StepKind::Stopped } else { StepKind::Done };
            activity.end(&run, kind, "");
        })
    }
}

/// Ends a run as stopped when its phrases are dropped before their end (the person spoke over the answer).
struct Ending {
    activity: Arc<Activity>,
    run: Run,
    ended: bool,
}

impl Drop for Ending {
    fn drop(&mut self) {
        if !self.ended {
            self.activity.end(&self.run, StepKind::Stopped, "");
        }
    }
}

fn trim(log: &mut VecDeque<Step>) {
    while log.len() > KEPT {
        log.pop_front();
    }
}

fn short(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= TEXT {
        return text.to_string();
    }
    let mut kept: String = text.chars().take(TEXT - 1).collect();
    kept.push('…');
    kept
}

/// What a call is about, for a person: its file, its phrase or its first text.
fn about(input: &Map<String, Value>) -> String {
    ["path", "phrase", "text", "name", "query"]
        .iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
        .or_else(|| input.values().find_map(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn kinds(steps: &[Step]) -> Vec<StepKind> {
        steps.iter().map(|step| step.kind).collect()
    }

    #[tokio::test]
    async fn keeps_what_was_asked_the_calls_and_what_was_said_and_ends_the_run_with_its_phrases() {
        let activity = Arc::new(Activity::default());
        let run = activity.begin("widget-1", By::Panel, "Сделай виджет: курс биткоина", &CancellationToken::new());
        let call = activity.call(&run, "write_file", json!({"path": "panel.js", "content": "…"}).as_object().unwrap());
        activity.answer(&run, call, "write_file", "Записано.", false);
        let phrases: PhraseStream = futures::stream::iter(vec![Ok("Готово.".to_string())]).boxed();
        let said: Vec<String> = activity.watch(run, phrases).map(|phrase| phrase.unwrap()).collect().await;
        assert_eq!(said, vec!["Готово."]);

        let (job, steps) = activity.log("widget-1", 0).unwrap();
        assert_eq!(
            kinds(&steps),
            vec![StepKind::Asked, StepKind::Tool, StepKind::Result, StepKind::Said, StepKind::Done]
        );
        assert_eq!((steps[1].tool.as_deref(), steps[1].text.as_str()), (Some("write_file"), "panel.js"));
        assert_eq!((steps[1].seq, steps[1].call, steps[2].call), (call, None, Some(call)), "the answer names its call");
        assert!(!job.busy && job.ended.is_some() && job.asked.contains("биткоина"));
        assert_eq!(activity.jobs().len(), 1, "an ended run stays on the plaque for a while");
        // Only the newer steps after a seq.
        let after = steps[2].seq;
        assert_eq!(kinds(&activity.log("widget-1", after).unwrap().1), vec![StepKind::Said, StepKind::Done]);
    }

    /// Review owner-5: the calls of one turn run at once and answer in any order, two of the same tool too. Each
    /// answer names its call, so the workshop's log puts the ✓ and «что увидел» on the right line.
    #[tokio::test]
    async fn each_answer_names_its_call_whatever_order_the_answers_come_in() {
        let activity = Arc::new(Activity::default());
        let run = activity.begin("widget-4", By::Panel, "Сделай виджет", &CancellationToken::new());
        let file = |path: &str| json!({ "path": path }).as_object().cloned().unwrap();
        let first = activity.call(&run, "read_file", &file("manifest.json"));
        let second = activity.call(&run, "read_file", &file("panel.js"));
        activity.answer(&run, second, "read_file", "export default definePlugin(…)", false);
        activity.answer(&run, first, "read_file", "{ \"id\": \"widget-4\" }", false);

        let steps = activity.log("widget-4", 0).unwrap().1;
        let pairs: Vec<(StepKind, Option<u64>)> = steps.iter().map(|step| (step.kind, step.call)).collect();
        assert_eq!(
            pairs,
            vec![
                (StepKind::Asked, None),
                (StepKind::Tool, None),
                (StepKind::Tool, None),
                (StepKind::Result, Some(second)),
                (StepKind::Result, Some(first)),
            ]
        );
        assert_eq!((steps[1].seq, steps[2].seq), (first, second));
        assert_eq!(serde_json::to_value(&steps[3]).unwrap()["call"], json!(second));
        assert!(serde_json::to_value(&steps[1]).unwrap().get("call").is_none(), "on results only");
    }

    #[tokio::test]
    async fn stops_a_run_from_the_panel_quietly_and_a_newer_request_replaces_one_still_going() {
        let activity = Arc::new(Activity::default());
        let first = activity.begin("golos-1", By::Voice, "Сделай виджет", &CancellationToken::new());
        let second = activity.begin("golos-1", By::Voice, "В долларах", &CancellationToken::new());
        assert!(first.cancel.is_cancelled() && !second.cancel.is_cancelled());
        activity.said(&first, "Поздно сказанное.");
        let (job, steps) = activity.log("golos-1", 0).unwrap();
        assert!(job.busy && job.asked == "В долларах");
        assert_eq!(kinds(&steps), vec![StepKind::Asked, StepKind::Stopped, StepKind::Asked]);

        // «Остановить»: the dialog's error after the cancel is not said.
        assert!(activity.stop("golos-1") && second.cancel.is_cancelled());
        assert!(!activity.stop("golos-1"), "nothing goes on any more");
        let phrases: PhraseStream = futures::stream::iter(vec![Err(anyhow::anyhow!("aborted"))]).boxed();
        let said: Vec<anyhow::Result<String>> = activity.watch(second, phrases).collect().await;
        assert!(said.is_empty());
        let (job, steps) = activity.log("golos-1", 0).unwrap();
        assert!(!job.busy);
        assert_eq!(steps.last().map(|step| step.kind), Some(StepKind::Stopped));
    }

    #[tokio::test]
    async fn tells_a_failure_with_its_reason_and_a_dropped_answer_as_stopped() {
        let activity = Arc::new(Activity::default());
        let run = activity.begin("widget-2", By::Panel, "Сделай", &CancellationToken::new());
        let phrases: PhraseStream =
            futures::stream::iter(vec![Err(anyhow::anyhow!("no answer from test-llm in 100 ms"))]).boxed();
        let said: Vec<anyhow::Result<String>> = activity.watch(run, phrases).collect().await;
        assert_eq!(said.len(), 1, "the error goes on to whoever says it");
        let last = activity.log("widget-2", 0).unwrap().1.pop().unwrap();
        assert!(last.kind == StepKind::Failed && last.failed && last.text.contains("no answer"));

        let run = activity.begin("widget-3", By::Voice, "Сделай", &CancellationToken::new());
        let endless: PhraseStream = futures::stream::pending().boxed();
        drop(activity.watch(run, endless));
        let (job, steps) = activity.log("widget-3", 0).unwrap();
        assert!(!job.busy);
        assert_eq!(steps.last().map(|step| step.kind), Some(StepKind::Stopped));
    }
}
