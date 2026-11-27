//! (Was talk/tool_work.rs: the talk asks it through `talk::executor::Background`, a floor below.)
//! The executor «вызов инструмента» (docs/plans/long-tools.md, points 2 and 3): an MCP tool marked «В фоне» is not
//! awaited in the conversation. Its call becomes a work of the kind `tool`:
//!
//! - the conversation hears a ready phrase at once — «Запущено: отзывы о кофейне. Скажу, когда будет готово.»; a paid
//!   run is asked first — «Запуск платный: … Запускать?» — and runs only on «да» (`Kind::Run`, ready.rs);
//! - the server is called on the gateway's workers with a longer wait (`BACKGROUND_WAIT`, ten minutes);
//! - the follow-up: the fast model reads what came back and may go on with the same server's tools that change
//!   nothing — check how a run goes, wait (`wait`, up to a minute), fetch the data — up to `FOLLOW_ROUNDS` rounds and
//!   `WORK_LIMIT` in all. Its last words are the retelling: two sentences for «Готово», the rest for «подробнее»;
//! - «как там?» says how long it runs and what it is at, «стоп» cancels the call (the server hears
//!   `notifications/cancelled`) and the follow-up;
//! - three go on at once, the rest wait in the queue (works.rs, `TOOLS_AT_ONCE`); a restart interrupts them, as any
//!   work — nothing starts again by itself.
//!
//! Text from outside the house never acts: the follow-up is offered only tools that read.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use futures::FutureExt;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::mcp_background::{BackgroundCall, BackgroundRunner};
use super::mcp_service::BACKGROUND_WAIT;
use super::McpService;
use crate::dialog::llm::{LlmMessage, LlmTool, ToolCall, ToolResult};
use crate::dialog::{DialogService, ToolOutcome};
use crate::memory::text::Lang;
use crate::talk::asked::{Kind, Pending};
use crate::talk::executor::{ByVoice, Closed, Doing, Executor, Facts, Put, Report, Reporter, Started, Told};
use crate::talk::layer::TalkLayer;
use crate::talk::origin::{Activator, Origin};
use crate::talk::pack::{self, fill};
use crate::talk::work_tools::in_sentence;
use crate::talk::works::{End, NewWork, Work, WorkState, TOOL};
use crate::utils::time::now_ms;

/// Its name in the records.
pub const TOOL_EXECUTOR: &str = "tool";
/// The follow-up's rounds of the model at most.
pub const FOLLOW_ROUNDS: usize = 8;
/// A whole background work at most: the call and its follow-up (the owner's default 5).
pub const WORK_LIMIT: Duration = Duration::from_secs(20 * 60);
/// The follow-up's `wait` at most, a time.
const WAIT_MAX_S: u64 = 60;
/// What the record keeps of a result for the panel.
const DATA_LIMIT: usize = 64 * 1024;
/// What the model reads of a result.
const READ_LIMIT: usize = 24 * 1024;
/// A paid run's question waits this long for its «да» in the executor (the device's question is shorter).
const ASKED_FOR: Duration = Duration::from_secs(10 * 60);

/// A call going on.
struct Run {
    cancel: CancellationToken,
    /// What it is at, in the pack's words or the model's last line.
    step: Mutex<Option<String>>,
}

pub struct ToolExecutor {
    me: Weak<ToolExecutor>,
    layer: Weak<TalkLayer>,
    mcp: Weak<McpService>,
    dialog: Weak<DialogService>,
    reporter: OnceLock<Reporter>,
    /// The call of a work the layer created, until it starts: by the work's id.
    staged: Mutex<HashMap<String, BackgroundCall>>,
    /// Paid runs waiting for «да»: by the question's key.
    asked: Mutex<HashMap<String, (BackgroundCall, Instant)>>,
    /// Calls going on: by the target.
    runs: Mutex<HashMap<String, Arc<Run>>>,
    /// What the ended ones came to: by the target.
    told: Mutex<HashMap<String, Told>>,
    /// A server's question waiting for the person's yes or no (elicitation, stage 8 of docs/plans/work-systems.md):
    /// by the target.
    waiting: Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    /// The server whose questions a work answers while it runs: by the target.
    asking: Mutex<HashMap<String, String>>,
    next: AtomicU64,
}

/// How long a server's question waits for the person's answer.
const ELICIT_WAIT: Duration = Duration::from_secs(120);

/// A server's question as a yes or no: a schema with no field (a confirmation) — None inside; with one boolean field —
/// its name. Anything else is not asked by voice.
fn yes_no_field(schema: &Value) -> Option<Option<String>> {
    let properties = schema.get("properties").and_then(Value::as_object);
    match properties.map(|properties| properties.iter().collect::<Vec<_>>()) {
        None => Some(None),
        Some(fields) if fields.is_empty() => Some(None),
        Some(fields) => match fields.as_slice() {
            [(name, field)] if field.get("type").and_then(Value::as_str) == Some("boolean") => {
                Some(Some(name.to_string()))
            }
            _ => None,
        },
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl ToolExecutor {
    /// The executor for a layer, with the servers it calls and the model of its follow-up; it joins the layer and
    /// takes the MCP servers' background calls.
    pub fn new(
        layer: &Arc<TalkLayer>,
        mcp: &Arc<McpService>,
        dialog: Option<&Arc<DialogService>>,
    ) -> Arc<ToolExecutor> {
        let executor = Arc::new_cyclic(|me| ToolExecutor {
            me: me.clone(),
            layer: Arc::downgrade(layer),
            mcp: Arc::downgrade(mcp),
            dialog: dialog.map(Arc::downgrade).unwrap_or_default(),
            reporter: OnceLock::new(),
            staged: Mutex::new(HashMap::new()),
            asked: Mutex::new(HashMap::new()),
            runs: Mutex::new(HashMap::new()),
            told: Mutex::new(HashMap::new()),
            waiting: Mutex::new(HashMap::new()),
            asking: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        });
        layer.add_executor(executor.clone());
        let background: Arc<dyn crate::talk::executor::Background> = executor.clone();
        layer.set_background(&background);
        mcp.attach_runner(executor.clone());
        executor
    }

    fn report(&self, report: Report) {
        if let Some(reporter) = self.reporter.get() {
            reporter(TOOL_EXECUTOR, report);
        }
    }

    /// The work for a call: at once, or in the queue when three go on. The phrase the conversation hears.
    fn start_work(&self, layer: &TalkLayer, origin: Origin, call: BackgroundCall) -> Result<String, String> {
        let words = &pack::of(layer.lang(&origin.device)).work.background;
        let title = title_of(&call);
        let wait = layer.works.may_start_kind(TOOL, &origin, false, now_ms()).is_err();
        let work = layer.works.create(NewWork {
            kind: TOOL,
            face: None,
            executor: TOOL_EXECUTOR.into(),
            target: None,
            title: title.clone(),
            brief: format!("{} · {}", call.server_title, call.tool_title),
            origin,
            again: false,
        });
        locked(&self.staged).insert(work.id.clone(), call);
        if wait {
            layer.enqueue(&work, None);
            return Ok(fill(words.queued, &[("title", &title)]));
        }
        let work = layer.launch(work).map_err(|refusal| format!("{refusal:?}"))?;
        Ok(fill(words.started, &[("title", &in_sentence(&work.title))]))
    }

    /// The answer to «Запуск платный: … Запускать?»: the run on «да», nothing on anything else.
    pub fn answer_paid(&self, origin: Origin, key: &str, yes: bool) -> String {
        let Some(layer) = self.layer.upgrade() else { return String::new() };
        let words = &pack::of(layer.lang(&origin.device)).work.background;
        let waited = locked(&self.asked).remove(key);
        let Some((call, _)) = waited.filter(|(_, asked)| asked.elapsed() < ASKED_FOR) else {
            return pack::of(layer.lang(&origin.device)).work.say.what.to_string();
        };
        if !yes {
            return words.not_run.to_string();
        }
        // The run is the person's who answered, said where the question was.
        let origin = call.origin.clone().map(|asked| Origin { who: origin.who.clone().or(asked.who), ..asked });
        self.start_work(&layer, origin.unwrap_or_else(|| Origin::computer(Activator::Button)), call)
            .unwrap_or_else(|why| fill(pack::of(Lang::Ru).work.say.failed, &[("why", &why)]))
    }

    /// «Подробнее» to a retelling that was cut.
    pub fn more(&self, work: &Work) -> Option<String> {
        let target = work.target.as_deref()?;
        locked(&self.told).get(target).and_then(|told| told.more.clone())
    }

    /// The call and its follow-up, on the gateway's workers; its end reported to the layer.
    async fn run(self: Arc<Self>, target: String, title: String, lang: Lang, call: BackgroundCall, run: Arc<Run>) {
        let cancel = run.cancel.clone();
        let deadline = tokio::time::Instant::now() + WORK_LIMIT;
        let words = &pack::of(lang).work.background;
        *locked(&run.step) = Some(words.waiting_answer.to_string());
        let Some(mcp) = self.mcp.upgrade() else { return };
        // The server's questions during the call go to this work: asked by voice, yes or no.
        mcp.ask_through(&call.server_id, Some(self.asker(&target, &call.server_title)));
        locked(&self.asking).insert(target.clone(), call.server_id.clone());
        let first = tokio::select! {
            outcome = mcp.call_in_background(&call.server_id, &call.tool, call.input.clone(), &cancel, BACKGROUND_WAIT) => outcome,
            _ = tokio::time::sleep_until(deadline) => Err(time_words(lang).to_string()),
        };
        if cancel.is_cancelled() {
            return self.end(&target, WorkState::Stopped, Told::default());
        }
        let first = match first {
            Ok(outcome) if !outcome.is_error => outcome,
            Ok(outcome) => return self.end(&target, WorkState::Failed, failed(&outcome.content)),
            Err(why) => return self.end(&target, WorkState::Failed, failed(&why)),
        };
        let mut data = first.content.clone();
        let follow =
            self.follow(&mcp, &call.server_id, &target, &title, lang, &first.content, &mut data, &run, deadline);
        let (summary, more) = match follow.await {
            Some(text) => crate::talk::task_work::split_summary(&text),
            None if cancel.is_cancelled() => return self.end(&target, WorkState::Stopped, Told::default()),
            None => (None, None),
        };
        let data = Some(data.chars().take(DATA_LIMIT).collect::<String>()).filter(|data| !data.trim().is_empty());
        self.end(&target, WorkState::Ready, Told { summary, more, data, error: None });
    }

    /// The way a server's question reaches the person while this work runs: its question, said aloud, answered by
    /// the home's admin; a question that is not a yes or no is declined.
    fn asker(&self, target: &str, server: &str) -> super::mcp_service::Asker {
        let (me, target, who) = (self.me.clone(), target.to_string(), server.to_string());
        Arc::new(move |message: String, schema: Value| {
            let (me, target, who) = (me.clone(), target.clone(), who.clone());
            Box::pin(async move {
                let field = yes_no_field(&schema)?;
                let me = me.upgrade()?;
                let layer = me.layer.upgrade()?;
                let (answer, answered) = tokio::sync::oneshot::channel();
                locked(&me.waiting).insert(target.clone(), answer);
                layer.ask_yes_no_on(TOOL_EXECUTOR, &target, &who, &message);
                let yes = tokio::time::timeout(ELICIT_WAIT, answered).await;
                locked(&me.waiting).remove(&target);
                let yes = yes.ok()?.ok()?;
                match field {
                    Some(name) => Some(serde_json::Map::from_iter([(name, Value::Bool(yes))])),
                    None if yes => Some(serde_json::Map::new()),
                    None => None,
                }
            }) as BoxFuture<'static, Option<serde_json::Map<String, Value>>>
        })
    }

    fn end(&self, target: &str, state: WorkState, told: Told) {
        if let Some(server) = locked(&self.asking).remove(target) {
            if let Some(mcp) = self.mcp.upgrade() {
                mcp.ask_through(&server, None);
            }
        }
        locked(&self.waiting).remove(target);
        locked(&self.runs).remove(target);
        locked(&self.told).insert(target.to_string(), told);
        self.report(Report::Ended { target: target.to_string(), state, line: None, title: None });
    }

    /// The follow-up: the model reads the first result and goes on with the reading tools of the same server and
    /// `wait`, then answers in words — the retelling. None without a model, or when it was cancelled.
    #[allow(clippy::too_many_arguments)]
    async fn follow(
        &self,
        mcp: &Arc<McpService>,
        server_id: &str,
        target: &str,
        title: &str,
        lang: Lang,
        first: &str,
        data: &mut String,
        run: &Run,
        deadline: tokio::time::Instant,
    ) -> Option<String> {
        let dialog = self.dialog.upgrade().filter(|dialog| dialog.enabled())?;
        let words = &pack::of(lang).work.background;
        let offer = mcp.follow_up_offer(server_id);
        let mut tools: Vec<LlmTool> = offer.as_ref().map(|offer| offer.tools.clone()).unwrap_or_default();
        if !tools.is_empty() {
            tools.push(wait_tool());
        }
        let system = format!(
            "{}\n\n{}",
            fill(words.follow_prompt, &[("title", title)]),
            fill(words.summary_prompt, &[("title", title), ("lang", lang_name(lang))])
        );
        // The router's deal of each round goes to this work (app.rs, `work:<executor>:<target>`).
        let key = format!("work:{TOOL_EXECUTOR}:{target}");
        let mut messages = vec![LlmMessage::User(format!("{}\n\n{}", title, cut(first, READ_LIMIT)))];
        for round in 0..=FOLLOW_ROUNDS {
            if run.cancel.is_cancelled() {
                return None;
            }
            // The last round, or past the time: words only.
            let offered = if round == FOLLOW_ROUNDS || tokio::time::Instant::now() >= deadline {
                Vec::new()
            } else {
                tools.clone()
            };
            let words_only = offered.is_empty();
            let turn = tokio::select! {
                turn = dialog.background_round(&key, system.clone(), messages.clone(), offered, run.cancel.clone()) => turn,
                _ = run.cancel.cancelled() => return None,
            };
            let turn = match turn {
                Ok(turn) => turn,
                Err(error) => {
                    tracing::warn!(target: "talk", "background work {target}: the model failed: {error}");
                    return None;
                }
            };
            if let (Some(usage), Some(layer)) = (turn.usage.as_ref(), self.layer.upgrade()) {
                layer.spent(TOOL_EXECUTOR, target, usage);
            }
            if turn.tool_calls.is_empty() || words_only {
                let text = turn.content.trim().to_string();
                return Some(text).filter(|text| !text.is_empty());
            }
            let calls = turn.tool_calls.clone();
            messages.push(LlmMessage::Assistant(turn));
            let mut results = Vec::new();
            for call in calls {
                let content = self.follow_call(offer.as_ref(), &call, run, lang).await;
                if run.cancel.is_cancelled() {
                    return None;
                }
                let (content, is_error) = content;
                if !is_error && call.name != WAIT {
                    *data = content.clone();
                }
                results.push(ToolResult { id: call.id.clone(), content: cut(&content, READ_LIMIT), is_error });
            }
            messages.push(LlmMessage::Tool(results));
        }
        None
    }

    /// One call of the follow-up: `wait`, or a reading tool of the server.
    async fn follow_call(
        &self,
        offer: Option<&crate::mcp::McpOffer>,
        call: &ToolCall,
        run: &Run,
        lang: Lang,
    ) -> (String, bool) {
        if call.name == WAIT {
            let seconds = call.input.get("seconds").and_then(Value::as_u64).unwrap_or(20).clamp(1, WAIT_MAX_S);
            *locked(&run.step) = Some(waiting_words(lang).to_string());
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(seconds)) => {}
                _ = run.cancel.cancelled() => {}
            }
            return (format!("waited {seconds} s"), false);
        }
        let Some(offer) = offer.filter(|offer| offer.owns(&call.name)) else {
            return (format!("error: {} is not available here", call.name), true);
        };
        *locked(&run.step) = Some(checking_words(lang).to_string());
        match offer.run(call, &run.cancel, None).await {
            Ok(outcome) => (outcome.content, outcome.is_error),
            Err(_) => (String::from("cancelled"), true),
        }
    }
}

impl BackgroundRunner for ToolExecutor {
    fn begin(&self, call: BackgroundCall) -> ToolOutcome {
        let Some(layer) = self.layer.upgrade() else {
            return ToolOutcome::error("error: background work is not available now");
        };
        let origin = call.origin.clone().unwrap_or_else(|| Origin::computer(Activator::Button));
        let words = &pack::of(layer.lang(&origin.device)).work.background;
        if call.paid {
            let key = format!("run{}", self.next.fetch_add(1, Ordering::Relaxed));
            let question = fill(words.paid, &[("title", &in_sentence(&title_of(&call)))]);
            locked(&self.asked).retain(|_, (_, asked)| asked.elapsed() < ASKED_FOR);
            locked(&self.asked).insert(key.clone(), (call, Instant::now()));
            let pending = Pending { question: Some(question.clone()), ..Pending::default() };
            layer.asked.ask_with(&origin.device, &key, Kind::Run, pending);
            return ToolOutcome::spoken(question);
        }
        match self.start_work(&layer, origin, call) {
            Ok(phrase) => ToolOutcome::spoken(phrase),
            Err(why) => ToolOutcome::error(format!("error: {why}")),
        }
    }
}

impl Executor for ToolExecutor {
    fn name(&self) -> &'static str {
        TOOL_EXECUTOR
    }

    fn listen(&self, reports: Reporter) {
        let _ = self.reporter.set(reports);
    }

    fn start(&self, work: &Work) -> Result<Started, String> {
        let call =
            locked(&self.staged).remove(&work.id).ok_or_else(|| "nothing to run: the call was lost".to_string())?;
        let target = format!("call{}", work.id);
        let run = Arc::new(Run { cancel: CancellationToken::new(), step: Mutex::new(None) });
        locked(&self.runs).insert(target.clone(), run.clone());
        let (Some(me), Ok(handle)) = (self.me.upgrade(), tokio::runtime::Handle::try_current()) else {
            return Err("no runtime for a background work".into());
        };
        let lang = self.layer.upgrade().map(|layer| layer.lang(work.device())).unwrap_or(Lang::Ru);
        handle.spawn(me.run(target.clone(), work.title.clone(), lang, call, run));
        Ok(Started { target, title: work.title.clone(), note: None })
    }

    fn add(&self, _work: &Work, _text: &str) -> bool {
        false
    }

    fn thing(&self, _name: &str) -> Option<Started> {
        None
    }

    fn open<'a>(&'a self, target: &'a str, _copy: bool) -> BoxFuture<'a, Result<Started, Closed>> {
        let lang = self
            .layer
            .upgrade()
            .and_then(|layer| layer.works.of_target(TOOL_EXECUTOR, target).map(|work| layer.lang(work.device())))
            .unwrap_or(Lang::Ru);
        async move { Err(Closed::Failed(pack::of(lang).work.background.no_add.to_string())) }.boxed()
    }

    fn status(&self, _work: &Work) -> Option<Doing> {
        Some(Doing::Thinking)
    }

    fn details(&self, work: &Work) -> Option<String> {
        let target = work.target.as_deref()?;
        if let Some(run) = locked(&self.runs).get(target) {
            return locked(&run.step).clone();
        }
        let told = locked(&self.told).get(target).cloned()?;
        match (told.summary, told.more, told.error) {
            (_, _, Some(error)) => Some(error),
            (Some(summary), Some(more), _) => Some(format!("{summary} {more}")),
            (Some(summary), None, _) => Some(summary),
            _ => None,
        }
    }

    fn facts<'a>(&'a self, _work: &'a Work) -> BoxFuture<'a, Facts> {
        async { Facts::default() }.boxed()
    }

    fn put<'a>(&'a self, _work: &'a Work, _end: End, _by: ByVoice) -> BoxFuture<'a, Result<Put, String>> {
        async { Ok(Put::Shown { open: false }) }.boxed()
    }

    fn stop(&self, work: &Work) -> bool {
        let Some(target) = work.target.as_deref() else { return false };
        match locked(&self.runs).get(target) {
            Some(run) => {
                run.cancel.cancel();
                true
            }
            None => false,
        }
    }

    fn told(&self, work: &Work) -> Option<Told> {
        locked(&self.told).get(work.target.as_deref()?).cloned()
    }

    /// The person's yes or no to the server's question the work waits on.
    fn answer_question(
        &self,
        work: &Work,
        yes: bool,
        _by_voice: bool,
    ) -> Result<(), crate::talk::executor::NotAnswered> {
        let target = work.target.as_deref().ok_or(crate::talk::executor::NotAnswered::Gone)?;
        let waiting = locked(&self.waiting).remove(target).ok_or(crate::talk::executor::NotAnswered::Gone)?;
        waiting.send(yes).map_err(|_| crate::talk::executor::NotAnswered::Gone)
    }
}

/// The follow-up's own tool: a pause before the next check.
const WAIT: &str = "wait";

fn wait_tool() -> LlmTool {
    LlmTool {
        name: WAIT.into(),
        description: "Wait before checking again how the service's run goes: up to 60 seconds.".into(),
        parameters: json!({
            "type": "object",
            "properties": { "seconds": { "type": "integer", "minimum": 1, "maximum": WAIT_MAX_S } },
            "required": ["seconds"],
        }),
    }
}

/// The job's name: the model's `_title`, else the tool's title.
fn title_of(call: &BackgroundCall) -> String {
    let title = call.title.clone().unwrap_or_else(|| format!("{}: {}", call.server_title, call.tool_title));
    let mut chars = title.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => call.tool.clone(),
    }
}

/// Why a call failed, short: «сервис ответил ошибкой: …» is said aloud.
fn failed(why: &str) -> Told {
    let why = why.trim().trim_start_matches("error:").trim();
    let short: String = why.split_whitespace().take(8).collect::<Vec<_>>().join(" ");
    Told { error: Some(short.trim_end_matches(['.', ',', ';', ':']).to_string()), ..Told::default() }
}

/// A text at most `limit` bytes, cut at a char.
fn cut(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn lang_name(lang: Lang) -> &'static str {
    match lang {
        Lang::Ru => "русский",
        _ => "English",
    }
}

fn time_words(lang: Lang) -> &'static str {
    match lang {
        Lang::Ru => "не уложилось в двадцать минут",
        _ => "it did not finish in twenty minutes",
    }
}

fn waiting_words(lang: Lang) -> &'static str {
    match lang {
        Lang::Ru => "Сервис ещё работает, жду.",
        _ => "The service is still working, waiting.",
    }
}

fn checking_words(lang: Lang) -> &'static str {
    match lang {
        Lang::Ru => "Проверяю ход и забираю данные.",
        _ => "Checking how it goes and fetching the data.",
    }
}

impl crate::talk::executor::Background for ToolExecutor {
    fn answer_paid(&self, origin: Origin, key: &str, yes: bool) -> String {
        ToolExecutor::answer_paid(self, origin, key, yes)
    }

    fn more(&self, work: &Work) -> Option<String> {
        ToolExecutor::more(self, work)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::EventBus;
    use crate::core::identity::principal::user_principal;
    use crate::core::identity::Role;
    use crate::mcp::mcp_service::tests::{input, ready, setup, until};
    use crate::mcp::mcp_service::ToolChange;
    use crate::mcp::OfferOptions;
    use crate::talk::asked::Kind;
    use crate::talk::ready::Answer;
    use crate::talk::COMPUTER;

    /// A server with the fixture's tools, its `slow` (three seconds) marked as the test says, and a layer with the
    /// executor; no model — the result comes without a retelling.
    async fn background(marks: serde_json::Value) -> (crate::mcp::mcp_service::tests::Setup, Arc<TalkLayer>) {
        background_with(marks, None).await
    }

    async fn background_with(
        marks: serde_json::Value,
        dialog: Option<&Arc<DialogService>>,
    ) -> (crate::mcp::mcp_service::tests::Setup, Arc<TalkLayer>) {
        let s = setup();
        let (router, _) = crate::mcp::fixtures::legacy_http();
        let base = crate::mcp::fixtures::spawn(router).await;
        let address = format!("{base}/mcp");
        let server =
            s.mcp.add(&input(json!({ "transport": "http", "title": "Fixture", "url": address })), None).unwrap();
        ready(&s.mcp, &server.id).await;
        let mut change = json!({ "on": true });
        change.as_object_mut().unwrap().extend(marks.as_object().unwrap().clone());
        let change: ToolChange = serde_json::from_value(change).unwrap();
        s.mcp.set_tool(&server.id, "slow", &change, None).unwrap();
        let on: ToolChange = serde_json::from_value(json!({ "on": true })).unwrap();
        s.mcp.set_tool(&server.id, "echo", &on, None).unwrap();
        s.mcp.set_tool(&server.id, "add_note", &on, None).unwrap();
        let layer = TalkLayer::new(None, EventBus::new());
        ToolExecutor::new(&layer, &s.mcp, dialog);
        (s, layer)
    }

    fn slow_call() -> ToolCall {
        let input = json!({ "_title": "медленное дело" }).as_object().unwrap().clone();
        ToolCall { id: "c1".into(), name: "mcp_fixture__slow".into(), input, error: None }
    }

    fn offer(s: &crate::mcp::mcp_service::tests::Setup) -> crate::mcp::McpOffer {
        let admin = user_principal("u1", "admin", Role::Admin, None);
        s.mcp.offer(&admin, OfferOptions { voice: false, capability_allowed: None }).unwrap()
    }

    #[tokio::test]
    async fn a_background_tool_answers_at_once_and_its_result_comes_as_a_work() {
        let (s, layer) = background(json!({ "background": true })).await;
        let offer = offer(&s);
        assert!(offer.background("mcp_fixture__slow"));
        let tool = offer.tools.iter().find(|tool| tool.name == "mcp_fixture__slow").unwrap();
        assert!(tool.description.contains("Работает в фоне"), "the model knows the result comes later");
        assert_eq!(tool.parameters["properties"]["_title"]["type"], json!("string"));
        let began = Instant::now();
        let origin = Origin::computer(Activator::Text);
        let outcome = offer.run(&slow_call(), &CancellationToken::new(), Some(origin)).await.unwrap();
        assert!(began.elapsed() < Duration::from_secs(2), "not awaited: the server takes three seconds");
        assert!(outcome.spoken);
        assert_eq!(outcome.content, "Запущено: медленное дело. Скажу, когда будет готово.");
        let work = layer.works().all(None, None).remove(0);
        assert_eq!((work.kind.as_str(), work.executor.as_str()), (TOOL, TOOL_EXECUTOR));
        assert!(work.going());
        assert_eq!(work.title, "Медленное дело");
        let id = work.id.clone();
        let done = || layer.works().get(&id).is_some_and(|work| work.state == WorkState::Done);
        until("the work is done", done, Duration::from_secs(10)).await;
        let result = layer.works().get(&id).unwrap().result.unwrap();
        assert_eq!(result.line, "Медленное дело: готово. Подробности в пульте.");
        assert_eq!(result.data.as_deref(), Some("slow done"));
        s.mcp.shutdown().await;
    }

    #[tokio::test]
    async fn the_follow_up_reads_with_the_same_servers_reading_tools_and_its_words_are_the_retelling() {
        use crate::dialog::dialog_service::tests::{scripted, words};
        use crate::dialog::llm::{AssistantTurn, LlmEvent};
        let (model, dialog) = scripted(Box::new(|n, _| match n {
            0 => {
                let input = json!({ "text": "данные" }).as_object().unwrap().clone();
                let call = ToolCall { id: "f1".into(), name: "mcp_fixture__echo".into(), input, error: None };
                let turn = AssistantTurn { tool_calls: vec![call], ..AssistantTurn::default() };
                vec![Ok(LlmEvent::Turn(turn))]
            }
            _ => words("Найдено три позиции. Первая дешевле всех. Вторая с доставкой. Третьей нет в наличии."),
        }));
        let (s, layer) = background_with(json!({ "background": true }), Some(&dialog)).await;
        let origin = Origin::computer(Activator::Text);
        offer(&s).run(&slow_call(), &CancellationToken::new(), Some(origin.clone())).await.unwrap();
        let id = layer.works().all(None, None).remove(0).id;
        let done = || layer.works().get(&id).is_some_and(|work| work.state == WorkState::Done);
        until("the work is done", done, Duration::from_secs(10)).await;
        let offered: Vec<String> =
            model.requests.lock().unwrap()[0].tools.iter().map(|tool| tool.name.clone()).collect();
        assert_eq!(offered, ["mcp_fixture__echo", "wait"], "only what reads, and the pause");
        let work = layer.works().get(&id).unwrap();
        let result = work.result.clone().unwrap();
        assert_eq!(result.data.as_deref(), Some("echo: данные"), "the data the follow-up fetched");
        assert_eq!(result.summary.as_deref(), Some("Найдено три позиции. Первая дешевле всех."));
        assert_eq!(
            result.line,
            "Медленное дело: готово. Найдено три позиции. Первая дешевле всех. Подробности в пульте. Рассказать подробнее?"
        );
        assert_eq!(work.question.as_ref().and_then(|question| question.kind), Some(Kind::More));
        let more = layer.answer(origin.clone(), &id, Kind::More, false, Answer::Yes).await;
        assert_eq!(more, "Вторая с доставкой. Третьей нет в наличии.");
        assert_eq!(layer.answer(origin, &id, Kind::More, false, Answer::No).await, "Хорошо.");
        s.mcp.shutdown().await;
    }

    #[tokio::test]
    async fn a_paid_run_is_asked_first_and_stop_cancels_it() {
        let (s, layer) = background(json!({ "background": true, "paid": true })).await;
        let origin = Origin::computer(Activator::Text);
        let outcome = offer(&s).run(&slow_call(), &CancellationToken::new(), Some(origin.clone())).await.unwrap();
        assert_eq!(outcome.content, "Запуск платный: медленное дело. Запускать?");
        assert!(layer.works().all(None, None).is_empty(), "no work until «да»");
        let question = layer.asked.open(COMPUTER).unwrap();
        assert_eq!(question.kind, Kind::Run);
        let said = layer.answer(origin.clone(), &question.work, Kind::Run, false, Answer::No).await;
        assert_eq!(said, "Хорошо, не запускаю.");
        assert!(layer.works().all(None, None).is_empty());

        offer(&s).run(&slow_call(), &CancellationToken::new(), Some(origin.clone())).await.unwrap();
        let question = layer.asked.open(COMPUTER).unwrap();
        let said = layer.answer(origin.clone(), &question.work, Kind::Run, false, Answer::Yes).await;
        assert_eq!(said, "Запущено: медленное дело. Скажу, когда будет готово.");
        let work = layer.works().all(None, None).remove(0);
        let status = layer.act(&origin, "status", None).await.unwrap();
        assert!(status.starts_with("Медленное дело: идёт"), "{status}");
        let stopped = layer.act(&origin, "stop", None).await.unwrap();
        assert_eq!(stopped, "Остановлено: медленное дело.");
        let id = work.id.clone();
        let stopped = || layer.works().get(&id).is_some_and(|work| work.state == WorkState::Stopped);
        until("the work is stopped", stopped, Duration::from_secs(5)).await;
        s.mcp.shutdown().await;
    }

    /// A server's question during a background call (elicitation, stage 8 of docs/plans/work-systems.md): the work's
    /// question by voice, the admin's «да» goes back as the answer; with no background work on the server, or a
    /// question that is not a yes or no, it is declined.
    #[tokio::test]
    async fn a_servers_question_during_a_background_call_is_the_works_question() {
        let (s, layer) = background(json!({ "background": true })).await;
        let server = s.mcp.view().servers[0].id.clone();
        let schema = json!({ "type": "object", "properties": { "go": { "type": "boolean" } } });
        assert_eq!(s.mcp.elicit_for_tests(&server, "Продолжить?", schema.clone()).await, None, "no work waits");
        let origin = Origin::computer(Activator::Text);
        offer(&s).run(&slow_call(), &CancellationToken::new(), Some(origin.clone())).await.unwrap();
        let work = layer.works().all(None, None).remove(0);
        let mcp = s.mcp.clone();
        let (at, ask) = (server.clone(), schema.clone());
        let asked =
            tokio::spawn(async move { mcp.elicit_for_tests(&at, "Взять платный тариф?", ask).await });
        let id = work.id.clone();
        let asks = || {
            layer
                .works()
                .get(&id)
                .and_then(|work| work.question)
                .is_some_and(|question| question.kind == Some(Kind::Permission))
        };
        until("the question is the work's", asks, Duration::from_secs(5)).await;
        let question = layer.works().get(&work.id).unwrap().question.unwrap();
        assert_eq!(question.text, "Fixture спрашивает: Взять платный тариф?");
        let mut admin = origin.clone();
        admin.role = Some(Role::Admin);
        layer.answer(admin, &work.id, Kind::Permission, false, Answer::Yes).await;
        assert_eq!(asked.await.unwrap(), Some(serde_json::Map::from_iter([("go".to_string(), json!(true))])));
        let free = json!({ "type": "object", "properties": { "city": { "type": "string" } } });
        assert_eq!(s.mcp.elicit_for_tests(&server, "Какой город?", free).await, None, "not a yes or no");
        s.mcp.shutdown().await;
    }

    #[test]
    fn a_question_is_a_yes_or_no_with_no_field_or_one_boolean() {
        assert_eq!(yes_no_field(&json!({ "type": "object", "properties": {} })), Some(None));
        assert_eq!(yes_no_field(&json!({ "type": "object" })), Some(None));
        let one = json!({ "type": "object", "properties": { "ok": { "type": "boolean" } } });
        assert_eq!(yes_no_field(&one), Some(Some("ok".into())));
        let two = json!({ "properties": { "ok": { "type": "boolean" }, "why": { "type": "string" } } });
        assert_eq!(yes_no_field(&two), None);
    }

    #[test]
    fn the_retelling_says_two_sentences_and_keeps_the_rest_for_more() {
        let (summary, more) = crate::talk::task_work::split_summary(
            "48 отзывов. В основном хвалят десерты. Жалуются на очередь по утрам. Цены называют высокими.",
        );
        assert_eq!(summary.as_deref(), Some("48 отзывов. В основном хвалят десерты."));
        assert_eq!(more.as_deref(), Some("Жалуются на очередь по утрам. Цены называют высокими."));
        let (summary, more) = crate::talk::task_work::split_summary("Готово, 3 позиции.");
        assert_eq!(summary.as_deref(), Some("Готово, 3 позиции."));
        assert_eq!(more, None);
        let long = (1..=50).map(|n| format!("слово{n}")).collect::<Vec<_>>().join(" ");
        let (summary, more) = crate::talk::task_work::split_summary(&long);
        assert!(summary.unwrap().ends_with("слово40…"));
        assert!(more.unwrap().starts_with("слово41"));
        assert_eq!(crate::talk::task_work::split_summary("  "), (None, None));
    }

    #[test]
    fn a_failure_is_said_in_a_few_words() {
        let told = failed("error: the server did not answer (request timed out after a very long wait)");
        assert_eq!(told.error.as_deref(), Some("the server did not answer (request timed out"));
    }
}
