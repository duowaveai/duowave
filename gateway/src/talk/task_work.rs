//! The executor «задача приложения» (docs/plans/long-tools.md, point 4): an app's long task (src/tasks) begun while
//! the app answered a conversation's call is a work of the kind `task` — «как там лента?» says its percent and
//! words, «стоп» asks the app to cancel it, and its end is said where the phrase was: «Лента: готово. …».
//!
//! A task the app began by itself (its schedule, its window) stays the panel's, as before (the owner's default 4).
//! The words of «Готово» are the app's own `result.say` when it gave one (its owner installed it), else the model's
//! retelling of the result, else the bare frame.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use futures::future::BoxFuture;
use futures::FutureExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::executor::{ByVoice, Closed, Doing, Executor, Facts, Put, Report, Reporter, Started, Told};
use super::layer::TalkLayer;
use super::pack::{self, fill};
use super::tool_work::split_summary;
use super::works::{End, NewWork, Work, WorkState, TASK};
use crate::core::events::{EventBus, GatewayEvent};
use crate::dialog::llm::LlmMessage;
use crate::dialog::DialogService;
use crate::memory::text::Lang;
use crate::tasks::{TaskState, Tasks};

/// Its name in the records.
pub const TASK_EXECUTOR: &str = "task";
/// What the model reads of a task's result for the retelling.
const READ_LIMIT: usize = 16 * 1024;
const DATA_LIMIT: usize = 64 * 1024;

pub struct TaskExecutor {
    me: Weak<TaskExecutor>,
    layer: Weak<TalkLayer>,
    tasks: Weak<Tasks>,
    dialog: Weak<DialogService>,
    reporter: OnceLock<Reporter>,
    told: Mutex<HashMap<String, Told>>,
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl TaskExecutor {
    /// The executor for a layer: it joins it and follows the tasks' changes on the bus.
    pub fn new(
        layer: &Arc<TalkLayer>,
        tasks: &Arc<Tasks>,
        events: &EventBus,
        dialog: Option<&Arc<DialogService>>,
    ) -> Arc<TaskExecutor> {
        let executor = Arc::new_cyclic(|me| TaskExecutor {
            me: me.clone(),
            layer: Arc::downgrade(layer),
            tasks: Arc::downgrade(tasks),
            dialog: dialog.map(Arc::downgrade).unwrap_or_default(),
            reporter: OnceLock::new(),
            told: Mutex::new(HashMap::new()),
        });
        layer.add_executor(executor.clone());
        let me = Arc::downgrade(&executor);
        events.on("talk.tasks", move |event| {
            let Some(executor) = me.upgrade() else { return };
            if let GatewayEvent::TasksChanged { task, change, .. } = event.as_ref() {
                executor.heard(task, change);
            }
        });
        executor
    }

    fn report(&self, report: Report) {
        if let Some(reporter) = self.reporter.get() {
            reporter(TASK_EXECUTOR, report);
        }
    }

    /// A task changed: one begun in a conversation becomes a work; its end is told to the layer.
    fn heard(&self, id: &str, change: &str) {
        let (Some(layer), Some(tasks)) = (self.layer.upgrade(), self.tasks.upgrade()) else { return };
        match change {
            "started" => {
                let Some(origin) = tasks.origin_of(id) else { return };
                let Some(task) = tasks.get(id).map(|log| log.task) else { return };
                let work = layer.works.create(NewWork {
                    kind: TASK,
                    face: None,
                    executor: TASK_EXECUTOR.into(),
                    target: Some(id.to_string()),
                    title: task.title.clone(),
                    brief: task.app.clone(),
                    origin,
                    again: false,
                });
                layer.told(&work, "started");
                tracing::info!(target: "talk", "work {} follows the task «{}» of {}", work.id, task.title, task.app);
            }
            "done" | "failed" | "cancelled" | "interrupted" => {
                if layer.works.of_target(TASK_EXECUTOR, id).filter(Work::going).is_none() {
                    return;
                }
                let Some(task) = tasks.get(id).map(|log| log.task) else { return };
                let (Some(me), Ok(handle)) = (self.me.upgrade(), tokio::runtime::Handle::try_current()) else { return };
                let lang = layer.works.of_target(TASK_EXECUTOR, id).map(|work| layer.lang(work.device()));
                handle.spawn(async move { me.ended(task, lang.unwrap_or(Lang::Ru)).await });
            }
            _ => {}
        }
    }

    /// The task ended: what it came to, then the layer hears it.
    async fn ended(self: Arc<Self>, task: crate::tasks::Task, lang: Lang) {
        let state = match task.state {
            TaskState::Done => WorkState::Ready,
            TaskState::Failed => WorkState::Failed,
            TaskState::Cancelled => WorkState::Stopped,
            _ => WorkState::Interrupted,
        };
        let told = match task.state {
            TaskState::Failed => Told { error: task.error.clone(), ..Told::default() },
            TaskState::Done => self.result_of(&task, lang).await,
            _ => Told::default(),
        };
        locked(&self.told).insert(task.id.clone(), told);
        self.report(Report::Ended { target: task.id.clone(), state, line: None, title: None });
    }

    /// The result's words: the app's `say`, else the model's retelling of the result.
    async fn result_of(&self, task: &crate::tasks::Task, lang: Lang) -> Told {
        let data = task.result.as_ref().map(|result| match result {
            Value::String(text) => text.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_default(),
        });
        let data = data.filter(|data| !data.trim().is_empty() && data.trim() != "null");
        let say = task.result.as_ref().and_then(|result| result.get("say")).and_then(Value::as_str);
        let retold = match (say, &data, self.dialog.upgrade().filter(|dialog| dialog.enabled())) {
            (Some(say), _, _) => Some(say.trim().to_string()),
            (None, Some(data), Some(dialog)) => {
                let words = &pack::of(lang).work.background;
                let system = fill(words.summary_prompt, &[("title", &task.title), ("lang", lang_name(lang))]);
                let read: String = data.chars().take(READ_LIMIT).collect();
                let messages = vec![LlmMessage::User(format!("{}\n\n{read}", task.title))];
                let key = format!("work:{TASK_EXECUTOR}:{}", task.id);
                match dialog.background_round(&key, system, messages, Vec::new(), CancellationToken::new()).await {
                    Ok(turn) => {
                        if let (Some(usage), Some(layer)) = (turn.usage.as_ref(), self.layer.upgrade()) {
                            layer.spent(TASK_EXECUTOR, &task.id, usage);
                        }
                        Some(turn.content.trim().to_string())
                    }
                    Err(error) => {
                        tracing::warn!(target: "talk", "the task «{}»: no retelling: {error}", task.title);
                        None
                    }
                }
            }
            _ => None,
        };
        let (summary, more) = retold.as_deref().map(split_summary).unwrap_or((None, None));
        Told { summary, more, data: data.map(|data| data.chars().take(DATA_LIMIT).collect()), error: None }
    }
}

impl Executor for TaskExecutor {
    fn name(&self) -> &'static str {
        TASK_EXECUTOR
    }

    fn listen(&self, reports: Reporter) {
        let _ = self.reporter.set(reports);
    }

    /// A task begins in its app, never here: the work follows it.
    fn start(&self, _work: &Work) -> Result<Started, String> {
        Err("a task begins in its app".into())
    }

    fn add(&self, _work: &Work, _text: &str) -> bool {
        false
    }

    fn thing(&self, _name: &str) -> Option<Started> {
        None
    }

    fn open<'a>(&'a self, _target: &'a str, _copy: bool) -> BoxFuture<'a, Result<Started, Closed>> {
        async { Err(Closed::Failed(pack::of(Lang::Ru).work.background.no_add.to_string())) }.boxed()
    }

    fn status(&self, _work: &Work) -> Option<Doing> {
        Some(Doing::Thinking)
    }

    /// The task's percent and words: «60%, склеиваю второй кусок».
    fn details(&self, work: &Work) -> Option<String> {
        let target = work.target.as_deref()?;
        if let Some(told) = locked(&self.told).get(target) {
            return told.error.clone().or_else(|| told.summary.clone());
        }
        let task = self.tasks.upgrade()?.get(target)?.task;
        let words = task.text.trim().trim_end_matches('.').to_string();
        Some(match (task.percent, words.is_empty()) {
            (Some(percent), false) => format!("{percent}%, {words}."),
            (Some(percent), true) => format!("{percent}%."),
            (None, false) => format!("{words}."),
            (None, true) => return None,
        })
    }

    fn facts<'a>(&'a self, _work: &'a Work) -> BoxFuture<'a, Facts> {
        async { Facts::default() }.boxed()
    }

    fn put<'a>(&'a self, _work: &'a Work, _end: End, _by: ByVoice) -> BoxFuture<'a, Result<Put, String>> {
        async { Ok(Put::Shown { open: false }) }.boxed()
    }

    fn stop(&self, work: &Work) -> bool {
        let (Some(target), Some(tasks)) = (work.target.as_deref(), self.tasks.upgrade()) else { return false };
        tasks.cancel(target).is_ok()
    }

    fn told(&self, work: &Work) -> Option<Told> {
        locked(&self.told).get(work.target.as_deref()?).cloned()
    }
}

fn lang_name(lang: Lang) -> &'static str {
    match lang {
        Lang::Ru => "русский",
        _ => "English",
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::mcp::mcp_service::tests::until;
    use crate::storage::Db;
    use crate::talk::{Activator, Origin};

    fn env() -> (tempfile::TempDir, Arc<TalkLayer>, Arc<Tasks>) {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(Some(dir.path())).unwrap());
        let events = EventBus::new();
        let tasks = Tasks::new(db, events.clone());
        let layer = TalkLayer::new(None, events.clone());
        TaskExecutor::new(&layer, &tasks, &events, None);
        (dir, layer, tasks)
    }

    #[tokio::test]
    async fn a_task_begun_in_a_conversation_is_its_work_and_its_end_is_said_there() {
        let (_dir, layer, tasks) = env();
        let origin = Origin::computer(Activator::Text);
        let task = {
            let _calling = tasks.calling("feed", origin.clone());
            tasks.start("feed", "Ролик о море", Some("начинаю"), Some(0.0), 1).unwrap()
        };
        until("the work", || layer.works().of_target(TASK_EXECUTOR, &task.id).is_some(), Duration::from_secs(5)).await;
        let work = layer.works().of_target(TASK_EXECUTOR, &task.id).unwrap();
        assert_eq!((work.kind.as_str(), work.title.as_str()), (TASK, "Ролик о море"));
        tasks.progress("feed", &task.id, Some(60.0), Some("склеиваю второй кусок")).unwrap();
        let status = layer.act(&origin, "status", None).await.unwrap();
        assert!(status.ends_with("60%, склеиваю второй кусок."), "{status}");
        let result = json!({ "say": "Ролик готов, две минуты.", "file": "sea.mp4" });
        tasks.finish("feed", &task.id, crate::tasks::Outcome::Done(Some(result))).unwrap();
        let id = work.id.clone();
        let done = || layer.works().get(&id).is_some_and(|work| work.state == WorkState::Done);
        until("the work is done", done, Duration::from_secs(5)).await;
        let line = layer.works().get(&id).unwrap().result.unwrap().line;
        assert_eq!(line, "Ролик о море: готово. Ролик готов, две минуты. Подробности в пульте.");

        // A task the app began by itself is the panel's.
        let quiet = tasks.start("feed", "Ночная сборка", None, None, 2).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(layer.works().of_target(TASK_EXECUTOR, &quiet.id).is_none());
    }
}
