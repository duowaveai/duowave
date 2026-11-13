//! The plugins' schedules (docs/plans/plugin-schedules.md, stages 1–2): a plugin acts by itself in time. Rhythms come
//! from its manifest («по будням в 7:30», «каждые 2 часа с 9 до 21»); the gateway keeps the times and loads the plugin
//! only for its moment, on a 15 s tick as the built-in reminders. A job either calls the plugin's function, whose
//! answer the gateway carries out (`say` — the station speaks, right `voice.say`), or carries a ready answer the
//! gateway says without loading the plugin at all.
//!
//! The fuses are the gateway's, whatever a plugin does (the plan's «Предохранители»):
//! - at most 12 speeches an hour per plugin, the rest journaled «не сказано: слишком часто»;
//! - after a stop (the Mac slept, the gateway was off) at most 3 late speeches in a row per plugin; repeats never
//!   catch up, a once late by more than 6 hours is dropped;
//! - speech waits while the station rings or someone talks to it, and while it is offline: a once up to 6 hours, a
//!   repeat until its next time;
//! - 5 failures in a row pause a job, with the reason;
//! - PLUGINS_SCHEDULES=off (and the panel's switch) stops them all, the jobs kept;
//! - only the gateway that keeps the station's session runs them: a computer that only helps a station does not, or
//!   they would run twice.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration as StdDuration, Instant};

use chrono::{Datelike, Duration, NaiveDateTime, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::controller::PluginHost;
use super::manifest::{asks, identifier, valid_id, Permission, ScheduleSpec};
use super::runtime::CallKind;
use crate::bus::Level;
use crate::skills::own::alarms_skill::DEFAULT_TIMEZONE;
use crate::storage::{Db, StateStore};
use crate::utils::time::now_ms;

/// The jobs' kind in the `state` table: a move carries it with the plugins' data (placement/transfer.rs).
pub const STORE: &str = "plugin_schedules";
const TICK: StdDuration = StdDuration::from_secs(15);
/// A once late by more than this (the gateway was off) is dropped; its speech waits for the station this long.
const GIVE_UP_MS: i64 = 6 * 3_600_000;
/// Late by more than this, a job counts as late (a stop came between).
const LATE_MS: i64 = 60_000;
/// At most this many late speeches in a row per plugin, the rest «пропущено».
const LATE_IN_A_ROW: u32 = 3;
/// At most this many speeches an hour per plugin.
const SPEECHES_AN_HOUR: usize = 12;
/// Failures in a row that pause a job.
const FAILURES: u32 = 5;
/// Rhythms in a manifest, at most.
pub const MAX_RHYTHMS: usize = 10;
/// `every`: from 5 minutes to a day.
const EVERY_MIN: u32 = 5;
const EVERY_MAX: u32 = 1440;
/// `every`'s window of the day when the manifest names none (the owner's decision 1).
const WINDOW: (&str, &str) = ("08:00", "22:00");
const TITLE_MAX: usize = 40;
/// Tasks the code sets, at most, per plugin.
const MAX_TASKS: usize = 50;
/// `set` calls an hour, at most, per plugin: a function that sets itself again and again stops here.
const SETS_AN_HOUR: usize = 60;
/// A task's `data`, at most, as JSON.
const DATA_MAX: usize = 4 * 1024;
/// A once at most a year ahead, and not sooner than a minute: shorter is the turn timer's, in a conversation.
const YEAR_MS: i64 = 366 * 86_400_000;
const MINUTE_MS: i64 = 60_000;
const WEEK: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// The days a time of day holds on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Days {
    Daily,
    Weekdays,
    Weekends,
    /// Days of the week, Monday 0.
    On(Vec<u8>),
}

impl Days {
    fn holds(&self, day: Weekday) -> bool {
        let index = day.num_days_from_monday() as u8;
        match self {
            Days::Daily => true,
            Days::Weekdays => index < 5,
            Days::Weekends => index >= 5,
            Days::On(days) => days.contains(&index),
        }
    }

    fn parse(value: Option<&Value>) -> Result<Days, String> {
        match value {
            None => Ok(Days::Daily),
            Some(Value::String(text)) => match text.as_str() {
                "daily" => Ok(Days::Daily),
                "weekdays" => Ok(Days::Weekdays),
                "weekends" => Ok(Days::Weekends),
                other => Err(format!("days «{other}» — daily, weekdays, weekends или список дней")),
            },
            Some(Value::Array(days)) => {
                let mut on = Vec::new();
                for day in days {
                    let name = day.as_str().unwrap_or_default();
                    let index = WEEK
                        .iter()
                        .position(|known| *known == name)
                        .ok_or_else(|| format!("день «{name}» — mon … sun"))?;
                    if !on.contains(&(index as u8)) {
                        on.push(index as u8);
                    }
                }
                if on.is_empty() {
                    return Err("days — пустой список".into());
                }
                on.sort_unstable();
                Ok(Days::On(on))
            }
            Some(_) => Err("days — строка или список дней".into()),
        }
    }
}

/// When a job comes due.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum When {
    /// A time of day where the station is — `HH:MM`, or `{key}`: a string setting of the plugin, read at each count —
    /// on some days.
    At { at: String, days: Days },
    /// Every so many minutes within a window of the day (`HH:MM`–`HH:MM`, `24:00` the day's end).
    Every { minutes: u32, from: String, to: String },
    /// Once, at a moment (ms since the epoch).
    Once { at_ms: i64 },
}

/// What a job does when it comes due: the plugin's function, or a ready answer.
///
/// A ready answer (`say`, `notify`) instead of a handler: the gateway says or sends it itself and never loads the
/// plugin — nothing out of the station's memory, and a phone can make it a system notification (the owner's
/// decision 8, 12.11.2026).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Then {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handler: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub say: Option<String>,
    /// A message to the owner (right `notify`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<String>,
}

/// How a job ended the last time it came due.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastRun {
    pub at_ms: i64,
    pub outcome: String,
}

/// A plugin's job as the gateway keeps it: a rhythm of its manifest or a task its code set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub title: String,
    /// A rhythm of the manifest (the code cannot take it away; a person pauses it), or a task of the code.
    pub manifest: bool,
    pub when: When,
    pub then: Then,
    /// What the code gave the task, back to its function.
    #[serde(default)]
    pub data: Value,
    /// The station it speaks on: the one of the conversation it was set in; none — whichever is connected.
    #[serde(default)]
    pub device: Option<String>,
    /// When it comes due next (ms since the epoch); none — never again (a once that went).
    #[serde(default)]
    pub next_ms: Option<i64>,
    #[serde(default)]
    pub paused: bool,
    /// Why the gateway paused it (failures in a row).
    #[serde(default)]
    pub problem: Option<String>,
    #[serde(default)]
    pub failures: u32,
    /// Speech that waits for the station (offline, busy), and until when.
    #[serde(default)]
    pub pending: Option<(String, i64)>,
    #[serde(default)]
    pub last: Option<LastRun>,
}

impl Job {
    fn of(spec: &ScheduleSpec) -> Job {
        Job {
            id: spec.id.clone(),
            title: spec.title.clone(),
            manifest: true,
            when: spec.when.clone(),
            then: spec.then.clone(),
            data: Value::Null,
            device: None,
            next_ms: None,
            paused: false,
            problem: None,
            failures: 0,
            pending: None,
            last: None,
        }
    }

    fn repeats(&self) -> bool {
        !matches!(self.when, When::Once { .. })
    }
}

/// `HH:MM` (or `H:MM`) as hours and minutes; 24:00 is not a time of day.
fn clock(text: &str) -> Option<(u32, u32)> {
    let (hours, minutes) = text.trim().split_once(':')?;
    let two = minutes.len() == 2 && (1..=2).contains(&hours.len());
    let (hours, minutes): (u32, u32) = (hours.parse().ok()?, minutes.parse().ok()?);
    (two && hours < 24 && minutes < 60).then_some((hours, minutes))
}

/// The end of a window in minutes of the day: `HH:MM`, or `24:00` — the day's end.
fn day_end(text: &str) -> Option<u32> {
    if text.trim() == "24:00" {
        return Some(1440);
    }
    clock(text).map(|(hours, minutes)| hours * 60 + minutes)
}

/// `5m`, `2h`, `1d` as minutes.
fn minutes_of(text: &str) -> Option<u32> {
    let text = text.trim();
    let unit = text.chars().last()?;
    let number: u32 = text[..text.len() - unit.len_utf8()].parse().ok()?;
    match unit {
        'm' => Some(number),
        'h' => number.checked_mul(60),
        'd' => number.checked_mul(1440),
        _ => None,
    }
}

/// A rhythm of `contributes.schedules`, checked; None (and the problems said) when it does not read.
pub fn rhythm(item: &Value, at: &str, permissions: &[Permission], problems: &mut Vec<String>) -> Option<ScheduleSpec> {
    let before = problems.len();
    let id = item.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    if !valid_id(&id) {
        problems.push(format!("{at}: id — строчные латинские, цифры и «-»"));
    }
    let title = item.get("title").and_then(Value::as_str).map(str::trim).filter(|title| !title.is_empty());
    let title = title.unwrap_or(&id).to_string();
    if title.chars().count() > TITLE_MAX {
        problems.push(format!("{at}: title — до {TITLE_MAX} знаков"));
    }
    let when = when_of(item, at, problems);
    if item.get("once").is_some() || item.get("after").is_some() {
        problems.push(format!("{at}: once и after — у задач из кода, не у ритма манифеста"));
    }
    let then = then_of(item, at, permissions, problems);
    if problems.len() > before {
        return None;
    }
    Some(ScheduleSpec { id, title, when: when?, then: then? })
}

/// A repeat: `at` (+ `days`) or `every` (+ `from`, `to`) — exactly one.
fn when_of(item: &Value, at: &str, problems: &mut Vec<String>) -> Option<When> {
    match (item.get("at"), item.get("every")) {
        (Some(time), None) => {
            let time = time.as_str().unwrap_or_default().trim().to_string();
            let setting = time.strip_prefix('{').and_then(|rest| rest.strip_suffix('}'));
            if setting.is_none_or(|key| key.is_empty()) && clock(&time).is_none() {
                problems.push(format!("{at}: at «{time}» — ЧЧ:ММ или {{ключ}} строковой настройки"));
            }
            match Days::parse(item.get("days")) {
                Ok(days) => Some(When::At { at: time, days }),
                Err(why) => {
                    problems.push(format!("{at}: {why}"));
                    None
                }
            }
        }
        (None, Some(every)) => {
            let minutes =
                every.as_str().and_then(minutes_of).filter(|minutes| (EVERY_MIN..=EVERY_MAX).contains(minutes));
            if minutes.is_none() {
                problems.push(format!("{at}: every — от 5m до 1d: не чаще раза в 5 минут"));
            }
            let from = item.get("from").and_then(Value::as_str).unwrap_or(WINDOW.0).trim().to_string();
            let to = item.get("to").and_then(Value::as_str).unwrap_or(WINDOW.1).trim().to_string();
            let start = clock(&from).map(|(hours, minutes)| hours * 60 + minutes);
            if !start.zip(day_end(&to)).is_some_and(|(start, end)| start < end) {
                problems.push(format!("{at}: from и to — ЧЧ:ММ, from раньше to (24:00 — конец дня)"));
            }
            if item.get("days").is_some() {
                problems.push(format!("{at}: days — только у at"));
            }
            minutes.map(|minutes| When::Every { minutes, from, to })
        }
        _ => {
            problems.push(format!("{at}: ровно одно из at и every"));
            None
        }
    }
}

fn default_zone() -> Tz {
    DEFAULT_TIMEZONE.parse().unwrap_or(Tz::UTC)
}

/// The workshop's sandbox for `host.schedule.set`: the task checked as the gateway checks it, set nowhere.
pub fn try_task(input: &Value, permissions: &[Permission]) -> Result<Value, String> {
    let (now, zone) = (now_ms() as i64, default_zone());
    let job = task_of(input, now, zone, permissions)?;
    let next = next_after(&job.when, now, zone, &|_| None).ok_or("срок не наступит: нет такого времени")?;
    Ok(json!({ "id": job.id, "next": iso(next, zone) }))
}

/// The workshop's sandbox for `host.schedule.parse`.
pub fn parse_now(text: &str) -> Value {
    parse(text, now_ms() as i64, default_zone()).unwrap_or(Value::Null)
}

/// A moment as the station's clock shows it, with its offset: `2026-11-13T09:00:00+03:00`.
pub fn iso(ms: i64, zone: Tz) -> Option<String> {
    Some(Utc.timestamp_millis_opt(ms).single()?.with_timezone(&zone).to_rfc3339())
}

/// A task the code sets (`host.schedule.set`): `after` (seconds) or `once` (`YYYY-MM-DDTHH:MM` where the station is),
/// or a repeat as a rhythm has it; `handler` or `say`; `id`, `title`, `data`, `device` if it likes.
fn task_of(input: &Value, now: i64, zone: Tz, permissions: &[Permission]) -> Result<Job, String> {
    let mut problems = Vec::new();
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty());
    let id = text("id").map(str::to_string).unwrap_or_else(|| format!("task-{}", now % 1_000_000_000));
    if !valid_id(&id) {
        problems.push("id — строчные латинские, цифры и «-»".into());
    }
    let title = text("title").unwrap_or(&id).to_string();
    if title.chars().count() > TITLE_MAX {
        problems.push(format!("title — до {TITLE_MAX} знаков"));
    }
    let once = |at_ms: i64| -> Result<When, String> {
        if at_ms < now + MINUTE_MS {
            return Err("раньше чем через минуту — это таймер хода, он только в разговоре".into());
        }
        if at_ms > now + YEAR_MS {
            return Err("не дальше года".into());
        }
        Ok(When::Once { at_ms })
    };
    let when = match (input.get("after"), input.get("once"), input.get("at").or(input.get("every"))) {
        (Some(after), None, None) => match after.as_f64() {
            Some(seconds) => once(now + (seconds * 1000.0) as i64).map_err(|why| problems.push(why)).ok(),
            None => {
                problems.push("after — секунды".into());
                None
            }
        },
        (None, Some(moment_at), None) => {
            let wall =
                moment_at.as_str().and_then(|text| NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%dT%H:%M").ok());
            match wall.and_then(|wall| moment(zone, wall)) {
                Some(at_ms) => once(at_ms).map_err(|why| problems.push(why)).ok(),
                None => {
                    problems.push("once — дата и время дома: ГГГГ-ММ-ДДTЧЧ:ММ".into());
                    None
                }
            }
        }
        (None, None, Some(_)) => when_of(input, "повтор", &mut problems),
        _ => {
            problems.push("ровно одно: after, once, at или every".into());
            None
        }
    };
    let then = then_of(input, "задача", permissions, &mut problems);
    let data = input.get("data").cloned().unwrap_or(Value::Null);
    if data.to_string().len() > DATA_MAX {
        problems.push("data — до 4 КБ".into());
    }
    match (when, then) {
        (Some(when), Some(then)) if problems.is_empty() => Ok(Job {
            id,
            title,
            manifest: false,
            when,
            then,
            data,
            device: text("device").map(str::to_string),
            next_ms: None,
            paused: false,
            problem: None,
            failures: 0,
            pending: None,
            last: None,
        }),
        _ => Err(problems.join("; ")),
    }
}

/// «каждый день в девять пить таблетки», «через час выключить чайник»: when, as `host.schedule.set` takes it, and
/// what remains — on the built-in reminders' reading of time (the owner's decision 4). None without a time.
pub fn parse(text: &str, now: i64, zone: Tz) -> Option<Value> {
    use crate::skills::own::reminders_skill::{parse_reminder, ABOUT, CLOCK_WORD, DURATION_WORD};
    use crate::skills::text::{normalize, parse_clock, parse_duration, read_number};
    let normalized = normalize(text);
    let words: Vec<&str> = normalized.split(' ').collect();
    let rest = |from: usize| ABOUT.replace(&words[from.min(words.len())..].join(" "), "").into_owned();
    let starts = |prefix: &[&str]| words.len() > prefix.len() && words[..prefix.len()] == *prefix;
    let days = [
        (&["каждый", "день"][..], "daily"),
        (&["ежедневно"][..], "daily"),
        (&["по", "будням"][..], "weekdays"),
        (&["в", "будни"][..], "weekdays"),
        (&["по", "выходным"][..], "weekends"),
        (&["в", "выходные"][..], "weekends"),
    ];
    if let Some((prefix, days)) = days.iter().find(|(prefix, _)| starts(prefix)) {
        let start = prefix.len() + usize::from(words.get(prefix.len()) == Some(&"в"));
        let mut end = start;
        while end < words.len() && (CLOCK_WORD.is_match(words[end]) || read_number(&words, end).is_some()) {
            end += 1;
        }
        let clock = parse_clock(&words[start..end].join(" "))?;
        let rest = rest(end);
        return (!rest.is_empty()).then(|| {
            json!({ "spec": { "at": format!("{:02}:{:02}", clock.hours, clock.minutes), "days": days }, "rest": rest })
        });
    }
    if words[0] == "каждые" || words[0] == "каждый" || words[0] == "каждую" {
        let mut end = 1;
        while end < words.len() && (DURATION_WORD.is_match(words[end]) || read_number(&words, end).is_some()) {
            end += 1;
        }
        // «каждый час»: one of them.
        let said = words[1..end].join(" ");
        let seconds =
            parse_duration(&said).or_else(|| parse_duration(&format!("1 {said}"))).filter(|seconds| *seconds > 0)?;
        let rest = rest(end);
        return (!rest.is_empty()).then(|| json!({ "spec": { "every": format!("{}m", seconds / 60) }, "rest": rest }));
    }
    let zone_name = zone.name();
    let reminder = parse_reminder(text, Utc.timestamp_millis_opt(now).single()?, zone_name)?;
    let wall = Utc.timestamp_millis_opt(reminder.at as i64).single()?.with_timezone(&zone);
    Some(json!({ "spec": { "once": wall.format("%Y-%m-%dT%H:%M").to_string() }, "rest": reminder.text }))
}

/// What a job does: the plugin's function (`handler`), or a ready answer (`say` and/or `notify`) — one or the other.
pub fn then_of(item: &Value, at: &str, permissions: &[Permission], problems: &mut Vec<String>) -> Option<Then> {
    let text = |key: &str| item.get(key).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty());
    let (handler, say, notify) = (text("handler"), text("say"), text("notify"));
    let short = |text: &str| -> String { text.chars().take(1000).collect() };
    match (handler, say.is_some() || notify.is_some()) {
        (Some(handler), false) if identifier(handler) => {
            Some(Then { handler: Some(handler.into()), say: None, notify: None })
        }
        (Some(handler), false) => {
            problems.push(format!("{at}: handler «{handler}» — имя функции, которую экспортирует backend.js"));
            None
        }
        (None, true) => {
            if say.is_some() && !asks(permissions, "system/voice", Level::Call) {
                problems.push(format!("{at}: say — речь без запроса, нужна связь call:system/voice"));
                return None;
            }
            if notify.is_some() && !asks(permissions, "system/notify", Level::Call) {
                problems.push(format!("{at}: notify — сообщение владельцу, нужна связь call:system/notify"));
                return None;
            }
            Some(Then { handler: None, say: say.map(short), notify: notify.map(short) })
        }
        (Some(_), true) => {
            problems.push(format!("{at}: ровно одно — handler или готовый ответ (say, notify)"));
            None
        }
        (None, false) => {
            problems.push(format!("{at}: нужен handler или готовый ответ say (notify)"));
            None
        }
    }
}

/// A wall-clock time where the station is, as a moment: a time the clock skips (spring) moves on to the first that
/// exists, a time it repeats (autumn) is the first of the two.
fn moment(zone: Tz, wall: NaiveDateTime) -> Option<i64> {
    (0..=180).find_map(|minutes| {
        zone.from_local_datetime(&(wall + Duration::minutes(minutes)))
            .earliest()
            .map(|moment| moment.timestamp_millis())
    })
}

/// When a job comes due next after `after` (ms since the epoch), where the station is; None — never again.
/// `setting` reads a string setting of the plugin for `at: "{key}"`.
pub fn next_after(when: &When, after: i64, zone: Tz, setting: &dyn Fn(&str) -> Option<String>) -> Option<i64> {
    let today = Utc.timestamp_millis_opt(after).single()?.with_timezone(&zone).date_naive();
    match when {
        When::Once { at_ms } => (*at_ms > after).then_some(*at_ms),
        When::At { at, days } => {
            let text = match at.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')) {
                Some(key) => setting(key)?,
                None => at.clone(),
            };
            let (hours, minutes) = clock(&text)?;
            (0..=8).find_map(|shift| {
                let day = today + Duration::days(shift);
                if !days.holds(day.weekday()) {
                    return None;
                }
                moment(zone, day.and_hms_opt(hours, minutes, 0)?).filter(|due| *due > after)
            })
        }
        When::Every { minutes, from, to } => {
            let (hours, start) = clock(from)?;
            let (start, end, step) = (hours * 60 + start, day_end(to)?.min(1439), (*minutes).max(EVERY_MIN));
            (0..=2).find_map(|shift| {
                let day = today + Duration::days(shift);
                (start..=end).step_by(step as usize).find_map(|minute| {
                    moment(zone, day.and_hms_opt(minute / 60, minute % 60, 0)?).filter(|due| *due > after)
                })
            })
        }
    }
}

/// What a job's function answered, as the gateway carries it out: its speech (a message and music come with stages 4
/// and 5).
fn speech_of(answer: &Value) -> Option<String> {
    let text = match answer {
        Value::String(text) => text.as_str(),
        Value::Object(answer) => answer.get("say").and_then(Value::as_str)?,
        _ => return None,
    };
    Some(text.trim().chars().take(1000).collect::<String>()).filter(|text| !text.is_empty())
}

/// What a job's function asked to send the owner (`{notify}`).
fn notice_of(answer: &Value) -> Option<String> {
    let text = answer.get("notify").and_then(Value::as_str)?;
    Some(text.trim().chars().take(1000).collect::<String>()).filter(|text| !text.is_empty())
}

/// The way to the owner (operations/station_watch.rs): the chat «Оповещения», the webhook, the Mac's notifications.
pub trait OwnerNotice: Send + Sync {
    /// Whether a word reaches the owner now; why not, in words for the plugin and the panel.
    fn ready(&self) -> Result<(), String>;
    /// Sends `text` by every way there is.
    fn tell(&self, text: String) -> BoxFuture<'static, ()>;
}

/// Messages to the owner an hour, at most, per plugin.
const NOTICES_AN_HOUR: usize = 20;

/// The station side of the schedules (the session registry).
pub trait ScheduleStation: Send + Sync {
    /// Says `text` on the device (`*`: whichever is connected); false when no station could.
    fn say(&self, device: &str, text: String) -> BoxFuture<'static, bool>;
    /// It rings, or someone talks to it (a conversation going on, words being heard): speech waits.
    fn busy(&self, device: &str) -> bool;
    /// The station's time zone as it said it (the newest connected), if one is.
    fn timezone(&self) -> Option<String>;
}

pub struct Scheduler {
    jobs: StateStore<Job>,
    /// This side's own: the station's last known zone under `station`, the panel's switch under `switch`.
    own: StateStore<String>,
    host: Weak<PluginHost>,
    station: OnceLock<Arc<dyn ScheduleStation>>,
    /// The way to the owner, once the station watch stands.
    notice: OnceLock<Arc<dyn OwnerNotice>>,

    /// When each plugin sent the owner a message within the last hour.
    notices: Mutex<HashMap<String, VecDeque<i64>>>,
    /// This gateway keeps the station's times: a computer that only helps a station does not.
    leads: Arc<dyn Fn() -> bool + Send + Sync>,
    /// PLUGINS_SCHEDULES: off — nothing runs whatever the panel says, the jobs stay.
    allowed: bool,
    /// The panel's switch «Расписания плагинов»: off — nothing runs, the jobs stay.
    on: AtomicBool,
    /// When each plugin spoke by its schedule within the last hour.
    spoken: Mutex<HashMap<String, VecDeque<i64>>>,
    /// When each plugin's code set a task within the last hour.
    sets: Mutex<HashMap<String, VecDeque<i64>>>,
    /// One edit of the jobs at a time: the tick, the code's tasks, the panel.
    edit: Mutex<()>,
    /// The plugins that were on at the last tick: one back on counts its rhythms from now.
    active: Mutex<HashSet<String>>,
    ticking: AtomicBool,
    stop: CancellationToken,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Whether one more fits `limit` an hour for this plugin; counted when it does.
fn within_an_hour(times: &Mutex<HashMap<String, VecDeque<i64>>>, plugin: &str, now: i64, limit: usize) -> bool {
    let mut times = locked(times);
    let times = times.entry(plugin.to_string()).or_default();
    while times.front().is_some_and(|at| now - at >= 3_600_000) {
        times.pop_front();
    }
    if times.len() >= limit {
        return false;
    }
    times.push_back(now);
    true
}

/// A job as the code and the panel see it.
pub fn job_view(job: &Job, zone: Tz) -> Value {
    json!({
        "id": job.id,
        "title": job.title,
        "source": if job.manifest { "manifest" } else { "code" },
        "spec": job.when,
        "say": job.then.say,
        "handler": job.then.handler,
        "next": job.next_ms.and_then(|next| iso(next, zone)),
        "paused": job.paused,
        "problem": job.problem,
        "last": job.last,
    })
}

impl Scheduler {
    pub fn new(db: Arc<Db>, host: Weak<PluginHost>, leads: Arc<dyn Fn() -> bool + Send + Sync>, on: bool) -> Scheduler {
        Scheduler {
            jobs: StateStore::open(db.clone(), STORE),
            own: StateStore::open(db, "plugin_schedules_own"),
            host,
            station: OnceLock::new(),
            notice: OnceLock::new(),
            notices: Mutex::new(HashMap::new()),
            leads,
            allowed: on,
            on: AtomicBool::new(true),
            spoken: Mutex::new(HashMap::new()),
            sets: Mutex::new(HashMap::new()),
            edit: Mutex::new(()),
            active: Mutex::new(HashSet::new()),
            ticking: AtomicBool::new(false),
            stop: CancellationToken::new(),
        }
        .switched()
    }

    /// The panel's switch as it was left.
    fn switched(self) -> Scheduler {
        let off = self.own.get("switch").first().is_some_and(|switch| switch == "off");
        self.on.store(!off, Ordering::Relaxed);
        self
    }

    /// The stations, once the session registry stands.
    pub fn set_station(&self, station: Arc<dyn ScheduleStation>) {
        let _ = self.station.set(station);
    }

    /// The way to the owner, once the station watch stands.
    /// Whether `plugin` may reach `to`'s `handle` by its schedule: the bus's word through the plugins' host (its links,
    /// the owner's «да»), or without a host what its manifest asked for.
    fn may(&self, plugin: &str, permissions: &[Permission], to: &str, handle: &str) -> Result<(), String> {
        match self.host.upgrade() {
            Some(host) => host.may(plugin, to, handle),
            None if asks(permissions, to, Level::Call) => Ok(()),
            None => Err(format!("нет связи call:{to} в манифесте")),
        }
    }

    pub fn set_notice(&self, notice: Arc<dyn OwnerNotice>) {
        let _ = self.notice.set(notice);
    }

    /// A plugin's message to the owner (`host.notify`, `{notify}`, a ready `notify`): the plugin's name first, never
    /// anyone else, 20 an hour at most per plugin.
    pub async fn notify(&self, plugin: &str, text: &str) -> Result<(), String> {
        let notice = self.notice.get().cloned().ok_or("Сообщение владельцу отсюда не отправить.")?;
        notice.ready()?;
        let text: String = text.trim().chars().take(1000).collect();
        if text.is_empty() {
            return Err("пустое сообщение".into());
        }
        if !within_an_hour(&self.notices, plugin, now_ms() as i64, NOTICES_AN_HOUR) {
            return Err(format!("не больше {NOTICES_AN_HOUR} сообщений в час"));
        }
        let title =
            self.host.upgrade().and_then(|host| host.package(plugin)).map(|package| package.manifest.title.clone());
        notice.tell(format!("{}: {text}", title.as_deref().unwrap_or(plugin))).await;
        Ok(())
    }

    /// The panel's switch «Расписания плагинов» for every plugin, kept over a restart; the jobs stay either way.
    pub fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
        self.own.set("switch", if on { Vec::new() } else { vec!["off".into()] });
    }

    /// The schedules run: PLUGINS_SCHEDULES allows them and the panel's switch is on.
    pub fn is_on(&self) -> bool {
        self.allowed && self.on.load(Ordering::Relaxed)
    }

    /// What the panel shows of the switch: whether it is on, and whether PLUGINS_SCHEDULES lets it be.
    pub fn switch_view(&self) -> Value {
        json!({ "on": self.on.load(Ordering::Relaxed), "allowed": self.allowed })
    }

    /// A plugin's schedule for the panel: its jobs, when each comes next where the station is.
    pub fn view(&self, plugin: &str) -> Value {
        let zone = self.zone_now();
        let jobs: Vec<Value> = self.jobs.get(plugin).iter().map(|job| job_view(job, zone)).collect();
        json!({ "on": self.is_on(), "jobs": jobs })
    }

    /// The panel's «Пауза»: a job pauses or goes on (a job paused for its failures starts them over).
    pub fn pause(&self, plugin: &str, id: &str, paused: bool) -> Result<(), String> {
        let host = self.host.upgrade().ok_or("приложений нет")?;
        let (now, zone) = (now_ms() as i64, self.zone_now());
        {
            let _edit = locked(&self.edit);
            let mut jobs = self.jobs.get(plugin);
            let job = jobs.iter_mut().find(|job| job.id == id).ok_or_else(|| format!("нет задачи «{id}»"))?;
            job.paused = paused;
            if !paused {
                (job.failures, job.problem) = (0, None);
                if job.repeats() {
                    job.next_ms = next_after(&job.when, now, zone, &|key| Scheduler::setting(&host, plugin, key));
                }
            }
            self.jobs.set(plugin, jobs);
        }
        host.schedule_changed(plugin);
        Ok(())
    }

    /// The panel's «Снять»: a task of the code goes; a rhythm of the manifest only pauses.
    pub fn remove(&self, plugin: &str, id: &str) -> Result<bool, String> {
        let gone = self.code(plugin, "cancel", json!({ "id": id }))?.as_bool().unwrap_or(false);
        if let Some(host) = self.host.upgrade() {
            host.schedule_changed(plugin);
        }
        Ok(gone)
    }

    /// The panel's «Запустить сейчас»: the job runs as if it came due now, by the same rules; what it did.
    pub async fn run_now(&self, plugin: &str, id: &str) -> Result<String, String> {
        let host = self.host.upgrade().ok_or("приложений нет")?;
        let package = host.package(plugin).ok_or("приложение выключено")?;
        let (now, zone) = (now_ms() as i64, self.zone_now());
        let mut job =
            self.jobs.get(plugin).into_iter().find(|job| job.id == id).ok_or_else(|| format!("нет задачи «{id}»"))?;
        let next = job.next_ms;
        (job.next_ms, job.paused) = (Some(now), false);
        let mut late = 0;
        self.run(&host, plugin, &package.manifest.permissions, &mut job, now, zone, &mut late).await;
        let outcome = job.last.as_ref().map(|last| last.outcome.clone()).unwrap_or_default();
        {
            let _edit = locked(&self.edit);
            let mut jobs = self.jobs.get(plugin);
            if let Some(slot) = jobs.iter_mut().find(|stored| stored.id == id) {
                // A repeat keeps its time; a once that ran is done.
                (job.paused, job.next_ms) = (slot.paused, if job.repeats() { next.or(job.next_ms) } else { None });
                *slot = job;
            }
            jobs.retain(|job| job.repeats() || job.next_ms.is_some() || job.pending.is_some());
            self.jobs.set(plugin, jobs);
        }
        host.schedule_changed(plugin);
        Ok(outcome)
    }

    /// A plugin's jobs as they stand.
    pub fn jobs(&self, plugin: &str) -> Vec<Job> {
        self.jobs.get(plugin)
    }

    /// A plugin's jobs for the history's snapshot of its data (history/data.rs): without what changes by itself —
    /// the last run and the speech waiting for the station.
    pub fn export(&self, plugin: &str) -> Vec<Value> {
        self.jobs
            .get(plugin)
            .iter()
            .filter_map(|job| {
                let mut value = serde_json::to_value(job).ok()?;
                if let Some(fields) = value.as_object_mut() {
                    fields.remove("last");
                    fields.remove("pending");
                }
                Some(value)
            })
            .collect()
    }

    /// A plugin's jobs replaced by those the history kept: a repeat counts its next time from now, a once already
    /// past is left out — a step back of the data never says yesterday's reminder again.
    pub fn import(&self, plugin: &str, jobs: &[Value]) -> Result<(), String> {
        let jobs: Vec<Job> = jobs
            .iter()
            .map(|job| serde_json::from_value(job.clone()))
            .collect::<Result<_, _>>()
            .map_err(|error| format!("задачи не читаются: {error}"))?;
        let host = self.host.upgrade();
        let (now, zone) = (now_ms() as i64, self.zone_now());
        let jobs: Vec<Job> = jobs
            .into_iter()
            .filter_map(|mut job| {
                if job.repeats() {
                    if let Some(host) = &host {
                        job.next_ms = next_after(&job.when, now, zone, &|key| Scheduler::setting(host, plugin, key));
                    }
                    Some(job)
                } else {
                    job.next_ms.is_some_and(|next| next > now).then_some(job)
                }
            })
            .collect();
        {
            let _edit = locked(&self.edit);
            self.jobs.set(plugin, jobs);
        }
        if let Some(host) = host {
            host.schedule_changed(plugin);
        }
        Ok(())
    }

    /// The tick every 15 s until `stop`; inside the runtime.
    pub fn start(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let me = Arc::downgrade(self);
        let stop = self.stop.clone();
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(TICK);
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = ticks.tick() => {}
                }
                let Some(scheduler) = me.upgrade() else { break };
                scheduler.tick().await;
            }
        })
    }

    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// The station's zone: as it says it now, else the last it said, else the gateway's default.
    fn zone_now(&self) -> Tz {
        let said = self.station.get().and_then(|station| station.timezone()).filter(|zone| zone.parse::<Tz>().is_ok());
        if let Some(zone) = &said {
            if self.own.get("station").first() != Some(zone) {
                self.own.set("station", vec![zone.clone()]);
            }
        }
        said.or_else(|| self.own.get("station").into_iter().next())
            .and_then(|zone| zone.parse().ok())
            .unwrap_or_else(default_zone)
    }

    /// A string setting of a plugin, as its code reads it.
    fn setting(host: &PluginHost, plugin: &str, key: &str) -> Option<String> {
        host.settings_of(plugin).get(key).and_then(Value::as_str).map(str::to_string)
    }

    /// Every job due is run, after the manifests' rhythms are brought in line with the plugins in force.
    pub async fn tick(&self) {
        if !self.is_on() || !(self.leads)() || self.ticking.swap(true, Ordering::SeqCst) {
            return;
        }
        let now = now_ms() as i64;
        let zone = self.zone_now();
        self.reconcile(now, zone);
        let host = self.host.upgrade();
        // Short of memory, work in the background waits; so do the schedules (budget.rs).
        if let Some(host) = host.filter(|host| !host.deferring()) {
            for plugin in self.jobs.devices() {
                let Some(package) = host.package(&plugin) else { continue };
                let mut jobs = self.jobs.get(&plugin);
                let before = jobs.clone();
                let mut late = 0;
                for job in jobs.iter_mut() {
                    self.run(&host, &plugin, &package.manifest.permissions, job, now, zone, &mut late).await;
                }
                if jobs != before {
                    // A task the code set while the job ran is not lost: only the jobs run are written back.
                    let _edit = locked(&self.edit);
                    let mut stored = self.jobs.get(&plugin);
                    for job in jobs {
                        if let Some(slot) = stored.iter_mut().find(|stored| stored.id == job.id) {
                            *slot = job;
                        }
                    }
                    stored.retain(|job| job.repeats() || job.next_ms.is_some() || job.pending.is_some());
                    self.jobs.set(&plugin, stored);
                    host.schedule_changed(&plugin);
                }
            }
        }
        self.ticking.store(false, Ordering::SeqCst);
    }

    /// The manifests' rhythms as the plugins in force have them; a plugin gone takes its jobs along, one back on counts
    /// its rhythms from now (what it missed is not caught up).
    fn reconcile(&self, now: i64, zone: Tz) {
        let Some(host) = self.host.upgrade() else { return };
        let _edit = locked(&self.edit);
        let installed: HashSet<String> = host.ids().into_iter().collect();
        for plugin in self.jobs.devices() {
            if !installed.contains(&plugin) && !self.jobs.get(&plugin).is_empty() {
                self.jobs.set(&plugin, Vec::new());
            }
        }
        let mut active = locked(&self.active);
        let mut now_active = HashSet::new();
        for plugin in installed {
            let Some(package) = host.package(&plugin) else { continue };
            let woke = !active.contains(&plugin);
            now_active.insert(plugin.clone());
            let specs = &package.manifest.contributes.schedules;
            let mut jobs = self.jobs.get(&plugin);
            if specs.is_empty() && jobs.is_empty() {
                continue;
            }
            let before = jobs.clone();
            jobs.retain(|job| !job.manifest || specs.iter().any(|spec| spec.id == job.id));
            for spec in specs {
                match jobs.iter_mut().find(|job| job.manifest && job.id == spec.id) {
                    Some(job) if job.when != spec.when || job.then != spec.then || job.title != spec.title => {
                        (job.when, job.then, job.title, job.next_ms) =
                            (spec.when.clone(), spec.then.clone(), spec.title.clone(), None);
                    }
                    Some(_) => {}
                    None => jobs.push(Job::of(spec)),
                }
            }
            for job in jobs.iter_mut().filter(|job| job.repeats() && (woke || job.next_ms.is_none())) {
                job.next_ms = next_after(&job.when, now, zone, &|key| Scheduler::setting(&host, &plugin, key));
            }
            if jobs != before {
                self.jobs.set(&plugin, jobs);
            }
        }
        *active = now_active;
    }

    /// Whether the plugin may speak once more this hour; counts it when it may.
    fn may_speak(&self, plugin: &str, now: i64) -> bool {
        within_an_hour(&self.spoken, plugin, now, SPEECHES_AN_HOUR)
    }

    /// The code's own schedule (`host.schedule.*` through `host.call`, right `schedule`): `set`, `list`, `cancel`,
    /// `parse`.
    pub fn code(&self, plugin: &str, action: &str, input: Value) -> Result<Value, String> {
        let host = self.host.upgrade().ok_or("приложений нет")?;
        let package = host.package(plugin).ok_or("приложение выключено")?;
        let (now, zone) = (now_ms() as i64, self.zone_now());
        let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_string);
        match action {
            "set" => {
                if !within_an_hour(&self.sets, plugin, now, SETS_AN_HOUR) {
                    return Err(format!("больше {SETS_AN_HOUR} в час"));
                }
                let mut job = task_of(&input, now, zone, &package.manifest.permissions)?;
                job.next_ms = next_after(&job.when, now, zone, &|key| Scheduler::setting(&host, plugin, key));
                let Some(next) = job.next_ms else {
                    return Err("срок не наступит: нет такого времени".into());
                };
                let _edit = locked(&self.edit);
                let mut jobs = self.jobs.get(plugin);
                if jobs.iter().any(|known| known.manifest && known.id == job.id) {
                    return Err(format!("«{}» — ритм манифеста: его ставит на паузу человек в пульте", job.id));
                }
                jobs.retain(|known| known.id != job.id);
                if jobs.iter().filter(|known| !known.manifest).count() >= MAX_TASKS {
                    return Err(format!("больше {MAX_TASKS} задач"));
                }
                let answer = json!({ "id": job.id, "next": iso(next, zone) });
                host.note_line(
                    plugin,
                    "info",
                    &format!("{}: поставлено, срок — {}", job.id, iso(next, zone).unwrap_or_default()),
                );
                jobs.push(job);
                self.jobs.set(plugin, jobs);
                host.schedule_changed(plugin);
                Ok(answer)
            }
            "list" => Ok(Value::Array(self.jobs.get(plugin).iter().map(|job| job_view(job, zone)).collect())),
            "cancel" => {
                let id = text("id").ok_or("cancel(id): id задачи")?;
                let _edit = locked(&self.edit);
                let mut jobs = self.jobs.get(plugin);
                if jobs.iter().any(|known| known.manifest && known.id == id) {
                    return Err(format!("«{id}» — ритм манифеста: его ставит на паузу человек в пульте"));
                }
                let before = jobs.len();
                jobs.retain(|known| known.id != id);
                let gone = jobs.len() < before;
                if gone {
                    self.jobs.set(plugin, jobs);
                }
                Ok(Value::Bool(gone))
            }
            "parse" => Ok(parse(&text("text").unwrap_or_default(), now, zone).unwrap_or(Value::Null)),
            other => Err(format!("нет функции schedule.{other}")),
        }
    }

    /// Says a job's words, or keeps them waiting for the station until `until`.
    async fn speak(&self, job: &mut Job, text: String, until: i64) -> String {
        let device = job.device.clone().unwrap_or_else(|| "*".into());
        let Some(station) = self.station.get().cloned() else {
            job.pending = Some((text, until));
            return "ждёт колонку".into();
        };
        if station.busy(&device) {
            job.pending = Some((text, until));
            return "ждёт: колонка занята".into();
        }
        if station.say(&device, text.clone()).await {
            job.pending = None;
            return format!("сказал «{text}»");
        }
        job.pending = Some((text, until));
        "ждёт колонку".into()
    }

    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        host: &PluginHost,
        plugin: &str,
        permissions: &[Permission],
        job: &mut Job,
        now: i64,
        zone: Tz,
        late_said: &mut u32,
    ) {
        // Words still waiting for the station go first, while they are still wanted.
        if let Some((text, until)) = job.pending.clone() {
            if now > until {
                job.pending = None;
                host.note_line(plugin, "warn", &format!("{} (по расписанию): не сказано — колонки не было", job.id));
            } else {
                let outcome = self.speak(job, text, until).await;
                if job.pending.is_none() {
                    host.note_line(plugin, "info", &format!("{} (по расписанию): {outcome}", job.id));
                }
            }
        }
        if job.paused {
            return;
        }
        let Some(due) = job.next_ms.filter(|due| *due <= now) else { return };
        let late = now - due;
        // A repeat counts on from now: a slow call or a stop never runs it twice, and what it missed is not caught up.
        job.next_ms = if job.repeats() {
            next_after(&job.when, now, zone, &|key| Scheduler::setting(host, plugin, key))
        } else {
            None
        };
        let done = |job: &mut Job, outcome: String, level: &str| {
            host.note_line(plugin, level, &format!("{} (по расписанию): {outcome}", job.id));
            job.last = Some(LastRun { at_ms: now, outcome });
        };
        if !job.repeats() && late > GIVE_UP_MS {
            return done(job, format!("пропущено: опоздала на {} ч", late / 3_600_000), "warn");
        }
        if late > LATE_MS {
            if *late_said >= LATE_IN_A_ROW {
                return done(job, "пропущено: после перерыва — не больше трёх подряд".into(), "warn");
            }
            *late_said += 1;
        }
        let began = Instant::now();
        let said = match job.then.handler.clone() {
            Some(handler) => {
                let input = json!({
                    "schedule": { "id": job.id, "title": job.title, "due": due, "late": late / 1000 },
                    "data": job.data,
                });
                match host.call(plugin, &handler, input, CallKind::Background).await {
                    Ok(answer) => {
                        job.failures = 0;
                        job.problem = None;
                        (speech_of(&answer), notice_of(&answer))
                    }
                    Err(error) => {
                        job.failures += 1;
                        if job.failures >= FAILURES {
                            job.paused = true;
                            job.problem = Some(format!("{FAILURES} ошибок подряд: {error}"));
                        }
                        return done(job, format!("ошибка: {error}"), "error");
                    }
                }
            }
            None => (job.then.say.clone(), job.then.notify.clone()),
        };
        let ms = began.elapsed().as_millis();
        let (said, notice) = said;
        let mut outcome = vec![format!("ответил за {ms} мс")];
        // A message to the owner first: it goes now whatever the station does.
        if let Some(notice) = notice {
            outcome.push(match self.may(plugin, permissions, "system/notify", "send") {
                Err(why) => format!("не прислано: {why}"),
                Ok(()) => match self.notify(plugin, &notice).await {
                    Ok(()) => format!("прислал «{notice}»"),
                    Err(why) => format!("не прислано: {why}"),
                },
            });
        }
        let speech = said.as_ref().map(|_| self.may(plugin, permissions, "system/voice", "say"));
        match said {
            None if outcome.len() == 1 => outcome.push("молча".into()),
            None => {}
            Some(_) if matches!(speech, Some(Err(_))) => {
                outcome.push(format!("не сказано: {}", speech.and_then(Result::err).unwrap_or_default()))
            }
            Some(_) if !self.may_speak(plugin, now) => outcome.push("не сказано: слишком часто".into()),
            Some(text) => {
                // Waits for the station: a once up to 6 hours, a repeat until its next time.
                let until = match job.next_ms {
                    Some(next) if job.repeats() => next,
                    _ => due + GIVE_UP_MS,
                };
                outcome.push(self.speak(job, text, until).await);
            }
        }
        let warn = outcome.iter().any(|part| part.starts_with("не "));
        done(job, outcome.join(", "), if warn { "warn" } else { "info" });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn right(text: &str) -> Permission {
        Permission::parse(text).unwrap()
    }

    fn at(text: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(text).unwrap().timestamp_millis()
    }

    fn moscow() -> Tz {
        "Europe/Moscow".parse().unwrap()
    }

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn counts_a_time_of_day_on_its_days_where_the_station_is() {
        let weekdays = When::At { at: "07:30".into(), days: Days::Weekdays };
        // Friday 13.11.2026 08:00 in Moscow: the next weekday at 7:30 is Monday.
        let friday = at("2026-11-13T08:00:00+03:00");
        assert_eq!(next_after(&weekdays, friday, moscow(), &none), Some(at("2026-11-16T07:30:00+03:00")));
        // Before 7:30 on Friday: the same day.
        assert_eq!(
            next_after(&weekdays, at("2026-11-13T07:00:00+03:00"), moscow(), &none),
            Some(at("2026-11-13T07:30:00+03:00"))
        );
        let days = When::At { at: "09:00".into(), days: Days::On(vec![2]) };
        assert_eq!(next_after(&days, friday, moscow(), &none), Some(at("2026-11-18T09:00:00+03:00")), "Wednesday");
        // A setting gives the time; without it, never.
        let set = When::At { at: "{time}".into(), days: Days::Daily };
        let six = |key: &str| (key == "time").then(|| "06:15".to_string());
        assert_eq!(next_after(&set, friday, moscow(), &six), Some(at("2026-11-14T06:15:00+03:00")));
        assert_eq!(next_after(&set, friday, moscow(), &none), None);
        // The end of the month and of the year.
        assert_eq!(
            next_after(
                &When::At { at: "00:05".into(), days: Days::Daily },
                at("2026-12-31T23:00:00+03:00"),
                moscow(),
                &none
            ),
            Some(at("2027-01-01T00:05:00+03:00"))
        );
    }

    #[test]
    fn counts_every_so_often_inside_the_window_of_the_day() {
        let water = When::Every { minutes: 120, from: "09:00".into(), to: "21:00".into() };
        assert_eq!(
            next_after(&water, at("2026-11-13T09:30:00+03:00"), moscow(), &none),
            Some(at("2026-11-13T11:00:00+03:00"))
        );
        // 21:00 is the last; after it — tomorrow's first.
        assert_eq!(
            next_after(&water, at("2026-11-13T20:00:00+03:00"), moscow(), &none),
            Some(at("2026-11-13T21:00:00+03:00"))
        );
        assert_eq!(
            next_after(&water, at("2026-11-13T21:00:00+03:00"), moscow(), &none),
            Some(at("2026-11-14T09:00:00+03:00"))
        );
        let all_day = When::Every { minutes: 30, from: "00:00".into(), to: "24:00".into() };
        assert_eq!(
            next_after(&all_day, at("2026-11-13T23:45:00+03:00"), moscow(), &none),
            Some(at("2026-11-14T00:00:00+03:00"))
        );
    }

    #[test]
    fn a_time_the_clock_skips_moves_on_and_one_it_repeats_is_the_first() {
        let berlin: Tz = "Europe/Berlin".parse().unwrap();
        // 29.03.2026: 02:00 → 03:00 in Berlin; 02:30 does not exist, so 03:00 (+02:00) it is.
        let skipped = When::At { at: "02:30".into(), days: Days::Daily };
        assert_eq!(
            next_after(&skipped, at("2026-03-29T00:00:00+01:00"), berlin, &none),
            Some(at("2026-03-29T03:00:00+02:00"))
        );
        // 25.10.2026: 03:00 → 02:00; 02:30 comes twice, the first (+02:00) is taken.
        assert_eq!(
            next_after(&skipped, at("2026-10-25T00:00:00+02:00"), berlin, &none),
            Some(at("2026-10-25T02:30:00+02:00"))
        );
    }

    #[test]
    fn reads_the_manifests_rhythms_and_says_what_is_wrong() {
        let rights = [right("call:system/schedule"), right("call:system/voice")];
        let mut problems = Vec::new();
        let brief = rhythm(
            &json!({ "id": "brief", "title": "Сводка", "at": "07:30", "days": "weekdays", "handler": "brief" }),
            "s[0]",
            &rights,
            &mut problems,
        )
        .unwrap();
        assert_eq!(brief.when, When::At { at: "07:30".into(), days: Days::Weekdays });
        let water =
            rhythm(&json!({ "id": "water", "every": "2h", "say": "Пора попить воды" }), "s[1]", &rights, &mut problems)
                .unwrap();
        assert_eq!(water.when, When::Every { minutes: 120, from: "08:00".into(), to: "22:00".into() });
        assert_eq!(water.then.say.as_deref(), Some("Пора попить воды"));
        assert!(problems.is_empty(), "{problems:?}");
        let wrong = [
            (json!({ "id": "a", "every": "1m", "handler": "f" }), "не чаще раза в 5 минут"),
            (json!({ "id": "b", "at": "25:00", "handler": "f" }), "ЧЧ:ММ"),
            (json!({ "id": "c", "at": "07:00", "every": "1h", "handler": "f" }), "ровно одно из at и every"),
            (json!({ "id": "d", "at": "07:00", "handler": "f", "say": "x" }), "ровно одно — handler"),
            (json!({ "id": "e", "at": "07:00" }), "нужен handler или готовый ответ"),
            (json!({ "id": "f", "every": "1h", "from": "22:00", "to": "08:00", "handler": "f" }), "from раньше to"),
            (json!({ "id": "g", "at": "07:00", "days": ["mon", "funday"], "handler": "f" }), "mon … sun"),
        ];
        for (item, why) in wrong {
            let mut problems = Vec::new();
            assert!(rhythm(&item, "s", &rights, &mut problems).is_none(), "{item}");
            assert!(problems.iter().any(|problem| problem.contains(why)), "{item}: {problems:?}");
        }
        // A ready `say` is speech without a request: the right voice.say first.
        let mut problems = Vec::new();
        assert!(rhythm(
            &json!({ "id": "w", "every": "1h", "say": "x" }),
            "s",
            &[right("call:system/schedule")],
            &mut problems
        )
        .is_none());
        assert!(problems[0].contains("call:system/voice"), "{problems:?}");
    }

    /// The station as the session registry gives it to the schedules: what it said, busy or not, there or not.
    #[derive(Default)]
    struct Station {
        said: Mutex<Vec<String>>,
        busy: AtomicBool,
        away: AtomicBool,
    }

    impl ScheduleStation for Station {
        fn say(&self, _device: &str, text: String) -> BoxFuture<'static, bool> {
            let there = !self.away.load(Ordering::Relaxed);
            if there {
                self.said.lock().unwrap().push(text);
            }
            Box::pin(async move { there })
        }

        fn busy(&self, _: &str) -> bool {
            self.busy.load(Ordering::Relaxed)
        }

        fn timezone(&self) -> Option<String> {
            Some("Europe/Moscow".into())
        }
    }

    const REPEAT: &str = r#"
        export function brief({ schedule }) { return { say: `Сводка: ${schedule.id}` }; }
        export function broken() { throw new Error("сломалось"); }
        export function both() { return { say: "Сводка готова", notify: "Сводка: всё спокойно" }; }
        export async function ping() { await host.bus.call("system/notify", "send", { text: "Привет из кода" }); return null; }
        export async function add({ text }) {
            const parsed = await host.bus.call("system/schedule", "parse", { text });
            if (!parsed) return "Не поняла, когда.";
            return await host.bus.call("system/schedule", "set", { ...parsed.spec, title: parsed.rest, say: `Напоминаю: ${parsed.rest}` });
        }
    "#;

    /// The gateway's services as far as the schedules go: `host.schedule.*` to the scheduler.
    #[derive(Default)]
    struct Schedules(OnceLock<Weak<Scheduler>>);

    impl crate::plugins::host_api::HostServices for Schedules {
        fn content(&self, _: &str, _: Option<String>, _: usize) -> BoxFuture<'static, Vec<Value>> {
            Box::pin(async { Vec::new() })
        }
        fn smart_home_devices(&self) -> BoxFuture<'static, Result<Value, String>> {
            Box::pin(async { Err("нет".to_string()) })
        }
        fn smart_home_act(&self, _: String, _: String, _: Option<f64>) -> BoxFuture<'static, Result<String, String>> {
            Box::pin(async { Err("нет".to_string()) })
        }
        fn weather_place(&self) -> Option<String> {
            None
        }
        fn news_feeds(&self) -> Vec<Value> {
            Vec::new()
        }
        fn schedule(&self, caller: &str, action: &str, input: Value) -> BoxFuture<'static, Result<Value, String>> {
            let answer = self
                .0
                .get()
                .and_then(Weak::upgrade)
                .ok_or("нет".to_string())
                .and_then(|s| s.code(caller, action, input));
            Box::pin(async move { answer })
        }
        fn notify(&self, caller: &str, text: String) -> BoxFuture<'static, Result<Value, String>> {
            let (scheduler, caller) = (self.0.get().and_then(Weak::upgrade), caller.to_string());
            Box::pin(async move {
                scheduler.ok_or("нет".to_string())?.notify(&caller, &text).await.map(|()| Value::Bool(true))
            })
        }
        /// Host API 2: the code's letters to `system/schedule` and `system/notify`, as the bus would carry them.
        fn bus(
            &self,
            caller: &str,
            name: &str,
            input: Value,
            _: Option<crate::bus::Trail>,
        ) -> Option<BoxFuture<'static, Result<Value, String>>> {
            assert_eq!(name, "bus.call");
            let letter = input["input"].clone();
            Some(match (input["to"].as_str(), input["handle"].as_str()) {
                (Some("system/schedule"), Some(action)) => self.schedule(caller, action, letter),
                (Some("system/notify"), Some("send")) => {
                    self.notify(caller, letter["text"].as_str().unwrap_or_default().to_string())
                }
                (to, _) => {
                    let to = to.unwrap_or_default().to_string();
                    Box::pin(async move { Err(format!("нет такого: {to}")) })
                }
            })
        }
    }

    /// The way to the owner: what it was told, and whether there is a way at all.
    #[derive(Default)]
    struct Owner {
        told: Mutex<Vec<String>>,
        nowhere: AtomicBool,
    }

    impl OwnerNotice for Owner {
        fn ready(&self) -> Result<(), String> {
            if self.nowhere.load(Ordering::Relaxed) {
                Err("некуда прислать".into())
            } else {
                Ok(())
            }
        }
        fn tell(&self, text: String) -> BoxFuture<'static, ()> {
            self.told.lock().unwrap().push(text);
            Box::pin(async {})
        }
    }

    async fn scheduler(dir: &std::path::Path) -> (Arc<Scheduler>, Arc<Station>, Arc<PluginHost>) {
        let (scheduler, station, host, _owner) = with_owner(dir).await;
        (scheduler, station, host)
    }

    async fn with_owner(dir: &std::path::Path) -> (Arc<Scheduler>, Arc<Station>, Arc<PluginHost>, Arc<Owner>) {
        use crate::config::PluginsConfig;
        use crate::core::availability::Placement;
        use crate::core::events::EventBus;
        use crate::plugins::controller::PluginsDeps;
        use crate::plugins::registry::tests::write_package;
        use crate::settings::SettingsService;
        use crate::skills::media::sources::MediaSources;
        let manifest = json!({
            "schema": 1, "id": "repeat", "version": "1.0.0", "title": "Повторы", "api": "^2", "runs": ["pc"],
            "permissions": ["call:system/schedule", "call:system/voice", "call:system/notify"],
            "contributes": { "schedules": [
                { "id": "water", "every": "5m", "from": "00:00", "to": "24:00", "say": "Пора попить воды" },
                { "id": "brief", "at": "07:30", "handler": "brief" },
                { "id": "broken", "at": "08:00", "handler": "broken" },
                { "id": "pills", "at": "09:00", "notify": "Пора принять таблетки" },
                { "id": "both", "at": "10:00", "handler": "both" }
            ] }
        });
        write_package(&dir.join("packages"), &manifest, Some(REPEAT));
        let db = Arc::new(Db::memory());
        let events = EventBus::new();
        let services = Arc::new(Schedules::default());
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
                schedules: true,
            },
            db: db.clone(),
            settings: Arc::new(SettingsService::new(db.clone(), events.clone())),
            events,
            media: MediaSources::new(Vec::new()),
            placement: Arc::new(|| Placement::Pc),
            runtime: Arc::new(crate::plugins::quickjs::QuickJs),
            memory: crate::plugins::budget::fake::plenty(),
            gauge: Default::default(),
            bundled: Vec::new(),
            player: None,
            watch: None,
            home: None,
            gateway_url: "http://127.0.0.1:9".into(),
            services: Some(services.clone()),
            secrets: Arc::new(
                crate::secrets::SecretsService::new(Arc::new(crate::storage::Db::memory()), None).unwrap(),
            ),
        });
        host.enable("repeat", None).await.unwrap();
        let scheduler = Arc::new(Scheduler::new(db, Arc::downgrade(&host), Arc::new(|| true), true));
        let _ = services.0.set(Arc::downgrade(&scheduler));
        let station = Arc::new(Station::default());
        scheduler.set_station(station.clone());
        let owner = Arc::new(Owner::default());
        scheduler.set_notice(owner.clone());
        (scheduler, station, host, owner)
    }

    /// Brings jobs due `ago` ms ago.
    fn due(scheduler: &Scheduler, ids: &[&str], ago: i64) {
        let now = now_ms() as i64;
        let mut jobs = scheduler.jobs("repeat");
        for job in jobs.iter_mut().filter(|job| ids.contains(&job.id.as_str())) {
            job.next_ms = Some(now - ago);
        }
        scheduler.jobs.set("repeat", jobs);
    }

    fn job(scheduler: &Scheduler, id: &str) -> Job {
        scheduler.jobs("repeat").into_iter().find(|job| job.id == id).unwrap()
    }

    #[tokio::test]
    async fn runs_due_rhythms_says_their_words_and_waits_for_a_busy_station() {
        let dir = tempfile::tempdir().unwrap();
        let (scheduler, station, host) = scheduler(dir.path()).await;
        scheduler.tick().await;
        let now = now_ms() as i64;
        assert_eq!(scheduler.jobs("repeat").len(), 5);
        assert!(scheduler.jobs("repeat").iter().all(|job| job.next_ms.is_some_and(|next| next > now)));
        // Due: the ready words are said without the plugin, the function's answer is said.
        due(&scheduler, &["water", "brief"], 1_000);
        scheduler.tick().await;
        assert_eq!(*station.said.lock().unwrap(), ["Пора попить воды", "Сводка: brief"]);
        assert!(job(&scheduler, "brief").last.unwrap().outcome.contains("сказал «Сводка: brief»"));
        assert!(job(&scheduler, "water").next_ms.is_some_and(|next| next > now), "a repeat counts on from now");
        assert!(host
            .journal("repeat")
            .iter()
            .any(|line| line.message.starts_with("brief (по расписанию): ответил за")));
        // The station is busy: the words wait, and are said when it is free.
        station.busy.store(true, Ordering::Relaxed);
        due(&scheduler, &["water"], 1_000);
        scheduler.tick().await;
        assert_eq!(station.said.lock().unwrap().len(), 2);
        assert!(job(&scheduler, "water").pending.is_some());
        station.busy.store(false, Ordering::Relaxed);
        scheduler.tick().await;
        assert_eq!(station.said.lock().unwrap().len(), 3);
        assert!(job(&scheduler, "water").pending.is_none());
        // Off: nothing runs, the jobs stay.
        scheduler.set_on(false);
        due(&scheduler, &["water"], 1_000);
        scheduler.tick().await;
        assert_eq!(station.said.lock().unwrap().len(), 3);
        assert_eq!(scheduler.jobs("repeat").len(), 5);
    }

    #[tokio::test]
    async fn keeps_the_fuses_too_often_late_in_a_row_and_failures_in_a_row() {
        let dir = tempfile::tempdir().unwrap();
        let (scheduler, station, _host) = scheduler(dir.path()).await;
        scheduler.tick().await;
        // Twelve speeches this hour already: the thirteenth is not said.
        let now = now_ms() as i64;
        for _ in 0..SPEECHES_AN_HOUR {
            assert!(scheduler.may_speak("repeat", now));
        }
        due(&scheduler, &["water"], 1_000);
        scheduler.tick().await;
        assert!(station.said.lock().unwrap().is_empty());
        assert!(job(&scheduler, "water").last.unwrap().outcome.ends_with("не сказано: слишком часто"));
        scheduler.spoken.lock().unwrap().clear();
        // After a stop: four onces late, three said, the fourth «пропущено».
        let mut jobs = scheduler.jobs("repeat");
        for index in 0..4 {
            jobs.push(Job {
                id: format!("once-{index}"),
                manifest: false,
                when: When::Once { at_ms: now - 600_000 },
                then: Then { handler: None, say: Some(format!("Разовое {index}")), notify: None },
                next_ms: Some(now - 600_000),
                ..Job::of(&ScheduleSpec {
                    id: String::new(),
                    title: "Разовое".into(),
                    when: When::Once { at_ms: 0 },
                    then: Then { handler: None, say: None, notify: None },
                })
            });
        }
        scheduler.jobs.set("repeat", jobs);
        scheduler.tick().await;
        assert_eq!(*station.said.lock().unwrap(), ["Разовое 0", "Разовое 1", "Разовое 2"]);
        // The onces that went are gone from the list.
        assert!(scheduler.jobs("repeat").iter().all(|job| job.repeats()));
        // A function that fails five times in a row: the job pauses with the reason.
        for _ in 0..FAILURES {
            due(&scheduler, &["broken"], 1_000);
            scheduler.tick().await;
        }
        let broken = job(&scheduler, "broken");
        assert!(
            broken.paused && broken.problem.as_deref().is_some_and(|why| why.contains("5 ошибок подряд")),
            "{broken:?}"
        );
        // A computer that only helps a station runs nothing.
        let helper = Scheduler::new(Arc::new(Db::memory()), Weak::new(), Arc::new(|| false), true);
        helper.tick().await;
        assert!(helper.jobs("repeat").is_empty());
    }

    #[test]
    fn reads_when_in_russian_as_the_reminders_do() {
        let now = at("2026-11-13T08:00:00+03:00");
        let spec = |text: &str| parse(text, now, moscow());
        assert_eq!(
            spec("каждый день в девять пить таблетки"),
            Some(json!({ "spec": { "at": "09:00", "days": "daily" }, "rest": "пить таблетки" }))
        );
        assert_eq!(
            spec("по будням в 7:30 утреннюю сводку"),
            Some(json!({ "spec": { "at": "07:30", "days": "weekdays" }, "rest": "утреннюю сводку" }))
        );
        assert_eq!(spec("каждые 2 часа пить воду"), Some(json!({ "spec": { "every": "120m" }, "rest": "пить воду" })));
        assert_eq!(spec("каждый час размяться"), Some(json!({ "spec": { "every": "60m" }, "rest": "размяться" })));
        assert_eq!(
            spec("через 10 минут выключить чайник"),
            Some(json!({ "spec": { "once": "2026-11-13T08:10" }, "rest": "выключить чайник" }))
        );
        assert_eq!(spec("привет"), None);
    }

    #[tokio::test]
    async fn the_code_sets_lists_and_cancels_its_tasks_within_the_fuses() {
        let dir = tempfile::tempdir().unwrap();
        let (scheduler, station, host) = scheduler(dir.path()).await;
        scheduler.tick().await;
        // The plugin's own code: «через 10 минут выключить чайник» read and set, through host.call.
        let set = host
            .call("repeat", "add", json!({ "text": "через 10 минут выключить чайник" }), CallKind::Tool)
            .await
            .unwrap();
        let id = set["id"].as_str().unwrap().to_string();
        assert!(set["next"].as_str().is_some_and(|next| next.ends_with("+03:00")), "{set}");
        let task = job(&scheduler, &id);
        assert!(!task.manifest && matches!(task.when, When::Once { .. }));
        assert_eq!(task.then.say.as_deref(), Some("Напоминаю: выключить чайник"));
        // Due: said, and gone from the list.
        due(&scheduler, &[id.as_str()], 1_000);
        scheduler.tick().await;
        assert_eq!(*station.said.lock().unwrap(), ["Напоминаю: выключить чайник"]);
        assert!(scheduler.jobs("repeat").iter().all(|job| job.id != id));
        // The fuses: sooner than a minute is the turn timer's; a rhythm of the manifest is a person's to pause.
        let code = |action: &str, input: Value| scheduler.code("repeat", action, input);
        let soon = code("set", json!({ "after": 30, "say": "x" })).unwrap_err();
        assert!(soon.contains("таймер хода"), "{soon}");
        assert!(code("set", json!({ "id": "water", "after": 600, "say": "x" }))
            .unwrap_err()
            .contains("ритм манифеста"));
        assert!(code("cancel", json!({ "id": "water" })).unwrap_err().contains("ритм манифеста"));
        assert!(code("set", json!({ "every": "1m", "say": "x" })).unwrap_err().contains("не чаще раза в 5 минут"));
        // The same id replaces; cancel takes it away.
        code("set", json!({ "id": "vitamins", "at": "09:00", "say": "Таблетки" })).unwrap();
        code("set", json!({ "id": "vitamins", "at": "10:00", "say": "Таблетки" })).unwrap();
        let listed = code("list", json!({})).unwrap();
        let pills: Vec<&Value> = listed.as_array().unwrap().iter().filter(|job| job["id"] == "vitamins").collect();
        assert_eq!((pills.len(), &pills[0]["spec"]["at"], &pills[0]["source"]), (1, &json!("10:00"), &json!("code")));
        assert_eq!(code("cancel", json!({ "id": "vitamins" })).unwrap(), json!(true));
        assert_eq!(code("cancel", json!({ "id": "vitamins" })).unwrap(), json!(false));
        // 50 tasks at most; 60 sets an hour at most.
        for index in 0..MAX_TASKS {
            code("set", json!({ "id": format!("t-{index}"), "after": 3600, "say": "x" })).unwrap();
        }
        assert!(code("set", json!({ "id": "one-more", "after": 3600, "say": "x" }))
            .unwrap_err()
            .contains("больше 50 задач"));
        let mut refused = None;
        for index in 0..SETS_AN_HOUR {
            if let Err(why) = code("set", json!({ "id": format!("t-{index}"), "after": 3600, "say": "x" })) {
                refused = Some(why);
                break;
            }
        }
        assert!(refused.is_some_and(|why| why.contains("больше 60 в час")));
    }

    #[tokio::test]
    async fn sends_the_owner_its_messages_by_the_plugins_name_and_within_the_fuses() {
        let dir = tempfile::tempdir().unwrap();
        let (scheduler, station, host, owner) = with_owner(dir.path()).await;
        scheduler.tick().await;
        // A ready message, and a function's speech with a message.
        due(&scheduler, &["pills", "both"], 1_000);
        scheduler.tick().await;
        assert_eq!(*owner.told.lock().unwrap(), ["Повторы: Пора принять таблетки", "Повторы: Сводка: всё спокойно"]);
        assert_eq!(*station.said.lock().unwrap(), ["Сводка готова"]);
        assert!(job(&scheduler, "both").last.unwrap().outcome.contains("прислал «Сводка: всё спокойно»"));
        // The plugin's own code: host.notify.
        host.call("repeat", "ping", json!({}), CallKind::Tool).await.unwrap();
        assert_eq!(owner.told.lock().unwrap().last().unwrap(), "Повторы: Привет из кода");
        // No way to the owner: said why, nothing sent.
        owner.nowhere.store(true, Ordering::Relaxed);
        due(&scheduler, &["pills"], 1_000);
        scheduler.tick().await;
        assert!(job(&scheduler, "pills").last.unwrap().outcome.contains("не прислано: некуда прислать"));
        owner.nowhere.store(false, Ordering::Relaxed);
        // 20 an hour at most: three went already.
        for _ in 0..17 {
            scheduler.notify("repeat", "ещё").await.unwrap();
        }
        assert!(scheduler.notify("repeat", "ещё").await.unwrap_err().contains("не больше 20 сообщений в час"));
        // A ready message without the right is a problem of the manifest.
        let mut problems = Vec::new();
        let item = json!({ "id": "n", "at": "09:00", "notify": "x" });
        assert!(rhythm(&item, "s", &[right("call:system/schedule")], &mut problems).is_none());
        assert!(problems[0].contains("call:system/notify"), "{problems:?}");
    }
}
