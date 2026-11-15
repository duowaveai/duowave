//! Long tasks for plain JS apps (docs/plans/long-tools.md, point 4): a call of a QuickJS app lives seconds (a voice
//! answer two, the background thirty), so a task is not one long call but many short ones. The app starts it —
//! `host.tasks.start({ title, step, state })` — and the gateway calls its exported function `step` again and again,
//! each time as a background call, until the step says the task is over:
//!
//! ```js
//! export async function collect({ task }) {          // one piece of the work, a few seconds
//!   const page = task.state?.page ?? 0;
//!   …
//!   if (more) return { state: { page: page + 1 }, percent: 40, text: "собираю вторую страницу", waitMs: 20000 };
//!   return { done: { say: "Собрала 48 отзывов.", items } };   // or { fail: "сайт не отвечает" }
//! }
//! ```
//!
//! Between the steps the task waits `waitMs` (one second by default, a minute at most); a person's «отмени» ends it
//! before the next step; a step that fails ends it failed. A task runs `TASK_LIMIT` at most (ten minutes on the
//! station, where memory is short — the owner's default 6). The task's end is the work's end when it was begun in a
//! conversation (talk/task_work.rs).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::controller::PluginHost;
use super::runtime::CallKind;
use crate::tasks::{Outcome, TaskState, Tasks};

/// A JS task at most, on a computer.
pub const TASK_LIMIT: Duration = Duration::from_secs(30 * 60);
/// On the station.
pub const STATION_LIMIT: Duration = Duration::from_secs(10 * 60);
/// The pause between steps when the step names none, and at most.
const PAUSE: Duration = Duration::from_secs(1);
const PAUSE_MAX: Duration = Duration::from_secs(60);

/// The step function of a task: a name of the app's exports, plain letters, digits and `_`.
pub fn step_name(text: &str) -> Option<String> {
    let text = text.trim();
    let valid = !text.is_empty()
        && text.len() <= 64
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !text.starts_with(|c: char| c.is_ascii_digit());
    valid.then(|| text.to_string())
}

/// Runs a task's steps until it ends: on the gateway's runtime, never on the app's thread between the calls.
pub fn pump(host: Arc<PluginHost>, tasks: Arc<Tasks>, app: String, id: String, step: String, state: Value) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
    let limit = if cfg!(feature = "station") { STATION_LIMIT } else { TASK_LIMIT };
    handle.spawn(async move {
        let began = tokio::time::Instant::now();
        let mut state = state;
        loop {
            let Some(task) = tasks.get(&id).map(|log| log.task) else { return };
            if task.state != TaskState::Running {
                return;
            }
            if task.cancel_requested {
                let _ = tasks.finish(&app, &id, Outcome::Cancelled);
                return;
            }
            if began.elapsed() > limit {
                let minutes = limit.as_secs() / 60;
                let why = crate::tr!("задача не уложилась в {minutes} мин", minutes = minutes);
                let _ = tasks.finish(&app, &id, Outcome::Failed(why));
                return;
            }
            let input = json!({ "task": { "id": id, "title": task.title, "state": state } });
            let answer = match host.call(&app, &step, input, CallKind::Background).await {
                Ok(answer) => answer,
                Err(error) => {
                    let _ = tasks.finish(&app, &id, Outcome::Failed(error.to_string()));
                    return;
                }
            };
            if let Some(done) = answer.get("done") {
                let _ = tasks.finish(&app, &id, Outcome::Done(Some(done.clone()).filter(|done| !done.is_null())));
                return;
            }
            if let Some(fail) = answer.get("fail") {
                let why = fail.as_str().map(str::to_string).unwrap_or_else(|| fail.to_string());
                let _ = tasks.finish(&app, &id, Outcome::Failed(why));
                return;
            }
            if let Some(next) = answer.get("state") {
                state = next.clone();
            }
            let percent = answer.get("percent").and_then(Value::as_f64);
            let text = answer.get("text").and_then(Value::as_str);
            match tasks.progress(&app, &id, percent, text) {
                Ok(true) => {
                    let _ = tasks.finish(&app, &id, Outcome::Cancelled);
                    return;
                }
                Ok(false) => {}
                // Ended meanwhile (cancelled by the panel, closed by the limits).
                Err(_) => return,
            }
            let pause =
                answer.get("waitMs").and_then(Value::as_u64).map(Duration::from_millis).unwrap_or(PAUSE).min(PAUSE_MAX);
            // A cancel asked during the pause is seen at the next turn; the pause is short enough for «отмени».
            let _ = tasks.wait(&app, &id, pause).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PluginsConfig;
    use crate::core::availability::Placement;
    use crate::core::events::EventBus;
    use crate::plugins::controller::PluginsDeps;
    use crate::plugins::quickjs::QuickJs;
    use crate::plugins::registry::tests::write_package;
    use crate::settings::SettingsService;
    use crate::skills::media::sources::MediaSources;
    use crate::storage::Db;

    const CODE: &str = r#"
export async function collect({ task }) {
  const n = (task.state?.n ?? 0) + 1;
  if (n < 3) return { state: { n }, percent: n * 30, text: `шаг ${n}`, waitMs: 10 };
  return { done: { say: `Готово за ${n} шага`, n } };
}
export async function endless({ task }) {
  return { state: task.state, text: "жду", waitMs: 50 };
}
"#;

    async fn host(dir: &std::path::Path) -> Arc<PluginHost> {
        let manifest = json!({
            "schema": 1, "id": "slowapp", "version": "1.0.0", "title": "Медленное", "api": "^2", "runs": ["pc"],
            "permissions": []
        });
        write_package(&dir.join("packages"), &manifest, Some(CODE));
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let host = PluginHost::new(PluginsDeps {
            config: PluginsConfig {
                packages_dir: dir.join("packages"),
                disabled: false,
                idle_unload_s: 600,
                feed_url: None,
                catalog_dir: dir.join("packages-catalog"),
                dev_dir: dir.join("packages-dev"),
                catalog_key: None,
                reserve_mb: 40,
                storage_reserve_mb: 15,
                pressure_action: crate::plugins::budget::PressureAction::Disable,
                bundled: false,
                schedules: false,
                node_dir: std::path::PathBuf::from("node"),
                own_node: None,
                node_mirror: None,
            },
            db: db.clone(),
            settings: Arc::new(SettingsService::new(db, events.clone())),
            events,
            media: MediaSources::new(Vec::new()),
            placement: Arc::new(|| Placement::Pc),
            runtime: Arc::new(QuickJs),
            memory: crate::plugins::budget::fake::plenty(),
            gauge: Default::default(),
            bundled: Vec::new(),
            player: None,
            watch: None,
            home: None,
            gateway_url: "http://127.0.0.1:9".into(),
            services: None,
            secrets: Arc::new(crate::secrets::SecretsService::new(Arc::new(Db::memory()), None).unwrap()),
        });
        host.enable("slowapp", None).await.unwrap();
        host
    }

    async fn until(what: &str, check: impl Fn() -> bool) {
        let began = std::time::Instant::now();
        while !check() {
            assert!(began.elapsed() < Duration::from_secs(10), "waited too long: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn the_gateway_calls_the_step_until_it_is_done_and_a_cancel_ends_it() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path()).await;
        let tasks = Tasks::new(Arc::new(Db::memory()), EventBus::new());
        let task = tasks.start("slowapp", "Сбор", None, None, 2).unwrap();
        pump(host.clone(), tasks.clone(), "slowapp".into(), task.id.clone(), "collect".into(), Value::Null);
        let state = |id: &str| tasks.get(id).map(|log| log.task.state);
        until("done", || state(&task.id) == Some(TaskState::Done)).await;
        let done = tasks.get(&task.id).unwrap();
        assert_eq!(done.task.result, Some(json!({ "say": "Готово за 3 шага", "n": 3 })));
        assert!(done.log.iter().any(|step| step.text == "шаг 2"), "the steps' words are its journal");

        let endless = tasks.start("slowapp", "Без конца", None, None, 2).unwrap();
        pump(host, tasks.clone(), "slowapp".into(), endless.id.clone(), "endless".into(), json!({ "a": 1 }));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(state(&endless.id), Some(TaskState::Running));
        tasks.cancel(&endless.id).unwrap();
        until("cancelled", || state(&endless.id) == Some(TaskState::Cancelled)).await;
    }

    #[test]
    fn a_step_is_a_plain_name_of_the_apps_exports() {
        assert_eq!(step_name(" collect ").as_deref(), Some("collect"));
        assert_eq!(step_name("step_2").as_deref(), Some("step_2"));
        for bad in ["", "2step", "a.b", "../x", "eval(1)"] {
            assert_eq!(step_name(bad), None, "{bad}");
        }
    }
}
