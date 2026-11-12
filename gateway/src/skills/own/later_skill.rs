//! «Через 10 минут выключи свет» (docs/plans/plugin-schedules.md, stage 9): the person's own words, done later. The
//! gateway keeps them and, in their time, runs them on the same station as a new request — quick rules, the plugins'
//! phrases, «Мозг» — as if said then. Not a plugin's: the person sets it, so a plugin needs nothing for it.
//!
//! - From a minute to a day ahead; up to 20 per station.
//! - Messages and calls are asked back first («Через 10 минут: «передай маме купи хлеба» — верно?»): the words may be
//!   misheard, and in their time nobody is there to ask.
//! - Never a delayed command inside another: only words a person said are put off, so nothing goes round.
//! - Late by more than 10 minutes (the Mac slept, the gateway was off): not run — «выключи свет» two hours late is not
//!   what was asked — and the owner is told.
//! - It waits while the station rings or someone talks to it.
//! - «отмени отложенное» (the last), «отмени все отложенные», «что отложено».

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::Duration as StdDuration;

use chrono::{Duration, TimeZone, Utc};
use chrono_tz::Tz;
use futures::future::BoxFuture;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::alarms_skill::DEFAULT_TIMEZONE;
use super::reminders_skill::{CLOCK_WORD, DURATION_WORD};
use crate::skills::capabilities::{CapabilityName, Intent};
use crate::skills::quick_commands::match_quick_command;
use crate::skills::skill::{ContinuationRequest, ExecutionContext, Outcome, OwnSkill};
use crate::skills::text::{normalize, parse_clock, parse_duration, read_number, say_duration};
use crate::storage::StateStore;
use crate::utils::time::now_ms;

const TICK: StdDuration = StdDuration::from_secs(15);
const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 86_400_000;
/// Late by more than this, a command is not run.
const LATE_MS: i64 = 10 * 60_000;
/// Delayed commands per station, at most.
const PER_STATION: usize = 20;
/// How long a «верно?» waits for its answer.
const ASK_MS: u64 = 60_000;

static YES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("^(да|верно|все верно|конечно|ага|угу|давай|подтверждаю|правильно|да верно|да давай)$")
        .expect("a valid pattern")
});

/// Words put off: what to run and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaterCommand {
    pub id: String,
    /// The words to run, as said.
    pub command: String,
    /// When (ms since the epoch).
    pub at: i64,
    pub set_at: i64,
}

/// The station side (the session registry): the words run as a new request on the device's live station.
pub trait CommandRunner: Send + Sync {
    /// It rings, or someone talks to it: the words wait.
    fn busy(&self, device: &str) -> bool;
    /// The words as a request on the device's station; false when no station of it took them.
    fn run(&self, device: &str, command: String) -> BoxFuture<'static, bool>;
}

/// «через 10 минут …», «в 18:30 …», «завтра в 9 утра …»: the intent to put the rest off; None — not a delayed command
/// (no time first, or nothing after it). The words come normalized (quick_commands.rs).
pub fn intent_of(text: &str) -> Option<Intent> {
    let words: Vec<&str> = text.split(' ').filter(|word| !word.is_empty()).collect();
    let mut intent = Intent::new(CapabilityName::Later, "set");
    let end = if words.first() == Some(&"через") {
        let mut end = 1;
        while end < words.len() && (DURATION_WORD.is_match(words[end]) || read_number(&words, end).is_some()) {
            end += 1;
        }
        let seconds = parse_duration(&words[1..end].join(" ")).filter(|seconds| *seconds > 0)?;
        intent.params.insert("after".into(), Value::from(seconds));
        end
    } else {
        let (start, tomorrow) = match words.first() {
            Some(&"завтра") => (1, true),
            Some(&"сегодня") => (1, false),
            _ => (0, false),
        };
        if words.get(start) != Some(&"в") {
            return None;
        }
        let mut end = start + 1;
        while end < words.len() && (CLOCK_WORD.is_match(words[end]) || read_number(&words, end).is_some()) {
            end += 1;
        }
        let clock = parse_clock(&words[start + 1..end].join(" "))?;
        intent.params.insert("hours".into(), Value::from(clock.hours));
        intent.params.insert("minutes".into(), Value::from(clock.minutes));
        intent.params.insert("tomorrow".into(), Value::Bool(tomorrow));
        end
    };
    let command = words.get(end..).map(|rest| rest.join(" ")).unwrap_or_default();
    // «напомни …» has its own time and its own skill.
    if command.is_empty() || command.starts_with("напомни") {
        return None;
    }
    intent.params.insert("command".into(), Value::String(command));
    Some(intent)
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct LaterSkill {
    store: StateStore<LaterCommand>,
    runner: RwLock<Option<Arc<dyn CommandRunner>>>,
    /// The owner told of a command not run (operations/station_watch.rs), once the app wires it.
    notice: OnceLock<Arc<dyn Fn(String) + Send + Sync>>,
    /// A message or a call waiting for its «да», per device, and until when.
    asked: Mutex<std::collections::HashMap<String, (LaterCommand, i64)>>,
    /// One edit of the store at a time: the tick and the person.
    edit: Mutex<()>,
    delivering: AtomicBool,
    stop: CancellationToken,
}

impl LaterSkill {
    pub fn new(store: StateStore<LaterCommand>) -> LaterSkill {
        LaterSkill {
            store,
            runner: RwLock::new(None),
            notice: OnceLock::new(),
            asked: Mutex::new(std::collections::HashMap::new()),
            edit: Mutex::new(()),
            delivering: AtomicBool::new(false),
            stop: CancellationToken::new(),
        }
    }

    /// How the words run in their time: the session registry gives the live station of a device.
    pub fn attach_runner(&self, runner: Arc<dyn CommandRunner>) {
        *self.runner.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(runner);
    }

    /// How the owner hears of a command not run.
    pub fn set_notice(&self, notice: Arc<dyn Fn(String) + Send + Sync>) {
        let _ = self.notice.set(notice);
    }

    /// Every station's delayed commands, the soonest first, for the panel.
    pub fn all(&self) -> Vec<(String, LaterCommand)> {
        let mut all: Vec<(String, LaterCommand)> = self
            .store
            .devices()
            .into_iter()
            .flat_map(|device| self.store.get(&device).into_iter().map(move |command| (device.clone(), command)))
            .collect();
        all.sort_by_key(|(_, command)| command.at);
        all
    }

    /// The panel's «Снять»: whether there was such a command.
    pub fn remove(&self, id: &str) -> bool {
        let _edit = locked(&self.edit);
        for device in self.store.devices() {
            let commands = self.store.get(&device);
            if commands.iter().any(|command| command.id == id) {
                self.store.set(&device, commands.into_iter().filter(|command| command.id != id).collect());
                return true;
            }
        }
        false
    }

    /// Checks for due commands every 15 s until `stop`; inside the runtime.
    pub fn start(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let me = Arc::downgrade(self);
        let stop = self.stop.clone();
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(TICK);
            ticks.tick().await;
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = ticks.tick() => {}
                }
                let Some(later) = me.upgrade() else { break };
                later.deliver().await;
            }
        })
    }

    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// Runs what is due; a busy or offline station is tried again until the command is 10 minutes late.
    pub async fn deliver(&self) {
        let Some(runner) = self.runner.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone() else { return };
        if self.delivering.swap(true, Ordering::SeqCst) {
            return;
        }
        let now = now_ms() as i64;
        for device in self.store.devices() {
            let due: Vec<LaterCommand> =
                self.store.get(&device).into_iter().filter(|command| command.at <= now).collect();
            let mut gone = Vec::new();
            for command in due {
                if now - command.at > LATE_MS {
                    let text = format!(
                        "Отложенная команда «{}» не выполнена: опоздала на {} мин — шлюз не работал или колонка была занята.",
                        command.command,
                        (now - command.at) / MINUTE_MS
                    );
                    tracing::warn!(target: "later", "{device}: {text}");
                    if let Some(notice) = self.notice.get() {
                        notice(text);
                    }
                    gone.push(command.id);
                    continue;
                }
                if runner.busy(&device) {
                    continue;
                }
                if runner.run(&device, command.command.clone()).await {
                    tracing::info!(target: "later", "{device}: ran «{}»", command.command);
                    gone.push(command.id);
                }
            }
            if !gone.is_empty() {
                let _edit = locked(&self.edit);
                self.store
                    .set(&device, self.store.get(&device).into_iter().filter(|c| !gone.contains(&c.id)).collect());
            }
        }
        self.delivering.store(false, Ordering::SeqCst);
    }

    fn zone(context: &ExecutionContext) -> Tz {
        context
            .timezone
            .as_deref()
            .and_then(|zone| zone.parse().ok())
            .unwrap_or_else(|| DEFAULT_TIMEZONE.parse().unwrap_or(Tz::UTC))
    }

    /// «18:30», «завтра в 9:00» — the time where the station is.
    fn said_at(at: i64, now: i64, zone: Tz) -> String {
        let Some(moment) = Utc.timestamp_millis_opt(at).single() else { return String::new() };
        let local = moment.with_timezone(&zone);
        let today = Utc.timestamp_millis_opt(now).single().map(|now| now.with_timezone(&zone).date_naive());
        let day = if Some(local.date_naive()) == today { "" } else { "завтра " };
        format!("{day}в {}", local.format("%H:%M"))
    }

    fn set(&self, intent: &Intent, context: &ExecutionContext) -> Outcome {
        let device = &context.device_tag;
        let number = |key: &str| intent.params.get(key).and_then(Value::as_u64);
        let command = intent.params.get("command").and_then(Value::as_str).unwrap_or_default().to_string();
        let (now, zone) = (context.now.timestamp_millis(), LaterSkill::zone(context));
        let (at, when) = match number("after") {
            Some(seconds) => (now + seconds as i64 * 1000, format!("через {}", say_duration(seconds as i64, true))),
            None => {
                let tomorrow = intent.params.get("tomorrow").and_then(Value::as_bool).unwrap_or(false);
                let (hours, minutes) = (number("hours").unwrap_or(0) as u32, number("minutes").unwrap_or(0) as u32);
                let mut day = context.now.with_timezone(&zone).date_naive();
                if tomorrow {
                    day += Duration::days(1);
                }
                let wall = day.and_hms_opt(hours, minutes, 0);
                let Some(mut at) =
                    wall.and_then(|wall| zone.from_local_datetime(&wall).earliest()).map(|at| at.timestamp_millis())
                else {
                    return Outcome::speech("Такого времени нет.");
                };
                // «в 7:30» said at 8:00: tomorrow's.
                if at <= now && !tomorrow {
                    at += DAY_MS;
                }
                (at, LaterSkill::said_at(at, now, zone))
            }
        };
        if at < now + MINUTE_MS {
            return Outcome::speech("Отложить можно на минуту и больше.");
        }
        if at > now + DAY_MS {
            return Outcome::speech("Дальше суток не откладываю — поставьте напоминание.");
        }
        if intent_of(&normalize(&command)).is_some() {
            return Outcome::speech("Отложить можно только саму команду, без второго времени.");
        }
        if self.store.get(device).len() >= PER_STATION {
            return Outcome::speech("Отложенных уже двадцать — сначала отмените какие-то.");
        }
        let later = LaterCommand { id: format!("later-{now}"), command: command.clone(), at, set_at: now };
        // A message or a call: asked back first.
        if match_quick_command(&command).is_some_and(|heard| heard.intent.capability == CapabilityName::Calls) {
            locked(&self.asked).insert(device.clone(), (later, now + ASK_MS as i64));
            return Outcome {
                speech: Some(format!("{}: «{command}» — верно?", capitalized(&when))),
                listen: true,
                continuation: Some(ContinuationRequest { capability: CapabilityName::Later, ttl_ms: ASK_MS }),
                ..Outcome::default()
            };
        }
        self.keep(device, later);
        Outcome::speech(format!("Хорошо, {when}: «{command}»."))
    }

    fn keep(&self, device: &str, later: LaterCommand) {
        let _edit = locked(&self.edit);
        let mut commands = self.store.get(device);
        commands.push(later);
        self.store.set(device, commands);
    }

    /// The answer to «— верно?».
    fn answer(&self, context: &ExecutionContext) -> Outcome {
        let asked = locked(&self.asked).remove(&context.device_tag);
        let Some((later, until)) = asked.filter(|(_, until)| *until >= context.now.timestamp_millis()) else {
            return Outcome::speech("Не нашла, что подтвердить.");
        };
        if YES.is_match(&normalize(&context.text)) {
            let _ = until;
            self.keep(&context.device_tag, later);
            return Outcome::speech("Хорошо, отложила.");
        }
        Outcome::speech("Не откладываю.")
    }

    fn cancel(&self, device: &str, all: bool) -> Outcome {
        let _edit = locked(&self.edit);
        let mut commands = self.store.get(device);
        if commands.is_empty() {
            return Outcome::speech("Отложенных команд нет.");
        }
        if all {
            let count = commands.len();
            self.store.set(device, Vec::new());
            return Outcome::speech(format!("Отменила отложенные: {count}."));
        }
        commands.sort_by_key(|command| command.set_at);
        let last = commands.pop().map(|command| command.command).unwrap_or_default();
        self.store.set(device, commands);
        Outcome::speech(format!("Отменила: «{last}»."))
    }

    fn list(&self, context: &ExecutionContext) -> Outcome {
        let mut commands = self.store.get(&context.device_tag);
        if commands.is_empty() {
            return Outcome::speech("Ничего не отложено.");
        }
        commands.sort_by_key(|command| command.at);
        let (now, zone) = (context.now.timestamp_millis(), LaterSkill::zone(context));
        let said: Vec<String> = commands
            .iter()
            .map(|command| format!("«{}» {}", command.command, LaterSkill::said_at(command.at, now, zone)))
            .collect();
        Outcome::speech(format!("Отложено: {}.", said.join("; ")))
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

impl OwnSkill for LaterSkill {
    fn capability(&self) -> CapabilityName {
        CapabilityName::Later
    }

    fn verified(&self, _intent: &Intent) -> bool {
        // Speech only; what runs later is verified as it runs.
        true
    }

    fn execute<'a>(
        &'a self,
        intent: &'a Intent,
        context: &'a ExecutionContext,
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        let outcome = match intent.action.as_str() {
            "set" => self.set(intent, context),
            "continue" => self.answer(context),
            "cancel" => self.cancel(&context.device_tag, false),
            "cancelAll" => self.cancel(&context.device_tag, true),
            "list" => self.list(context),
            _ => Outcome::speech("Не поняла, что отложить."),
        };
        Box::pin(async move { Ok(outcome) })
    }
}

/// `/api/v1/later`: the delayed commands for the panel — whoever sees the station sees them, a person at home cancels
/// one.
pub fn routes() -> axum::Router<Arc<crate::app::App>> {
    use crate::core::identity::permissions::STATION_VIEW;
    use crate::platform::http::{ok, ApiError, ApiResult, Caller, Zone};
    use axum::extract::{Path, State};
    use axum::routing::{delete, get};

    async fn list(State(app): State<Arc<crate::app::App>>, caller: Caller) -> ApiResult<Value> {
        caller.permitted(STATION_VIEW)?;
        let commands: Vec<Value> = app
            .skills
            .later
            .all()
            .into_iter()
            .map(|(device, command)| {
                serde_json::json!({ "id": command.id, "device": device, "command": command.command, "at": command.at })
            })
            .collect();
        Ok(ok(Value::Array(commands)))
    }

    async fn remove(
        State(app): State<Arc<crate::app::App>>,
        caller: Caller,
        Path(id): Path<String>,
    ) -> ApiResult<Value> {
        caller.zone_at_least(Zone::Home)?;
        caller.permitted(STATION_VIEW)?;
        if !app.skills.later.remove(&id) {
            return Err(ApiError::not_found(format!("Нет отложенной команды {id}.")));
        }
        Ok(ok(Value::Bool(true)))
    }

    axum::Router::new().route("/api/v1/later", get(list)).route("/api/v1/later/{id}", delete(remove))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;
    use std::sync::atomic::AtomicUsize;

    fn context(text: &str) -> ExecutionContext {
        let mut context = ExecutionContext::new("kitchen", Utc::now());
        context.text = text.into();
        context.timezone = Some("Europe/Moscow".into());
        context
    }

    #[test]
    fn reads_when_and_what_to_put_off() {
        let after = intent_of("через 10 минут выключи свет").unwrap();
        assert_eq!(
            (after.params["after"].as_u64(), after.params["command"].as_str()),
            (Some(600), Some("выключи свет"))
        );
        let clock = intent_of("в 18:30 включи радио").unwrap();
        assert_eq!((clock.params["hours"].as_u64(), clock.params["minutes"].as_u64()), (Some(18), Some(30)));
        let tomorrow = intent_of("завтра в 9 утра включи радио").unwrap();
        assert_eq!(tomorrow.params["tomorrow"], Value::Bool(true));
        assert!(intent_of("через сколько обед").is_none());
        assert!(intent_of("в спальне включи свет").is_none());
        assert!(intent_of("через 5 минут напомни купить хлеб").is_none(), "the reminders' own");
        assert!(intent_of("через 10 минут").is_none(), "nothing to put off");
    }

    #[tokio::test]
    async fn puts_off_asks_back_for_a_message_and_cancels() {
        let later = LaterSkill::new(StateStore::open(Arc::new(Db::memory()), "later_commands"));
        let run = |text: &str| {
            let intent = intent_of(text).unwrap();
            later.set(&intent, &context(text))
        };
        assert_eq!(
            run("через 10 минут выключи свет").speech.as_deref(),
            Some("Хорошо, через 10 минут: «выключи свет».")
        );
        assert_eq!(later.store.get("kitchen").len(), 1);
        // Sooner than a minute, further than a day, one inside another.
        assert_eq!(run("через 30 секунд выключи свет").speech.as_deref(), Some("Отложить можно на минуту и больше."));
        assert!(run("через 25 часов выключи свет").speech.unwrap().contains("Дальше суток"));
        assert!(run("через 10 минут через 5 минут выключи свет").speech.unwrap().contains("без второго времени"));
        // A message: asked back, kept only after «да».
        let asked = run("через 10 минут передай маме купи хлеба");
        assert!(asked.speech.as_deref().is_some_and(|speech| speech.ends_with("— верно?")) && asked.listen);
        assert!(asked.continuation.is_some());
        assert_eq!(later.store.get("kitchen").len(), 1);
        assert_eq!(later.answer(&context("да")).speech.as_deref(), Some("Хорошо, отложила."));
        assert_eq!(later.store.get("kitchen").len(), 2);
        run("через 10 минут позвони маме");
        assert_eq!(later.answer(&context("нет")).speech.as_deref(), Some("Не откладываю."));
        assert_eq!(later.store.get("kitchen").len(), 2);
        // What is put off, and cancelling.
        assert!(later.list(&context("что отложено")).speech.unwrap().contains("«выключи свет» в "));
        assert_eq!(later.cancel("kitchen", false).speech.as_deref(), Some("Отменила: «передай маме купи хлеба»."));
        assert_eq!(later.cancel("kitchen", true).speech.as_deref(), Some("Отменила отложенные: 1."));
        assert_eq!(later.cancel("kitchen", true).speech.as_deref(), Some("Отложенных команд нет."));
    }

    #[derive(Default)]
    struct Station {
        ran: Mutex<Vec<String>>,
        busy: AtomicBool,
        calls: AtomicUsize,
    }

    impl CommandRunner for Station {
        fn busy(&self, _: &str) -> bool {
            self.busy.load(Ordering::Relaxed)
        }
        fn run(&self, _: &str, command: String) -> BoxFuture<'static, bool> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.ran.lock().unwrap().push(command);
            Box::pin(async { true })
        }
    }

    #[tokio::test]
    async fn runs_what_is_due_waits_for_a_busy_station_and_drops_what_is_too_late() {
        let later = LaterSkill::new(StateStore::open(Arc::new(Db::memory()), "later_commands"));
        let station = Arc::new(Station::default());
        later.attach_runner(station.clone());
        let told: Arc<Mutex<Vec<String>>> = Arc::default();
        let heard = told.clone();
        later.set_notice(Arc::new(move |text| heard.lock().unwrap().push(text)));
        let now = now_ms() as i64;
        let command = |id: &str, words: &str, ago: i64| LaterCommand {
            id: id.into(),
            command: words.into(),
            at: now - ago,
            set_at: now - 600_000,
        };
        later
            .store
            .set("kitchen", vec![command("a", "выключи свет", 1_000), command("b", "включи радио", 30 * MINUTE_MS)]);
        // Busy: nothing runs, the late one is dropped and the owner told.
        station.busy.store(true, Ordering::Relaxed);
        later.deliver().await;
        assert_eq!(station.calls.load(Ordering::Relaxed), 0);
        assert_eq!(later.store.get("kitchen").len(), 1);
        assert!(told.lock().unwrap()[0].contains("«включи радио» не выполнена"));
        // Free: it runs and goes.
        station.busy.store(false, Ordering::Relaxed);
        later.deliver().await;
        assert_eq!(*station.ran.lock().unwrap(), ["выключи свет"]);
        assert!(later.store.get("kitchen").is_empty());
    }
}
